//! Suite de tests du contrat `ChronotopeStore`, PARAMETREE par
//! l'implementation et rejouee a l'identique contre `ChronotopeEngine` et
//! `NaiveRowStore` (voir `docs/conventions.md`, section Tests). Deux suites
//! dupliquees divergeraient en silence et biaiseraient H1 : ce que le banc
//! compare doit etre deux structures de stockage, pas deux garanties.
//!
//! Les ticks utilises restent dans la fenetre T0 du moteur (`ANNEAU` ticks) :
//! au-dela, le moteur recycle son anneau et rejette l'ecriture tardive alors
//! que le temoin a ligne, deliberement non borne en memoire, l'accepterait
//! encore. Ce point precis est couvert cote moteur uniquement
//! (`src/engine.rs`, `ecriture_tardive_hors_anneau_rejetee`).

use std::sync::Arc;
use std::thread;

use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, NaiveRowStore, Pose, RoomId,
    Tick, WriteError,
};
use proptest::prelude::*;

const ROOM: RoomId = RoomId(0);
const CELL: CellId = CellId(3);

fn pose(x: f32) -> Pose {
    Pose {
        pos: [x, 0.0, 0.0],
        yaw: x,
    }
}

fn ecriture_puis_lecture_apres_scellement<S: ChronotopeStore>(store: &S) {
    let tick = Tick(1);
    store
        .ecrire(ROOM, CELL, tick, EntityId(7), pose(1.0))
        .unwrap();
    store
        .ecrire(ROOM, CELL, tick, EntityId(2), pose(2.0))
        .unwrap();

    let scelle = store.sceller(ROOM, CELL, tick);
    assert!(scelle.sealed);
    assert_eq!(
        scelle.entities,
        vec![(EntityId(2), pose(2.0)), (EntityId(7), pose(1.0))]
    );

    let lus = store.lire(ROOM, &[CELL], tick);
    assert_eq!(lus, vec![scelle]);
}

fn ecriture_refusee_apres_scellement<S: ChronotopeStore>(store: &S) {
    let tick = Tick(2);
    store
        .ecrire(ROOM, CELL, tick, EntityId(1), pose(1.0))
        .unwrap();
    store.sceller(ROOM, CELL, tick);

    assert_eq!(
        store.ecrire(ROOM, CELL, tick, EntityId(1), pose(9.0)),
        Err(WriteError::AlreadySealed(ROOM, CELL, tick))
    );
    assert_eq!(
        store.ecrire(ROOM, CELL, tick, EntityId(42), pose(9.0)),
        Err(WriteError::AlreadySealed(ROOM, CELL, tick))
    );
    assert_eq!(
        store.lire(ROOM, &[CELL], tick)[0].entities,
        vec![(EntityId(1), pose(1.0))]
    );
}

fn sceller_est_idempotent<S: ChronotopeStore>(store: &S) {
    let tick = Tick(3);
    store
        .ecrire(ROOM, CELL, tick, EntityId(5), pose(1.0))
        .unwrap();

    let premier = store.sceller(ROOM, CELL, tick);
    for _ in 0..10 {
        assert_eq!(store.sceller(ROOM, CELL, tick), premier);
    }
}

fn lww_sur_double_ecriture_avant_scellement<S: ChronotopeStore>(store: &S) {
    let tick = Tick(4);
    for i in 0..5u32 {
        store
            .ecrire(ROOM, CELL, tick, EntityId(1), pose(i as f32))
            .unwrap();
    }
    let scelle = store.sceller(ROOM, CELL, tick);
    assert_eq!(scelle.entities, vec![(EntityId(1), pose(4.0))]);
}

fn lire_ne_renvoie_jamais_de_non_scelle<S: ChronotopeStore>(store: &S) {
    let tick = Tick(5);
    store
        .ecrire(ROOM, CELL, tick, EntityId(1), pose(1.0))
        .unwrap();
    assert!(store.lire(ROOM, &[CELL], tick).is_empty());

    store.sceller(ROOM, CELL, tick);
    assert_eq!(store.lire(ROOM, &[CELL], tick).len(), 1);
}

fn lire_ignore_les_cellules_absentes<S: ChronotopeStore>(store: &S) {
    let tick = Tick(6);
    store
        .ecrire(ROOM, CellId(10), tick, EntityId(1), pose(1.0))
        .unwrap();
    store.sceller(ROOM, CellId(10), tick);

    let lus = store.lire(ROOM, &[CellId(9), CellId(10), CellId(11)], tick);
    assert_eq!(lus.len(), 1);
    assert_eq!(lus[0].cell, CellId(10));
}

fn sceller_une_cellule_vide_donne_un_chronotope_vide<S: ChronotopeStore>(store: &S) {
    let tick = Tick(7);
    let scelle = store.sceller(ROOM, CellId(20), tick);
    assert!(scelle.sealed);
    assert!(scelle.entities.is_empty());
    assert_eq!(
        store.ecrire(ROOM, CellId(20), tick, EntityId(1), pose(1.0)),
        Err(WriteError::AlreadySealed(ROOM, CellId(20), tick))
    );
}

fn observer_ne_pousse_que_du_scelle<S: ChronotopeStore>(store: &S) {
    let flux = store.observer(ROOM, &[CELL]);
    let tick = Tick(8);

    store
        .ecrire(ROOM, CELL, tick, EntityId(1), pose(1.0))
        .unwrap();
    assert!(flux.try_recv().is_err());

    store.sceller(ROOM, CELL, tick);
    let recu = flux.try_recv().unwrap();
    assert!(recu.sealed);
    assert_eq!(recu.entities, vec![(EntityId(1), pose(1.0))]);

    store.sceller(ROOM, CellId(30), tick);
    assert!(flux.try_recv().is_err());
}

fn observer_borne_le_canal_sans_bloquer<S: ChronotopeStore>(store: &S) {
    let flux = store.observer(ROOM, &[CELL]);
    for tick in 0..2048u64 {
        store.sceller(ROOM, CELL, Tick(tick % 16));
    }
    assert_eq!(flux.len(), 1024);
    while flux.try_recv().is_ok() {}
    store.sceller(ROOM, CELL, Tick(0));
    assert!(flux.try_recv().is_ok());
}

fn ecritures_concurrentes_ne_perdent_aucune_entite<S: ChronotopeStore + 'static>(store: Arc<S>) {
    let tick = Tick(9);
    let fils: Vec<_> = (0..8u32)
        .map(|f| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                for i in 0..50u32 {
                    store
                        .ecrire(ROOM, CELL, tick, EntityId(f * 50 + i), pose(f as f32))
                        .unwrap();
                }
            })
        })
        .collect();
    for fil in fils {
        fil.join().unwrap();
    }

    let scelle = store.sceller(ROOM, CELL, tick);
    assert_eq!(scelle.entities.len(), 400);
    let ids: Vec<u32> = scelle.entities.iter().map(|(e, _)| e.0).collect();
    let mut attendus: Vec<u32> = (0..400).collect();
    attendus.sort_unstable();
    assert_eq!(ids, attendus);
}

fn ecritures_triees_quel_que_soit_l_ordre<S: ChronotopeStore>(store: &S, ordre: &[u32]) {
    let tick = Tick(10);
    for (i, entite) in ordre.iter().enumerate() {
        store
            .ecrire(ROOM, CELL, tick, EntityId(*entite), pose(i as f32))
            .unwrap();
    }
    let scelle = store.sceller(ROOM, CELL, tick);

    let ids: Vec<u32> = scelle.entities.iter().map(|(e, _)| e.0).collect();
    let mut tries = ids.clone();
    tries.sort_unstable();
    tries.dedup();
    assert_eq!(ids, tries, "l'ordre par EntityId n'est pas respecte");

    for (entite, pose_lue) in &scelle.entities {
        let derniere = ordre
            .iter()
            .rposition(|candidat| *candidat == entite.0)
            .unwrap();
        assert_eq!(*pose_lue, pose(derniere as f32), "LWW viole");
    }
}

fn scelle_refuse_tout_quel_que_soit_l_ordre<S: ChronotopeStore>(store: &S, ordre: &[u32]) {
    let tick = Tick(11);
    let coupure = ordre.len() / 2;
    for entite in &ordre[..coupure] {
        store
            .ecrire(ROOM, CELL, tick, EntityId(*entite), pose(1.0))
            .unwrap();
    }
    let scelle = store.sceller(ROOM, CELL, tick);

    for entite in &ordre[coupure..] {
        assert_eq!(
            store.ecrire(ROOM, CELL, tick, EntityId(*entite), pose(2.0)),
            Err(WriteError::AlreadySealed(ROOM, CELL, tick))
        );
    }
    assert_eq!(store.sceller(ROOM, CELL, tick), scelle);
}

macro_rules! contrat_pour {
    ($nom:ident, $ctor:expr) => {
        mod $nom {
            use super::*;

            fn store() -> impl ChronotopeStore + 'static {
                $ctor
            }

            #[test]
            fn ecriture_puis_lecture_apres_scellement() {
                super::ecriture_puis_lecture_apres_scellement(&store());
            }

            #[test]
            fn ecriture_refusee_apres_scellement() {
                super::ecriture_refusee_apres_scellement(&store());
            }

            #[test]
            fn sceller_est_idempotent() {
                super::sceller_est_idempotent(&store());
            }

            #[test]
            fn lww_sur_double_ecriture_avant_scellement() {
                super::lww_sur_double_ecriture_avant_scellement(&store());
            }

            #[test]
            fn lire_ne_renvoie_jamais_de_non_scelle() {
                super::lire_ne_renvoie_jamais_de_non_scelle(&store());
            }

            #[test]
            fn lire_ignore_les_cellules_absentes() {
                super::lire_ignore_les_cellules_absentes(&store());
            }

            #[test]
            fn sceller_une_cellule_vide_donne_un_chronotope_vide() {
                super::sceller_une_cellule_vide_donne_un_chronotope_vide(&store());
            }

            #[test]
            fn observer_ne_pousse_que_du_scelle() {
                super::observer_ne_pousse_que_du_scelle(&store());
            }

            #[test]
            fn observer_borne_le_canal_sans_bloquer() {
                super::observer_borne_le_canal_sans_bloquer(&store());
            }

            #[test]
            fn ecritures_concurrentes_ne_perdent_aucune_entite() {
                super::ecritures_concurrentes_ne_perdent_aucune_entite(Arc::new(store()));
            }

            proptest! {
                #![proptest_config(ProptestConfig::with_cases(64))]

                #[test]
                fn prop_entites_triees_et_lww(ordre in prop::collection::vec(0u32..24, 1..80)) {
                    super::ecritures_triees_quel_que_soit_l_ordre(&store(), &ordre);
                }

                #[test]
                fn prop_scelle_refuse_toute_ecriture(ordre in prop::collection::vec(0u32..24, 2..80)) {
                    super::scelle_refuse_tout_quel_que_soit_l_ordre(&store(), &ordre);
                }
            }
        }
    };
}

contrat_pour!(engine, ChronotopeEngine::new(Horizon::default()));
contrat_pour!(naive, NaiveRowStore::new());

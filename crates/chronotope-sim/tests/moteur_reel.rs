//! H3 et H4 contre le VRAI substrat (`chronotope_core::ChronotopeEngine`).
//!
//! Tous ces tests sont `#[ignore]` tant que `ChronotopeEngine` est un
//! squelette a `todo!()` : ils compilent, ils sont listes, ils paniqueraient a
//! la premiere ecriture. Les de-marquer est la derniere etape de la fusion des
//! branches `feat/core-engine`, `feat/directory` et `feat/sim-harness` — a ce
//! moment-la ils deviennent la mesure qui compte, celle qui porte sur le
//! substrat reel et pas sur un double.
//!
//! Les memes proprietes sont deja verifiees MAINTENANT contre un double de
//! test en memoire, dans les tests unitaires de `src/harness.rs` : ce fichier
//! ne teste donc pas la logique du harnais (elle est acquise), il teste que le
//! moteur reel se comporte comme le double sur les deux clauses de contrat
//! dont le harnais depend — rejet d'ecriture apres sceau, idempotence du
//! scellement.

use chronotope_core::{CellId, ChronotopeStore, EntityId, RoomId, Tick};
use chronotope_sim::{MultiNodeSim, Palier, SimConfig, SimReport};

const CELLULE_A: CellId = CellId(0);
const CELLULE_B: CellId = CellId(1);

fn va_et_vient(t: u64) -> [f32; 3] {
    let phase = t as f32 * std::f32::consts::TAU / 16.0;
    [15.0 + 9.0 * phase.sin(), 0.0, 5.0]
}

fn dans_a(_t: u64) -> [f32; 3] {
    [5.0, 0.0, 5.0]
}

fn dans_b(_t: u64) -> [f32; 3] {
    [15.0, 0.0, 5.0]
}

/// H3 — une entite qui traverse plusieurs fois une frontiere de cellule (donc
/// de noeud) ne doit jamais etre dupliquee, jamais perdue, et sa latence de
/// bascule doit rester bornee par l'hysteresis configuree.
#[test]
#[ignore = "en attente de l'implementation reelle de ChronotopeEngine/Directory, voir feat/core-engine et feat/directory"]
fn h3_franchissements_repetes_sur_le_moteur_reel() {
    let mut sim = MultiNodeSim::avec_config(SimConfig {
        hysteresis_ticks: 3,
        ..SimConfig::default()
    });
    sim.track(EntityId(1), va_et_vient);
    sim.executer(320);
    let r: SimReport = sim.report();

    assert_eq!(r.duplications, 0, "{r:?}");
    assert_eq!(r.duplications_inter_noeuds, 0, "{r:?}");
    assert_eq!(r.pertes, 0, "{r:?}");
    assert_eq!(r.ecritures_rejetees, 0, "{r:?}");
    assert_eq!(r.ecritures_acceptees, 320);
    assert!(!r.latence_bascule_ms.is_empty());
    assert!(r.latence_bascule_max_ms().unwrap_or(0.0) <= 150.0);
}

/// H3, verification sur le CONTENU scelle du moteur reel plutot que sur les
/// compteurs du harnais : a chaque tick scelle, l'entite est presente dans
/// exactement un chronotope, sur exactement un noeud.
#[test]
#[ignore = "en attente de l'implementation reelle de ChronotopeEngine/Directory, voir feat/core-engine et feat/directory"]
fn h3_le_contenu_scelle_du_moteur_reel_ne_duplique_ni_ne_perd() {
    let mut sim = MultiNodeSim::avec_config(SimConfig::default());
    sim.track(EntityId(1), va_et_vient);
    sim.executer(200);

    let room = RoomId(0);
    let cellules: Vec<CellId> = (0..3).map(CellId).collect();
    for tick in 0..198u64 {
        let mut presences = 0;
        for noeud in sim.noeuds() {
            for chrono in noeud.lire(room, &cellules, Tick(tick)) {
                assert!(chrono.sealed, "lire a renvoye du speculatif");
                presences += chrono
                    .entities
                    .iter()
                    .filter(|(e, _)| *e == EntityId(1))
                    .count();
            }
        }
        assert_eq!(presences, 1, "tick {tick} : {presences} inscriptions");
    }
}

/// H4 — la degradation d'une cellule chargee ne doit pas toucher sa voisine
/// calme dans la MEME simulation. C'est la propriete §9 point 1, celle qu'une
/// erreur d'implementation casse le plus silencieusement.
#[test]
#[ignore = "en attente de l'implementation reelle de ChronotopeEngine/Directory, voir feat/core-engine et feat/directory"]
fn h4_localite_de_la_degradation_sur_le_moteur_reel() {
    let mut sim = MultiNodeSim::avec_config(SimConfig::default());
    for i in 0..40 {
        sim.track(EntityId(i), dans_a);
    }
    for i in 100..103 {
        sim.track(EntityId(i), dans_b);
    }
    sim.executer(200);
    let r = sim.report();

    assert_eq!(r.palier_max_de(CELLULE_A), Palier::Quart);
    assert_eq!(r.palier_max_de(CELLULE_B), Palier::Nominal);
    assert!((r.cadence_de(CELLULE_A).unwrap() - 5.0).abs() < 0.6);
    assert!((r.cadence_de(CELLULE_B).unwrap() - 20.0).abs() < 0.6);
    assert_eq!(
        r.ecritures_rejetees, 0,
        "la degradation a refuse des entrees"
    );
    assert_eq!(r.pertes, 0);
}

/// H4 — la cadence doit descendre par paliers 20 -> 10 -> 5 Hz sous charge
/// croissante, jamais s'arreter net a un plafond.
#[test]
#[ignore = "en attente de l'implementation reelle de ChronotopeEngine/Directory, voir feat/core-engine et feat/directory"]
fn h4_paliers_sous_charge_croissante_sur_le_moteur_reel() {
    fn rejoint_a_au_tick_100(t: u64) -> [f32; 3] {
        if t >= 100 {
            [5.0, 0.0, 5.0]
        } else {
            [5.0, 0.0, 55.0]
        }
    }
    fn rejoint_a_au_tick_200(t: u64) -> [f32; 3] {
        if t >= 200 {
            [5.0, 0.0, 5.0]
        } else {
            [5.0, 0.0, 75.0]
        }
    }

    let mut sim = MultiNodeSim::avec_config(SimConfig::default());
    for i in 0..4 {
        sim.track(EntityId(i), dans_a);
    }
    for i in 4..20 {
        sim.track(EntityId(i), rejoint_a_au_tick_100);
    }
    for i in 20..40 {
        sim.track(EntityId(i), rejoint_a_au_tick_200);
    }

    sim.executer(100);
    let r1 = sim.report();
    sim.executer(100);
    let r2 = sim.report();
    sim.executer(100);
    let r3 = sim.report();

    let hz = |avant: &SimReport, apres: &SimReport| {
        f64::from(apres.sceaux_de(CELLULE_A) - avant.sceaux_de(CELLULE_A))
            / ((apres.ticks - avant.ticks) as f64 / 20.0)
    };
    let vide = SimReport::default();
    assert!((hz(&vide, &r1) - 20.0).abs() < 1.0);
    assert!((hz(&r1, &r2) - 10.0).abs() < 1.0);
    assert!((hz(&r2, &r3) - 5.0).abs() < 1.0);
    assert_eq!(r3.ecritures_rejetees, 0);
}

/// Les deux clauses du contrat de `ChronotopeStore` dont le harnais depend
/// pour que ses mesures aient un sens. Si l'une tombe, tous les chiffres
/// ci-dessus deviennent ininterpretables — d'ou un test dedie, separe des
/// scenarios.
#[test]
#[ignore = "en attente de l'implementation reelle de ChronotopeEngine/Directory, voir feat/core-engine et feat/directory"]
fn le_moteur_reel_honore_les_clauses_dont_le_harnais_depend() {
    use chronotope_core::{ChronotopeEngine, Horizon, Pose, WriteError};

    let engine = ChronotopeEngine::new(Horizon::default());
    let pose = Pose {
        pos: [1.0, 2.0, 3.0],
        yaw: 0.0,
    };
    engine
        .ecrire(RoomId(0), CELLULE_A, Tick(0), EntityId(1), pose)
        .expect("premiere ecriture refusee");

    let a = engine.sceller(RoomId(0), CELLULE_A, Tick(0));
    let b = engine.sceller(RoomId(0), CELLULE_A, Tick(0));
    assert_eq!(a, b, "sceller n'est pas idempotent");
    assert!(a.sealed);

    assert_eq!(
        engine.ecrire(RoomId(0), CELLULE_A, Tick(0), EntityId(2), pose),
        Err(WriteError::AlreadySealed(RoomId(0), CELLULE_A, Tick(0))),
        "une ecriture apres sceau a ete acceptee ou fusionnee"
    );
}

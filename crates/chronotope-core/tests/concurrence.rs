//! Tests de resistance concurrente et d'entrees adversariales pour
//! `ChronotopeEngine`, en complement de `contrat.rs` (contrat fonctionnel,
//! mono-thread pour l'essentiel) et `arene.rs` (regime d'allocation).
//!
//! Ce fichier verifie, sous charge multi-thread reelle, les invariants que
//! `docs/chronotope.md` documente mais que les tests unitaires de
//! `engine.rs` n'exercent qu'en sequentiel : aucun chronotope partiel/dechire
//! ne doit jamais etre observable, `sceller` reste idempotent meme couru,
//! l'ordre `EntityId` et la deduplication tiennent sous ecriture concurrente,
//! et aucune combinaison d'entrees ne doit produire de blocage ni de panique
//! hors du contrat documente (cellule/salle hors domaine -> panique
//! deliberee, voir `docs/conventions.md`).

use std::panic::{self, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick, WriteError,
    CELLULES_PAR_SALLE, SALLES_MAX,
};

fn pose(x: f32) -> Pose {
    Pose {
        pos: [x, 0.0, 0.0],
        yaw: x,
    }
}

/// Verifie l'invariant central de `Chronotope::entities` (§5.1 du document
/// source, doc de `lib.rs`) : trie par `EntityId`, sans doublon.
fn verifie_trie_et_uniques(entities: &[(EntityId, Pose)]) {
    for paire in entities.windows(2) {
        assert!(
            paire[0].0 < paire[1].0,
            "entites non triees ou dupliquees : {:?} puis {:?}",
            paire[0].0,
            paire[1].0
        );
    }
}

/// Execute `travail` (qui gere ses propres threads en interne) sous une
/// supervision anti-blocage : si `travail` ne termine pas sous `delai`, la
/// cause la plus probable est un deadlock reel (deux verrous de cellule pris
/// dans un ordre incoherent, par exemple) plutot qu'une simple lenteur —
/// `delai` est choisi tres au-dessus du temps d'execution normal (quelques
/// dizaines de ms) pour ne jamais etre un faux positif en CI. Un panique a
/// l'interieur de `travail` (une assertion violee) est re-propage tel quel,
/// distinct d'un timeout, pour ne jamais confondre les deux diagnostics.
fn avec_garde_anti_blocage<F>(delai: Duration, travail: F)
where
    F: FnOnce() + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let resultat = panic::catch_unwind(AssertUnwindSafe(travail));
        let _ = tx.send(resultat);
    });
    match rx.recv_timeout(delai) {
        Ok(Ok(())) => {}
        Ok(Err(charge)) => panic::resume_unwind(charge),
        Err(_) => panic!("blocage suspecte : le travail n'a pas termine sous {delai:?}"),
    }
}

const DELAI_GARDE: Duration = Duration::from_secs(30);

/// H1 (§1 du document source) vise explicitement le regime agglomere : des
/// dizaines de threads qui ecrivent, scellent, lisent et s'abonnent sur des
/// cellules qui se recouvrent partiellement, sans coordination applicative
/// autre que le moteur lui-meme. Aucune assertion de contenu precis n'est
/// possible ici (l'entrelacement n'est pas deterministe) : ce test verifie
/// seulement l'absence de panique, de blocage, et que tout `Chronotope`
/// produit reste bien forme.
#[test]
fn hammer_multithread_toutes_operations_sans_deadlock_ni_panique() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::avec_dimensions(Horizon::default(), 8, 8));
        const THREADS: u32 = 24;
        const ITERATIONS: u32 = 400;

        let fils: Vec<_> = (0..THREADS)
            .map(|f| {
                let moteur = Arc::clone(&moteur);
                thread::Builder::new()
                    .name(format!("hammer-{f}"))
                    .spawn(move || {
                        for i in 0..ITERATIONS {
                            let room = RoomId(f % 3);
                            let cell = CellId((f + i) % 8);
                            let tick = Tick(u64::from((f * 7 + i * 3) % 64));
                            let entity = EntityId(f * ITERATIONS + i);

                            match i % 5 {
                                0 => {
                                    let _ = moteur.ecrire(room, cell, tick, entity, pose(i as f32));
                                }
                                1 => {
                                    let chrono = moteur.sceller(room, cell, tick);
                                    verifie_trie_et_uniques(&chrono.entities);
                                    assert!(chrono.sealed);
                                }
                                2 => {
                                    let voisine = CellId((cell.0 + 1) % 8);
                                    for lu in moteur.lire(room, &[cell, voisine], tick) {
                                        verifie_trie_et_uniques(&lu.entities);
                                        assert!(lu.sealed);
                                        assert_eq!(lu.tick, tick);
                                    }
                                }
                                3 => {
                                    let flux = moteur.observer(room, &[cell]);
                                    while flux.try_recv().is_ok() {}
                                }
                                _ => {
                                    let _ =
                                        moteur.ecrire(room, cell, tick, entity, pose(-(i as f32)));
                                }
                            }
                        }
                    })
                    .expect("spawn")
            })
            .collect();

        for fil in fils {
            fil.join().expect("un thread hammer a panique");
        }
    });
}

/// Ecritures qui courent explicitement contre un scellement sur EXACTEMENT
/// la meme cle (room, cell, tick) — le cas que §5.2 encadre : "une ecriture
/// arrivant apres le sceau est rejetee, jamais fusionnee". Verifie qu'aucune
/// ecriture reussie n'est silencieusement perdue et qu'aucun rejet ne porte
/// une autre cause que celle documentee.
#[test]
fn ecritures_courent_contre_un_scellement_sans_fusion_tardive() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::new(Horizon::default()));
        let (room, cell, tick) = (RoomId(0), CellId(0), Tick(1));
        const ECRIVAINS: u32 = 16;
        const ECRITURES: u32 = 100;

        let resultats: Arc<Mutex<Vec<Result<(), WriteError>>>> = Arc::default();

        let mut fils = Vec::new();
        for f in 0..ECRIVAINS {
            let moteur = Arc::clone(&moteur);
            let resultats = Arc::clone(&resultats);
            fils.push(thread::spawn(move || {
                for i in 0..ECRITURES {
                    let r = moteur.ecrire(
                        room,
                        cell,
                        tick,
                        EntityId(f * ECRITURES + i),
                        pose(i as f32),
                    );
                    resultats.lock().unwrap().push(r);
                }
            }));
        }
        {
            let moteur = Arc::clone(&moteur);
            fils.push(thread::spawn(move || {
                for _ in 0..50 {
                    moteur.sceller(room, cell, tick);
                }
            }));
        }
        for fil in fils {
            fil.join().expect("un thread a panique");
        }

        let scelle = moteur.sceller(room, cell, tick);
        verifie_trie_et_uniques(&scelle.entities);

        let resultats = resultats.lock().unwrap();
        let reussies = resultats.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            reussies,
            scelle.entities.len(),
            "des ecritures rapportees reussies ne se retrouvent pas dans le chronotope scelle"
        );
        for r in resultats.iter() {
            if let Err(e) = r {
                assert_eq!(*e, WriteError::AlreadySealed(room, cell, tick));
            }
        }
    });
}

/// §5.2 : "le sceau est un fait, pas une negociation". Plusieurs threads
/// appellent `sceller` sur la MEME cle, certains plusieurs fois, en
/// alternance avec des ecritures qui devraient toutes finir rejetees des la
/// premiere prise de sceau. Tous les `Chronotope` retournes doivent etre
/// rigoureusement identiques : l'idempotence ne doit pas etre "vraie en
/// pratique la plupart du temps", elle doit tenir meme couru.
#[test]
fn sceller_idempotent_meme_couru_par_plusieurs_threads() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::new(Horizon::default()));
        let (room, cell, tick) = (RoomId(1), CellId(5), Tick(9));

        for e in 0..30u32 {
            moteur
                .ecrire(room, cell, tick, EntityId(e), pose(e as f32))
                .unwrap();
        }

        let resultats: Arc<Mutex<Vec<_>>> = Arc::default();
        let fils: Vec<_> = (0..12u32)
            .map(|_| {
                let moteur = Arc::clone(&moteur);
                let resultats = Arc::clone(&resultats);
                thread::spawn(move || {
                    for _ in 0..5 {
                        let c = moteur.sceller(room, cell, tick);
                        resultats.lock().unwrap().push(c);
                    }
                })
            })
            .collect();
        for fil in fils {
            fil.join().expect("un thread sceller a panique");
        }

        let resultats = resultats.lock().unwrap();
        let reference = &resultats[0];
        verifie_trie_et_uniques(&reference.entities);
        assert_eq!(reference.entities.len(), 30);
        for c in resultats.iter() {
            assert_eq!(
                c, reference,
                "sceller a renvoye des resultats differents sous course"
            );
        }
    });
}

/// Mode d'echec §5.11.3 : l'anneau recycle un slot en place. Un ecrivain qui
/// avance vite les ticks fait tourner l'anneau plusieurs fois pendant que
/// d'autres threads lisent/scellent des ticks plus anciens qui peuvent avoir
/// deja ete recycles. Le contrat est que `lire`/`sceller` ne renvoient
/// JAMAIS un `Chronotope` dont le tick ne correspond pas a celui demande
/// (jamais de lecture "dechiree" melangeant deux occupants successifs du
/// meme slot) — le mutex par cellule couvre tout l'anneau, donc ceci doit
/// tenir par construction ; ce test le verifie sous charge reelle plutot que
/// de le supposer.
#[test]
fn recyclage_de_l_anneau_sous_charge_ne_produit_jamais_de_lecture_dechiree() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::avec_dimensions(Horizon::default(), 4, 4));
        let (room, cell) = (RoomId(0), CellId(1));
        const TICKS_ECRITS: u64 = 5000;

        let ecrivain = {
            let moteur = Arc::clone(&moteur);
            thread::spawn(move || {
                for tick in 0..TICKS_ECRITS {
                    let _ = moteur.ecrire(room, cell, Tick(tick), EntityId(0), pose(tick as f32));
                }
            })
        };

        let lecteurs: Vec<_> = (0..8u64)
            .map(|f| {
                let moteur = Arc::clone(&moteur);
                thread::spawn(move || {
                    for tick in 0..TICKS_ECRITS {
                        let cible = Tick((tick + f) % TICKS_ECRITS);
                        for lu in moteur.lire(room, &[cell], cible) {
                            assert_eq!(
                                lu.tick, cible,
                                "tick renvoye ne correspond pas au tick demande"
                            );
                            verifie_trie_et_uniques(&lu.entities);
                        }
                        let scelle = moteur.sceller(room, cell, cible);
                        assert_eq!(scelle.tick, cible);
                        verifie_trie_et_uniques(&scelle.entities);
                    }
                })
            })
            .collect();

        ecrivain.join().expect("l'ecrivain a panique");
        for fil in lecteurs {
            fil.join().expect("un lecteur a panique");
        }
    });
}

/// Des abonnes s'inscrivent puis se desinscrivent (le `Receiver` est
/// simplement abandonne, cote appelant) PENDANT que d'autres threads
/// scellent en boucle et donc diffusent. `Diffuseur::diffuser` doit purger
/// les abonnes morts sans jamais paniquer ni bloquer, et un abonnement
/// souscrit APRES le chaos doit continuer a recevoir normalement — la purge
/// des morts ne doit pas casser la diffusion aux vivants.
#[test]
fn abonnes_qui_se_desinscrivent_pendant_la_diffusion_ne_cassent_rien() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::new(Horizon::default()));
        let room = RoomId(2);
        const CELLULES: u32 = 6;

        let scelleurs: Vec<_> = (0..8u32)
            .map(|f| {
                let moteur = Arc::clone(&moteur);
                thread::spawn(move || {
                    for i in 0..300u64 {
                        let cell = CellId((f + i as u32) % CELLULES);
                        moteur.sceller(room, cell, Tick(i % 32));
                    }
                })
            })
            .collect();

        let abonnes: Vec<_> = (0..16u32)
            .map(|f| {
                let moteur = Arc::clone(&moteur);
                thread::spawn(move || {
                    for _ in 0..20 {
                        let flux = moteur.observer(room, &[CellId(f % CELLULES)]);
                        let _ = flux.recv_timeout(Duration::from_millis(5));
                        drop(flux);
                    }
                })
            })
            .collect();

        for fil in scelleurs {
            fil.join().expect("un scelleur a panique");
        }
        for fil in abonnes {
            fil.join().expect("un abonne a panique");
        }

        // Un abonnement souscrit apres coup doit encore recevoir un sceau
        // normalement : la purge des abonnes morts n'a pas laisse le
        // diffuseur dans un etat degrade.
        let flux = moteur.observer(room, &[CellId(0)]);
        moteur.sceller(room, CellId(0), Tick(999));
        assert!(flux.recv_timeout(Duration::from_secs(1)).is_ok());
    });
}

/// §4 : la cardinalite d'ecriture 1 rend le LWW trivialement convergent,
/// mais seulement si le registre ne peut jamais retenir une valeur qui n'a
/// ete ecrite par personne (une pose "dechiree", moitie d'un ecrivain moitie
/// d'un autre). Beaucoup de threads ecrivent des valeurs mutuellement
/// distinguables pour la MEME entite au meme tick ; apres scellement, la
/// pose retenue doit etre EXACTEMENT une des valeurs ecrites, jamais une
/// combinaison.
#[test]
fn lww_sous_course_ne_retient_jamais_une_pose_dechiree() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::new(Horizon::default()));
        let (room, cell) = (RoomId(3), CellId(2));
        const ECRIVAINS: u32 = 20;
        const TOURS: u32 = 25;

        for tour in 0..TOURS {
            let tick = Tick(u64::from(tour));
            let fils: Vec<_> = (0..ECRIVAINS)
                .map(|f| {
                    let moteur = Arc::clone(&moteur);
                    thread::spawn(move || {
                        // Chaque ecrivain grave son identite dans les deux
                        // champs de la pose ; une valeur dechiree (pos[0] d'un
                        // ecrivain, yaw d'un autre) serait detectable.
                        let valeur = f as f32;
                        let p = Pose {
                            pos: [valeur, valeur, valeur],
                            yaw: valeur,
                        };
                        moteur.ecrire(room, cell, tick, EntityId(0), p).unwrap();
                    })
                })
                .collect();
            for fil in fils {
                fil.join().expect("un ecrivain LWW a panique");
            }

            let scelle = moteur.sceller(room, cell, tick);
            assert_eq!(scelle.entities.len(), 1);
            let (_, retenue) = scelle.entities[0];
            let attendu = retenue.pos[0];
            assert!(
                (0.0..ECRIVAINS as f32).contains(&attendu) && attendu.fract() == 0.0,
                "pose retenue hors du domaine des valeurs ecrites : {retenue:?}"
            );
            assert_eq!(
                retenue,
                Pose {
                    pos: [attendu, attendu, attendu],
                    yaw: attendu
                },
                "pose dechiree : champs incoherents entre eux, {retenue:?}"
            );
        }
    });
}

/// Compaction (§4, `SEUIL_COMPACTION`) declenchee plusieurs fois pendant une
/// rafale concurrente qui melange une poignee d'entites tres bavardes
/// (reecrites des centaines de fois - le cas que la compaction doit borner)
/// et beaucoup d'entites distinctes ecrites une seule fois (le regime
/// agglomere que H1 vise). Apres scellement : aucune entite distincte
/// perdue, aucun doublon, les bavardes convergent vers UNE valeur valide.
#[test]
fn compaction_sous_charge_concurrente_ne_perd_ni_ne_duplique() {
    avec_garde_anti_blocage(DELAI_GARDE, || {
        let moteur = Arc::new(ChronotopeEngine::new(Horizon::default()));
        let (room, cell, tick) = (RoomId(4), CellId(9), Tick(42));
        const ENTITES_BAVARDES: u32 = 5;
        const REECRITURES: u32 = 300;
        const ENTITES_UNIQUES: u32 = 2000;

        let mut fils = Vec::new();
        for e in 0..ENTITES_BAVARDES {
            let moteur = Arc::clone(&moteur);
            fils.push(thread::spawn(move || {
                for i in 0..REECRITURES {
                    let _ = moteur.ecrire(room, cell, tick, EntityId(e), pose(i as f32));
                }
            }));
        }
        for lot in 0..4u32 {
            let moteur = Arc::clone(&moteur);
            fils.push(thread::spawn(move || {
                let debut = ENTITES_BAVARDES + lot * (ENTITES_UNIQUES / 4);
                let fin = ENTITES_BAVARDES + (lot + 1) * (ENTITES_UNIQUES / 4);
                for e in debut..fin {
                    moteur
                        .ecrire(room, cell, tick, EntityId(e), pose(e as f32))
                        .unwrap();
                }
            }));
        }
        for fil in fils {
            fil.join().expect("un ecrivain de compaction a panique");
        }

        let scelle = moteur.sceller(room, cell, tick);
        verifie_trie_et_uniques(&scelle.entities);
        assert_eq!(
            scelle.entities.len(),
            (ENTITES_BAVARDES + ENTITES_UNIQUES) as usize,
            "des entites ont ete perdues ou dupliquees pendant la compaction concurrente"
        );
        for (id, p) in &scelle.entities {
            if id.0 >= ENTITES_BAVARDES {
                assert_eq!(*p, pose(id.0 as f32), "entite unique corrompue : {id:?}");
            }
        }
    });
}

/// §6/§7 : `RoomId`/`CellId` hors domaine sont documentes comme un bug
/// appelant qui doit paniquer, jamais corrompre l'etat ni renvoyer un
/// resultat partiel. Verifie les deux bornes ET que l'assertion se declenche
/// AVANT toute prise de verrou (pas de mutex empoisonne, pas d'etat
/// partiellement modifie) en enchainant une operation valide juste apres.
#[test]
fn domaine_hors_bornes_panique_sans_corrompre_le_moteur() {
    let moteur = ChronotopeEngine::new(Horizon::default());

    let resultat = panic::catch_unwind(AssertUnwindSafe(|| {
        moteur.ecrire(
            RoomId(SALLES_MAX),
            CellId(0),
            Tick(0),
            EntityId(0),
            pose(0.0),
        )
    }));
    assert!(resultat.is_err(), "salle hors domaine aurait du paniquer");

    let resultat = panic::catch_unwind(AssertUnwindSafe(|| {
        moteur.ecrire(
            RoomId(0),
            CellId(CELLULES_PAR_SALLE as u32),
            Tick(0),
            EntityId(0),
            pose(0.0),
        )
    }));
    assert!(resultat.is_err(), "cellule hors domaine aurait du paniquer");

    // Le moteur doit rester parfaitement utilisable : parking_lot n'empoisonne
    // pas ses mutex sur panique, et les deux assertions ci-dessus se
    // declenchent avant toute prise de verrou (voir `coordonnees`/`base`).
    moteur
        .ecrire(RoomId(0), CellId(0), Tick(0), EntityId(1), pose(1.0))
        .unwrap();
    let scelle = moteur.sceller(RoomId(0), CellId(0), Tick(0));
    assert_eq!(scelle.entities, vec![(EntityId(1), pose(1.0))]);
}

/// Les bornes VALIDES les plus hautes du domaine (§6) doivent au contraire
/// fonctionner normalement, sans jamais deborder l'arithmetique d'adressage
/// du §5.6 (le risque etant `room * cellules * anneau` pres de `u64::MAX`).
#[test]
fn bornes_hautes_valides_du_domaine_fonctionnent() {
    let moteur = ChronotopeEngine::new(Horizon::default());
    let room = RoomId(SALLES_MAX - 1);
    let cell = CellId(CELLULES_PAR_SALLE as u32 - 1);

    moteur
        .ecrire(room, cell, Tick(0), EntityId(u32::MAX), pose(1.0))
        .unwrap();
    let scelle = moteur.sceller(room, cell, Tick(0));
    assert_eq!(scelle.entities, vec![(EntityId(u32::MAX), pose(1.0))]);
}

/// `Tick(u64::MAX)` doit s'ecrire, se sceller et se lire sans deborder
/// l'arithmetique de l'anneau (`tick.0 % anneau`), meme si un tick aussi
/// extreme n'a aucun sens operationnel (§5.7, compteur logique par salle).
#[test]
fn tick_u64_max_ne_deborde_pas() {
    let moteur = ChronotopeEngine::new(Horizon::default());
    let (room, cell) = (RoomId(0), CellId(0));

    moteur
        .ecrire(room, cell, Tick(u64::MAX), EntityId(1), pose(7.0))
        .unwrap();
    let scelle = moteur.sceller(room, cell, Tick(u64::MAX));
    assert_eq!(scelle.entities, vec![(EntityId(1), pose(7.0))]);
    assert_eq!(moteur.lire(room, &[cell], Tick(u64::MAX)), vec![scelle]);
}

/// Sceller un tick "ancien" — dont le slot de l'anneau a deja ete recycle
/// par une ecriture bien plus recente — ne doit ni paniquer ni fusionner
/// quoi que ce soit : §5.11.3 documente cette retention bornee pour
/// `ecrire`/`lire`, ce test verifie que `sceller` suit la meme regle plutot
/// que de re-scelle une donnee qui n'est plus la bonne.
#[test]
fn sceller_un_tick_recycle_par_une_ecriture_plus_recente_ne_panique_pas() {
    let moteur = ChronotopeEngine::avec_dimensions(Horizon::default(), 16, 4);
    let (room, cell) = (RoomId(0), CellId(1));

    moteur
        .ecrire(room, cell, Tick(0), EntityId(1), pose(1.0))
        .unwrap();
    moteur
        .ecrire(room, cell, Tick(100), EntityId(2), pose(2.0))
        .unwrap();

    let scelle = moteur.sceller(room, cell, Tick(0));
    assert!(scelle.sealed);
    assert!(
        scelle.entities.is_empty(),
        "sceller un tick deja recycle ne doit jamais renvoyer les entites de l'occupant courant"
    );
}

/// Ecrire sur un tick tres loin dans le futur (largement au-dela de la
/// fenetre retenue) doit etre accepte comme une ecriture normale — rien dans
/// le contrat ne borne le tick a venir, seule la retention en arriere (§5.5)
/// est bornee.
#[test]
fn ecriture_tres_loin_dans_le_futur_est_acceptee() {
    let moteur = ChronotopeEngine::avec_dimensions(Horizon::default(), 16, 4);
    let (room, cell) = (RoomId(0), CellId(2));
    let futur = Tick(1_000_000_000);

    moteur
        .ecrire(room, cell, futur, EntityId(1), pose(3.0))
        .unwrap();
    let scelle = moteur.sceller(room, cell, futur);
    assert_eq!(scelle.entities, vec![(EntityId(1), pose(3.0))]);
}

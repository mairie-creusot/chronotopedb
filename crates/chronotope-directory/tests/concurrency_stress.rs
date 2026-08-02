//! Stress de concurrence au-dela du test 4 threads deja present
//! (`annuaire_partageable_entre_threads`, `src/directory.rs`) : plus de
//! threads (16 a 64 au total), charge MELANGEE (`inscrire`/`locate`/
//! `migrer`/`instantane` simultanes) a la fois sur un petit ensemble
//! d'entites partagees (pour maximiser la contention) et sur des entites
//! disjointes (pour verifier l'absence de blocage pathologique quand il n'y
//! a pas de contention reelle). Verifie :
//! - `instantane()` ne renvoie jamais un instantane incoherent (entite
//!   dupliquee, ou emplacement hors du domaine possible) ;
//! - pas d'interblocage — verification par timeout explicite plutot que par
//!   simple absence de panique (un test qui pend est un echec silencieux en
//!   CI).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, NodeId};

const ENTITES_PARTAGEES: u32 = 8;
const NOEUDS: u32 = 4;
const CELLULES: u32 = 16;

fn destination_valide(n: u32, c: u32) -> Location {
    Location {
        node: NodeId(n % NOEUDS),
        cell: CellId(c % CELLULES),
    }
}

/// Execute `travail` sur un thread dedie et fait echouer le test s'il ne
/// termine pas dans `budget`. Sans cela, un interblocage se traduit par un
/// hang silencieux plutot que par un echec exploitable.
fn avec_timeout(budget: Duration, travail: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        travail();
        let _ = tx.send(());
    });
    rx.recv_timeout(budget)
        .expect("le workload n'a pas termine dans le budget — interblocage suspecte");
}

#[test]
fn charge_melangee_forte_contention_pas_d_interblocage_pas_de_snapshot_incoherent() {
    avec_timeout(Duration::from_secs(30), || {
        let annuaire = Arc::new(Directory::avec_hysteresis(Hysteresis::ticks(2)));
        for e in 0..ENTITES_PARTAGEES {
            annuaire.inscrire(EntityId(e), destination_valide(e, e), Tick(0));
        }

        let arret = Arc::new(AtomicBool::new(false));
        let mut ecrivains = Vec::new();
        let mut lecteurs = Vec::new();

        // Ecrivains en forte contention : tous tapent sur les MEMES entites.
        for w in 0..16u32 {
            let annuaire = Arc::clone(&annuaire);
            ecrivains.push(std::thread::spawn(move || {
                for t in 0..2_000u64 {
                    let e = EntityId(t as u32 % ENTITES_PARTAGEES);
                    let dest = destination_valide(w + t as u32, t as u32);
                    let _ = annuaire.migrer(e, dest, Tick(t));
                }
            }));
        }

        // Ecrivains sur entites disjointes : pas de contention reelle entre
        // eux, sert a verifier que le RwLock n'introduit pas de blocage
        // global inutile.
        for w in 0..16u32 {
            let annuaire = Arc::clone(&annuaire);
            ecrivains.push(std::thread::spawn(move || {
                let base = 1_000 + w * 1_000;
                for t in 0..500u64 {
                    let e = EntityId(base + t as u32);
                    annuaire.inscrire(e, destination_valide(w, t as u32), Tick(t));
                    let dest = destination_valide(w, t as u32 + 1);
                    let _ = annuaire.migrer(e, dest, Tick(t + 1));
                }
            }));
        }

        // Lecteurs locate() : boucle tant que les ecrivains tournent.
        for _ in 0..16u32 {
            let annuaire = Arc::clone(&annuaire);
            let arret = Arc::clone(&arret);
            lecteurs.push(std::thread::spawn(move || {
                while !arret.load(Ordering::Relaxed) {
                    for e in 0..ENTITES_PARTAGEES {
                        if let Some(loc) = annuaire.locate(EntityId(e)) {
                            assert!(loc.node.0 < NOEUDS);
                            assert!(loc.cell.0 < CELLULES);
                        }
                    }
                }
            }));
        }

        // Lecteurs instantane() : verifient l'absence de duplication a
        // chaque coup d'oeil, pendant que les ecrivains mutent l'annuaire.
        for _ in 0..8u32 {
            let annuaire = Arc::clone(&annuaire);
            let arret = Arc::clone(&arret);
            lecteurs.push(std::thread::spawn(move || {
                while !arret.load(Ordering::Relaxed) {
                    let vue = annuaire.instantane();
                    let mut ids: Vec<u32> = vue.iter().map(|(e, _)| e.0).collect();
                    ids.sort_unstable();
                    let avant = ids.len();
                    ids.dedup();
                    assert_eq!(
                        avant,
                        ids.len(),
                        "instantane a renvoye une entite en double"
                    );
                    for (_, loc) in &vue {
                        assert!(loc.node.0 < NOEUDS);
                        assert!(loc.cell.0 < CELLULES);
                    }
                }
            }));
        }

        for f in ecrivains {
            f.join().unwrap();
        }
        arret.store(true, Ordering::Relaxed);
        for f in lecteurs {
            f.join().unwrap();
        }

        assert_eq!(
            annuaire.len(),
            ENTITES_PARTAGEES as usize + 16 * 500,
            "aucune entite ne doit avoir ete perdue ni fabriquee par la contention"
        );
    });
}

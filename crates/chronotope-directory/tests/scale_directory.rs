//! Correction (pas cout — voir `migration_o1.rs` pour le O(1) de `migrer`)
//! a grande echelle : au-dela des 50 000 entites deja couvertes par
//! `migration_o1::locate_reste_correct_a_grande_echelle`, verifie que
//! `instantane()` et `locate()` restent exacts a 150 000 entites — aucune
//! perdue, aucune dupliquee, aucun emplacement fantome.
//!
//! Note (hors perimetre de ce test) : `Directory` n'expose aucune methode
//! `retirer` — la `HashMap` sous-jacente ne peut donc que croitre, meme si
//! des entites quittent logiquement le monde simule. Ce n'est pas un defaut
//! de ce test, c'est un vrai trou de couverture a signaler.

use std::collections::HashSet;

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};

const N: u32 = 150_000;
const NOEUDS: u32 = 8;
const CELLULES: u32 = 4_096;

#[test]
fn instantane_et_locate_restent_corrects_a_150k_entites() {
    let annuaire = Directory::avec_hysteresis(Hysteresis::aucune());
    for i in 0..N {
        annuaire.inscrire(
            EntityId(i),
            Location {
                node: NodeId(i % NOEUDS),
                cell: CellId(i % CELLULES),
            },
            Tick(0),
        );
    }
    assert_eq!(annuaire.len(), N as usize);

    // Migre une entite sur deux ; les autres doivent rester intactes.
    for i in (0..N).step_by(2) {
        let dest = Location {
            node: NodeId((i + 1) % NOEUDS),
            cell: CellId((i + 1) % CELLULES),
        };
        assert!(matches!(
            annuaire.migrer(EntityId(i), dest, Tick(1)),
            Ok(Migration::Effectuee { .. })
        ));
    }

    let vue = annuaire.instantane();
    assert_eq!(
        vue.len(),
        N as usize,
        "instantane ne doit ni perdre ni dupliquer d'entite"
    );
    let distinctes: HashSet<_> = vue.iter().map(|(e, _)| *e).collect();
    assert_eq!(distinctes.len(), N as usize);

    for i in 0..N {
        let attendu = if i % 2 == 0 {
            Location {
                node: NodeId((i + 1) % NOEUDS),
                cell: CellId((i + 1) % CELLULES),
            }
        } else {
            Location {
                node: NodeId(i % NOEUDS),
                cell: CellId(i % CELLULES),
            }
        };
        assert_eq!(annuaire.locate(EntityId(i)), Some(attendu));
    }
}

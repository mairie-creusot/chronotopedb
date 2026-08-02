//! Preuve empirique que `migrer` est O(1) (§5.3 : "le cout de bascule est une
//! entree de table de routage").
//!
//! Le pire cas vise est precis : toutes les entites deja enregistrees sont
//! dans la cellule DESTINATION. Une implementation qui maintiendrait un index
//! cellule -> entites, ou qui recopierait quoi que ce soit a la bascule,
//! verrait son cout croitre avec cette occupation. Le test compare donc le
//! cout moyen d'une migration a 64 entites et a 64 000 entites (facteur
//! 1000) : un cout lineaire ferait exploser le rapport de ~1000x, un cout
//! constant le laisse dans le bruit de mesure (caches, hachage).
//!
//! Le seuil est volontairement large — il ne s'agit pas de mesurer une
//! performance absolue (c'est le role des benches criterion de
//! `chronotope-core`) mais de distinguer O(1) de O(n), ce qui ne demande pas
//! de precision.

use std::time::{Duration, Instant};

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};

const SOURCE: Location = Location {
    node: NodeId(1),
    cell: CellId(10),
};
const DESTINATION: Location = Location {
    node: NodeId(2),
    cell: CellId(11),
};

/// Cout moyen d'une migration quand `occupation` entites sont deja inscrites
/// dans la cellule destination. Le minimum sur plusieurs lots absorbe les
/// interruptions de l'ordonnanceur, qui n'ajoutent que du bruit positif.
fn cout_moyen_migration(occupation: u32, lots: u32, migrations_par_lot: u32) -> Duration {
    let annuaire = Directory::avec_hysteresis(Hysteresis::aucune());
    let migrante = EntityId(u32::MAX);

    for i in 0..occupation {
        annuaire.inscrire(EntityId(i), DESTINATION, Tick(0));
    }
    annuaire.inscrire(migrante, SOURCE, Tick(0));

    let mut meilleur = Duration::MAX;
    let mut tick = 1u64;
    for _ in 0..lots {
        let debut = Instant::now();
        for i in 0..migrations_par_lot {
            let dest = if i % 2 == 0 { DESTINATION } else { SOURCE };
            let issue = annuaire.migrer(migrante, dest, Tick(tick));
            assert!(matches!(issue, Ok(Migration::Effectuee { .. })));
            tick += 1;
        }
        meilleur = meilleur.min(debut.elapsed() / migrations_par_lot);
    }
    meilleur
}

#[test]
fn migrer_est_o1_independamment_de_l_occupation_de_la_destination() {
    // Un tour a blanc : premier acces aux pages, chauffage de l'allocateur.
    let _ = cout_moyen_migration(64, 2, 1_000);

    let petit = cout_moyen_migration(64, 8, 20_000);
    let grand = cout_moyen_migration(64_000, 8, 20_000);

    let rapport = grand.as_secs_f64() / petit.as_secs_f64().max(f64::MIN_POSITIVE);
    println!(
        "migrer : {petit:?}/op a 64 entites, {grand:?}/op a 64 000 entites \
         (occupation x1000) — rapport {rapport:.2}"
    );

    assert!(
        rapport < 25.0,
        "cout de migration proportionnel a l'occupation : {petit:?} -> {grand:?} \
         (rapport {rapport:.2} pour une occupation multipliee par 1000)"
    );
}

#[test]
fn locate_reste_correct_a_grande_echelle() {
    let annuaire = Directory::new();
    for i in 0..50_000u32 {
        annuaire.inscrire(
            EntityId(i),
            Location {
                node: NodeId(i % 4),
                cell: CellId(i % 128),
            },
            Tick(0),
        );
    }
    assert_eq!(annuaire.len(), 50_000);

    assert_eq!(
        annuaire.migrer(EntityId(1234), DESTINATION, Tick(100)),
        Ok(Migration::Effectuee {
            depuis: Some(Location {
                node: NodeId(1234 % 4),
                cell: CellId(1234 % 128),
            })
        })
    );
    assert_eq!(annuaire.locate(EntityId(1234)), Some(DESTINATION));
    // La migration n'a touche personne d'autre.
    assert_eq!(
        annuaire.locate(EntityId(1235)),
        Some(Location {
            node: NodeId(1235 % 4),
            cell: CellId(1235 % 128),
        })
    );
    assert_eq!(annuaire.len(), 50_000);
}

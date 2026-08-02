//! Invariants de l'annuaire sur une sequence quelconque de migrations
//! (`docs/conventions.md`, "Tests") : une migration ne duplique jamais une
//! entite et n'en perd jamais.
//!
//! "Ne duplique pas" a un sens precis ici : l'annuaire est la seule source de
//! verite du routage (§5.6), donc une entite doit y avoir exactement UN
//! emplacement a tout instant — deux emplacements signifieraient deux noeuds
//! s'estimant autoritaires, soit la duplication de pose que H3 doit exclure.

use std::collections::{HashMap, HashSet};

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};
use proptest::prelude::*;

/// (entite, noeud, cellule, avance du tick depuis l'operation precedente)
type Operation = (u32, u32, u32, u64);

fn operations() -> impl Strategy<Value = Vec<Operation>> {
    prop::collection::vec((0u32..12, 0u32..4, 0u32..8, 0u64..6), 1..300)
}

proptest! {
    #[test]
    fn une_entite_a_toujours_exactement_un_emplacement(
        ops in operations(),
        hysteresis in 0u64..12,
    ) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(hysteresis));
        let mut connues: HashSet<EntityId> = HashSet::new();
        let mut tick = 0u64;

        for (e, n, c, avance) in ops {
            tick += avance;
            let entite = EntityId(e);
            let dest = Location { node: NodeId(n), cell: CellId(c) };

            let avant = annuaire.locate(entite);
            let issue = annuaire.migrer(entite, dest, Tick(tick));
            connues.insert(entite);

            match issue {
                Ok(Migration::Effectuee { depuis }) => {
                    prop_assert_eq!(depuis, avant);
                    prop_assert_eq!(annuaire.locate(entite), Some(dest));
                }
                Ok(Migration::Inchangee) => {
                    prop_assert_eq!(avant, Some(dest));
                    prop_assert_eq!(annuaire.locate(entite), Some(dest));
                }
                Err(refus) => {
                    // Un refus ne modifie rien — ni perte, ni deplacement
                    // partiel.
                    prop_assert_eq!(refus.entite, entite);
                    prop_assert_eq!(refus.dest, dest);
                    prop_assert_eq!(annuaire.locate(entite), avant);
                }
            }

            // Aucune entite connue n'a disparu, aucune n'est apparue deux fois.
            let vue = annuaire.instantane();
            prop_assert_eq!(vue.len(), connues.len());
            prop_assert_eq!(annuaire.len(), connues.len());
            let distinctes: HashSet<EntityId> = vue.iter().map(|(e, _)| *e).collect();
            prop_assert_eq!(distinctes.len(), vue.len());
            for entite in &connues {
                prop_assert!(annuaire.locate(*entite).is_some());
            }
        }
    }

    /// L'annuaire ne peut jamais designer un emplacement que personne n'a
    /// demande pour cette entite : l'hysteresis retarde des migrations, elle
    /// n'en invente pas.
    #[test]
    fn l_emplacement_est_toujours_un_emplacement_demande(
        ops in operations(),
        hysteresis in 0u64..12,
    ) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(hysteresis));
        let mut demandes: HashMap<EntityId, HashSet<(u32, u32)>> = HashMap::new();
        let mut tick = 0u64;

        for (e, n, c, avance) in ops {
            tick += avance;
            let entite = EntityId(e);
            let dest = Location { node: NodeId(n), cell: CellId(c) };
            let _ = annuaire.migrer(entite, dest, Tick(tick));
            demandes.entry(entite).or_default().insert((n, c));
        }

        for (entite, loc) in annuaire.instantane() {
            let demandes_de_l_entite = &demandes[&entite];
            prop_assert!(demandes_de_l_entite.contains(&(loc.node.0, loc.cell.0)));
        }
    }

    /// Sans hysteresis, l'annuaire suit exactement la derniere intention —
    /// c'est le comportement de reference que l'hysteresis degrade
    /// volontairement (et que le test H3 utilise comme temoin).
    #[test]
    fn sans_hysteresis_l_annuaire_suit_la_derniere_intention(ops in operations()) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::aucune());
        let mut attendu: HashMap<EntityId, Location> = HashMap::new();
        let mut tick = 0u64;

        for (e, n, c, avance) in ops {
            tick += avance;
            let entite = EntityId(e);
            let dest = Location { node: NodeId(n), cell: CellId(c) };
            prop_assert!(annuaire.migrer(entite, dest, Tick(tick)).is_ok());
            attendu.insert(entite, dest);
        }

        for (entite, loc) in attendu {
            prop_assert_eq!(annuaire.locate(entite), Some(loc));
        }
    }

    /// Le nombre de migrations acceptees pour une entite est borne par le
    /// nombre de ticks divise par l'hysteresis — la garantie formelle
    /// derriere le test H3, verifiee ici sur des sequences arbitraires.
    #[test]
    fn les_migrations_acceptees_sont_bornees_par_l_hysteresis(
        ops in prop::collection::vec((0u32..3, 0u64..3), 1..400),
        hysteresis in 1u64..10,
    ) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(hysteresis));
        let entite = EntityId(1);
        annuaire.inscrire(entite, Location { node: NodeId(0), cell: CellId(0) }, Tick(0));

        let mut tick = 0u64;
        let mut acceptees = 0u64;
        for (c, avance) in ops {
            tick += avance;
            let dest = Location { node: NodeId(c % 2), cell: CellId(c) };
            if let Ok(Migration::Effectuee { .. }) = annuaire.migrer(entite, dest, Tick(tick)) {
                acceptees += 1;
            }
        }

        // La premiere migration est gratuite (aucune precedente a espacer),
        // les suivantes exigent chacune `hysteresis` ticks ecoules.
        prop_assert!(acceptees <= 1 + tick / hysteresis, "acceptees = {}, ticks = {}", acceptees, tick);
    }
}

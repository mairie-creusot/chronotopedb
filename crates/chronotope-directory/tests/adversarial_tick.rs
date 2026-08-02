//! Va plus loin que "ne panique pas" sur un tick non monotone (voir
//! `tick_non_monotone_ne_panique_pas` et
//! `tick_regressif_est_refuse_de_facon_correcte_pas_seulement_non_panique`
//! dans `src/directory.rs`) : prouve, sur un grand espace de sequences de
//! ticks arbitraires (regressifs, repetes, aux bornes de u64), que
//! l'hysteresis reste CORRECTE — pas seulement silencieuse.

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};
use proptest::prelude::*;

fn loc(node: u32, cell: u32) -> Location {
    Location {
        node: NodeId(node),
        cell: CellId(cell),
    }
}

proptest! {
    /// Meme si les ticks fournis par l'appelant ne sont pas monotones, deux
    /// migrations EFFECTUEES consecutives (au-dela de la premiere, gratuite
    /// car aucune precedente a espacer) sont toujours separees d'au moins
    /// `hysteresis` ticks. Un rejeu de tick ancien ne doit jamais faire
    /// passer une migration en avance sur son delai.
    #[test]
    fn aucun_tick_ne_permet_de_contourner_l_hysteresis(
        ticks in prop::collection::vec(0u64..50, 1..300),
        hysteresis in 1u64..10,
    ) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(hysteresis));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));

        let mut dernier_accepte: Option<u64> = None;
        for (i, &t) in ticks.iter().enumerate() {
            let dest = if i % 2 == 0 { b } else { a };
            let avant = annuaire.locate(EntityId(1));
            match annuaire.migrer(EntityId(1), dest, Tick(t)) {
                Ok(Migration::Effectuee { .. }) => {
                    if let Some(precedent) = dernier_accepte {
                        prop_assert!(
                            t >= precedent + hysteresis,
                            "migration acceptee au tick {t} alors que la precedente \
                             acceptee etait au tick {precedent} (hysteresis = {hysteresis})"
                        );
                    }
                    dernier_accepte = Some(t);
                }
                Ok(Migration::Inchangee) => {
                    prop_assert_eq!(avant, Some(dest));
                }
                Err(refus) => {
                    // Un refus est un no-op total : ni l'emplacement ni
                    // l'horloge (verifiee indirectement au tour suivant)
                    // ne bougent.
                    prop_assert_eq!(annuaire.locate(EntityId(1)), avant);
                    prop_assert_eq!(refus.dest, dest);
                }
            }
        }
    }

    /// Une sequence qui alterne des sauts en avant et en arriere autour de
    /// u64::MAX ne doit jamais paniquer (debordement arithmetique) ni violer
    /// l'invariant d'espacement ci-dessus.
    #[test]
    fn ticks_aux_bornes_de_u64_restent_corrects(
        deltas in prop::collection::vec(-20i64..20, 1..100),
        hysteresis in 1u64..8,
    ) {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(hysteresis));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(u64::MAX - 1000));

        let mut tick: u64 = u64::MAX - 1000;
        let mut dernier_accepte: Option<u64> = None;
        for (i, d) in deltas.iter().enumerate() {
            tick = if *d >= 0 {
                tick.saturating_add(*d as u64)
            } else {
                tick.saturating_sub((-*d) as u64)
            };
            let dest = if i % 2 == 0 { b } else { a };
            if let Ok(Migration::Effectuee { .. }) = annuaire.migrer(EntityId(1), dest, Tick(tick)) {
                if let Some(precedent) = dernier_accepte {
                    prop_assert!(tick >= precedent.saturating_add(hysteresis));
                }
                dernier_accepte = Some(tick);
            }
        }
    }
}

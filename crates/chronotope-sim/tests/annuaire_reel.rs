//! H3 contre le VRAI annuaire (`chronotope_directory::Directory`).
//!
//! Ce fichier est compile UNIQUEMENT sous la feature `annuaire-reel`
//! (desactivee par defaut) : `cargo test -p chronotope-sim --features
//! annuaire-reel`.

#![cfg(feature = "annuaire-reel")]

use chronotope_core::{ChronotopeEngine, EntityId, Horizon};
use chronotope_directory::Directory;
use chronotope_sim::{MultiNodeSim, SimConfig};

fn va_et_vient(t: u64) -> [f32; 3] {
    let phase = t as f32 * std::f32::consts::TAU / 16.0;
    [15.0 + 9.0 * phase.sin(), 0.0, 5.0]
}

/// Meme scenario que `moteur_reel::h3_franchissements_repetes_sur_le_moteur_reel`,
/// mais avec le vrai annuaire dans la boucle : c'est lui qui doit fournir
/// l'hysteresis, et donc lui qui decide de la latence de bascule mesuree ici.
#[test]
fn h3_franchissements_repetes_avec_le_vrai_annuaire() {
    let config = SimConfig::default();
    let noeuds: Vec<ChronotopeEngine> = (0..config.nb_noeuds)
        .map(|_| ChronotopeEngine::new(Horizon::default()))
        .collect();
    let mut sim = MultiNodeSim::avec(config, noeuds, Directory::new());
    sim.track(EntityId(1), va_et_vient);
    sim.executer(320);
    let r = sim.report();

    assert_eq!(r.duplications, 0, "{r:?}");
    assert_eq!(r.duplications_inter_noeuds, 0, "{r:?}");
    assert_eq!(r.pertes, 0, "{r:?}");
    assert_eq!(r.ecritures_rejetees, 0, "{r:?}");
    assert!(
        !r.latence_bascule_ms.is_empty(),
        "aucune bascule : l'hysteresis du Directory bloque tout"
    );
    assert!(
        r.migrations_accordees < r.migrations_tentees,
        "l'annuaire n'oppose aucune hysteresis — thrashing non amorti (§4)"
    );
}

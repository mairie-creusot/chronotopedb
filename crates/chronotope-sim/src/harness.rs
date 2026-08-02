use chronotope_core::{ChronotopeEngine, EntityId};
use chronotope_directory::Directory;

/// Resultat d'une simulation — voir le module doc de `lib.rs` pour ce que
/// chaque champ doit mesurer honnetement (pas juste "0 crash").
#[derive(Debug, Clone, Default)]
pub struct SimReport {
    pub duplications: u32,
    pub pertes: u32,
    pub latence_bascule_ms: Vec<f64>,
    pub cadence_effective_hz_par_cellule: Vec<(chronotope_core::CellId, f64)>,
}

/// Simulation multi-noeud en memoire. IMPLEMENTATION A FAIRE ICI — voir le
/// module doc de `lib.rs` pour H3/H4 et les criteres a mesurer.
pub struct MultiNodeSim {
    _directory: Directory,
    _nodes: Vec<ChronotopeEngine>,
}

impl MultiNodeSim {
    pub fn new(_node_count: usize) -> Self {
        todo!("MultiNodeSim::new")
    }

    /// Fait avancer la simulation d'un tick pour toutes les entites suivies,
    /// en appliquant leurs trajectoires programmees (voir les tests
    /// d'integration pour des trajectoires concretes — ex. longer une
    /// frontiere de cellule en va-et-vient pour stresser l'hysteresis).
    pub fn step(&mut self) {
        todo!("MultiNodeSim::step")
    }

    pub fn track(&mut self, _entity: EntityId, _trajectory: fn(u64) -> [f32; 3]) {
        todo!("MultiNodeSim::track")
    }

    pub fn report(&self) -> SimReport {
        todo!("MultiNodeSim::report")
    }
}

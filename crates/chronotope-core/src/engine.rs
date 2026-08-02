//! Le substrat reel — unite de stockage cellule-tick (§5.1), horizon de
//! scellement (§5.2), arithmetique d'adressage (§5.6), hierarchie T0
//! DRAM-seule pour ce prototype (§5.5, T1/CXL degrade proprement en
//! "T1 = DRAM du meme noeud, plus petit" selon le document source).
//!
//! IMPLEMENTATION A FAIRE ICI. Points imposes par le contrat (voir
//! `docs/chronotope.md` et `store.rs`) :
//!
//! - Adressage arithmetique O(1), pas d'index spatial (§5.6) :
//!   `adresse(room, cell, tick) = base[room] ⊕ (cell × ANNEAU + tick mod ANNEAU)`.
//! - Ecriture = ajout sequentiel contigu, jamais mutation a adresse aleatoire
//!   (§5.1) — c'est la propriete que H1 mesure contre `NaiveRowStore`.
//! - Mode d'echec §5.11.3 a anticiper des la conception : sceller a haute
//!   frequence sur des centaines de cellules produit un flux d'objets
//!   immuables — prevoir une allocation par arene + recyclage d'anneau
//!   plutot que des allocations individuelles par chronotope scelle.
//! - `Horizon` (voir lib.rs) doit rester un parametre runtime, jamais une
//!   constante compilee en dur : H2 en depend directement.

use crossbeam_channel::Receiver;

use crate::{store::ChronotopeStore, Chronotope, CellId, EntityId, Horizon, Pose, RoomId, Tick, WriteError};

pub struct ChronotopeEngine {
    _horizon: Horizon,
}

impl ChronotopeEngine {
    pub fn new(horizon: Horizon) -> Self {
        Self { _horizon: horizon }
    }
}

impl ChronotopeStore for ChronotopeEngine {
    fn ecrire(
        &self,
        _room: RoomId,
        _cell: CellId,
        _tick: Tick,
        _entity: EntityId,
        _pose: Pose,
    ) -> Result<(), WriteError> {
        todo!("ChronotopeEngine::ecrire — voir docs/chronotope.md §5.1-5.2")
    }

    fn sceller(&self, _room: RoomId, _cell: CellId, _tick: Tick) -> Chronotope {
        todo!("ChronotopeEngine::sceller — idempotent, voir §5.2")
    }

    fn lire(&self, _room: RoomId, _cells: &[CellId], _tick: Tick) -> Vec<Chronotope> {
        todo!("ChronotopeEngine::lire — uniquement des chronotopes scelles")
    }

    fn observer(&self, _room: RoomId, _cells: &[CellId]) -> Receiver<Chronotope> {
        todo!("ChronotopeEngine::observer — pousse au scellement uniquement")
    }
}

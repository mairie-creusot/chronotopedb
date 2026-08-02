//! Temoin de comparaison pour H1 : un `HashMap<(Tick, EntityId), Pose>`
//! derriere un verrou, une ecriture = une mutation en place a adresse
//! aleatoire. C'est le modele implicite de PostgreSQL/Redis/toute base a
//! ligne que le document source disqualifie en §5.1 — DOIT rester
//! deliberement naif, ne pas l'optimiser au-dela d'un verrou raisonnable, sinon
//! la comparaison H1 perd son sens (elle mesure "cellule-tick contre ligne",
//! pas "l'implementation la plus habile possible des deux cotes").
//!
//! Respecte EXACTEMENT le meme contrat `ChronotopeStore` — y compris le
//! scellement et le refus d'ecriture apres coup — pour que la comparaison de
//! benchmark soit honnete (memes garanties, seule la structure de stockage
//! differe).

use crossbeam_channel::Receiver;

use crate::{store::ChronotopeStore, Chronotope, CellId, EntityId, Pose, RoomId, Tick, WriteError};

pub struct NaiveRowStore;

impl NaiveRowStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NaiveRowStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ChronotopeStore for NaiveRowStore {
    fn ecrire(
        &self,
        _room: RoomId,
        _cell: CellId,
        _tick: Tick,
        _entity: EntityId,
        _pose: Pose,
    ) -> Result<(), WriteError> {
        todo!("NaiveRowStore::ecrire — une ligne par (tick, entity), verrou global ou par shard simple, deliberement pas optimise")
    }

    fn sceller(&self, _room: RoomId, _cell: CellId, _tick: Tick) -> Chronotope {
        todo!("NaiveRowStore::sceller")
    }

    fn lire(&self, _room: RoomId, _cells: &[CellId], _tick: Tick) -> Vec<Chronotope> {
        todo!("NaiveRowStore::lire")
    }

    fn observer(&self, _room: RoomId, _cells: &[CellId]) -> Receiver<Chronotope> {
        todo!("NaiveRowStore::observer")
    }
}

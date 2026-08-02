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
//!
//! Un seul ecart avec le modele "une ligne, un point" : un index secondaire
//! `(room, cell, tick) -> [entites]`, maintenu a l'insertion d'une ligne
//! NEUVE uniquement. Sans lui, `sceller` devrait balayer toute la table et le
//! banc mesurerait un balayage quadratique plutot que le cout d'ecriture par
//! ligne — toute base a ligne reelle a cet index, et son entretien (une
//! amplification d'ecriture) fait partie du cout honnete du modele.

use std::collections::{HashMap, HashSet};

use crossbeam_channel::Receiver;
use parking_lot::Mutex;

use crate::diffusion::Diffuseur;
use crate::{store::ChronotopeStore, CellId, Chronotope, EntityId, Pose, RoomId, Tick, WriteError};

type Cle = (RoomId, CellId, Tick);
type Ligne = (RoomId, CellId, Tick, EntityId);

#[derive(Default)]
struct Table {
    lignes: HashMap<Ligne, Pose>,
    index: HashMap<Cle, Vec<EntityId>>,
    scelles: HashSet<Cle>,
}

pub struct NaiveRowStore {
    table: Mutex<Table>,
    diffuseur: Diffuseur,
}

impl NaiveRowStore {
    pub fn new() -> Self {
        Self {
            table: Mutex::new(Table::default()),
            diffuseur: Diffuseur::default(),
        }
    }

    /// Remet le store a vide EN CONSERVANT les allocations des tables. Pendant
    /// de `ChronotopeEngine::vider`, pour que les deux cotes du banc H1
    /// rejouent des trames dans les memes conditions d'allocateur.
    pub fn vider(&self) {
        let mut table = self.table.lock();
        table.lignes.clear();
        table.index.clear();
        table.scelles.clear();
    }

    fn assembler(table: &Table, cle: Cle) -> Vec<(EntityId, Pose)> {
        let Some(entites) = table.index.get(&cle) else {
            return Vec::new();
        };
        let mut assemble: Vec<(EntityId, Pose)> = entites
            .iter()
            .filter_map(|entite| {
                table
                    .lignes
                    .get(&(cle.0, cle.1, cle.2, *entite))
                    .map(|pose| (*entite, *pose))
            })
            .collect();
        assemble.sort_unstable_by_key(|(entite, _)| *entite);
        assemble
    }
}

impl Default for NaiveRowStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ChronotopeStore for NaiveRowStore {
    #[tracing::instrument(level = "trace", skip_all, fields(room = room.0, cell = cell.0, tick = tick.0, entity = entity.0))]
    fn ecrire(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        entity: EntityId,
        pose: Pose,
    ) -> Result<(), WriteError> {
        let cle = (room, cell, tick);
        let mut table = self.table.lock();
        if table.scelles.contains(&cle) {
            return Err(WriteError::AlreadySealed(room, cell, tick));
        }
        if table
            .lignes
            .insert((room, cell, tick, entity), pose)
            .is_none()
        {
            table.index.entry(cle).or_default().push(entity);
        }
        Ok(())
    }

    #[tracing::instrument(level = "debug", skip_all, fields(room = room.0, cell = cell.0, tick = tick.0))]
    fn sceller(&self, room: RoomId, cell: CellId, tick: Tick) -> Chronotope {
        let cle = (room, cell, tick);
        let entities = {
            let mut table = self.table.lock();
            table.scelles.insert(cle);
            Self::assembler(&table, cle)
        };

        let chrono = Chronotope {
            room,
            cell,
            tick,
            sealed: true,
            entities,
        };
        tracing::debug!(entites = chrono.entities.len(), "chronotope scelle");
        self.diffuseur.diffuser(&chrono);
        chrono
    }

    #[tracing::instrument(level = "trace", skip_all, fields(room = room.0, cells = cells.len(), tick = tick.0))]
    fn lire(&self, room: RoomId, cells: &[CellId], tick: Tick) -> Vec<Chronotope> {
        let table = self.table.lock();
        cells
            .iter()
            .filter(|cell| table.scelles.contains(&(room, **cell, tick)))
            .map(|cell| Chronotope {
                room,
                cell: *cell,
                tick,
                sealed: true,
                entities: Self::assembler(&table, (room, *cell, tick)),
            })
            .collect()
    }

    #[tracing::instrument(level = "debug", skip_all, fields(room = room.0, cells = cells.len()))]
    fn observer(&self, room: RoomId, cells: &[CellId]) -> Receiver<Chronotope> {
        self.diffuseur.abonner(room, cells)
    }
}

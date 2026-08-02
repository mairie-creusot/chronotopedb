//! Double de test : un `ChronotopeStore` en memoire, volontairement naif.
//!
//! Il existe pour une seule raison : valider la logique DU HARNAIS (choix de
//! cellule, migration, quantification temporelle, comptage des artefacts)
//! independamment de l'avancement de `ChronotopeEngine`. Il ne mesure rien de
//! H1, ne pretend a aucune performance, et n'est pas une seconde
//! implementation du substrat — les tests qui portent sur le vrai moteur sont
//! dans `tests/`, marques `#[ignore]`.
//!
//! Il respecte en revanche exactement les deux clauses du contrat dont le
//! harnais depend pour ses mesures : une ecriture apres scellement est
//! REJETEE (jamais fusionnee), et `sceller` est idempotent.

use std::collections::HashMap;
use std::sync::Mutex;

use chronotope_core::{
    CellId, Chronotope, ChronotopeStore, EntityId, Pose, RoomId, Tick, WriteError,
};
use crossbeam_channel::{bounded, Receiver, Sender};

type Cle = (RoomId, CellId, Tick);

#[derive(Default)]
struct Etat {
    ouverts: HashMap<Cle, Vec<(EntityId, Pose)>>,
    scelles: HashMap<Cle, Chronotope>,
    abonnes: Vec<(RoomId, Vec<CellId>, Sender<Chronotope>)>,
}

pub struct StoreMemoire {
    etat: Mutex<Etat>,
}

impl StoreMemoire {
    pub fn new() -> Self {
        Self {
            etat: Mutex::new(Etat::default()),
        }
    }

    /// Tous les chronotopes scelles de ce store. Sert aux assertions qui
    /// verifient l'absence de duplication/perte SUR LE CONTENU REEL du
    /// stockage, pas seulement sur les compteurs que le harnais tient
    /// lui-meme — deux sources independantes valent mieux qu'une.
    pub fn scelles(&self) -> Vec<Chronotope> {
        let etat = self.etat.lock().expect("mutex du double empoisonne");
        let mut v: Vec<Chronotope> = etat.scelles.values().cloned().collect();
        v.sort_by_key(|c| (c.room.0, c.tick.0, c.cell.0));
        v
    }
}

impl Default for StoreMemoire {
    fn default() -> Self {
        Self::new()
    }
}

impl ChronotopeStore for StoreMemoire {
    fn ecrire(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        entity: EntityId,
        pose: Pose,
    ) -> Result<(), WriteError> {
        let mut etat = self.etat.lock().expect("mutex du double empoisonne");
        let cle = (room, cell, tick);
        if etat.scelles.contains_key(&cle) {
            return Err(WriteError::AlreadySealed(room, cell, tick));
        }
        let entites = etat.ouverts.entry(cle).or_default();
        match entites.binary_search_by_key(&entity.0, |(e, _)| e.0) {
            Ok(i) => entites[i].1 = pose,
            Err(i) => entites.insert(i, (entity, pose)),
        }
        Ok(())
    }

    fn sceller(&self, room: RoomId, cell: CellId, tick: Tick) -> Chronotope {
        let mut etat = self.etat.lock().expect("mutex du double empoisonne");
        let cle = (room, cell, tick);
        if let Some(c) = etat.scelles.get(&cle) {
            return c.clone();
        }
        let entities = etat.ouverts.remove(&cle).unwrap_or_default();
        let chrono = Chronotope {
            room,
            cell,
            tick,
            sealed: true,
            entities,
        };
        etat.scelles.insert(cle, chrono.clone());
        for (r, cells, tx) in &etat.abonnes {
            if *r == room && cells.contains(&cell) {
                let _ = tx.try_send(chrono.clone());
            }
        }
        chrono
    }

    fn lire(&self, room: RoomId, cells: &[CellId], tick: Tick) -> Vec<Chronotope> {
        let etat = self.etat.lock().expect("mutex du double empoisonne");
        cells
            .iter()
            .filter_map(|c| etat.scelles.get(&(room, *c, tick)).cloned())
            .collect()
    }

    /// Canal borne ; un abonne trop lent perd des sceaux (`try_send` ignore),
    /// jamais la memoire du store qui grossit. Documente ici parce que le
    /// contrat l'exige, meme d'un double.
    fn observer(&self, room: RoomId, cells: &[CellId]) -> Receiver<Chronotope> {
        let (tx, rx) = bounded(1024);
        let mut etat = self.etat.lock().expect("mutex du double empoisonne");
        etat.abonnes.push((room, cells.to_vec(), tx));
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32) -> Pose {
        Pose {
            pos: [x, 0.0, 0.0],
            yaw: 0.0,
        }
    }

    #[test]
    fn une_ecriture_apres_sceau_est_rejetee() {
        let s = StoreMemoire::new();
        s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(1), pose(1.0))
            .unwrap();
        s.sceller(RoomId(0), CellId(0), Tick(0));
        assert_eq!(
            s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(2), pose(2.0)),
            Err(WriteError::AlreadySealed(RoomId(0), CellId(0), Tick(0)))
        );
    }

    #[test]
    fn sceller_est_idempotent() {
        let s = StoreMemoire::new();
        s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(1), pose(1.0))
            .unwrap();
        let a = s.sceller(RoomId(0), CellId(0), Tick(0));
        let b = s.sceller(RoomId(0), CellId(0), Tick(0));
        assert_eq!(a, b);
        assert_eq!(a.entities.len(), 1);
    }

    #[test]
    fn les_entites_restent_triees_et_la_derniere_ecriture_gagne() {
        let s = StoreMemoire::new();
        for e in [7u32, 3, 9, 1] {
            s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(e), pose(e as f32))
                .unwrap();
        }
        s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(3), pose(42.0))
            .unwrap();
        let c = s.sceller(RoomId(0), CellId(0), Tick(0));
        let ids: Vec<u32> = c.entities.iter().map(|(e, _)| e.0).collect();
        assert_eq!(ids, vec![1, 3, 7, 9]);
        assert_eq!(c.entities[1].1, pose(42.0));
    }

    #[test]
    fn lire_ne_renvoie_que_du_scelle() {
        let s = StoreMemoire::new();
        s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(1), pose(1.0))
            .unwrap();
        assert!(s.lire(RoomId(0), &[CellId(0)], Tick(0)).is_empty());
        s.sceller(RoomId(0), CellId(0), Tick(0));
        assert_eq!(s.lire(RoomId(0), &[CellId(0)], Tick(0)).len(), 1);
    }

    #[test]
    fn observer_pousse_au_scellement_uniquement() {
        let s = StoreMemoire::new();
        let rx = s.observer(RoomId(0), &[CellId(0)]);
        s.ecrire(RoomId(0), CellId(0), Tick(0), EntityId(1), pose(1.0))
            .unwrap();
        assert!(rx.try_recv().is_err());
        s.sceller(RoomId(0), CellId(0), Tick(0));
        assert!(rx.try_recv().is_ok());
    }
}

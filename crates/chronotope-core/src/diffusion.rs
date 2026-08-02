//! Registre d'abonnements de `observer`, partage par les DEUX implementations
//! de `ChronotopeStore`.
//!
//! Il est partage deliberement : si `ChronotopeEngine` et `NaiveRowStore`
//! avaient chacun leur diffusion, H1 (`benches/h1_frame_vs_row.rs`) mesurerait
//! aussi l'ecart entre deux diffusions, pas seulement l'ecart entre
//! cellule-tick et ligne.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use parking_lot::RwLock;

use crate::{CellId, Chronotope, RoomId};

/// Borne du canal d'observation. Un abonne trop lent perd des sceaux (voir
/// [`Diffuseur::diffuser`]) plutot que de faire grossir la memoire du store
/// sans limite — contrat impose par `ChronotopeStore::observer`.
pub(crate) const CAPACITE_CANAL: usize = 1024;

struct Abonnement {
    room: RoomId,
    cells: Vec<CellId>,
    tx: Sender<Chronotope>,
    mort: AtomicBool,
}

#[derive(Default)]
pub(crate) struct Diffuseur {
    abonnes: RwLock<Vec<Abonnement>>,
    actifs: AtomicUsize,
}

impl Diffuseur {
    pub(crate) fn abonner(&self, room: RoomId, cells: &[CellId]) -> Receiver<Chronotope> {
        let (tx, rx) = bounded(CAPACITE_CANAL);
        let mut cells = cells.to_vec();
        cells.sort_unstable();
        cells.dedup();

        let mut abonnes = self.abonnes.write();
        abonnes.push(Abonnement {
            room,
            cells,
            tx,
            mort: AtomicBool::new(false),
        });
        self.actifs.store(abonnes.len(), Ordering::Release);
        rx
    }

    /// Pousse un chronotope SCELLE aux abonnes concernes. Canal plein : le
    /// sceau est abandonne pour cet abonne (`warn`), jamais mis en attente.
    /// Recepteur ferme : l'abonnement est purge au prochain passage.
    pub(crate) fn diffuser(&self, chrono: &Chronotope) {
        debug_assert!(chrono.sealed);
        if self.actifs.load(Ordering::Acquire) == 0 {
            return;
        }

        let mut a_purger = false;
        {
            let abonnes = self.abonnes.read();
            for abonne in abonnes.iter() {
                if abonne.room != chrono.room
                    || abonne.mort.load(Ordering::Relaxed)
                    || abonne.cells.binary_search(&chrono.cell).is_err()
                {
                    continue;
                }
                match abonne.tx.try_send(chrono.clone()) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => tracing::warn!(
                        room = chrono.room.0,
                        cell = chrono.cell.0,
                        tick = chrono.tick.0,
                        capacite = CAPACITE_CANAL,
                        "canal d'observation plein — sceau abandonne pour cet abonne"
                    ),
                    Err(TrySendError::Disconnected(_)) => {
                        abonne.mort.store(true, Ordering::Relaxed);
                        a_purger = true;
                    }
                }
            }
        }

        if a_purger {
            let mut abonnes = self.abonnes.write();
            abonnes.retain(|a| !a.mort.load(Ordering::Relaxed));
            self.actifs.store(abonnes.len(), Ordering::Release);
        }
    }
}

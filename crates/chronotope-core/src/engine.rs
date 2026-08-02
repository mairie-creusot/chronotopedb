//! Le substrat reel — unite de stockage cellule-tick (§5.1), horizon de
//! scellement (§5.2), arithmetique d'adressage (§5.6), hierarchie T0
//! DRAM-seule pour ce prototype (§5.5, T1/CXL degrade proprement en
//! "T1 = DRAM du meme noeud, plus petit" selon le document source).
//!
//! ## Structure retenue
//!
//! Une salle = un tableau de cellules ; une cellule = un ANNEAU de `anneau`
//! slots indexes par `tick mod anneau`. Le slot d'un couple (cellule, tick)
//! est donc trouve par arithmetique pure (voir [`ChronotopeEngine::adresse`]),
//! sans index spatial ni table de hachage sur le chemin chaud.
//!
//! Chaque slot porte un JOURNAL d'ajout sequentiel : `ecrire` pousse en queue,
//! sans jamais chercher ni muter une entree existante. La deduplication
//! dernier-ecrivain-gagne (§4, cardinalite d'ecriture 1) est appliquee une
//! seule fois, au scellement, ce qui rend l'ecriture O(1) reelle et non
//! "O(1) amorti apres recherche".
//!
//! ## Arene et recyclage d'anneau (mode d'echec §5.11.3)
//!
//! L'anneau d'une cellule est alloue en UN bloc a la premiere ecriture dans
//! cette cellule, puis n'est plus jamais desalloue : quand le tick avance
//! au-dela de `anneau`, le slot est recycle en place (`Vec::clear`, capacite
//! conservee). En regime etabli, ecrire et sceller n'allouent plus rien
//! cote moteur — seul le `Chronotope` RENDU a l'appelant est alloue, parce
//! que le contrat public le veut proprietaire.
//!
//! Consequence assumee : la retention est bornee a `anneau` ticks (T0). Une
//! ecriture ou une lecture visant un tick sorti de la fenetre est traitee
//! comme une arrivee tardive — rejetee, jamais fusionnee.

use crossbeam_channel::Receiver;
use parking_lot::{Mutex, RwLock};

use crate::diffusion::Diffuseur;
use crate::{
    store::ChronotopeStore, CellId, Chronotope, EntityId, Horizon, Pose, RoomId, Tick, WriteError,
};

/// Nombre de ticks retenus par cellule (T0). Puissance de deux : l'adressage
/// du §5.6 suppose que `cell * ANNEAU + tick mod ANNEAU` ne deborde pas d'un
/// champ de bits, donc que `⊕` et `+` coincident.
pub const ANNEAU: u64 = 64;

/// Domaine de cellules par salle. Une cellule hors domaine est un bug
/// appelant (`panic`), pas un etat metier — voir `docs/conventions.md`.
pub const CELLULES_PAR_SALLE: u64 = 4096;

/// Domaine de salles. Meme regle que les cellules.
pub const SALLES_MAX: u32 = 65_536;

const SEUIL_COMPACTION: usize = 256;

type AnneauCellule = Option<Box<[Slot]>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Etat {
    Vide,
    Ouvert,
    Scelle,
}

struct Slot {
    tick: u64,
    etat: Etat,
    journal: Vec<(EntityId, Pose)>,
    /// Entrees poussees depuis la derniere compaction. Sert de declencheur a
    /// la place de `journal.len()` : une fois `journal.len()` au-dessus de
    /// `SEUIL_COMPACTION`, un journal sans doublons (beaucoup d'entites
    /// distinctes dans une meme cellule, le regime agglomere que §1 vise
    /// justement a rendre bon marche) ne raccourcit jamais sous compaction —
    /// re-verifier `journal.len()` a chaque ecriture y redeclenche alors une
    /// compaction complete a CHAQUE appel, donc un cout quadratique en le
    /// nombre d'entites. Compter depuis la derniere compaction borne la
    /// frequence des passes independamment du taux de doublons.
    depuis_compaction: usize,
}

impl Slot {
    const fn vide() -> Self {
        Slot {
            tick: 0,
            etat: Etat::Vide,
            journal: Vec::new(),
            depuis_compaction: 0,
        }
    }

    fn recycler(&mut self, tick: u64) {
        self.journal.clear();
        self.tick = tick;
        self.etat = Etat::Ouvert;
        self.depuis_compaction = 0;
    }

    fn inscrire(&mut self, entity: EntityId, pose: Pose) {
        self.journal.push((entity, pose));
        self.depuis_compaction += 1;
    }

    /// Applique le LWW du §4 : trie par `EntityId` en conservant, pour chaque
    /// entite, la DERNIERE ecriture arrivee. L'inversion prealable transforme
    /// "garder le premier d'une serie egale" (semantique de `dedup_by_key`) en
    /// "garder le dernier ecrivain".
    fn compacter(&mut self) {
        self.journal.reverse();
        self.journal.sort_by_key(|(entite, _)| *entite);
        self.journal.dedup_by_key(|(entite, _)| *entite);
        self.depuis_compaction = 0;
    }
}

struct Salle {
    cellules: Box<[Mutex<AnneauCellule>]>,
}

/// Le substrat cellule-tick. `Horizon` reste un parametre d'execution (H2 en
/// depend) : le moteur ne scelle jamais de lui-meme, c'est l'horloge appelante
/// qui decide, et elle derive son retard de [`ChronotopeEngine::horizon`].
pub struct ChronotopeEngine {
    horizon: Horizon,
    cellules: u64,
    anneau: u64,
    salles: RwLock<Vec<Option<Box<Salle>>>>,
    diffuseur: Diffuseur,
}

impl ChronotopeEngine {
    /// Moteur aux dimensions par defaut ([`CELLULES_PAR_SALLE`], [`ANNEAU`]).
    pub fn new(horizon: Horizon) -> Self {
        Self::avec_dimensions(horizon, CELLULES_PAR_SALLE, ANNEAU)
    }

    /// Variante pour les tests et la simulation : `cellules` et `anneau`
    /// doivent etre des puissances de deux non nulles, faute de quoi
    /// l'adressage du §5.6 cesse d'etre injectif.
    pub fn avec_dimensions(horizon: Horizon, cellules: u64, anneau: u64) -> Self {
        assert!(
            cellules.is_power_of_two() && anneau.is_power_of_two(),
            "cellules ({cellules}) et anneau ({anneau}) doivent etre des puissances de deux"
        );
        Self {
            horizon,
            cellules,
            anneau,
            salles: RwLock::new(Vec::new()),
            diffuseur: Diffuseur::default(),
        }
    }

    /// L'horizon Δ configure. Le moteur ne l'applique pas lui-meme : il
    /// l'expose pour que la boucle d'horloge (cote `chronotope-sim` ou
    /// `chronotope-server`) en derive son retard de scellement au lieu d'une
    /// constante compilee.
    pub fn horizon(&self) -> Horizon {
        self.horizon
    }

    /// Nombre de ticks retenus par cellule (fenetre T0).
    pub fn ticks_retenus(&self) -> u64 {
        self.anneau
    }

    /// `adresse(room, cell, tick) = base[room] ⊕ (cell × ANNEAU + tick mod ANNEAU)`
    /// (§5.6). O(1), sans maintenance, sans reequilibrage : `base[room]` est un
    /// multiple de `cellules × anneau` (puissance de deux), donc `⊕` se comporte
    /// ici comme une concatenation de champs de bits, et l'adresse est injective
    /// sur `(room, cell, tick mod anneau)`.
    pub fn adresse(&self, room: RoomId, cell: CellId, tick: Tick) -> u64 {
        self.base(room) ^ (u64::from(cell.0) * self.anneau + tick.0 % self.anneau)
    }

    /// Remet le store a vide EN CONSERVANT les allocations d'anneaux. Sert aux
    /// bancs d'essai (`benches/h1_frame_vs_row.rs`), qui doivent rejouer des
    /// trames sans mesurer la montee en charge de l'allocateur.
    pub fn vider(&self) {
        let salles = self.salles.read();
        for salle in salles.iter().flatten() {
            for cellule in salle.cellules.iter() {
                let mut anneau = cellule.lock();
                if let Some(slots) = anneau.as_mut() {
                    for slot in slots.iter_mut() {
                        slot.etat = Etat::Vide;
                        slot.tick = 0;
                        slot.journal.clear();
                    }
                }
            }
        }
    }

    fn base(&self, room: RoomId) -> u64 {
        assert!(
            room.0 < SALLES_MAX,
            "salle {} hors domaine (max {SALLES_MAX})",
            room.0
        );
        u64::from(room.0) * self.cellules * self.anneau
    }

    fn coordonnees(&self, room: RoomId, cell: CellId, tick: Tick) -> (usize, usize) {
        assert!(
            u64::from(cell.0) < self.cellules,
            "cellule {} hors domaine (max {})",
            cell.0,
            self.cellules
        );
        let locale = self.adresse(room, cell, tick) ^ self.base(room);
        (
            (locale / self.anneau) as usize,
            (locale % self.anneau) as usize,
        )
    }

    fn nouvel_anneau(anneau: u64) -> Box<[Slot]> {
        (0..anneau)
            .map(|_| Slot::vide())
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }

    fn avec_slot<R>(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        f: impl FnOnce(&mut Slot) -> R,
    ) -> R {
        let (i_cellule, i_slot) = self.coordonnees(room, cell, tick);
        let index_salle = room.0 as usize;

        loop {
            {
                let salles = self.salles.read();
                if let Some(Some(salle)) = salles.get(index_salle) {
                    let mut anneau = salle.cellules[i_cellule].lock();
                    let slots = anneau.get_or_insert_with(|| Self::nouvel_anneau(self.anneau));
                    return f(&mut slots[i_slot]);
                }
            }
            self.creer_salle(index_salle);
        }
    }

    fn avec_slot_existant<R>(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        f: impl FnOnce(&Slot) -> R,
    ) -> Option<R> {
        let (i_cellule, i_slot) = self.coordonnees(room, cell, tick);
        let index_salle = room.0 as usize;

        let salles = self.salles.read();
        let salle = salles.get(index_salle)?.as_ref()?;
        let anneau = salle.cellules[i_cellule].lock();
        let slots = anneau.as_ref()?;
        Some(f(&slots[i_slot]))
    }

    fn creer_salle(&self, index: usize) {
        let mut salles = self.salles.write();
        if salles.len() <= index {
            salles.resize_with(index + 1, || None);
        }
        if salles[index].is_none() {
            let cellules = (0..self.cellules)
                .map(|_| Mutex::new(None))
                .collect::<Vec<_>>()
                .into_boxed_slice();
            salles[index] = Some(Box::new(Salle { cellules }));
            tracing::debug!(salle = index, cellules = self.cellules, "salle allouee");
        }
    }
}

impl ChronotopeStore for ChronotopeEngine {
    #[tracing::instrument(level = "trace", skip_all, fields(room = room.0, cell = cell.0, tick = tick.0, entity = entity.0))]
    fn ecrire(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        entity: EntityId,
        pose: Pose,
    ) -> Result<(), WriteError> {
        self.avec_slot(room, cell, tick, |slot| match slot.etat {
            Etat::Vide => {
                slot.recycler(tick.0);
                slot.inscrire(entity, pose);
                Ok(())
            }
            _ if slot.tick < tick.0 => {
                slot.recycler(tick.0);
                slot.inscrire(entity, pose);
                Ok(())
            }
            _ if slot.tick > tick.0 || slot.etat == Etat::Scelle => {
                Err(WriteError::AlreadySealed(room, cell, tick))
            }
            _ => {
                if slot.depuis_compaction >= SEUIL_COMPACTION {
                    slot.compacter();
                }
                slot.inscrire(entity, pose);
                Ok(())
            }
        })
    }

    #[tracing::instrument(level = "debug", skip_all, fields(room = room.0, cell = cell.0, tick = tick.0))]
    fn sceller(&self, room: RoomId, cell: CellId, tick: Tick) -> Chronotope {
        let entities = self.avec_slot(room, cell, tick, |slot| {
            if slot.etat != Etat::Vide && slot.tick > tick.0 {
                return Vec::new();
            }
            if slot.etat == Etat::Vide || slot.tick < tick.0 {
                slot.recycler(tick.0);
            }
            if slot.etat != Etat::Scelle {
                slot.compacter();
                slot.etat = Etat::Scelle;
            }
            slot.journal.clone()
        });

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
        let mut lus = Vec::with_capacity(cells.len());
        for &cell in cells {
            let entities = self.avec_slot_existant(room, cell, tick, |slot| {
                if slot.etat == Etat::Scelle && slot.tick == tick.0 {
                    Some(slot.journal.clone())
                } else {
                    None
                }
            });
            if let Some(Some(entities)) = entities {
                lus.push(Chronotope {
                    room,
                    cell,
                    tick,
                    sealed: true,
                    entities,
                });
            }
        }
        lus
    }

    #[tracing::instrument(level = "debug", skip_all, fields(room = room.0, cells = cells.len()))]
    fn observer(&self, room: RoomId, cells: &[CellId]) -> Receiver<Chronotope> {
        self.diffuseur.abonner(room, cells)
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
    fn adresse_injective_dans_la_fenetre() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let mut vues = std::collections::HashSet::new();
        for room in 0..3u32 {
            for cell in 0..64u32 {
                for tick in 0..ANNEAU {
                    let a = moteur.adresse(RoomId(room), CellId(cell), Tick(tick));
                    assert!(
                        vues.insert(a),
                        "collision d'adresse room={room} cell={cell}"
                    );
                }
            }
        }
    }

    #[test]
    fn adresse_recycle_l_anneau() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let a = moteur.adresse(RoomId(1), CellId(7), Tick(3));
        let b = moteur.adresse(RoomId(1), CellId(7), Tick(3 + ANNEAU));
        assert_eq!(a, b);
    }

    #[test]
    fn ecriture_tardive_hors_anneau_rejetee() {
        let moteur = ChronotopeEngine::avec_dimensions(Horizon::default(), 16, 4);
        let (room, cell) = (RoomId(0), CellId(1));
        moteur
            .ecrire(room, cell, Tick(8), EntityId(1), pose(1.0))
            .unwrap();
        assert_eq!(
            moteur.ecrire(room, cell, Tick(4), EntityId(1), pose(2.0)),
            Err(WriteError::AlreadySealed(room, cell, Tick(4)))
        );
    }

    #[test]
    fn slot_recycle_ne_conserve_pas_l_ancien_tick() {
        let moteur = ChronotopeEngine::avec_dimensions(Horizon::default(), 16, 4);
        let (room, cell) = (RoomId(0), CellId(2));
        moteur
            .ecrire(room, cell, Tick(0), EntityId(1), pose(1.0))
            .unwrap();
        moteur.sceller(room, cell, Tick(0));
        moteur
            .ecrire(room, cell, Tick(4), EntityId(2), pose(2.0))
            .unwrap();
        let scelle = moteur.sceller(room, cell, Tick(4));
        assert_eq!(scelle.entities, vec![(EntityId(2), pose(2.0))]);
        assert!(moteur.lire(room, &[cell], Tick(0)).is_empty());
    }

    #[test]
    fn compaction_borne_le_journal_d_un_ecrivain_bavard() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let (room, cell, tick) = (RoomId(0), CellId(0), Tick(0));
        for i in 0..(SEUIL_COMPACTION * 4) {
            moteur
                .ecrire(room, cell, tick, EntityId(0), pose(i as f32))
                .unwrap();
        }
        let scelle = moteur.sceller(room, cell, tick);
        assert_eq!(
            scelle.entities,
            vec![(EntityId(0), pose((SEUIL_COMPACTION * 4 - 1) as f32))]
        );
    }

    #[test]
    fn compaction_reste_correcte_avec_bavards_et_distinctes_entrelaces() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let (room, cell, tick) = (RoomId(0), CellId(1), Tick(0));
        for i in 0..(SEUIL_COMPACTION * 3) {
            moteur
                .ecrire(room, cell, tick, EntityId(0), pose(i as f32))
                .unwrap();
            moteur
                .ecrire(room, cell, tick, EntityId((i + 1) as u32), pose(i as f32))
                .unwrap();
        }
        let scelle = moteur.sceller(room, cell, tick);
        assert_eq!(scelle.entities.len(), SEUIL_COMPACTION * 3 + 1);
        assert_eq!(
            scelle.entities[0],
            (EntityId(0), pose((SEUIL_COMPACTION * 3 - 1) as f32))
        );
    }

    /// Regression : declencher la compaction sur `journal.len() >=
    /// SEUIL_COMPACTION` plutot que sur les entrees ecrites DEPUIS la
    /// derniere compaction recompactait tout le journal a CHAQUE ecriture
    /// une fois le seuil franchi. Sans doublon a eliminer (le regime
    /// agglomere que §1 vise, beaucoup d'entites distinctes), cela degradait
    /// en O(n^2) : mesure sur ce moteur, 8000 entites distinctes prenaient
    /// ~600ms avant le correctif contre quelques ms apres. Le seuil ci-dessous
    /// laisse une marge large (10-100x) pour ne jamais etre un faux positif
    /// en CI tout en detectant un retour du comportement quadratique.
    #[test]
    fn compaction_reste_amortie_sur_un_afflux_d_entites_distinctes() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let (room, cell, tick) = (RoomId(0), CellId(2), Tick(0));
        let debut = std::time::Instant::now();
        for i in 0..20_000u32 {
            moteur
                .ecrire(room, cell, tick, EntityId(i), pose(i as f32))
                .unwrap();
        }
        assert!(
            debut.elapsed() < std::time::Duration::from_secs(1),
            "20 000 ecritures distinctes ont pris {:?} : la compaction s'est probablement remise \
             a re-trier tout le journal a chaque ecriture (regression O(n^2))",
            debut.elapsed()
        );
        assert_eq!(moteur.sceller(room, cell, tick).entities.len(), 20_000);
    }

    #[test]
    fn vider_conserve_les_anneaux_et_efface_les_sceaux() {
        let moteur = ChronotopeEngine::new(Horizon::default());
        let (room, cell, tick) = (RoomId(0), CellId(3), Tick(1));
        moteur
            .ecrire(room, cell, tick, EntityId(1), pose(1.0))
            .unwrap();
        moteur.sceller(room, cell, tick);
        moteur.vider();
        assert!(moteur.lire(room, &[cell], tick).is_empty());
        moteur
            .ecrire(room, cell, tick, EntityId(1), pose(2.0))
            .unwrap();
        assert_eq!(
            moteur.sceller(room, cell, tick).entities,
            vec![(EntityId(1), pose(2.0))]
        );
    }
}

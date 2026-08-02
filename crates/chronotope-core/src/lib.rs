//! ChronotopeDB — substrat spatio-temporel a horizon scelle.
//!
//! Concept source : `docs/chronotope.md` (repris de PawChat,
//! `docs/serveur-hyperscale-metaverse.md` §5). Ce crate implemente le
//! substrat lui-meme : l'unite de stockage est la cellule-tick, pas la ligne.
//!
//! ## Contrat fige (§5.9 du document source)
//!
//! ```text
//! sceller(room, cell, tick) -> Chronotope            // idempotent
//! ecrire(room, cell, tick, entity, pose) -> Ack|Rejet // Rejet si deja scelle
//! lire(room, [cells], tick) -> [Chronotope]           // tick < maintenant - Δ garanti
//! observer(room, [cells]) -> Flux<Chronotope>         // pousse les sceaux
//! ```
//!
//! `migrer` (la 5e operation) vit dans `chronotope-directory`, pas ici : ce
//! crate ne connait ni les noeuds ni le routage, seulement le stockage local
//! d'une cellule. C'est deliberement le partage de responsabilite du §5.6 :
//! "Chronotope est le magasin lieu -> etat."
//!
//! ## Ce que ce crate NE fait PAS
//!
//! Pas de transaction multi-cellule, pas de jointure, pas d'index secondaire,
//! pas de langage de requete, pas de replication reseau. Un systeme qui ne
//! fait qu'une chose (§5.9). Toute donnee a cardinalite d'ecriture > 1 (solde,
//! propriete, score) n'a pas sa place ici — elle reste dans un systeme
//! transactionnel (SpacetimeDB, chez PawChat). Voir §4 du document source
//! pour le critere de frontiere.

#![deny(unsafe_code)]

mod diffusion;
mod engine;
mod naive;
mod store;

pub use engine::{ChronotopeEngine, ANNEAU, CELLULES_PAR_SALLE, SALLES_MAX};
pub use naive::NaiveRowStore;
pub use store::ChronotopeStore;

use std::time::Duration;

/// Salle causalement disjointe (§5.7) — aucune relation "arrive-avant" entre
/// deux salles distinctes n'a besoin d'etre preservee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomId(pub u32);

/// Cellule spatiale. Suit le decoupage de terrain existant cote PawChat
/// (`cell_of_position`, `spacetimedb/src/parcels.rs:117`) — ce crate ne
/// prescrit pas de fonction de decoupage, seulement l'identifiant qui en
/// resulte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellId(pub u32);

/// Tick logique monotone PAR SALLE (§5.7) — pas une horloge murale, pas
/// synchronise entre salles (elles sont causalement disjointes, donc rien ne
/// l'exige). L'age d'un chronotope est connu exactement par construction :
/// il est dans sa cle (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(pub u64);

impl Tick {
    pub fn next(self) -> Tick {
        Tick(self.0 + 1)
    }
}

/// Identite d'entite (avatar, objet tenu — cardinalite d'ecriture 1 uniquement,
/// voir §4). Une entite a cardinalite d'ecriture > 1 n'appartient pas ici.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u32);

/// Etat d'une entite a un instant : position + orientation. La quantification
/// fine du format sur le fil (smallest-three, etc., voir
/// `docs/temps-reel-vr-mmo.md` §3.2 cote PawChat) est un souci de la couche
/// reseau, hors du perimetre de ce crate — le stockage manipule des f32 pour
/// rester exact dans les benchmarks et les tests de non-regression du
/// modele de coherence, pas du format d'octets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pos: [f32; 3],
    /// Lacet uniquement (yaw) — suffisant pour un avatar VR debout, comme le
    /// reste de l'architecture PawChat (`rot_y`). Pas un quaternion complet :
    /// generaliser est un non-but explicite de ce prototype (§5.11.4 —
    /// aucune requete historique riche n'est visee).
    pub yaw: f32,
}

/// Un chronotope : l'etat empaquete de toutes les entites d'une cellule a un
/// tick donne (§5.1). Scelle = immuable, replicable, cachable sans
/// coordination. Non scelle = encore ouvert aux ecritures (phase CRDT LWW
/// degeneree, §4 point 1 — un seul ecrivain possible par entite, donc pas de
/// vecteur de version necessaire).
#[derive(Debug, Clone, PartialEq)]
pub struct Chronotope {
    pub room: RoomId,
    pub cell: CellId,
    pub tick: Tick,
    pub sealed: bool,
    /// Triees par `EntityId` — invariant a preserver par toute implementation
    /// (simplifie la comparaison dans les tests et le proptest de §H1).
    pub entities: Vec<(EntityId, Pose)>,
}

/// Horizon de scellement (§5.2). Reprend TEL QUEL le delai d'interpolation
/// que le client doit deja subir pour absorber la gigue — le tampon de
/// fluidite et la fenetre de coherence sont le meme budget, depense une
/// seule fois. Valeur par defaut alignee sur `docs/temps-reel-vr-mmo.md`
/// (~100 ms) ; H2 (voir docs/chronotope.md) teste directement la sensibilite
/// a ce choix, donc il DOIT rester configurable, jamais une constante figee
/// dans le moteur.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Horizon(pub Duration);

impl Default for Horizon {
    fn default() -> Self {
        Horizon(Duration::from_millis(100))
    }
}

/// Refus d'ecriture — UNIQUEMENT parce que le chronotope cible est deja
/// scelle (§5.2 : "une ecriture arrivant apres le sceau est rejetee, jamais
/// fusionnee"). Ce n'est pas un type d'erreur generique : toute autre
/// defaillance (cellule invalide, salle inconnue) est un bug appelant, pas
/// un etat metier a gerer, et doit paniquer ou etre un autre type dans
/// l'implementation concrete — ne pas elargir cette enum par confort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("chronotope ({0:?}, {1:?}, {2:?}) deja scelle — ecriture rejetee")]
    AlreadySealed(RoomId, CellId, Tick),
}

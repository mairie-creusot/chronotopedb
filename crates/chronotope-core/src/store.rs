use crossbeam_channel::Receiver;

use crate::{CellId, Chronotope, EntityId, Pose, RoomId, Tick, WriteError};

/// Le contrat de stockage fige (§5.9 du document source). Deux implementations
/// le respectent dans ce crate :
///
/// - [`crate::ChronotopeEngine`] — le substrat reel (cellule-tick, §5.1-5.2).
/// - [`crate::NaiveRowStore`] — le temoin de comparaison pour H1 : une ligne
///   par entite, mutation en place, verrou par ligne. C'est le modele que
///   H1 doit battre d'au moins 3x en regime agglomere pour ne pas etre
///   refute (voir `docs/chronotope.md` §H1 et
///   `benches/h1_frame_vs_row.rs`).
///
/// Les deux DOIVENT passer exactement la meme suite de tests (voir
/// `tests/` a la racine du crate) : c'est ce qui rend la comparaison de
/// benchmark honnete plutot que biaisee par des garanties differentes.
pub trait ChronotopeStore: Send + Sync {
    /// Ecrit la pose d'une entite dans le chronotope (room, cell, tick).
    /// Refuse si ce chronotope est deja scelle (§5.2) — jamais de fusion
    /// tardive. Ecrire deux fois la meme entite au meme tick avant scellement
    /// est un LWW (dernier appel gagne) : c'est le CRDT degenere de §4,
    /// correct ici parce qu'une pose n'a par construction qu'un seul
    /// ecrivain concurrent possible.
    fn ecrire(
        &self,
        room: RoomId,
        cell: CellId,
        tick: Tick,
        entity: EntityId,
        pose: Pose,
    ) -> Result<(), WriteError>;

    /// Scelle (room, cell, tick) : idempotent, renvoie le meme resultat que
    /// l'appel soit le premier ou le centieme. Apres cet appel, `ecrire` sur
    /// la meme cle renvoie toujours `WriteError::AlreadySealed`.
    fn sceller(&self, room: RoomId, cell: CellId, tick: Tick) -> Chronotope;

    /// Lit les chronotopes scelles des cellules demandees a un tick donne.
    /// Le contrat `tick < maintenant - Δ garanti` (§5.9) est une
    /// responsabilite de l'APPELANT (typiquement la boucle d'horloge qui
    /// pilote `sceller`) : cette methode ne lit QUE des chronotopes deja
    /// scelles et ne retourne jamais d'etat speculatif, quel que soit le
    /// tick demande — un tick pas encore scelle est simplement absent du
    /// resultat, jamais renvoye partiellement.
    fn lire(&self, room: RoomId, cells: &[CellId], tick: Tick) -> Vec<Chronotope>;

    /// Flux des sceaux pour les cellules donnees, au fil de l'eau. Pousse
    /// UNIQUEMENT apres scellement (jamais d'etat speculatif observable, la
    /// propriete centrale de §5.2 : "le client ne lit jamais que du scelle").
    /// Le canal est borne : un abonne trop lent doit rattraper ou etre
    /// coupe, jamais faire grossir la memoire du store sans limite —
    /// c'est un choix d'implementation delegue, mais le comportement en cas
    /// de depassement DOIT etre documente et teste par l'implementation.
    fn observer(&self, room: RoomId, cells: &[CellId]) -> Receiver<Chronotope>;
}

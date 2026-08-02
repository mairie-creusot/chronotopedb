//! Annuaire entite -> (noeud, cellule) et migration SANS transfert d'etat
//! (§5.3). L'idee centrale, a preserver dans toute implementation :
//!
//! ```text
//! migration de E, cellule A (noeud 1) -> cellule B (noeud 2), au tick T :
//!   noeud 1 : cesse d'inscrire E a partir de T+1
//!   noeud 2 : commence a inscrire E a partir de T+1
//!   routage : entity_directory[E] <- (B, noeud 2)
//! ```
//!
//! Il n'y a rien a transferer parce que chaque trame de pose est
//! auto-suffisante (ne depend pas d'un etat anterieur) — c'est une propriete
//! du modele de donnees de `chronotope-core`, pas de ce crate. Ce crate se
//! contente d'ecrire l'annuaire ; il ne touche jamais au contenu d'un
//! chronotope.
//!
//! ## Hysteresis obligatoire (§5.3)
//!
//! Le retour d'experience SpatialOS cite dans le document source est
//! explicite : sans bande de recouvrement, une entite qui longe une
//! frontiere de cellule peut migrer en rafale (thrashing). L'implementation
//! DOIT inclure une hysteresis (par ex. rayon d'entree > rayon de sortie, ou
//! un delai minimum entre deux migrations de la meme entite) et le prouver
//! par un test qui simule une trajectoire le long d'une frontiere.
//!
//! Mecanisme retenu ici : le delai minimum entre deux migrations de la meme
//! entite ([`Hysteresis`]), plutot que le couple rayon d'entree / rayon de
//! sortie. Un annuaire ne connait que des `CellId`, jamais une geometrie ; il
//! ne peut donc pas comparer des rayons sans dependre du decoupage spatial,
//! ce qui remonterait une dependance dans le mauvais sens
//! (`docs/conventions.md`, "Modularite"). Le tick, lui, est deja fourni par
//! l'appelant et rend l'hysteresis deterministe et rejouable.
//!
//! ## Ce que ce crate doit prouver (H3)
//!
//! "La migration sans transfert n'introduit pas d'artefact" — refutee si un
//! taux de glitch superieur a une bascule d'autorite classique (a la
//! SpatialOS) est observe au franchissement de frontiere. Voir `tests/`
//! pour le harnais de simulation correspondant :
//! `tests/h3_thrashing.rs` (comparaison avec / sans hysteresis sur la meme
//! trajectoire), `tests/migration_o1.rs` (cout de migration independant de
//! l'occupation de la cellule destination) et `tests/proptest_annuaire.rs`
//! (une entite a toujours exactement un emplacement).

mod directory;

pub use directory::{Directory, Hysteresis, Location, Migration, NodeId, RefusHysteresis};

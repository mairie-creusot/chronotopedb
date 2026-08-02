//! Harnais de simulation en memoire — plusieurs "noeuds" logiques dans le
//! meme process, connectes par un `chronotope_directory::Directory` partage
//! et des `chronotope_core::ChronotopeEngine` distincts (un par noeud). Sert
//! a tester H3 et H4 sans reseau reel, avec des trajectoires controlees et
//! rejouables (donc des tests deterministes, pas juste des benchmarks).
//!
//! ## H3 — migration sans transfert n'introduit pas d'artefact
//!
//! Refutee si un taux de glitch superieur a une bascule d'autorite classique
//! (a la SpatialOS — transfert d'etat explicite) est observe au
//! franchissement de frontiere. "Glitch" a definir operationnellement ici :
//! au minimum, aucune duplication de pose (l'entite apparait dans deux
//! chronotopes du meme tick sur deux noeuds), aucune perte de pose (un tick
//! sans aucune inscription pour une entite active), et une latence de
//! bascule mesuree — pas juste "ca ne plante pas".
//!
//! ## H4 — la dilatation par cellule est preferable au plafond dur
//!
//! Le harnais doit pouvoir simuler une charge croissante sur une cellule et
//! verifier que la cadence se degrade par paliers (20 Hz -> 10 Hz -> 5 Hz,
//! §5.8) plutot que de refuser des entrees au-dela d'un seuil fixe — et que
//! cette degradation reste LOCALE a la cellule chargee (les autres cellules
//! de la meme simulation ne doivent pas etre affectees).
//!
//! Un vrai jugement de "confortable ou non" (H2, H4) exige un humain — ce
//! harnais mesure ce qui est objectivement mesurable (latence, taux de
//! glitch, cadence effective) ; il ne pretend pas trancher H2/H4 seul.
//!
//! ## Comment ce crate est branche, et pourquoi generiquement
//!
//! [`MultiNodeSim`] est generique sur son store et son routeur, avec
//! `ChronotopeEngine` et [`AnnuaireMemoire`] pour valeurs par defaut. Ce n'est
//! pas de la generalite gratuite : la logique du harnais (arbitrage de
//! cellule, quantification temporelle, comptage des artefacts) est ce que H3 et
//! H4 mesurent, et elle doit pouvoir etre validee contre des doubles
//! deterministes independamment de l'etat d'avancement du substrat. Les tests
//! qui exigent le VRAI moteur vivent dans `tests/` et sont marques `#[ignore]`
//! tant que `ChronotopeEngine` est un squelette.
//!
//! Le routage passe par le trait [`Routeur`] plutot que par
//! `chronotope_directory::Directory` en dur — voir le module doc de
//! `routage.rs` pour les deux raisons, dont une contrainte de compilation
//! actuelle du crate annuaire.

#![deny(unsafe_code)]

mod cadence;
#[cfg(test)]
mod double;
mod harness;
mod routage;
mod topologie;

pub use cadence::{Palier, PolitiqueCadence};
pub use harness::{MultiNodeSim, SimConfig, SimReport};
pub use routage::{AnnuaireMemoire, Emplacement, Routeur};
pub use topologie::{noeud_de_cellule, Grille};

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

mod harness;

pub use harness::{MultiNodeSim, SimReport};

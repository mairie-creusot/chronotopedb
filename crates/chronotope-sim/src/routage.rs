//! Le routage entite -> (noeud, cellule) tel que le harnais le consomme : un
//! trait `Routeur` qui n'expose que `ou_est` / `migrer` (§10, l'operation
//! `migrer`), plus une implementation en memoire a hysteresis.
//!
//! ## Pourquoi un trait plutot que `chronotope_directory::Directory` en dur
//!
//! Deux raisons, dans cet ordre d'importance :
//!
//! 1. Le harnais doit pouvoir faire varier l'hysteresis et mesurer le
//!    thrashing en fonction de son reglage (contrainte explicite du contrat
//!    de `Directory`) ; un trait permet de comparer plusieurs politiques dans
//!    la meme suite de tests.
//! 2. `chronotope_directory` ne re-exporte pas `NodeId` (seulement `Directory`
//!    et `Location`, dont le champ `node` devient donc innommable hors du
//!    crate) : `Location` est litteralement inconstructible depuis ici tant
//!    que ce `pub use` n'est pas elargi. L'implementation de `Routeur` pour le
//!    vrai `Directory` existe plus bas mais reste derriere la feature
//!    `annuaire-reel`, desactivee par defaut, pour ne pas casser la
//!    compilation de ce crate en attendant.
//!
//! ## Contrat commun a toute implementation
//!
//! `migrer` peut etre IGNOREE — c'est precisement le role de l'hysteresis
//! (§4 : sans bande de recouvrement, une entite qui longe une frontiere migre
//! en rafale). L'appelant ne doit donc JAMAIS supposer qu'une migration a pris
//! effet : il relit `ou_est` et ecrit la pose la ou l'annuaire fait autorite,
//! pas la ou la geometrie voudrait qu'elle soit. C'est ce qui garantit qu'une
//! entite n'est jamais inscrite a deux endroits au meme tick.

use std::collections::HashMap;

use chronotope_core::{CellId, EntityId};

/// Emplacement d'autorite d'une entite. Volontairement distinct de
/// `chronotope_directory::Location` : le harnais n'a pas besoin de la nommer
/// pour fonctionner, et la conversion est confinee a l'implementation du
/// trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Emplacement {
    pub noeud: u32,
    pub cellule: CellId,
}

/// Annuaire vu par le harnais. `&mut self` plutot que `&self` : ici il n'y a
/// qu'un seul appelant, dans un seul thread, et exiger la mutabilite exclusive
/// evite d'imposer une mutabilite interieure a une implementation qui n'en a
/// pas besoin. Une implementation `&self` (comme le vrai `Directory`) satisfait
/// ce trait sans effort.
pub trait Routeur {
    /// Emplacement d'autorite courant, ou `None` si l'entite n'est pas encore
    /// inscrite a l'annuaire.
    fn ou_est(&mut self, entity: EntityId) -> Option<Emplacement>;

    /// Demande de migration. Renvoie `true` si elle a pris effet, `false` si
    /// l'hysteresis l'a refusee. Ne transfere AUCUN etat (§4) : c'est une
    /// ecriture d'annuaire, O(1), rien de plus.
    fn migrer(&mut self, entity: EntityId, dest: Emplacement) -> bool;
}

/// Annuaire en memoire a hysteresis par delai minimum.
///
/// L'hysteresis retenue est le "delai minimum entre deux migrations de la meme
/// entite" propose par §4, compte en tentatives consecutives refusees. Le
/// harnais appelant `migrer` au plus une fois par entite et par tick, une
/// tentative vaut exactement un tick : `hysteresis_ticks = 3` signifie qu'une
/// entite doit etre sortie de sa cellule d'autorite pendant 3 ticks consecutifs
/// avant que la bascule soit accordee, et borne donc la latence de bascule que
/// H3 mesure.
///
/// L'alternative de §4 (rayon d'entree > rayon de sortie) n'est pas
/// implementable ici : un annuaire ne voit que des identifiants de cellule,
/// jamais de geometrie — c'est un constat sur le contrat de `Directory`, pas
/// un raccourci.
#[derive(Debug, Clone)]
pub struct AnnuaireMemoire {
    hysteresis_ticks: u32,
    entrees: HashMap<EntityId, Entree>,
    migrations_accordees: u64,
    migrations_refusees: u64,
}

#[derive(Debug, Clone, Copy)]
struct Entree {
    emplacement: Emplacement,
    tentatives_consecutives: u32,
}

impl AnnuaireMemoire {
    /// `hysteresis_ticks = 0` ou `1` accorde toute migration des la premiere
    /// tentative : c'est le temoin "sans hysteresis" auquel les tests
    /// comparent le comportement amorti.
    pub fn new(hysteresis_ticks: u32) -> Self {
        Self {
            hysteresis_ticks,
            entrees: HashMap::new(),
            migrations_accordees: 0,
            migrations_refusees: 0,
        }
    }

    pub fn migrations_accordees(&self) -> u64 {
        self.migrations_accordees
    }

    pub fn migrations_refusees(&self) -> u64 {
        self.migrations_refusees
    }

    pub fn hysteresis_ticks(&self) -> u32 {
        self.hysteresis_ticks
    }
}

impl Default for AnnuaireMemoire {
    fn default() -> Self {
        Self::new(3)
    }
}

impl Routeur for AnnuaireMemoire {
    #[tracing::instrument(level = "trace", skip(self), fields(entity = entity.0))]
    fn ou_est(&mut self, entity: EntityId) -> Option<Emplacement> {
        self.entrees.get(&entity).map(|e| e.emplacement)
    }

    #[tracing::instrument(
        level = "debug",
        skip(self),
        fields(entity = entity.0, noeud = dest.noeud, cellule = dest.cellule.0)
    )]
    fn migrer(&mut self, entity: EntityId, dest: Emplacement) -> bool {
        match self.entrees.get_mut(&entity) {
            None => {
                self.entrees.insert(
                    entity,
                    Entree {
                        emplacement: dest,
                        tentatives_consecutives: 0,
                    },
                );
                self.migrations_accordees += 1;
                tracing::debug!("premiere inscription a l'annuaire");
                true
            }
            Some(entree) if entree.emplacement == dest => {
                entree.tentatives_consecutives = 0;
                true
            }
            Some(entree) => {
                entree.tentatives_consecutives += 1;
                if entree.tentatives_consecutives >= self.hysteresis_ticks.max(1) {
                    entree.emplacement = dest;
                    entree.tentatives_consecutives = 0;
                    self.migrations_accordees += 1;
                    tracing::debug!("migration accordee");
                    true
                } else {
                    self.migrations_refusees += 1;
                    tracing::trace!(
                        tentatives = entree.tentatives_consecutives,
                        "migration retenue par l'hysteresis"
                    );
                    false
                }
            }
        }
    }
}

/// Implementation pour le vrai annuaire. Derriere la feature `annuaire-reel`
/// (desactivee par defaut) tant que `chronotope_directory` ne re-exporte pas
/// `NodeId` : sans ce `pub use`, `Location` est inconstructible depuis ce
/// crate et ce bloc ne compile pas. Voir le module doc ci-dessus.
#[cfg(feature = "annuaire-reel")]
impl Routeur for chronotope_directory::Directory {
    fn ou_est(&mut self, entity: EntityId) -> Option<Emplacement> {
        chronotope_directory::Directory::locate(self, entity).map(|l| Emplacement {
            noeud: l.node.0,
            cellule: l.cell,
        })
    }

    fn migrer(&mut self, entity: EntityId, dest: Emplacement) -> bool {
        let cible = chronotope_directory::Location {
            node: chronotope_directory::NodeId(dest.noeud),
            cell: dest.cellule,
        };
        chronotope_directory::Directory::migrer(self, entity, cible);
        chronotope_directory::Directory::locate(self, entity) == Some(cible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emp(noeud: u32, cellule: u32) -> Emplacement {
        Emplacement {
            noeud,
            cellule: CellId(cellule),
        }
    }

    #[test]
    fn la_premiere_inscription_est_toujours_accordee() {
        let mut a = AnnuaireMemoire::new(5);
        assert!(a.migrer(EntityId(1), emp(0, 0)));
        assert_eq!(a.ou_est(EntityId(1)), Some(emp(0, 0)));
    }

    #[test]
    fn l_hysteresis_retarde_la_bascule_d_un_nombre_exact_de_tentatives() {
        let mut a = AnnuaireMemoire::new(3);
        a.migrer(EntityId(1), emp(0, 0));
        assert!(!a.migrer(EntityId(1), emp(1, 1)));
        assert!(!a.migrer(EntityId(1), emp(1, 1)));
        assert!(a.migrer(EntityId(1), emp(1, 1)));
        assert_eq!(a.ou_est(EntityId(1)), Some(emp(1, 1)));
    }

    #[test]
    fn sans_hysteresis_la_bascule_est_immediate() {
        for h in [0u32, 1] {
            let mut a = AnnuaireMemoire::new(h);
            a.migrer(EntityId(1), emp(0, 0));
            assert!(a.migrer(EntityId(1), emp(1, 1)));
        }
    }

    #[test]
    fn une_oscillation_frontaliere_est_amortie() {
        let mut a = AnnuaireMemoire::new(4);
        a.migrer(EntityId(1), emp(0, 0));
        for t in 0..100 {
            let cible = if t % 2 == 0 { emp(1, 1) } else { emp(0, 0) };
            if a.ou_est(EntityId(1)) != Some(cible) {
                a.migrer(EntityId(1), cible);
            }
        }
        assert!(
            a.migrations_accordees() <= 100 / 4 + 1,
            "thrashing non amorti : {} migrations accordees",
            a.migrations_accordees()
        );
    }

    #[test]
    fn migrer_vers_l_emplacement_courant_est_un_no_op_reussi() {
        let mut a = AnnuaireMemoire::new(3);
        a.migrer(EntityId(1), emp(0, 0));
        assert!(a.migrer(EntityId(1), emp(0, 0)));
        assert_eq!(a.migrations_refusees(), 0);
    }
}

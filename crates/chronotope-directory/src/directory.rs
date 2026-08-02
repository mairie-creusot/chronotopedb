use std::collections::hash_map::Entry;
use std::collections::HashMap;

use chronotope_core::{CellId, EntityId, Tick};
use parking_lot::RwLock;

/// Identifiant de noeud de simulation. Un prototype mono-machine peut se
/// contenter d'entiers arbitraires (simulation multi-noeud en memoire, voir
/// `chronotope-sim`) ; rien ici ne suppose un vrai reseau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub node: NodeId,
    pub cell: CellId,
}

/// Hysteresis de migration : delai minimum, en ticks, entre deux migrations
/// de la MEME entite (§5.3).
///
/// Des deux mecanismes proposes par le document source (rayon d'entree >
/// rayon de sortie, ou delai minimum entre deux migrations), seul le second
/// est implementable ici sans violer la frontiere de responsabilite du
/// crate : un annuaire ne connait que des `CellId`, jamais une geometrie —
/// il ne peut donc pas comparer des rayons. Le delai, lui, ne depend que du
/// tick, que l'appelant fournit deja.
///
/// La contrepartie est explicite et mesurable : pendant au plus
/// `ticks_min_entre_migrations` ticks apres une migration, l'annuaire peut
/// designer une cellule que l'entite a geometriquement quittee. C'est le
/// prix de la bande de recouvrement, pas un bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hysteresis {
    pub ticks_min_entre_migrations: u64,
}

impl Hysteresis {
    /// Aucune bande de recouvrement — le comportement que le retour
    /// d'experience SpatialOS decrit comme sujet au thrashing. N'existe que
    /// comme temoin de comparaison pour les tests et pour `chronotope-sim`.
    pub const fn aucune() -> Self {
        Hysteresis {
            ticks_min_entre_migrations: 0,
        }
    }

    pub const fn ticks(n: u64) -> Self {
        Hysteresis {
            ticks_min_entre_migrations: n,
        }
    }
}

impl Default for Hysteresis {
    /// 4 ticks — a 20 Hz, 200 ms, soit 2Δ pour l'horizon par defaut de
    /// `chronotope_core::Horizon`. Le choix n'a rien d'universel : c'est
    /// exactement pour cela qu'il est un parametre et non une constante.
    fn default() -> Self {
        Hysteresis::ticks(4)
    }
}

/// Ce qu'a fait une migration acceptee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    /// L'annuaire a ete ecrit. `depuis` vaut `None` pour une premiere
    /// inscription (l'entite etait inconnue).
    Effectuee { depuis: Option<Location> },
    /// L'entite est deja a cet emplacement : aucune ecriture, et l'horloge
    /// d'hysteresis n'est pas relancee.
    Inchangee,
}

/// Migration refusee par l'hystere. Ce n'est pas une perte : l'appelant
/// reemet son intention au tick suivant (c'est le mode d'emploi normal d'une
/// boucle de simulation), et la migration passe des que
/// `ticks_restants` est ecoule si l'entite est toujours de l'autre cote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "migration de {entite:?} vers {dest:?} refusee par l'hysteresis — {ticks_restants} tick(s) restants"
)]
pub struct RefusHysteresis {
    pub entite: EntityId,
    pub dest: Location,
    pub ticks_restants: u64,
}

#[derive(Debug, Clone, Copy)]
struct Entree {
    loc: Location,
    derniere_migration: Option<Tick>,
}

/// Annuaire entite -> emplacement courant, avec migration a hysteresis.
///
/// Voir le module doc de `lib.rs` pour le contrat. Deux proprietes
/// structurantes :
///
/// - `migrer` est O(1) : une seule ecriture d'annuaire, jamais proportionnelle
///   au nombre d'entites deja presentes dans la cellule destination — c'est
///   la propriete qui rend la migration sans transfert d'etat interessante
///   (§5.3). Aucun index cellule -> entites n'est maintenu ici, precisement
///   parce qu'il rendrait la migration proportionnelle a l'occupation.
/// - L'hysteresis est un parametre ([`Hysteresis`]), jamais une constante,
///   pour que `chronotope-sim` puisse la faire varier et mesurer le
///   thrashing en fonction de son reglage.
///
/// Toutes les methodes prennent `&self` : l'annuaire est partageable entre
/// noeuds simules sans enveloppe supplementaire.
pub struct Directory {
    hysteresis: Hysteresis,
    entrees: RwLock<HashMap<EntityId, Entree>>,
}

impl Directory {
    pub fn new() -> Self {
        Directory::avec_hysteresis(Hysteresis::default())
    }

    pub fn avec_hysteresis(hysteresis: Hysteresis) -> Self {
        Directory {
            hysteresis,
            entrees: RwLock::new(HashMap::new()),
        }
    }

    pub fn hysteresis(&self) -> Hysteresis {
        self.hysteresis
    }

    pub fn len(&self) -> usize {
        self.entrees.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entrees.read().is_empty()
    }

    /// Premiere inscription (ou replacement autoritaire) d'une entite, sans
    /// passer par l'hysteresis : une entite qui entre dans le monde n'a pas
    /// de migration precedente a espacer. Renvoie l'emplacement precedent
    /// s'il y en avait un.
    #[tracing::instrument(level = "debug", skip(self), fields(entity = entity.0, node = dest.node.0, cell = dest.cell.0, tick = at.0))]
    pub fn inscrire(&self, entity: EntityId, dest: Location, at: Tick) -> Option<Location> {
        let precedent = self.entrees.write().insert(
            entity,
            Entree {
                loc: dest,
                derniere_migration: None,
            },
        );
        tracing::debug!(deja_connue = precedent.is_some(), "entite inscrite");
        precedent.map(|e| e.loc)
    }

    #[tracing::instrument(level = "trace", skip(self), fields(entity = entity.0))]
    pub fn locate(&self, entity: EntityId) -> Option<Location> {
        let loc = self.entrees.read().get(&entity).map(|e| e.loc);
        match loc {
            Some(l) => tracing::trace!(node = l.node.0, cell = l.cell.0, "entite localisee"),
            None => tracing::trace!("entite inconnue de l'annuaire"),
        }
        loc
    }

    /// Ecrit le nouvel emplacement. Ne transfere AUCUN etat — c'est le point
    /// central de §5.3 : chaque trame de pose etant auto-suffisante, la
    /// bascule coute une entree de table de routage, pas un transfert.
    ///
    /// `at` est le tick de la migration (§5.3 : "migration de E ... au tick
    /// T") ; il sert d'unique horloge a l'hysteresis, ce qui rend le
    /// comportement deterministe et rejouable, contrairement a un delai en
    /// temps mural.
    ///
    /// Refuse ([`RefusHysteresis`]) si moins de
    /// `hysteresis.ticks_min_entre_migrations` ticks se sont ecoules depuis
    /// la derniere migration acceptee de CETTE entite. Une destination egale
    /// a l'emplacement courant n'est jamais un refus ni une ecriture
    /// ([`Migration::Inchangee`]) : c'est le cas normal d'une boucle qui
    /// reemet l'emplacement souhaite a chaque tick.
    ///
    /// ```
    /// use chronotope_core::{CellId, EntityId, Tick};
    /// use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};
    ///
    /// let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(4));
    /// let a = Location { node: NodeId(1), cell: CellId(10) };
    /// let b = Location { node: NodeId(2), cell: CellId(11) };
    ///
    /// annuaire.inscrire(EntityId(7), a, Tick(0));
    /// assert_eq!(
    ///     annuaire.migrer(EntityId(7), b, Tick(1)),
    ///     Ok(Migration::Effectuee { depuis: Some(a) })
    /// );
    ///
    /// // Retour immediat vers A : refuse, l'entite vient de migrer.
    /// assert!(annuaire.migrer(EntityId(7), a, Tick(2)).is_err());
    /// assert_eq!(annuaire.locate(EntityId(7)), Some(b));
    ///
    /// // Une fois le delai ecoule, la meme intention passe.
    /// assert_eq!(
    ///     annuaire.migrer(EntityId(7), a, Tick(5)),
    ///     Ok(Migration::Effectuee { depuis: Some(b) })
    /// );
    /// ```
    #[tracing::instrument(level = "debug", skip(self), fields(entity = entity.0, node = dest.node.0, cell = dest.cell.0, tick = at.0))]
    pub fn migrer(
        &self,
        entity: EntityId,
        dest: Location,
        at: Tick,
    ) -> Result<Migration, RefusHysteresis> {
        let mut entrees = self.entrees.write();

        let mut occupee = match entrees.entry(entity) {
            Entry::Vacant(libre) => {
                libre.insert(Entree {
                    loc: dest,
                    derniere_migration: None,
                });
                tracing::debug!("premiere inscription par migration");
                return Ok(Migration::Effectuee { depuis: None });
            }
            Entry::Occupied(occupee) => occupee,
        };
        let entree = occupee.get_mut();

        if entree.loc == dest {
            tracing::trace!("destination deja courante");
            return Ok(Migration::Inchangee);
        }

        if let Some(precedente) = entree.derniere_migration {
            // Tick non monotone = bug appelant (§5.7 : monotone par salle) ;
            // saturate plutot que paniquer dans le chemin chaud.
            let ecoule = at.0.saturating_sub(precedente.0);
            let requis = self.hysteresis.ticks_min_entre_migrations;
            if ecoule < requis {
                let ticks_restants = requis - ecoule;
                tracing::trace!(ticks_restants, "migration retenue par l'hysteresis");
                return Err(RefusHysteresis {
                    entite: entity,
                    dest,
                    ticks_restants,
                });
            }
        }

        let depuis = entree.loc;
        entree.loc = dest;
        entree.derniere_migration = Some(at);
        tracing::debug!(
            depuis_node = depuis.node.0,
            depuis_cell = depuis.cell.0,
            "migration effectuee"
        );
        Ok(Migration::Effectuee {
            depuis: Some(depuis),
        })
    }

    /// Vue complete de l'annuaire, pour le diagnostic et les tests
    /// (`chronotope-sim` en a besoin pour reconstituer la vue par noeud).
    /// O(n) assume : hors chemin chaud, jamais appele par `migrer`.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn instantane(&self) -> Vec<(EntityId, Location)> {
        self.entrees
            .read()
            .iter()
            .map(|(entity, entree)| (*entity, entree.loc))
            .collect()
    }
}

impl Default for Directory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(node: u32, cell: u32) -> Location {
        Location {
            node: NodeId(node),
            cell: CellId(cell),
        }
    }

    #[test]
    fn migrer_puis_locate_renvoie_le_nouvel_emplacement() {
        let annuaire = Directory::new();
        let a = loc(1, 10);
        let b = loc(2, 11);

        annuaire.inscrire(EntityId(1), a, Tick(0));
        assert_eq!(annuaire.locate(EntityId(1)), Some(a));

        assert_eq!(
            annuaire.migrer(EntityId(1), b, Tick(1)),
            Ok(Migration::Effectuee { depuis: Some(a) })
        );
        assert_eq!(annuaire.locate(EntityId(1)), Some(b));
    }

    #[test]
    fn entite_inconnue_est_introuvable() {
        let annuaire = Directory::new();
        assert_eq!(annuaire.locate(EntityId(404)), None);
        assert!(annuaire.is_empty());
    }

    #[test]
    fn migrer_une_entite_inconnue_l_inscrit() {
        let annuaire = Directory::new();
        assert_eq!(
            annuaire.migrer(EntityId(1), loc(1, 10), Tick(0)),
            Ok(Migration::Effectuee { depuis: None })
        );
        assert_eq!(annuaire.len(), 1);
    }

    #[test]
    fn destination_courante_ne_declenche_ni_ecriture_ni_hysteresis() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(4));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));

        for t in 0..10 {
            assert_eq!(
                annuaire.migrer(EntityId(1), a, Tick(t)),
                Ok(Migration::Inchangee)
            );
        }
        // L'horloge d'hysteresis n'a pas ete relancee : la vraie migration passe.
        assert!(annuaire.migrer(EntityId(1), b, Tick(10)).is_ok());
    }

    #[test]
    fn hysteresis_refuse_puis_laisse_passer_au_tick_exact() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(3));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));

        assert!(annuaire.migrer(EntityId(1), b, Tick(10)).is_ok());
        for (tick, restants) in [(10, 3), (11, 2), (12, 1)] {
            assert_eq!(
                annuaire.migrer(EntityId(1), a, Tick(tick)),
                Err(RefusHysteresis {
                    entite: EntityId(1),
                    dest: a,
                    ticks_restants: restants,
                })
            );
        }
        assert!(annuaire.migrer(EntityId(1), a, Tick(13)).is_ok());
        assert_eq!(annuaire.locate(EntityId(1)), Some(a));
    }

    #[test]
    fn hysteresis_nulle_accepte_toute_migration() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::aucune());
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));

        for t in 1..20 {
            let dest = if t % 2 == 0 { a } else { b };
            assert!(matches!(
                annuaire.migrer(EntityId(1), dest, Tick(t)),
                Ok(Migration::Effectuee { .. })
            ));
        }
    }

    #[test]
    fn hysteresis_est_par_entite_pas_globale() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(8));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        annuaire.inscrire(EntityId(2), a, Tick(0));

        assert!(annuaire.migrer(EntityId(1), b, Tick(1)).is_ok());
        assert!(annuaire.migrer(EntityId(2), b, Tick(1)).is_ok());
        assert!(annuaire.migrer(EntityId(1), a, Tick(2)).is_err());
        assert!(annuaire.migrer(EntityId(2), a, Tick(2)).is_err());
    }

    #[test]
    fn tick_non_monotone_ne_panique_pas() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(4));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));

        assert!(annuaire.migrer(EntityId(1), b, Tick(100)).is_ok());
        assert!(annuaire.migrer(EntityId(1), a, Tick(3)).is_err());
    }

    /// Va au-dela de "ne panique pas" : un tick regressif, meme tres en
    /// arriere, ne doit ni contourner l'hysteresis ni produire un
    /// `ticks_restants` fantome calcule sur un ecart negatif. `saturating_sub`
    /// force l'ecoule a 0 pour tout tick < derniere migration, ce qui est le
    /// resultat le plus conservateur possible (pas seulement "qui ne panique
    /// pas") : on refuse comme si aucun temps ne s'etait ecoule.
    #[test]
    fn tick_regressif_est_refuse_de_facon_correcte_pas_seulement_non_panique() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(10));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(1_000));
        assert!(annuaire.migrer(EntityId(1), b, Tick(1_010)).is_ok());

        // Rejeu d'un tick anterieur meme a l'inscription : refuse, et l'etat
        // reste celui de la derniere migration acceptee.
        let refus = annuaire.migrer(EntityId(1), a, Tick(0)).unwrap_err();
        assert_eq!(
            refus,
            RefusHysteresis {
                entite: EntityId(1),
                dest: a,
                ticks_restants: 10,
            }
        );
        assert_eq!(annuaire.locate(EntityId(1)), Some(b));
    }

    /// `at = 0` rejoue en boucle apres une premiere migration ne doit ni
    /// paniquer, ni faire deriver l'horloge d'hysteresis, ni finir par
    /// passer par usure : le refus reste identique a chaque appel.
    #[test]
    fn tick_zero_repete_des_milliers_de_fois_ne_corrompt_pas_l_etat() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(5));
        let a = loc(1, 10);
        let b = loc(2, 11);
        let c = loc(3, 12);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        // Consomme le seul passage gratuit (aucune precedente a espacer).
        assert!(annuaire.migrer(EntityId(1), b, Tick(0)).is_ok());

        for _ in 0..5_000 {
            assert_eq!(
                annuaire.migrer(EntityId(1), c, Tick(0)),
                Err(RefusHysteresis {
                    entite: EntityId(1),
                    dest: c,
                    ticks_restants: 5,
                })
            );
        }
        assert_eq!(annuaire.locate(EntityId(1)), Some(b));
    }

    /// `at = u64::MAX` ne provoque aucun debordement arithmetique, et
    /// verrouille correctement l'entite : aucun tick fini ne peut plus
    /// jamais depasser u64::MAX pour satisfaire l'hysteresis.
    #[test]
    fn tick_u64_max_ne_deborde_pas_et_verrouille_les_migrations_suivantes() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(4));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(u64::MAX));
        assert!(annuaire.migrer(EntityId(1), b, Tick(u64::MAX)).is_ok());

        assert!(annuaire.migrer(EntityId(1), a, Tick(u64::MAX - 1)).is_err());
        assert!(annuaire.migrer(EntityId(1), a, Tick(0)).is_err());
        assert!(annuaire.migrer(EntityId(1), a, Tick(u64::MAX)).is_err());
        assert_eq!(annuaire.locate(EntityId(1)), Some(b));
    }

    /// Une hysteresis a `u64::MAX` n'empeche pas la premiere migration
    /// (gratuite), et le calcul de `ticks_restants` pour les suivantes ne
    /// deborde jamais meme quand l'ecart requis est le plus grand possible.
    #[test]
    fn hysteresis_u64_max_ne_deborde_pas() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(u64::MAX));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        assert!(annuaire.migrer(EntityId(1), b, Tick(1)).is_ok());

        let refus = annuaire.migrer(EntityId(1), a, Tick(u64::MAX)).unwrap_err();
        assert_eq!(refus.ticks_restants, 1);
    }

    #[test]
    fn hysteresis_ticks_zero_se_comporte_comme_aucune() {
        assert_eq!(Hysteresis::ticks(0), Hysteresis::aucune());
    }

    /// Plusieurs refus consecutifs, vers des destinations differentes, ne
    /// doivent ni deplacer l'entite ni relancer l'horloge d'hysteresis : la
    /// migration passe exactement au tick attendu depuis la DERNIERE
    /// migration effective, jamais depuis une tentative refusee.
    #[test]
    fn un_refus_ne_modifie_ni_l_emplacement_ni_l_horloge_d_hysteresis() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(10));
        let a = loc(1, 10);
        let b = loc(2, 11);
        let c = loc(3, 12);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        assert!(annuaire.migrer(EntityId(1), b, Tick(0)).is_ok());

        assert!(annuaire.migrer(EntityId(1), a, Tick(3)).is_err());
        assert!(annuaire.migrer(EntityId(1), c, Tick(5)).is_err());
        assert_eq!(annuaire.locate(EntityId(1)), Some(b));

        assert!(annuaire.migrer(EntityId(1), c, Tick(9)).is_err());
        assert!(annuaire.migrer(EntityId(1), c, Tick(10)).is_ok());
    }

    /// Rejouer la destination courante des milliers de fois reste un no-op
    /// et ne relance jamais l'horloge d'hysteresis (`Migration::Inchangee`
    /// est cense etre gratuit et sans effet de bord — voir la doc de
    /// `migrer`).
    #[test]
    fn spam_de_la_destination_courante_reste_sans_effet_a_grande_echelle() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(10));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        assert!(annuaire.migrer(EntityId(1), b, Tick(0)).is_ok());

        for t in 1..5_000u64 {
            assert_eq!(
                annuaire.migrer(EntityId(1), b, Tick(t)),
                Ok(Migration::Inchangee)
            );
        }
        assert!(annuaire.migrer(EntityId(1), a, Tick(9)).is_err());
        assert!(annuaire.migrer(EntityId(1), a, Tick(10)).is_ok());
    }

    #[test]
    fn inscrire_replace_sans_hysteresis_et_renvoie_le_precedent() {
        let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(100));
        let a = loc(1, 10);
        let b = loc(2, 11);
        annuaire.inscrire(EntityId(1), a, Tick(0));
        assert!(annuaire.migrer(EntityId(1), b, Tick(1)).is_ok());

        assert_eq!(annuaire.inscrire(EntityId(1), a, Tick(2)), Some(b));
        assert_eq!(annuaire.locate(EntityId(1)), Some(a));
        assert!(annuaire.migrer(EntityId(1), b, Tick(2)).is_ok());
    }

    #[test]
    fn instantane_reflete_l_annuaire() {
        let annuaire = Directory::new();
        annuaire.inscrire(EntityId(1), loc(1, 10), Tick(0));
        annuaire.inscrire(EntityId(2), loc(2, 20), Tick(0));

        let mut vue = annuaire.instantane();
        vue.sort_by_key(|(e, _)| e.0);
        assert_eq!(
            vue,
            vec![(EntityId(1), loc(1, 10)), (EntityId(2), loc(2, 20))]
        );
    }

    #[test]
    fn annuaire_partageable_entre_threads() {
        use std::sync::Arc;

        let annuaire = Arc::new(Directory::avec_hysteresis(Hysteresis::aucune()));
        let fils: Vec<_> = (0..4u32)
            .map(|i| {
                let annuaire = Arc::clone(&annuaire);
                std::thread::spawn(move || {
                    for t in 0..100u64 {
                        let _ = annuaire.migrer(EntityId(i), loc(i, t as u32 % 7), Tick(t));
                    }
                })
            })
            .collect();
        for f in fils {
            f.join().unwrap();
        }
        assert_eq!(annuaire.len(), 4);
    }
}

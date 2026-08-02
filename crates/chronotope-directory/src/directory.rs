use chronotope_core::{CellId, EntityId};

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

/// Annuaire entite -> emplacement courant, avec migration a hysteresis.
///
/// IMPLEMENTATION A FAIRE ICI. Voir le module doc de `lib.rs` pour le
/// contrat. Points imposes :
///
/// - `migrer` doit etre O(1) (une ecriture d'annuaire, §5.3) — jamais
///   proportionnel au nombre d'entites deja presentes dans la cellule
///   destination.
/// - L'hysteresis doit etre un parametre explicite du `Directory` (pas une
///   constante compilee en dur), pour permettre au harnais de simulation de
///   `chronotope-sim` de la faire varier et de mesurer le thrashing en
///   fonction de son reglage.
pub struct Directory {
    _todo: (),
}

impl Directory {
    pub fn new() -> Self {
        todo!("Directory::new — voir le contrat dans lib.rs")
    }

    pub fn locate(&self, _entity: EntityId) -> Option<Location> {
        todo!("Directory::locate")
    }

    /// Ecrit le nouvel emplacement. Ne transfere AUCUN etat — c'est le point
    /// central de §5.3. Doit refuser (ou ignorer, a documenter et tester) une
    /// migration qui violerait l'hysteresis configuree.
    pub fn migrer(&self, _entity: EntityId, _dest: Location) {
        todo!("Directory::migrer")
    }
}

impl Default for Directory {
    fn default() -> Self {
        Self::new()
    }
}

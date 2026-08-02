//! Ou est quoi : position -> cellule (grille reguliere sur le plan XZ) et
//! cellule -> noeud (placement modulo). `chronotope-core` ne prescrit
//! deliberement aucune fonction de decoupage (voir la doc de `CellId`) ; le
//! harnais en fixe une, minimale et deterministe, pour que les trajectoires
//! de test soient rejouables a l'identique.

use chronotope_core::CellId;

/// Grille reguliere carree. Le plan XZ porte le decoupage, Y est ignore :
/// c'est le decoupage de terrain de PawChat (`cell_of_position`), ou la
/// hauteur ne partitionne rien.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grille {
    taille_cellule: f32,
    largeur: u32,
}

impl Grille {
    /// # Panics
    /// Si `taille_cellule` n'est pas strictement positive ou si `largeur` est
    /// nulle — une violation d'invariant appelant, pas un etat metier.
    pub fn new(taille_cellule: f32, largeur: u32) -> Self {
        assert!(
            taille_cellule > 0.0,
            "taille de cellule strictement positive attendue"
        );
        assert!(
            largeur > 0,
            "grille d'au moins une cellule de cote attendue"
        );
        Self {
            taille_cellule,
            largeur,
        }
    }

    /// Cellule contenant `pos`. Une position hors domaine est ramenee au bord
    /// plutot que de paniquer : une trajectoire de test doit pouvoir sortir du
    /// monde sans faire tomber la simulation, et le harnais n'est pas une
    /// frontiere de confiance au sens de `docs/conventions.md`.
    pub fn cellule(&self, pos: [f32; 3]) -> CellId {
        CellId(self.axe(pos[2]) * self.largeur + self.axe(pos[0]))
    }

    fn axe(&self, v: f32) -> u32 {
        let i = (v / self.taille_cellule).floor();
        if i < 0.0 {
            0
        } else if i >= self.largeur as f32 {
            self.largeur - 1
        } else {
            i as u32
        }
    }

    pub fn taille_cellule(&self) -> f32 {
        self.taille_cellule
    }

    pub fn largeur(&self) -> u32 {
        self.largeur
    }
}

impl Default for Grille {
    fn default() -> Self {
        Self::new(10.0, 16)
    }
}

/// Noeud proprietaire d'une cellule. Modulo plutot que blocs contigus : deux
/// cellules voisines tombent sur deux noeuds differents, donc le moindre
/// franchissement de frontiere est aussi un franchissement de noeud — c'est
/// exactement le cas que H3 doit stresser.
///
/// # Panics
/// Si `nb_noeuds` est nul.
pub fn noeud_de_cellule(cell: CellId, nb_noeuds: usize) -> u32 {
    assert!(nb_noeuds > 0, "au moins un noeud attendu");
    (cell.0 as usize % nb_noeuds) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cellule_suit_le_decoupage_regulier() {
        let g = Grille::new(10.0, 8);
        assert_eq!(g.cellule([0.0, 0.0, 0.0]), CellId(0));
        assert_eq!(g.cellule([9.99, 0.0, 0.0]), CellId(0));
        assert_eq!(g.cellule([10.0, 0.0, 0.0]), CellId(1));
        assert_eq!(g.cellule([0.0, 0.0, 10.0]), CellId(8));
        assert_eq!(g.cellule([25.0, 0.0, 35.0]), CellId(3 * 8 + 2));
    }

    #[test]
    fn hors_domaine_ramene_au_bord_sans_paniquer() {
        let g = Grille::new(10.0, 8);
        assert_eq!(g.cellule([-1000.0, 0.0, -1000.0]), CellId(0));
        assert_eq!(g.cellule([1e9, 0.0, 1e9]), CellId(7 * 8 + 7));
        assert_eq!(g.cellule([f32::NAN, 0.0, f32::NAN]), CellId(0));
    }

    #[test]
    fn cellules_voisines_tombent_sur_des_noeuds_differents() {
        assert_ne!(
            noeud_de_cellule(CellId(1), 2),
            noeud_de_cellule(CellId(2), 2)
        );
    }
}

//! Dilatation temporelle PAR CELLULE (§9 de `docs/chronotope.md`) : quand le
//! budget d'une cellule est depasse, sa cadence de scellement descend par
//! paliers (20 -> 10 -> 5 Hz, Δ 100 -> 200 -> 400 ms) au lieu de refuser des
//! entrees. C'est le mecanisme que H4 mesure, et la propriete qui compte est
//! qu'il est indexe par cellule et par rien d'autre : aucune fonction de ce
//! module ne voit l'etat global de la simulation.

/// Palier de degradation d'une cellule. L'ordre `Nominal < Demi < Quart` est
/// l'ordre de gravite : `max` de deux paliers est le plus degrade des deux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Palier {
    /// 20 Hz, Δ = 100 ms — le regime nominal.
    Nominal,
    /// 10 Hz, Δ = 200 ms.
    Demi,
    /// 5 Hz, Δ = 400 ms — le plancher, jamais un refus d'ecriture.
    Quart,
}

impl Palier {
    /// Nombre de ticks de simulation regroupes dans un seul chronotope scelle.
    /// C'est la seule chose que la degradation change : la granularite
    /// temporelle du sceau, jamais l'acceptation d'une ecriture.
    pub fn diviseur(self) -> u64 {
        match self {
            Palier::Nominal => 1,
            Palier::Demi => 2,
            Palier::Quart => 4,
        }
    }

    pub fn hz(self, hz_nominale: f64) -> f64 {
        hz_nominale / self.diviseur() as f64
    }
}

/// Budget d'une cellule et paliers associes. Les seuils sont des parametres
/// runtime, jamais des constantes compilees : le harnais doit pouvoir les
/// faire varier pour tracer la courbe de degradation que H4 demande.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolitiqueCadence {
    /// Cadence de la boucle de simulation elle-meme, en Hz. Le corps propre
    /// du joueur n'est jamais ralenti (§9 point 2) : seule la cadence de
    /// scellement descend en dessous.
    pub hz_nominale: f64,
    /// Horizon Δ au palier nominal, exprime en ticks de simulation. 2 ticks a
    /// 20 Hz = 100 ms, la valeur par defaut de `chronotope_core::Horizon`.
    pub horizon_nominal_ticks: u64,
    /// Nombre d'entites dans la cellule a partir duquel on passe a 10 Hz.
    pub seuil_demi: usize,
    /// Nombre d'entites dans la cellule a partir duquel on passe a 5 Hz.
    pub seuil_quart: usize,
}

impl Default for PolitiqueCadence {
    fn default() -> Self {
        Self {
            hz_nominale: 20.0,
            horizon_nominal_ticks: 2,
            seuil_demi: 16,
            seuil_quart: 32,
        }
    }
}

impl PolitiqueCadence {
    /// Palier d'une cellule d'apres SA charge seule. Monotone meme si les
    /// seuils sont configures dans le desordre.
    pub fn palier(&self, charge: usize) -> Palier {
        if charge >= self.seuil_quart.max(self.seuil_demi) {
            Palier::Quart
        } else if charge >= self.seuil_demi {
            Palier::Demi
        } else {
            Palier::Nominal
        }
    }

    /// Δ de la cellule, en ticks. Croit avec le palier : sceller moins souvent
    /// implique mecaniquement d'attendre plus longtemps avant de sceller.
    pub fn horizon_ticks(&self, palier: Palier) -> u64 {
        self.horizon_nominal_ticks * palier.diviseur()
    }

    /// Tick du chronotope dans lequel tombe le tick de simulation `tick` pour
    /// une cellule a ce palier. Au palier nominal c'est l'identite ; degrade,
    /// plusieurs ticks de simulation partagent un meme chronotope (LWW, §1
    /// point 1 — le dernier ecrivain du groupe gagne, ce qui est correct
    /// parce qu'il n'y a qu'un seul ecrivain par entite).
    pub fn tick_de_sceau(&self, palier: Palier, tick: u64) -> u64 {
        let d = palier.diviseur();
        tick - tick % d
    }

    pub fn periode_ms(&self) -> f64 {
        1000.0 / self.hz_nominale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_paliers_suivent_le_document() {
        let p = PolitiqueCadence::default();
        assert_eq!(Palier::Nominal.hz(p.hz_nominale), 20.0);
        assert_eq!(Palier::Demi.hz(p.hz_nominale), 10.0);
        assert_eq!(Palier::Quart.hz(p.hz_nominale), 5.0);
        assert_eq!(p.horizon_ticks(Palier::Nominal), 2);
        assert_eq!(p.horizon_ticks(Palier::Demi), 4);
        assert_eq!(p.horizon_ticks(Palier::Quart), 8);
    }

    #[test]
    fn le_palier_ne_depend_que_de_la_charge_de_la_cellule() {
        let p = PolitiqueCadence::default();
        assert_eq!(p.palier(0), Palier::Nominal);
        assert_eq!(p.palier(15), Palier::Nominal);
        assert_eq!(p.palier(16), Palier::Demi);
        assert_eq!(p.palier(31), Palier::Demi);
        assert_eq!(p.palier(32), Palier::Quart);
        assert_eq!(p.palier(10_000), Palier::Quart);
    }

    #[test]
    fn un_sceau_degrade_regroupe_des_ticks_contigus() {
        let p = PolitiqueCadence::default();
        for t in 0..16u64 {
            assert_eq!(p.tick_de_sceau(Palier::Nominal, t), t);
            assert_eq!(p.tick_de_sceau(Palier::Demi, t), t - t % 2);
            assert_eq!(p.tick_de_sceau(Palier::Quart, t), t - t % 4);
        }
    }

    #[test]
    fn un_chronotope_n_est_jamais_scelle_avant_sa_derniere_ecriture() {
        let p = PolitiqueCadence::default();
        for palier in [Palier::Nominal, Palier::Demi, Palier::Quart] {
            let derniere_ecriture = palier.diviseur() - 1;
            assert!(p.horizon_ticks(palier) > derniere_ecriture);
        }
    }
}

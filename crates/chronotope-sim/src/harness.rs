//! Le harnais lui-meme : il avance des entites sur leurs trajectoires, arbitre
//! leur cellule d'autorite via le routeur, ecrit leur pose dans le store du
//! noeud proprietaire, scelle a l'horizon de la cellule, et compte les
//! artefacts que H3 et H4 exigent de mesurer.
//!
//! ## Deux regles non evidentes, et pourquoi elles sont la
//!
//! **1. La geometrie ne fait pas autorite, l'annuaire fait autorite.** A chaque
//! tick le harnais calcule la cellule geometrique de l'entite, demande la
//! migration, puis RELIT l'annuaire et ecrit la pose la ou l'annuaire l'a
//! placee — pas la ou la geometrie la voudrait. Une migration refusee par
//! l'hysteresis (§4) laisse donc l'entite inscrite dans son ancienne cellule,
//! ce qui se paie en latence de bascule et jamais en duplication. C'est la
//! seule discipline qui rend "une entite, une inscription par tick" vrai par
//! construction plutot que par chance.
//!
//! **2. Une bascule de cellule attend une frontiere de chronotope.** Sous
//! degradation (§9) plusieurs ticks de simulation partagent un meme chronotope.
//! Si une entite changeait de cellule au milieu d'un tel groupe, elle
//! apparaitrait dans DEUX chronotopes scelles portant le meme tick, sur deux
//! noeuds differents — exactement le glitch que H3 doit exclure. Le harnais
//! n'adopte donc la nouvelle cellule que lorsque le tick de sceau de la
//! destination depasse strictement le dernier tick de sceau deja ecrit. Cela
//! transpose litteralement la regle du §4 ("noeud 1 cesse a T+1, noeud 2
//! commence a T+1") a une echelle de temps ou le tick de sceau, pas le tick de
//! simulation, est l'unite observable.
//!
//! Corollaire du meme raisonnement : le palier d'une cellule ne peut changer
//! qu'aux frontieres du palier le plus grossier (tous les 4 ticks), sans quoi
//! un chronotope deja ouvert serait redecoupe et une ecriture tomberait dans un
//! chronotope deja scelle. Cette contrainte est ce qui permet d'affirmer que la
//! degradation ne refuse JAMAIS une ecriture — la propriete meme que H4 oppose
//! au plafond dur.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick,
};

use crate::cadence::{Palier, PolitiqueCadence};
use crate::routage::{AnnuaireMemoire, Emplacement, Routeur};
use crate::topologie::{noeud_de_cellule, Grille};

/// Parametres d'une simulation. Tout y est explicite : deux executions avec la
/// meme config et les memes trajectoires produisent le meme rapport, au bit
/// pres — un harnais de mesure non deterministe ne mesure rien.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimConfig {
    pub room: RoomId,
    pub grille: Grille,
    pub cadence: PolitiqueCadence,
    pub nb_noeuds: usize,
    /// Hysteresis de l'annuaire construit par [`MultiNodeSim::new`]. Ignore si
    /// le routeur est injecte via [`MultiNodeSim::avec`].
    pub hysteresis_ticks: u32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            room: RoomId(0),
            grille: Grille::default(),
            cadence: PolitiqueCadence::default(),
            nb_noeuds: 2,
            hysteresis_ticks: 3,
        }
    }
}

/// Resultat d'une simulation. Chaque champ est une mesure, pas un indicateur
/// de bonne sante : `duplications = 0` ne veut rien dire si `ticks = 0`, et le
/// rapport expose donc aussi de quoi juger de la representativite de la
/// mesure.
#[derive(Debug, Clone, Default)]
pub struct SimReport {
    /// Nombre de fois qu'une entite s'est retrouvee inscrite dans plus d'une
    /// cellule pour un MEME tick de sceau. Definition volontairement plus
    /// large que celle du module doc de `lib.rs` ("deux noeuds differents") :
    /// une duplication intra-noeud est deja un glitch.
    pub duplications: u32,
    /// Sous-ensemble strict de `duplications` : les cas ou les deux
    /// inscriptions concurrentes vivent sur deux noeuds differents. C'est la
    /// forme exacte visee par H3.
    pub duplications_inter_noeuds: u32,
    /// Ticks de simulation pendant lesquels une entite suivie n'a obtenu
    /// aucune inscription (ecriture rejetee parce que le chronotope cible
    /// etait deja scelle).
    pub pertes: u32,
    /// Une entree par bascule de cellule effectivement observee : delai entre
    /// la sortie geometrique de l'ancienne cellule et la premiere inscription
    /// dans la nouvelle.
    pub latence_bascule_ms: Vec<f64>,
    /// Cadence de scellement mesuree, par cellule : sceaux emis divises par la
    /// duree simulee. C'est la grandeur que H4 compare a 20/10/5 Hz.
    pub cadence_effective_hz_par_cellule: Vec<(CellId, f64)>,
    /// Sceaux bruts par cellule. Permet de recalculer une cadence sur une
    /// FENETRE (par difference entre deux rapports) plutot que depuis le debut
    /// de la simulation — indispensable pour observer une charge croissante.
    pub sceaux_par_cellule: Vec<(CellId, u32)>,
    /// Palier le plus degrade atteint par chaque cellule. `Nominal` ici prouve
    /// qu'une cellule n'a jamais ete penalisee — c'est la mesure de localite du
    /// §9 point 1.
    pub palier_max_par_cellule: Vec<(CellId, Palier)>,
    pub ticks: u64,
    pub entites_suivies: usize,
    pub ecritures_acceptees: u64,
    /// Doit rester nul : la degradation ralentit le scellement, elle ne refuse
    /// pas d'entree (§9). Un rapport non nul ici refute la moitie de H4.
    pub ecritures_rejetees: u64,
    pub migrations_tentees: u64,
    pub migrations_accordees: u64,
    /// Bascules amorcees puis annulees parce que l'entite est revenue dans sa
    /// cellule d'origine avant que l'hysteresis ne cede : le thrashing evite,
    /// mesure plutot que suppose.
    pub bascules_evitees: u64,
}

impl SimReport {
    /// Latence de bascule moyenne, ou `None` si aucune bascule n'a eu lieu —
    /// jamais 0.0, qui se lirait comme "instantane" alors qu'il signifierait
    /// "rien mesure".
    pub fn latence_bascule_moyenne_ms(&self) -> Option<f64> {
        if self.latence_bascule_ms.is_empty() {
            return None;
        }
        Some(self.latence_bascule_ms.iter().sum::<f64>() / self.latence_bascule_ms.len() as f64)
    }

    pub fn latence_bascule_max_ms(&self) -> Option<f64> {
        self.latence_bascule_ms
            .iter()
            .copied()
            .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.max(v))))
    }

    pub fn cadence_de(&self, cell: CellId) -> Option<f64> {
        self.cadence_effective_hz_par_cellule
            .iter()
            .find(|(c, _)| *c == cell)
            .map(|(_, hz)| *hz)
    }

    pub fn sceaux_de(&self, cell: CellId) -> u32 {
        self.sceaux_par_cellule
            .iter()
            .find(|(c, _)| *c == cell)
            .map_or(0, |(_, n)| *n)
    }

    pub fn palier_max_de(&self, cell: CellId) -> Palier {
        self.palier_max_par_cellule
            .iter()
            .find(|(c, _)| *c == cell)
            .map_or(Palier::Nominal, |(_, p)| *p)
    }
}

struct EntiteSuivie {
    id: EntityId,
    trajectoire: fn(u64) -> [f32; 3],
    /// Derniere inscription reussie : (emplacement, tick de sceau). Sert a la
    /// regle 2 du module doc — on ne change de cellule qu'en franchissant une
    /// frontiere de chronotope.
    ecriture: Option<(Emplacement, u64)>,
    /// Bascule en cours : (cellule geometrique visee, tick de sortie).
    attente: Option<(CellId, u64)>,
}

/// Simulation multi-noeud en memoire.
///
/// Generique sur le store et le routeur pour une raison de methode, pas de
/// gout : la logique du harnais doit pouvoir etre validee contre des doubles
/// deterministes, independamment de l'avancement de `ChronotopeEngine` et de
/// `Directory`. Les parametres par defaut donnent le montage reel.
pub struct MultiNodeSim<S = ChronotopeEngine, R = AnnuaireMemoire> {
    config: SimConfig,
    noeuds: Vec<S>,
    routeur: R,
    suivies: Vec<EntiteSuivie>,
    tick: u64,

    charge_precedente: HashMap<CellId, usize>,
    palier_effectif: HashMap<CellId, Palier>,
    palier_max: HashMap<CellId, Palier>,
    buckets_ouverts: HashMap<CellId, BTreeSet<u64>>,
    sceaux: HashMap<CellId, u32>,
    inscriptions: HashMap<(EntityId, u64), Vec<Emplacement>>,

    duplications: u32,
    duplications_inter_noeuds: u32,
    pertes: u32,
    latences_ticks: Vec<u64>,
    ecritures_acceptees: u64,
    ecritures_rejetees: u64,
    migrations_tentees: u64,
    migrations_accordees: u64,
    bascules_evitees: u64,
}

impl MultiNodeSim<ChronotopeEngine, AnnuaireMemoire> {
    /// Montage reel : un `ChronotopeEngine` par noeud, annuaire en memoire.
    ///
    /// # Panics
    /// Si `nb_noeuds` est nul.
    pub fn new(nb_noeuds: usize) -> Self {
        Self::avec_config(SimConfig {
            nb_noeuds,
            ..SimConfig::default()
        })
    }

    /// # Panics
    /// Si `config.nb_noeuds` est nul.
    pub fn avec_config(config: SimConfig) -> Self {
        let horizon = Horizon(Duration::from_secs_f64(
            config.cadence.horizon_nominal_ticks as f64 / config.cadence.hz_nominale,
        ));
        let noeuds = (0..config.nb_noeuds)
            .map(|_| ChronotopeEngine::new(horizon))
            .collect();
        let routeur = AnnuaireMemoire::new(config.hysteresis_ticks);
        Self::avec(config, noeuds, routeur)
    }
}

impl<S: ChronotopeStore, R: Routeur> MultiNodeSim<S, R> {
    /// Montage explicite. Le harnais pilote lui-meme le scellement (§10 :
    /// "sceller ... declenche par l'horloge") ; l'`Horizon` interne d'un store
    /// n'est donc jamais consulte ici, c'est la politique de cadence de la
    /// config qui fait foi.
    ///
    /// # Panics
    /// Si `noeuds` est vide ou de taille differente de `config.nb_noeuds`.
    pub fn avec(config: SimConfig, noeuds: Vec<S>, routeur: R) -> Self {
        assert!(!noeuds.is_empty(), "au moins un noeud attendu");
        assert_eq!(
            noeuds.len(),
            config.nb_noeuds,
            "nb_noeuds doit decrire la liste de noeuds fournie"
        );
        Self {
            config,
            noeuds,
            routeur,
            suivies: Vec::new(),
            tick: 0,
            charge_precedente: HashMap::new(),
            palier_effectif: HashMap::new(),
            palier_max: HashMap::new(),
            buckets_ouverts: HashMap::new(),
            sceaux: HashMap::new(),
            inscriptions: HashMap::new(),
            duplications: 0,
            duplications_inter_noeuds: 0,
            pertes: 0,
            latences_ticks: Vec::new(),
            ecritures_acceptees: 0,
            ecritures_rejetees: 0,
            migrations_tentees: 0,
            migrations_accordees: 0,
            bascules_evitees: 0,
        }
    }

    /// Enregistre une entite et la trajectoire qu'elle suivra. Une trajectoire
    /// est une fonction pure du tick : c'est ce qui rend une simulation
    /// rejouable a l'identique, donc utilisable comme test de non-regression et
    /// pas seulement comme demonstration.
    pub fn track(&mut self, entity: EntityId, trajectory: fn(u64) -> [f32; 3]) {
        self.suivies.push(EntiteSuivie {
            id: entity,
            trajectoire: trajectory,
            ecriture: None,
            attente: None,
        });
    }

    pub fn executer(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Fait avancer la simulation d'un tick pour toutes les entites suivies.
    #[tracing::instrument(level = "trace", skip(self), fields(tick = self.tick))]
    pub fn step(&mut self) {
        let t = self.tick;
        self.reevaluer_paliers(t);

        let room = self.config.room;
        let mut charge: HashMap<CellId, usize> = HashMap::new();
        let mut suivies = std::mem::take(&mut self.suivies);

        for e in suivies.iter_mut() {
            let pos = (e.trajectoire)(t);
            let cellule_geo = self.config.grille.cellule(pos);
            let emplacement_geo = Emplacement {
                noeud: noeud_de_cellule(cellule_geo, self.config.nb_noeuds),
                cellule: cellule_geo,
            };
            let autorite = self.arbitrer(e.id, cellule_geo, emplacement_geo);
            let cible = self.differer_jusqu_a_la_frontiere(e, autorite, t);

            self.compter_bascule(e, cible.cellule, cellule_geo, t);

            let bucket = self
                .config
                .cadence
                .tick_de_sceau(self.palier_de(cible.cellule), t);
            let pose = Pose {
                pos,
                yaw: cap(e.trajectoire, t),
            };
            let resultat = self.noeuds[cible.noeud as usize].ecrire(
                room,
                cible.cellule,
                Tick(bucket),
                e.id,
                pose,
            );

            match resultat {
                Ok(()) => {
                    self.ecritures_acceptees += 1;
                    self.buckets_ouverts
                        .entry(cible.cellule)
                        .or_default()
                        .insert(bucket);
                    self.enregistrer_inscription(e.id, bucket, cible);
                    e.ecriture = Some((cible, bucket));
                }
                Err(err) => {
                    self.ecritures_rejetees += 1;
                    self.pertes += 1;
                    tracing::warn!(
                        entity = e.id.0,
                        cellule = cible.cellule.0,
                        tick_de_sceau = bucket,
                        erreur = %err,
                        "ecriture rejetee — tick sans inscription pour cette entite"
                    );
                }
            }

            *charge.entry(cible.cellule).or_default() += 1;
        }

        self.suivies = suivies;
        self.sceller_les_chronotopes_expires(t);
        self.charge_precedente = charge;
        self.oublier_les_inscriptions_closes(t);
        self.tick += 1;
    }

    /// Rapport honnete de ce qui s'est passe jusqu'ici. Appelable a tout
    /// moment : c'est en differenciant deux rapports qu'on obtient une cadence
    /// sur une fenetre plutot que depuis l'origine.
    pub fn report(&self) -> SimReport {
        let secondes = self.tick as f64 / self.config.cadence.hz_nominale;
        let periode_ms = self.config.cadence.periode_ms();

        let mut cellules: Vec<CellId> = self.sceaux.keys().copied().collect();
        cellules.extend(self.palier_max.keys().copied());
        cellules.sort_by_key(|c| c.0);
        cellules.dedup();

        let cadence_effective_hz_par_cellule = cellules
            .iter()
            .map(|c| {
                let n = f64::from(self.sceaux.get(c).copied().unwrap_or(0));
                (*c, if secondes > 0.0 { n / secondes } else { 0.0 })
            })
            .collect();
        let sceaux_par_cellule = cellules
            .iter()
            .map(|c| (*c, self.sceaux.get(c).copied().unwrap_or(0)))
            .collect();
        let palier_max_par_cellule = cellules
            .iter()
            .map(|c| {
                (
                    *c,
                    self.palier_max.get(c).copied().unwrap_or(Palier::Nominal),
                )
            })
            .collect();

        SimReport {
            duplications: self.duplications,
            duplications_inter_noeuds: self.duplications_inter_noeuds,
            pertes: self.pertes,
            latence_bascule_ms: self
                .latences_ticks
                .iter()
                .map(|n| *n as f64 * periode_ms)
                .collect(),
            cadence_effective_hz_par_cellule,
            sceaux_par_cellule,
            palier_max_par_cellule,
            ticks: self.tick,
            entites_suivies: self.suivies.len(),
            ecritures_acceptees: self.ecritures_acceptees,
            ecritures_rejetees: self.ecritures_rejetees,
            migrations_tentees: self.migrations_tentees,
            migrations_accordees: self.migrations_accordees,
            bascules_evitees: self.bascules_evitees,
        }
    }

    pub fn tick_courant(&self) -> u64 {
        self.tick
    }

    pub fn config(&self) -> &SimConfig {
        &self.config
    }

    /// Acces aux stores pour les assertions qui verifient le CONTENU scelle,
    /// pas seulement les compteurs tenus par le harnais.
    pub fn noeuds(&self) -> &[S] {
        &self.noeuds
    }

    pub fn palier_courant_de(&self, cell: CellId) -> Palier {
        self.palier_de(cell)
    }

    fn arbitrer(
        &mut self,
        entity: EntityId,
        cellule_geo: CellId,
        emplacement_geo: Emplacement,
    ) -> Emplacement {
        match self.routeur.ou_est(entity) {
            None => {
                self.routeur.migrer(entity, emplacement_geo);
                self.routeur.ou_est(entity).unwrap_or(emplacement_geo)
            }
            Some(actuel) if actuel.cellule != cellule_geo => {
                self.migrations_tentees += 1;
                if self.routeur.migrer(entity, emplacement_geo) {
                    self.migrations_accordees += 1;
                }
                self.routeur.ou_est(entity).unwrap_or(actuel)
            }
            Some(actuel) => actuel,
        }
    }

    fn differer_jusqu_a_la_frontiere(
        &self,
        e: &EntiteSuivie,
        autorite: Emplacement,
        t: u64,
    ) -> Emplacement {
        match e.ecriture {
            Some((precedent, bucket_precedent)) if precedent != autorite => {
                let bucket_candidat = self
                    .config
                    .cadence
                    .tick_de_sceau(self.palier_de(autorite.cellule), t);
                if bucket_candidat > bucket_precedent {
                    autorite
                } else {
                    precedent
                }
            }
            _ => autorite,
        }
    }

    fn compter_bascule(
        &mut self,
        e: &mut EntiteSuivie,
        cellule_ecrite: CellId,
        cellule_geo: CellId,
        t: u64,
    ) {
        if cellule_ecrite != cellule_geo {
            let redemarrer = match e.attente {
                None => true,
                Some((visee, _)) => visee != cellule_geo,
            };
            if redemarrer {
                e.attente = Some((cellule_geo, t));
            }
        } else if let Some((visee, sortie)) = e.attente.take() {
            if visee == cellule_geo {
                self.latences_ticks.push(t - sortie);
            } else {
                self.bascules_evitees += 1;
            }
        }
    }

    fn enregistrer_inscription(&mut self, entity: EntityId, bucket: u64, cible: Emplacement) {
        let places = self.inscriptions.entry((entity, bucket)).or_default();
        if places.contains(&cible) {
            return;
        }
        if let Some(premier) = places.first().copied() {
            self.duplications += 1;
            if places.iter().any(|p| p.noeud != cible.noeud) {
                self.duplications_inter_noeuds += 1;
            }
            tracing::error!(
                entity = entity.0,
                tick_de_sceau = bucket,
                cellule_a = premier.cellule.0,
                cellule_b = cible.cellule.0,
                "duplication : la meme entite inscrite dans deux chronotopes du meme tick"
            );
        }
        places.push(cible);
    }

    /// Le palier d'une cellule ne bouge qu'aux frontieres du palier le plus
    /// grossier — voir le corollaire du module doc : c'est cette contrainte qui
    /// interdit qu'un chronotope deja ouvert soit redecoupe, donc qu'une
    /// ecriture tombe dans un chronotope deja scelle.
    fn reevaluer_paliers(&mut self, t: u64) {
        if !t.is_multiple_of(Palier::Quart.diviseur()) {
            return;
        }
        let mut nouveau = HashMap::with_capacity(self.charge_precedente.len());
        for (cell, charge) in &self.charge_precedente {
            let palier = self.config.cadence.palier(*charge);
            if self
                .palier_effectif
                .get(cell)
                .copied()
                .unwrap_or(Palier::Nominal)
                != palier
            {
                if palier == Palier::Nominal {
                    tracing::debug!(
                        cellule = cell.0,
                        charge = *charge,
                        "cellule revenue a la cadence nominale"
                    );
                } else {
                    tracing::warn!(
                        cellule = cell.0,
                        charge = *charge,
                        hz = palier.hz(self.config.cadence.hz_nominale),
                        "cadence de scellement degradee pour cette cellule"
                    );
                }
            }
            nouveau.insert(*cell, palier);
            let max = self.palier_max.entry(*cell).or_insert(Palier::Nominal);
            *max = (*max).max(palier);
        }
        self.palier_effectif = nouveau;
    }

    fn palier_de(&self, cell: CellId) -> Palier {
        self.palier_effectif
            .get(&cell)
            .copied()
            .unwrap_or(Palier::Nominal)
    }

    fn sceller_les_chronotopes_expires(&mut self, t: u64) {
        let mut a_sceller: Vec<(CellId, u64)> = Vec::new();
        for (cell, buckets) in &self.buckets_ouverts {
            let horizon = self.config.cadence.horizon_ticks(self.palier_de(*cell));
            for bucket in buckets {
                if bucket + horizon <= t {
                    a_sceller.push((*cell, *bucket));
                } else {
                    break;
                }
            }
        }

        let room = self.config.room;
        for (cell, bucket) in a_sceller {
            let noeud = noeud_de_cellule(cell, self.config.nb_noeuds) as usize;
            self.noeuds[noeud].sceller(room, cell, Tick(bucket));
            if let Some(buckets) = self.buckets_ouverts.get_mut(&cell) {
                buckets.remove(&bucket);
            }
            *self.sceaux.entry(cell).or_default() += 1;
            tracing::debug!(
                room = room.0,
                cellule = cell.0,
                tick = bucket,
                noeud,
                "chronotope scelle"
            );
        }
    }

    /// Un tick de sceau ne peut plus recevoir d'ecriture passe le palier le
    /// plus grossier : au-dela, memoriser ses inscriptions ne sert plus a
    /// detecter une duplication, seulement a faire grossir la table.
    fn oublier_les_inscriptions_closes(&mut self, t: u64) {
        let seuil = t.saturating_sub(2 * Palier::Quart.diviseur());
        self.inscriptions.retain(|(_, bucket), _| *bucket >= seuil);
    }
}

/// Lacet deduit du deplacement entre deux ticks : une pose complete et
/// coherente avec la trajectoire, sans etat a porter d'un tick a l'autre.
fn cap(trajectoire: fn(u64) -> [f32; 3], t: u64) -> f32 {
    let ici = trajectoire(t);
    let avant = trajectoire(t.saturating_sub(1));
    let (dx, dz) = (ici[0] - avant[0], ici[2] - avant[2]);
    if dx == 0.0 && dz == 0.0 {
        0.0
    } else {
        dz.atan2(dx)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::double::StoreMemoire;

    const CELLULE_A: CellId = CellId(0);
    const CELLULE_B: CellId = CellId(1);

    fn sim(config: SimConfig, hysteresis: u32) -> MultiNodeSim<StoreMemoire, AnnuaireMemoire> {
        let noeuds = (0..config.nb_noeuds).map(|_| StoreMemoire::new()).collect();
        MultiNodeSim::avec(config, noeuds, AnnuaireMemoire::new(hysteresis))
    }

    /// Va-et-vient sur l'axe X : traverse les frontieres x=10 et x=20, donc les
    /// cellules 0, 1 et 2, donc (placement modulo, 2 noeuds) les deux noeuds.
    fn va_et_vient(t: u64) -> [f32; 3] {
        let phase = t as f32 * std::f32::consts::TAU / 16.0;
        [15.0 + 9.0 * phase.sin(), 0.0, 5.0]
    }

    /// Le pire cas du §4 : saute d'un cote a l'autre de x=10 a chaque tick.
    fn oscillation_frontaliere(t: u64) -> [f32; 3] {
        if t.is_multiple_of(2) {
            [9.5, 0.0, 5.0]
        } else {
            [10.5, 0.0, 5.0]
        }
    }

    fn dans_a(_t: u64) -> [f32; 3] {
        [5.0, 0.0, 5.0]
    }

    fn dans_b(_t: u64) -> [f32; 3] {
        [15.0, 0.0, 5.0]
    }

    fn rejoint_a_au_tick_100(t: u64) -> [f32; 3] {
        if t >= 100 {
            [5.0, 0.0, 5.0]
        } else {
            [5.0, 0.0, 55.0]
        }
    }

    fn rejoint_a_au_tick_200(t: u64) -> [f32; 3] {
        if t >= 200 {
            [5.0, 0.0, 5.0]
        } else {
            [5.0, 0.0, 75.0]
        }
    }

    /// Verifie l'absence de duplication sur le CONTENU scelle des stores, sans
    /// passer par les compteurs du harnais : une entite ne doit jamais figurer
    /// dans deux chronotopes portant le meme tick.
    fn cellules_par_tick(
        sim: &MultiNodeSim<StoreMemoire, AnnuaireMemoire>,
    ) -> HashMap<(u32, u64), HashSet<u32>> {
        let mut vu: HashMap<(u32, u64), HashSet<u32>> = HashMap::new();
        for noeud in sim.noeuds() {
            for chrono in noeud.scelles() {
                for (entity, _) in &chrono.entities {
                    vu.entry((entity.0, chrono.tick.0))
                        .or_default()
                        .insert(chrono.cell.0);
                }
            }
        }
        vu
    }

    #[test]
    fn h3_traversee_repetee_de_frontiere_sans_duplication_ni_perte() {
        let mut s = sim(SimConfig::default(), 3);
        s.track(EntityId(1), va_et_vient);
        s.executer(320);
        let r = s.report();

        assert_eq!(r.duplications, 0, "duplication observee : {r:?}");
        assert_eq!(r.duplications_inter_noeuds, 0);
        assert_eq!(r.pertes, 0, "tick sans inscription : {r:?}");
        assert_eq!(r.ecritures_rejetees, 0);
        assert_eq!(r.ecritures_acceptees, 320);

        assert!(
            r.latence_bascule_ms.len() >= 30,
            "trajectoire trop pauvre en franchissements : {} bascules",
            r.latence_bascule_ms.len()
        );

        for ((entity, tick), cellules) in cellules_par_tick(&s) {
            assert_eq!(
                cellules.len(),
                1,
                "entite {entity} inscrite dans {cellules:?} au tick {tick}"
            );
        }
    }

    #[test]
    fn h3_chaque_tick_actif_recoit_exactement_une_inscription() {
        let mut s = sim(SimConfig::default(), 3);
        s.track(EntityId(1), va_et_vient);
        s.executer(200);

        let vu = cellules_par_tick(&s);
        // Les 2 derniers ticks sont encore dans l'horizon, donc pas scelles.
        for tick in 0..198u64 {
            assert!(
                vu.contains_key(&(1, tick)),
                "aucune inscription scellee pour l'entite 1 au tick {tick}"
            );
        }
    }

    #[test]
    fn h3_la_latence_de_bascule_est_bornee_par_l_hysteresis() {
        let periode = SimConfig::default().cadence.periode_ms();
        for hysteresis in [1u32, 2, 3, 4] {
            let mut s = sim(SimConfig::default(), hysteresis);
            s.track(EntityId(1), va_et_vient);
            s.executer(320);
            let r = s.report();

            // A hysteresis = 1 la bascule est accordee des la premiere
            // tentative : rien a attendre, donc aucune latence a enregistrer.
            // C'est un resultat, pas une mesure manquante.
            if hysteresis == 1 {
                assert!(r.latence_bascule_ms.is_empty());
                assert!(r.migrations_accordees > 40);
            } else {
                assert!(
                    !r.latence_bascule_ms.is_empty(),
                    "hysteresis {hysteresis} : aucune bascule observee"
                );
            }

            let max = r.latence_bascule_max_ms().unwrap_or(0.0);
            assert!(
                max <= hysteresis.max(1) as f64 * periode,
                "hysteresis {hysteresis} : latence max {max} ms au-dela du budget"
            );
            assert_eq!(r.duplications, 0);
            assert_eq!(r.pertes, 0);
        }
    }

    #[test]
    fn h3_l_hysteresis_amortit_le_thrashing_frontalier() {
        let mut sans = sim(SimConfig::default(), 1);
        sans.track(EntityId(1), oscillation_frontaliere);
        sans.executer(200);
        let r_sans = sans.report();

        let mut avec = sim(SimConfig::default(), 8);
        avec.track(EntityId(1), oscillation_frontaliere);
        avec.executer(200);
        let r_avec = avec.report();

        assert!(
            r_sans.migrations_accordees > 150,
            "temoin sans hysteresis : {} migrations",
            r_sans.migrations_accordees
        );
        assert!(
            r_avec.migrations_accordees * 4 < r_sans.migrations_accordees,
            "hysteresis inefficace : {} contre {}",
            r_avec.migrations_accordees,
            r_sans.migrations_accordees
        );
        assert_eq!(r_sans.duplications, 0);
        assert_eq!(r_avec.duplications, 0);
        assert_eq!(r_sans.pertes, 0);
        assert_eq!(r_avec.pertes, 0);
        assert!(r_avec.bascules_evitees > 0);
    }

    #[test]
    fn h4_la_degradation_est_locale_a_la_cellule_chargee() {
        let mut s = sim(SimConfig::default(), 3);
        for i in 0..40 {
            s.track(EntityId(i), dans_a);
        }
        for i in 100..103 {
            s.track(EntityId(i), dans_b);
        }
        s.executer(200);
        let r = s.report();

        assert_eq!(r.palier_max_de(CELLULE_A), Palier::Quart);
        assert_eq!(
            r.palier_max_de(CELLULE_B),
            Palier::Nominal,
            "la cellule voisine non chargee a ete degradee — H4 refutee"
        );

        let a = r.cadence_de(CELLULE_A).expect("cellule A absente");
        let b = r.cadence_de(CELLULE_B).expect("cellule B absente");
        assert!(
            (a - 5.0).abs() < 0.6,
            "cadence de la cellule chargee : {a} Hz"
        );
        assert!(
            (b - 20.0).abs() < 0.6,
            "cadence de la cellule calme : {b} Hz"
        );

        assert_eq!(
            r.ecritures_rejetees, 0,
            "la degradation a refuse des entrees"
        );
        assert_eq!(r.pertes, 0);
        assert_eq!(r.duplications, 0);
    }

    #[test]
    fn h4_la_cadence_descend_par_paliers_sous_charge_croissante() {
        let mut s = sim(SimConfig::default(), 3);
        for i in 0..4 {
            s.track(EntityId(i), dans_a);
        }
        for i in 4..20 {
            s.track(EntityId(i), rejoint_a_au_tick_100);
        }
        for i in 20..40 {
            s.track(EntityId(i), rejoint_a_au_tick_200);
        }
        for i in 100..103 {
            s.track(EntityId(i), dans_b);
        }

        s.executer(100);
        let r1 = s.report();
        s.executer(100);
        let r2 = s.report();
        s.executer(100);
        let r3 = s.report();

        let hz = |avant: &SimReport, apres: &SimReport, cell: CellId| {
            let sceaux = f64::from(apres.sceaux_de(cell) - avant.sceaux_de(cell));
            sceaux / ((apres.ticks - avant.ticks) as f64 / 20.0)
        };
        let vide = SimReport::default();

        let paliers_a = [
            hz(&vide, &r1, CELLULE_A),
            hz(&r1, &r2, CELLULE_A),
            hz(&r2, &r3, CELLULE_A),
        ];
        assert!(
            (paliers_a[0] - 20.0).abs() < 1.0,
            "palier 1 : {paliers_a:?}"
        );
        assert!(
            (paliers_a[1] - 10.0).abs() < 1.0,
            "palier 2 : {paliers_a:?}"
        );
        assert!((paliers_a[2] - 5.0).abs() < 1.0, "palier 3 : {paliers_a:?}");

        let paliers_b = [
            hz(&vide, &r1, CELLULE_B),
            hz(&r1, &r2, CELLULE_B),
            hz(&r2, &r3, CELLULE_B),
        ];
        for (i, hz_b) in paliers_b.iter().enumerate() {
            assert!(
                (hz_b - 20.0).abs() < 1.0,
                "la cellule calme a decroche sur la fenetre {i} : {paliers_b:?}"
            );
        }

        let r = s.report();
        assert_eq!(r.ecritures_rejetees, 0);
        assert_eq!(r.pertes, 0);
        assert_eq!(r.duplications, 0);
    }

    #[test]
    fn h4_le_palier_intermediaire_existe_vraiment() {
        let mut s = sim(SimConfig::default(), 3);
        for i in 0..20 {
            s.track(EntityId(i), dans_a);
        }
        s.executer(200);
        let r = s.report();
        assert_eq!(r.palier_max_de(CELLULE_A), Palier::Demi);
        let a = r.cadence_de(CELLULE_A).expect("cellule A absente");
        assert!((a - 10.0).abs() < 0.6, "cadence intermediaire : {a} Hz");
    }

    /// Migration ET degradation en meme temps : c'est la combinaison ou la
    /// quantification temporelle du §9 pourrait faire apparaitre une entite
    /// dans deux chronotopes du meme tick, sur deux noeuds. Le cas que le
    /// harnais doit exclure par construction.
    #[test]
    fn h3_sous_h4_migrer_depuis_une_cellule_degradee_ne_duplique_pas() {
        let mut s = sim(SimConfig::default(), 2);
        for i in 0..40 {
            s.track(EntityId(i), dans_a);
        }
        for i in 200..210 {
            s.track(EntityId(i), va_et_vient);
        }
        s.executer(320);
        let r = s.report();

        assert_eq!(r.palier_max_de(CELLULE_A), Palier::Quart);
        assert_eq!(r.duplications, 0, "{r:?}");
        assert_eq!(r.duplications_inter_noeuds, 0);
        assert_eq!(r.pertes, 0);
        assert_eq!(r.ecritures_rejetees, 0);

        for ((entity, tick), cellules) in cellules_par_tick(&s) {
            assert_eq!(
                cellules.len(),
                1,
                "entite {entity} inscrite dans {cellules:?} au tick {tick}"
            );
        }
    }

    /// Balayage aleatoire mais reproductible : melange de trajectoires, de
    /// tailles de population et de reglages d'hysteresis. Les invariants "ni
    /// duplication, ni perte, ni rejet" doivent tenir sur tout le balayage,
    /// pas seulement sur les scenarios ecrits a la main.
    #[test]
    fn invariants_sur_un_balayage_aleatoire_reproductible() {
        let trajectoires: [fn(u64) -> [f32; 3]; 5] = [
            va_et_vient,
            oscillation_frontaliere,
            dans_a,
            dans_b,
            rejoint_a_au_tick_100,
        ];
        let mut rng = StdRng::seed_from_u64(0xC010_0DE5);

        for essai in 0..24 {
            let nb_noeuds = rng.gen_range(1..=4usize);
            let hysteresis = rng.gen_range(0..=6u32);
            let config = SimConfig {
                nb_noeuds,
                ..SimConfig::default()
            };
            let mut s = sim(config, hysteresis);
            let population = rng.gen_range(1..=45u32);
            for i in 0..population {
                s.track(
                    EntityId(i),
                    trajectoires[rng.gen_range(0..trajectoires.len())],
                );
            }
            let ticks = rng.gen_range(40..=250u64);
            s.executer(ticks);
            let r = s.report();

            assert_eq!(
                r.duplications, 0,
                "essai {essai} (noeuds={nb_noeuds}, hysteresis={hysteresis}) : {r:?}"
            );
            assert_eq!(r.pertes, 0, "essai {essai} : {r:?}");
            assert_eq!(r.ecritures_rejetees, 0, "essai {essai} : {r:?}");
            assert_eq!(
                r.ecritures_acceptees,
                ticks * u64::from(population),
                "essai {essai} : une inscription par entite et par tick attendue"
            );
            for ((entity, tick), cellules) in cellules_par_tick(&s) {
                assert_eq!(
                    cellules.len(),
                    1,
                    "essai {essai} : entite {entity} dans {cellules:?} au tick {tick}"
                );
            }
        }
    }

    #[test]
    fn un_rapport_vide_ne_ment_pas() {
        let s = sim(SimConfig::default(), 3);
        let r = s.report();
        assert_eq!(r.ticks, 0);
        assert_eq!(r.latence_bascule_moyenne_ms(), None);
        assert!(r.cadence_effective_hz_par_cellule.is_empty());
    }
}

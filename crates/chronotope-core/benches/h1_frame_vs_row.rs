//! H1 : "l'ecriture par trame reduit le cout d'ecriture dans le regime
//! agglomere". Refutee si le gain mesure est < 3x. Voir `docs/chronotope.md`
//! §H1.
//!
//! Ce que mesure une iteration : UNE trame complete de simulation — chaque
//! avatar ecrit sa pose, puis chaque cellule occupee est scellee. C'est la
//! boucle reelle du §5.2, pas une ecriture isolee : sceller fait partie du
//! cout du modele cellule-tick et doit donc etre paye dans la mesure.
//!
//! Trois regimes, un `criterion_group!` par regime, pour que le rapport HTML
//! les separe sans calcul manuel :
//!
//! - `disperse` : 1 entite par cellule — le regime ou l'AOI gagne deja et ou
//!   Chronotope n'a aucune raison structurelle d'etre tres au-dessus.
//! - `agglomere` : 40 entites dans UNE cellule — le regime que H1 cible.
//! - `social` : 30 / 80 / 200 avatars repartis selon l'agglutination reelle
//!   d'un espace social (une place bondee, quelques groupes, une queue
//!   dispersee), pas un tirage uniforme qui diluerait l'effet mesure.
//!
//! `Throughput::Elements` = nombre d'avatars par trame, donc criterion publie
//! directement des poses/s comparables entre les deux stores.
//!
//! Les stores sont reutilises d'un lot a l'autre et vides HORS mesure
//! (`vider`) : sans cela le temoin a ligne, non borne en memoire, finirait par
//! mesurer la croissance de sa table plutot que son cout d'ecriture — ce
//! serait un avantage artificiel pour le moteur.

use std::time::{Duration, Instant};

use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, NaiveRowStore, Pose, RoomId, Tick,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

const ROOM: RoomId = RoomId(0);
const TRAMES_PAR_LOT: u64 = 64;

const POSE: Pose = Pose {
    pos: [12.5, 1.7, -3.25],
    yaw: 0.75,
};

/// Une trame : (cellule, entite) pour chaque avatar, plus la liste des
/// cellules occupees a sceller.
struct Scene {
    avatars: Vec<(CellId, EntityId)>,
    cellules: Vec<CellId>,
}

impl Scene {
    fn new(avatars: Vec<(CellId, EntityId)>) -> Self {
        let mut cellules: Vec<CellId> = avatars.iter().map(|(c, _)| *c).collect();
        cellules.sort_unstable();
        cellules.dedup();
        Self { avatars, cellules }
    }

    /// 1 avatar par cellule.
    fn dispersee(n: u32) -> Self {
        Self::new((0..n).map(|i| (CellId(i), EntityId(i))).collect())
    }

    /// Tous les avatars dans la meme cellule.
    fn agglomeree(n: u32) -> Self {
        Self::new((0..n).map(|i| (CellId(0), EntityId(i))).collect())
    }

    /// Agglutination sociale : ~40 % sur la place centrale, ~20 % et ~15 % sur
    /// deux points d'interet, ~10 % en petit groupe, le reste disperse. C'est
    /// la forme que decrit le §5.1 du document source ("l'agglutination est le
    /// comportement normal d'un espace social"), pas une loi mesuree.
    fn sociale(n: u32) -> Self {
        let parts = [
            (CellId(0), 40),
            (CellId(1), 20),
            (CellId(2), 15),
            (CellId(3), 10),
        ];
        let mut avatars = Vec::with_capacity(n as usize);
        let mut suivant = 0u32;
        for (cellule, pourcent) in parts {
            let combien = n * pourcent / 100;
            for _ in 0..combien {
                avatars.push((cellule, EntityId(suivant)));
                suivant += 1;
            }
        }
        let mut cellule_libre = 16u32;
        while suivant < n {
            avatars.push((CellId(cellule_libre), EntityId(suivant)));
            cellule_libre += 1;
            suivant += 1;
        }
        Self::new(avatars)
    }
}

fn trame<S: ChronotopeStore>(store: &S, scene: &Scene, tick: Tick) {
    for (cellule, entite) in &scene.avatars {
        let _ = black_box(store.ecrire(ROOM, *cellule, tick, *entite, POSE));
    }
    for cellule in &scene.cellules {
        black_box(store.sceller(ROOM, *cellule, tick));
    }
}

fn mesurer<S: ChronotopeStore>(iters: u64, store: &S, vider: impl Fn(), scene: &Scene) -> Duration {
    let mut total = Duration::ZERO;
    let mut faites = 0u64;
    let mut tick = 0u64;
    while faites < iters {
        let lot = (iters - faites).min(TRAMES_PAR_LOT);
        vider();
        let debut = Instant::now();
        for _ in 0..lot {
            trame(store, scene, Tick(tick));
            tick += 1;
        }
        total += debut.elapsed();
        faites += lot;
    }
    total
}

fn comparer(c: &mut Criterion, groupe: &str, scenes: &[(String, Scene)]) {
    let mut g = c.benchmark_group(groupe);
    for (nom, scene) in scenes {
        g.throughput(Throughput::Elements(scene.avatars.len() as u64));

        let moteur = ChronotopeEngine::new(Horizon::default());
        g.bench_with_input(BenchmarkId::new("chronotope", nom), scene, |b, scene| {
            b.iter_custom(|iters| mesurer(iters, &moteur, || moteur.vider(), scene));
        });

        let ligne = NaiveRowStore::new();
        g.bench_with_input(BenchmarkId::new("ligne", nom), scene, |b, scene| {
            b.iter_custom(|iters| mesurer(iters, &ligne, || ligne.vider(), scene));
        });
    }
    g.finish();
}

fn bench_disperse(c: &mut Criterion) {
    let scenes: Vec<(String, Scene)> = [30u32, 80, 200]
        .into_iter()
        .map(|n| (format!("{n}_avatars_1_par_cellule"), Scene::dispersee(n)))
        .collect();
    comparer(c, "h1_disperse", &scenes);
}

fn bench_agglomere(c: &mut Criterion) {
    let scenes: Vec<(String, Scene)> = [40u32, 80, 200]
        .into_iter()
        .map(|n| (format!("{n}_avatars_1_cellule"), Scene::agglomeree(n)))
        .collect();
    comparer(c, "h1_agglomere", &scenes);
}

fn bench_social(c: &mut Criterion) {
    let scenes: Vec<(String, Scene)> = [30u32, 80, 200]
        .into_iter()
        .map(|n| (format!("{n}_avatars_agglutines"), Scene::sociale(n)))
        .collect();
    comparer(c, "h1_social", &scenes);
}

criterion_group!(disperse, bench_disperse);
criterion_group!(agglomere, bench_agglomere);
criterion_group!(social, bench_social);
criterion_main!(disperse, agglomere, social);

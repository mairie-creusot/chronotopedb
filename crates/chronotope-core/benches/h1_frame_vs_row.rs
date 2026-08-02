//! H1 : "l'ecriture par trame reduit le cout d'ecriture dans le regime
//! agglomere". Refutee si le gain mesure est < 3x. Voir `docs/chronotope.md`
//! §H1.
//!
//! A IMPLEMENTER : comparer `ChronotopeEngine` et `NaiveRowStore` (memes
//! donnees, meme sequence d'ecritures) sur au moins ces regimes, reprenant
//! les effectifs du document source (§3.2 tableau AOI) :
//!
//! - dispersees : 1 entite par cellule (regime ou l'AOI est censee gagner)
//! - agglomerees : 40 entites dans UNE cellule (le regime que H1 cible)
//! - 30 / 80 / 200 avatars au total, repartition realiste (pas uniforme —
//!   voir la remarque du document source sur l'agglutination en VR sociale)
//!
//! Rapporter les ops/s ET le temps CPU (criterion mesure les deux via
//! `Bencher::iter` + `criterion_group!` normal — pas besoin d'instrumentation
//! externe). Le seuil de refutation (gain < 3x) doit pouvoir se lire
//! directement dans le rapport HTML de criterion (`--features html_reports`
//! deja active dans Cargo.toml) sans calcul manuel.

use chronotope_core::{ChronotopeEngine, ChronotopeStore, Horizon, NaiveRowStore};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_placeholder(c: &mut Criterion) {
    let _engine = ChronotopeEngine::new(Horizon::default());
    let _naive = NaiveRowStore::new();
    // TODO(H1) : remplacer par les groupes de benchmarks reels (dispersees /
    // agglomerees / 30 / 80 / 200), un `criterion_group!` par regime pour
    // que le rapport les distingue clairement.
    c.bench_function("placeholder — a remplacer par les groupes H1", |b| {
        b.iter(|| 1 + 1)
    });
}

criterion_group!(benches, bench_placeholder);
criterion_main!(benches);

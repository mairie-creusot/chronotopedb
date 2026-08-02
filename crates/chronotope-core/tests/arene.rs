//! Mode d'echec §5.11.3 (brassage memoire) : sceller a haute frequence sur
//! beaucoup de cellules ne doit PAS produire un flux d'allocations. Ce test
//! compte les allocations reelles du processus et verifie qu'en regime etabli
//! une trame n'alloue plus que ce que le contrat public impose — un `Vec` par
//! `Chronotope` rendu a l'appelant — l'anneau interne etant recycle en place.
//!
//! Il echouerait si `ChronotopeEngine` allouait par chronotope scelle (le
//! piege que le document source signale) ou si le recyclage d'anneau cessait
//! de conserver la capacite des journaux.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick,
};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Compteur;

#[allow(unsafe_code)]
// SAFETY: `Compteur` delegue integralement a `System`, l'allocateur global de
// la bibliotheque standard, qui satisfait deja le contrat de `GlobalAlloc`
// (memes `Layout`, memes pointeurs rendus tels quels). La seule addition est
// un compteur atomique sans effet sur la memoire retournee, donc aucun
// invariant supplementaire n'est introduit.
unsafe impl GlobalAlloc for Compteur {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, taille: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, layout, taille)
    }
}

#[global_allocator]
static ALLOCATEUR: Compteur = Compteur;

const CELLULES: u32 = 16;
const ENTITES_PAR_CELLULE: u32 = 40;

fn trame(moteur: &ChronotopeEngine, tick: Tick) {
    let pose = Pose {
        pos: [1.0, 2.0, 3.0],
        yaw: 0.5,
    };
    for cellule in 0..CELLULES {
        for entite in 0..ENTITES_PAR_CELLULE {
            moteur
                .ecrire(RoomId(0), CellId(cellule), tick, EntityId(entite), pose)
                .unwrap();
        }
    }
    for cellule in 0..CELLULES {
        std::hint::black_box(moteur.sceller(RoomId(0), CellId(cellule), tick));
    }
}

fn allocations_de(f: impl FnOnce()) -> usize {
    let avant = ALLOCATIONS.load(Ordering::Relaxed);
    f();
    ALLOCATIONS.load(Ordering::Relaxed) - avant
}

#[test]
fn le_regime_etabli_n_alloue_plus_que_les_chronotopes_rendus() {
    let moteur = ChronotopeEngine::new(Horizon::default());
    let anneau = moteur.ticks_retenus();

    for tick in 0..(anneau * 3) {
        trame(&moteur, Tick(tick));
    }

    let apres_rodage = allocations_de(|| trame(&moteur, Tick(anneau * 3)));
    let apres_deux_tours = allocations_de(|| trame(&moteur, Tick(anneau * 5)));

    assert_eq!(
        apres_rodage, apres_deux_tours,
        "le cout d'allocation d'une trame doit etre stable une fois l'anneau chaud"
    );
    assert_eq!(
        apres_rodage as u32, CELLULES,
        "une trame ne doit allouer qu'un Vec par chronotope rendu ({CELLULES} cellules scellees)"
    );
}

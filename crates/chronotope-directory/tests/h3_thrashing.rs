//! H3 au niveau de ce crate : "la migration sans transfert n'introduit pas
//! d'artefact" — ici, l'artefact teste est le thrashing decrit par le retour
//! d'experience SpatialOS (§5.3) : une entite qui longe une frontiere de
//! cellule migre en rafale si aucune bande de recouvrement ne l'en empeche.
//!
//! Le test simule la MEME trajectoire deux fois — avec et sans hysteresis —
//! et compare les deux comptages. Le temoin "sans hysteresis" est un vrai
//! `Directory` regle sur `Hysteresis::aucune()`, pas une constante ecrite a
//! la main : la comparaison porte donc sur du code reellement execute.
//!
//! Il mesure aussi le prix de l'hysteresis (ticks pendant lesquels l'annuaire
//! designe une cellule que l'entite a geometriquement quittee) et la latence
//! de bascule quand l'entite s'eloigne pour de bon — sans quoi "moins de
//! migrations" se satisferait d'un annuaire qui ne migre jamais.

use chronotope_core::{CellId, EntityId, Tick};
use chronotope_directory::{Directory, Hysteresis, Location, Migration, NodeId};

const TAILLE_CELLULE: f32 = 10.0;
const FRONTIERE: f32 = 10.0;
/// Ticks passes a osciller de part et d'autre de la frontiere.
const TICKS_FRONTIERE: u64 = 600;
/// Ticks passes a s'eloigner franchement dans la cellule voisine.
const TICKS_DEPART: u64 = 60;

/// Decoupage spatial de test — volontairement local au test : le crate
/// `chronotope-directory` ne connait aucune geometrie (§ "Modularite" de
/// `docs/conventions.md`), c'est l'appelant qui traduit une position en
/// `CellId`.
fn cellule_de(x: f32) -> CellId {
    CellId((x / TAILLE_CELLULE).floor().max(0.0) as u32)
}

/// Cellules paires et impaires sur deux noeuds distincts : franchir la
/// frontiere, c'est aussi changer de noeud — exactement le scenario du §5.3
/// (cellule A noeud 1 -> cellule B noeud 2).
fn emplacement_de(x: f32) -> Location {
    let cell = cellule_de(x);
    Location {
        node: NodeId(cell.0 % 2),
        cell,
    }
}

/// Trajectoire deterministe : 600 ticks a osciller a cheval sur la frontiere
/// x = 10.0 (amplitude 35 cm, periode ~3.3 ticks — un joueur adosse a la
/// limite de cellule), puis 60 ticks a s'en eloigner franchement.
fn trajectoire(t: u64) -> f32 {
    if t < TICKS_FRONTIERE {
        FRONTIERE + 0.35 * (t as f32 * 1.9).sin()
    } else {
        FRONTIERE + 0.35 + (t - TICKS_FRONTIERE) as f32 * 0.05
    }
}

#[derive(Debug)]
struct Mesure {
    migrations: u32,
    refus: u32,
    /// Ticks pendant lesquels l'annuaire designe une autre cellule que celle
    /// ou l'entite se trouve geometriquement. Le cout de l'hysteresis.
    ticks_desaccord: u32,
    /// Ticks entre le depart franc (t = TICKS_FRONTIERE) et le moment ou
    /// l'annuaire se fixe definitivement sur la bonne cellule.
    latence_bascule: u64,
    finale: Location,
}

fn simuler(hysteresis: Hysteresis) -> Mesure {
    let annuaire = Directory::avec_hysteresis(hysteresis);
    let entite = EntityId(1);
    annuaire.inscrire(entite, emplacement_de(trajectoire(0)), Tick(0));

    let mut migrations = 0;
    let mut refus = 0;
    let mut ticks_desaccord = 0;
    let mut dernier_desaccord_apres_depart = None;

    for t in 1..(TICKS_FRONTIERE + TICKS_DEPART) {
        let souhaite = emplacement_de(trajectoire(t));
        match annuaire.migrer(entite, souhaite, Tick(t)) {
            Ok(Migration::Effectuee { .. }) => migrations += 1,
            Ok(Migration::Inchangee) => {}
            Err(_) => refus += 1,
        }
        if annuaire.locate(entite) != Some(souhaite) {
            ticks_desaccord += 1;
            if t >= TICKS_FRONTIERE {
                dernier_desaccord_apres_depart = Some(t);
            }
        }
    }

    Mesure {
        migrations,
        refus,
        ticks_desaccord,
        latence_bascule: dernier_desaccord_apres_depart
            .map(|t| t - TICKS_FRONTIERE + 1)
            .unwrap_or(0),
        finale: annuaire
            .locate(entite)
            .expect("entite toujours localisable"),
    }
}

#[test]
fn hysteresis_supprime_le_thrashing_le_long_d_une_frontiere() {
    const TICKS: u64 = 8;
    let sans = simuler(Hysteresis::aucune());
    let avec = simuler(Hysteresis::ticks(TICKS));

    println!("sans hysteresis : {sans:?}");
    println!("avec hysteresis (8 ticks) : {avec:?}");

    // Le temoin thrashe bel et bien : sans cela, la comparaison ne
    // prouverait rien.
    assert!(
        sans.migrations > 300,
        "la trajectoire doit provoquer un vrai thrashing sans hysteresis, \
         observe : {} migrations",
        sans.migrations
    );
    assert_eq!(sans.refus, 0);
    assert_eq!(sans.ticks_desaccord, 0);

    assert!(
        avec.migrations * 4 <= sans.migrations,
        "l'hysteresis doit diviser les migrations par au moins 4 : \
         avec = {}, sans = {}",
        avec.migrations,
        sans.migrations
    );

    // ... sans pour autant cesser de router : l'entite qui s'eloigne pour de
    // bon est retrouvee au bon endroit, dans le delai borne par l'hysteresis.
    let attendu = emplacement_de(trajectoire(TICKS_FRONTIERE + TICKS_DEPART - 1));
    assert_eq!(avec.finale, attendu);
    assert_eq!(sans.finale, attendu);
    assert!(
        avec.latence_bascule <= TICKS,
        "la bascule doit se produire dans le delai d'hysteresis, observe : {} ticks",
        avec.latence_bascule
    );
}

#[test]
fn le_thrashing_decroit_de_facon_monotone_avec_le_reglage() {
    let reglages = [0, 2, 4, 8, 16, 32];
    let mesures: Vec<_> = reglages
        .iter()
        .map(|n| (*n, simuler(Hysteresis::ticks(*n))))
        .collect();

    for (n, m) in &mesures {
        println!(
            "hysteresis {n:>2} ticks : {:>3} migrations, {:>3} refus, \
             {:>3} ticks de desaccord, latence de bascule {} ticks",
            m.migrations, m.refus, m.ticks_desaccord, m.latence_bascule
        );
    }

    for paire in mesures.windows(2) {
        let (n_faible, faible) = &paire[0];
        let (n_fort, fort) = &paire[1];
        assert!(
            fort.migrations <= faible.migrations,
            "une hysteresis plus large ne doit jamais produire plus de \
             migrations : {n_faible} ticks -> {} migrations, \
             {n_fort} ticks -> {} migrations",
            faible.migrations,
            fort.migrations
        );
    }

    // Meme au reglage le plus large, l'entite finit au bon endroit : le gain
    // ne vient pas d'un annuaire qui aurait renonce a router.
    let attendu = emplacement_de(trajectoire(TICKS_FRONTIERE + TICKS_DEPART - 1));
    for (n, m) in &mesures {
        assert_eq!(m.finale, attendu, "reglage {n} ticks");
        assert!(m.latence_bascule <= *n, "reglage {n} ticks");
    }
}

#[test]
fn plusieurs_entites_sur_la_meme_frontiere_ne_s_influencent_pas() {
    let annuaire = Directory::avec_hysteresis(Hysteresis::ticks(8));
    let entites: Vec<_> = (0..16).map(EntityId).collect();
    for e in &entites {
        annuaire.inscrire(*e, emplacement_de(trajectoire(0)), Tick(0));
    }

    let mut migrations = 0;
    for t in 1..TICKS_FRONTIERE {
        // Chaque entite longe la frontiere avec un dephasage propre : leurs
        // horloges d'hysteresis doivent rester independantes.
        for (i, e) in entites.iter().enumerate() {
            let souhaite = emplacement_de(trajectoire(t + i as u64));
            if let Ok(Migration::Effectuee { .. }) = annuaire.migrer(*e, souhaite, Tick(t)) {
                migrations += 1;
            }
        }
    }

    println!("16 entites sur la frontiere : {migrations} migrations");
    assert_eq!(annuaire.len(), entites.len());
    assert!(
        migrations < 16 * (TICKS_FRONTIERE as u32) / 4,
        "observe : {migrations}"
    );
}

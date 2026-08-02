//! Binaire d'integration — expose `ChronotopeStore` en HTTP/WebSocket pour
//! des tests reels contre une vraie pile pawchat (`docker-compose.dev.yml`),
//! en dehors du perimetre de mesure H1-H4 (qui restent des benchmarks/tests
//! en memoire, sans reseau — voir `chronotope-core/benches` et
//! `chronotope-sim`). Ce binaire n'est PAS le "vrai" transport envisage par
//! le concept (WebTransport, §3.3/§4 de docs/chronotope.md) : c'est un
//! harnais HTTP volontairement simple pour prouver que le moteur peut etre
//! atteint depuis l'exterieur du process, rien de plus.
//!
//! `/health` ne touche jamais le moteur et n'exige aucune authentification :
//! c'est deliberement le seul endpoint garanti de repondre (c'est aussi ce
//! que sonde le `HEALTHCHECK` Docker, interne au conteneur). Toutes les
//! autres routes (`/write` `/seal` `/read` `/metrics`) sont derriere
//! `authentifier` — `chronotope-server` est l'unique frontiere de confiance
//! du depot (`docs/conventions.md`), donc c'est ici, et seulement ici, que
//! l'acces est controle.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    error_handling::HandleErrorLayer,
    extract::{Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick,
    CELLULES_PAR_SALLE, SALLES_MAX,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tower::ServiceBuilder;
use tracing_subscriber::EnvFilter;

mod banner;
mod metrics;

use metrics::Metrics;

struct AppState {
    engine: ChronotopeEngine,
    secret: String,
    metrics: Metrics,
}

#[tokio::main]
async fn main() {
    // RUST_LOG configurable (ex: RUST_LOG=chronotope_core=trace,tower_http=debug)
    // — jamais un niveau fige en dur. `with_target(true)` + `with_line_number(true)`
    // : un log doit dire QUI a parle sans avoir a deviner, c'est la premiere
    // moitie de "super tracable".
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .init();

    banner::banner();

    let secret =
        secret_depuis_env(std::env::var("CHRONOTOPE_INTERNAL_SECRET")).unwrap_or_else(|erreur| {
            tracing::error!(
                "{erreur} — chronotope-server est l'unique frontiere de confiance du depot \
                 (docs/conventions.md) et refuse de demarrer sans secret configure"
            );
            std::process::exit(1);
        });

    let state = Arc::new(AppState {
        engine: ChronotopeEngine::new(Horizon::default()),
        secret,
        metrics: Metrics::default(),
    });

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3200);
    let limite = limite_concurrence();
    // Un seul log, tous les parametres effectifs qui font varier le
    // comportement du process — la premiere chose qu'on regarde quand un
    // comportement en prod surprend, c'est "avec quelle config a-t-il
    // demarre", et ca ne devrait jamais exiger de recouper plusieurs lignes.
    tracing::info!(
        port,
        concurrency_limit = limite,
        secret_configure = true,
        secret_longueur = state.secret.len(),
        "configuration effective au demarrage"
    );

    let app = router(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!(%addr, "chronotope-server en ecoute");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

/// Plafond de concurrence effectif (voir `router`) — extrait de `router`
/// pour que le log de demarrage de `main` reflete la VALEUR REELLEMENT
/// appliquee, pas une supposition qui pourrait diverger si la lecture de la
/// variable d'environnement changeait d'un cote sans l'autre.
fn limite_concurrence() -> usize {
    std::env::var("CHRONOTOPE_MAX_CONCURRENT_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256)
}

/// Lit le secret partage depuis la variable d'environnement. Separee de
/// `main` pour rester unit-testable sans jamais appeler
/// `std::process::exit` dans un test. Echoue (fail-closed) sur secret
/// absent OU vide — pas de mode dev permissif : un secret par defaut
/// "pratique" est exactement le genre de trou qui survit silencieusement
/// jusqu'en production.
fn secret_depuis_env(v: Result<String, std::env::VarError>) -> Result<String, &'static str> {
    match v {
        Ok(s) if !s.is_empty() => Ok(s),
        _ => Err("CHRONOTOPE_INTERNAL_SECRET manquant ou vide"),
    }
}

/// Comparaison a temps constant (`subtle::ConstantTimeEq`) plutot qu'une
/// primitive ecrite a la main : se tromper sur une comparaison
/// "constant-time" faite maison (l'optimiseur peut raccourcir une boucle
/// naive) est precisement le genre d'erreur qu'une crate minimale et
/// auditee evite. La longueur est verifiee AVANT l'appel a `ct_eq` — la
/// longueur d'un secret n'est pas elle-meme l'information sensible ici.
fn secret_valide(configure: &str, fourni: Option<&str>) -> bool {
    let Some(fourni) = fourni else {
        return false;
    };
    fourni.len() == configure.len() && bool::from(fourni.as_bytes().ct_eq(configure.as_bytes()))
}

/// Middleware applique au sous-routeur "protege" (voir `router`). Rejette
/// avec `401` + `WWW-Authenticate: Bearer` avant meme d'atteindre le
/// moteur — la meme discipline que `room_ou_cell_hors_domaine` plus bas :
/// une entree non fiable est rejetee a la frontiere, jamais laissee
/// atteindre `chronotope-core`.
#[tracing::instrument(level = "trace", skip_all, fields(path = %request.uri().path()))]
async fn authentifier(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let fourni = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    if secret_valide(&state.secret, fourni) {
        tracing::trace!("authentification acceptee");
        next.run(request).await
    } else {
        state.metrics.auth_rejected.fetch_add(1, Ordering::Relaxed);
        tracing::warn!("requete rejetee par l'authentification");
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
            Json(serde_json::json!({ "error": "authentification requise" })),
        )
            .into_response()
    }
}

/// Convertit une erreur du sous-systeme resilience (`load_shed`/
/// `concurrency_limit`, voir `router`) en reponse HTTP propre. `axum::Router`
/// exige un service final infaillible ; ces deux couches sont fallibles par
/// construction (c'est exactement ce qui permet a `load_shed` de rejeter
/// vite plutot que de mettre en file indefiniment), donc cette couche DOIT
/// exister pour que le routeur compile.
async fn gerer_surcharge(erreur: tower::BoxError) -> Response {
    tracing::warn!(%erreur, "requete rejetee — limite de resilience atteinte");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "serveur sature, reessayer" })),
    )
        .into_response()
}

/// Sous-routeur "protege" : `/write` `/seal` `/read` `/metrics`, SANS
/// authentification a ce niveau — voir `router` pour pourquoi.
fn router_protege(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/write", post(write))
        .route("/seal", post(seal))
        .route("/read", get(read))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Construit le `Router` complet sans lancer de serveur — separe de `main`
/// pour que les tests d'integration (`tower::ServiceExt::oneshot`) puissent
/// driver les routes sans ouvrir de vraie socket TCP.
///
/// `/health` reste sur ce routeur de premier niveau, hors de toute couche de
/// resilience (il doit repondre vite meme si le reste est sature — verifie
/// par `sante_repond_pendant_la_saturation` plus bas). Le reste passe par
/// `authentifier` PUIS le filet load-shed/concurrency-limit, les DEUX
/// montes en `fallback_service` en enveloppant un `Router` deja
/// completement construit — jamais via `Router::layer()`/`.route_layer()`
/// sur le sous-routeur lui-meme. Deux bugs reels, verifies empiriquement,
/// justifient ce choix plutot que l'approche "plus simple" :
/// - `load_shed`+`concurrency_limit` appliques par `Router::layer()` ne
///   partageaient PAS leur etat entre deux requetes (la limite n'etait
///   jamais appliquee, quel que soit le nombre de requetes concurrentes).
/// - `authentifier` applique par `.route_layer()` sur `router_protege`
///   NE PROTEGEAIT PAS une requete sans `Content-Type` (ou avec un
///   mauvais) : elle atteignait le rejet `415` de l'extracteur `Json`
///   SANS jamais passer par `authentifier` (aucune ligne de log,
///   confirme via `docker logs` en conditions reelles) — `route_layer`
///   n'enveloppe pas tous les chemins de rejet internes d'axum.
///
/// Enveloppes autour d'un `Router` deja construit et montes en
/// `fallback_service`, les deux fonctionnent correctement — c'est aussi
/// le patron documente par axum pour composer un `Router` avec des
/// couches falibles comme `load_shed`/`timeout`.
fn router(state: Arc<AppState>) -> Router {
    let limite = limite_concurrence();

    let protege = ServiceBuilder::new()
        .layer(middleware::from_fn_with_state(state.clone(), authentifier))
        .layer(HandleErrorLayer::new(gerer_surcharge))
        .load_shed()
        .concurrency_limit(limite)
        .service(router_protege(state));

    Router::new()
        .route("/health", get(health))
        .fallback_service(protege)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "service": "chronotope-server" }))
}

/// `room`/`cell` hors du domaine du moteur (`SALLES_MAX`/`CELLULES_PAR_SALLE`)
/// font `panic!` dans `chronotope-core` (voir `engine.rs::base`/`coordonnees`)
/// — c'est un bug appelant *pour le moteur en bibliotheque*, mais ici
/// l'appelant, c'est un client HTTP non fiable. `chronotope-server` est la
/// seule frontiere de confiance du depot (`docs/conventions.md`) : ces
/// valeurs doivent etre rejetees ICI, avant tout appel au moteur.
fn room_ou_cell_hors_domaine(room: u32, cell: u32) -> bool {
    room >= SALLES_MAX || u64::from(cell) >= CELLULES_PAR_SALLE
}

#[tracing::instrument(level = "trace", skip_all)]
async fn metrics_handler(State(state): State<Arc<AppState>>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render_prometheus(),
    )
        .into_response()
}

#[derive(Deserialize)]
struct WriteRequest {
    room: u32,
    cell: u32,
    tick: u64,
    entity: u32,
    pos: [f32; 3],
    yaw: f32,
}

#[derive(Serialize)]
struct WriteResponse {
    ok: bool,
    error: Option<String>,
}

/// `debug`, pas `trace` : une requete HTTP est deja une decision arrivee a
/// la frontiere (voir `docs/conventions.md`, "niveaux disciplines" — le
/// detail d'implementation individuel, lui, est trace par
/// `chronotope_core::engine::ecrire` en dessous). Les champs de domaine sont
/// sur le span des le depart : un `grep` sur `room=42` retrouve TOUTE
/// l'activite de cette salle, succes ET echecs, sans avoir a recouper des
/// lignes eparses.
#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(room = req.room, cell = req.cell, tick = req.tick, entity = req.entity)
)]
async fn write(State(state): State<Arc<AppState>>, Json(req): Json<WriteRequest>) -> Response {
    let debut = Instant::now();

    let (succes, reponse) = if room_ou_cell_hors_domaine(req.room, req.cell) {
        tracing::warn!("ecriture refusee — room ou cell hors domaine");
        (
            false,
            (
                StatusCode::BAD_REQUEST,
                Json(WriteResponse {
                    ok: false,
                    error: Some(format!(
                        "room ou cell hors domaine (room < {SALLES_MAX}, cell < {CELLULES_PAR_SALLE})"
                    )),
                }),
            )
                .into_response(),
        )
    } else {
        match state.engine.ecrire(
            RoomId(req.room),
            CellId(req.cell),
            Tick(req.tick),
            EntityId(req.entity),
            Pose {
                pos: req.pos,
                yaw: req.yaw,
            },
        ) {
            Ok(()) => (
                true,
                Json(WriteResponse {
                    ok: true,
                    error: None,
                })
                .into_response(),
            ),
            Err(e) => {
                tracing::warn!(raison = %e, "ecriture refusee par le moteur");
                (
                    false,
                    Json(WriteResponse {
                        ok: false,
                        error: Some(e.to_string()),
                    })
                    .into_response(),
                )
            }
        }
    };

    state.metrics.write.observer(succes, debut.elapsed());
    reponse
}

#[derive(Deserialize)]
struct SealRequest {
    room: u32,
    cell: u32,
    tick: u64,
}

#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(room = req.room, cell = req.cell, tick = req.tick)
)]
async fn seal(State(state): State<Arc<AppState>>, Json(req): Json<SealRequest>) -> Response {
    let debut = Instant::now();

    let (succes, reponse) = if room_ou_cell_hors_domaine(req.room, req.cell) {
        tracing::warn!("scellement refuse — room ou cell hors domaine");
        (
            false,
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "room ou cell hors domaine (room < {SALLES_MAX}, cell < {CELLULES_PAR_SALLE})"
                    ),
                })),
            )
                .into_response(),
        )
    } else {
        let chronotope = state
            .engine
            .sceller(RoomId(req.room), CellId(req.cell), Tick(req.tick));
        tracing::debug!(
            entity_count = chronotope.entities.len(),
            "chronotope scelle"
        );
        (
            true,
            Json(serde_json::json!({
                "room": chronotope.room.0,
                "cell": chronotope.cell.0,
                "tick": chronotope.tick.0,
                "sealed": chronotope.sealed,
                "entity_count": chronotope.entities.len(),
            }))
            .into_response(),
        )
    };

    state.metrics.seal.observer(succes, debut.elapsed());
    reponse
}

#[derive(Deserialize)]
struct ReadQuery {
    room: u32,
    cells: String, // liste separee par des virgules
    tick: u64,
}

#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(room = q.room, tick = q.tick, cells_demandees = q.cells.as_str())
)]
async fn read(State(state): State<Arc<AppState>>, Query(q): Query<ReadQuery>) -> Response {
    let debut = Instant::now();

    let (succes, reponse) = if q.room >= SALLES_MAX {
        tracing::warn!("lecture refusee — room hors domaine");
        (
            false,
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("room hors domaine (room < {SALLES_MAX})"),
                })),
            )
                .into_response(),
        )
    } else {
        // Meme politique que le parsing : une valeur hors domaine est aussi
        // silencieusement ecartee (elle ne designerait de toute facon aucune
        // cellule reelle) plutot que de faire paniquer le moteur.
        let cells: Vec<CellId> = q
            .cells
            .split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok())
            .filter(|&c| u64::from(c) < CELLULES_PAR_SALLE)
            .map(CellId)
            .collect();
        if cells.is_empty() {
            tracing::debug!("aucune cellule valide dans la requete — lecture vide");
        }
        let chronotopes = state.engine.lire(RoomId(q.room), &cells, Tick(q.tick));
        tracing::trace!(count = chronotopes.len(), "lecture terminee");
        (
            true,
            Json(serde_json::json!({ "count": chronotopes.len() })).into_response(),
        )
    };

    state.metrics.read.observer(succes, debut.elapsed());
    reponse
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request as HttpRequest;
    use std::time::Duration;
    use tower::ServiceExt;

    const SECRET_TEST: &str = "secret-de-test";

    fn app() -> Router {
        router(Arc::new(AppState {
            engine: ChronotopeEngine::new(Horizon::default()),
            secret: SECRET_TEST.to_string(),
            metrics: Metrics::default(),
        }))
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post(path: &str, body: serde_json::Value) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {SECRET_TEST}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_sans_secret(path: &str, body: serde_json::Value) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_authentifie(path: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("GET")
            .uri(path)
            .header("authorization", format!("Bearer {SECRET_TEST}"))
            .body(Body::empty())
            .unwrap()
    }

    fn get_sans_secret(path: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    // --- secret_valide / secret_depuis_env ---------------------------------

    #[test]
    fn secret_valide_accepte_le_bon_secret() {
        assert!(secret_valide("abc123", Some("abc123")));
    }

    #[test]
    fn secret_valide_refuse_un_mauvais_secret_meme_longueur() {
        assert!(!secret_valide("abc123", Some("xbc123")));
    }

    #[test]
    fn secret_valide_refuse_une_longueur_differente() {
        assert!(!secret_valide("abc123", Some("abc1234")));
        assert!(!secret_valide("abc123", Some("abc12")));
    }

    #[test]
    fn secret_valide_refuse_l_absence_de_secret() {
        assert!(!secret_valide("abc123", None));
    }

    #[test]
    fn secret_valide_refuse_un_secret_client_vide() {
        assert!(!secret_valide("abc123", Some("")));
    }

    #[test]
    fn secret_depuis_env_accepte_une_valeur_non_vide() {
        assert_eq!(secret_depuis_env(Ok("x".to_string())), Ok("x".to_string()));
    }

    #[test]
    fn secret_depuis_env_refuse_une_valeur_vide() {
        assert!(secret_depuis_env(Ok(String::new())).is_err());
    }

    #[test]
    fn secret_depuis_env_refuse_une_variable_absente() {
        assert!(secret_depuis_env(Err(std::env::VarError::NotPresent)).is_err());
    }

    // --- authentification sur chaque route protegee -------------------------

    #[tokio::test]
    async fn write_sans_secret_est_rejete_401() {
        let response = app()
            .oneshot(post_sans_secret(
                "/write",
                serde_json::json!({"room":0,"cell":0,"tick":0,"entity":0,"pos":[0.0,0.0,0.0],"yaw":0.0}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    /// Regression : une requete SANS aucun header (ni `Authorization` ni
    /// `Content-Type`, ni corps) atteignait auparavant le rejet `415` de
    /// l'extracteur `Json` sans jamais passer par `authentifier` — parce
    /// que `.route_layer()` sur le sous-routeur "protege" n'enveloppait
    /// pas ce chemin de rejet interne d'axum. Confirme en conditions
    /// reelles (`docker logs` ne montrait AUCUNE ligne d'authentification
    /// pour cette requete). Corrige en deplacant `authentifier` sur le
    /// `Router` deja construit (voir `router`), pas sur le sous-routeur.
    #[tokio::test]
    async fn write_sans_aucun_header_est_rejete_401_pas_415() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/write")
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn write_avec_mauvais_secret_est_rejete_401() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/write")
            .header("content-type", "application/json")
            .header("authorization", "Bearer mauvais-secret")
            .body(Body::from(
                serde_json::json!({"room":0,"cell":0,"tick":0,"entity":0,"pos":[0.0,0.0,0.0],"yaw":0.0})
                    .to_string(),
            ))
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn seal_sans_secret_est_rejete_401() {
        let response = app()
            .oneshot(post_sans_secret(
                "/seal",
                serde_json::json!({"room":0,"cell":0,"tick":0}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn read_sans_secret_est_rejete_401() {
        let response = app()
            .oneshot(get_sans_secret("/read?room=0&cells=0&tick=0"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn metrics_sans_secret_est_rejete_401() {
        let response = app().oneshot(get_sans_secret("/metrics")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_reste_public_sans_secret() {
        let response = app().oneshot(get_sans_secret("/health")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn un_rejet_d_authentification_incremente_le_compteur() {
        let application = app();

        application
            .clone()
            .oneshot(get_sans_secret("/write"))
            .await
            .unwrap();
        application
            .clone()
            .oneshot(get_sans_secret("/write"))
            .await
            .unwrap();

        let response = application
            .oneshot(get_authentifie("/metrics"))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let texte = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(texte.contains("chronotope_auth_rejected_total 2"));
    }

    // --- /metrics reflete du vrai trafic ------------------------------------

    #[tokio::test]
    async fn metrics_reflete_une_sequence_ecriture_scellement_lecture() {
        let application = app();

        application
            .clone()
            .oneshot(post(
                "/write",
                serde_json::json!({"room":2,"cell":2,"tick":2,"entity":1,"pos":[0.0,0.0,0.0],"yaw":0.0}),
            ))
            .await
            .unwrap();
        application
            .clone()
            .oneshot(post(
                "/seal",
                serde_json::json!({"room":2,"cell":2,"tick":2}),
            ))
            .await
            .unwrap();
        application
            .clone()
            .oneshot(get_authentifie("/read?room=2&cells=2&tick=2"))
            .await
            .unwrap();

        let response = application
            .oneshot(get_authentifie("/metrics"))
            .await
            .unwrap();
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let texte = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"ok\"} 1"));
        assert!(texte.contains("chronotope_requests_total{route=\"seal\",result=\"ok\"} 1"));
        assert!(texte.contains("chronotope_requests_total{route=\"read\",result=\"ok\"} 1"));
    }

    #[tokio::test]
    async fn metrics_compte_un_echec_semantique_malgre_un_statut_http_200() {
        // seconde ecriture sur le meme (room,cell,tick) apres scellement :
        // reponse HTTP 200 avec {ok:false} dans le corps — ceci doit compter
        // comme un echec dans les metriques, pas comme un succes.
        let application = app();
        application
            .clone()
            .oneshot(post(
                "/write",
                serde_json::json!({"room":3,"cell":3,"tick":3,"entity":1,"pos":[0.0,0.0,0.0],"yaw":0.0}),
            ))
            .await
            .unwrap();
        application
            .clone()
            .oneshot(post(
                "/seal",
                serde_json::json!({"room":3,"cell":3,"tick":3}),
            ))
            .await
            .unwrap();
        let response = application
            .clone()
            .oneshot(post(
                "/write",
                serde_json::json!({"room":3,"cell":3,"tick":3,"entity":2,"pos":[0.0,0.0,0.0],"yaw":0.0}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["ok"], false);

        let response = application
            .oneshot(get_authentifie("/metrics"))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let texte = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"ok\"} 1"));
        assert!(texte.contains("chronotope_requests_total{route=\"write\",result=\"err\"} 1"));
    }

    // --- limite de concurrence / load-shed -----------------------------------

    #[tokio::test]
    async fn une_rafale_sous_la_limite_par_defaut_reussit_entierement() {
        let application = app();
        let mut resultats = Vec::new();
        for i in 0..20u32 {
            let r = application
                .clone()
                .oneshot(post(
                    "/write",
                    serde_json::json!({"room":4,"cell":4,"tick":4,"entity":i,"pos":[0.0,0.0,0.0],"yaw":0.0}),
                ))
                .await
                .unwrap();
            resultats.push(r.status());
        }
        assert!(resultats.iter().all(|s| *s == StatusCode::OK));
    }

    /// Routeur dedie a la limite de concurrence : meme architecture que
    /// `router()` (fallback_service, pas `Router::layer()` — voir sa doc)
    /// mais avec une limite injectee basse et un handler artificiellement
    /// lent a la place de `/write`, pour rendre le depassement deterministe
    /// sans ralentir la suite de tests normale avec un vrai delai partout.
    fn routeur_lent(limite: usize) -> Router {
        async fn lent() -> &'static str {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "ok"
        }

        let protege = ServiceBuilder::new()
            .layer(HandleErrorLayer::new(gerer_surcharge))
            .load_shed()
            .concurrency_limit(limite)
            .service(Router::<()>::new().route("/lent", get(lent)));

        Router::new()
            .route("/health", get(health))
            .fallback_service(protege)
    }

    fn requete(path: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    // Vraies taches (`tokio::spawn` sur un runtime multi-thread) plutot que
    // `tokio::join!` sur le runtime mono-thread par defaut des tests : on
    // veut une vraie concurrence, pas un entrelacement cooperatif qui
    // pourrait masquer un probleme de partage d'etat entre requetes — c'est
    // exactement ce qui a fait passer inapercu le bug de `Router::layer()`
    // documente sur `router()`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn au_dela_de_la_limite_de_concurrence_le_surplus_recoit_503() {
        let routeur = routeur_lent(1);

        let a = routeur.clone();
        let tache_a = tokio::spawn(async move { a.oneshot(requete("/lent")).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let b = routeur.clone();
        let tache_b = tokio::spawn(async move { b.oneshot(requete("/lent")).await });

        let ra = tache_a.await.unwrap().unwrap();
        let rb = tache_b.await.unwrap().unwrap();
        let statuts = [ra.status(), rb.status()];
        assert!(
            statuts.contains(&StatusCode::SERVICE_UNAVAILABLE),
            "{statuts:?}"
        );
        assert!(statuts.contains(&StatusCode::OK), "{statuts:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sante_repond_pendant_la_saturation() {
        let routeur = routeur_lent(1);

        let occupe = routeur.clone();
        let tache_lente = tokio::spawn(async move { occupe.oneshot(requete("/lent")).await });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let sonde = routeur.clone();
        let reponse_sante = sonde.oneshot(requete("/health")).await.unwrap();
        assert_eq!(reponse_sante.status(), StatusCode::OK);

        tache_lente.await.unwrap().unwrap();
    }

    // --- comportements deja acquis (domaine, roundtrip valide) --------------

    // Avant le correctif d'origine, ces deux tests faisaient paniquer le
    // moteur (`assert!` dans `engine.rs::base`/`coordonnees`), tuant la
    // connexion HTTP de l'appelant sans reponse propre.
    #[tokio::test]
    async fn write_room_hors_domaine_rejete_proprement() {
        let response = app()
            .oneshot(post(
                "/write",
                serde_json::json!({
                    "room": SALLES_MAX,
                    "cell": 0,
                    "tick": 0,
                    "entity": 0,
                    "pos": [0.0, 0.0, 0.0],
                    "yaw": 0.0,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["ok"], false);
        assert!(json["error"].as_str().unwrap().contains("hors domaine"));
    }

    #[tokio::test]
    async fn write_cell_hors_domaine_rejete_proprement() {
        let response = app()
            .oneshot(post(
                "/write",
                serde_json::json!({
                    "room": 0,
                    "cell": CELLULES_PAR_SALLE,
                    "tick": 0,
                    "entity": 0,
                    "pos": [0.0, 0.0, 0.0],
                    "yaw": 0.0,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn seal_room_hors_domaine_rejete_proprement() {
        let response = app()
            .oneshot(post(
                "/seal",
                serde_json::json!({ "room": SALLES_MAX, "cell": 0, "tick": 0 }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn seal_cell_hors_domaine_rejete_proprement() {
        let response = app()
            .oneshot(post(
                "/seal",
                serde_json::json!({ "room": 0, "cell": CELLULES_PAR_SALLE, "tick": 0 }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn read_room_hors_domaine_rejete_proprement() {
        let uri = format!("/read?room={SALLES_MAX}&cells=0&tick=0");
        let response = app().oneshot(get_authentifie(&uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // Cote lecture, une cellule hors domaine melangee a des cellules valides
    // est ecartee silencieusement (meme politique que le parsing existant
    // pour les entrees non numeriques) plutot que de faire paniquer le
    // moteur — la requete reste un succes.
    #[tokio::test]
    async fn read_cell_hors_domaine_ecartee_silencieusement() {
        let uri = format!("/read?room=0&cells=0,{CELLULES_PAR_SALLE},99999999&tick=0");
        let response = app().oneshot(get_authentifie(&uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ecriture_scellement_lecture_valides_fonctionnent_toujours() {
        let application = app();

        let response = application
            .clone()
            .oneshot(post(
                "/write",
                serde_json::json!({
                    "room": 1,
                    "cell": 1,
                    "tick": 1,
                    "entity": 1,
                    "pos": [1.0, 2.0, 3.0],
                    "yaw": 0.5,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["ok"], true);

        let response = application
            .clone()
            .oneshot(post(
                "/seal",
                serde_json::json!({ "room": 1, "cell": 1, "tick": 1 }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["entity_count"], 1);

        let response = application
            .oneshot(get_authentifie("/read?room=1&cells=1&tick=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["count"], 1);
    }

    #[tokio::test]
    async fn health_ne_touche_pas_le_moteur_et_repond_toujours() {
        let response = app().oneshot(get_sans_secret("/health")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

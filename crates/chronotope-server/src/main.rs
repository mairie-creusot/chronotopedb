//! Binaire d'integration — expose `ChronotopeStore` en HTTP/WebSocket pour
//! des tests reels contre une vraie pile pawchat (`docker-compose.dev.yml`),
//! en dehors du perimetre de mesure H1-H4 (qui restent des benchmarks/tests
//! en memoire, sans reseau — voir `chronotope-core/benches` et
//! `chronotope-sim`). Ce binaire n'est PAS le "vrai" transport envisage par
//! le concept (WebTransport, §3.3/§4 de docs/chronotope.md) : c'est un
//! harnais HTTP volontairement simple pour prouver que le moteur peut etre
//! atteint depuis l'exterieur du process, rien de plus.
//!
//! `/health` ne touche jamais le moteur : c'est deliberement le seul
//! endpoint garanti de repondre tant que `chronotope-core` est encore en
//! `todo!()` (voir le scaffold initial du depot), pour que le conteneur
//! puisse etre demarre et sonde des le squelette, avant meme que le moteur
//! soit implemente.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick,
    CELLULES_PAR_SALLE, SALLES_MAX,
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

mod banner;

struct AppState {
    engine: ChronotopeEngine,
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

    let state = Arc::new(AppState {
        engine: ChronotopeEngine::new(Horizon::default()),
    });

    let app = router(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3200);
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("chronotope-server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

/// Construit le `Router` sans lancer de serveur — separe de `main` pour que
/// les tests d'integration (`tower::ServiceExt::oneshot`) puissent driver
/// les routes sans ouvrir de vraie socket TCP.
fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/write", post(write))
        .route("/seal", post(seal))
        .route("/read", get(read))
        .with_state(state)
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

async fn write(State(state): State<Arc<AppState>>, Json(req): Json<WriteRequest>) -> Response {
    if room_ou_cell_hors_domaine(req.room, req.cell) {
        return (
            StatusCode::BAD_REQUEST,
            Json(WriteResponse {
                ok: false,
                error: Some(format!(
                    "room ou cell hors domaine (room < {SALLES_MAX}, cell < {CELLULES_PAR_SALLE})"
                )),
            }),
        )
            .into_response();
    }

    let result = state.engine.ecrire(
        RoomId(req.room),
        CellId(req.cell),
        Tick(req.tick),
        EntityId(req.entity),
        Pose {
            pos: req.pos,
            yaw: req.yaw,
        },
    );
    match result {
        Ok(()) => Json(WriteResponse {
            ok: true,
            error: None,
        })
        .into_response(),
        Err(e) => Json(WriteResponse {
            ok: false,
            error: Some(e.to_string()),
        })
        .into_response(),
    }
}

#[derive(Deserialize)]
struct SealRequest {
    room: u32,
    cell: u32,
    tick: u64,
}

async fn seal(State(state): State<Arc<AppState>>, Json(req): Json<SealRequest>) -> Response {
    if room_ou_cell_hors_domaine(req.room, req.cell) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "room ou cell hors domaine (room < {SALLES_MAX}, cell < {CELLULES_PAR_SALLE})"
                ),
            })),
        )
            .into_response();
    }

    let chronotope = state
        .engine
        .sceller(RoomId(req.room), CellId(req.cell), Tick(req.tick));
    Json(serde_json::json!({
        "room": chronotope.room.0,
        "cell": chronotope.cell.0,
        "tick": chronotope.tick.0,
        "sealed": chronotope.sealed,
        "entity_count": chronotope.entities.len(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ReadQuery {
    room: u32,
    cells: String, // liste separee par des virgules
    tick: u64,
}

async fn read(State(state): State<Arc<AppState>>, Query(q): Query<ReadQuery>) -> Response {
    if q.room >= SALLES_MAX {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("room hors domaine (room < {SALLES_MAX})"),
            })),
        )
            .into_response();
    }

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
    let chronotopes = state.engine.lire(RoomId(q.room), &cells, Tick(q.tick));
    Json(serde_json::json!({ "count": chronotopes.len() })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn app() -> Router {
        router(Arc::new(AppState {
            engine: ChronotopeEngine::new(Horizon::default()),
        }))
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // Avant le correctif, ces deux tests faisaient paniquer le moteur
    // (`assert!` dans `engine.rs::base`/`coordonnees`), tuant la connexion
    // HTTP de l'appelant sans reponse propre.
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
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // Cote lecture, une cellule hors domaine melangee a des cellules valides
    // est ecartee silencieusement (meme politique que le parsing existant
    // pour les entrees non numeriques) plutot que de faire paniquer le
    // moteur — la requete reste un succes.
    #[tokio::test]
    async fn read_cell_hors_domaine_ecartee_silencieusement() {
        let uri = format!("/read?room=0&cells=0,{CELLULES_PAR_SALLE},99999999&tick=0");
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app().oneshot(request).await.unwrap();
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

        let request = Request::builder()
            .method("GET")
            .uri("/read?room=1&cells=1&tick=1")
            .body(Body::empty())
            .unwrap();
        let response = application.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["count"], 1);
    }

    #[tokio::test]
    async fn health_ne_touche_pas_le_moteur_et_repond_toujours() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

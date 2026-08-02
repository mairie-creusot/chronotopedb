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
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chronotope_core::{
    CellId, ChronotopeEngine, ChronotopeStore, EntityId, Horizon, Pose, RoomId, Tick,
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

    let app = Router::new()
        .route("/health", get(health))
        .route("/write", post(write))
        .route("/seal", post(seal))
        .route("/read", get(read))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3200);
    let addr = format!("0.0.0.0:{port}");
    tracing::info!("chronotope-server listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "service": "chronotope-server" }))
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

async fn write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WriteRequest>,
) -> impl IntoResponse {
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
        }),
        Err(e) => Json(WriteResponse {
            ok: false,
            error: Some(e.to_string()),
        }),
    }
}

#[derive(Deserialize)]
struct SealRequest {
    room: u32,
    cell: u32,
    tick: u64,
}

async fn seal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SealRequest>,
) -> impl IntoResponse {
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
}

#[derive(Deserialize)]
struct ReadQuery {
    room: u32,
    cells: String, // liste separee par des virgules
    tick: u64,
}

async fn read(State(state): State<Arc<AppState>>, Query(q): Query<ReadQuery>) -> impl IntoResponse {
    let cells: Vec<CellId> = q
        .cells
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .map(CellId)
        .collect();
    let chronotopes = state.engine.lire(RoomId(q.room), &cells, Tick(q.tick));
    Json(serde_json::json!({ "count": chronotopes.len() }))
}

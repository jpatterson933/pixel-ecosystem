//! Gateway: fans live chunk events out to browser WebSocket clients.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Json,
};
use clap::Parser;
use common::WorldMeta;
use futures::{SinkExt, StreamExt};
use protocol::{subjects, ChunkEvents, ChunkState, ClientMessage, ServerMessage, StateRequest, WorldInfo};
use sim_core::ChunkCoord;
use tokio::sync::{mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "gateway")]
struct Args {
    #[arg(long, env = "WORLD_ID")]
    world: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats: String,
    #[arg(long, env = "BIND", default_value = "0.0.0.0:8080")]
    bind: String,
}

type ClientId = u64;

struct AppState {
    meta: Arc<WorldMeta>,
    client: async_nats::Client,
    subs: RwLock<HashMap<ChunkCoord, HashSet<ClientId>>>,
    clients: RwLock<HashMap<ClientId, mpsc::UnboundedSender<String>>>,
    tick: AtomicU64,
    next_id: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();
    let (client, js) = common::connect_nats(&args.nats).await?;
    let meta = Arc::new(common::load_world(&js, &args.world).await?);

    let state = Arc::new(AppState {
        tick: AtomicU64::new(meta.current_tick()),
        meta,
        client,
        subs: RwLock::new(HashMap::new()),
        clients: RwLock::new(HashMap::new()),
        next_id: AtomicU64::new(1),
    });

    tokio::spawn(fanout(state.clone()));

    let app = axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/world", get(world))
        .route("/ws", get(ws_upgrade))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, world = %args.world, "gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn fanout(state: Arc<AppState>) {
    let subject = format!("pe.{}.ev.*.*", state.meta.world_id);
    let mut sub = match state.client.subscribe(subject).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "event subscription failed");
            return;
        }
    };
    while let Some(msg) = sub.next().await {
        let Some((cx, cy)) = subjects::parse_events(&msg.subject) else { continue };
        let ce: ChunkEvents = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        state.tick.fetch_max(ce.tick, Ordering::Relaxed);
        let coord = ChunkCoord { cx, cy };
        let targets: Vec<ClientId> = {
            let subs = state.subs.read().await;
            match subs.get(&coord) {
                Some(set) if !set.is_empty() => set.iter().copied().collect(),
                _ => continue,
            }
        };
        let Ok(text) = serde_json::to_string(&ServerMessage::Events(ce)) else { continue };
        let clients = state.clients.read().await;
        for id in targets {
            if let Some(tx) = clients.get(&id) {
                let _ = tx.send(text.clone());
            }
        }
    }
}

async fn world(State(s): State<Arc<AppState>>) -> Json<WorldInfo> {
    Json(WorldInfo {
        world_id: s.meta.world_id.clone(),
        config: s.meta.config.clone(),
        summary_levels: s.meta.config.summary_levels(),
    })
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(s): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| session(socket, s))
}

async fn session(socket: WebSocket, state: Arc<AppState>) {
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    state.clients.write().await.insert(id, tx.clone());

    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let hello = ServerMessage::World {
        world_id: state.meta.world_id.clone(),
        config: state.meta.config.clone(),
        summary_levels: state.meta.config.summary_levels(),
        tick: state.tick.load(Ordering::Relaxed),
    };
    if let Ok(t) = serde_json::to_string(&hello) {
        let _ = tx.send(t);
    }

    let mut current: HashSet<ChunkCoord> = HashSet::new();
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(ClientMessage::Subscribe { chunks }) = serde_json::from_str::<ClientMessage>(&text) else {
            continue;
        };
        let wanted: HashSet<ChunkCoord> = chunks
            .into_iter()
            .map(|c| ChunkCoord { cx: c[0], cy: c[1] })
            .filter(|c| state.meta.config.contains_chunk(*c))
            .collect();
        let added: Vec<ChunkCoord> = wanted.difference(&current).copied().collect();
        let removed: Vec<ChunkCoord> = current.difference(&wanted).copied().collect();
        {
            let mut subs = state.subs.write().await;
            for c in &removed {
                if let Some(set) = subs.get_mut(c) {
                    set.remove(&id);
                    if set.is_empty() {
                        subs.remove(c);
                    }
                }
            }
            for c in &added {
                subs.entry(*c).or_default().insert(id);
            }
        }
        current = wanted;
        for c in added {
            let st = state.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let chunk_state = fetch_state(&st, c).await;
                if let Ok(t) = serde_json::to_string(&ServerMessage::Chunk(chunk_state)) {
                    let _ = tx.send(t);
                }
            });
        }
    }

    {
        let mut subs = state.subs.write().await;
        for c in &current {
            if let Some(set) = subs.get_mut(c) {
                set.remove(&id);
                if set.is_empty() {
                    subs.remove(c);
                }
            }
        }
    }
    state.clients.write().await.remove(&id);
    writer.abort();
}

async fn fetch_state(state: &AppState, c: ChunkCoord) -> ChunkState {
    let empty = ChunkState {
        tick: state.tick.load(Ordering::Relaxed),
        cx: c.cx,
        cy: c.cy,
        pixels: Vec::new(),
    };
    let Some(owner) = state.meta.directory.owner_of(c) else { return empty };
    let subject = subjects::state(&state.meta.world_id, owner);
    let Ok(payload) = serde_json::to_vec(&StateRequest { cx: c.cx, cy: c.cy }) else { return empty };
    match tokio::time::timeout(Duration::from_secs(3), state.client.request(subject, payload.into())).await {
        Ok(Ok(msg)) => serde_json::from_slice(&msg.payload).unwrap_or(empty),
        _ => empty,
    }
}

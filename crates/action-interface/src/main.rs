//! Action interface adapters: HTTP, WebSocket, and MCP over one router.

use std::sync::Arc;

use action_interface::Router;
use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use protocol::{ActionBatch, ActionClientMessage, ActionServerMessage, Envelope, WorldInfo};
use serde_json::{json, Value};
use sim_core::Action;
use tower_http::cors::CorsLayer;
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "action-interface")]
struct Args {
    #[arg(long, env = "WORLD_ID")]
    world: String,
    #[arg(long, env = "NATS_URL", default_value = "nats://localhost:4222")]
    nats: String,
    #[arg(long, env = "BIND", default_value = "0.0.0.0:8081")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();
    let (client, js) = common::connect_nats(&args.nats).await?;
    let meta = Arc::new(common::load_world(&js, &args.world).await?);
    let router = Arc::new(Router::new(client, meta));

    let app = axum::Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/world", get(world))
        .route("/actions", get(list_actions).post(http_actions))
        .route("/ws", get(ws_upgrade))
        .route("/mcp", post(mcp))
        .layer(CorsLayer::permissive())
        .with_state(router);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, world = %args.world, "action interface listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn world(State(r): State<Arc<Router>>) -> Json<WorldInfo> {
    Json(WorldInfo {
        world_id: r.meta.world_id.clone(),
        config: r.meta.config.clone(),
        summary_levels: r.meta.config.summary_levels(),
    })
}

async fn list_actions(State(r): State<Arc<Router>>) -> Json<Vec<action_interface::EnabledAction>> {
    Json(action_interface::enabled_actions(&r.meta))
}

async fn http_actions(State(r): State<Arc<Router>>, Json(batch): Json<ActionBatch>) -> impl IntoResponse {
    Json(r.submit(batch).await)
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(r): State<Arc<Router>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(socket, r))
}

async fn ws_session(socket: WebSocket, r: Arc<Router>) {
    let (mut tx, mut rx) = socket.split();
    while let Some(Ok(msg)) = rx.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let out = match serde_json::from_str::<ActionClientMessage>(&text) {
            Ok(ActionClientMessage::Actions { envelopes }) => {
                let res = r.submit(ActionBatch { envelopes }).await;
                ActionServerMessage::Results {
                    results: res.results,
                }
            }
            Err(e) => ActionServerMessage::Error {
                message: format!("bad message: {e}"),
            },
        };
        let Ok(payload) = serde_json::to_string(&out) else { continue };
        if tx.send(Message::Text(payload)).await.is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// MCP adapter: JSON-RPC over HTTP (Streamable HTTP transport, request/response only).
// ---------------------------------------------------------------------------

async fn mcp(State(r): State<Arc<Router>>, Json(req): Json<Value>) -> impl IntoResponse {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    if method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "pixel-ecosystem", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions(&r) })),
        "tools/call" => tools_call(&r, &params).await,
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    let body = match result {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": code, "message": message }
        }),
    };
    Json(body).into_response()
}

fn tool_definitions(r: &Router) -> Value {
    let mut tools: Vec<Value> = action_interface::enabled_actions(&r.meta)
        .into_iter()
        .map(|a| a.info)
        .map(|info| {
            json!({
                "name": info.name,
                "description": format!("{} x and y are the caller's last known position of the pixel.", info.description),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pixel_id": { "type": "integer" },
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "credential": { "type": "string" },
                        "params": info.schema
                    },
                    "required": ["pixel_id", "x", "y", "credential"]
                }
            })
        })
        .collect();
    tools.push(json!({
        "name": "world_info",
        "description": "The world's id and configuration.",
        "inputSchema": { "type": "object", "properties": {} }
    }));
    tools.push(json!({
        "name": "list_actions",
        "description": "Every action a pixel can take, with parameter schemas.",
        "inputSchema": { "type": "object", "properties": {} }
    }));
    Value::Array(tools)
}

async fn tools_call(r: &Router, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match name {
        "world_info" => {
            let info = WorldInfo {
                world_id: r.meta.world_id.clone(),
                config: r.meta.config.clone(),
                summary_levels: r.meta.config.summary_levels(),
            };
            Ok(text_content(&info, false))
        }
        "list_actions" => Ok(text_content(&action_interface::enabled_actions(&r.meta), false)),
        kind if r.meta.config.action_enabled(kind) => {
            let get_i = |k: &str| args.get(k).and_then(|v| v.as_i64());
            let (Some(pixel_id), Some(x), Some(y)) = (get_i("pixel_id"), get_i("x"), get_i("y")) else {
                return Err((-32602, "pixel_id, x, and y are required".into()));
            };
            let credential = args
                .get("credential")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let envelope = Envelope {
                pixel_id: pixel_id as u64,
                action: Action {
                    kind: kind.to_string(),
                    params: args.get("params").cloned().unwrap_or(Value::Null),
                },
                at: [x as u32, y as u32],
                credential,
            };
            let res = r
                .submit(ActionBatch {
                    envelopes: vec![envelope],
                })
                .await;
            let first = res.results.into_iter().next();
            let is_error = first.as_ref().is_some_and(|f| f.error.is_some());
            Ok(text_content(&first, is_error))
        }
        _ => Err((-32602, format!("unknown tool: {name}"))),
    }
}

fn text_content<T: serde::Serialize>(v: &T, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(v).unwrap_or_default() }],
        "isError": is_error
    })
}

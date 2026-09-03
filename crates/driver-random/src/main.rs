//! Reference driver. Pixels wander randomly; if `build` is enabled, each wandering
//! pixel starts a build with the world's `build_chance` per tick and then stays put,
//! sending `build` every tick to construct and then guard its structure.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use protocol::{ActionClientMessage, ActionServerMessage, DriverFile, Envelope};
use rand::{Rng, SeedableRng};
use serde_json::json;
use sim_core::{Action, Pixel, PixelId, RejectReason, WorldConfig};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "driver-random")]
struct Args {
    /// Directory written by worldgen: contains config.json, driver.json, genesis.json.
    #[arg(long)]
    world_dir: PathBuf,
    /// WebSocket adapter of the action interface.
    #[arg(long, env = "INTERFACE_URL", default_value = "ws://localhost:8081/ws")]
    interface: String,
    /// Envelopes per WebSocket message.
    #[arg(long, default_value_t = 2000)]
    batch_size: usize,
    /// Probability per wandering pixel per tick of sending `mark` instead of `move`.
    #[arg(long, env = "MARK_CHANCE", default_value_t = 0.0)]
    mark_chance: f64,
    /// Override the world's `build_chance` setting.
    #[arg(long, env = "BUILD_CHANCE")]
    build_chance: Option<f64>,
    /// Blueprint to build.
    #[arg(long, env = "BLUEPRINT", default_value = "boundary_wall")]
    blueprint: String,
}

const DIRS: [(i8, i8); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Wander,
    Building,
    Done,
}

#[derive(Debug, Clone, Copy)]
struct PState {
    at: [u32; 2],
    mode: Mode,
}

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();

    let cfg: WorldConfig = read_json(&args.world_dir.join("config.json"))?;
    let driver: DriverFile = read_json(&args.world_dir.join("driver.json"))?;
    let genesis: Vec<Pixel> = read_json(&args.world_dir.join("genesis.json"))?;
    let secret = hex::decode(driver.secret.trim())?;

    let enabled: HashSet<String> = cfg.actions.keys().cloned().collect();
    let can_move = enabled.contains("move");
    let can_mark = enabled.contains("mark") && args.mark_chance > 0.0;
    let build_chance = args.build_chance.unwrap_or_else(|| {
        cfg.action_settings("build")
            .get("build_chance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    });
    let can_build = enabled.contains("build") && build_chance > 0.0;

    let tokens: HashMap<PixelId, String> = genesis
        .iter()
        .map(|p| (p.id, protocol::pixel_token(&secret, p.id)))
        .collect();
    let states: Arc<Mutex<HashMap<PixelId, PState>>> = Arc::new(Mutex::new(
        genesis
            .iter()
            .map(|p| {
                (
                    p.id,
                    PState {
                        at: [p.x, p.y],
                        mode: Mode::Wander,
                    },
                )
            })
            .collect(),
    ));
    info!(
        world = %driver.world_id,
        pixels = tokens.len(),
        ?enabled,
        can_move,
        can_mark,
        can_build,
        build_chance,
        "driver loaded"
    );

    let mut rng = rand::rngs::StdRng::from_entropy();
    let interval = Duration::from_millis(cfg.tick_interval_ms());

    loop {
        let (ws, _) = match connect_async(&args.interface).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "connect failed, retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        info!(url = %args.interface, "connected");
        let (mut tx, mut rx) = ws.split();

        let states_reader = states.clone();
        let reader = tokio::spawn(async move {
            let mut standing: HashSet<PixelId> = HashSet::new();
            while let Some(Ok(msg)) = rx.next().await {
                let Message::Text(text) = msg else { continue };
                match serde_json::from_str::<ActionServerMessage>(&text) {
                    Ok(ActionServerMessage::Results { results }) => {
                        let mut st = states_reader.lock().await;
                        for r in results {
                            let Some(s) = st.get_mut(&r.pixel_id) else { continue };
                            if r.accepted || (r.reason.is_some() && r.reason != Some(RejectReason::NotFound)) {
                                s.at = [r.x, r.y];
                            }
                            if s.mode != Mode::Building {
                                continue;
                            }
                            let gave_up = r.error.is_some()
                                || matches!(
                                    r.reason,
                                    Some(RejectReason::Refused)
                                        | Some(RejectReason::InvalidParams)
                                        | Some(RejectReason::DisabledAction)
                                        | Some(RejectReason::UnknownAction)
                                );
                            if gave_up {
                                s.mode = Mode::Wander;
                                continue;
                            }
                            let status = r.data.as_ref().and_then(|d| d.get("status")).and_then(|v| v.as_str());
                            let progress = r.data.as_ref().and_then(|d| d.get("progress")).and_then(|v| v.as_u64());
                            let total = r.data.as_ref().and_then(|d| d.get("total")).and_then(|v| v.as_u64());
                            if status == Some("done") && progress == total && r.accepted {
                                s.mode = Mode::Done;
                                if standing.insert(r.pixel_id) {
                                    info!(pixel = r.pixel_id, standing = standing.len(), "structure complete");
                                }
                            }
                        }
                    }
                    Ok(ActionServerMessage::Error { message }) => warn!(%message, "interface error"),
                    Err(e) => warn!(error = %e, "bad results payload"),
                }
            }
        });

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut disconnected = false;
        while !disconnected {
            ticker.tick().await;
            let snapshot: Vec<(PixelId, PState)> = {
                let mut st = states.lock().await;
                let mut out = Vec::with_capacity(st.len());
                for (id, s) in st.iter_mut() {
                    if s.mode == Mode::Wander && can_build && rng.gen_bool(build_chance) {
                        s.mode = Mode::Building;
                    }
                    out.push((*id, *s));
                }
                out
            };
            for chunk in snapshot.chunks(args.batch_size) {
                let envelopes: Vec<Envelope> = chunk
                    .iter()
                    .filter_map(|(id, s)| {
                        let action = match s.mode {
                            Mode::Building => Action {
                                kind: "build".into(),
                                params: json!({ "blueprint": args.blueprint }),
                            },
                            Mode::Wander if can_mark && rng.gen_bool(args.mark_chance) => Action {
                                kind: "mark".into(),
                                params: json!({}),
                            },
                            Mode::Wander if can_move => {
                                let (dx, dy) = DIRS[rng.gen_range(0..DIRS.len())];
                                Action {
                                    kind: "move".into(),
                                    params: json!({ "dx": dx, "dy": dy }),
                                }
                            }
                            Mode::Wander | Mode::Done => return None,
                        };
                        Some(Envelope {
                            pixel_id: *id,
                            action,
                            at: s.at,
                            credential: tokens.get(id).cloned().unwrap_or_default(),
                        })
                    })
                    .collect();
                if envelopes.is_empty() {
                    continue;
                }
                let msg = ActionClientMessage::Actions { envelopes };
                let text = serde_json::to_string(&msg)?;
                if tx.send(Message::Text(text)).await.is_err() {
                    warn!("send failed, reconnecting");
                    disconnected = true;
                    break;
                }
            }
        }
        reader.abort();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parsing {}", path.display()))
}

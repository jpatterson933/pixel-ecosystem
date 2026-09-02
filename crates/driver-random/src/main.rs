//! Reference driver: every tick, move every pixel it controls in a random direction.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use protocol::{ActionClientMessage, ActionServerMessage, DriverFile, Envelope};
use rand::{Rng, SeedableRng};
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

#[tokio::main]
async fn main() -> Result<()> {
    common::init_logging();
    let args = Args::parse();

    let cfg: WorldConfig = read_json(&args.world_dir.join("config.json"))?;
    let driver: DriverFile = read_json(&args.world_dir.join("driver.json"))?;
    let genesis: Vec<Pixel> = read_json(&args.world_dir.join("genesis.json"))?;
    let secret = hex::decode(driver.secret.trim())?;

    let tokens: HashMap<PixelId, String> = genesis
        .iter()
        .map(|p| (p.id, protocol::pixel_token(&secret, p.id)))
        .collect();
    let positions: Arc<Mutex<HashMap<PixelId, [u32; 2]>>> = Arc::new(Mutex::new(
        genesis.iter().map(|p| (p.id, [p.x, p.y])).collect(),
    ));
    info!(world = %driver.world_id, pixels = tokens.len(), "driver loaded");

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

        let pos_reader = positions.clone();
        let reader = tokio::spawn(async move {
            while let Some(Ok(msg)) = rx.next().await {
                let Message::Text(text) = msg else { continue };
                match serde_json::from_str::<ActionServerMessage>(&text) {
                    Ok(ActionServerMessage::Results { results }) => {
                        let mut pos = pos_reader.lock().await;
                        for r in results {
                            if r.accepted || (r.reason.is_some() && r.reason != Some(RejectReason::NotFound)) {
                                pos.insert(r.pixel_id, [r.x, r.y]);
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
            let snapshot: Vec<(PixelId, [u32; 2])> = {
                let pos = positions.lock().await;
                pos.iter().map(|(id, p)| (*id, *p)).collect()
            };
            for chunk in snapshot.chunks(args.batch_size) {
                let envelopes: Vec<Envelope> = chunk
                    .iter()
                    .map(|(id, at)| {
                        let (dx, dy) = DIRS[rng.gen_range(0..DIRS.len())];
                        Envelope {
                            pixel_id: *id,
                            action: Action::Move { dx, dy },
                            at: *at,
                            credential: tokens.get(id).cloned().unwrap_or_default(),
                        }
                    })
                    .collect();
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

//! Wire formats and naming shared by every service. No logic beyond routing lookups.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sim_core::{hilbert, Action, Arrival, CellEntry, Chunk, ChunkCoord, Departure, Event, Pixel, PixelId, RejectReason, Tick, WorldConfig};

// ---------------------------------------------------------------------------
// NATS naming
// ---------------------------------------------------------------------------

pub mod subjects {
    /// JetStream stream holding every event of a world.
    pub fn stream_name(world: &str) -> String {
        format!("PE_{world}")
    }
    /// KV bucket holding config, directory, secret, genesis time.
    pub fn kv_bucket(world: &str) -> String {
        format!("pe_{world}")
    }
    /// Subject filter covering all events of a world.
    pub fn events_all(world: &str) -> String {
        format!("pe.{world}.ev.>")
    }
    /// Subject for one chunk's events.
    pub fn events(world: &str, cx: u32, cy: u32) -> String {
        format!("pe.{world}.ev.{cx}.{cy}")
    }
    /// Subject for world-level events (event zero).
    pub fn events_world(world: &str) -> String {
        format!("pe.{world}.ev.world.config")
    }
    /// Parse chunk coords out of an events subject. None for world-level subjects.
    pub fn parse_events(subject: &str) -> Option<(u32, u32)> {
        let mut parts = subject.rsplit('.');
        let cy = parts.next()?.parse().ok()?;
        let cx = parts.next()?.parse().ok()?;
        Some((cx, cy))
    }
    /// Request/reply: action batches to a region server.
    pub fn actions(world: &str, server: &str) -> String {
        format!("pe.{world}.act.{server}")
    }
    /// Pixel handoffs to a region server.
    pub fn handoff(world: &str, server: &str) -> String {
        format!("pe.{world}.ho.{server}")
    }
    /// Request/reply: chunk state from a region server.
    pub fn state(world: &str, server: &str) -> String {
        format!("pe.{world}.st.{server}")
    }
}

pub mod kv_keys {
    pub const CONFIG: &str = "config";
    pub const DIRECTORY: &str = "directory";
    pub const SECRET: &str = "secret";
    pub const GENESIS_MS: &str = "genesis_ms";
}

pub mod paths {
    pub fn snapshot(world: &str, cx: u32, cy: u32) -> String {
        format!("snapshots/{world}/{cx}/{cy}.json")
    }
    pub fn tile(world: &str, level: u32, tx: u32, ty: u32) -> String {
        format!("tiles/{world}/{level}/{tx}/{ty}.png")
    }
    pub fn tile_meta(world: &str) -> String {
        format!("tiles/{world}/meta.json")
    }
}

// ---------------------------------------------------------------------------
// Directory: chunk ownership
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipRange {
    /// Inclusive Hilbert index start.
    pub start: u64,
    /// Exclusive Hilbert index end.
    pub end: u64,
    pub server_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Directory {
    pub order: u32,
    pub ranges: Vec<OwnershipRange>,
}

impl Directory {
    pub fn single(cfg: &WorldConfig, server_id: &str) -> Self {
        let order = cfg.hilbert_order();
        Directory {
            order,
            ranges: vec![OwnershipRange {
                start: 0,
                end: hilbert::index_space(order),
                server_id: server_id.to_string(),
            }],
        }
    }

    pub fn owner_of(&self, c: ChunkCoord) -> Option<&str> {
        let idx = hilbert::chunk_index(self.order, c);
        self.ranges
            .iter()
            .find(|r| idx >= r.start && idx < r.end)
            .map(|r| r.server_id.as_str())
    }

    pub fn ranges_for(&self, server_id: &str) -> Vec<&OwnershipRange> {
        self.ranges.iter().filter(|r| r.server_id == server_id).collect()
    }

    /// Every in-map chunk owned by a server.
    pub fn chunks_for(&self, cfg: &WorldConfig, server_id: &str) -> Vec<ChunkCoord> {
        let mut out = Vec::new();
        for r in self.ranges_for(server_id) {
            for idx in r.start..r.end {
                let c = hilbert::chunk_at(self.order, idx);
                if cfg.contains_chunk(c) {
                    out.push(c);
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Event stream payloads
// ---------------------------------------------------------------------------

/// One chunk's events for one tick. The unit published to the log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkEvents {
    pub tick: Tick,
    pub cx: u32,
    pub cy: u32,
    pub events: Vec<Event>,
}

/// Periodic snapshot of a chunk. `seq` is the stream sequence of the last
/// event folded into it, so replay resumes from `seq + 1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: Tick,
    pub seq: u64,
    pub chunk: Chunk,
    /// Pixels that departed another chunk for this one and had not yet arrived.
    #[serde(default)]
    pub pending_arrivals: Vec<Arrival>,
}

// ---------------------------------------------------------------------------
// Action interface
// ---------------------------------------------------------------------------

/// The one contract for acting on the world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub pixel_id: PixelId,
    pub action: Action,
    /// Caller's last known position of the pixel. Used only to route to the
    /// owning chunk; the region server locates the pixel by id.
    pub at: [u32; 2],
    pub credential: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionBatch {
    pub envelopes: Vec<Envelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub pixel_id: PixelId,
    pub accepted: bool,
    pub tick: Tick,
    pub x: u32,
    pub y: u32,
    pub reason: Option<RejectReason>,
    /// Routing or auth failure before the world saw the action.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResults {
    pub results: Vec<ActionResult>,
}

/// Interface to region server, already authenticated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerActionBatch {
    pub actions: Vec<ServerAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerAction {
    pub pixel_id: PixelId,
    pub action: Action,
    pub at: [u32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerActionResults {
    pub tick: Tick,
    pub results: Vec<ActionResult>,
}

/// WebSocket adapter of the action interface: driver -> interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionClientMessage {
    Actions { envelopes: Vec<Envelope> },
}

/// WebSocket adapter of the action interface: interface -> driver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionServerMessage {
    Results { results: Vec<ActionResult> },
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Region server internals
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffMessage {
    pub tick: Tick,
    pub departures: Vec<Departure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateRequest {
    pub cx: u32,
    pub cy: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkState {
    pub tick: Tick,
    pub cx: u32,
    pub cy: u32,
    pub pixels: Vec<Pixel>,
    #[serde(default)]
    pub cells: Vec<CellEntry>,
}

// ---------------------------------------------------------------------------
// Gateway <-> client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Replace the client's full subscription set.
    Subscribe { chunks: Vec<[u32; 2]> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    World {
        world_id: String,
        config: WorldConfig,
        summary_levels: u32,
        tick: Tick,
    },
    Chunk(ChunkState),
    Events(ChunkEvents),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldInfo {
    pub world_id: String,
    pub config: WorldConfig,
    pub summary_levels: u32,
}

// ---------------------------------------------------------------------------
// Files written by worldgen
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverFile {
    pub world_id: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileMeta {
    pub world_id: String,
    pub levels: u32,
    pub map_width: u32,
    pub map_height: u32,
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

type HmacSha256 = Hmac<Sha256>;

/// Control token for a pixel: HMAC-SHA256(secret, pixel_id) truncated to 128 bits.
pub fn pixel_token(secret: &[u8], pixel_id: PixelId) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&pixel_id.to_le_bytes());
    let bytes = mac.finalize().into_bytes();
    hex::encode(&bytes[..16])
}

pub fn verify_token(secret: &[u8], pixel_id: PixelId, token: &str) -> bool {
    let expected = pixel_token(secret, pixel_id);
    constant_time_eq(expected.as_bytes(), token.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Current tick for a world given its genesis time.
pub fn tick_at(cfg: &WorldConfig, genesis_ms: u64, now_ms: u64) -> Tick {
    if now_ms <= genesis_ms {
        return 0;
    }
    (now_ms - genesis_ms) * cfg.tick_rate as u64 / 1000
}

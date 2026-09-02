use serde::{Deserialize, Serialize};

use crate::CHUNK_SIZE;

pub type PixelId = u64;
pub type Tick = u64;

/// RGBA color. Serialized as a 4-element array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub fn from_rgb24(v: u32) -> Self {
        Rgba([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pixel {
    pub id: PixelId,
    pub x: u32,
    pub y: u32,
    pub color: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkCoord {
    pub cx: u32,
    pub cy: u32,
}

impl ChunkCoord {
    pub fn of_cell(x: u32, y: u32) -> Self {
        ChunkCoord {
            cx: x / CHUNK_SIZE,
            cy: y / CHUNK_SIZE,
        }
    }
}

/// The only action in version one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Move { dx: i8, dy: i8 },
}

impl Action {
    pub fn is_valid(&self) -> bool {
        match self {
            Action::Move { dx, dy } => {
                (-1..=1).contains(dx) && (-1..=1).contains(dy) && !(*dx == 0 && *dy == 0)
            }
        }
    }
}

/// An action queued for resolution at the next tick.
/// `seq` is arrival order and is the primary sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedAction {
    pub seq: u64,
    pub pixel_id: PixelId,
    pub action: Action,
}

/// A pixel arriving from another chunk, to be placed before actions resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub pixel: Pixel,
}

/// A pixel leaving this chunk for another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Departure {
    pub pixel: Pixel,
    pub to: ChunkCoord,
}

/// Events are the only thing the world emits. Every accepted change is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    /// Event zero of a world.
    Config { config: crate::WorldConfig },
    /// Genesis placement.
    Placed { id: PixelId, x: u32, y: u32, color: Rgba },
    /// Move within a chunk.
    Moved { id: PixelId, from: [u32; 2], to: [u32; 2] },
    /// Pixel left this chunk. The destination chunk emits Arrived next tick.
    Departed { id: PixelId, from: [u32; 2], to: [u32; 2] },
    /// Pixel entered this chunk.
    Arrived { id: PixelId, x: u32, y: u32, color: Rgba },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Occupied,
    OffMap,
    NotFound,
    InvalidAction,
}

/// Per-action outcome, returned to the submitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub seq: u64,
    pub pixel_id: PixelId,
    pub accepted: bool,
    pub x: u32,
    pub y: u32,
    pub reason: Option<RejectReason>,
}

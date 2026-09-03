//! Simulation core: the rules of the world and nothing else.
//! No networking, no storage, no clock. Deterministic by construction.

pub mod actions;
pub mod chunk;
pub mod config;
pub mod generate;
pub mod hilbert;
pub mod types;

pub use actions::{registry, ActionContext, ActionInfo, ActionModule, Module};
pub use chunk::{Cell, CellEntry, Chunk, TickInput, TickOutput};
pub use config::WorldConfig;
pub use types::*;

/// Cells per chunk side. A chunk is CHUNK_SIZE x CHUNK_SIZE cells.
pub const CHUNK_SIZE: u32 = 256;
/// Texels per summary tile side.
pub const TILE_SIZE: u32 = 256;
/// Snapshot every this many ticks.
pub const SNAPSHOT_INTERVAL_TICKS: u64 = 1000;
/// Default ticks per second when config omits it.
pub const DEFAULT_TICK_RATE: u32 = 10;
/// Maximum screen pixels per cell in the client.
pub const MAX_ZOOM: u32 = 64;
/// Tile write cadence in milliseconds.
pub const TILE_WRITE_INTERVAL_MS: u64 = 1000;

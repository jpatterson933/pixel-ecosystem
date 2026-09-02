use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ChunkCoord, CHUNK_SIZE};

/// The immutable definition of a world. Same config, same world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldConfig {
    pub map_width: u32,
    pub map_height: u32,
    pub pixel_count: u64,
    pub color_count: u32,
    #[serde(default = "default_tick_rate")]
    pub tick_rate: u32,
    pub seed: u64,
}

fn default_tick_rate() -> u32 {
    crate::DEFAULT_TICK_RATE
}

impl WorldConfig {
    /// World identity: first 16 hex chars of the SHA-256 of the canonical config.
    pub fn world_id(&self) -> String {
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}",
            self.map_width,
            self.map_height,
            self.pixel_count,
            self.color_count,
            self.tick_rate,
            self.seed
        );
        let digest = Sha256::digest(canonical.as_bytes());
        hex::encode(&digest[..8])
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.map_width == 0 || self.map_height == 0 {
            return Err("map dimensions must be positive".into());
        }
        let cells = self.map_width as u64 * self.map_height as u64;
        if self.pixel_count > cells {
            return Err("pixel_count exceeds cell count".into());
        }
        if self.color_count == 0 || self.color_count as u64 > (1u64 << 24) {
            return Err("color_count must be in 1..=16777216".into());
        }
        if self.tick_rate == 0 || self.tick_rate > 1000 {
            return Err("tick_rate must be in 1..=1000".into());
        }
        Ok(())
    }

    pub fn chunks_x(&self) -> u32 {
        self.map_width.div_ceil(CHUNK_SIZE)
    }

    pub fn chunks_y(&self) -> u32 {
        self.map_height.div_ceil(CHUNK_SIZE)
    }

    /// Hilbert curve order covering the chunk grid: smallest power of two >= both dims.
    pub fn hilbert_order(&self) -> u32 {
        let n = self.chunks_x().max(self.chunks_y());
        n.next_power_of_two().max(1)
    }

    pub fn contains_chunk(&self, c: ChunkCoord) -> bool {
        c.cx < self.chunks_x() && c.cy < self.chunks_y()
    }

    pub fn contains_cell(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && (x as u64) < self.map_width as u64 && (y as u64) < self.map_height as u64
    }

    /// Number of summary pyramid levels above level zero. Level L has one texel per
    /// 2^L x 2^L cells. The top level fits the whole map in a single tile.
    pub fn summary_levels(&self) -> u32 {
        let max_dim = self.map_width.max(self.map_height) as f64;
        let l = (max_dim / crate::TILE_SIZE as f64).log2().ceil();
        (l.max(1.0)) as u32
    }

    pub fn tick_interval_ms(&self) -> u64 {
        (1000.0 / self.tick_rate as f64).round().max(1.0) as u64
    }
}

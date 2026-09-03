use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ChunkCoord, CHUNK_SIZE};

/// The immutable definition of a world. Same config, same world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldConfig {
    pub map_width: u32,
    pub map_height: u32,
    pub pixel_count: u64,
    pub color_count: u32,
    #[serde(default = "default_tick_rate")]
    pub tick_rate: u32,
    pub seed: u64,
    /// Enabled action modules and their world-level settings. Absent means every
    /// registered module with default settings; `finalize_actions` fills it in.
    #[serde(default)]
    pub actions: BTreeMap<String, Value>,
}

fn default_tick_rate() -> u32 {
    crate::DEFAULT_TICK_RATE
}

impl WorldConfig {
    /// World identity: first 16 hex chars of the SHA-256 of the canonical config.
    pub fn world_id(&self) -> String {
        let actions = serde_json::to_string(&self.actions).unwrap_or_default();
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.map_width,
            self.map_height,
            self.pixel_count,
            self.color_count,
            self.tick_rate,
            self.seed,
            actions
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
        for (name, settings) in &self.actions {
            let Some(module) = crate::registry().get(name) else {
                return Err(format!("unknown action module: {name}"));
            };
            module
                .validate_settings(settings)
                .map_err(|e| format!("bad settings for action {name}: {e}"))?;
        }
        Ok(())
    }

    /// Resolve the enabled action set. `only` restricts to the given names; otherwise
    /// an empty map becomes every registered module. Missing settings get defaults,
    /// and given settings are merged over defaults so partial settings work.
    pub fn finalize_actions(&mut self, only: Option<&[String]>) -> Result<(), String> {
        let reg = crate::registry();
        let names: Vec<String> = match only {
            Some(list) => list.to_vec(),
            None if self.actions.is_empty() => reg.names().iter().map(|n| n.to_string()).collect(),
            None => self.actions.keys().cloned().collect(),
        };
        let mut out = BTreeMap::new();
        for name in names {
            let Some(module) = reg.get(&name) else {
                return Err(format!("unknown action module: {name}"));
            };
            let mut settings = module.default_settings();
            if let (Value::Object(base), Some(Value::Object(given))) = (&settings, self.actions.get(&name)) {
                let mut merged = base.clone();
                for (k, v) in given {
                    merged.insert(k.clone(), v.clone());
                }
                settings = Value::Object(merged);
            }
            out.insert(name, settings);
        }
        self.actions = out;
        Ok(())
    }

    pub fn action_enabled(&self, name: &str) -> bool {
        self.actions.contains_key(name)
    }

    pub fn action_settings(&self, name: &str) -> Value {
        self.actions.get(name).cloned().unwrap_or(Value::Object(Default::default()))
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

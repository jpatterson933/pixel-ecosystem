//! Action modules. Actions are open: add a file here and a line in `registry_modules`.
//! Events are closed: modules can only act through `ActionContext`, which emits primitives.

use std::collections::HashMap;
use std::sync::OnceLock;

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Chunk, ChunkCoord, Departure, Event, Pixel, PixelId, RejectReason, Rgba, Tick, WorldConfig};

pub mod build;
pub mod mark;
#[path = "move.rs"]
pub mod move_;

/// Every action module in the world. Order is irrelevant; names must be unique.
fn registry_modules() -> Vec<Box<dyn ActionModule>> {
    vec![Box::new(move_::Move), Box::new(mark::Mark), Box::new(build::Build)]
}

// ---------------------------------------------------------------------------
// Module traits
// ---------------------------------------------------------------------------

/// What a module author implements. Typed params, one resolve function.
pub trait Module: Send + Sync + 'static {
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    /// Per-action parameters sent by the driver.
    type Params: DeserializeOwned + JsonSchema;
    /// World-level settings from the config's `actions` map. Use an empty struct if none.
    type Settings: DeserializeOwned + Serialize + Default + JsonSchema;

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, params: Self::Params) -> Result<(), RejectReason>;
}

/// Object-safe view used by the registry, the tick loop, and the action interface.
pub trait ActionModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> Value;
    fn settings_schema(&self) -> Value;
    fn default_settings(&self) -> Value;
    fn validate(&self, params: &Value) -> Result<(), String>;
    fn validate_settings(&self, settings: &Value) -> Result<(), String>;
    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, params: &Value) -> Result<(), RejectReason>;
}

fn schema_of<T: JsonSchema>() -> Value {
    let root = schemars::gen::SchemaGenerator::default().into_root_schema_for::<T>();
    serde_json::to_value(root).unwrap_or(Value::Null)
}

fn normalize(params: &Value) -> Value {
    if params.is_null() {
        Value::Object(Default::default())
    } else {
        params.clone()
    }
}

impl<M: Module> ActionModule for M {
    fn name(&self) -> &'static str {
        M::NAME
    }

    fn description(&self) -> &'static str {
        M::DESCRIPTION
    }

    fn schema(&self) -> Value {
        schema_of::<M::Params>()
    }

    fn settings_schema(&self) -> Value {
        schema_of::<M::Settings>()
    }

    fn default_settings(&self) -> Value {
        serde_json::to_value(M::Settings::default()).unwrap_or(Value::Object(Default::default()))
    }

    fn validate(&self, params: &Value) -> Result<(), String> {
        serde_json::from_value::<M::Params>(normalize(params))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn validate_settings(&self, settings: &Value) -> Result<(), String> {
        serde_json::from_value::<M::Settings>(normalize(settings))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, params: &Value) -> Result<(), RejectReason> {
        let typed: M::Params =
            serde_json::from_value(normalize(params)).map_err(|_| RejectReason::InvalidParams)?;
        Module::resolve(self, ctx, pixel, typed)
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionInfo {
    pub name: String,
    pub description: String,
    pub schema: Value,
    pub settings_schema: Value,
    pub default_settings: Value,
}

pub struct Registry {
    modules: Vec<Box<dyn ActionModule>>,
    by_name: HashMap<&'static str, usize>,
}

impl Registry {
    fn build() -> Self {
        let modules = registry_modules();
        let mut by_name = HashMap::with_capacity(modules.len());
        for (i, m) in modules.iter().enumerate() {
            let prev = by_name.insert(m.name(), i);
            assert!(prev.is_none(), "duplicate action module name: {}", m.name());
        }
        Registry { modules, by_name }
    }

    pub fn get(&self, name: &str) -> Option<&dyn ActionModule> {
        self.by_name.get(name).map(|i| self.modules[*i].as_ref())
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.modules.iter().map(|m| m.name()).collect()
    }

    pub fn infos(&self) -> Vec<ActionInfo> {
        self.modules
            .iter()
            .map(|m| ActionInfo {
                name: m.name().to_string(),
                description: m.description().to_string(),
                schema: m.schema(),
                settings_schema: m.settings_schema(),
                default_settings: m.default_settings(),
            })
            .collect()
    }
}

pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::build)
}

// ---------------------------------------------------------------------------
// ActionContext: the only surface a module has on the world
// ---------------------------------------------------------------------------

/// Scoped to the acting pixel's chunk for one action at one tick. Reads see the
/// chunk as it was when the action began. Writes are buffered as primitive events
/// and applied by the chunk only if the module returns Ok.
pub struct ActionContext<'a> {
    chunk: &'a Chunk,
    cfg: &'a WorldConfig,
    tick: Tick,
    seq: u64,
    module: &'static str,
    pixel: Pixel,
    moved: bool,
    events: Vec<Event>,
    departure: Option<Departure>,
    rng: Option<ChaCha8Rng>,
    reply: Option<Value>,
}

impl<'a> ActionContext<'a> {
    pub(crate) fn new(
        chunk: &'a Chunk,
        cfg: &'a WorldConfig,
        tick: Tick,
        seq: u64,
        module: &'static str,
        pixel: Pixel,
    ) -> Self {
        ActionContext {
            chunk,
            cfg,
            tick,
            seq,
            module,
            pixel,
            moved: false,
            events: Vec::new(),
            departure: None,
            rng: None,
            reply: None,
        }
    }

    pub(crate) fn finish(self) -> (Vec<Event>, Option<Departure>, Pixel, Option<Value>) {
        (self.events, self.departure, self.pixel, self.reply)
    }

    pub fn config(&self) -> &WorldConfig {
        self.cfg
    }

    /// This module's world-level settings, typed.
    pub fn settings<S: DeserializeOwned + Default>(&self) -> S {
        serde_json::from_value(self.cfg.action_settings(self.module)).unwrap_or_default()
    }

    /// Payload returned to the driver alongside the outcome.
    pub fn reply(&mut self, value: Value) {
        self.reply = Some(value);
    }

    /// Whether a cell lies inside this chunk and the map.
    pub fn cell_in_chunk(&self, x: i64, y: i64) -> bool {
        self.in_chunk(x, y)
    }

    pub fn tick(&self) -> Tick {
        self.tick
    }

    pub fn chunk(&self) -> ChunkCoord {
        self.chunk.coord
    }

    /// Deterministic randomness: same world, tick, chunk, and action order give the same stream.
    pub fn rng(&mut self) -> &mut ChaCha8Rng {
        if self.rng.is_none() {
            let c = self.chunk.coord;
            let mut s = self.cfg.seed;
            for v in [self.tick, ((c.cx as u64) << 32) | c.cy as u64, self.seq] {
                s ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15);
                s = (s ^ (s >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
                s = (s ^ (s >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
                s ^= s >> 31;
            }
            self.rng = Some(ChaCha8Rng::seed_from_u64(s));
        }
        self.rng.as_mut().expect("initialized above")
    }

    fn in_chunk(&self, x: i64, y: i64) -> bool {
        self.cfg.contains_cell(x, y) && self.chunk.contains_cell(x as u32, y as u32)
    }

    /// Pixel occupying a cell in this chunk, if any.
    pub fn occupant(&self, x: i64, y: i64) -> Option<PixelId> {
        if !self.in_chunk(x, y) {
            return None;
        }
        self.chunk.occupancy.get(&(x as u32, y as u32)).copied()
    }

    /// Terrain color at a cell in this chunk, if any.
    pub fn terrain(&self, x: i64, y: i64) -> Option<Rgba> {
        if !self.in_chunk(x, y) {
            return None;
        }
        self.chunk.cells.get(&(x as u32, y as u32)).copied()
    }

    /// This module's attribute on a pixel in this chunk.
    pub fn attribute(&self, pixel: PixelId, key: &str) -> Option<&Value> {
        self.chunk.attributes.get(&pixel)?.get(self.module)?.get(key)
    }

    /// This module's attribute `key` on every pixel in this chunk that has it.
    pub fn attributes_in_chunk(&self, key: &str) -> Vec<(PixelId, &Value)> {
        self.chunk
            .attributes
            .iter()
            .filter_map(|(id, bag)| bag.get(self.module)?.get(key).map(|v| (*id, v)))
            .collect()
    }

    /// Move the acting pixel to an adjacent cell. Exactly one move per action.
    pub fn move_to(&mut self, x: i64, y: i64) -> Result<(), RejectReason> {
        if self.moved {
            return Err(RejectReason::Refused);
        }
        if (x - self.pixel.x as i64).abs() > 1 || (y - self.pixel.y as i64).abs() > 1 {
            return Err(RejectReason::Refused);
        }
        if !self.cfg.contains_cell(x, y) {
            return Err(RejectReason::OffMap);
        }
        let (nx, ny) = (x as u32, y as u32);
        let from = [self.pixel.x, self.pixel.y];
        let dest = ChunkCoord::of_cell(nx, ny);

        if dest == self.chunk.coord {
            if self.chunk.occupancy.contains_key(&(nx, ny)) {
                return Err(RejectReason::Occupied);
            }
            self.events.push(Event::Moved {
                id: self.pixel.id,
                from,
                to: [nx, ny],
            });
        } else {
            self.events.push(Event::Departed {
                id: self.pixel.id,
                from,
                to: [nx, ny],
            });
            self.departure = Some(Departure {
                pixel: Pixel {
                    x: nx,
                    y: ny,
                    ..self.pixel
                },
                to: dest,
                attributes: self.chunk.attributes.get(&self.pixel.id).cloned().unwrap_or_default(),
            });
        }
        self.pixel.x = nx;
        self.pixel.y = ny;
        self.moved = true;
        Ok(())
    }

    /// Set or clear terrain at a cell in this chunk.
    pub fn set_cell(&mut self, x: i64, y: i64, color: Option<Rgba>) -> Result<(), RejectReason> {
        if !self.in_chunk(x, y) {
            return Err(RejectReason::OffMap);
        }
        self.events.push(Event::CellSet {
            x: x as u32,
            y: y as u32,
            color,
        });
        Ok(())
    }

    /// Set this module's attribute on a pixel in this chunk.
    pub fn set_attribute(&mut self, pixel: PixelId, key: &str, value: Value) -> Result<(), RejectReason> {
        if pixel != self.pixel.id && !self.chunk.pixels.contains_key(&pixel) {
            return Err(RejectReason::NotFound);
        }
        self.events.push(Event::AttributeSet {
            id: pixel,
            module: self.module.to_string(),
            key: key.to_string(),
            value,
        });
        Ok(())
    }
}

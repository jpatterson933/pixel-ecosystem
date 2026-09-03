use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::actions::{registry, ActionContext};
use crate::{
    Arrival, Attributes, ChunkCoord, Departure, Event, Outcome, Pixel, PixelId, QueuedAction,
    RejectReason, Rgba, Tick, WorldConfig, CHUNK_SIZE,
};

/// One chunk's authoritative state. Sparse: only pixels, terrain, and attributes are stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ChunkRepr", into = "ChunkRepr")]
pub struct Chunk {
    pub coord: ChunkCoord,
    pub pixels: HashMap<PixelId, Pixel>,
    /// Cell (x, y) in map coordinates to the pixel occupying it.
    pub occupancy: HashMap<(u32, u32), PixelId>,
    /// Terrain color per cell. Absent means white.
    pub cells: HashMap<(u32, u32), Rgba>,
    /// Module-owned per-pixel state.
    pub attributes: HashMap<PixelId, Attributes>,
}

/// Serialized form: JSON needs string keys, so maps keyed by tuples become lists.
#[derive(Serialize, Deserialize)]
struct ChunkRepr {
    coord: ChunkCoord,
    pixels: Vec<Pixel>,
    #[serde(default)]
    cells: Vec<CellEntry>,
    #[serde(default)]
    attributes: Vec<(PixelId, Attributes)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellEntry {
    pub x: u32,
    pub y: u32,
    pub color: Rgba,
}

impl From<ChunkRepr> for Chunk {
    fn from(r: ChunkRepr) -> Self {
        let mut c = Chunk::new(r.coord);
        for p in r.pixels {
            c.insert(p);
        }
        for e in r.cells {
            c.cells.insert((e.x, e.y), e.color);
        }
        for (id, a) in r.attributes {
            c.attributes.insert(id, a);
        }
        c
    }
}

impl From<Chunk> for ChunkRepr {
    fn from(c: Chunk) -> Self {
        ChunkRepr {
            coord: c.coord,
            pixels: c.pixels.into_values().collect(),
            cells: c
                .cells
                .into_iter()
                .map(|((x, y), color)| CellEntry { x, y, color })
                .collect(),
            attributes: c.attributes.into_iter().collect(),
        }
    }
}

pub struct TickInput {
    pub tick: Tick,
    pub arrivals: Vec<Arrival>,
    pub actions: Vec<QueuedAction>,
}

#[derive(Debug, Default)]
pub struct TickOutput {
    pub events: Vec<Event>,
    pub outcomes: Vec<Outcome>,
    pub departures: Vec<Departure>,
}

impl Chunk {
    pub fn new(coord: ChunkCoord) -> Self {
        Chunk {
            coord,
            pixels: HashMap::new(),
            occupancy: HashMap::new(),
            cells: HashMap::new(),
            attributes: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty() && self.cells.is_empty()
    }

    pub fn contains_cell(&self, x: u32, y: u32) -> bool {
        ChunkCoord::of_cell(x, y) == self.coord
    }

    pub fn cell_entries(&self) -> Vec<CellEntry> {
        self.cells
            .iter()
            .map(|((x, y), color)| CellEntry {
                x: *x,
                y: *y,
                color: *color,
            })
            .collect()
    }

    /// Insert without emitting events. Used for genesis, arrivals, and replay.
    pub fn insert(&mut self, p: Pixel) {
        self.occupancy.insert((p.x, p.y), p.id);
        self.pixels.insert(p.id, p);
    }

    pub fn remove(&mut self, id: PixelId) -> Option<Pixel> {
        let p = self.pixels.remove(&id)?;
        self.occupancy.remove(&(p.x, p.y));
        Some(p)
    }

    /// Apply a single primitive event to this chunk's state. Replay and live
    /// resolution both go through here, so state is always the fold of the log.
    pub fn apply(&mut self, ev: &Event) {
        match ev {
            Event::Config { .. } => {}
            Event::Placed { id, x, y, color } => {
                self.insert(Pixel {
                    id: *id,
                    x: *x,
                    y: *y,
                    color: *color,
                });
            }
            Event::Arrived {
                id,
                x,
                y,
                color,
                attributes,
            } => {
                self.insert(Pixel {
                    id: *id,
                    x: *x,
                    y: *y,
                    color: *color,
                });
                if !attributes.is_empty() {
                    self.attributes.insert(*id, attributes.clone());
                }
            }
            Event::Moved { id, to, .. } => {
                if let Some(mut p) = self.remove(*id) {
                    p.x = to[0];
                    p.y = to[1];
                    self.insert(p);
                }
            }
            Event::Departed { id, .. } => {
                self.remove(*id);
                self.attributes.remove(id);
            }
            Event::CellSet { x, y, color } => match color {
                Some(c) => {
                    self.cells.insert((*x, *y), *c);
                }
                None => {
                    self.cells.remove(&(*x, *y));
                }
            },
            Event::AttributeSet {
                id,
                module,
                key,
                value,
            } => {
                self.attributes
                    .entry(*id)
                    .or_default()
                    .entry(module.clone())
                    .or_default()
                    .insert(key.clone(), value.clone());
            }
        }
    }

    /// Advance this chunk one tick. Pure with respect to everything outside the chunk.
    ///
    /// Order: arrivals first (sorted by pixel id), then actions (sorted by seq, then pixel id).
    pub fn tick(&mut self, cfg: &WorldConfig, mut input: TickInput) -> TickOutput {
        let mut out = TickOutput::default();

        // Arrivals. If the target cell has been taken since the pixel departed its
        // source chunk, place it at the nearest free cell within this chunk so a
        // pixel is never lost.
        input.arrivals.sort_by_key(|a| a.pixel.id);
        for a in input.arrivals {
            let mut p = a.pixel;
            if !self.contains_cell(p.x, p.y) || self.pixels.contains_key(&p.id) {
                continue;
            }
            if self.occupancy.contains_key(&(p.x, p.y)) {
                match self.nearest_free(cfg, p.x, p.y) {
                    Some((nx, ny)) => {
                        p.x = nx;
                        p.y = ny;
                    }
                    None => continue,
                }
            }
            let ev = Event::Arrived {
                id: p.id,
                x: p.x,
                y: p.y,
                color: p.color,
                attributes: a.attributes,
            };
            self.apply(&ev);
            out.events.push(ev);
        }

        // Actions.
        input.actions.sort_by_key(|a| (a.seq, a.pixel_id));
        for qa in input.actions {
            let outcome = self.resolve(cfg, input.tick, &qa, &mut out);
            out.outcomes.push(outcome);
        }

        out
    }

    fn resolve(&mut self, cfg: &WorldConfig, tick: Tick, qa: &QueuedAction, out: &mut TickOutput) -> Outcome {
        let reject = |p: Option<&Pixel>, reason: RejectReason| Outcome {
            seq: qa.seq,
            pixel_id: qa.pixel_id,
            accepted: false,
            x: p.map(|p| p.x).unwrap_or(0),
            y: p.map(|p| p.y).unwrap_or(0),
            reason: Some(reason),
            data: None,
        };

        let Some(p) = self.pixels.get(&qa.pixel_id).copied() else {
            return reject(None, RejectReason::NotFound);
        };
        let Some(module) = registry().get(&qa.action.kind) else {
            return reject(Some(&p), RejectReason::UnknownAction);
        };
        if !cfg.action_enabled(module.name()) {
            return reject(Some(&p), RejectReason::DisabledAction);
        }
        if module.validate(&qa.action.params).is_err() {
            return reject(Some(&p), RejectReason::InvalidParams);
        }

        let mut ctx = ActionContext::new(self, cfg, tick, qa.seq, module.name(), p);
        let result = module.resolve(&mut ctx, &p, &qa.action.params);
        let (events, departure, after, data) = ctx.finish();

        match result {
            Ok(()) => {
                for ev in &events {
                    self.apply(ev);
                }
                out.events.extend(events);
                if let Some(d) = departure {
                    out.departures.push(d);
                }
                Outcome {
                    seq: qa.seq,
                    pixel_id: p.id,
                    accepted: true,
                    x: after.x,
                    y: after.y,
                    reason: None,
                    data,
                }
            }
            Err(reason) => reject(Some(&p), reason),
        }
    }

    /// Nearest free cell to (x, y) inside this chunk and the map, by expanding rings.
    fn nearest_free(&self, cfg: &WorldConfig, x: u32, y: u32) -> Option<(u32, u32)> {
        let x0 = self.coord.cx * CHUNK_SIZE;
        let y0 = self.coord.cy * CHUNK_SIZE;
        for r in 1..CHUNK_SIZE as i64 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let nx = x as i64 + dx;
                    let ny = y as i64 + dy;
                    if nx < x0 as i64
                        || ny < y0 as i64
                        || nx >= (x0 + CHUNK_SIZE) as i64
                        || ny >= (y0 + CHUNK_SIZE) as i64
                    {
                        continue;
                    }
                    if !cfg.contains_cell(nx, ny) {
                        continue;
                    }
                    let c = (nx as u32, ny as u32);
                    if !self.occupancy.contains_key(&c) {
                        return Some(c);
                    }
                }
            }
        }
        None
    }
}

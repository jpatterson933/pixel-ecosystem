use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    Action, Arrival, ChunkCoord, Departure, Event, Outcome, Pixel, PixelId, QueuedAction,
    RejectReason, Tick, WorldConfig, CHUNK_SIZE,
};

/// One chunk's authoritative state. Sparse: only pixels are stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub coord: ChunkCoord,
    pub pixels: HashMap<PixelId, Pixel>,
    /// Cell (x, y) in map coordinates to the pixel occupying it.
    pub occupancy: HashMap<(u32, u32), PixelId>,
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
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    pub fn contains_cell(&self, x: u32, y: u32) -> bool {
        ChunkCoord::of_cell(x, y) == self.coord
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

    /// Apply a single event to this chunk's state. Used for replay from the log.
    pub fn apply(&mut self, ev: &Event) {
        match ev {
            Event::Config { .. } => {}
            Event::Placed { id, x, y, color } | Event::Arrived { id, x, y, color } => {
                self.insert(Pixel {
                    id: *id,
                    x: *x,
                    y: *y,
                    color: *color,
                });
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
            self.insert(p);
            out.events.push(Event::Arrived {
                id: p.id,
                x: p.x,
                y: p.y,
                color: p.color,
            });
        }

        // Actions.
        input.actions.sort_by_key(|a| (a.seq, a.pixel_id));
        for qa in input.actions {
            let outcome = self.resolve(cfg, &qa, &mut out);
            out.outcomes.push(outcome);
        }

        out
    }

    fn resolve(&mut self, cfg: &WorldConfig, qa: &QueuedAction, out: &mut TickOutput) -> Outcome {
        let reject = |p: Option<&Pixel>, reason: RejectReason| Outcome {
            seq: qa.seq,
            pixel_id: qa.pixel_id,
            accepted: false,
            x: p.map(|p| p.x).unwrap_or(0),
            y: p.map(|p| p.y).unwrap_or(0),
            reason: Some(reason),
        };

        let Some(p) = self.pixels.get(&qa.pixel_id).copied() else {
            return reject(None, RejectReason::NotFound);
        };
        if !qa.action.is_valid() {
            return reject(Some(&p), RejectReason::InvalidAction);
        }

        match qa.action {
            Action::Move { dx, dy } => {
                let nx = p.x as i64 + dx as i64;
                let ny = p.y as i64 + dy as i64;
                if !cfg.contains_cell(nx, ny) {
                    return reject(Some(&p), RejectReason::OffMap);
                }
                let (nx, ny) = (nx as u32, ny as u32);
                let dest = ChunkCoord::of_cell(nx, ny);

                if dest == self.coord {
                    if self.occupancy.contains_key(&(nx, ny)) {
                        return reject(Some(&p), RejectReason::Occupied);
                    }
                    self.remove(p.id);
                    let moved = Pixel {
                        x: nx,
                        y: ny,
                        ..p
                    };
                    self.insert(moved);
                    out.events.push(Event::Moved {
                        id: p.id,
                        from: [p.x, p.y],
                        to: [nx, ny],
                    });
                    Outcome {
                        seq: qa.seq,
                        pixel_id: p.id,
                        accepted: true,
                        x: nx,
                        y: ny,
                        reason: None,
                    }
                } else {
                    // Cross-chunk: depart now, arrive next tick in the destination.
                    self.remove(p.id);
                    let moved = Pixel {
                        x: nx,
                        y: ny,
                        ..p
                    };
                    out.events.push(Event::Departed {
                        id: p.id,
                        from: [p.x, p.y],
                        to: [nx, ny],
                    });
                    out.departures.push(Departure {
                        pixel: moved,
                        to: dest,
                    });
                    Outcome {
                        seq: qa.seq,
                        pixel_id: p.id,
                        accepted: true,
                        x: nx,
                        y: ny,
                        reason: None,
                    }
                }
            }
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

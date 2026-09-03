//! build: construct a structure from a blueprint, one cell per tick, around the acting pixel.
//!
//! The builder stands at the blueprint's center and stays there. Each `build` call
//! places the next cell in the builder's color. While a structure is unfinished, any
//! other pixel inside its footprint clears everything placed so far and the build
//! starts over. A finished structure is permanent and impassable: its walls are solid
//! and its interior is sealed. A new structure may not overlap another structure's
//! footprint or be placed over existing terrain.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ActionContext, Module};
use crate::{Pixel, RejectReason};

pub struct Build;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Params {
    /// Blueprint name. See `blueprints()`.
    #[serde(default = "default_blueprint")]
    pub blueprint: String,
}

fn default_blueprint() -> String {
    "boundary_wall".into()
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Settings {
    /// Suggested probability per pixel per tick that a driver starts a build. Read by drivers, not enforced.
    pub build_chance: f64,
    /// Blueprints pixels may build. Empty means all.
    pub blueprints: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            build_chance: 0.0005,
            blueprints: Vec::new(),
        }
    }
}

/// A structure as cells relative to the builder, who stands at (0, 0).
pub struct Blueprint {
    pub name: &'static str,
    /// Cells placed, in build order.
    pub cells: Vec<(i32, i32)>,
    /// Every cell the structure claims, including interior. Intruders anywhere here destroy it.
    pub footprint: Vec<(i32, i32)>,
}

pub fn blueprints() -> Vec<Blueprint> {
    vec![boundary_wall()]
}

pub fn blueprint(name: &str) -> Option<Blueprint> {
    blueprints().into_iter().find(|b| b.name == name)
}

/// 5 by 5 ring of 16 cells with the builder in the center.
fn boundary_wall() -> Blueprint {
    let mut cells = Vec::with_capacity(16);
    let mut footprint = Vec::with_capacity(25);
    for dy in -2i32..=2 {
        for dx in -2i32..=2 {
            footprint.push((dx, dy));
            if dx.abs() == 2 || dy.abs() == 2 {
                cells.push((dx, dy));
            }
        }
    }
    Blueprint {
        name: "boundary_wall",
        cells,
        footprint,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    blueprint: String,
    origin: [u32; 2],
    progress: usize,
    done: bool,
}

impl Module for Build {
    const NAME: &'static str = "build";
    const DESCRIPTION: &'static str = "Build a structure from a blueprint around the pixel, one cell per call, in the pixel's color. While unfinished, any other pixel inside the footprint destroys the work and it restarts. Finished structures are permanent and cannot be entered. A new structure cannot overlap another structure's footprint or existing terrain.";
    type Params = Params;
    type Settings = Settings;

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, p: Params) -> Result<(), RejectReason> {
        let settings: Settings = ctx.settings();
        let Some(bp) = blueprint(&p.blueprint) else {
            return Err(RejectReason::InvalidParams);
        };
        if !settings.blueprints.is_empty() && !settings.blueprints.iter().any(|b| b == bp.name) {
            return Err(RejectReason::Refused);
        }

        let origin = [pixel.x, pixel.y];
        let (ox, oy) = (pixel.x as i64, pixel.y as i64);

        // The whole footprint must lie in this chunk and on the map.
        if bp.footprint.iter().any(|(dx, dy)| !ctx.cell_in_chunk(ox + *dx as i64, oy + *dy as i64)) {
            return Err(RejectReason::Refused);
        }

        let previous: Option<State> = ctx
            .attribute(pixel.id, "state")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let fresh = !matches!(&previous, Some(s) if s.blueprint == bp.name && s.origin == origin);
        let mut st = match previous {
            Some(s) if s.blueprint == bp.name && s.origin == origin => s,
            Some(old) => {
                // Builder moved or changed blueprint: abandon the old work.
                if let Some(old_bp) = blueprint(&old.blueprint) {
                    let (ax, ay) = (old.origin[0] as i64, old.origin[1] as i64);
                    for (dx, dy) in old_bp.cells.iter().take(old.progress) {
                        let _ = ctx.set_cell(ax + *dx as i64, ay + *dy as i64, None);
                    }
                }
                State {
                    blueprint: bp.name.to_string(),
                    origin,
                    progress: 0,
                    done: false,
                }
            }
            None => State {
                blueprint: bp.name.to_string(),
                origin,
                progress: 0,
                done: false,
            },
        };

        let total = bp.cells.len();

        if fresh {
            // Structures may not overlap: no footprint cell may lie in another builder's
            // footprint, and no wall cell may land on existing terrain.
            let mine: Vec<(i64, i64)> = bp.footprint.iter().map(|(dx, dy)| (ox + *dx as i64, oy + *dy as i64)).collect();
            for (other_id, state) in ctx.attributes_in_chunk("state") {
                if other_id == pixel.id {
                    continue;
                }
                let Ok(other) = serde_json::from_value::<State>(state.clone()) else { continue };
                let Some(other_bp) = blueprint(&other.blueprint) else { continue };
                let (bx, by) = (other.origin[0] as i64, other.origin[1] as i64);
                let overlaps = other_bp
                    .footprint
                    .iter()
                    .any(|(dx, dy)| mine.contains(&(bx + *dx as i64, by + *dy as i64)));
                if overlaps {
                    return Err(RejectReason::Refused);
                }
            }
            if bp.footprint.iter().any(|(dx, dy)| ctx.has_terrain(ox + *dx as i64, oy + *dy as i64)) {
                return Err(RejectReason::Refused);
            }
        }

        if st.done {
            ctx.reply(json!({
                "status": "done",
                "blueprint": bp.name,
                "progress": total,
                "total": total
            }));
            return Ok(());
        }

        // Intrusion only matters while unfinished: any other pixel anywhere in the footprint.
        let intruded = bp
            .footprint
            .iter()
            .filter(|(dx, dy)| !(*dx == 0 && *dy == 0))
            .any(|(dx, dy)| ctx.occupant(ox + *dx as i64, oy + *dy as i64).is_some());

        if intruded {
            for (dx, dy) in bp.cells.iter().take(st.progress) {
                ctx.set_cell(ox + *dx as i64, oy + *dy as i64, None)?;
            }
            let had = st.progress;
            st.progress = 0;
            st.done = false;
            ctx.set_attribute(pixel.id, "state", serde_json::to_value(&st).unwrap_or(Value::Null))?;
            ctx.reply(json!({
                "status": "destroyed",
                "blueprint": bp.name,
                "cleared": had,
                "progress": 0,
                "total": total
            }));
            return Ok(());
        }

        let (dx, dy) = bp.cells[st.progress];
        ctx.set_cell_solid(ox + dx as i64, oy + dy as i64, Some(pixel.color), true)?;
        st.progress += 1;
        st.done = st.progress >= total;
        if st.done {
            // Seal the interior so nothing can be displaced into it.
            for (dx, dy) in bp.footprint.iter().filter(|c| !bp.cells.contains(c)) {
                ctx.set_cell_solid(ox + *dx as i64, oy + *dy as i64, None, true)?;
            }
        }
        ctx.set_attribute(pixel.id, "state", serde_json::to_value(&st).unwrap_or(Value::Null))?;
        ctx.reply(json!({
            "status": if st.done { "done" } else { "building" },
            "blueprint": bp.name,
            "progress": st.progress,
            "total": total
        }));
        Ok(())
    }
}

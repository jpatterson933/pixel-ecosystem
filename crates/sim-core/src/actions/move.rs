//! move: step one cell in any of the eight directions.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{ActionContext, Module};
use crate::{Pixel, RejectReason};

pub struct Move;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Params {
    /// -1, 0, or 1
    pub dx: i8,
    /// -1, 0, or 1
    pub dy: i8,
}

impl Module for Move {
    const NAME: &'static str = "move";
    const DESCRIPTION: &'static str =
        "Move one cell. dx and dy are each -1, 0, or 1 and not both zero. Fails if the target cell is occupied or off the map.";
    type Params = Params;

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, p: Params) -> Result<(), RejectReason> {
        if !(-1..=1).contains(&p.dx) || !(-1..=1).contains(&p.dy) || (p.dx == 0 && p.dy == 0) {
            return Err(RejectReason::InvalidParams);
        }
        ctx.move_to(pixel.x as i64 + p.dx as i64, pixel.y as i64 + p.dy as i64)
    }
}

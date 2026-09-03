//! mark: paint the cell under the acting pixel with the pixel's own color.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{ActionContext, Module};
use crate::{Pixel, RejectReason};

pub struct Mark;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct Params {}

impl Module for Mark {
    const NAME: &'static str = "mark";
    const DESCRIPTION: &'static str =
        "Set the terrain of the cell the pixel is standing on to the pixel's own color. Moving on and marking again leaves a trail.";
    type Params = Params;

    fn resolve(&self, ctx: &mut ActionContext, pixel: &Pixel, _p: Params) -> Result<(), RejectReason> {
        ctx.set_cell(pixel.x as i64, pixel.y as i64, Some(pixel.color))
    }
}

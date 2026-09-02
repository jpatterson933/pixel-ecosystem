//! World generation. Fully determined by the config.

use std::collections::HashSet;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::{Pixel, Rgba, WorldConfig};

pub struct Generated {
    pub palette: Vec<Rgba>,
    pub pixels: Vec<Pixel>,
}

/// Generate palette and genesis placement. Deterministic in the seed.
pub fn generate(cfg: &WorldConfig) -> Generated {
    let palette = generate_palette(cfg);
    let pixels = generate_pixels(cfg, &palette);
    Generated { palette, pixels }
}

pub fn generate_palette(cfg: &WorldConfig) -> Vec<Rgba> {
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed ^ 0x636f_6c6f_7273); // "colors"
    let mut seen: HashSet<u32> = HashSet::with_capacity(cfg.color_count as usize);
    let mut palette = Vec::with_capacity(cfg.color_count as usize);
    while palette.len() < cfg.color_count as usize {
        let v: u32 = rng.gen_range(0..(1u32 << 24));
        if seen.insert(v) {
            palette.push(Rgba::from_rgb24(v));
        }
    }
    palette
}

pub fn generate_pixels(cfg: &WorldConfig, palette: &[Rgba]) -> Vec<Pixel> {
    let mut rng = ChaCha8Rng::seed_from_u64(cfg.seed ^ 0x706c_6163_6500); // "place"
    let cells = cfg.map_width as u64 * cfg.map_height as u64;
    let mut occupied: HashSet<u64> = HashSet::with_capacity(cfg.pixel_count as usize);
    let mut pixels = Vec::with_capacity(cfg.pixel_count as usize);

    if cfg.pixel_count * 2 > cells {
        // Dense: shuffle-select to avoid rejection sampling stalls.
        let mut all: Vec<u64> = (0..cells).collect();
        for i in 0..cfg.pixel_count as usize {
            let j = rng.gen_range(i..all.len());
            all.swap(i, j);
        }
        for (i, cell) in all.iter().take(cfg.pixel_count as usize).enumerate() {
            pixels.push(pixel_at(cfg, palette, &mut rng, i as u64 + 1, *cell));
        }
        return pixels;
    }

    let mut id: u64 = 1;
    while pixels.len() < cfg.pixel_count as usize {
        let cell = rng.gen_range(0..cells);
        if occupied.insert(cell) {
            pixels.push(pixel_at(cfg, palette, &mut rng, id, cell));
            id += 1;
        }
    }
    pixels
}

fn pixel_at(cfg: &WorldConfig, palette: &[Rgba], rng: &mut ChaCha8Rng, id: u64, cell: u64) -> Pixel {
    let x = (cell % cfg.map_width as u64) as u32;
    let y = (cell / cfg.map_width as u64) as u32;
    let color = palette[rng.gen_range(0..palette.len())];
    Pixel { id, x, y, color }
}

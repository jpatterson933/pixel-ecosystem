//! Hilbert curve over the chunk grid. Adjacent indices are spatially adjacent,
//! so contiguous index ranges make good ownership ranges.

use crate::ChunkCoord;

/// Map (x, y) in an n x n grid (n a power of two) to its Hilbert index.
pub fn xy2d(n: u32, x: u32, y: u32) -> u64 {
    let mut d: u64 = 0;
    let mut x = x as u64;
    let mut y = y as u64;
    let mut s: u64 = (n as u64) / 2;
    while s > 0 {
        let rx = ((x & s) > 0) as u64;
        let ry = ((y & s) > 0) as u64;
        d += s * s * ((3 * rx) ^ ry);
        rot(n as u64, &mut x, &mut y, rx, ry);
        s /= 2;
    }
    d
}

/// Inverse of `xy2d`.
pub fn d2xy(n: u32, d: u64) -> (u32, u32) {
    let mut t = d;
    let mut x: u64 = 0;
    let mut y: u64 = 0;
    let mut s: u64 = 1;
    while s < n as u64 {
        let rx = 1 & (t / 2);
        let ry = 1 & (t ^ rx);
        rot(s, &mut x, &mut y, rx, ry);
        x += s * rx;
        y += s * ry;
        t /= 4;
        s *= 2;
    }
    (x as u32, y as u32)
}

fn rot(n: u64, x: &mut u64, y: &mut u64, rx: u64, ry: u64) {
    if ry == 0 {
        if rx == 1 {
            *x = n - 1 - *x;
            *y = n - 1 - *y;
        }
        std::mem::swap(x, y);
    }
}

pub fn chunk_index(order: u32, c: ChunkCoord) -> u64 {
    xy2d(order, c.cx, c.cy)
}

pub fn chunk_at(order: u32, index: u64) -> ChunkCoord {
    let (cx, cy) = d2xy(order, index);
    ChunkCoord { cx, cy }
}

/// Total index space for an order.
pub fn index_space(order: u32) -> u64 {
    (order as u64) * (order as u64)
}

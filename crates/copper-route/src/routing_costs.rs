//! Grid-independent routing costs shared by global and detailed search
//! (`pcbir.routing_costs`).
//!
//! One fixed-event cost unit is one million integer micro-units. Planar costs
//! are per millimetre (10 baseline units/mm), kept at nanometre precision.

use copper_physical::geometry::Bounds;
use copper_physical::{CopperLayer, Point};

pub const COST_UNIT: i64 = 1_000_000;

/// `LayerBias`: an escape plan's soft layer assignment, a penalty on `layer` inside `area`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerBias {
    pub net: String,
    pub layer: CopperLayer,
    pub area: Bounds,
    pub rate: i64,
}

impl LayerBias {
    /// The penalty of a track from `first` to `second` whose midpoint is in `area`.
    pub fn cost(&self, first: Point, second: Point) -> i64 {
        let x = (first.x_nm + second.x_nm).div_euclid(2);
        let y = (first.y_nm + second.y_nm).div_euclid(2);
        if !(self.area.min_x <= x && x <= self.area.max_x && self.area.min_y <= y && y <= self.area.max_y) {
            return 0;
        }
        length_cost(first, second, self.rate)
    }
}

/// `isqrt` of a non-negative integer.
pub fn isqrt(value: i128) -> i128 {
    if value < 2 {
        return value;
    }
    let mut x = (value as f64).sqrt() as i128;
    while x * x > value {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= value {
        x += 1;
    }
    x
}

pub fn length_cost(first: Point, second: Point, rate: i64) -> i64 {
    let dx = i128::from(second.x_nm - first.x_nm);
    let dy = i128::from(second.y_nm - first.y_nm);
    (i128::from(rate) * isqrt(dx * dx + dy * dy)) as i64
}

pub fn preference_cost(
    first: Point,
    second: Point,
    rank: i64,
    layer_cost: i64,
    wrong_way_cost: i64,
    diagonal: bool,
) -> i64 {
    // Half-rate diagonal heading penalties retain integer-nanometre precision.
    let rate_twice = 2 * rank * layer_cost + if diagonal { wrong_way_cost } else { 2 * wrong_way_cost };
    length_cost(first, second, rate_twice).div_euclid(2)
}

/// Soft run cost of already-committed local demand, before any overflow.
pub fn demand_cost(first: Point, second: Point, rate: i64, usage: i64, capacity: i64) -> i64 {
    if rate <= 0 || usage <= 0 || capacity <= 0 {
        return 0;
    }
    (i128::from(length_cost(first, second, rate)) * i128::from(usage.min(capacity))).div_euclid(i128::from(capacity))
        as i64
}

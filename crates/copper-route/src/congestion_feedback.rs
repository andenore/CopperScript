//! Tile congestion of a detailed routing attempt (`pcbir.congestion_feedback`:
//! the `CongestionMap` the unified router exports). The RUDY estimate and the
//! placement feedback of that module are not here.

use copper_physical::Point;

/// `CongestionMap`: per-tile demand against capacity, rows indexed `[iy][ix]`.
#[derive(Clone, Debug, PartialEq)]
pub struct CongestionMap {
    pub tile_nm: i64,
    pub x0_nm: i64,
    pub y0_nm: i64,
    pub nx: usize,
    pub ny: usize,
    pub demand: Vec<Vec<f64>>,
    pub capacity: Vec<Vec<f64>>,
    pub source: String,
    pub failed_points: Vec<(String, Point)>,
}

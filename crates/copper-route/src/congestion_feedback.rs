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

// ---------------------------------------------------------------------------
// Map queries, the RUDY estimate and the placement feedback loop
//
// numpy reductions are reproduced exactly: `sum` is numpy's pairwise sum,
// `percentile` its linear method. The gradient's least-squares plane fit is
// solved exactly and rounded once, where numpy's LAPACK solve may differ in
// the last bits; the fitted direction only feeds grid-snapped moves.

use std::cmp::Ordering;

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive, Zero};

use copper_json::Value;
use copper_pcb::board::{PhysicalBoard, Placement};
use copper_physical::footprint::PadKind;
use copper_physical::pyfloat::{hypot, round_digits, round_int};
use copper_physical::{BoardSide, CopperLayer};
use copper_place::PlannerOptions;
use copper_units::Decimal;

use crate::progress::{ProgressCallback, emit, int, text};
use crate::routing::{GlobalRouterOptions, GlobalRoutingResult, GlobalRoutingStatus, route_global};
use crate::unified_router::geometry::{polygon_inside, spine_distance};
use crate::{Error, Result};

/// `UTILISATION_SENTINEL`: a tile with demand but no capacity.
pub const UTILISATION_SENTINEL: f64 = 1.0e6;
/// `DEFAULT_TILE_NM`.
pub const DEFAULT_TILE_NM: i64 = 1_000_000;
const SUBSAMPLES: usize = 4;
/// `NORMALISATION_PERCENTILE`.
pub const NORMALISATION_PERCENTILE: f64 = 95.0;
/// `MINIMUM_STEP_NM`.
pub const MINIMUM_STEP_NM: i64 = 250_000;

/// numpy's pairwise summation (`np.add.reduce` of a contiguous float64 run).
pub fn pairwise_sum(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 8 {
        let mut result = 0.0;
        for value in values {
            result += value;
        }
        return result;
    }
    if n <= 128 {
        let mut r = [0.0; 8];
        r.copy_from_slice(&values[..8]);
        let mut i = 8;
        while i < n - (n % 8) {
            for j in 0..8 {
                r[j] += values[i + j];
            }
            i += 8;
        }
        let mut result = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            result += values[i];
            i += 1;
        }
        return result;
    }
    let mut half = n / 2;
    half -= half % 8;
    pairwise_sum(&values[..half]) + pairwise_sum(&values[half..])
}

/// Python's `float // float` (`float_floor_div`).
fn py_floordiv(a: f64, b: f64) -> f64 {
    let modulo = a % b;
    let mut div = (a - modulo) / b;
    if modulo != 0.0 && ((b < 0.0) != (modulo < 0.0)) {
        div -= 1.0;
    }
    if div == 0.0 {
        return 0.0f64.copysign(a / b);
    }
    let floor = div.floor();
    if div - floor > 0.5 { floor + 1.0 } else { floor }
}

/// `np.percentile(values, q)` with the linear method.
fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = sorted.len();
    let quantile = q / 100.0;
    let virtual_index = (n as f64 - 1.0) * quantile;
    if virtual_index >= (n - 1) as f64 {
        return sorted[n - 1];
    }
    let previous = virtual_index.floor() as usize;
    let next = previous + 1;
    let gamma = virtual_index - virtual_index.floor();
    let (a, b) = (sorted[previous], sorted[next]);
    let difference = b - a;
    if gamma >= 0.5 { b - difference * (1.0 - gamma) } else { a + difference * gamma }
}

impl CongestionMap {
    fn lattice(&self) -> (i64, i64, i64, usize, usize) {
        (self.tile_nm, self.x0_nm, self.y0_nm, self.nx, self.ny)
    }

    /// `utilisation()`.
    pub fn utilisation(&self) -> Vec<Vec<f64>> {
        (0..self.ny)
            .map(|iy| {
                (0..self.nx)
                    .map(|ix| {
                        let (demand, capacity) = (self.demand[iy][ix], self.capacity[iy][ix]);
                        if capacity > 0.0 {
                            demand / capacity
                        } else if demand > 0.0 {
                            UTILISATION_SENTINEL
                        } else {
                            0.0
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// `tile_of(point)`.
    pub fn tile_of(&self, point: copper_physical::Point) -> (usize, usize) {
        let ix = (point.x_nm - self.x0_nm).div_euclid(self.tile_nm);
        let iy = (point.y_nm - self.y0_nm).div_euclid(self.tile_nm);
        (ix.clamp(0, self.nx as i64 - 1) as usize, iy.clamp(0, self.ny as i64 - 1) as usize)
    }

    /// `tile_bounds(ix, iy)`.
    pub fn tile_bounds(&self, ix: usize, iy: usize) -> (i64, i64, i64, i64) {
        let x = self.x0_nm + ix as i64 * self.tile_nm;
        let y = self.y0_nm + iy as i64 * self.tile_nm;
        (x, y, x + self.tile_nm, y + self.tile_nm)
    }

    /// `gradient_at(point, radius_tiles=2)`: the unit direction of decreasing
    /// utilisation of a least-squares plane fit, or `(0, 0)`.
    pub fn gradient_at(&self, point: copper_physical::Point) -> (f64, f64) {
        let radius = 2usize;
        let (cx, cy) = self.tile_of(point);
        let (x0, x1) = (cx.saturating_sub(radius), self.nx.min(cx + radius + 1));
        let (y0, y1) = (cy.saturating_sub(radius), self.ny.min(cy + radius + 1));
        let mut samples: Vec<(i64, i64, f64)> = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let capacity = self.capacity[y][x];
                if capacity > 0.0 {
                    samples.push((x as i64 - cx as i64, y as i64 - cy as i64, self.demand[y][x] / capacity));
                }
            }
        }
        if samples.len() < 3 {
            return (0.0, 0.0);
        }
        // Normal equations of [1, dx, dy], exactly.
        let r = |v: i64| BigRational::from_integer(BigInt::from(v));
        let mut a = [
            [BigRational::zero(), BigRational::zero(), BigRational::zero()],
            [BigRational::zero(), BigRational::zero(), BigRational::zero()],
            [BigRational::zero(), BigRational::zero(), BigRational::zero()],
        ];
        let mut b = [BigRational::zero(), BigRational::zero(), BigRational::zero()];
        for (dx, dy, u) in &samples {
            let row = [r(1), r(*dx), r(*dy)];
            let value = BigRational::from_float(*u).unwrap_or_else(BigRational::zero);
            for i in 0..3 {
                for j in 0..3 {
                    a[i][j] += &row[i] * &row[j];
                }
                b[i] += &row[i] * &value;
            }
        }
        let det = |m: &[[BigRational; 3]; 3]| {
            &m[0][0] * (&m[1][1] * &m[2][2] - &m[1][2] * &m[2][1])
                - &m[0][1] * (&m[1][0] * &m[2][2] - &m[1][2] * &m[2][0])
                + &m[0][2] * (&m[1][0] * &m[2][1] - &m[1][1] * &m[2][0])
        };
        let denominator = det(&a);
        if denominator.is_zero() {
            return (0.0, 0.0);
        }
        let solve = |column: usize| {
            let mut m = a.clone();
            for row in 0..3 {
                m[row][column] = b[row].clone();
            }
            (det(&m) / &denominator).to_f64().unwrap_or(0.0)
        };
        let (slope_x, slope_y) = (solve(1), solve(2));
        let norm = hypot(slope_x, slope_y);
        let scale = samples.iter().fold(1.0f64, |acc, (_, _, u)| acc.max(u.abs()));
        if norm <= 1e-9 * scale {
            return (0.0, 0.0);
        }
        (-slope_x / norm, -slope_y / norm)
    }

    /// `hot_tiles(limit)`: positive utilisation, hottest first, ties by `(iy, ix)`.
    pub fn hot_tiles(&self, limit: usize) -> Vec<(usize, usize, f64)> {
        if limit == 0 {
            return Vec::new();
        }
        let values = self.utilisation();
        let mut hot = Vec::new();
        for (iy, row) in values.iter().enumerate() {
            for (ix, value) in row.iter().enumerate() {
                if *value > 0.0 {
                    hot.push((ix, iy, *value));
                }
            }
        }
        hot.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));
        hot.truncate(limit);
        hot
    }

    fn flat(rows: &[Vec<f64>]) -> Vec<f64> {
        rows.iter().flatten().copied().collect()
    }
}

/// `_lattice(board, tile_nm)`.
fn board_lattice(board: &PhysicalBoard, tile_nm: i64) -> (i64, i64, usize, usize) {
    let vertices = &board.outline.vertices;
    let x0 = vertices.iter().map(|p| p.x_nm).min().expect("outline");
    let y0 = vertices.iter().map(|p| p.y_nm).min().expect("outline");
    let x1 = vertices.iter().map(|p| p.x_nm).max().expect("outline");
    let y1 = vertices.iter().map(|p| p.y_nm).max().expect("outline");
    let count =
        |span: i64| (crate::routing_lattice::true_div(i128::from(span), i128::from(tile_nm)).ceil() as i64).max(1);
    (x0, y0, count(x1 - x0) as usize, count(y1 - y0) as usize)
}

/// `_pitch_estimates`: per footprint, the nearest pad-centre spacing.
fn pitch_estimates(board: &PhysicalBoard) -> IndexMap<String, f64> {
    let mut result = IndexMap::new();
    for (name, footprint) in &board.footprints {
        let pads: Vec<_> = footprint.pads.iter().filter(|pad| pad.kind != PadKind::Aperture).collect();
        if pads.len() >= 2 {
            let mut nearest = f64::INFINITY;
            for a in &pads {
                for b in &pads {
                    let distance = (a.position.x_nm as f64 - b.position.x_nm as f64)
                        .hypot(a.position.y_nm as f64 - b.position.y_nm as f64);
                    if distance != 0.0 && distance < nearest {
                        nearest = distance;
                    }
                }
            }
            if nearest.is_finite() {
                result.insert(name.clone(), nearest);
                continue;
            }
        }
        let largest = pads.iter().map(|pad| pad.size.width_nm.max(pad.size.height_nm)).max().unwrap_or(0);
        result.insert(name.clone(), largest as f64);
    }
    result
}

/// `estimate_demand(board, tile_nm, lattice=)`: RUDY wire density plus pin
/// density against free area per signal layer.
#[allow(clippy::needless_range_loop)] // numpy index arithmetic, row-major.
pub fn estimate_demand(
    board: &PhysicalBoard,
    tile_nm: i64,
    lattice: Option<(i64, i64, usize, usize)>,
) -> Result<CongestionMap> {
    if tile_nm <= 0 {
        return Err(Error::Value("estimate tile must be positive".to_owned()));
    }
    let (x0, y0, nx, ny) = lattice.unwrap_or_else(|| board_lattice(board, tile_nm));
    if nx == 0 || ny == 0 {
        return Err(Error::Value("estimate lattice requires positive tile counts".to_owned()));
    }
    let lane_nm = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm;
    let lane_area = (lane_nm as f64) * (lane_nm as f64);
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let net_lane = |name: &str| -> f64 {
        match rules.get(name) {
            Some(rule) if rule.width_nm.is_some() => {
                let clearance = rule.clearance_nm.filter(|c| *c != 0).unwrap_or(board.rules.minimum_clearance_nm);
                (rule.width_nm.expect("width") + clearance) as f64
            }
            _ => lane_nm as f64,
        }
    };
    let sub = tile_nm as f64 / SUBSAMPLES as f64;
    let (width, height) = (nx * SUBSAMPLES, ny * SUBSAMPLES);
    let sxs: Vec<f64> = (0..width).map(|i| x0 as f64 + sub * (i as f64 + 0.5)).collect();
    let sys: Vec<f64> = (0..height).map(|j| y0 as f64 + sub * (j as f64 + 0.5)).collect();
    let mut material = vec![vec![false; width]; height];
    for (j, py) in sys.iter().enumerate() {
        for (i, px) in sxs.iter().enumerate() {
            let mut inside = polygon_inside(&board.outline.vertices, *px, *py);
            for cutout in &board.outline.cutouts {
                inside &= !polygon_inside(&cutout.vertices, *px, *py);
            }
            material[j][i] = inside;
        }
    }
    let layers = copper_pcb::zones::routing_layers(board, "", None);
    let mut free: IndexMap<CopperLayer, Vec<Vec<bool>>> = layers.iter().map(|l| (*l, material.clone())).collect();
    let window = |xmin: f64, ymin: f64, xmax: f64, ymax: f64| -> Option<(usize, usize, usize, usize)> {
        let i0 = (py_floordiv(xmin - x0 as f64, sub) as i64).max(0);
        let i1 = (width as i64).min(py_floordiv(xmax - x0 as f64, sub) as i64 + 1);
        let j0 = (py_floordiv(ymin - y0 as f64, sub) as i64).max(0);
        let j1 = (height as i64).min(py_floordiv(ymax - y0 as f64, sub) as i64 + 1);
        (i0 < i1 && j0 < j1).then_some((j0 as usize, j1 as usize, i0 as usize, i1 as usize))
    };
    let mut net_of_pad: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            net_of_pad.insert((pad.component.as_str(), pad.pad.as_str()), net.name.as_str());
        }
    }
    let pitches = pitch_estimates(board);
    let zone_nets = copper_pcb::zones::deferred_zone_nets(board)?;
    let mut demand = vec![vec![0.0f64; nx]; ny];
    let mut pad_points: IndexMap<(String, String), Vec<copper_physical::Point>> = IndexMap::new();
    for placement in &board.placements {
        let footprint = &board.footprints[&placement.footprint];
        let side_layer = if placement.side == BoardSide::Back { CopperLayer::Back } else { CopperLayer::Front };
        for pad in &footprint.pads {
            if pad.kind == PadKind::Aperture {
                continue;
            }
            let position = copper_pcb::transform::transformed_local_point(placement, pad.position);
            pad_points.entry((placement.reference.clone(), pad.number.clone())).or_default().push(position);
            let shape = copper_pcb::transform::placed_pad_shape(position, pad, placement);
            let on: Vec<CopperLayer> = if pad.kind == PadKind::Smd { vec![side_layer] } else { layers.clone() };
            let reach = shape.radius_nm;
            let spine = &shape.spine;
            let bounds = window(
                (spine.iter().map(|p| p.x_nm).min().expect("spine") - reach) as f64,
                (spine.iter().map(|p| p.y_nm).min().expect("spine") - reach) as f64,
                (spine.iter().map(|p| p.x_nm).max().expect("spine") + reach) as f64,
                (spine.iter().map(|p| p.y_nm).max().expect("spine") + reach) as f64,
            );
            if let Some((j0, j1, i0, i1)) = bounds {
                for j in j0..j1 {
                    for i in i0..i1 {
                        if spine_distance(spine, sxs[i], sys[j]) <= reach as f64 {
                            for layer in &on {
                                if let Some(mask) = free.get_mut(layer) {
                                    mask[j][i] = false;
                                }
                            }
                        }
                    }
                }
            }
            let Some(net) = net_of_pad.get(&(placement.reference.as_str(), pad.number.as_str())) else { continue };
            let estimate = pitches.get(&placement.footprint).copied().unwrap_or(0.0);
            let pitch = (if estimate != 0.0 { estimate } else { tile_nm as f64 }).min(tile_nm as f64);
            let ix = (position.x_nm - x0).div_euclid(tile_nm).clamp(0, nx as i64 - 1) as usize;
            let iy = (position.y_nm - y0).div_euclid(tile_nm).clamp(0, ny as i64 - 1) as usize;
            demand[iy][ix] += net_lane(net) * pitch / lane_area;
        }
    }
    for keepout in copper_pcb::keepouts::resolved_copper_keepouts(board)? {
        if !keepout.block_tracks || !keepout.layers.iter().any(|l| free.contains_key(l)) {
            continue;
        }
        let outer = &keepout.vertices;
        let Some((j0, j1, i0, i1)) = window(
            outer.iter().map(|p| p.x_nm).min().expect("keepout") as f64,
            outer.iter().map(|p| p.y_nm).min().expect("keepout") as f64,
            outer.iter().map(|p| p.x_nm).max().expect("keepout") as f64,
            outer.iter().map(|p| p.y_nm).max().expect("keepout") as f64,
        ) else {
            continue;
        };
        for j in j0..j1 {
            for i in i0..i1 {
                if polygon_inside(outer, sxs[i], sys[j]) {
                    for layer in &keepout.layers {
                        if let Some(mask) = free.get_mut(layer) {
                            mask[j][i] = false;
                        }
                    }
                }
            }
        }
    }
    let sample_area = sub * sub;
    let mut capacity = vec![vec![0.0f64; nx]; ny];
    let tile_count = |mask: &Vec<Vec<bool>>, ix: usize, iy: usize| {
        let mut count = 0i64;
        for j in iy * SUBSAMPLES..(iy + 1) * SUBSAMPLES {
            for i in ix * SUBSAMPLES..(ix + 1) * SUBSAMPLES {
                count += mask[j][i] as i64;
            }
        }
        count
    };
    for mask in free.values() {
        for iy in 0..ny {
            for ix in 0..nx {
                capacity[iy][ix] += tile_count(mask, ix, iy) as f64 * sample_area / lane_area;
            }
        }
    }
    let has_material: Vec<Vec<bool>> =
        (0..ny).map(|iy| (0..nx).map(|ix| tile_count(&material, ix, iy) > 0).collect()).collect();
    let xe: Vec<f64> = (0..=nx).map(|i| x0 as f64 + tile_nm as f64 * i as f64).collect();
    let ye: Vec<f64> = (0..=ny).map(|j| y0 as f64 + tile_nm as f64 * j as f64).collect();
    let mut rudy = vec![vec![0.0f64; nx]; ny];
    let half = tile_nm as f64 / 2.0;
    let overlap = |edges: &[f64], low: f64, high: f64| -> Vec<f64> {
        edges.windows(2).map(|pair| (pair[1].min(high) - pair[0].max(low)).max(0.0)).collect()
    };
    for net in &board.nets {
        if net.pads.len() < 2 || zone_nets.contains(&net.name) {
            continue;
        }
        let points: Vec<copper_physical::Point> = net
            .pads
            .iter()
            .flat_map(|pad| pad_points.get(&(pad.component.clone(), pad.pad.clone())).cloned().unwrap_or_default())
            .collect();
        if points.len() < 2 {
            continue;
        }
        let mut bx0 = points.iter().map(|p| p.x_nm).min().expect("points") as f64 - half;
        let mut bx1 = points.iter().map(|p| p.x_nm).max().expect("points") as f64 + half;
        let mut by0 = points.iter().map(|p| p.y_nm).min().expect("points") as f64 - half;
        let mut by1 = points.iter().map(|p| p.y_nm).max().expect("points") as f64 + half;
        if bx1 - bx0 < tile_nm as f64 {
            let middle = (bx0 + bx1) / 2.0;
            (bx0, bx1) = (middle - half, middle + half);
        }
        if by1 - by0 < tile_nm as f64 {
            let middle = (by0 + by1) / 2.0;
            (by0, by1) = (middle - half, middle + half);
        }
        let (w, h) = (bx1 - bx0, by1 - by0);
        let density = (w + h) * net_lane(&net.name) / (w * h) / lane_area;
        let tile = tile_nm as f64;
        let i0 = (py_floordiv(bx0 - x0 as f64, tile) as i64).max(0);
        let i1 = (nx as i64).min(py_floordiv(bx1 - x0 as f64, tile) as i64 + 1);
        let j0 = (py_floordiv(by0 - y0 as f64, tile) as i64).max(0);
        let j1 = (ny as i64).min(py_floordiv(by1 - y0 as f64, tile) as i64 + 1);
        if i0 >= i1 || j0 >= j1 {
            continue;
        }
        let (i0, i1, j0, j1) = (i0 as usize, i1 as usize, j0 as usize, j1 as usize);
        let ox = overlap(&xe[i0..=i1], bx0, bx1);
        let oy = overlap(&ye[j0..=j1], by0, by1);
        for (dj, yv) in oy.iter().enumerate() {
            for (di, xv) in ox.iter().enumerate() {
                rudy[j0 + dj][i0 + di] += density * (yv * xv);
            }
        }
    }
    for iy in 0..ny {
        for ix in 0..nx {
            demand[iy][ix] += if has_material[iy][ix] { rudy[iy][ix] } else { 0.0 };
        }
    }
    Ok(CongestionMap {
        tile_nm,
        x0_nm: x0,
        y0_nm: y0,
        nx,
        ny,
        demand,
        capacity,
        source: "rudy".to_owned(),
        failed_points: Vec::new(),
    })
}

/// `_estimate_score`: overflow area, then the sum of squared utilisation.
fn estimate_score(congestion: &CongestionMap) -> (f64, f64) {
    let utilisation = CongestionMap::flat(&congestion.utilisation());
    let overflow: Vec<f64> = utilisation.iter().map(|u| (u - 1.0).max(0.0)).collect();
    let squares: Vec<f64> = utilisation.iter().map(|u| u * u).collect();
    (pairwise_sum(&overflow), pairwise_sum(&squares))
}

/// `Proposal`: one legal pose change.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub reference: String,
    pub board: PhysicalBoard,
    pub delta_nm: (i64, i64),
    pub rotation_degrees: Decimal,
    pub reason: String,
    rank: Rank,
}

/// `(group, 0.0 - heat, multiplier, reference, kind, order)`.
#[derive(Clone, Debug, PartialEq)]
struct Rank(i64, f64, i64, String, i64, i64);

impl Rank {
    fn cmp(&self, other: &Rank) -> Ordering {
        self.0
            .cmp(&other.0)
            .then(self.1.partial_cmp(&other.1).unwrap_or(Ordering::Equal))
            .then(self.2.cmp(&other.2))
            .then(self.3.cmp(&other.3))
            .then(self.4.cmp(&other.4))
            .then(self.5.cmp(&other.5))
    }
}

/// `_snap(delta, grid_nm)`.
fn snap(delta: (f64, f64), grid_nm: i64) -> (i64, i64) {
    if grid_nm <= 0 {
        return (round_int(delta.0), round_int(delta.1));
    }
    let mut snapped = [round_int(delta.0 / grid_nm as f64) * grid_nm, round_int(delta.1 / grid_nm as f64) * grid_nm];
    if snapped == [0, 0] && (delta.0 != 0.0 || delta.1 != 0.0) {
        let axis = if delta.0.abs() >= delta.1.abs() { 0 } else { 1 };
        let value = if axis == 0 { delta.0 } else { delta.1 };
        snapped[axis] = if value > 0.0 { grid_nm } else { -grid_nm };
    }
    (snapped[0], snapped[1])
}

/// `_rotation_order(current, angle)`.
fn rotation_order(current: &Decimal, angle: &Decimal) -> Result<(i64, Decimal, Decimal)> {
    let full = Decimal::from_i64(360);
    let value = |result: std::result::Result<Decimal, copper_units::DecimalError>| {
        result.map_err(|_| Error::NotImplemented("a rotation Decimal operation Python would signal".to_owned()))
    };
    let modulo = |x: Decimal| -> Result<Decimal> {
        // Decimal % keeps the dividend's sign; Python's `%` on Decimal does too.
        value(x.rem(&full))
    };
    let turn = modulo(value(angle.sub(current))?)?;
    let back = modulo(value(current.sub(angle))?)?;
    let first = if turn == Decimal::from_i64(180) { 0 } else { 1 };
    Ok((first, turn.min(back), angle.clone()))
}

fn signature(placements: &IndexMap<String, Placement>) -> Vec<(String, (i64, i64), String, bool)> {
    let mut items: Vec<_> = placements
        .values()
        .map(|p| {
            // Decimal equality is numeric: `90` and `90.0` are one pose.
            let rotation = p
                .rotation_degrees
                .normalize()
                .map_or_else(|_| p.rotation_degrees.to_sci_string(), |d| d.to_sci_string());
            (p.reference.clone(), (p.position.x_nm, p.position.y_nm), rotation, p.side == BoardSide::Back)
        })
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

/// `propose_moves(board, congestion, failed_nets, options, step_nm=)`.
pub fn propose_moves(
    board: &PhysicalBoard,
    congestion: &CongestionMap,
    failed_nets: &IndexSet<String>,
    options: &PlannerOptions,
    step_nm: i64,
) -> Result<Vec<Proposal>> {
    let steps = [1i64, 2, 4];
    let limit = 24;
    if step_nm <= 0 {
        return Err(Error::Value("congestion proposal step, multipliers and limit must be positive".to_owned()));
    }
    if !board.tracks.is_empty() || !board.vias.is_empty() {
        return Ok(Vec::new());
    }
    let mut fixed: IndexSet<String> = options.fixed_references.iter().cloned().collect();
    fixed.extend(board.placement_rules.iter().filter(|r| r.fixed_position.is_some()).map(|r| r.reference.clone()));
    let poses: IndexMap<String, Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let mut on_failed: Vec<String> = board
        .nets
        .iter()
        .filter(|net| failed_nets.contains(&net.name))
        .flat_map(|net| net.pads.iter().map(|pad| pad.component.clone()))
        .filter(|component| poses.contains_key(component) && !fixed.contains(component))
        .collect::<IndexSet<String>>()
        .into_iter()
        .collect();
    on_failed.sort();
    let failed_net_of: IndexMap<String, String> = on_failed
        .iter()
        .map(|reference| {
            let name = board
                .nets
                .iter()
                .filter(|net| failed_nets.contains(&net.name) && net.pads.iter().any(|pad| pad.component == *reference))
                .map(|net| net.name.clone())
                .min()
                .expect("a failed net");
            (reference.clone(), name)
        })
        .collect();
    let scene = copper_place::scene::Scene::new(board, options.policy());
    let hot = congestion.hot_tiles(8);
    let mut hot_parts: IndexMap<String, (usize, usize)> = IndexMap::new();
    let mut others: Vec<&String> = poses.keys().filter(|r| !fixed.contains(*r) && !on_failed.contains(*r)).collect();
    others.sort();
    for reference in others {
        let polygon = scene.polygon(&poses[reference]);
        for (ix, iy, _) in &hot {
            let (x0, y0, x1, y1) = congestion.tile_bounds(*ix, *iy);
            if polygon.iter().map(|p| p.x_nm).min().expect("polygon") < x1
                && polygon.iter().map(|p| p.x_nm).max().expect("polygon") > x0
                && polygon.iter().map(|p| p.y_nm).min().expect("polygon") < y1
                && polygon.iter().map(|p| p.y_nm).max().expect("polygon") > y0
            {
                hot_parts.insert(reference.clone(), (*ix, *iy));
                break;
            }
        }
    }
    let grid = options.grid_step_nm;
    let utilisation = congestion.utilisation();
    let mut proposals: Vec<Proposal> = Vec::new();
    let mut seen: IndexSet<Vec<(String, (i64, i64), String, bool)>> = IndexSet::new();
    let checker = copper_drc::legality::Checker::new(board, options.policy());
    let mut offer = |reference: &str, proposed: Placement, rank: Rank, reason: String| -> Result<()> {
        let trial = match crate::routeflow::move_placement_unit(board, &poses, reference, proposed) {
            Ok(trial) => trial,
            Err(Error::Value(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        if !seen.insert(signature(&trial)) {
            return Ok(());
        }
        if !checker.placement_solution_is_legal(&trial)? {
            return Ok(());
        }
        let moved = &trial[reference];
        let current = &poses[reference];
        let mut moved_board = board.clone();
        moved_board.placements = board.placements.iter().map(|p| trial[&p.reference].clone()).collect();
        proposals.push(Proposal {
            reference: reference.to_owned(),
            board: moved_board,
            delta_nm: (moved.position.x_nm - current.position.x_nm, moved.position.y_nm - current.position.y_nm),
            rotation_degrees: moved.rotation_degrees.clone(),
            reason,
            rank,
        });
        Ok(())
    };
    let hot_list: Vec<String> = {
        let mut keys: Vec<String> = hot_parts.keys().cloned().collect();
        keys.sort();
        keys
    };
    for (group, references) in [(0i64, &on_failed), (1, &hot_list)] {
        for reference in references {
            let current = poses[reference].clone();
            let (ix, iy) = congestion.tile_of(current.position);
            let heat = utilisation[iy][ix];
            let why = if group == 0 {
                format!("failed_net:{}", failed_net_of[reference])
            } else {
                let (hx, hy) = hot_parts[reference];
                format!("hot_tile:{hx},{hy}")
            };
            let gradient = congestion.gradient_at(current.position);
            let directions: Vec<((f64, f64), &str)> = if gradient != (0.0, 0.0) {
                vec![(gradient, "gradient")]
            } else {
                vec![((-1.0, 0.0), "axis"), ((0.0, -1.0), "axis"), ((0.0, 1.0), "axis"), ((1.0, 0.0), "axis")]
            };
            for multiplier in steps {
                let distance = (step_nm * multiplier) as f64;
                for (order, ((dx, dy), how)) in directions.iter().enumerate() {
                    let delta = snap((dx * distance, dy * distance), grid);
                    if delta == (0, 0) {
                        continue;
                    }
                    let mut proposed = current.clone();
                    proposed.position =
                        copper_physical::Point::new(current.position.x_nm + delta.0, current.position.y_nm + delta.1);
                    offer(
                        reference,
                        proposed,
                        Rank(group, 0.0 - heat, multiplier, reference.clone(), 0, order as i64),
                        format!("{why};{how}x{multiplier}"),
                    )?;
                }
            }
            let mut angles: Vec<(Decimal, (i64, Decimal, Decimal))> = Vec::new();
            for angle in copper_drc::legality::allowed_orientations(board, reference) {
                if angle != current.rotation_degrees {
                    let key = rotation_order(&current.rotation_degrees, &angle)?;
                    angles.push((angle, key));
                }
            }
            angles.sort_by(|a, b| a.1.cmp(&b.1));
            for (order, (angle, _)) in angles.into_iter().enumerate() {
                let mut proposed = current.clone();
                proposed.rotation_degrees = angle.clone();
                offer(
                    reference,
                    proposed,
                    Rank(group, 0.0 - heat, 1, reference.clone(), 1, order as i64),
                    format!("{why};rotate{}", angle.to_sci_string()),
                )?;
            }
        }
    }
    proposals.sort_by(|a, b| a.rank.cmp(&b.rank));
    proposals.truncate(limit);
    Ok(proposals)
}

/// `screen(board, proposals, global_options, keep=, estimate_tile_nm=)`.
pub fn screen(
    board: &PhysicalBoard,
    proposals: Vec<Proposal>,
    global_options: &GlobalRouterOptions,
    keep: usize,
    estimate_tile_nm: i64,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<Vec<(Proposal, GlobalRoutingResult)>> {
    let count = proposals.len() as i64;
    emit(on_progress, "congestion_screen", "started", vec![("proposal_count", int(count)), ("keep", int(keep as i64))]);
    if proposals.is_empty() || keep == 0 {
        emit(
            on_progress,
            "congestion_screen",
            "finished",
            vec![("proposal_count", int(count)), ("routed_count", int(0)), ("kept_count", int(0))],
        );
        return Ok(Vec::new());
    }
    let (base_overflow, base_spread) = estimate_score(&estimate_demand(board, estimate_tile_nm, None)?);
    let mut cheap: Vec<((f64, f64), Proposal)> = Vec::new();
    for item in proposals {
        let (overflow, spread) = estimate_score(&estimate_demand(&item.board, estimate_tile_nm, None)?);
        cheap.push(((overflow - base_overflow, spread - base_spread), item));
    }
    let float_order = |a: &(f64, f64), b: &(f64, f64)| {
        a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
    };
    cheap.sort_by(|a, b| float_order(&a.0, &b.0).then(a.1.rank.cmp(&b.1.rank)));
    let best_overflow = cheap[0].0.0;
    let mut routed = Vec::new();
    for (score, item) in cheap.into_iter().take(2 * keep) {
        let route = route_global(&item.board, global_options, None, None, None)?;
        routed.push((score, item, route));
    }
    routed.sort_by(|a, b| {
        crate::routeflow::route_score(&a.2)
            .cmp(&crate::routeflow::route_score(&b.2))
            .then(float_order(&a.0, &b.0))
            .then(a.1.reference.cmp(&b.1.reference))
            .then(a.1.rank.cmp(&b.1.rank))
    });
    let routed_count = routed.len() as i64;
    let result: Vec<(Proposal, GlobalRoutingResult)> =
        routed.into_iter().take(keep).map(|(_, item, route)| (item, route)).collect();
    emit(
        on_progress,
        "congestion_screen",
        "finished",
        vec![
            ("proposal_count", int(count)),
            ("routed_count", int(routed_count)),
            ("kept_count", int(result.len() as i64)),
            ("best_overflow_delta", Value::Float(round_digits(best_overflow, 6))),
        ],
    );
    Ok(result)
}

/// `_normalised_utilisation`.
fn normalised_utilisation(congestion: &CongestionMap) -> Vec<Vec<f64>> {
    let utilisation = congestion.utilisation();
    let real: Vec<f64> =
        CongestionMap::flat(&utilisation).into_iter().filter(|u| *u > 0.0 && *u < UTILISATION_SENTINEL).collect();
    if real.is_empty() {
        return vec![vec![0.0; congestion.nx]; congestion.ny];
    }
    let hottest = real.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let scale = percentile(&real, NORMALISATION_PERCENTILE);
    utilisation
        .iter()
        .map(|row| row.iter().map(|u| (if *u >= UTILISATION_SENTINEL { hottest } else { *u }) / scale).collect())
        .collect()
}

/// `combine_normalised(primary, other, weight)`.
pub fn combine_normalised(primary: &CongestionMap, other: &CongestionMap, weight: f64) -> Result<CongestionMap> {
    if primary.lattice() != other.lattice() {
        return Err(Error::Value(format!(
            "congestion lattices differ: {:?} != {:?}",
            primary.lattice(),
            other.lattice()
        )));
    }
    let (a, b) = (normalised_utilisation(primary), normalised_utilisation(other));
    let demand: Vec<Vec<f64>> = (0..primary.ny)
        .map(|iy| {
            (0..primary.nx)
                .map(|ix| {
                    let utilisation = a[iy][ix] + weight * b[iy][ix];
                    let capacity = primary.capacity[iy][ix];
                    if capacity > 0.0 { utilisation * capacity } else { 0.0 }
                })
                .collect()
        })
        .collect();
    let mut failed: Vec<(String, copper_physical::Point)> = Vec::new();
    for item in primary.failed_points.iter().chain(other.failed_points.iter()) {
        if !failed.contains(item) {
            failed.push(item.clone());
        }
    }
    Ok(CongestionMap {
        tile_nm: primary.tile_nm,
        x0_nm: primary.x0_nm,
        y0_nm: primary.y0_nm,
        nx: primary.nx,
        ny: primary.ny,
        demand,
        capacity: primary.capacity.clone(),
        source: "combined".to_owned(),
        failed_points: failed,
    })
}

/// `feedback_map(board, router_map)`: the router's contention plus the RUDY
/// estimate on its lattice, or the estimate alone.
pub fn feedback_map(board: &PhysicalBoard, router_map: Option<&CongestionMap>) -> Result<CongestionMap> {
    match router_map {
        None => estimate_demand(board, DEFAULT_TILE_NM, None),
        Some(map) => {
            let estimate = estimate_demand(board, map.tile_nm, Some((map.x0_nm, map.y0_nm, map.nx, map.ny)))?;
            combine_normalised(map, &estimate, 1.0)
        }
    }
}

/// `FeedbackState`: one full pass of the pipeline tail on one placement.
pub struct FeedbackState<P> {
    pub board: PhysicalBoard,
    pub global_route: GlobalRoutingResult,
    pub critical: crate::critical::CriticalRoutingResult,
    pub detailed: crate::detailed::DetailedRoutingResult,
    pub drc: crate::drc_report::DrcReport,
    pub deferred_nets: IndexSet<String>,
    pub payload: P,
}

/// `FeedbackTrial`.
#[derive(Clone, Debug)]
pub struct FeedbackTrial {
    pub index: i64,
    pub round: i64,
    pub reference: String,
    pub delta_nm: (i64, i64),
    pub rotation_degrees: String,
    pub reason: String,
    pub step_nm: i64,
    pub outcome: String,
    pub score: Option<Vec<i64>>,
}

/// `FeedbackOutcome` without its state.
#[derive(Clone, Debug)]
pub struct FeedbackOutcome {
    pub trials: Vec<FeedbackTrial>,
    pub accepted_moves: i64,
    pub reruns: i64,
    pub rounds: i64,
    pub stop_reason: String,
    pub initial_score: Vec<i64>,
    pub final_score: Vec<i64>,
    pub initial_failed_nets: Vec<String>,
    pub final_failed_nets: Vec<String>,
    pub congestion: Option<CongestionMap>,
}

/// `float(Decimal(value) / Decimal(1_000_000))`.
fn millimetres(value: i64) -> f64 {
    let text =
        Decimal::from_i64(value).div(&Decimal::from_i64(1_000_000)).map(|d| d.to_sci_string()).unwrap_or_default();
    text.parse().unwrap_or(f64::NAN)
}

impl FeedbackOutcome {
    /// `hot_tiles(16)` of the final map.
    fn hot_tiles(&self) -> Value {
        let Some(congestion) = &self.congestion else { return Value::Array(Vec::new()) };
        Value::Array(
            congestion
                .hot_tiles(16)
                .into_iter()
                .map(|(ix, iy, value)| {
                    let (x0, y0, x1, y1) = congestion.tile_bounds(ix, iy);
                    object(vec![
                        ("ix", Value::from(ix as i64)),
                        ("iy", Value::from(iy as i64)),
                        ("utilisation", Value::Float(round_digits(value, 6))),
                        (
                            "bounds_mm",
                            Value::Array([x0, y0, x1, y1].iter().map(|v| Value::Float(millimetres(*v))).collect()),
                        ),
                    ])
                })
                .collect(),
        )
    }

    /// `report()`.
    pub fn report(&self) -> Value {
        let ints = |items: &[i64]| Value::Array(items.iter().map(|v| Value::from(*v)).collect());
        let texts = |items: &[String]| Value::Array(items.iter().map(|v| Value::from(v.as_str())).collect());
        let trials = self
            .trials
            .iter()
            .map(|t| {
                object(vec![
                    ("index", Value::from(t.index)),
                    ("round", Value::from(t.round)),
                    ("reference", Value::from(t.reference.as_str())),
                    ("delta_nm", ints(&[t.delta_nm.0, t.delta_nm.1])),
                    ("rotation_degrees", Value::from(t.rotation_degrees.as_str())),
                    ("reason", Value::from(t.reason.as_str())),
                    ("step_nm", Value::from(t.step_nm)),
                    ("outcome", Value::from(t.outcome.as_str())),
                    ("score", t.score.as_ref().map_or(Value::Null, |s| ints(s))),
                ])
            })
            .collect();
        object(vec![
            ("accepted_moves", Value::from(self.accepted_moves)),
            ("reruns", Value::from(self.reruns)),
            ("rounds", Value::from(self.rounds)),
            ("stop_reason", Value::from(self.stop_reason.as_str())),
            ("score_order", Value::Array(crate::flow::SCORE_ORDER.iter().map(|s| Value::from(*s)).collect())),
            ("initial_score", ints(&self.initial_score)),
            ("final_score", ints(&self.final_score)),
            ("initial_failed_nets", texts(&self.initial_failed_nets)),
            ("final_failed_nets", texts(&self.final_failed_nets)),
            ("map_source", self.congestion.as_ref().map_or(Value::Null, |c| Value::from(c.source.as_str()))),
            ("trials", Value::Array(trials)),
            ("hot_tiles", self.hot_tiles()),
        ])
    }
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// `failed_nets(state)`: unrouted ordinary nets, failed critical nets and
/// DRC-open nets, less deferred zone nets.
pub fn failed_nets<P>(state: &FeedbackState<P>) -> IndexSet<String> {
    let deferred = &state.deferred_nets;
    let mut failed: IndexSet<String> = state
        .detailed
        .nets
        .iter()
        .filter(|item| !item.connected && !deferred.contains(&item.net))
        .map(|item| item.net.clone())
        .collect();
    for item in &state.critical.nets {
        if !item.connected || !item.diagnostics.is_empty() {
            failed.extend(item.nets.iter().filter(|net| !net.starts_with('<')).cloned());
        }
    }
    for finding in state.drc.findings() {
        if finding.code == "DRC-OPEN-NET" {
            failed.extend(finding.nets.iter().filter(|net| !deferred.contains(*net)).cloned());
        }
    }
    failed
}

fn sorted(items: &IndexSet<String>) -> Vec<String> {
    let mut list: Vec<String> = items.iter().cloned().collect();
    list.sort();
    list
}

/// `run_placement_feedback`: bounded routability-driven placement feedback.
/// `rerun(board, global_route)` routes the pipeline tail of a kept proposal;
/// `score` is the acceptance tuple (lower is better).
#[allow(clippy::too_many_arguments)]
pub fn run_placement_feedback<P>(
    state: FeedbackState<P>,
    trials: i64,
    movement_nm: i64,
    placement_options: &PlannerOptions,
    global_options: &GlobalRouterOptions,
    rerun: &mut dyn FnMut(
        &PhysicalBoard,
        &GlobalRoutingResult,
        &mut Option<ProgressCallback<'_>>,
    ) -> Result<FeedbackState<P>>,
    score: &dyn Fn(&FeedbackState<P>) -> Result<Vec<i64>>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(FeedbackState<P>, FeedbackOutcome)> {
    let keep = 2usize;
    if trials < 0 || movement_nm <= 0 {
        return Err(Error::Value("placement feedback bounds are invalid".to_owned()));
    }
    let mut incumbent = state;
    let initial_score = score(&incumbent)?;
    let mut incumbent_score = initial_score.clone();
    let initial_failed = failed_nets(&incumbent);
    let mut records: Vec<FeedbackTrial> = Vec::new();
    let (mut accepted, mut rounds, mut stale, mut spent) = (0i64, 0i64, 0i64, 0i64);
    let mut step = movement_nm;
    let floor = MINIMUM_STEP_NM.min(step);
    let current_signature =
        |board: &PhysicalBoard| signature(&board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect());
    let mut tried = IndexSet::new();
    tried.insert(current_signature(&incumbent.board));
    let mut last_map: Option<CongestionMap> = None;
    let mut map_is_current = false;
    let texts = |items: &[String]| Value::Array(items.iter().map(|v| text(v)).collect());
    let ints = |items: &[i64]| Value::Array(items.iter().map(|v| Value::from(*v)).collect());
    emit(
        on_progress,
        "placement_feedback",
        "started",
        vec![
            ("trials", int(trials)),
            ("failed_nets", texts(&sorted(&initial_failed))),
            ("score", ints(&incumbent_score)),
        ],
    );
    let stop;
    loop {
        let failed = failed_nets(&incumbent);
        if failed.is_empty() {
            stop = "no_failed_nets";
            break;
        }
        if spent >= trials {
            stop = "trial_budget";
            break;
        }
        if stale >= 2 {
            stop = "no_improvement";
            break;
        }
        rounds += 1;
        let congestion = feedback_map(&incumbent.board, incumbent.detailed.congestion.as_ref())?;
        let proposals: Vec<Proposal> = propose_moves(&incumbent.board, &congestion, &failed, placement_options, step)?
            .into_iter()
            .filter(|item| !tried.contains(&current_signature(&item.board)))
            .collect();
        let tile = congestion.tile_nm;
        last_map = Some(congestion);
        map_is_current = true;
        let kept = screen(&incumbent.board, proposals, global_options, keep, tile, on_progress)?;
        let mut best: Option<(Vec<i64>, FeedbackState<P>, usize)> = None;
        for (proposal, global_route) in kept {
            if spent >= trials {
                break;
            }
            tried.insert(current_signature(&proposal.board));
            let index = records.len() as i64 + 1;
            emit(
                on_progress,
                "placement_feedback_trial",
                "started",
                vec![
                    ("index", int(index)),
                    ("round", int(rounds)),
                    ("reference", text(&proposal.reference)),
                    ("delta_nm", ints(&[proposal.delta_nm.0, proposal.delta_nm.1])),
                    ("reason", text(&proposal.reason)),
                ],
            );
            let mut result = None;
            if global_route.status == GlobalRoutingStatus::Success {
                spent += 1;
                result = Some(rerun(&proposal.board, &global_route, on_progress)?);
            }
            let trial_score = match &result {
                Some(result) => Some(score(result)?),
                None => None,
            };
            let mut improved = false;
            if let (Some(trial_score), Some(result)) = (&trial_score, result) {
                let threshold = best.as_ref().map_or(&incumbent_score, |b| &b.0);
                if trial_score < threshold {
                    best = Some((trial_score.clone(), result, records.len()));
                    improved = true;
                }
            }
            let outcome = if trial_score.is_none() { "global_failed" } else { "not_improved" };
            records.push(FeedbackTrial {
                index,
                round: rounds,
                reference: proposal.reference.clone(),
                delta_nm: proposal.delta_nm,
                rotation_degrees: proposal.rotation_degrees.to_sci_string(),
                reason: proposal.reason.clone(),
                step_nm: step,
                outcome: outcome.to_owned(),
                score: trial_score.clone(),
            });
            let reported = if trial_score.is_none() {
                "global_failed"
            } else if improved {
                "improved"
            } else {
                "not_improved"
            };
            emit(
                on_progress,
                "placement_feedback_trial",
                "finished",
                vec![
                    ("index", int(index)),
                    ("outcome", text(reported)),
                    ("score", trial_score.as_ref().map_or(Value::Null, |s| ints(s))),
                ],
            );
        }
        if let Some((best_score, best_state, position)) = best {
            incumbent_score = best_score;
            incumbent = best_state;
            records[position].outcome = "accepted".to_owned();
            accepted += 1;
            stale = 0;
            map_is_current = false;
        } else {
            stale += 1;
            step = (step / 2).max(floor);
        }
    }
    if last_map.is_some() && !map_is_current {
        last_map = Some(feedback_map(&incumbent.board, incumbent.detailed.congestion.as_ref())?);
    }
    let final_failed = failed_nets(&incumbent);
    emit(
        on_progress,
        "placement_feedback",
        "finished",
        vec![
            ("stop_reason", text(stop)),
            ("trials", int(spent)),
            ("accepted_moves", int(accepted)),
            ("score", ints(&incumbent_score)),
            ("failed_nets", texts(&sorted(&final_failed))),
        ],
    );
    let outcome = FeedbackOutcome {
        trials: records,
        accepted_moves: accepted,
        reruns: spent,
        rounds,
        stop_reason: stop.to_owned(),
        initial_score,
        final_score: incumbent_score,
        initial_failed_nets: sorted(&initial_failed),
        final_failed_nets: sorted(&final_failed),
        congestion: last_map,
    };
    Ok((incumbent, outcome))
}

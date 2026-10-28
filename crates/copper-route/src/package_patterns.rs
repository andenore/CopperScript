//! Ball-array detection (`pcbir.package_patterns`: `circular_array` and
//! `ball_grid`). The dogbone pattern proposals of that module are not here.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::PhysicalBoard;
use copper_physical::footprint::{PadKind, PadShape};
use copper_physical::Point;
use copper_physical::pyfloat::round_int;

use crate::{Result, fail};

/// `CircularArray`: a complete square-pitch array in footprint-local coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CircularArray {
    pub component: String,
    pub xs_nm: Vec<i64>,
    pub ys_nm: Vec<i64>,
    pub pitch_nm: i64,
    pub land_diameter_nm: i64,
}

/// `circular_array`: `Ok(None)` when the footprint is not a full round-land array.
pub fn circular_array(board: &PhysicalBoard, component: &str) -> Result<Option<CircularArray>> {
    let Some(pose) = board.placements.iter().find(|p| p.reference == component) else {
        return fail("unknown package component");
    };
    let pads = &board.footprints[&pose.footprint].pads;
    let numbers: IndexSet<&str> = pads.iter().map(|p| p.number.as_str()).collect();
    if pads.is_empty()
        || pads.iter().any(|p| {
            p.kind != PadKind::Smd || p.shape != PadShape::Circle || p.size.width_nm != p.size.height_nm
        })
        || numbers.len() != pads.len()
    {
        return Ok(None);
    }
    let points: IndexSet<Point> = pads.iter().map(|p| p.position).collect();
    let mut xs: Vec<i64> = points.iter().map(|p| p.x_nm).collect::<IndexSet<_>>().into_iter().collect();
    let mut ys: Vec<i64> = points.iter().map(|p| p.y_nm).collect::<IndexSet<_>>().into_iter().collect();
    xs.sort_unstable();
    ys.sort_unstable();
    let pitches: IndexSet<i64> =
        xs.windows(2).chain(ys.windows(2)).map(|pair| pair[1] - pair[0]).collect();
    let sizes: IndexSet<i64> = pads.iter().map(|p| p.size.width_nm).collect();
    if xs.len().min(ys.len()) < 3
        || points.len() != xs.len() * ys.len()
        || points.len() != pads.len()
        || pitches.len() != 1
        || sizes.len() != 1
    {
        return Ok(None);
    }
    Ok(Some(CircularArray {
        component: component.to_owned(),
        xs_nm: xs,
        ys_nm: ys,
        pitch_nm: pitches[0],
        land_diameter_nm: sizes[0],
    }))
}

/// `BallGrid`: a square-pitch ball grid, possibly depopulated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BallGrid {
    pub component: String,
    pub xs_nm: Vec<i64>,
    pub ys_nm: Vec<i64>,
    pub pitch_nm: i64,
    pub land_diameter_nm: i64,
    pub balls: IndexSet<String>,
    pub tolerance_nm: i64,
}

impl BallGrid {
    /// `index`: (column, row) of a lattice position within tolerance.
    pub fn index(&self, position: Point) -> Option<(usize, usize)> {
        let ix = round_int((position.x_nm - self.xs_nm[0]) as f64 / self.pitch_nm as f64);
        let iy = round_int((position.y_nm - self.ys_nm[0]) as f64 / self.pitch_nm as f64);
        if ix >= 0
            && (ix as usize) < self.xs_nm.len()
            && iy >= 0
            && (iy as usize) < self.ys_nm.len()
            && (position.x_nm - self.xs_nm[ix as usize]).abs() <= self.tolerance_nm
            && (position.y_nm - self.ys_nm[iy as usize]).abs() <= self.tolerance_nm
        {
            Some((ix as usize, iy as usize))
        } else {
            None
        }
    }
}

/// `ball_grid` with its default tolerance (2 um) and density (0.5).
pub fn ball_grid(board: &PhysicalBoard, component: &str) -> Result<Option<BallGrid>> {
    let tolerance_nm: i64 = 2_000;
    let minimum_density = 0.5;
    let Some(pose) = board.placements.iter().find(|p| p.reference == component) else {
        return fail("unknown package component");
    };
    let round_lands: Vec<_> = board.footprints[&pose.footprint]
        .pads
        .iter()
        .filter(|p| p.kind == PadKind::Smd && p.shape == PadShape::Circle && p.size.width_nm == p.size.height_nm)
        .collect();
    if round_lands.len() < 9 {
        return Ok(None);
    }
    // Counter.most_common(1): the highest count, first seen on a tie.
    let mut counts: IndexMap<i64, usize> = IndexMap::new();
    for pad in &round_lands {
        *counts.entry(pad.size.width_nm).or_insert(0) += 1;
    }
    let best = *counts.values().max().expect("lands");
    let size = *counts.iter().find(|(_, count)| **count == best).map(|(k, _)| k).expect("lands");
    let balls: Vec<_> = round_lands.iter().filter(|p| p.size.width_nm == size).collect();
    let numbers: IndexSet<&str> = balls.iter().map(|p| p.number.as_str()).collect();
    if numbers.len() != balls.len() {
        return Ok(None);
    }
    // Column centres, then the pitch they share (None when irregular).
    let axis = |mut values: Vec<i64>| -> Option<(Vec<i64>, i64)> {
        values.sort_unstable();
        let mut centres: Vec<i64> = Vec::new();
        let mut group: Vec<i64> = Vec::new();
        for value in values {
            if !group.is_empty() && value - group[group.len() - 1] > tolerance_nm {
                centres.push(round_int(group.iter().sum::<i64>() as f64 / group.len() as f64));
                group.clear();
            }
            group.push(value);
        }
        centres.push(round_int(group.iter().sum::<i64>() as f64 / group.len() as f64));
        if centres.len() < 3 {
            return None;
        }
        let step = centres.windows(2).map(|pair| pair[1] - pair[0]).min().expect("three centres");
        let span = centres[centres.len() - 1] - centres[0];
        // The mean spacing, so rounding in single gaps does not accumulate.
        let pitch = round_int(span as f64 / round_int(span as f64 / step as f64) as f64);
        if centres.windows(2).any(|pair| {
            let gap = pair[1] - pair[0];
            (gap - round_int(gap as f64 / pitch as f64) * pitch).abs() > 2 * tolerance_nm
        }) {
            return None;
        }
        Some((centres, pitch))
    };
    let Some((xs, pitch_x)) = axis(balls.iter().map(|p| p.position.x_nm).collect()) else { return Ok(None) };
    let Some((ys, pitch_y)) = axis(balls.iter().map(|p| p.position.y_nm).collect()) else { return Ok(None) };
    if (pitch_x - pitch_y).abs() > 2 * tolerance_nm {
        return Ok(None);
    }
    let pitch = round_int((pitch_x + pitch_y) as f64 / 2.0);
    let nx = round_int((xs[xs.len() - 1] - xs[0]) as f64 / pitch as f64) + 1;
    let ny = round_int((ys[ys.len() - 1] - ys[0]) as f64 / pitch as f64) + 1;
    if (balls.len() as f64) < minimum_density * (nx * ny) as f64 {
        return Ok(None);
    }
    let grid = BallGrid {
        component: component.to_owned(),
        xs_nm: (0..nx).map(|k| xs[0] + k * pitch).collect(),
        ys_nm: (0..ny).map(|k| ys[0] + k * pitch).collect(),
        pitch_nm: pitch,
        land_diameter_nm: size,
        balls: balls.iter().map(|p| p.number.clone()).collect(),
        tolerance_nm,
    };
    if balls.iter().any(|p| grid.index(p.position).is_none()) {
        return Ok(None);
    }
    Ok(Some(grid))
}

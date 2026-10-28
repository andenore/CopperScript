//! Finite geometry-derived via proposals (`pcbir.access_sites`); callers own
//! all legality checks.

use indexmap::IndexSet;

use copper_pcb::board::{PhysicalBoard, Placement};
use copper_pcb::transform::transformed_local_point;
use copper_physical::Point;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::{Error, Result};

/// `circular_array_depth`: ring depth in a complete regular circular-land array, else None.
pub fn circular_array_depth(
    board: &PhysicalBoard,
    placement: &Placement,
    pad: &FootprintPad,
    maximum_neighbor_distance_nm: i64,
) -> Option<i64> {
    if pad.kind != PadKind::Smd || pad.shape != PadShape::Circle {
        return None;
    }
    let lands: Vec<&FootprintPad> =
        board.footprints[&placement.footprint].pads.iter().filter(|p| p.kind == PadKind::Smd).collect();
    if lands.iter().any(|p| p.shape != PadShape::Circle) {
        return None;
    }
    let points: IndexSet<Point> = lands.iter().map(|p| p.position).collect();
    let mut xs: Vec<i64> = points.iter().map(|p| p.x_nm).collect();
    xs.sort_unstable();
    xs.dedup();
    let mut ys: Vec<i64> = points.iter().map(|p| p.y_nm).collect();
    ys.sort_unstable();
    ys.dedup();
    if xs.len().min(ys.len()) < 3 || points.len() != xs.len() * ys.len() {
        return None;
    }
    let pitches: IndexSet<i64> = [&xs, &ys].iter().flat_map(|axis| axis.windows(2).map(|w| w[1] - w[0])).collect();
    if pitches.len() != 1 || pitches[0] > maximum_neighbor_distance_nm {
        return None;
    }
    let x = xs.iter().position(|v| *v == pad.position.x_nm)? as i64;
    let y = ys.iter().position(|v| *v == pad.position.y_nm)? as i64;
    Some(x.min(y).min(xs.len() as i64 - 1 - x).min(ys.len() as i64 - 1 - y))
}

/// `circular_cell_sites`: at most four actual square-cell centres.
pub fn circular_cell_sites(
    board: &PhysicalBoard,
    placement: &Placement,
    pad: &FootprintPad,
    maximum_neighbor_distance_nm: i64,
    maximum_radius_nm: i64,
) -> Vec<Point> {
    if pad.kind != PadKind::Smd || pad.shape != PadShape::Circle {
        return Vec::new();
    }
    let points: IndexSet<Point> = board.footprints[&placement.footprint]
        .pads
        .iter()
        .filter(|p| p.kind == PadKind::Smd && p.shape == PadShape::Circle)
        .map(|p| p.position)
        .collect();
    let (x, y) = (pad.position.x_nm, pad.position.y_nm);
    let mut sites: IndexSet<Point> = IndexSet::new();
    for (sx, sy) in [(1i64, 1i64), (1, -1), (-1, 1), (-1, -1)] {
        let dx = points
            .iter()
            .filter(|p| p.y_nm == y && sx * (p.x_nm - x) > 0)
            .map(|p| sx * (p.x_nm - x))
            .min()
            .unwrap_or(0);
        let dy = points
            .iter()
            .filter(|p| p.x_nm == x && sy * (p.y_nm - y) > 0)
            .map(|p| sy * (p.y_nm - y))
            .min()
            .unwrap_or(0);
        if dx == 0
            || dx != dy
            || dx > maximum_neighbor_distance_nm
            || i128::from(dx) * i128::from(dx) > 2 * i128::from(maximum_radius_nm) * i128::from(maximum_radius_nm)
            || !points.contains(&Point::new(x + sx * dx, y + sy * dy))
        {
            continue;
        }
        sites.insert(transformed_local_point(
            placement,
            Point::new(x + sx * dx.div_euclid(2), y + sy * dy.div_euclid(2)),
        ));
    }
    let start = transformed_local_point(placement, pad.position);
    let mut sites: Vec<Point> = sites.into_iter().collect();
    sites.sort_by_key(|p| {
        (
            -(i128::from(p.x_nm - start.x_nm) * i128::from(start.x_nm - placement.position.x_nm)
                + i128::from(p.y_nm - start.y_nm) * i128::from(start.y_nm - placement.position.y_nm)),
            p.x_nm,
            p.y_nm,
        )
    });
    sites
}

/// Python's `a // b` (floor division, any sign).
fn floor_div(a: i64, b: i64) -> i64 {
    let quotient = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) { quotient - 1 } else { quotient }
}

fn squared(dx: i64, dy: i64) -> i128 {
    i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy)
}

/// `obstacle_boundary_sites`: local obstacle frontiers projected around a
/// finite set of launch seeds.
#[allow(clippy::too_many_arguments)]
pub fn obstacle_boundary_sites(
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    size_nm: i64,
    drill_nm: i64,
    span: (copper_physical::CopperLayer, copper_physical::CopperLayer),
    maximum_radius_nm: i64,
    maximum_sites: i64,
    seeds: &[Point],
    maximum_obstacles: i64,
) -> Result<Vec<Point>> {
    if maximum_sites < 0 {
        return Err(Error::Value("geometry site limit must be a nonnegative integer".to_owned()));
    }
    if maximum_sites == 0 {
        return Ok(Vec::new());
    }
    if maximum_obstacles <= 0 {
        return Err(Error::Value("geometry obstacle limit must be a positive integer".to_owned()));
    }
    let boundaries = clearance.via_obstacle_bounds(
        net,
        start,
        size_nm,
        drill_nm,
        span.0,
        span.1,
        maximum_radius_nm,
        maximum_obstacles as usize,
    )?;
    let mut all_seeds: IndexSet<Point> = seeds.iter().copied().collect();
    all_seeds.insert(start);
    let seeds: Vec<Point> = all_seeds.into_iter().collect();
    let radius_squared = i128::from(maximum_radius_nm) * i128::from(maximum_radius_nm);
    let mut candidates: IndexSet<Point> = IndexSet::new();
    let mut rays: IndexSet<Point> = IndexSet::new();
    for seed in &seeds {
        let mut xs: Vec<i64> = boundaries.iter().flat_map(|b| [b.min_x, b.max_x]).collect();
        xs.sort_unstable();
        xs.dedup();
        xs.sort_by_key(|x| ((x - seed.x_nm).abs(), *x));
        xs.truncate(4);
        let mut ys: Vec<i64> = boundaries.iter().flat_map(|b| [b.min_y, b.max_y]).collect();
        ys.sort_unstable();
        ys.dedup();
        ys.sort_by_key(|y| ((y - seed.y_nm).abs(), *y));
        ys.truncate(4);
        let (dx, dy) = (seed.x_nm - start.x_nm, seed.y_nm - start.y_nm);
        if (dx != 0 || dy != 0) && (dx == 0 || dy == 0 || dx.abs() == dy.abs()) {
            for x in xs.iter().flat_map(|v| [v - 1, *v, v + 1]) {
                let along = i128::from(x - start.x_nm) * i128::from(dx);
                if dx != 0 && 0 < along && along < i128::from(dx) * i128::from(dx) {
                    rays.insert(Point::new(x, start.y_nm + floor_div((x - start.x_nm) * dy, dx)));
                }
            }
            for y in ys.iter().flat_map(|v| [v - 1, *v, v + 1]) {
                let along = i128::from(y - start.y_nm) * i128::from(dy);
                if dy != 0 && 0 < along && along < i128::from(dy) * i128::from(dy) {
                    rays.insert(Point::new(start.x_nm + floor_div((y - start.y_nm) * dx, dy), y));
                }
            }
        }
        let column: Vec<i64> = xs.iter().copied().chain([seed.x_nm]).collect();
        let row: Vec<i64> = ys.iter().copied().chain([seed.y_nm]).collect();
        for x in &column {
            for y in &row {
                let point = Point::new(*x, *y);
                if !seeds.contains(&point) && squared(x - start.x_nm, y - start.y_nm) <= radius_squared {
                    candidates.insert(point);
                }
            }
        }
    }
    rays.retain(|p| !seeds.contains(p) && squared(p.x_nm - start.x_nm, p.y_nm - start.y_nm) <= radius_squared);
    let mut points: Vec<Point> =
        candidates.into_iter().chain(rays.iter().copied()).collect::<IndexSet<_>>().into_iter().collect();
    points.sort_by_key(|p| {
        (
            !rays.contains(p),
            seeds.iter().map(|s| squared(p.x_nm - s.x_nm, p.y_nm - s.y_nm)).min().expect("seeds"),
            squared(p.x_nm - start.x_nm, p.y_nm - start.y_nm),
            p.x_nm,
            p.y_nm,
        )
    });
    points.truncate(maximum_sites as usize);
    Ok(points)
}

//! Placement quality: wirelength, group spread, the coarse layer-aware
//! routing estimate and the candidate metrics (`placement_metrics`,
//! `_coarse_route`, `_hpwl`, `_planning_wirelength`, `_fast_score`).

use indexmap::IndexMap;

use copper_pcb::Result;
use copper_physical::Point;
use copper_physical::geometry::segments_intersect;

use crate::escape::EscapeSpacingModel;
use crate::options::PlannerOptions;
use crate::power::power_domain_penalty;
use crate::scene::{Poses, Scene, point_bounds};

/// `PlacementMetrics`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementMetrics {
    pub component_count: i64,
    pub net_count: i64,
    pub estimated_connection_count: i64,
    pub half_perimeter_wire_length_nm: i64,
    pub crossing_count: i64,
    pub estimated_via_count: i64,
    pub pin_escape_pressure: i64,
    pub escape_channel_penalty_nm: i64,
    pub escape_channel_deficit_nm: i64,
    pub escape_channel_pair_count: i64,
    pub constraint_penalty_nm: i64,
    pub minimum_constraint_margin_nm: i64,
    pub group_spread_nm: i64,
    pub congestion_bin_count: i64,
    pub congestion_capacity_per_bin: i64,
    pub congestion_overflow: i64,
    pub maximum_congestion_utilization_ppm: i64,
    pub power_domain_penalty_nm: i64,
    pub weighted_wire_length_nm: i64,
    pub local_connection_length_nm: i64,
}

impl PlacementMetrics {
    /// `quality_vector`: lexicographic candidate quality, lower is better.
    pub fn quality_vector(&self) -> [i64; 10] {
        [
            self.constraint_penalty_nm,
            self.escape_channel_deficit_nm,
            self.congestion_overflow,
            self.crossing_count,
            self.estimated_via_count,
            self.pin_escape_pressure,
            self.local_connection_length_nm,
            -self.minimum_constraint_margin_nm,
            self.weighted_wire_length_nm + self.power_domain_penalty_nm,
            self.group_spread_nm,
        ]
    }
}

/// `_dominates`: no worse everywhere and better somewhere.
pub fn dominates(left: &PlacementMetrics, right: &PlacementMetrics) -> bool {
    let (a, b) = (left.quality_vector(), right.quality_vector());
    a.iter().zip(&b).all(|(x, y)| x <= y) && a.iter().zip(&b).any(|(x, y)| x < y)
}

/// `_net_points`: each placed pad of a net, keyed `component.pad:index`.
pub fn net_points(scene: &Scene, placements: &Poses, net_index: usize) -> Result<Vec<(String, Point)>> {
    let net = &scene.board.nets[net_index];
    let mut result = Vec::new();
    for (index, pad) in net.pads.iter().enumerate() {
        if let Some(pose) = placements.get(&pad.component) {
            result.push((format!("{}.{}:{index}", pad.component, pad.pad), scene.pad_position(pose, &pad.pad)?));
        }
    }
    Ok(result)
}

fn span(points: &[(String, Point)]) -> i64 {
    let (min_x, max_x) = (points.iter().map(|p| p.1.x_nm).min(), points.iter().map(|p| p.1.x_nm).max());
    let (min_y, max_y) = (points.iter().map(|p| p.1.y_nm).min(), points.iter().map(|p| p.1.y_nm).max());
    max_x.unwrap() - min_x.unwrap() + max_y.unwrap() - min_y.unwrap()
}

/// `_hpwl`: the half-perimeter wire length of every net.
pub fn hpwl(scene: &Scene, placements: &Poses) -> Result<i64> {
    let mut total = 0;
    for index in 0..scene.board.nets.len() {
        let points = net_points(scene, placements, index)?;
        if points.len() >= 2 {
            total += span(&points);
        }
    }
    Ok(total)
}

/// `_planning_wirelength`: plane rails need terminal access, not a full distribution tree.
pub fn planning_wirelength(scene: &Scene, placements: &Poses) -> Result<i64> {
    let mut total = 0;
    for (index, net) in scene.board.nets.iter().enumerate() {
        let points = net_points(scene, placements, index)?;
        if points.len() >= 2 {
            let span = span(&points);
            total += if scene.plane_nets.contains(&net.name) { span.div_euclid(4) } else { span };
        }
    }
    Ok(total)
}

/// `_group_spread`: weighted L1 distance of each group's members from its anchor.
pub fn group_spread(scene: &Scene, placements: &Poses) -> i64 {
    let mut total = 0;
    for group in &scene.board.placement_groups {
        let members: Vec<&copper_pcb::board::Placement> =
            group.references.iter().filter_map(|r| placements.get(r)).collect();
        if members.len() < 2 {
            continue;
        }
        let anchor = match group.anchor.as_ref().and_then(|a| placements.get(a)) {
            Some(anchor) => anchor.position,
            None => {
                let count = members.len() as i64;
                Point::new(
                    members.iter().map(|m| m.position.x_nm).sum::<i64>().div_euclid(count),
                    members.iter().map(|m| m.position.y_nm).sum::<i64>().div_euclid(count),
                )
            }
        };
        let distance: i64 =
            members.iter().map(|m| (m.position.x_nm - anchor.x_nm).abs() + (m.position.y_nm - anchor.y_nm).abs()).sum();
        total += distance * group.priority.max(1);
    }
    total
}

/// `_fast_score`: `(relative penalty, escape deficit, local length, weighted wirelength)`.
pub fn fast_score(scene: &Scene, placements: &Poses, spacing: &EscapeSpacingModel, weight: f64) -> Result<[i64; 4]> {
    let deficit = spacing.deficit_nm(placements);
    Ok([
        scene.penalty(placements)?,
        deficit,
        scene.local_connection_length(placements, &scene.relative)?,
        planning_wirelength(scene, placements)?
            + group_spread(scene, placements).div_euclid(20)
            + power_domain_penalty(scene, placements, weight)?,
    ])
}

/// `_ESCAPE_SCORE_STEP_NM`.
const ESCAPE_SCORE_STEP_NM: i64 = 100_000;

/// `_meaningfully_better`: `new < old`, with escape-width differences under 0.1 mm as ties.
pub fn meaningfully_better(new: &[i64; 4], old: &[i64; 4]) -> bool {
    if new[0] != old[0] {
        return new[0] < old[0];
    }
    if (new[1] - old[1]).abs() >= ESCAPE_SCORE_STEP_NM {
        return new[1] < old[1];
    }
    new[2..] < old[2..]
}

/// `_CoarseRoute`.
#[derive(Clone, Debug, PartialEq)]
pub struct CoarseRoute {
    pub bin_count: i64,
    pub capacity: i64,
    pub overflow: i64,
    pub maximum_utilization_ppm: i64,
    pub connection_count: i64,
    pub crossing_count: i64,
    pub estimated_vias: i64,
}

struct Segment<'n> {
    net: &'n str,
    layer: usize,
    start: Point,
    end: Point,
}

/// `_manhattan_mst`: Prim's tree in L1, ties by name.
fn manhattan_mst(points: &[(String, Point)]) -> Vec<(usize, usize)> {
    if points.len() < 2 {
        return Vec::new();
    }
    let first = (0..points.len()).min_by(|a, b| points[*a].0.cmp(&points[*b].0)).expect("points");
    let mut connected = vec![first];
    let mut remaining: Vec<usize> = (0..points.len()).filter(|i| *i != first).collect();
    let mut edges = Vec::new();
    while !remaining.is_empty() {
        let mut best: Option<(i64, usize, usize)> = None;
        for &source in &connected {
            for &target in &remaining {
                let (a, b) = (points[source].1, points[target].1);
                let distance = (a.x_nm - b.x_nm).abs() + (a.y_nm - b.y_nm).abs();
                let better = match best {
                    None => true,
                    Some((d, s, t)) => {
                        (distance, &points[source].0, &points[target].0) < (d, &points[s].0, &points[t].0)
                    }
                };
                if better {
                    best = Some((distance, source, target));
                }
            }
        }
        let (_, left, right) = best.expect("an edge");
        edges.push((left, right));
        connected.push(right);
        remaining.retain(|i| *i != right);
    }
    edges
}

fn bin(point: Point, min_x: i64, min_y: i64, columns: i64, rows: i64, size: i64) -> (i64, i64) {
    (
        (columns - 1).min(0i64.max((point.x_nm - min_x).div_euclid(size))),
        (rows - 1).min(0i64.max((point.y_nm - min_y).div_euclid(size))),
    )
}

fn axis_path(start: (i64, i64), end: (i64, i64)) -> Vec<(i64, i64)> {
    if start.0 != end.0 {
        let step = if end.0 > start.0 { 1 } else { -1 };
        let mut cells = Vec::new();
        let mut x = start.0;
        while x != end.0 + step {
            cells.push((x, start.1));
            x += step;
        }
        return cells;
    }
    let step = if end.1 > start.1 { 1 } else { -1 };
    let mut cells = Vec::new();
    let mut y = start.1;
    while y != end.1 + step {
        cells.push((start.0, y));
        y += step;
    }
    cells
}

fn grid_path(start: (i64, i64), end: (i64, i64), horizontal_first: bool) -> Vec<(i64, i64)> {
    let bend = if horizontal_first { (end.0, start.1) } else { (start.0, end.1) };
    let mut path = axis_path(start, bend);
    path.extend(axis_path(bend, end).into_iter().skip(1));
    path
}

/// `_coarse_route`: a greedy two-bend layer-aware estimate on congestion bins.
pub fn coarse_route(scene: &Scene, placements: &Poses, options: &PlannerOptions) -> Result<CoarseRoute> {
    let board = scene.board;
    let (min_x, min_y, max_x, max_y) = point_bounds(&board.outline.vertices);
    let size = options.congestion_bin_nm;
    let columns = 1i64.max((max_x - min_x + size - 1).div_euclid(size));
    let rows = 1i64.max((max_y - min_y + size - 1).div_euclid(size));
    let pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm;
    let capacity = 1i64.max(size.div_euclid(pitch));
    let mut demand: IndexMap<(usize, i64, i64), i64> = IndexMap::new();
    let mut segments: Vec<Segment> = Vec::new();
    let (mut connections, mut cross_side) = (0i64, 0i64);
    let layers = board.stackup.copper_layers.len().max(1);
    for (net_index, net) in board.nets.iter().enumerate() {
        let points = net_points(scene, placements, net_index)?;
        for (left, right) in manhattan_mst(&points) {
            connections += 1;
            let reference = |key: &str| key.split_once('.').map_or(key, |(r, _)| r).to_owned();
            let (left_ref, right_ref) = (reference(&points[left].0), reference(&points[right].0));
            if placements[&left_ref].side != placements[&right_ref].side {
                cross_side += 1;
            }
            let (a, b) = (points[left].1, points[right].1);
            let start = bin(a, min_x, min_y, columns, rows, size);
            let end = bin(b, min_x, min_y, columns, rows, size);
            let mut best: Option<((i64, i64, usize, bool), Vec<(i64, i64)>)> = None;
            for layer in 0..layers {
                for horizontal_first in [true, false] {
                    let path = grid_path(start, end, horizontal_first);
                    let used = |cell: &(i64, i64)| demand.get(&(layer, cell.0, cell.1)).copied().unwrap_or(0);
                    let cost: i64 = path.iter().map(used).sum();
                    let overflow: i64 = path.iter().map(|cell| 0i64.max(used(cell) + 1 - capacity)).sum();
                    let key = (overflow, cost, layer, horizontal_first);
                    if best.as_ref().is_none_or(|(k, p)| (key, &path) < (*k, p)) {
                        best = Some((key, path));
                    }
                }
            }
            let ((_, _, layer, horizontal_first), path) = best.expect("a layer");
            for cell in path {
                *demand.entry((layer, cell.0, cell.1)).or_insert(0) += 1;
            }
            let bend = if horizontal_first { Point::new(b.x_nm, a.y_nm) } else { Point::new(a.x_nm, b.y_nm) };
            if a != bend {
                segments.push(Segment { net: &net.name, layer, start: a, end: bend });
            }
            if bend != b {
                segments.push(Segment { net: &net.name, layer, start: bend, end: b });
            }
        }
    }
    let mut crossings = 0i64;
    for (index, left) in segments.iter().enumerate() {
        for right in &segments[index + 1..] {
            if left.net == right.net || left.layer != right.layer {
                continue;
            }
            if segments_intersect(left.start, left.end, right.start, right.end) {
                crossings += 1;
            }
        }
    }
    let overflow = demand.values().map(|v| 0i64.max(v - capacity)).sum();
    let maximum = demand.values().map(|v| (v * 1_000_000).div_euclid(capacity)).max().unwrap_or(0);
    Ok(CoarseRoute {
        bin_count: columns * rows * layers as i64,
        capacity,
        overflow,
        maximum_utilization_ppm: maximum,
        connection_count: connections,
        crossing_count: crossings,
        estimated_vias: cross_side + crossings.div_euclid(layers as i64),
    })
}

/// `placement_metrics`.
pub fn placement_metrics(
    scene: &Scene,
    placements: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<PlacementMetrics> {
    let route = coarse_route(scene, placements, options)?;
    let channels = spacing.channels(placements, false);
    let deficit: i64 = channels.iter().map(|c| c.deficit_nm()).sum();
    Ok(PlacementMetrics {
        component_count: placements.len() as i64,
        net_count: scene.board.nets.len() as i64,
        estimated_connection_count: route.connection_count,
        half_perimeter_wire_length_nm: hpwl(scene, placements)?,
        crossing_count: route.crossing_count,
        estimated_via_count: route.estimated_vias,
        pin_escape_pressure: channels.iter().map(|c| c.pad_count).sum(),
        escape_channel_penalty_nm: deficit * 40,
        escape_channel_deficit_nm: deficit,
        escape_channel_pair_count: channels.len() as i64,
        constraint_penalty_nm: scene.penalty(placements)?,
        minimum_constraint_margin_nm: scene.relative_margin(placements)?,
        group_spread_nm: group_spread(scene, placements),
        congestion_bin_count: route.bin_count,
        congestion_capacity_per_bin: route.capacity,
        congestion_overflow: route.overflow,
        maximum_congestion_utilization_ppm: route.maximum_utilization_ppm,
        power_domain_penalty_nm: power_domain_penalty(scene, placements, options.power_domain_weight)?,
        weighted_wire_length_nm: planning_wirelength(scene, placements)?,
        local_connection_length_nm: scene.local_connection_length(placements, &scene.relative)?,
    })
}

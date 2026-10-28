//! Package-boundary surface escapes (`pcbir.boundary_access`): the collar
//! and the surface escapes of pins whose net may not change layer.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::geometry::{Bounds, bounds};
use copper_physical::{CopperLayer, Point};

use crate::escape_assignment::EscapeAssignmentOptions;
use crate::pin_escape::checked_access_paths;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::track_inside_board;
use crate::{Error, Result};

/// `BoundaryAccessOptions`.
#[derive(Clone, Debug)]
pub struct BoundaryAccessOptions {
    pub collar_margin_nm: i64,
    pub port_step_nm: i64,
    pub refinement_step_nm: i64,
    pub maximum_ports_per_edge: i64,
    pub maximum_refined_ports_per_edge: i64,
    pub maximum_detour_nm: i64,
    pub maze_escapes: bool,
    pub maze_state_budget: i64,
    pub assignment_options: EscapeAssignmentOptions,
    pub destination_ports: bool,
}

impl Default for BoundaryAccessOptions {
    fn default() -> Self {
        BoundaryAccessOptions {
            collar_margin_nm: 250_000,
            port_step_nm: 500_000,
            refinement_step_nm: 100_000,
            maximum_ports_per_edge: 8,
            maximum_refined_ports_per_edge: 32,
            maximum_detour_nm: 2_000_000,
            maze_escapes: true,
            maze_state_budget: 12_000,
            assignment_options: EscapeAssignmentOptions::default(),
            destination_ports: false,
        }
    }
}

/// `package_collar(...).bounds`: courtyard (or body) and pads, grown by the margin.
pub fn package_collar(board: &PhysicalBoard, reference: &str, margin_nm: i64) -> Result<Bounds> {
    if margin_nm < 0 {
        return Err(Error::Value("package collar margin cannot be negative".to_owned()));
    }
    let Some(pose) = board.placements.iter().find(|p| p.reference == reference) else {
        return Err(Error::Value(format!("unknown package collar component {}", copper_json::python_str_repr(reference))));
    };
    let footprint = &board.footprints[&pose.footprint];
    let local: Vec<Point> = if footprint.courtyard.is_empty() {
        let (w, h) = (footprint.body_size.width_nm, footprint.body_size.height_nm);
        let mut corners = Vec::new();
        for x in [(-w).div_euclid(2), w.div_euclid(2)] {
            for y in [(-h).div_euclid(2), h.div_euclid(2)] {
                corners.push(Point::new(x, y));
            }
        }
        corners
    } else {
        footprint.courtyard.clone()
    };
    let transformed: Vec<Point> = local.iter().map(|p| transformed_local_point(pose, *p)).collect();
    let mut regions = vec![bounds(&transformed)];
    for pad in &footprint.pads {
        regions.push(placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose).bounds);
    }
    let area = Bounds {
        min_x: regions.iter().map(|b| b.min_x).min().expect("regions"),
        min_y: regions.iter().map(|b| b.min_y).min().expect("regions"),
        max_x: regions.iter().map(|b| b.max_x).max().expect("regions"),
        max_y: regions.iter().map(|b| b.max_y).max().expect("regions"),
    };
    Ok(area.expanded(margin_nm))
}

/// `_ports`: collar-edge launch ports, the exact projection first.
fn ports(start: Point, area: Bounds, extension: i64, step: i64, limit: i64) -> Vec<Point> {
    let mut result = Vec::new();
    for edge in ["left", "right", "top", "bottom"] {
        let vertical = edge == "left" || edge == "right";
        let coordinate = if vertical { start.y_nm } else { start.x_nm };
        let (low, high) = if vertical { (area.min_y, area.max_y) } else { (area.min_x, area.max_x) };
        let coordinate = coordinate.min(high).max(low);
        let mut offsets = vec![0];
        for i in 1..limit {
            offsets.extend([-i * step, i * step]);
        }
        let mut values: IndexSet<i64> = IndexSet::new();
        for d in offsets {
            values.insert((coordinate + d).min(high).max(low));
        }
        for value in values.into_iter().take(limit as usize) {
            result.push(match edge {
                "left" => Point::new(area.min_x.min(start.x_nm) - extension, value),
                "right" => Point::new(area.max_x.max(start.x_nm) + extension, value),
                "top" => Point::new(value, area.min_y.min(start.y_nm) - extension),
                _ => Point::new(value, area.max_y.max(start.y_nm) + extension),
            });
        }
    }
    result
}

fn integer_sqrt(value: i128) -> i128 {
    let mut root = (value as f64).sqrt() as i128;
    while root * root > value {
        root -= 1;
    }
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    root
}

/// `_maze_path`: a bounded octilinear surface path out of the collar.
#[allow(clippy::too_many_arguments)]
fn maze_path(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    width: i64,
    layer: CopperLayer,
    area: Bounds,
    extension: i64,
    options: &BoundaryAccessOptions,
) -> Result<Option<Vec<TrackSegment>>> {
    let step = options.refinement_step_nm;
    let goal = Bounds {
        min_x: area.min_x.min(start.x_nm) - extension,
        min_y: area.min_y.min(start.y_nm) - extension,
        max_x: area.max_x.max(start.x_nm) + extension,
        max_y: area.max_y.max(start.y_nm) + extension,
    };
    let search = goal.expanded(options.maximum_detour_nm);
    let distances = [
        ("left", start.x_nm - goal.min_x),
        ("right", goal.max_x - start.x_nm),
        ("top", start.y_nm - goal.min_y),
        ("bottom", goal.max_y - start.y_nm),
    ];
    let maximum_length = distances.iter().map(|(_, d)| *d).min().expect("edges") + options.maximum_detour_nm;
    let eligible: Vec<&str> = distances.iter().filter(|(_, d)| *d <= maximum_length).map(|(e, _)| *e).collect();
    const DIRECTIONS: [(i64, i64); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
    let at = |x: i64, y: i64| Point::new(start.x_nm + x * step, start.y_nm + y * step);
    let heuristic = |p: Point| -> i64 {
        let remaining = |edge: &str| match edge {
            "left" => p.x_nm - goal.min_x,
            "right" => goal.max_x - p.x_nm,
            "top" => p.y_nm - goal.min_y,
            _ => goal.max_y - p.y_nm,
        };
        let distance = eligible.iter().map(|e| remaining(e)).min().unwrap_or(0).max(0);
        10 * (distance + step - 1).div_euclid(step)
    };
    let mut queue = BinaryHeap::from([Reverse((heuristic(start), 0i64, 0i64, 0i64, -1i64))]);
    let mut best: IndexMap<(i64, i64, i64), i64> = IndexMap::from_iter([((0, 0, -1), 0)]);
    let mut parent: IndexMap<(i64, i64, i64), Option<(i64, i64, i64)>> = IndexMap::from_iter([((0, 0, -1), None)]);
    let mut lengths: IndexMap<(i64, i64, i64), i64> = IndexMap::from_iter([((0, 0, -1), 0)]);
    let squared = 2 * i128::from(step) * i128::from(step);
    let mut diagonal = integer_sqrt(squared) as i64;
    if i128::from(diagonal) * i128::from(diagonal) < squared {
        diagonal += 1;
    }
    let mut legal_edges: IndexMap<((i64, i64), (i64, i64)), bool> = IndexMap::new();
    let mut states = 0;
    while states < options.maze_state_budget {
        let Some(Reverse((_, cost, x, y, heading))) = queue.pop() else { break };
        let state = (x, y, heading);
        if best.get(&state) != Some(&cost) {
            continue;
        }
        states += 1;
        let p = at(x, y);
        let reached = [
            ("left", p.x_nm <= goal.min_x),
            ("right", p.x_nm >= goal.max_x),
            ("top", p.y_nm <= goal.min_y),
            ("bottom", p.y_nm >= goal.max_y),
        ]
        .iter()
        .any(|(edge, hit)| *hit && eligible.contains(edge));
        if reached && (x != 0 || y != 0) {
            let mut nodes = Vec::new();
            let mut cursor = Some(state);
            while let Some(current) = cursor {
                nodes.push(at(current.0, current.1));
                cursor = parent[&current];
            }
            nodes.reverse();
            let mut turns = vec![nodes[0]];
            for i in 1..nodes.len() - 1 {
                let (a, b, c) = (nodes[i - 1], nodes[i], nodes[i + 1]);
                if (b.x_nm - a.x_nm) * (c.y_nm - b.y_nm) != (b.y_nm - a.y_nm) * (c.x_nm - b.x_nm) {
                    turns.push(b);
                }
            }
            turns.push(nodes[nodes.len() - 1]);
            let path = turns
                .windows(2)
                .map(|w| TrackSegment { net: net.to_owned(), start: w[0], end: w[1], width_nm: width, layer })
                .collect();
            return Ok(Some(path));
        }
        for (direction, (dx, dy)) in DIRECTIONS.iter().enumerate() {
            let direction = direction as i64;
            if heading != -1 && ![0, 1, 7].contains(&(direction - heading).rem_euclid(8)) {
                continue;
            }
            let q = at(x + dx, y + dy);
            if !(search.min_x <= q.x_nm && q.x_nm <= search.max_x && search.min_y <= q.y_nm && q.y_nm <= search.max_y) {
                continue;
            }
            let ends = ((x, y), (x + dx, y + dy));
            let key = if ends.0 <= ends.1 { ends } else { (ends.1, ends.0) };
            let legal = match legal_edges.get(&key) {
                Some(legal) => *legal,
                None => {
                    let pieces = clearance.route_pieces(net, p, q, width, layer)?;
                    let mut legal = !pieces.is_empty();
                    for t in &pieces {
                        if !legal {
                            break;
                        }
                        legal = track_inside_board(board, t.start, t.end, t.width_nm)?
                            && clearance.can_track(net, t.start, t.end, t.width_nm, layer)?;
                    }
                    legal_edges.insert(key, legal);
                    legal
                }
            };
            if !legal {
                continue;
            }
            let next_state = (x + dx, y + dy, direction);
            let next_length = lengths[&state] + if *dx != 0 && *dy != 0 { diagonal } else { step };
            if next_length > maximum_length {
                continue;
            }
            let next_cost =
                cost + if *dx != 0 && *dy != 0 { 14 } else { 10 } + if heading != -1 && heading != direction { 3 } else { 0 };
            if next_cost < *best.get(&next_state).unwrap_or(&i64::MAX) {
                best.insert(next_state, next_cost);
                parent.insert(next_state, Some(state));
                lengths.insert(next_state, next_length);
                queue.push(Reverse((next_cost + heuristic(q), next_cost, x + dx, y + dy, direction)));
            }
        }
    }
    Ok(None)
}

/// `surface_escapes`: surface escapes beyond the collar, shortest first.
#[allow(clippy::too_many_arguments)]
pub fn surface_escapes(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    layer: CopperLayer,
    area: Bounds,
    options: &BoundaryAccessOptions,
    refined: bool,
) -> Result<Vec<Vec<TrackSegment>>> {
    let rule = board.net_routing_rules.iter().find(|r| r.net == net);
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let rule_clearance = rule.and_then(|r| r.clearance_nm).unwrap_or(0);
    let extension = options
        .collar_margin_nm
        .max((width + 1).div_euclid(2) + board.rules.minimum_clearance_nm.max(rule_clearance));
    let (step, limit) = if refined {
        (options.refinement_step_nm, options.maximum_refined_ports_per_edge)
    } else {
        (options.port_step_nm, options.maximum_ports_per_edge)
    };
    let endpoints = ports(start, area, extension, step, limit);
    let manhattan = |p: Point| (p.x_nm - start.x_nm).abs() + (p.y_nm - start.y_nm).abs();
    let nearest = endpoints.iter().map(|p| manhattan(*p)).min().expect("ports");
    let mut paths: Vec<Vec<TrackSegment>> = Vec::new();
    for end in &endpoints {
        if manhattan(*end) <= nearest + options.maximum_detour_nm {
            for path in checked_access_paths(board, clearance, net, start, *end, width, layer, false, false, None, false)? {
                if !path.is_empty() {
                    paths.push(path);
                }
            }
        }
    }
    if refined && options.maze_escapes {
        if let Some(found) = maze_path(board, clearance, net, start, width, layer, area, extension, options)? {
            let mut pieces = Vec::new();
            for track in &found {
                pieces.extend(clearance.route_pieces(net, track.start, track.end, width, layer)?);
            }
            let mut legal = true;
            for t in &pieces {
                if !track_inside_board(board, t.start, t.end, t.width_nm)?
                    || !clearance.can_track(net, t.start, t.end, t.width_nm, layer)?
                {
                    legal = false;
                    break;
                }
            }
            if legal {
                paths.push(pieces);
            }
        }
    }
    let mut unique: Vec<Vec<TrackSegment>> = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique.sort_by_key(|path| {
        let length: i64 =
            path.iter().map(|t| (t.end.x_nm - t.start.x_nm).abs().max((t.end.y_nm - t.start.y_nm).abs())).sum();
        let last = path[path.len() - 1].end;
        (length, path.len(), last.x_nm, last.y_nm)
    });
    Ok(unique)
}

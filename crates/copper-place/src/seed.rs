//! Deterministic seeds and the bounded analytical pass (`_initial_seed`,
//! `_satellite_anchors`, `_spiral_offset`, `_seed_transform`,
//! `_analytical_place`, `_project_coordinate`, `_choose_orientations`).
//!
//! The analytical pass uses floats exactly as the Python code does: the same
//! operations, in the same accumulation order.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::Result;
use copper_pcb::board::PlacementGroup;
use copper_physical::Point;
use copper_physical::pyfloat::{round_int, sum};

use crate::escape::EscapeSpacingModel;
use crate::metrics::fast_score;
use crate::options::PlannerOptions;
use crate::power::power_domain_gradient;
use crate::scene::{Poses, Scene, moved, point_bounds};

/// Python's `max(a, b)` for floats: `a` unless `b` is larger.
pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// Python's `min(a, b)` for floats: `a` unless `b` is smaller.
pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// Python's `int / int`: the correctly rounded quotient.
pub fn true_div(numerator: i128, denominator: i128) -> f64 {
    const EXACT: i128 = 1 << 53;
    if numerator.abs() < EXACT && denominator.abs() < EXACT {
        numerator as f64 / denominator as f64
    } else {
        copper_physical::pyfloat::big_ratio_to_f64(&numerator.into(), &denominator.into())
    }
}

/// `_spiral_offset`: square-spiral lattice offsets around the origin.
pub fn spiral_offset(index: i64) -> (i64, i64) {
    if index == 0 {
        return (0, 0);
    }
    let mut layer = 1;
    while (2 * layer + 1) * (2 * layer + 1) <= index {
        layer += 1;
    }
    let side = 2 * layer;
    let offset = index - (2 * layer - 1) * (2 * layer - 1);
    let (segment, position) = (offset.div_euclid(side), offset.rem_euclid(side));
    match segment {
        0 => (layer, -layer + position),
        1 => (layer - position, layer),
        2 => (-layer, layer - position),
        _ => (-layer + position, -layer),
    }
}

fn seed_transform(point: Point, bounds: (i64, i64, i64, i64), seed: i64, index: i64) -> Point {
    let (min_x, min_y, max_x, max_y) = bounds;
    let point = match seed.rem_euclid(4) {
        1 => Point::new(max_x - (point.x_nm - min_x), point.y_nm),
        2 => Point::new(point.x_nm, max_y - (point.y_nm - min_y)),
        3 => Point::new(
            min_x
                + (i128::from(point.y_nm - min_y) * i128::from(max_x - min_x))
                    .div_euclid(i128::from(1.max(max_y - min_y))) as i64,
            min_y
                + (i128::from(point.x_nm - min_x) * i128::from(max_y - min_y))
                    .div_euclid(i128::from(1.max(max_x - min_x))) as i64,
        ),
        _ => point,
    };
    let jitter = ((index * 17 + seed * 31).rem_euclid(5) - 2) * 500_000;
    Point::new(point.x_nm + jitter, point.y_nm - jitter)
}

/// `_satellite_anchors`: seed ungrouped movable parts at the mean of their connected pads.
fn satellite_anchors(scene: &Scene, fixed: &Poses, movable: &[String]) -> Result<IndexMap<String, Point>> {
    let board = scene.board;
    let mut excluded: IndexSet<&str> = scene.plane_nets.iter().map(String::as_str).collect();
    excluded.extend(board.power_domains.iter().map(|d| d.net.as_str()));
    excluded.extend(board.decoupling_links.iter().map(|l| l.return_net.as_str()));
    let movable: Vec<&String> = movable.iter().filter(|r| !fixed.contains_key(*r)).collect();
    let mut located: IndexMap<String, Point> = fixed.iter().map(|(r, p)| (r.clone(), p.position)).collect();
    let mut nets_of: IndexMap<&str, Vec<&copper_pcb::board::PhysicalNet>> = IndexMap::new();
    for net in &board.nets {
        if excluded.contains(net.name.as_str()) || net.pads.len() > 12 {
            continue;
        }
        for pad in &net.pads {
            nets_of.entry(pad.component.as_str()).or_default().push(net);
        }
    }
    let mut anchors: IndexMap<String, Point> = IndexMap::new();
    for _ in 0..2 {
        for reference in &movable {
            let mut points: Vec<Point> = Vec::new();
            for net in nets_of.get(reference.as_str()).map(Vec::as_slice).unwrap_or(&[]) {
                for pad in &net.pads {
                    if pad.component == **reference || !located.contains_key(&pad.component) {
                        continue;
                    }
                    if let Some(pose) = fixed.get(&pad.component) {
                        points.push(scene.pad_position(pose, &pad.pad)?);
                    } else {
                        points.push(located[&pad.component]);
                    }
                }
            }
            if !points.is_empty() {
                let count = points.len() as i128;
                let x: i128 = points.iter().map(|p| i128::from(p.x_nm)).sum();
                let y: i128 = points.iter().map(|p| i128::from(p.y_nm)).sum();
                anchors.insert(
                    (*reference).clone(),
                    Point::new(round_int(true_div(x, count)), round_int(true_div(y, count))),
                );
            }
        }
        for (reference, point) in &anchors {
            located.insert(reference.clone(), *point);
        }
    }
    Ok(anchors)
}

/// `_initial_seed`: fixed poses, group spirals, satellite anchors and transformed prototype positions.
pub fn initial_seed(scene: &Scene, source: &Poses, options: &PlannerOptions, seed: i64) -> Result<Poses> {
    let board = scene.board;
    let bounds = point_bounds(&board.outline.vertices);
    let fixed = scene.fixed_placements(source, &options.fixed_references);
    let mut result = fixed.clone();
    let mut sorted_groups: Vec<&PlacementGroup> = board.placement_groups.iter().collect();
    sorted_groups.sort_by(|a, b| (-a.priority, &a.name).cmp(&(-b.priority, &b.name)));
    let mut primary: IndexMap<&str, &PlacementGroup> = IndexMap::new();
    for group in &sorted_groups {
        for reference in &group.references {
            primary.entry(reference.as_str()).or_insert(group);
        }
    }
    let mut by_name: IndexMap<&str, &PlacementGroup> = IndexMap::new();
    for group in primary.values() {
        by_name.insert(group.name.as_str(), group);
    }
    let mut groups: Vec<&PlacementGroup> = by_name.into_values().collect();
    groups.sort_by(|a, b| (-a.priority, &a.name).cmp(&(-b.priority, &b.name)));
    let center = ((bounds.0 + bounds.2).div_euclid(2), (bounds.1 + bounds.3).div_euclid(2));
    let group_step = (options.congestion_bin_nm * 2).max(10_000_000);
    let mut centers: IndexMap<&str, Point> = IndexMap::new();
    for (index, group) in groups.iter().enumerate() {
        let (dx, dy) = spiral_offset(index as i64 + seed);
        centers.insert(group.name.as_str(), Point::new(center.0 + dx * group_step, center.1 + dy * group_step));
    }
    let mut movable: Vec<String> = source.keys().filter(|r| !fixed.contains_key(*r)).cloned().collect();
    movable.sort_by(|a, b| {
        let key = |r: &String| {
            (-primary.get(r.as_str()).map_or(0, |g| g.priority), -scene.footprint_area(&source[r]), r.clone())
        };
        key(a).cmp(&key(b))
    });
    let anchors = if options.satellite_seeding { satellite_anchors(scene, &fixed, &movable)? } else { IndexMap::new() };
    let mut members_seen: IndexMap<&str, i64> = IndexMap::new();
    for (index, reference) in movable.iter().enumerate() {
        let original = &source[reference];
        let point = match primary.get(reference.as_str()) {
            Some(group) => {
                let member = members_seen.get(group.name.as_str()).copied().unwrap_or(0);
                members_seen.insert(group.name.as_str(), member + 1);
                let (dx, dy) = spiral_offset(member + seed);
                let target = centers[group.name.as_str()];
                Point::new(target.x_nm + dx * options.grid_step_nm * 2, target.y_nm + dy * options.grid_step_nm * 2)
            }
            None => match anchors.get(reference) {
                Some(anchor) => {
                    let (dx, dy) = spiral_offset(index as i64 + seed);
                    Point::new(anchor.x_nm + dx * options.grid_step_nm, anchor.y_nm + dy * options.grid_step_nm)
                }
                None => seed_transform(original.position, bounds, seed, index as i64),
            },
        };
        let orientations = scene.allowed_orientations(reference);
        let rotation = &orientations[(seed as usize + index) % orientations.len()];
        let mut pose = moved(original, point);
        pose.rotation_degrees = rotation.clone();
        result.insert(reference.clone(), pose);
    }
    Ok(result)
}

/// `_smooth_span_gradient`: the log-sum-exp span gradient.
fn smooth_span_gradient(values: &[f64], gamma: f64) -> Vec<f64> {
    let maximum = values.iter().copied().reduce(py_max).expect("values");
    let minimum = values.iter().copied().reduce(py_min).expect("values");
    let positive: Vec<f64> = values.iter().map(|v| ((v - maximum) / gamma).exp()).collect();
    let negative: Vec<f64> = values.iter().map(|v| ((minimum - v) / gamma).exp()).collect();
    let (sum_positive, sum_negative) = (sum(positive.iter().copied()), sum(negative.iter().copied()));
    positive.iter().zip(&negative).map(|(p, n)| p / sum_positive - n / sum_negative).collect()
}

/// `_project_coordinate`: clamp a centre inside the board (or its region) by its half extents.
fn project_coordinate(
    scene: &Scene,
    pose: &copper_pcb::board::Placement,
    coordinate: &mut [f64; 2],
    bounds: (i64, i64, i64, i64),
    options: &PlannerOptions,
) {
    let (half_width, half_height) = scene.half_extents_mm(pose);
    let mm = |value: i64| value as f64 / 1_000_000.0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (mm(bounds.0), mm(bounds.1), mm(bounds.2), mm(bounds.3));
    let mut edge = options.edge_clearance_nm as f64 / 1_000_000.0;
    if let Some(name) = scene.rule(&pose.reference).and_then(|r| r.region.as_ref()) {
        let region = scene.board.regions.iter().find(|r| r.name == *name).expect("validated region");
        let region_bounds = point_bounds(&region.outline.vertices);
        (min_x, min_y, max_x, max_y) =
            (mm(region_bounds.0), mm(region_bounds.1), mm(region_bounds.2), mm(region_bounds.3));
        edge = 0.0; // Region boundaries contain courtyards; they are not board edges.
    }
    coordinate[0] = py_min(max_x - edge - half_width, py_max(min_x + edge + half_width, coordinate[0]));
    coordinate[1] = py_min(max_y - edge - half_height, py_max(min_y + edge + half_height, coordinate[1]));
}

/// `_analytical_place`: momentum descent on wirelength, overlap, domain, escape and group forces.
pub fn analytical_place(
    scene: &Scene,
    source: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
) -> Result<Poses> {
    if source.is_empty() || options.analytical_iterations == 0 {
        return Ok(source.clone());
    }
    let board = scene.board;
    let fixed: IndexSet<String> = scene.fixed_placements(source, &options.fixed_references).into_keys().collect();
    let mut coordinates: IndexMap<String, [f64; 2]> = source
        .iter()
        .map(|(r, p)| (r.clone(), [p.position.x_nm as f64 / 1_000_000.0, p.position.y_nm as f64 / 1_000_000.0]))
        .collect();
    let mut velocity: IndexMap<String, [f64; 2]> = source.keys().map(|r| (r.clone(), [0.0, 0.0])).collect();
    let gamma = py_max(1.0, options.congestion_bin_nm as f64 / 1_000_000.0);
    let bounds = point_bounds(&board.outline.vertices);
    let mut references: Vec<&String> = source.keys().collect();
    references.sort();
    let extents: IndexMap<&str, (f64, f64)> =
        references.iter().map(|r| (r.as_str(), scene.half_extents_mm(&source[*r]))).collect();
    let mut gaps: IndexMap<(usize, usize), f64> = IndexMap::new();
    for (index, left) in references.iter().enumerate() {
        for (offset, right) in references[index + 1..].iter().enumerate() {
            if source[*left].side == source[*right].side {
                let gap = scene.courtyard_gap(left, right, options.component_clearance_nm)?;
                gaps.insert((index, index + 1 + offset), gap as f64 / 1_000_000.0);
            }
        }
    }
    let mut offsets: IndexMap<(String, String), (f64, f64)> = IndexMap::new();
    for net in &board.nets {
        for pad in net.pads.iter().filter(|p| source.contains_key(&p.component)) {
            let offset = scene.pad_offset_mm(&source[&pad.component], &pad.pad);
            offsets.insert((pad.component.clone(), pad.pad.clone()), offset);
        }
    }
    for _ in 0..options.analytical_iterations {
        let mut gradient: IndexMap<String, [f64; 2]> = source.keys().map(|r| (r.clone(), [0.0, 0.0])).collect();
        for net in &board.nets {
            let pads: Vec<&copper_pcb::board::PadReference> =
                net.pads.iter().filter(|p| source.contains_key(&p.component)).collect();
            if pads.len() < 2 {
                continue;
            }
            let mut xs = Vec::with_capacity(pads.len());
            let mut ys = Vec::with_capacity(pads.len());
            for pad in &pads {
                let offset = offsets[&(pad.component.clone(), pad.pad.clone())];
                let coordinate = coordinates[&pad.component];
                xs.push(coordinate[0] + offset.0);
                ys.push(coordinate[1] + offset.1);
            }
            let (gx, gy) = (smooth_span_gradient(&xs, gamma), smooth_span_gradient(&ys, gamma));
            let weight = if scene.plane_nets.contains(&net.name) { 0.25 } else { 1.0 };
            for ((pad, dx), dy) in pads.iter().zip(&gx).zip(&gy) {
                let entry = &mut gradient[&pad.component];
                entry[0] += dx * weight;
                entry[1] += dy * weight;
            }
        }
        for (index, left) in references.iter().enumerate() {
            let (left_width, left_height) = extents[left.as_str()];
            for (offset, right) in references[index + 1..].iter().enumerate() {
                let Some(clearance) = gaps.get(&(index, index + 1 + offset)).copied() else { continue };
                let (right_width, right_height) = extents[right.as_str()];
                let dx = coordinates[*left][0] - coordinates[*right][0];
                let dy = coordinates[*left][1] - coordinates[*right][1];
                let overlap_x = left_width + right_width + clearance - dx.abs();
                let overlap_y = left_height + right_height + clearance - dy.abs();
                if overlap_x <= 0.0 || overlap_y <= 0.0 {
                    continue;
                }
                if overlap_x < overlap_y {
                    let force = py_min(4.0, overlap_x) * if dx >= 0.0 { 1.0 } else { -1.0 };
                    gradient[*left][0] -= force;
                    gradient[*right][0] += force;
                } else {
                    let force = py_min(4.0, overlap_y) * if dy >= 0.0 { 1.0 } else { -1.0 };
                    gradient[*left][1] -= force;
                    gradient[*right][1] += force;
                }
            }
        }
        let current: Poses = coordinates
            .iter()
            .map(|(r, xy)| {
                let point = Point::new(round_int(xy[0] * 1_000_000.0), round_int(xy[1] * 1_000_000.0));
                (r.clone(), moved(&source[r], point))
            })
            .collect();
        for (reference, force) in power_domain_gradient(scene, &current, options.power_domain_weight)? {
            gradient[&reference][0] += force[0];
            gradient[&reference][1] += force[1];
        }
        for channel in spacing.channels(&current, false) {
            let axis = if channel.axis == "x" { 0 } else { 1 };
            let delta = coordinates[&channel.left][axis] - coordinates[&channel.right][axis];
            let force = py_min(4.0, channel.deficit_nm() as f64 / 1_000_000.0) * if delta >= 0.0 { 1.0 } else { -1.0 };
            gradient[&channel.left][axis] -= force;
            gradient[&channel.right][axis] += force;
        }
        for group in &board.placement_groups {
            let members: Vec<&String> = group.references.iter().filter(|r| coordinates.contains_key(*r)).collect();
            if members.len() < 2 {
                continue;
            }
            let count = members.len() as f64;
            let center_x = sum(members.iter().map(|m| coordinates[*m][0])) / count;
            let center_y = sum(members.iter().map(|m| coordinates[*m][1])) / count;
            let strength = py_min(0.25, 0.01 + true_div(i128::from(group.priority.max(0)), 4000));
            for reference in &members {
                let coordinate = coordinates[*reference];
                let entry = &mut gradient[*reference];
                entry[0] += (coordinate[0] - center_x) * strength;
                entry[1] += (coordinate[1] - center_y) * strength;
            }
        }
        for reference in &references {
            if fixed.contains(*reference) {
                continue;
            }
            let push = gradient[*reference];
            let speed = &mut velocity[*reference];
            speed[0] = options.analytical_momentum * speed[0] - options.analytical_step_mm * push[0];
            speed[1] = options.analytical_momentum * speed[1] - options.analytical_step_mm * push[1];
            let step = *speed;
            let coordinate = &mut coordinates[*reference];
            coordinate[0] += step[0];
            coordinate[1] += step[1];
            let mut value = *coordinate;
            project_coordinate(scene, &source[*reference], &mut value, bounds, options);
            coordinates[*reference] = value;
        }
    }
    let result: Poses = source
        .iter()
        .map(|(r, pose)| {
            let xy = coordinates[r];
            (r.clone(), moved(pose, Point::new(round_int(xy[0] * 1_000_000.0), round_int(xy[1] * 1_000_000.0))))
        })
        .collect();
    choose_orientations(scene, &result, &fixed, spacing, options.power_domain_weight)
}

/// `_choose_orientations`: per component, the allowed angle with the best fast score.
pub fn choose_orientations(
    scene: &Scene,
    placements: &Poses,
    fixed: &IndexSet<String>,
    spacing: &EscapeSpacingModel,
    weight: f64,
) -> Result<Poses> {
    let mut result = placements.clone();
    let mut references: Vec<String> = result.keys().cloned().collect();
    references.sort();
    for reference in references {
        if fixed.contains(&reference) {
            continue;
        }
        let current = result[&reference].clone();
        let mut best = current.clone();
        let mut best_cost = fast_score(scene, &result, spacing, weight)?;
        for orientation in scene.allowed_orientations(&reference) {
            let mut candidate = current.clone();
            candidate.rotation_degrees = orientation.clone();
            result[&reference] = candidate.clone();
            let cost = fast_score(scene, &result, spacing, weight)?;
            if (cost, decimal_int(&orientation)) < (best_cost, decimal_int(&best.rotation_degrees)) {
                best = candidate;
                best_cost = cost;
            }
        }
        result[&reference] = best;
    }
    Ok(result)
}

/// Python's `int(Decimal)`: truncation toward zero.
pub fn decimal_int(value: &copper_units::Decimal) -> i64 {
    value.to_i64().expect("an angle")
}

//! Bounded legalization (`_legalize` and its helpers): candidate domains,
//! anchored companion packing with a bounded backtracking search, and local
//! exact repair. Every accepted pose passes the complete `_legal` gate.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{AlignmentAxis, Placement, PlacementTarget, RelativePlacementKind, RelativePlacementRule};
use copper_pcb::{Error, Result};
use copper_physical::footprint::PadKind;
use copper_physical::pyfloat::{hypot, round_int};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;

use crate::escape::{EscapeSpacingModel, placement_units};
use crate::options::PlannerOptions;
use crate::scene::{Poses, Rules, Scene, moved, point_bounds, posed, with};
use crate::seed::true_div;

/// `round(i * (n - 1) / max(1, count - 1))`: evenly spread sample indexes.
pub fn spread_index(i: usize, n: usize, count: usize) -> usize {
    round_int(true_div((i * (n - 1)) as i128, 1.max(count as i128 - 1))) as usize
}

/// `_adjacency`: connection weights between components.
pub fn adjacency(scene: &Scene) -> IndexMap<String, IndexMap<String, i64>> {
    let board = scene.board;
    let mut result: IndexMap<String, IndexMap<String, i64>> =
        board.placements.iter().map(|p| (p.reference.clone(), IndexMap::new())).collect();
    for net in &board.nets {
        let references: BTreeSet<&str> =
            net.pads.iter().map(|p| p.component.as_str()).filter(|c| result.contains_key(*c)).collect();
        if references.len() < 2 {
            continue;
        }
        let edges = (references.len() as i64 - 1).pow(2);
        let mut weight = 1i64.max(1000i64.div_euclid(edges));
        if scene.plane_nets.contains(&net.name) {
            weight = 1i64.max(weight.div_euclid(4));
        }
        let references: Vec<&str> = references.into_iter().collect();
        for (index, left) in references.iter().enumerate() {
            for right in &references[index + 1..] {
                *result[*left].entry((*right).to_owned()).or_insert(0) += weight;
                *result[*right].entry((*left).to_owned()).or_insert(0) += weight;
            }
        }
    }
    for group in &board.placement_groups {
        for reference in &group.references {
            for other in &group.references {
                if reference != other && result.contains_key(reference) && result.contains_key(other) {
                    *result[reference].entry(other.clone()).or_insert(0) += group.priority.max(1);
                }
            }
        }
    }
    result
}

/// `_component_priorities`.
fn component_priorities(scene: &Scene) -> IndexMap<String, i64> {
    let mut result: IndexMap<String, i64> =
        scene.board.placement_rules.iter().map(|r| (r.reference.clone(), r.priority)).collect();
    for group in &scene.board.placement_groups {
        for reference in &group.references {
            let value = result.get(reference).copied().unwrap_or(0).max(group.priority);
            result.insert(reference.clone(), value);
        }
    }
    result
}

/// `_incremental_cost`: adjacency-weighted L1 distance to placed neighbours.
fn incremental_cost(
    reference: &str,
    candidate: &Placement,
    placed: &Poses,
    adjacency: &IndexMap<String, IndexMap<String, i64>>,
) -> i64 {
    let Some(neighbours) = adjacency.get(reference) else { return 0 };
    neighbours
        .iter()
        .filter_map(|(other, weight)| {
            placed.get(other).map(|pose| {
                ((candidate.position.x_nm - pose.position.x_nm).abs()
                    + (candidate.position.y_nm - pose.position.y_nm).abs())
                    * weight
            })
        })
        .sum()
}

/// `_relative_cluster_order`: schedule flexible companions together, including paths through fixed ICs.
pub fn relative_cluster_order(scene: &Scene, movable: &[String], targets: &Poses) -> Vec<String> {
    let units = placement_units(scene.board, &scene.relative);
    let movable_set: IndexSet<&str> = movable.iter().map(String::as_str).collect();
    let order: IndexMap<&str, usize> = movable.iter().enumerate().map(|(i, r)| (r.as_str(), i)).collect();
    let area = |reference: &str| scene.footprint_area(&targets[reference]);
    let mut groups: Vec<&BTreeSet<String>> = Vec::new();
    for reference in movable {
        let unit = &units[reference];
        if unit.len() > 1 && !groups.iter().any(|g| std::ptr::eq(*g, &**unit)) {
            groups.push(unit);
        }
    }
    groups.sort_by_key(|group| {
        let outside = group.iter().any(|r| !movable_set.contains(r.as_str()));
        let largest = group.iter().map(|r| area(r)).max().expect("members");
        let first = group.iter().filter_map(|r| order.get(r.as_str())).min().copied().expect("movable member");
        (!outside, -largest, first)
    });
    let mut result: Vec<String> = Vec::new();
    for group in groups {
        let mut pending: Vec<&String> = group.iter().filter(|r| movable_set.contains(r.as_str())).collect();
        let mut anchors: IndexSet<&str> =
            group.iter().filter(|r| !movable_set.contains(r.as_str())).map(String::as_str).collect();
        if anchors.is_empty() {
            let anchor = *pending.iter().min_by_key(|r| (-area(r), order[r.as_str()])).expect("pending");
            result.push(anchor.clone());
            pending.retain(|r| *r != anchor);
            anchors.insert(anchor);
        }
        while !pending.is_empty() {
            let difficulty = |reference: &String| {
                let limits: Vec<i64> = scene
                    .relative
                    .iter()
                    .filter(|rule| {
                        rule.kind == RelativePlacementKind::MaxDistance
                            && rule.targets.iter().any(|t| t.reference == *reference)
                            && rule.targets.iter().any(|t| anchors.contains(t.reference.as_str()))
                    })
                    .map(|rule| rule.distance_nm.expect("a maximum distance"))
                    .collect();
                (
                    limits.is_empty(),
                    limits.iter().copied().min().map_or(i128::from(i64::MAX) + 1, i128::from),
                    -(limits.len() as i64),
                    -area(reference),
                    reference.clone(),
                )
            };
            let companion = *pending.iter().min_by_key(|r| difficulty(r)).expect("pending");
            result.push(companion.clone());
            pending.retain(|r| *r != companion);
            anchors.insert(companion);
        }
    }
    let scheduled: IndexSet<String> = result.iter().cloned().collect();
    result.extend(movable.iter().filter(|r| !scheduled.contains(*r)).cloned());
    result
}

/// `_local_obstacles`: local rule anchors first, then the rest in their held order.
pub fn local_obstacles(scene: &Scene, reference: &str, placed: &Poses) -> Poses {
    let anchors: IndexSet<&str> = scene
        .relative
        .iter()
        .filter(|rule| rule.targets.iter().any(|t| t.reference == reference))
        .flat_map(|rule| rule.targets.iter().map(|t| t.reference.as_str()))
        .collect();
    let mut result: Poses =
        placed.iter().filter(|(r, _)| anchors.contains(r.as_str())).map(|(r, p)| (r.clone(), p.clone())).collect();
    result.extend(placed.iter().filter(|(r, _)| !anchors.contains(r.as_str())).map(|(r, p)| (r.clone(), p.clone())));
    result
}

/// A boxed, lazy candidate stream.
pub type Candidates<'c> = Box<dyn Iterator<Item = Placement> + 'c>;

/// `_bounded_local_candidates`: keep outer-row poses within the probe budget,
/// reserving a bounded ordinary-grid fallback.
pub fn bounded_local_candidates<'c>(
    candidates: Vec<Placement>,
    limit: usize,
    mut fallback: Candidates<'c>,
) -> Candidates<'c> {
    if candidates.len() >= limit {
        let reserved: Vec<Placement> = fallback.by_ref().take(limit / 4).collect();
        let count = limit - reserved.len();
        let n = candidates.len();
        let sampled: Vec<Placement> = (0..count).map(|i| candidates[spread_index(i, n, count)].clone()).collect();
        return Box::new(sampled.into_iter().chain(reserved));
    }
    Box::new(candidates.into_iter().chain(fallback))
}

/// Ordinary grid poses at each position and angle, computed on first use
/// (`replace(target, position=point, rotation_degrees=angle) for point ... for angle ...`).
pub fn general_candidates<'c>(
    scene: &'c Scene<'c>,
    target: Placement,
    options: &'c PlannerOptions,
    orientations: Vec<Decimal>,
) -> Candidates<'c> {
    let mut positions: Option<std::vec::IntoIter<Point>> = None;
    let mut current: Option<Point> = None;
    let mut angle = 0;
    Box::new(std::iter::from_fn(move || {
        if orientations.is_empty() {
            return None;
        }
        if current.is_none() || angle == orientations.len() {
            let points = positions.get_or_insert_with(|| candidate_positions(scene, &target, options).into_iter());
            current = Some(points.next()?);
            angle = 0;
        }
        let point = current.expect("position");
        let pose = posed(&target, point, &orientations[angle]);
        angle += 1;
        Some(pose)
    }))
}

/// `_candidate_positions`: the ordinary grid (board or region), nearest first,
/// plus exact region boundary centres within the unused allowance.
pub fn candidate_positions(scene: &Scene, placement: &Placement, options: &PlannerOptions) -> Vec<Point> {
    let board = scene.board;
    let mut bounds = point_bounds(&board.outline.vertices);
    let rule = scene.rule(&placement.reference);
    let mut edge = options.edge_clearance_nm;
    let region = rule
        .and_then(|r| r.region.as_ref())
        .map(|name| board.regions.iter().find(|r| r.name == *name).expect("validated region"));
    if let Some(region) = region {
        bounds = point_bounds(&region.outline.vertices);
        edge = 0; // _legal still enforces the independent physical board edge.
    }
    let (min_x, min_y, max_x, max_y) = bounds;
    let orientations = scene.allowed_orientations(&placement.reference);
    let (mut max_half_width, mut max_half_height) = (0, 0);
    for orientation in &orientations {
        let candidate = Placement { rotation_degrees: orientation.clone(), ..placement.clone() };
        let (w, h) = scene.half_extents(&candidate);
        max_half_width = max_half_width.max(w);
        max_half_height = max_half_height.max(h);
    }
    let step = options.grid_step_nm;
    let ceil_grid = |value: i64| (value + step - 1).div_euclid(step) * step;
    let (first_x, first_y) = (ceil_grid(min_x + edge + max_half_width), ceil_grid(min_y + edge + max_half_height));
    let (last_x, last_y) = (max_x - edge - max_half_width, max_y - edge - max_half_height);
    let mut coarse: Vec<Point> = Vec::new();
    let mut y = first_y;
    while y <= last_y {
        let mut x = first_x;
        while x <= last_x {
            coarse.push(Point::new(x, y));
            x += step;
        }
        y += step;
    }
    let (px, py) = (placement.position.x_nm, placement.position.y_nm);
    coarse.sort_by_key(|p| ((p.x_nm - px).abs() + (p.y_nm - py).abs(), p.y_nm, p.x_nm));
    if region.is_none() {
        return coarse;
    }
    // Small hard regions can have a legal centre domain between lattice sites;
    // spend only unused ordinary allowance on exact boundary/midpoint/target centres.
    let allowance =
        0i64.max(options.legalization_candidates.div_euclid(orientations.len() as i64) - coarse.len() as i64) as usize;
    if allowance == 0 {
        return coarse;
    }
    let mut seen: IndexSet<Point> = coarse.iter().copied().collect();
    let mut extra: Vec<Point> = Vec::new();
    for orientation in &orientations {
        let (half_width, half_height) =
            scene.half_extents(&Placement { rotation_degrees: orientation.clone(), ..placement.clone() });
        let (low_x, high_x) = (min_x + edge + half_width, max_x - edge - half_width);
        let (low_y, high_y) = (min_y + edge + half_height, max_y - edge - half_height);
        if low_x > high_x || low_y > high_y {
            continue;
        }
        let xs = [(low_x + high_x).div_euclid(2), low_x, high_x];
        let ys = [(low_y + high_y).div_euclid(2), low_y, high_y];
        let mut proposals = vec![Point::new(px.max(low_x).min(high_x), py.max(low_y).min(high_y))];
        for y in ys {
            for x in xs {
                proposals.push(Point::new(x, y));
            }
        }
        for point in proposals {
            if !seen.insert(point) {
                continue;
            }
            extra.push(point);
            if extra.len() == allowance {
                coarse.extend(extra);
                return coarse;
            }
        }
    }
    coarse.extend(extra);
    coarse
}

const EIGHT: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

/// `_relative_candidates`: both permitted surfaces for pin-associated SMD decouplers.
pub fn relative_candidates(
    scene: &Scene,
    rules: &Rules,
    reference: &str,
    placement: &Placement,
    placed: &Poses,
    options: &PlannerOptions,
    step_nm: i64,
) -> Result<Vec<Placement>> {
    let board = scene.board;
    let primary = relative_candidates_on_side(scene, rules, reference, placement, placed, options, step_nm)?;
    let rule = scene.rule(reference);
    let links: Vec<&copper_pcb::board::DecouplingLink> =
        board.decoupling_links.iter().filter(|l| l.capacitor.component == reference).collect();
    if options.fixed_references.contains(reference)
        || rule.is_some_and(|r| r.side.is_some() || r.fixed_position.is_some())
        || links.is_empty()
        || board.footprints[&placement.footprint].pads.iter().any(|p| p.kind != PadKind::Smd)
        || board.rigid_clusters.iter().any(|c| c.members.iter().any(|m| m.reference == reference))
    {
        return Ok(primary);
    }
    let opposite = if placement.side == BoardSide::Front { BoardSide::Back } else { BoardSide::Front };
    let layer = if opposite == BoardSide::Back { CopperLayer::Back } else { CopperLayer::Front };
    let routing: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    for link in &links {
        for net in [&link.net, &link.return_net] {
            let route = routing.get(net.as_str()).copied();
            if route.is_some_and(|r| r.max_vias == Some(0))
                || !copper_pcb::zones::routing_layers(board, net, route).contains(&layer)
            {
                return Ok(primary);
            }
        }
    }
    let flipped = Placement { side: opposite, ..placement.clone() };
    let alternate = relative_candidates_on_side(scene, rules, reference, &flipped, placed, options, step_nm)?;
    let mut result = Vec::with_capacity(primary.len() + alternate.len());
    for i in 0..primary.len().max(alternate.len()) {
        for domain in [&primary, &alternate] {
            if let Some(pose) = domain.get(i) {
                result.push(pose.clone());
            }
        }
    }
    Ok(result)
}

/// `_relative_candidates_on_side`: bounded local XY search with orientation-specific origins.
fn relative_candidates_on_side(
    scene: &Scene,
    rules: &Rules,
    reference: &str,
    placement: &Placement,
    placed: &Poses,
    options: &PlannerOptions,
    step_nm: i64,
) -> Result<Vec<Placement>> {
    let grid = options.grid_step_nm;
    let step = grid.min(step_nm);
    let mut result: IndexMap<Placement, i128> = IndexMap::new();
    for rule in rules {
        let Some(own_index) = rule.targets.iter().position(|t| t.reference == reference) else { continue };
        let own = &rule.targets[own_index];
        let companions: Vec<&PlacementTarget> = if rule.kind == RelativePlacementKind::Align {
            rule.targets.iter().collect()
        } else {
            rule.targets[own_index.saturating_sub(1)..own_index]
                .iter()
                .chain(rule.targets.iter().skip(own_index + 1).take(1))
                .collect()
        };
        for companion in companions {
            if companion.reference == reference || !placed.contains_key(&companion.reference) {
                continue;
            }
            let anchor = scene.target_point(placed, companion)?.expect("placed companion");
            for angle in scene.allowed_orientations(reference) {
                let pose = posed(placement, Point::new(0, 0), &angle);
                let mut alone = Poses::new();
                alone.insert(reference.to_owned(), pose.clone());
                let offset = scene.target_point(&alone, own)?.expect("own target");
                let origin = Point::new(anchor.x_nm - offset.x_nm, anchor.y_nm - offset.y_nm);
                let mut points: IndexSet<Point> = IndexSet::new();
                let (ox, oy) = (origin.x_nm, origin.y_nm);
                match rule.kind {
                    RelativePlacementKind::MaxDistance if own.pad.is_none() || companion.pad.is_none() => {
                        let radius = 1i64.max(rule.distance_nm.unwrap_or(0).div_euclid(grid));
                        for distance in 0..=radius {
                            for (dx, dy) in [
                                (distance, 0),
                                (-distance, 0),
                                (0, distance),
                                (0, -distance),
                                (distance, distance),
                                (distance, -distance),
                                (-distance, distance),
                                (-distance, -distance),
                            ] {
                                let candidate = moved(&pose, Point::new(ox + dx * grid, oy + dy * grid));
                                let next = result.len() as i128;
                                result.entry(candidate).or_insert(next);
                            }
                        }
                        continue;
                    }
                    RelativePlacementKind::MaxDistance => {
                        let distance = rule.distance_nm.unwrap_or(0);
                        let squared = i128::from(distance) * i128::from(distance);
                        // Bound each neighbourhood independently of board size.
                        let radius = 24i64.min(distance.div_euclid(step));
                        for x in -radius..=radius {
                            for y in -radius..=radius {
                                let (sx, sy) = (i128::from(x * step), i128::from(y * step));
                                if sx * sx + sy * sy <= squared {
                                    points.insert(Point::new(ox + x * step, oy + y * step));
                                }
                            }
                        }
                        if step < 250_000 {
                            for x in -2..3 {
                                for y in -2..3 {
                                    points.insert(Point::new(
                                        placement.position.x_nm + x * step,
                                        placement.position.y_nm + y * step,
                                    ));
                                }
                            }
                        }
                        let own_bounds = point_bounds(&scene.polygon(&pose));
                        let mut xs: IndexSet<i64> = [ox].into_iter().collect();
                        let mut ys: IndexSet<i64> = [oy].into_iter().collect();
                        for other in placed.values() {
                            if other.side != placement.side {
                                continue;
                            }
                            let bounds = point_bounds(&scene.polygon(other));
                            let gap = scene.courtyard_gap(
                                &placement.reference,
                                &other.reference,
                                options.component_clearance_nm,
                            )?;
                            let edges_x = [bounds.0 - gap - own_bounds.2, bounds.2 + gap - own_bounds.0];
                            let edges_y = [bounds.1 - gap - own_bounds.3, bounds.3 + gap - own_bounds.1];
                            // Distant objects cannot bound this local neighbourhood.
                            if edges_x[1] < ox - distance
                                || edges_x[0] > ox + distance
                                || edges_y[1] < oy - distance
                                || edges_y[0] > oy + distance
                            {
                                continue;
                            }
                            for edge in edges_x {
                                for x in [edge - step, edge, edge + step] {
                                    if (x - ox).abs() <= distance {
                                        xs.insert(x);
                                    }
                                }
                            }
                            for edge in edges_y {
                                for y in [edge - step, edge, edge + step] {
                                    if (y - oy).abs() <= distance {
                                        ys.insert(y);
                                    }
                                }
                            }
                        }
                        let mut xs: Vec<i64> = xs.into_iter().collect();
                        xs.sort_by_key(|x| ((x - ox).abs(), *x));
                        xs.truncate(16);
                        let mut ys: Vec<i64> = ys.into_iter().collect();
                        ys.sort_by_key(|y| ((y - oy).abs(), *y));
                        ys.truncate(16);
                        for x in &xs {
                            for y in &ys {
                                let (dx, dy) = (i128::from(x - ox), i128::from(y - oy));
                                if dx * dx + dy * dy <= squared {
                                    points.insert(Point::new(*x, *y));
                                }
                            }
                        }
                        // Retain coarse reach for constraints wider than the local window.
                        let steps = 1i64.max(64i64.min(distance.div_euclid(grid)));
                        for i in 0..=steps {
                            let radius = (i * distance).div_euclid(steps);
                            for (dx, dy) in EIGHT {
                                points.insert(Point::new(ox + dx * radius, oy + dy * radius));
                            }
                        }
                    }
                    RelativePlacementKind::MinDistance => {
                        let distance = 1i64.max((rule.distance_nm.unwrap_or(0) + grid - 1).div_euclid(grid)) * grid;
                        for (dx, dy) in EIGHT {
                            points.insert(Point::new(ox + dx * distance, oy + dy * distance));
                        }
                    }
                    RelativePlacementKind::Align => {
                        points.insert(if rule.axis == Some(AlignmentAxis::X) {
                            Point::new(ox, placement.position.y_nm)
                        } else {
                            Point::new(placement.position.x_nm, oy)
                        });
                    }
                }
                let squared = |p: &Point| {
                    let (dx, dy) = (i128::from(p.x_nm - ox), i128::from(p.y_nm - oy));
                    dx * dx + dy * dy
                };
                let mut ordered: Vec<Point> = points.into_iter().collect();
                ordered.sort_by_key(|p| (squared(p), p.y_nm, p.x_nm));
                for point in ordered {
                    let distance = squared(&point);
                    let entry = result.entry(moved(&pose, point)).or_insert(distance);
                    *entry = (*entry).min(distance);
                }
            }
        }
    }
    let pin_limited = rules.iter().any(|r| {
        r.kind == RelativePlacementKind::MaxDistance
            && r.targets.iter().all(|t| t.pad.is_some())
            && r.targets.iter().any(|t| t.reference == reference)
    });
    if !pin_limited {
        let mut point_order: IndexMap<Point, usize> = IndexMap::new();
        for pose in result.keys() {
            let next = point_order.len();
            point_order.entry(pose.position).or_insert(next);
        }
        let mut poses: Vec<Placement> = result.into_keys().collect();
        poses.sort_by(|a, b| {
            (point_order[&a.position], &a.rotation_degrees).cmp(&(point_order[&b.position], &b.rotation_degrees))
        });
        return Ok(poses);
    }
    let local: Vec<&RelativePlacementRule> =
        rules.iter().copied().filter(|r| r.targets.iter().any(|t| t.reference == reference)).collect();
    let related: IndexSet<&str> = local.iter().flat_map(|r| r.targets.iter().map(|t| t.reference.as_str())).collect();
    let anchors: Poses =
        placed.iter().filter(|(r, _)| related.contains(r.as_str())).map(|(r, p)| (r.clone(), p.clone())).collect();
    let mut ranked: Vec<((i64, i64, i128, i64, i64), Placement)> = Vec::with_capacity(result.len());
    for (pose, value) in result {
        let trial = with(&anchors, &pose);
        let key = (
            scene.relative_penalty(&trial, &local)?,
            scene.local_connection_length(&trial, &local)?,
            value,
            pose.position.y_nm,
            pose.position.x_nm,
        );
        ranked.push((key, pose));
    }
    ranked.sort_by(|a, b| (&a.0, &a.1.rotation_degrees).cmp(&(&b.0, &b.1.rotation_degrees)));
    Ok(ranked.into_iter().map(|(_, pose)| pose).collect())
}

/// `_anchored_candidate_rules`: candidate-only reach bounds through identical intermediate pads.
pub fn anchored_candidate_rules(scene: &Scene, anchors: &Poses) -> Vec<RelativePlacementRule> {
    let mut graph: IndexMap<PlacementTarget, IndexMap<PlacementTarget, i64>> = IndexMap::new();
    for rule in &scene.relative {
        if rule.kind != RelativePlacementKind::MaxDistance || rule.targets.iter().any(|t| t.pad.is_none()) {
            continue;
        }
        let distance = rule.distance_nm.expect("a maximum distance");
        for pair in rule.targets.windows(2) {
            for (a, b) in [(&pair[0], &pair[1]), (&pair[1], &pair[0])] {
                let neighbours = graph.entry(a.clone()).or_default();
                let value = neighbours.get(b).copied().unwrap_or(distance).min(distance);
                neighbours.insert(b.clone(), value);
            }
        }
    }
    let key = |t: &PlacementTarget| (t.reference.clone(), t.pad.clone().expect("pin target"));
    let index: IndexMap<&PlacementTarget, usize> = graph.keys().enumerate().map(|(i, t)| (t, i)).collect();
    let targets: Vec<&PlacementTarget> = graph.keys().collect();
    let mut pending: BinaryHeap<Reverse<(i64, u64, usize, usize, i64)>> = BinaryHeap::new();
    let mut serial = 0u64;
    let mut starts: Vec<&PlacementTarget> = graph.keys().filter(|t| anchors.contains_key(&t.reference)).collect();
    starts.sort_by_key(|t| key(t));
    for anchor in starts {
        pending.push(Reverse((0, serial, index[anchor], index[anchor], 0)));
        serial += 1;
    }
    let mut reached: IndexMap<usize, (i64, usize, i64)> = IndexMap::new();
    while let Some(Reverse((distance, _, target, anchor, hops))) = pending.pop() {
        if reached.contains_key(&target) {
            continue;
        }
        reached.insert(target, (distance, anchor, hops));
        let mut neighbours: Vec<(&PlacementTarget, &i64)> = graph[targets[target]].iter().collect();
        neighbours.sort_by_key(|(t, _)| key(t));
        for (neighbour, length) in neighbours {
            let id = index[neighbour];
            if !reached.contains_key(&id) {
                pending.push(Reverse((distance + length, serial, id, anchor, hops + 1)));
                serial += 1;
            }
        }
    }
    let mut found: Vec<(usize, (i64, usize, i64))> = reached.into_iter().collect();
    found.sort_by_key(|(t, _)| key(targets[*t]));
    found
        .into_iter()
        .filter(|(target, (_, _, hops))| *hops > 1 && !anchors.contains_key(&targets[*target].reference))
        .map(|(target, (distance, anchor, _))| RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: vec![targets[target].clone(), targets[anchor].clone()],
            distance_nm: Some(distance),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        })
        .collect()
}

/// `_courtyard_contact_candidates`: bounded support-edge sites inside a hard region.
fn courtyard_contact_candidates(
    scene: &Scene,
    target: &Placement,
    anchors: &[&Placement],
    options: &PlannerOptions,
    excluded: &IndexSet<Placement>,
    limit: i64,
) -> Result<Vec<Placement>> {
    let board = scene.board;
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let source: IndexMap<&str, &Placement> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let single: Poses = [(target.reference.clone(), target.clone())].into_iter().collect();
    match source.get(target.reference.as_str()) {
        Some(original)
            if original.footprint == target.footprint
                && !scene.fixed_placements(&single, &options.fixed_references).contains_key(&target.reference)
                && !board.rigid_clusters.iter().any(|c| c.members.iter().any(|m| m.reference == target.reference)) => {}
        _ => return Ok(Vec::new()),
    }
    let Some(rule) = scene.rule(&target.reference).filter(|r| r.region.is_some()) else { return Ok(Vec::new()) };
    let region = board.regions.iter().find(|r| Some(&r.name) == rule.region.as_ref()).expect("validated region");
    let (x0, y0, x1, y1) = point_bounds(&region.outline.vertices);
    let limit = limit as usize;
    let mut local: Vec<(&Placement, (i64, i64, i64, i64), i64)> = Vec::new();
    let mut unique: Vec<&Placement> = Vec::new();
    for anchor in anchors {
        if !unique.contains(anchor) {
            unique.push(anchor);
        }
    }
    for anchor in unique {
        let owner = source.get(anchor.reference.as_str());
        if owner.is_none_or(|o| o.footprint != anchor.footprint) || anchor.side != target.side {
            continue;
        }
        let bounds = point_bounds(&scene.polygon(anchor));
        // A removed member's own pose is a proposal support anchor, not a collision pair.
        let gap = if anchor.reference == target.reference {
            rule.additional_courtyard_gap_nm.unwrap_or(options.component_clearance_nm)
        } else {
            scene.courtyard_gap(&target.reference, &anchor.reference, options.component_clearance_nm)?
        };
        if bounds.2 + gap < x0 || bounds.0 - gap > x1 || bounds.3 + gap < y0 || bounds.1 - gap > y1 {
            continue;
        }
        local.push((anchor, bounds, gap));
    }
    let (tx, ty) = (target.position.x_nm, target.position.y_nm);
    local.sort_by(|a, b| {
        let key = |item: &(&Placement, (i64, i64, i64, i64), i64)| {
            ((item.0.position.x_nm - tx).abs() + (item.0.position.y_nm - ty).abs(), item.1)
        };
        (key(a), &a.0.rotation_degrees).cmp(&(key(b), &b.0.rotation_degrees))
    });
    let mut result: IndexSet<Placement> = IndexSet::new();
    let finish = |result: IndexSet<Placement>| {
        let mut poses: Vec<Placement> = result.into_iter().collect();
        poses.sort_by(|a, b| {
            let key = |p: &Placement| {
                ((p.position.x_nm - tx).abs() + (p.position.y_nm - ty).abs(), p.position.y_nm, p.position.x_nm)
            };
            (key(a), &a.rotation_degrees).cmp(&(key(b), &b.rotation_degrees))
        });
        poses
    };
    let diagonal_scale = hypot(1.0, 1.0);
    for angle in scene.allowed_orientations(&target.reference) {
        let origin = posed(target, Point::new(0, 0), &angle);
        let (lx, ly, rx, ry) = point_bounds(&scene.polygon(&origin));
        let allowed = (x0 - lx, y0 - ly, x1 - rx, y1 - ry);
        if allowed.0 > allowed.2 || allowed.1 > allowed.3 {
            continue;
        }
        let add = |x: i64, y: i64, result: &mut IndexSet<Placement>| {
            if allowed.0 <= x && x <= allowed.2 && allowed.1 <= y && y <= allowed.3 {
                let candidate = posed(target, Point::new(x, y), &angle);
                if !excluded.contains(&candidate) {
                    result.insert(candidate);
                }
            }
            result.len() >= limit
        };
        let region_xs = [allowed.0, (allowed.0 + allowed.2).div_euclid(2), allowed.2];
        let region_ys = [allowed.1, (allowed.1 + allowed.3).div_euclid(2), allowed.3];
        for x in region_xs {
            for y in region_ys {
                if add(x, y, &mut result) {
                    return Ok(finish(result));
                }
            }
        }
        for (_, (ax0, ay0, ax1, ay1), gap) in &local {
            let dedup = |values: [i64; 7]| {
                let mut unique: Vec<i64> = Vec::new();
                for value in values {
                    if !unique.contains(&value) {
                        unique.push(value);
                    }
                }
                unique
            };
            let xs = dedup([
                tx,
                (ax0 + ax1 - lx - rx).div_euclid(2),
                ax0 - lx,
                ax1 - rx,
                region_xs[0],
                region_xs[1],
                region_xs[2],
            ]);
            let ys = dedup([
                ty,
                (ay0 + ay1 - ly - ry).div_euclid(2),
                ay0 - ly,
                ay1 - ry,
                region_ys[0],
                region_ys[1],
                region_ys[2],
            ]);
            for x in [ax0 - gap - rx, ax1 + gap - lx] {
                for y in &ys {
                    if add(x, *y, &mut result) {
                        return Ok(finish(result));
                    }
                }
            }
            for y in [ay0 - gap - ry, ay1 + gap - ly] {
                for x in &xs {
                    if add(*x, y, &mut result) {
                        return Ok(finish(result));
                    }
                }
            }
            // Conservative nm rounding for corner support; _legal still checks the true polygon.
            let diagonal = (*gap as f64 / diagonal_scale).ceil() as i64;
            for x in [ax0 - rx - diagonal, ax1 - lx + diagonal] {
                for y in [ay0 - ry - diagonal, ay1 - ly + diagonal] {
                    if add(x, y, &mut result) {
                        return Ok(finish(result));
                    }
                }
            }
        }
    }
    Ok(finish(result))
}

/// The residual budget a region-refinement pass of `_pack_local_components` continues with.
struct RegionState {
    domains: IndexMap<String, Vec<Placement>>,
    refinable: IndexSet<String>,
    domain_remaining: IndexMap<String, i64>,
    search_remaining: i64,
    heuristic_remaining: i64,
}

struct Search<'s> {
    scene: &'s Scene<'s>,
    references: &'s [String],
    rules: Vec<&'s RelativePlacementRule>,
    domains: Vec<Vec<Placement>>,
    polygons: Vec<Vec<Vec<Point>>>,
    pair_rules: IndexMap<(usize, usize), Vec<&'s RelativePlacementRule>>,
    clashes: IndexMap<(usize, usize, usize, usize), bool>,
    remaining: i64,
    heuristic_remaining: i64,
    exact_repair_candidates: usize,
    component_clearance_nm: i64,
}

impl Search<'_> {
    fn clash(&mut self, left: usize, i: usize, right: usize, j: usize) -> Result<bool> {
        if let Some(value) = self.clashes.get(&(left, i, right, j)) {
            return Ok(*value);
        }
        let (a, b) = (&self.domains[left][i], &self.domains[right][j]);
        let mut value = false;
        if a.side == b.side {
            let gap = self.scene.courtyard_gap(
                &self.references[left],
                &self.references[right],
                self.component_clearance_nm,
            )?;
            value = copper_drc::legality::courtyards_too_close(&self.polygons[left][i], &self.polygons[right][j], gap);
        }
        if !value {
            let key = if left <= right { (left, right) } else { (right, left) };
            if let Some(rules) = self.pair_rules.get(&key) {
                let mut pair = Poses::new();
                pair.insert(self.references[left].clone(), a.clone());
                pair.insert(self.references[right].clone(), b.clone());
                value = self.scene.relative_penalty(&pair, rules)? > 0;
            }
        }
        self.clashes.insert((left, i, right, j), value);
        Ok(value)
    }

    /// `solve`: most-constrained-first backtracking with courtyard forward checking.
    fn solve(&mut self, available: &IndexMap<usize, Vec<usize>>, current: &Poses) -> Result<Option<Poses>> {
        if available.is_empty() {
            return Ok((self.scene.relative_penalty(current, &self.rules)? == 0).then(|| current.clone()));
        }
        let reference = *available
            .iter()
            .min_by_key(|(item, indices)| (indices.len(), **item, &self.references[**item]))
            .map(|(item, _)| item)
            .expect("available");
        let obstacles = local_obstacles(self.scene, &self.references[reference], current);
        // Among equally feasible poses, leave space for the other companions.
        let samples: Vec<(usize, Vec<usize>)> = available
            .iter()
            .filter(|(other, _)| **other != reference)
            .map(|(other, indices)| {
                let sample = if indices.len() <= 16 {
                    indices.clone()
                } else {
                    (0..16)
                        .map(|i| indices[round_int(true_div((i * (indices.len() - 1)) as i128, 15)) as usize])
                        .collect()
                };
                (*other, sample)
            })
            .collect();
        let mut choices = available[&reference].clone();
        let limit = self.exact_repair_candidates;
        if choices.len() > limit {
            choices = (0..limit).map(|i| choices[spread_index(i, choices.len(), limit)]).collect();
        }
        let mut ranked: Vec<(f64, usize)> = Vec::new();
        'rank: for index in choices {
            let mut conflicts = 0.0f64;
            for (other, indices) in &samples {
                if self.heuristic_remaining < indices.len() as i64 {
                    break 'rank;
                }
                self.heuristic_remaining -= indices.len() as i64;
                let mut count = 0i64;
                for j in indices {
                    count += self.clash(reference, index, *other, *j)? as i64;
                }
                conflicts += true_div(i128::from(count), indices.len() as i128);
            }
            ranked.push((conflicts, index));
        }
        // Heuristic exhaustion cannot reject an otherwise feasible group.
        let ranked_indices: IndexSet<usize> = ranked.iter().map(|(_, i)| *i).collect();
        ranked.sort_by(|a, b| a.partial_cmp(b).expect("finite conflicts"));
        let mut ordered: Vec<usize> = ranked.iter().map(|(_, i)| *i).collect();
        ordered.extend(available[&reference].iter().filter(|i| !ranked_indices.contains(*i)));
        let mut others: Vec<(&usize, &Vec<usize>)> = available.iter().collect();
        others.sort_by_key(|(other, indices)| (indices.len(), &self.references[**other]));
        for index in ordered {
            if self.remaining <= 0 {
                return Ok(None);
            }
            self.remaining -= 1;
            let candidate = self.domains[reference][index].clone();
            let trial = with(current, &candidate);
            if self.scene.relative_penalty(&trial, &self.rules)? != 0 || !self.scene.legal(&candidate, &obstacles)? {
                continue;
            }
            let mut rest: IndexMap<usize, Vec<usize>> = IndexMap::new();
            let mut complete = true;
            for (other, indices) in &others {
                if **other == reference {
                    continue;
                }
                let mut kept = Vec::new();
                for j in indices.iter() {
                    if self.remaining <= 0 {
                        return Ok(None);
                    }
                    self.remaining -= 1;
                    if !self.clash(reference, index, **other, *j)? {
                        kept.push(*j);
                    }
                }
                if kept.is_empty() {
                    complete = false;
                    break;
                }
                rest.insert(**other, kept);
            }
            if complete && let Some(result) = self.solve(&rest, &trial)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }
}

/// `_pack_local_components`: one bounded domain search for anchored companions and local repair.
///
/// Domains contain individually legal poses; courtyard forward checking
/// removes obvious conflicts, while every selected pose still passes the
/// complete gate. Flexible positions change; rigid membership and locks never do.
#[allow(clippy::too_many_arguments)]
pub fn pack_local_components(
    scene: &Scene,
    references: &[String],
    base: &Poses,
    targets: &Poses,
    options: &PlannerOptions,
    allow_general: bool,
    fine: bool,
    contact_anchors: &[Placement],
) -> Result<Option<Poses>> {
    pack(scene, references, base, targets, options, allow_general, fine, None, contact_anchors)
}

#[allow(clippy::too_many_arguments)]
fn pack(
    scene: &Scene,
    references: &[String],
    base: &Poses,
    targets: &Poses,
    options: &PlannerOptions,
    allow_general: bool,
    fine: bool,
    state: Option<&mut RegionState>,
    contact_anchors: &[Placement],
) -> Result<Option<Poses>> {
    let board = scene.board;
    let in_references = |r: &str| references.iter().any(|x| x == r);
    let rules: Vec<&RelativePlacementRule> =
        scene.relative.iter().copied().filter(|r| r.targets.iter().any(|t| in_references(&t.reference))).collect();
    let derived = anchored_candidate_rules(scene, base);
    let candidate_rules: Vec<&RelativePlacementRule> = scene.relative.iter().copied().chain(derived.iter()).collect();
    let limit = 2.max(options.legalization_candidates * 4) as usize;
    let probe_limit = options.probe_limit();
    let mut refinable = false;
    let mut region_refinable: IndexSet<String> = IndexSet::new();
    let mut region_probe_remaining: IndexMap<String, i64> = IndexMap::new();
    let mut immutable: IndexSet<String> =
        scene.fixed_placements(targets, &options.fixed_references).into_keys().collect();
    immutable.extend(board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.clone())));
    let source: IndexMap<&str, &Placement> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let mut held_seeds: Vec<&Placement> = Vec::new();
    for pose in contact_anchors {
        let reference = pose.reference.as_str();
        if in_references(reference)
            && !immutable.contains(reference)
            && targets.contains_key(reference)
            && source.contains_key(reference)
            && pose.footprint == targets[reference].footprint
            && targets[reference].footprint == source[reference].footprint
            && pose.side == targets[reference].side
            && scene.allowed_orientations(reference).contains(&pose.rotation_degrees)
            && scene.legal(pose, base)?
        {
            held_seeds.push(pose);
        }
    }
    let mut anchors: Vec<&Placement> = base.values().collect();
    anchors.extend(references.iter().map(|r| &targets[r]));
    anchors.extend(held_seeds.iter().copied());
    let own_rules_of = |reference: &str| -> Vec<&RelativePlacementRule> {
        rules.iter().copied().filter(|r| r.targets.iter().any(|t| t.reference == reference)).collect()
    };
    let shift = options.grid_step_nm.min(100_000);
    let neighbours_of = |legal: &[Placement], seen: Option<&IndexSet<Placement>>| -> Vec<Placement> {
        let mut result: IndexSet<Placement> = IndexSet::new();
        if seen.is_none() {
            result.extend(legal.iter().cloned());
        }
        for pose in legal {
            for dx in [-shift, 0, shift] {
                for dy in [-shift, 0, shift] {
                    let candidate = moved(pose, Point::new(pose.position.x_nm + dx, pose.position.y_nm + dy));
                    if seen.is_none_or(|s| !s.contains(&candidate)) {
                        result.insert(candidate);
                    }
                }
            }
        }
        result.into_iter().collect()
    };
    let mut domains: IndexMap<String, Vec<Placement>> = IndexMap::new();
    let has_state = state.is_some();
    match &state {
        None => {
            for reference in references {
                let target = &targets[reference];
                let obstacles = local_obstacles(scene, reference, base);
                let own_rules = own_rules_of(reference);
                let relative = relative_candidates(scene, &candidate_rules, reference, target, base, options, 250_000)?;
                let general: Candidates = if allow_general {
                    general_candidates(scene, target.clone(), options, scene.allowed_orientations(reference))
                } else {
                    Box::new(std::iter::empty())
                };
                let mut legal: Vec<Placement> = Vec::new();
                let mut seen: IndexSet<Placement> = IndexSet::new();
                let mut probed = 0i64;
                for (index, candidate) in bounded_local_candidates(relative, probe_limit, general).enumerate() {
                    if index >= probe_limit {
                        break;
                    }
                    probed += 1;
                    if !seen.insert(candidate.clone()) {
                        continue;
                    }
                    if scene.relative_penalty(&with(base, &candidate), &own_rules)? == 0
                        && scene.legal(&candidate, &obstacles)?
                    {
                        legal.push(candidate);
                    }
                }
                let ordinary_count = legal.len();
                let rule = scene.rule(reference);
                let in_region = rule.is_some_and(|r| r.region.is_some());
                if !fine
                    && allow_general
                    && in_region
                    && own_rules.is_empty()
                    && !immutable.contains(reference)
                    && ordinary_count < limit
                {
                    let contacts = courtyard_contact_candidates(
                        scene,
                        target,
                        &anchors,
                        options,
                        &seen,
                        probe_limit as i64 - probed,
                    )?;
                    for candidate in contacts {
                        probed += 1;
                        if scene.relative_penalty(&with(base, &candidate), &own_rules)? == 0
                            && scene.legal(&candidate, &obstacles)?
                        {
                            legal.push(candidate);
                        }
                    }
                }
                region_probe_remaining.insert(reference.clone(), probe_limit as i64 - probed);
                if legal.is_empty() {
                    return Ok(None);
                }
                let narrow = legal.len() <= options.legalization_candidates as usize
                    && own_rules.iter().any(|r| {
                        r.kind == RelativePlacementKind::MaxDistance && r.targets.iter().all(|t| t.pad.is_some())
                    });
                refinable |= narrow;
                if allow_general
                    && in_region
                    && own_rules.is_empty()
                    && legal.len() <= options.legalization_candidates as usize
                    && !immutable.contains(reference)
                    && region_probe_remaining[reference] > 0
                {
                    region_refinable.insert(reference.clone());
                }
                if fine && narrow {
                    // Quarter-grid poses can fit a tight row individually while
                    // missing the small shifts needed to fit together.
                    let mut candidates = neighbours_of(&legal, None);
                    if candidates.len() > probe_limit {
                        let n = candidates.len();
                        candidates = (0..probe_limit)
                            .map(|i| {
                                candidates
                                    [round_int(true_div((i * (n - 1)) as i128, (probe_limit - 1) as i128)) as usize]
                                    .clone()
                            })
                            .collect();
                    }
                    let mut refined = Vec::new();
                    for pose in candidates {
                        if scene.relative_penalty(&with(base, &pose), &own_rules)? == 0
                            && scene.legal(&pose, &obstacles)?
                        {
                            refined.push(pose);
                        }
                    }
                    legal = refined;
                }
                // Preserve candidates across the complete local range, including a second row.
                if legal.len() > limit {
                    if !fine && ordinary_count < limit {
                        let contacts = legal.split_off(ordinary_count);
                        let slots = limit - ordinary_count;
                        legal.extend((0..slots).map(|i| contacts[spread_index(i, contacts.len(), slots)].clone()));
                    } else {
                        let n = legal.len();
                        legal = (0..limit)
                            .map(|i| {
                                legal[round_int(true_div((i * (n - 1)) as i128, (limit - 1) as i128)) as usize].clone()
                            })
                            .collect();
                    }
                }
                domains.insert(reference.clone(), legal);
            }
        }
        Some(region) => {
            let refinable_set = region.refinable.clone();
            let mut remaining_by_ref = region.domain_remaining.clone();
            for reference in references {
                let mut legal = region.domains[reference].clone();
                if refinable_set.contains(reference) {
                    // Reuse accepted coarse sites; only new neighbour probes spend the residual allowance.
                    let seen: IndexSet<Placement> = legal.iter().cloned().collect();
                    let mut candidates = neighbours_of(&legal, Some(&seen));
                    let available = remaining_by_ref[reference] as usize;
                    if candidates.len() > available {
                        let n = candidates.len();
                        candidates =
                            (0..available).map(|i| candidates[spread_index(i, n, available)].clone()).collect();
                    }
                    let obstacles = local_obstacles(scene, reference, base);
                    let own_rules = own_rules_of(reference);
                    for candidate in candidates {
                        remaining_by_ref[reference] -= 1;
                        if scene.relative_penalty(&with(base, &candidate), &own_rules)? == 0
                            && scene.legal(&candidate, &obstacles)?
                        {
                            legal.push(candidate);
                        }
                    }
                }
                if legal.len() > limit {
                    let n = legal.len();
                    legal = (0..limit)
                        .map(|i| {
                            legal[round_int(true_div((i * (n - 1)) as i128, (limit - 1) as i128)) as usize].clone()
                        })
                        .collect();
                }
                domains.insert(reference.clone(), legal);
            }
            let _ = remaining_by_ref;
        }
    }
    let total = options.legalization_candidates * options.exact_repair_candidates * 64;
    let (remaining, heuristic_remaining) = match &state {
        None => (total, total.div_euclid(3)),
        Some(region) => (region.search_remaining, region.heuristic_remaining),
    };
    let order: Vec<String> = domains.keys().cloned().collect();
    let position = |reference: &str| references.iter().position(|r| r == reference).expect("member");
    let mut pair_rules: IndexMap<(usize, usize), Vec<&RelativePlacementRule>> = IndexMap::new();
    for rule in &rules {
        let pair: BTreeSet<&str> = rule.targets.iter().map(|t| t.reference.as_str()).collect();
        if pair.len() == 2 && pair.iter().all(|r| domains.contains_key(*r)) {
            let indexes: Vec<usize> = pair.iter().map(|r| order.iter().position(|o| o == r).expect("domain")).collect();
            let key = (indexes[0].min(indexes[1]), indexes[0].max(indexes[1]));
            pair_rules.entry(key).or_default().push(rule);
        }
    }
    // `references.index(item)` orders ties; domains follow the references order.
    debug_assert!(order.iter().enumerate().all(|(i, r)| position(r) == i));
    let mut search = Search {
        scene,
        references: &order,
        rules: rules.clone(),
        polygons: domains.values().map(|poses| poses.iter().map(|p| scene.polygon(p)).collect()).collect(),
        domains: domains.values().cloned().collect(),
        pair_rules,
        clashes: IndexMap::new(),
        remaining,
        heuristic_remaining,
        exact_repair_candidates: options.exact_repair_candidates as usize,
        component_clearance_nm: options.component_clearance_nm,
    };
    let initial: IndexMap<usize, Vec<usize>> =
        domains.values().enumerate().map(|(i, poses)| (i, (0..poses.len()).collect())).collect();
    let result = search.solve(&initial, base)?;
    let (remaining, heuristic_remaining) = (search.remaining, search.heuristic_remaining);
    if let Some(region) = state {
        region.search_remaining = remaining;
        region.heuristic_remaining = heuristic_remaining;
        return Ok(result);
    }
    let _ = has_state;
    if result.is_none() && refinable && !fine {
        // Preserve the pre-existing pad-MAX_DISTANCE fine behaviour.
        return pack(scene, references, base, targets, options, allow_general, true, None, contact_anchors);
    }
    if result.is_none() && !region_refinable.is_empty() && !fine && remaining > 0 {
        let mut region = RegionState {
            domains,
            refinable: region_refinable,
            domain_remaining: region_probe_remaining,
            search_remaining: remaining,
            heuristic_remaining,
        };
        return pack(
            scene,
            references,
            base,
            targets,
            options,
            allow_general,
            true,
            Some(&mut region),
            contact_anchors,
        );
    }
    Ok(result)
}

/// `_best_legal_choice`: the best individually legal pose near its target.
#[allow(clippy::too_many_arguments)]
pub fn best_legal_choice(
    scene: &Scene,
    reference: &str,
    target: &Placement,
    placed: &Poses,
    adjacency: &IndexMap<String, IndexMap<String, i64>>,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
    seed: i64,
    limit: Option<usize>,
) -> Result<Option<Placement>> {
    let relative_rules: Vec<&RelativePlacementRule> =
        scene.relative.iter().copied().filter(|r| r.targets.iter().any(|t| t.reference == reference)).collect();
    let local = relative_rules.iter().any(|r| r.targets.iter().all(|t| t.pad.is_some()));
    let mut best: Option<((i64, i64, i64, i64, i64, i64), Placement)> = None;
    let obstacles = local_obstacles(scene, reference, placed);
    let mut orientations = scene.allowed_orientations(reference);
    let shift = (seed as usize) % orientations.len();
    orientations.rotate_left(shift);
    let mut general: Option<Candidates> = Some(general_candidates(scene, target.clone(), options, orientations));
    for step in [250_000, 100_000] {
        let fallback: Candidates = general.take().unwrap_or_else(|| Box::new(std::iter::empty()));
        let mut count = 0usize;
        let local_target = best.as_ref().map_or(target, |(_, pose)| pose).clone();
        let relative = relative_candidates(scene, &scene.relative, reference, &local_target, placed, options, step)?;
        let candidates: Candidates = if local {
            bounded_local_candidates(relative, options.probe_limit(), fallback)
        } else {
            Box::new(relative.into_iter().chain(fallback))
        };
        for (probe, candidate) in candidates.enumerate() {
            if local && probe >= options.probe_limit() {
                break;
            }
            if !scene.legal(&candidate, &obstacles)? {
                continue;
            }
            let trial = with(placed, &candidate);
            let penalty = scene.relative_penalty(&trial, &relative_rules)?;
            let incremental = incremental_cost(reference, &candidate, placed, adjacency);
            let deficit = if local { spacing.channels(&trial, false).iter().map(|c| c.deficit_nm()).sum() } else { 0 };
            let rank = (
                if local { penalty } else { incremental + penalty * 100 },
                deficit,
                scene.local_connection_length(&trial, &relative_rules)?,
                if local { incremental } else { 0 },
                candidate.position.y_nm,
                candidate.position.x_nm,
            );
            let better = match &best {
                None => true,
                Some((key, pose)) => (&rank, &candidate.rotation_degrees) < (key, &pose.rotation_degrees),
            };
            if better {
                best = Some((rank, candidate));
            }
            count += 1;
            if limit.is_some_and(|limit| count >= limit) {
                break;
            }
        }
        if let Some((_, pose)) = &best
            && scene.relative_penalty(&with(placed, pose), &relative_rules)? == 0
        {
            break;
        }
    }
    Ok(best.map(|(_, pose)| pose))
}

/// `_exact_repair`: repack the failed component with its actual blockers and local companions.
#[allow(clippy::too_many_arguments)]
fn exact_repair(
    scene: &Scene,
    failed: &str,
    placed: &Poses,
    placed_order: &[String],
    targets: &Poses,
    options: &PlannerOptions,
) -> Result<Option<Poses>> {
    let fixed = scene.fixed_placements(placed, &options.fixed_references);
    let movable: BTreeSet<&str> = placed_order.iter().map(String::as_str).filter(|r| !fixed.contains_key(*r)).collect();
    // Prefer actual blockers and local companions to an unrelated insertion tail.
    let mut blockers: IndexMap<&str, i64> = IndexMap::new();
    let mut probes: Vec<Placement> =
        relative_candidates(scene, &scene.relative, failed, &targets[failed], placed, options, 250_000)?;
    if scene.rule(failed).is_some_and(|r| r.region.is_some()) {
        // Spread the raw probe allowance over the complete ordinary regional domain.
        let positions = candidate_positions(scene, &targets[failed], options);
        let angles = scene.allowed_orientations(failed);
        let count = positions.len() * angles.len();
        let sample_count = (options.exact_repair_candidates as usize).min(count);
        for i in 0..sample_count {
            let index = spread_index(i, count, sample_count);
            probes.push(posed(&targets[failed], positions[index / angles.len()], &angles[index % angles.len()]));
        }
    }
    let mut checked = 0;
    for candidate in &probes {
        if !scene.legal(candidate, &fixed)? {
            continue;
        }
        for reference in &movable {
            let single: Poses = [((*reference).to_owned(), placed[*reference].clone())].into_iter().collect();
            if !scene.legal(candidate, &single)? {
                *blockers.entry(reference).or_insert(0) += 1;
            }
        }
        checked += 1;
        if checked >= options.exact_repair_candidates {
            break;
        }
    }
    let units = placement_units(scene.board, &scene.relative);
    let companions: IndexSet<&str> = units[failed].iter().map(String::as_str).filter(|r| movable.contains(r)).collect();
    let order: IndexMap<&str, usize> = placed_order.iter().enumerate().map(|(i, r)| (r.as_str(), i)).collect();
    let mut neighbours: Vec<&str> = movable.iter().copied().collect();
    neighbours.sort_by_key(|r| {
        (
            !blockers.contains_key(r),
            !companions.contains(r),
            -blockers.get(r).copied().unwrap_or(0),
            -(order[r] as i64),
            (*r).to_owned(),
        )
    });
    let mut repair: Vec<String> = vec![failed.to_owned()];
    repair.extend(neighbours.iter().take((options.exact_repair_limit - 1) as usize).map(|r| (*r).to_owned()));
    let base: Poses = placed.iter().filter(|(r, _)| !repair.contains(r)).map(|(r, p)| (r.clone(), p.clone())).collect();
    let anchors: Vec<Placement> = repair.iter().filter_map(|r| placed.get(r).cloned()).collect();
    pack_local_components(scene, &repair, &base, targets, options, true, false, &anchors)
}

/// `_local_failure_details`: explain a bounded-search failure without claiming impossibility.
pub fn local_failure_details(scene: &Scene, placements: &Poses, options: &PlannerOptions) -> Result<String> {
    let violations = scene.violations(placements)?;
    let shown = &violations[..violations.len().min(3)];
    let mut messages: Vec<String> = shown.iter().map(|v| v.message.clone()).collect();
    let mut blockers: BTreeSet<String> = BTreeSet::new();
    let references: BTreeSet<&String> = shown.iter().flat_map(|v| v.references.iter()).collect();
    for reference in references {
        if scene.fixed_placements(placements, &options.fixed_references).contains_key(reference) {
            continue;
        }
        let others = crate::scene::without(placements, reference);
        let probes =
            relative_candidates(scene, &scene.relative, reference, &placements[reference], &others, options, 100_000)?;
        for candidate in probes.iter().take(options.exact_repair_candidates as usize) {
            if !scene.legal(candidate, &Poses::new())? {
                continue;
            }
            for (other_reference, other) in &others {
                let single: Poses = [(other_reference.clone(), other.clone())].into_iter().collect();
                if !scene.legal(candidate, &single)? {
                    blockers.insert(other_reference.clone());
                }
            }
        }
    }
    if !blockers.is_empty() {
        let names: Vec<&str> = blockers.iter().take(8).map(String::as_str).collect();
        messages.push(format!("local candidate blockers: {}", names.join(", ")));
    }
    let text = messages.join("; ");
    Ok(if text.is_empty() { "no legal pose found in the bounded candidate search".to_owned() } else { text })
}

/// `_BoundedLegalizationFailure`: an exhausted mutable legalization discards one seed.
pub struct LegalizationFailure {
    pub message: String,
    pub failed_reference: String,
    pub accepted: Poses,
}

/// The outcome of `_legalize`.
pub enum Legalized {
    Placed(Poses, i64),
    Failed(LegalizationFailure),
}

/// `_legalize`: fixed poses first, anchored companion packs, then greedy choices with exact repair.
pub fn legalize(
    scene: &Scene,
    targets: &Poses,
    original: &Poses,
    options: &PlannerOptions,
    spacing: &EscapeSpacingModel,
    seed: i64,
) -> Result<Legalized> {
    let board = scene.board;
    let fixed = scene.fixed_placements(original, &options.fixed_references);
    let mut placed = Poses::new();
    for (reference, candidate) in &fixed {
        if !scene.legal(candidate, &placed)? {
            return Err(Error::Value(format!(
                "fixed component {} violates board, keepout, region, or clearance constraints",
                copper_json::python_str_repr(reference)
            )));
        }
        placed.insert(reference.clone(), candidate.clone());
    }
    let adjacency = adjacency(scene);
    let priorities = component_priorities(scene);
    let mut movable: Vec<String> = targets.keys().filter(|r| !fixed.contains_key(*r)).cloned().collect();
    movable.sort_by_key(|r| {
        (
            -priorities.get(r).copied().unwrap_or(0),
            -adjacency.get(r).map_or(0, |n| n.values().sum::<i64>()),
            -scene.footprint_area(&targets[r]),
            r.clone(),
        )
    });
    let movable = relative_cluster_order(scene, &movable, targets);
    let mut placed_order: Vec<String> = Vec::new();
    let mut repair_count = 0;
    let rigid: IndexSet<&str> =
        board.rigid_clusters.iter().flat_map(|c| c.members.iter().map(|m| m.reference.as_str())).collect();
    let derived = anchored_candidate_rules(scene, &fixed);
    let candidate_rules: Vec<&RelativePlacementRule> = scene.relative.iter().copied().chain(derived.iter()).collect();
    let mut direct: IndexSet<&str> = IndexSet::new();
    for rule in &candidate_rules {
        if rule.kind != RelativePlacementKind::MaxDistance
            || !rule.targets.iter().any(|t| fixed.contains_key(&t.reference))
        {
            continue;
        }
        for target in &rule.targets {
            if !fixed.contains_key(&target.reference) && !rigid.contains(target.reference.as_str()) {
                direct.insert(&target.reference);
            }
        }
    }
    let units = placement_units(board, &scene.relative);
    let local_rules: Vec<&RelativePlacementRule> = candidate_rules
        .iter()
        .copied()
        .filter(|rule| {
            let refs: IndexSet<&str> = rule.targets.iter().map(|t| t.reference.as_str()).collect();
            refs.iter().filter(|r| direct.contains(**r)).count() > 1
        })
        .collect();
    let local_units = placement_units(board, &local_rules);
    let order: IndexMap<&str, usize> = movable.iter().enumerate().map(|(i, r)| (r.as_str(), i)).collect();
    let by_order = |refs: &BTreeSet<String>| refs.iter().map(|r| order[r.as_str()]).min().expect("members");
    let mut groups: Vec<BTreeSet<String>> = Vec::new();
    for reference in &direct {
        let group: BTreeSet<String> =
            units[*reference].iter().filter(|r| direct.contains(r.as_str())).cloned().collect();
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups.sort_by_key(|g| by_order(g));
    for group in groups {
        if group.len() < 2 {
            continue;
        }
        let mut references: Vec<String> = group.iter().cloned().collect();
        references.sort_by_key(|r| order[r.as_str()]);
        scene.progress(&format!("support_pack:{}", references.join(",")));
        if let Some(packed) = pack_local_components(scene, &references, &placed, targets, options, false, false, &[])? {
            placed = packed;
            placed_order.extend(references);
            repair_count += 1;
        } else if group.len() as i64 > options.exact_repair_limit {
            let mut packs: Vec<BTreeSet<String>> = Vec::new();
            for reference in &group {
                let pack: BTreeSet<String> =
                    local_units[reference].iter().filter(|r| group.contains(*r)).cloned().collect();
                if !packs.contains(&pack) {
                    packs.push(pack);
                }
            }
            packs.sort_by_key(|p| by_order(p));
            for pack in packs {
                if pack.len() < 2 {
                    continue;
                }
                let mut members: Vec<String> = pack.into_iter().collect();
                members.sort_by_key(|r| order[r.as_str()]);
                scene.progress(&format!("support_pack:{}", members.join(",")));
                if let Some(packed) =
                    pack_local_components(scene, &members, &placed, targets, options, false, false, &[])?
                {
                    placed = packed;
                    placed_order.extend(members);
                    repair_count += 1;
                }
            }
        }
    }
    for reference in &movable {
        if placed.contains_key(reference) {
            continue;
        }
        scene.progress(&format!("legalize:{reference}"));
        let choice = best_legal_choice(
            scene,
            reference,
            &targets[reference],
            &placed,
            &adjacency,
            options,
            spacing,
            seed,
            Some(options.legalization_candidates as usize),
        )?;
        let worse = match &choice {
            None => true,
            Some(pose) => scene.penalty(&with(&placed, pose))? > scene.penalty(&placed)?,
        };
        if worse {
            match exact_repair(scene, reference, &placed, &placed_order, targets, options)? {
                Some(repaired) => {
                    placed = repaired;
                    repair_count += 1;
                }
                None => match choice {
                    Some(pose) => {
                        placed.insert(reference.clone(), pose);
                    }
                    None => {
                        let details = local_failure_details(scene, &with(&placed, &targets[reference]), options)?;
                        return Ok(Legalized::Failed(LegalizationFailure {
                            message: format!(
                                "cannot legalize component {} within the local repair budget; {details}",
                                copper_json::python_str_repr(reference)
                            ),
                            failed_reference: reference.clone(),
                            accepted: placed,
                        }));
                    }
                },
            }
        } else {
            placed.insert(reference.clone(), choice.expect("a choice"));
        }
        placed_order.push(reference.clone());
    }
    Ok(Legalized::Placed(placed, repair_count))
}

#[cfg(test)]
#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod test_boards;

#[cfg(test)]
mod tests {
    use super::test_boards::*;
    use super::*;
    use copper_pcb::board::{ComponentPlacementRule, PlacementRegion, PlacementTarget, RelativePlacementKind};
    use copper_physical::BoardOutline;

    fn max_distance(a: (&str, &str), b: (&str, &str), distance: f64) -> RelativePlacementRule {
        let target = |(reference, pad): (&str, &str)| PlacementTarget {
            reference: reference.to_owned(),
            pad: Some(pad.to_owned()),
        };
        RelativePlacementRule {
            kind: RelativePlacementKind::MaxDistance,
            targets: vec![target(a), target(b)],
            distance_nm: Some(mm(distance)),
            axis: None,
            tolerance_nm: 0,
            weight: 1,
        }
    }

    fn fixed(reference: &str, x: f64, y: f64) -> ComponentPlacementRule {
        ComponentPlacementRule {
            fixed_position: Some(pt(x, y)),
            fixed_rotation_degrees: Some(Decimal::from_i64(0)),
            ..ComponentPlacementRule::new(reference)
        }
    }

    /// `tests/test_escape_placement.py::test_local_repair_moves_actual_blocker_instead_of_unrelated_recent_component`.
    #[test]
    fn local_repair_moves_actual_blocker_instead_of_unrelated_recent_component() {
        for unrelated_failure in [false, true] {
            let chip = footprint("chip", vec![pad("1", 1.0, -0.6, 0.3, 0.3), pad("2", 1.0, 0.6, 0.3, 0.3)], 2.0, 2.0);
            let cap =
                footprint("capacitor", vec![pad("1", -0.3, 0.0, 0.3, 0.3), pad("2", 0.3, 0.0, 0.3, 0.3)], 1.0, 0.5);
            let mut placements = vec![
                placement("U", "chip", 10.0, 10.0),
                placement("C1", "capacitor", 20.0, 20.0),
                placement("X", "capacitor", 12.0, 9.4),
                placement("Y", "capacitor", 20.0, 20.0),
            ];
            let mut rules = vec![
                fixed("U", 10.0, 10.0),
                ComponentPlacementRule { region: Some("slot".to_owned()), ..ComponentPlacementRule::new("C1") },
            ];
            let mut relative = vec![max_distance(("C1", "1"), ("U", "1"), 1.5)];
            if unrelated_failure {
                placements.push(placement("Z", "chip", 22.0, 10.0));
                rules.push(fixed("Z", 22.0, 10.0));
                relative.push(max_distance(("Y", "1"), ("Z", "1"), 3.0));
            }
            let mut board = board("local", 30.0, 30.0, vec![chip, cap], placements, Vec::new());
            board.placement_rules = rules;
            board.relative_rules = relative;
            board.regions = vec![PlacementRegion {
                name: "slot".to_owned(),
                outline: BoardOutline::new(
                    [(11.4, 8.9), (12.6, 8.9), (12.6, 9.9), (11.4, 9.9)].iter().map(|(x, y)| pt(*x, *y)).collect(),
                    Vec::new(),
                    None,
                    None,
                )
                .expect("outline"),
                side: None,
            }];
            let options = PlannerOptions {
                candidate_count: 1,
                analytical_iterations: 0,
                refinement_passes: 0,
                exact_repair_limit: 2,
                ..PlannerOptions::default()
            }
            .validated()
            .expect("options");
            let scene = Scene::new(&board, options.policy());
            let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            let placed: Poses =
                targets.iter().filter(|(r, _)| *r != "C1").map(|(r, p)| (r.clone(), p.clone())).collect();
            let order = vec!["X".to_owned(), "Y".to_owned()];
            let result =
                exact_repair(&scene, "C1", &placed, &order, &targets, &options).expect("repair").expect("a repair");
            assert!(result["U"] == targets["U"] && result["Y"] == targets["Y"]);
            assert_ne!(result["X"], targets["X"]);
            assert_eq!(scene.checker.placement_solution_is_legal(&result).expect("legal"), !unrelated_failure);
        }
    }
}

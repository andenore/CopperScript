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
use copper_units::{Decimal, Fraction};

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
    adjacency_with_hierarchy(scene, true)
}

fn adjacency_with_hierarchy(scene: &Scene, include_hierarchy: bool) -> IndexMap<String, IndexMap<String, i64>> {
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
    for group in board.placement_groups.iter().filter(|group| include_hierarchy || group.source != "hierarchy") {
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
    for group in scene.board.placement_groups.iter().filter(|group| group.source != "hierarchy") {
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
    // Use the occupied courtyard rather than the body when one is supplied.
    // The ratio is only a scheduling heuristic; actual candidate domains still
    // pass the full outline, region, height, orientation and clearance checks.
    let courtyard_areas: IndexMap<&str, Fraction> = targets
        .iter()
        .map(|(reference, pose)| {
            let footprint = &scene.board.footprints[&pose.footprint];
            let twice_area = if footprint.courtyard.is_empty() {
                2 * i128::from(footprint.body_size.width_nm) * i128::from(footprint.body_size.height_nm)
            } else {
                copper_physical::mechanical::signed_area_twice(&footprint.courtyard).abs()
            };
            (reference.as_str(), Fraction::from_integer(twice_area.max(1).into()))
        })
        .collect();
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
                let radius = limits.iter().copied().min();
                let relative_space = radius.map(|radius| {
                    let radius = Fraction::from_integer(radius.into());
                    // Big rational arithmetic avoids squared-distance/cross-
                    // product overflow and floating-point ordering ties.
                    &radius * &radius / &courtyard_areas[reference.as_str()]
                });
                (
                    limits.is_empty(),
                    relative_space,
                    radius.map_or(i128::from(i64::MAX) + 1, i128::from),
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

/// An unanchored pin-relative unit has no local neighbourhood yet. Keep the
/// nearest three quarters of its ordinary allowance, reserving the remainder
/// for positions spread across the unused domain. Sample positions rather than
/// flattened poses so every sampled site retains every allowed orientation.
fn unanchored_candidates(
    scene: &Scene,
    target: &Placement,
    options: &PlannerOptions,
    orientations: &[Decimal],
) -> (Vec<Placement>, usize) {
    if orientations.is_empty() {
        return (Vec::new(), 0);
    }
    let allowance = options.probe_limit() / orientations.len();
    if allowance == 0 {
        // A source may permit more angles than one raw probe allowance. Keep
        // the original partial cycle instead of producing an empty domain.
        let candidates: Vec<_> = general_candidates(scene, target.clone(), options, orientations.to_vec())
            .take(options.probe_limit())
            .collect();
        let nearest = candidates.len();
        return (candidates, nearest);
    }
    let positions = candidate_positions(scene, target, options);
    let nearest = if positions.len() <= allowance { positions.len() } else { allowance * 3 / 4 };
    let remainder = (allowance - nearest).min(positions.len() - nearest);
    let indexes =
        (0..nearest).chain((0..remainder).map(|i| nearest + spread_index(i, positions.len() - nearest, remainder)));
    let candidates = indexes
        .flat_map(|i| {
            let point = positions[i];
            orientations.iter().map(move |angle| posed(target, point, angle))
        })
        .collect();
    (candidates, nearest * orientations.len())
}

/// Only a held maximum-distance peer bounds a local neighbourhood. Minimum
/// distances exclude nearby material; alignments still leave an unbounded axis.
fn bounded_by_held_peer(rules: &[&RelativePlacementRule], reference: &str, held: &Poses) -> bool {
    rules.iter().any(|rule| {
        rule.kind == RelativePlacementKind::MaxDistance
            && rule.targets.iter().any(|target| target.reference != reference && held.contains_key(&target.reference))
    })
}

/// `_candidate_positions`: the ordinary grid (board or region), nearest first,
/// plus exact region boundary centres within the unused allowance.
pub fn candidate_positions(scene: &Scene, placement: &Placement, options: &PlannerOptions) -> Vec<Point> {
    let in_region = scene.rule(&placement.reference).is_some_and(|r| r.region.is_some());
    let orientations = scene.allowed_orientations(&placement.reference);
    // A pose need fit at one allowed orientation, not at every orientation.
    // Union the signed origin domains; the consuming search still checks the
    // complete pose against exact geometry and the original work budget.
    // Symmetric courtyards can produce the same bounds for multiple rotations.
    // The pose stream still tries every permitted angle at each sampled site.
    let domains: IndexSet<_> = orientations
        .iter()
        .map(|orientation| {
            let candidate = Placement { rotation_degrees: orientation.clone(), ..placement.clone() };
            scene.anchor_bounds(&candidate, options.edge_clearance_nm)
        })
        .collect();
    let step = options.grid_step_nm;
    let ceil_grid = |value: i64| (value + step - 1).div_euclid(step) * step;
    let mut points = IndexSet::new();
    for &(low_x, low_y, high_x, high_y) in &domains {
        let mut y = ceil_grid(low_y);
        while y <= high_y {
            let mut x = ceil_grid(low_x);
            while x <= high_x {
                points.insert(Point::new(x, y));
                x += step;
            }
            y += step;
        }
    }
    let mut coarse: Vec<Point> = points.into_iter().collect();
    let (px, py) = (placement.position.x_nm, placement.position.y_nm);
    coarse.sort_by_key(|p| ((p.x_nm - px).abs() + (p.y_nm - py).abs(), p.y_nm, p.x_nm));
    if !in_region {
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
    for &(low_x, low_y, high_x, high_y) in &domains {
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
    relative_group_sizes: Vec<usize>,
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

    /// Complete coupled relative groups before independent neighbours, with
    /// most-constrained-first ordering inside a group and exact forward checking.
    fn solve(&mut self, available: &IndexMap<usize, Vec<usize>>, current: &Poses) -> Result<Option<Poses>> {
        if available.is_empty() {
            return Ok((self.scene.relative_penalty(current, &self.rules)? == 0).then(|| current.clone()));
        }
        let reference = *available
            .iter()
            .min_by_key(|(item, indices)| {
                (Reverse(self.relative_group_sizes[**item]), indices.len(), **item, &self.references[**item])
            })
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

/// Sizes of connected components in this pack's internal relative-rule graph.
/// External anchors remain in `base`; independent neighbours stay movable and
/// participate in every exact courtyard/relative forward check.
fn relative_group_sizes(count: usize, edges: impl Iterator<Item = (usize, usize)>) -> Vec<usize> {
    let mut groups: Vec<usize> = (0..count).collect();
    for (left, right) in edges {
        let (keep, replace) = (groups[left], groups[right]);
        if keep != replace {
            for group in &mut groups {
                if *group == replace {
                    *group = keep;
                }
            }
        }
    }
    groups.iter().map(|group| groups.iter().filter(|other| *other == group).count()).collect()
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
    pack(scene, references, base, targets, options, allow_general, fine, None, contact_anchors, None)
}

/// Observation of the existing bounded search, never an additional search.
#[derive(Default, Debug)]
pub(crate) struct PackingDiagnostics {
    pub domains: IndexMap<String, (usize, usize)>,
    pub empty_reference: Option<String>,
    pub search_remaining: Option<i64>,
    pub passes: usize,
    pub fine: bool,
}

#[derive(Default)]
struct RepairDiagnostics {
    references: Vec<String>,
    packing: PackingDiagnostics,
}

impl PackingDiagnostics {
    pub(crate) fn value(&self, references: &[String], result: Option<bool>) -> copper_json::Value {
        use copper_json::{Object, Value};
        Value::Object(Object::from([
            ("selected_references".into(), Value::Array(references.iter().map(|r| Value::from(r.as_str())).collect())),
            (
                "outcome".into(),
                Value::from(match result {
                    None => "pending",
                    Some(true) => "placed",
                    Some(false) => "no_sampled_witness",
                }),
            ),
            ("passes".into(), Value::from(self.passes as i64)),
            ("final_pass".into(), Value::from(if self.fine { "fine" } else { "coarse" })),
            (
                "domains".into(),
                Value::Array(
                    self.domains
                        .iter()
                        .map(|(reference, (sampled, legal))| {
                            Value::Object(Object::from([
                                ("reference".into(), Value::from(reference.as_str())),
                                ("sampled".into(), Value::from(*sampled as i64)),
                                ("legal".into(), Value::from(*legal as i64)),
                            ]))
                        })
                        .collect(),
                ),
            ),
            ("empty_reference".into(), self.empty_reference.as_ref().map_or(Value::Null, |r| Value::from(r.as_str()))),
            ("search_remaining".into(), self.search_remaining.map_or(Value::Null, Value::from)),
        ]))
    }
}

impl RepairDiagnostics {
    fn value(&self, result: Option<bool>) -> copper_json::Value {
        self.packing.value(&self.references, result)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn pack_local_components_observed(
    scene: &Scene,
    references: &[String],
    base: &Poses,
    targets: &Poses,
    options: &PlannerOptions,
    diagnostics: &mut PackingDiagnostics,
) -> Result<Option<Poses>> {
    pack(scene, references, base, targets, options, true, false, None, &[], Some(diagnostics))
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
    mut diagnostics: Option<&mut PackingDiagnostics>,
) -> Result<Option<Poses>> {
    if let Some(diagnostic) = diagnostics.as_deref_mut() {
        diagnostic.passes += 1;
        diagnostic.fine = fine;
        diagnostic.domains.clear();
        diagnostic.empty_reference = None;
        diagnostic.search_remaining = None;
    }
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
    // A removed mutable anchor is no longer in `base`. Independent ordinary
    // domains can then miss every compatible pair, even though both members
    // can move. Construct these repair domains in dependency order, but retain
    // the caller's order for the bounded search and its deterministic ties.
    let mut joint_peers: IndexMap<&str, IndexSet<&str>> =
        references.iter().map(|r| (r.as_str(), IndexSet::new())).collect();
    if allow_general && !held_seeds.is_empty() {
        for rule in &rules {
            if rule.kind != RelativePlacementKind::MaxDistance
                || rule.targets.len() != 2
                || rule
                    .targets
                    .iter()
                    .any(|t| t.pad.is_none() || !in_references(&t.reference) || immutable.contains(&t.reference))
            {
                continue;
            }
            let (a, b) = (rule.targets[0].reference.as_str(), rule.targets[1].reference.as_str());
            if a != b {
                joint_peers[a].insert(b);
                joint_peers[b].insert(a);
            }
        }
    }
    let mut domain_order: Vec<&String> = references.iter().collect();
    if joint_peers.values().any(|peers| !peers.is_empty()) {
        domain_order.sort_by_key(|r| {
            (
                joint_peers[r.as_str()].is_empty(),
                !held_seeds.iter().any(|p| p.reference == **r),
                Reverse(joint_peers[r.as_str()].len()),
                Reverse(scene.footprint_area(&targets[*r])),
                references.iter().position(|original| original == *r).expect("member"),
            )
        });
    }
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
            for reference in &domain_order {
                let reference = *reference;
                let target = &targets[reference];
                let obstacles = local_obstacles(scene, reference, base);
                let own_rules = own_rules_of(reference);
                let relative = relative_candidates(scene, &candidate_rules, reference, target, base, options, 250_000)?;
                let unanchored_joint =
                    own_rules.iter().any(|rule| rule.targets.iter().all(|target| target.pad.is_some()))
                        && !bounded_by_held_peer(&own_rules, reference, base);
                let general: Candidates = if allow_general {
                    if unanchored_joint {
                        Box::new(
                            unanchored_candidates(scene, target, options, &scene.allowed_orientations(reference))
                                .0
                                .into_iter(),
                        )
                    } else {
                        general_candidates(scene, target.clone(), options, scene.allowed_orientations(reference))
                    }
                } else {
                    Box::new(std::iter::empty())
                };
                let peer_poses: Vec<&Placement> = joint_peers[reference.as_str()]
                    .iter()
                    .filter_map(|peer| domains.get(*peer))
                    .flat_map(|poses| poses.iter())
                    .collect();
                let propagated_budget = if peer_poses.is_empty() { 0 } else { probe_limit / 2 };
                let anchor_count =
                    peer_poses.len().min(options.exact_repair_candidates as usize).min(propagated_budget);
                let mut propagated = Vec::new();
                for i in 0..anchor_count {
                    let anchor = peer_poses[spread_index(i, peer_poses.len(), anchor_count)];
                    let held = with(base, anchor);
                    let proposals =
                        relative_candidates(scene, &candidate_rules, reference, target, &held, options, 250_000)?;
                    let allowance = (propagated_budget - propagated.len()) / (anchor_count - i);
                    let count = proposals.len().min(allowance);
                    propagated.extend((0..count).map(|j| proposals[spread_index(j, proposals.len(), count)].clone()));
                }
                let retained_poses: Vec<Placement> = if joint_peers[reference.as_str()].is_empty() {
                    Vec::new()
                } else {
                    held_seeds.iter().filter(|p| p.reference == *reference).map(|p| (*p).clone()).collect()
                };
                let ordinary_budget = probe_limit - propagated.len();
                let candidates = retained_poses
                    .into_iter()
                    .chain(bounded_local_candidates(relative, ordinary_budget, general))
                    .take(ordinary_budget)
                    .chain(propagated);
                let mut legal: Vec<Placement> = Vec::new();
                let mut seen: IndexSet<Placement> = IndexSet::new();
                let mut probed = 0i64;
                for (index, candidate) in candidates.enumerate() {
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
                if let Some(diagnostic) = diagnostics.as_deref_mut() {
                    diagnostic.domains.insert(reference.clone(), (probed as usize, legal.len()));
                    if legal.is_empty() {
                        diagnostic.empty_reference = Some(reference.clone());
                        diagnostic.search_remaining = None;
                    }
                }
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
            // Domain generation may follow movable anchors; search ordering
            // remains the original caller ordering, including its tie breaks.
            domains = references.iter().map(|r| (r.clone(), domains.shift_remove(r).expect("domain"))).collect();
        }
        Some(region) => {
            let refinable_set = region.refinable.clone();
            let mut remaining_by_ref = region.domain_remaining.clone();
            for reference in references {
                let mut legal = region.domains[reference].clone();
                let before = remaining_by_ref[reference];
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
                if let Some(diagnostic) = diagnostics.as_deref_mut() {
                    diagnostic
                        .domains
                        .insert(reference.clone(), ((before - remaining_by_ref[reference]) as usize, legal.len()));
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
        relative_group_sizes: relative_group_sizes(order.len(), pair_rules.keys().copied()),
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
    if let Some(diagnostic) = diagnostics.as_deref_mut() {
        diagnostic.search_remaining = Some(remaining);
    }
    if let Some(region) = state {
        region.search_remaining = remaining;
        region.heuristic_remaining = heuristic_remaining;
        return Ok(result);
    }
    let _ = has_state;
    if result.is_none() && refinable && !fine {
        // Preserve the pre-existing pad-MAX_DISTANCE fine behaviour.
        return pack(
            scene,
            references,
            base,
            targets,
            options,
            allow_general,
            true,
            None,
            contact_anchors,
            diagnostics,
        );
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
            diagnostics,
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
    let unanchored = local && !bounded_by_held_peer(&relative_rules, reference, placed);
    let mut wide_start = None;
    let mut general: Option<Candidates> = Some(if unanchored {
        let (candidates, nearest_count) = unanchored_candidates(scene, target, options, &orientations);
        wide_start = Some(nearest_count);
        Box::new(candidates.into_iter())
    } else {
        general_candidates(scene, target.clone(), options, orientations)
    });
    for step in [250_000, 100_000] {
        let fallback: Candidates = general.take().unwrap_or_else(|| Box::new(std::iter::empty()));
        let mut count = 0usize;
        let local_target = best.as_ref().map_or(target, |(_, pose)| pose).clone();
        let relative = relative_candidates(scene, &scene.relative, reference, &local_target, placed, options, step)?;
        // The wide boundary is an index into the ordinary fallback. Offset it
        // by the relative prefix before comparing to the combined probe index.
        // A saturated relative search samples only the initial fallback quota;
        // the boundary then remains beyond the shared budget.
        let coverage_start = wide_start.take().map(|nearest| relative.len().saturating_add(nearest));
        let candidates: Candidates = if local {
            bounded_local_candidates(relative, options.probe_limit(), fallback)
        } else {
            Box::new(relative.into_iter().chain(fallback))
        };
        for (probe, candidate) in candidates.enumerate() {
            if local && probe >= options.probe_limit() {
                break;
            }
            if coverage_start == Some(probe) {
                // A normal nearby success keeps its existing ranking. Distant
                // coverage only spends the reserved allowance on local failure.
                if best.as_ref().is_some_and(|(rank, _)| rank.0 == 0) {
                    break;
                }
                scene.progress(&format!("legalize_wide:{reference}"));
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
#[cfg(test)]
fn exact_repair(
    scene: &Scene,
    failed: &str,
    placed: &Poses,
    placed_order: &[String],
    targets: &Poses,
    options: &PlannerOptions,
) -> Result<Option<Poses>> {
    exact_repair_observed(scene, failed, placed, placed_order, targets, options, &mut RepairDiagnostics::default())
}

fn exact_repair_observed(
    scene: &Scene,
    failed: &str,
    placed: &Poses,
    placed_order: &[String],
    targets: &Poses,
    options: &PlannerOptions,
    diagnostic: &mut RepairDiagnostics,
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
    let unit = &units[failed];
    let complete_mutable_unit = unit.len() > 1
        && unit.len() <= options.exact_repair_limit as usize
        && unit.iter().all(|r| {
            targets.contains_key(r)
                && !options.fixed_references.contains(r)
                && !scene.rule(r).is_some_and(|rule| rule.fixed_position.is_some())
                && (r == failed || !placed.contains_key(r) || movable.contains(r.as_str()))
        })
        && !scene
            .board
            .rigid_clusters
            .iter()
            .any(|cluster| cluster.members.iter().any(|m| unit.contains(&m.reference)));
    if complete_mutable_unit {
        // A proximity anchor need not be a collision blocker (for example on
        // the opposite face). Keep a small movable unit together before other
        // blockers consume the same bounded repair slots. Its not-yet-placed
        // supports must participate too: moving the early anchor alone can
        // strand them again in a pocket that only fits part of the unit.
        repair.extend(neighbours.iter().filter(|r| unit.contains(**r)).map(|r| (*r).to_owned()));
        repair.extend(unit.iter().filter(|r| *r != failed && !placed.contains_key(*r)).cloned());
    }
    for reference in neighbours {
        if repair.len() == options.exact_repair_limit as usize {
            break;
        }
        if !repair.iter().any(|r| r == reference) {
            repair.push(reference.to_owned());
        }
    }
    let base: Poses = placed.iter().filter(|(r, _)| !repair.contains(r)).map(|(r, p)| (r.clone(), p.clone())).collect();
    let anchors: Vec<Placement> = repair.iter().filter_map(|r| placed.get(r).cloned()).collect();
    diagnostic.references = repair.clone();
    scene.progress(&format!(
        "repair_pack_started:{failed}:{}",
        copper_json::Dumps::new().dumps(&diagnostic.value(None))
    ));
    let result =
        pack(scene, &repair, &base, targets, options, true, false, None, &anchors, Some(&mut diagnostic.packing))?;
    scene.progress(&format!(
        "repair_pack_finished:{failed}:{}",
        copper_json::Dumps::new().dumps(&diagnostic.value(Some(result.is_some())))
    ));
    Ok(result)
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
    pub bounded_packing: Option<copper_json::Value>,
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
    legalize_with_reservations(scene, targets, original, &Poses::new(), options, spacing, seed)
}

/// Carry early region choices as mutable accepted poses, not source locks.
pub(crate) fn legalize_with_reservations(
    scene: &Scene,
    targets: &Poses,
    original: &Poses,
    reservations: &Poses,
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
    for (reference, candidate) in reservations {
        if !scene.legal(candidate, &placed)? {
            return Ok(Legalized::Failed(LegalizationFailure {
                message: format!(
                    "regional reservation {} violates held geometry",
                    copper_json::python_str_repr(reference)
                ),
                failed_reference: reference.clone(),
                accepted: placed,
                bounded_packing: None,
            }));
        }
        placed.insert(reference.clone(), candidate.clone());
    }
    let adjacency = adjacency(scene);
    // Module provenance supplies locality costs, not first claim on free area.
    // Keep those weights for pose ranking while ordering by physical/source
    // priority and degree without the automatically generated hierarchy clique.
    let ordering_adjacency = adjacency_with_hierarchy(scene, false);
    let priorities = component_priorities(scene);
    let mut movable: Vec<String> = targets.keys().filter(|r| !fixed.contains_key(*r)).cloned().collect();
    movable.sort_by_key(|r| {
        (
            -priorities.get(r).copied().unwrap_or(0),
            -ordering_adjacency.get(r).map_or(0, |n| n.values().sum::<i64>()),
            -scene.footprint_area(&targets[r]),
            r.clone(),
        )
    });
    let movable = relative_cluster_order(scene, &movable, targets);
    let mut placed_order: Vec<String> = reservations.keys().cloned().collect();
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
            if !placed.contains_key(&target.reference) && !rigid.contains(target.reference.as_str()) {
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
            let mut diagnostic = RepairDiagnostics::default();
            match exact_repair_observed(scene, reference, &placed, &placed_order, targets, options, &mut diagnostic)? {
                Some(repaired) => {
                    // A complete ordinary-unit repair can place later members
                    // early. Keep them mutable and available to later repairs.
                    placed_order
                        .extend(repaired.keys().filter(|r| *r != reference && !placed.contains_key(*r)).cloned());
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
                            bounded_packing: Some(diagnostic.value(Some(false))),
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
    use crate::scene::without;
    use copper_pcb::board::{
        ComponentPlacementRule, PhysicalBoard, PlacementRegion, PlacementTarget, RelativePlacementKind,
        RigidPlacementCluster, RigidPlacementMember,
    };
    use copper_physical::BoardOutline;
    use copper_physical::footprint::footprint_geometry_digest;

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

    fn mutable_reservation_board() -> PhysicalBoard {
        let mut board = board(
            "mutable-reservation",
            30.0,
            30.0,
            vec![one_pad()],
            vec![back("J", "test/one-pad", 10.0, 10.0), back("K", "test/one-pad", 10.0, 10.0)],
            Vec::new(),
        );
        board.regions = vec![region("flexible", 9.45, 9.45, 4.1, 1.1), region("tight", 9.45, 9.45, 1.1, 1.1)];
        board.placement_rules = [("J", "flexible"), ("K", "tight")]
            .iter()
            .map(|(reference, region)| ComponentPlacementRule {
                region: Some((*region).to_owned()),
                allowed_orientations: vec![Decimal::from_i64(0)],
                ..ComponentPlacementRule::new(reference)
            })
            .collect();
        board
    }

    #[test]
    fn packing_observation_distinguishes_empty_domain_from_incompatible_nonempty_domains() {
        for failure in ["none", "empty", "incompatible"] {
            let mut board = mutable_reservation_board();
            if failure == "empty" {
                board.regions[1] = region("tight", 40.0, 40.0, 1.1, 1.1);
            } else if failure == "incompatible" {
                board.regions[0] = region("flexible", 9.45, 9.45, 1.1, 1.1);
            }
            let options = repair_options();
            let scene = Scene::new(&board, options.policy());
            let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            let references = ["J".to_owned(), "K".to_owned()];
            let base = Poses::new();
            let unobserved =
                pack_local_components(&scene, &references, &base, &targets, &options, true, false, &[]).unwrap();
            let mut diagnostic = PackingDiagnostics::default();
            let observed =
                pack_local_components_observed(&scene, &references, &base, &targets, &options, &mut diagnostic)
                    .unwrap();
            assert_eq!(observed, unobserved, "observations must not alter search decisions");
            assert!(diagnostic.domains["J"].1 > 0);
            assert!(diagnostic.domains.values().all(|(probes, _)| *probes <= options.probe_limit()));
            if failure == "empty" {
                assert!(observed.is_none());
                assert_eq!(diagnostic.empty_reference.as_deref(), Some("K"));
                assert_eq!(diagnostic.domains["K"].1, 0);
                assert_eq!(diagnostic.search_remaining, None, "joint search never started");
            } else {
                assert!(diagnostic.empty_reference.is_none());
                assert!(diagnostic.domains["K"].1 > 0);
                assert!(diagnostic.search_remaining.is_some_and(|remaining| remaining > 0));
                assert_eq!(observed.is_some(), failure == "none");
            }
        }
    }

    #[test]
    fn early_region_reservation_can_move_in_later_exact_repair() {
        let board = mutable_reservation_board();
        let options = repair_options();
        let scene = Scene::new(&board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let reservations: Poses = [("J".to_owned(), targets["J"].clone())].into_iter().collect();
        let Legalized::Placed(placed, repairs) =
            legalize_with_reservations(&scene, &targets, &targets, &reservations, &options, &spacing, 0).unwrap()
        else {
            panic!("reservation must remain movable");
        };
        assert!(repairs > 0);
        assert_ne!(placed["J"].position, reservations["J"].position);
        assert_eq!(placed["K"].position, pt(10.0, 10.0));
        assert!(!options.fixed_references.contains("J"));
        assert!(scene.checker.placement_solution_is_legal(&placed).unwrap());
    }

    #[test]
    fn region_reservations_are_revalidated_against_regions_and_final_fixed_geometry() {
        let board = mutable_reservation_board();
        let mut options = repair_options();
        options.fixed_references.insert("K".to_owned());
        let scene = Scene::new(&board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        for position in [pt(20.0, 20.0), pt(10.0, 10.0)] {
            let mut pose = targets["J"].clone();
            pose.position = position;
            let reservations: Poses = [("J".to_owned(), pose)].into_iter().collect();
            let Legalized::Failed(failure) =
                legalize_with_reservations(&scene, &targets, &targets, &reservations, &options, &spacing, 0).unwrap()
            else {
                panic!("invalid reservation must reject the seed");
            };
            assert_eq!(failure.failed_reference, "J");
            assert_eq!(failure.accepted.len(), 1);
            assert_eq!(failure.accepted["K"], targets["K"]);
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

    fn trapped_relative_unit() -> PhysicalBoard {
        let small = one_pad();
        let blocker = footprint("blocker", Vec::new(), 6.0, 6.0);
        let mut board = board(
            "relative-unit-repair",
            30.0,
            30.0,
            vec![small, blocker],
            vec![
                placement("U", "test/one-pad", 10.0, 10.0),
                back("C", "test/one-pad", 20.0, 20.0),
                back("X", "blocker", 10.0, 10.0),
            ],
            Vec::new(),
        );
        // U is on the opposite face, so it is a proximity anchor but never a
        // physical blocker of C. X blocks every close pose and its region does
        // not let it move far enough to clear one. Moving U and C together works.
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
        board.regions.push(region("blocker-slot", 6.95, 6.95, 6.1, 6.1));
        board.placement_rules.push(ComponentPlacementRule {
            region: Some("blocker-slot".to_owned()),
            ..ComponentPlacementRule::new("X")
        });
        board
    }

    #[test]
    fn unanchored_pin_relative_unit_can_use_distant_board_space_within_probe_budget() {
        let mut board = board(
            "distant-relative-unit",
            100.0,
            44.0,
            vec![one_pad(), footprint("held-bank", Vec::new(), 68.0, 40.0)],
            vec![
                placement("U", "test/one-pad", 10.0, 22.0),
                placement("C", "test/one-pad", 12.0, 22.0),
                placement("K", "held-bank", 36.0, 22.0),
            ],
            Vec::new(),
        );
        board.placement_rules.push(fixed("K", 36.0, 22.0));
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
        let options = PlannerOptions::default().validated().unwrap();
        assert_eq!(options.probe_limit(), 6144);
        let scene = Scene::new(&board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let held: Poses = [("K".to_owned(), targets["K"].clone())].into_iter().collect();
        assert!(scene.legal(&held["K"], &Poses::new()).unwrap());
        assert!(
            relative_candidates(&scene, &scene.relative, "U", &targets["U"], &held, &options, 250_000)
                .unwrap()
                .is_empty(),
            "the companion has not been placed, so there is no local anchor"
        );
        let prefix: Vec<_> =
            general_candidates(&scene, targets["U"].clone(), &options, scene.allowed_orientations("U"))
                .take(options.probe_limit())
                .collect();
        assert_eq!(prefix.len(), options.probe_limit());
        assert!(
            prefix.iter().all(|pose| !scene.legal(pose, &held).unwrap()),
            "the old nearest prefix misses all free material"
        );
        let witness = with(
            &with(&held, &placement("U", "test/one-pad", 90.0, 22.0)),
            &placement("C", "test/one-pad", 92.0, 22.0),
        );
        assert_eq!(scene.penalty(&witness).unwrap(), 0);
        assert!(scene.checker.placement_solution_is_legal(&witness).unwrap(), "the distant pair is exactly legal");
        let choice = best_legal_choice(
            &scene,
            "U",
            &targets["U"],
            &held,
            &IndexMap::new(),
            &options,
            &spacing,
            0,
            Some(options.legalization_candidates as usize),
        )
        .unwrap()
        .expect("bounded coverage must reach the free board area for an unanchored unit");
        assert!(scene.legal(&choice, &held).unwrap());
        assert!(choice.position.x_nm > mm(70.0));
        let accepted = with(&held, &choice);
        let cap = best_legal_choice(
            &scene,
            "C",
            &targets["C"],
            &accepted,
            &IndexMap::new(),
            &options,
            &spacing,
            0,
            Some(options.legalization_candidates as usize),
        )
        .unwrap()
        .expect("companion placement");
        let result = with(&accepted, &cap);
        assert_eq!(scene.penalty(&result).unwrap(), 0);
        assert!(scene.checker.placement_solution_is_legal(&result).unwrap());
        assert_eq!(result["K"], targets["K"]);
    }

    #[test]
    fn joint_repair_domains_relocate_a_local_ic_only_pocket_to_a_distant_legal_pair() {
        let mut board = board(
            "unanchored-pair-repair-domain",
            100.0,
            44.0,
            vec![one_pad()],
            vec![placement("U", "test/one-pad", 10.0, 22.0), placement("C", "test/one-pad", 12.0, 22.0)],
            Vec::new(),
        );
        // A 1.1 mm square local pocket holds this 1 mm IC, but not its
        // capacitor. The remaining free material starts beyond X=70 mm.
        for (name, x, y, width, height) in [
            ("above", 0.0, 0.0, 70.0, 21.45),
            ("below", 0.0, 22.55, 70.0, 21.45),
            ("left", 0.0, 21.45, 9.45, 1.1),
            ("right", 10.55, 21.45, 59.45, 1.1),
        ] {
            board.keepouts.push(
                copper_physical::PlacementKeepout::new(name, region(name, x, y, width, height).outline, None, None)
                    .unwrap(),
            );
        }
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
        let options = PlannerOptions::default().validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let chosen = best_legal_choice(
            &scene,
            "U",
            &targets["U"],
            &Poses::new(),
            &IndexMap::new(),
            &options,
            &spacing,
            0,
            Some(options.legalization_candidates as usize),
        )
        .unwrap()
        .unwrap();
        assert_eq!(chosen.position, pt(10.0, 22.0), "the normal local IC choice succeeds");
        let placed = [("U".to_owned(), chosen)].into_iter().collect();
        let witness: Poses = [placement("U", "test/one-pad", 90.0, 22.0), placement("C", "test/one-pad", 92.0, 22.0)]
            .into_iter()
            .map(|p| (p.reference.clone(), p))
            .collect();
        assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
        assert_eq!(scene.penalty(&witness).unwrap(), 0);
        let repaired = exact_repair(&scene, "C", &placed, &["U".to_owned()], &targets, &options)
            .unwrap()
            .expect("joint domains must reach the distant compatible pair");
        assert!(repaired["U"].position.x_nm > mm(70.0));
        assert!(scene.checker.placement_solution_is_legal(&repaired).unwrap());
        assert_eq!(scene.penalty(&repaired).unwrap(), 0);
        let Legalized::Placed(result, _) = legalize(&scene, &targets, &targets, &options, &spacing, 0).unwrap() else {
            panic!("the same bounded repair must recover ordinary legalization")
        };
        assert!(scene.checker.placement_solution_is_legal(&result).unwrap());
        assert_eq!(scene.penalty(&result).unwrap(), 0);
    }

    #[test]
    fn joint_repair_domains_keep_valid_held_anchor_rotations_and_probe_allowance() {
        let mut board = board(
            "joint-held-anchor",
            30.0,
            12.0,
            vec![one_pad(), footprint("chip", vec![pad("1", -1.0, 0.0, 0.3, 0.3)], 2.0, 2.0)],
            vec![
                rotated("U", "chip", 5.0, 5.0, 90),
                rotated("C", "test/one-pad", 5.0, 5.0, 270),
                placement("K", "test/one-pad", 25.0, 5.0),
            ],
            Vec::new(),
        );
        board
            .regions
            .push(PlacementRegion { side: Some(BoardSide::Front), ..region("cap-pocket", 14.575, 7.075, 1.1, 1.1) });
        board.placement_rules = vec![
            ComponentPlacementRule {
                allowed_orientations: vec![Decimal::from_i64(90)],
                ..ComponentPlacementRule::new("U")
            },
            ComponentPlacementRule {
                region: Some("cap-pocket".to_owned()),
                allowed_orientations: vec![Decimal::from_i64(270)],
                ..ComponentPlacementRule::new("C")
            },
            fixed("K", 25.0, 5.0),
        ];
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 1.3));
        let options = PlannerOptions::default().validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let held = rotated("U", "chip", 15.125, 5.375, 90);
        let base: Poses = [("K".to_owned(), targets["K"].clone())].into_iter().collect();
        let witness = with(&with(&base, &held), &rotated("C", "test/one-pad", 15.125, 7.625, 270));
        assert!(
            scene.checker.placement_solution_is_legal(&witness).unwrap(),
            "{:?}",
            copper_drc::legality::placement_rejection_reasons(&board, &witness).unwrap()
        );
        assert_eq!(scene.penalty(&witness).unwrap(), 0);
        let mut diagnostic = PackingDiagnostics::default();
        let result = pack(
            &scene,
            &["C".to_owned(), "U".to_owned()],
            &base,
            &targets,
            &options,
            true,
            false,
            None,
            &[held.clone()],
            Some(&mut diagnostic),
        )
        .unwrap()
        .expect("the off-grid held anchor must remain a mutable domain candidate");
        assert_eq!(result["U"], held);
        assert_eq!(result["K"], base["K"], "immutable base cannot move");
        assert_eq!(result["C"].rotation_degrees, Decimal::from_i64(270));
        assert!(scene.checker.placement_solution_is_legal(&result).unwrap());
        assert_eq!(scene.penalty(&result).unwrap(), 0);
        assert_eq!(diagnostic.domains.len(), 2);
        assert!(diagnostic.domains.values().all(|(probed, legal)| *probed <= options.probe_limit() && *legal > 0));
        assert!(diagnostic.search_remaining.is_some_and(|remaining| remaining >= 0));
    }

    fn hierarchy_pocket_board() -> (PhysicalBoard, PlannerOptions) {
        let mut poses = vec![placement("J", "large-header", 5.0, 5.0)];
        for (i, (x, y)) in
            [(3.0, 3.0), (5.0, 3.0), (7.0, 3.0), (3.0, 5.0), (5.0, 5.0), (7.0, 5.0), (3.0, 7.0), (5.0, 7.0)]
                .into_iter()
                .enumerate()
        {
            poses.push(placement(&format!("M/R{i}"), "test/one-pad", x, y));
        }
        let mut board = board(
            "hierarchy-packing",
            30.0,
            10.0,
            vec![one_pad(), footprint("large-header", Vec::new(), 8.0, 8.0)],
            poses,
            Vec::new(),
        );
        board.keepouts.push(
            copper_physical::PlacementKeepout::new(
                "right-lower",
                region("right-lower", 10.0, 3.0, 20.0, 7.0).outline,
                None,
                None,
            )
            .unwrap(),
        );
        board.placement_groups.push(
            copper_pcb::board::PlacementGroup::new(
                "module:M",
                &(0..8).map(|i| format!("M/R{i}")).collect::<Vec<_>>(),
                Some("M/R0"),
                100,
                "hierarchy",
            )
            .unwrap(),
        );
        let options = PlannerOptions {
            legalization_candidates: 1,
            exact_repair_limit: 2,
            edge_clearance_nm: 250_000,
            analytical_iterations: 0,
            refinement_passes: 0,
            escape_spacing_passes: 0,
            ..PlannerOptions::default()
        }
        .validated()
        .unwrap();
        (board, options)
    }

    #[test]
    fn relative_companion_area_order_preserves_the_only_large_pocket() {
        // A free opposite-face anchor has a star larger than the repair budget.
        // Eight small supports fit the narrow top shelf; the large support only
        // fits the left pocket. Raw radius ordering fills that pocket first.
        let mut poses = vec![back("U", "anchor", 5.0, 5.0), placement("L", "large", 5.0, 5.0)];
        for i in 0..8 {
            poses.push(placement(&format!("S{i}"), "test/one-pad", 5.0, 5.0));
        }
        let mut board = board(
            "relative-area-packing",
            30.0,
            10.0,
            vec![
                one_pad(),
                footprint("anchor", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 9.0, 9.0),
                footprint("large", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 8.0, 8.0),
            ],
            poses,
            Vec::new(),
        );
        board.keepouts.push(
            copper_physical::PlacementKeepout::new(
                "right-lower",
                region("right-lower", 10.0, 3.0, 20.0, 7.0).outline,
                None,
                None,
            )
            .unwrap(),
        );
        board.relative_rules.push(max_distance(("L", "1"), ("U", "1"), 10.0));
        for i in 0..8 {
            board.relative_rules.push(max_distance((&format!("S{i}"), "1"), ("U", "1"), 9.0));
        }
        let options = PlannerOptions {
            edge_clearance_nm: 250_000,
            analytical_iterations: 0,
            refinement_passes: 0,
            escape_spacing_passes: 0,
            ..PlannerOptions::default()
        }
        .validated()
        .unwrap();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let movable = targets.keys().cloned().collect::<Vec<_>>();
        let order = relative_cluster_order(&scene, &movable, &targets);
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let Legalized::Placed(result, _) = legalize(&scene, &targets, &targets, &options, &spacing, 0).unwrap() else {
            panic!("area ordering must fit the star without a larger repair allowance")
        };
        assert_eq!(&order[..2], &["U", "L"], "larger support needs its pocket before smaller tighter-radius supports");
        assert!(scene.checker.placement_solution_is_legal(&result).unwrap());
        assert_eq!(scene.penalty(&result).unwrap(), 0);
    }

    #[test]
    fn relative_companion_area_order_uses_courtyards_and_exact_deterministic_ratios() {
        for (large_radius, small_radius, first) in [
            (mm(2.5), mm(2.0), "L"),
            (mm(6.0), mm(5.0), "L"),
            // Equal area-normalized radii retain the old tighter-radius tie.
            (mm(4.0), mm(2.0), "S"),
            // Squaring and comparing these radii must not overflow i128.
            (i64::MAX, i64::MAX, "L"),
        ] {
            let mut large = footprint("large-court", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 1.0, 1.0);
            large.courtyard = vec![pt(-1.0, -1.0), pt(-1.0, 1.0), pt(1.0, 1.0), pt(1.0, -1.0)];
            let mut board = board(
                "companion-ratios",
                30.0,
                30.0,
                vec![one_pad(), large, footprint("anchor", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 5.0, 5.0)],
                vec![
                    placement("U", "anchor", 15.0, 15.0),
                    placement("L", "large-court", 10.0, 10.0),
                    placement("T", "test/one-pad", 10.0, 10.0),
                    placement("S", "test/one-pad", 10.0, 10.0),
                ],
                Vec::new(),
            );
            for (reference, radius) in [("L", large_radius), ("S", small_radius), ("T", small_radius)] {
                let mut rule = max_distance((reference, "1"), ("U", "1"), 1.0);
                rule.distance_nm = Some(radius);
                board.relative_rules.push(rule);
            }
            let options = PlannerOptions::default().validated().unwrap();
            let scene = Scene::new(&board, options.policy());
            let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            let movable = targets.keys().cloned().collect::<Vec<_>>();
            let ordered = relative_cluster_order(&scene, &movable, &targets);
            assert_eq!(ordered[0], "U", "anchor selection is unchanged");
            assert_eq!(ordered[1], first);
            assert!(ordered.iter().position(|r| r == "S") < ordered.iter().position(|r| r == "T"));
            // Fixed/rigid anchors enter this function outside the movable list.
            // They must remain outside it, with identical companion ordering.
            let companions = movable.into_iter().filter(|r| r != "U").collect::<Vec<_>>();
            assert_eq!(relative_cluster_order(&scene, &companions, &targets), ordered[1..]);
        }
    }

    fn legalize_hierarchy_pocket(board: &PhysicalBoard, options: &PlannerOptions) -> Legalized {
        let scene = Scene::new(board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        legalize(&scene, &targets, &targets, options, &spacing, 0).unwrap()
    }

    #[test]
    fn hierarchy_locality_must_not_strand_a_large_root_component_behind_module_passives() {
        let (board, options) = hierarchy_pocket_board();
        let scene = Scene::new(&board, options.policy());
        assert_eq!(
            adjacency(&scene)["M/R0"]["M/R1"],
            100,
            "logical grouping still contributes its original locality cost"
        );
        let mut flat = board.clone();
        flat.placement_groups.clear();
        let witness = match legalize_hierarchy_pocket(&flat, &options) {
            Legalized::Placed(poses, _) => poses,
            Legalized::Failed(failure) => panic!(
                "flat witness: {} after {} poses: {}",
                failure.failed_reference,
                failure.accepted.len(),
                failure.message
            ),
        };
        assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&witness).unwrap());
        let Legalized::Placed(result, _) = legalize_hierarchy_pocket(&board, &options) else {
            panic!("logical namespace must not give tiny members first claim on the only large pocket")
        };
        assert_eq!(result["J"], witness["J"]);
        assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&result).unwrap());
    }

    #[test]
    fn authored_group_and_component_priorities_still_control_legalization_order() {
        let (mut board, options) = hierarchy_pocket_board();
        board.placement_groups[0].source = "constraint".to_owned();
        assert!(
            matches!(legalize_hierarchy_pocket(&board, &options), Legalized::Failed(ref f) if f.failed_reference == "J"),
            "an authored group priority must retain its preference even when this bounded layout fails"
        );
        board.placement_rules.push(ComponentPlacementRule { priority: 150, ..ComponentPlacementRule::new("J") });
        let Legalized::Placed(result, _) = legalize_hierarchy_pocket(&board, &options) else {
            panic!("higher authored component priority gets the pocket")
        };
        assert!(Scene::new(&board, options.policy()).checker.placement_solution_is_legal(&result).unwrap());
    }

    #[test]
    fn unanchored_coverage_is_bounded_unique_and_keeps_allowed_rotation_cycles() {
        for width in [8.0, 100.0] {
            for angles in [vec![90], vec![0, 90, 270], vec![0, 90, 180, 270]] {
                let target = placement("U", "test/one-pad", 4.0, 4.0);
                let mut board = board("coverage-budget", width, 8.0, vec![one_pad()], vec![target.clone()], Vec::new());
                let orientations: Vec<_> = angles.into_iter().map(Decimal::from_i64).collect();
                board.placement_rules.push(ComponentPlacementRule {
                    allowed_orientations: orientations.clone(),
                    ..ComponentPlacementRule::new("U")
                });
                let options =
                    PlannerOptions { legalization_candidates: 1, ..PlannerOptions::default() }.validated().unwrap();
                let scene = Scene::new(&board, options.policy());
                let (candidates, nearest) = unanchored_candidates(&scene, &target, &options, &orientations);
                assert!(candidates.len() <= options.probe_limit());
                assert_eq!(candidates.iter().collect::<IndexSet<_>>().len(), candidates.len());
                assert_eq!(candidates.len() % orientations.len(), 0);
                assert_eq!(nearest % orientations.len(), 0);
                for poses in candidates.chunks(orientations.len()) {
                    for (pose, angle) in poses.iter().zip(&orientations) {
                        assert_eq!(&pose.rotation_degrees, angle);
                        assert_eq!(pose.position, poses[0].position);
                    }
                }
                let ordinary: Vec<_> =
                    general_candidates(&scene, target.clone(), &options, orientations.clone()).collect();
                assert_eq!(&candidates[..nearest], &ordinary[..nearest], "nearby order is unchanged");
                assert_eq!(
                    candidates.last().unwrap().position,
                    ordinary.last().unwrap().position,
                    "coverage reaches the far end without losing orientations"
                );
                if ordinary.len() <= options.probe_limit() {
                    assert_eq!(candidates, ordinary, "small domains retain the complete old stream");
                }
                assert_eq!(unanchored_candidates(&scene, &target, &options, &orientations), (candidates, nearest));
            }
        }
    }

    #[test]
    fn unanchored_search_keeps_bounded_poses_when_a_rotation_cycle_exceeds_the_budget() {
        let target = placement("U", "test/one-pad", 10.0, 10.0);
        let mut board = board(
            "many-orientations",
            30.0,
            30.0,
            vec![one_pad()],
            vec![target.clone(), placement("C", "test/one-pad", 12.0, 10.0)],
            Vec::new(),
        );
        let orientations: Vec<_> = (0..65).map(Decimal::from_i64).collect();
        board.placement_rules.push(ComponentPlacementRule {
            allowed_orientations: orientations.clone(),
            ..ComponentPlacementRule::new("U")
        });
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
        let options = PlannerOptions { legalization_candidates: 1, ..PlannerOptions::default() }.validated().unwrap();
        assert_eq!(options.probe_limit(), 64);
        let scene = Scene::new(&board, options.policy());
        let (candidates, nearest) = unanchored_candidates(&scene, &target, &options, &orientations);
        let expected: Vec<_> =
            general_candidates(&scene, target.clone(), &options, orientations).take(options.probe_limit()).collect();
        assert_eq!(candidates, expected, "an incomplete rotation cycle must retain the old bounded prefix");
        assert_eq!(nearest, candidates.len(), "there is no separate wide-search boundary");
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let choice =
            best_legal_choice(&scene, "U", &target, &Poses::new(), &IndexMap::new(), &options, &spacing, 0, Some(1))
                .unwrap()
                .expect("a clear board must still admit the first permitted pose");
        assert_eq!(choice, target);
        assert!(scene.legal(&choice, &Poses::new()).unwrap());
    }

    #[test]
    fn unanchored_nearby_success_keeps_the_existing_choice_before_distant_coverage() {
        let mut board = board(
            "nearby-choice",
            100.0,
            44.0,
            vec![one_pad()],
            vec![placement("U", "test/one-pad", 80.0, 20.0), placement("C", "test/one-pad", 82.0, 20.0)],
            Vec::new(),
        );
        board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
        let options = PlannerOptions::default().validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        for seed in 0..4 {
            let pose = best_legal_choice(
                &scene,
                "U",
                &board.placements[0],
                &Poses::new(),
                &IndexMap::new(),
                &options,
                &spacing,
                seed,
                Some(96),
            )
            .unwrap()
            .unwrap();
            // With no held neighbours, the old nearest 96 poses are 24 sites;
            // their lowest-y tie winner is three grid steps above the target.
            // Distant sampling contains lower-y sites and must not replace it.
            assert_eq!(pose.position, pt(80.0, 17.0));
            assert_eq!(pose.rotation_degrees, Decimal::from_i64(0));
            assert!(scene.legal(&pose, &Poses::new()).unwrap());
        }
    }

    #[test]
    fn relative_probes_do_not_shorten_the_nearby_ordinary_domain() {
        let target = placement("U", "test/one-pad", 50.0, 20.0);
        let peer = placement("P", "test/one-pad", 4.0, 60.0);
        let mut board =
            board("relative-prefix", 80.0, 70.0, vec![one_pad()], vec![target.clone(), peer.clone()], Vec::new());
        board.placement_rules.push(ComponentPlacementRule {
            fixed_rotation_degrees: Some(Decimal::from_i64(0)),
            ..ComponentPlacementRule::new("U")
        });
        let mut exclusion = max_distance(("P", "1"), ("U", "1"), 2.0);
        exclusion.kind = RelativePlacementKind::MinDistance;
        board.relative_rules.push(exclusion);
        let options = PlannerOptions { legalization_candidates: 1, ..PlannerOptions::default() }.validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let held: Poses = [(peer.reference.clone(), peer)].into_iter().collect();
        let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
        let selected = best_legal_choice(&scene, "U", &target, &held, &IndexMap::new(), &options, &spacing, 0, None)
            .unwrap()
            .unwrap();
        // Eight exclusion-ring probes precede the 48 nearby ordinary probes.
        // The first Y=15 site is in that nearby prefix, after combined probe 48.
        assert_eq!(selected.position, pt(50.0, 15.0));
        assert_eq!(scene.penalty(&with(&held, &selected)).unwrap(), 0);
    }

    #[test]
    fn local_repair_moves_the_complete_small_relative_unit_when_its_anchor_is_not_a_blocker() {
        let board = trapped_relative_unit();
        let options = repair_options();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let placed = without(&targets, "C");
        let order = vec!["U".to_owned(), "X".to_owned()];
        for pose in placed.values() {
            assert!(scene.legal(pose, &without(&placed, &pose.reference)).expect("held geometry"));
        }
        let blocker_pack = pack_local_components(
            &scene,
            &["C".to_owned(), "X".to_owned()],
            &without(&placed, "X"),
            &targets,
            &options,
            true,
            false,
            &[targets["X"].clone()],
        )
        .expect("blocker-only repair");
        assert!(blocker_pack.is_none(), "the old two-member neighborhood cannot solve the pair");
        let result = exact_repair(&scene, "C", &placed, &order, &targets, &options)
            .expect("repair")
            .expect("the same two-member budget must include the movable proximity anchor");
        assert_ne!(result["U"].position, targets["U"].position);
        assert_eq!(result["X"], targets["X"]);
        assert_eq!(scene.penalty(&result).expect("relative constraints"), 0);
        assert!(scene.checker.placement_solution_is_legal(&result).expect("complete legality"));
    }

    #[test]
    fn fixed_rigid_or_oversized_relative_units_do_not_displace_actual_blocker_selection() {
        for exclusion in ["fixed", "rigid", "oversized"] {
            let mut board = trapped_relative_unit();
            let mut options = repair_options();
            match exclusion {
                "fixed" => {
                    options.fixed_references.insert("U".to_owned());
                }
                "rigid" => {
                    let fp = &board.footprints["test/one-pad"];
                    board.rigid_clusters.push(
                        RigidPlacementCluster {
                            name: "held-macro".to_owned(),
                            anchor_reference: "U".to_owned(),
                            anchor_pad: None,
                            members: vec![
                                RigidPlacementMember::new(
                                    "U",
                                    &fp.name,
                                    &footprint_geometry_digest(fp),
                                    pt(0.0, 0.0),
                                    &Decimal::from_i64(0),
                                )
                                .expect("member"),
                            ],
                            source: "repair regression".to_owned(),
                            allowed_rotations: vec![Decimal::from_i64(0)],
                            keepouts: Vec::new(),
                            internal_clearance_nm: None,
                            edge_openings: Vec::new(),
                        }
                        .normalized()
                        .expect("cluster"),
                    );
                }
                "oversized" => {
                    board.placements.push(placement("Y", "test/one-pad", 20.0, 5.0));
                    board.relative_rules.push(max_distance(("U", "1"), ("Y", "1"), 20.0));
                }
                _ => unreachable!(),
            }
            let scene = Scene::new(&board, options.policy());
            let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            let placed = without(&targets, "C");
            let order = placed.keys().cloned().collect::<Vec<_>>();
            assert!(
                exact_repair(&scene, "C", &placed, &order, &targets, &options).expect("repair").is_none(),
                "{exclusion}"
            );
            assert_eq!(placed, without(&targets, "C"), "failed repair leaves the held base unchanged");
        }
    }

    #[test]
    fn ordinary_unit_repair_includes_unplaced_supports_before_unrelated_blockers() {
        let mut board = trapped_relative_unit();
        for reference in ["C2", "C3", "C4"] {
            board.placements.push(back(reference, "test/one-pad", 20.0, 20.0));
            board.relative_rules.push(max_distance((reference, "1"), ("U", "1"), 2.0));
        }
        let original = board.clone();
        let options = PlannerOptions { exact_repair_limit: 5, ..PlannerOptions::default() }.validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let held: Poses = ["U", "X"].into_iter().map(|r| (r.to_owned(), targets[r].clone())).collect();
        let mut witness = targets.clone();
        for (reference, position) in [
            ("U", pt(20.0, 15.0)),
            ("C", pt(20.0, 16.5)),
            ("C2", pt(18.5, 15.0)),
            ("C3", pt(21.5, 15.0)),
            ("C4", pt(20.0, 13.5)),
        ] {
            witness.get_mut(reference).unwrap().position = position;
        }
        assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
        assert_eq!(scene.penalty(&witness).unwrap(), 0);
        let repaired = exact_repair(&scene, "C", &held, &["U".to_owned(), "X".to_owned()], &targets, &options)
            .unwrap()
            .expect("the five-member unit has a legal joint placement away from the trapped anchor");
        assert_eq!(repaired.len(), targets.len(), "all three future supports participate in the same witness");
        assert_eq!(repaired["X"], held["X"], "an unrelated blocker must not consume the unit's bounded slots");
        assert_ne!(repaired["U"], held["U"]);
        assert!(scene.checker.placement_solution_is_legal(&repaired).unwrap());
        assert_eq!(scene.penalty(&repaired).unwrap(), 0);
        assert_eq!(board, original);
    }

    #[test]
    fn ordinary_unit_repair_with_an_impossible_future_support_leaves_held_poses_untouched() {
        let mut board = trapped_relative_unit();
        board.placements.push(back("C2", "test/one-pad", 10.0, 10.0));
        board.relative_rules.push(max_distance(("C2", "1"), ("U", "1"), 2.0));
        board.regions.push(region("impossible-support", 9.75, 9.75, 0.5, 0.5));
        board.placement_rules.push(ComponentPlacementRule {
            region: Some("impossible-support".to_owned()),
            ..ComponentPlacementRule::new("C2")
        });
        let options = PlannerOptions { exact_repair_limit: 3, ..PlannerOptions::default() }.validated().unwrap();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let held: Poses = ["U", "X"].into_iter().map(|r| (r.to_owned(), targets[r].clone())).collect();
        let before = held.clone();
        assert!(
            exact_repair(&scene, "C", &held, &["U".to_owned(), "X".to_owned()], &targets, &options).unwrap().is_none(),
            "the complete unit cannot accept a partial solution that ignores its future support"
        );
        assert_eq!(held, before);
        let mut diagnostic = RepairDiagnostics::default();
        assert!(
            exact_repair_observed(
                &scene,
                "C",
                &held,
                &["U".to_owned(), "X".to_owned()],
                &targets,
                &options,
                &mut diagnostic
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(diagnostic.packing.empty_reference.as_deref(), Some("C2"));
        assert_eq!(diagnostic.packing.search_remaining, None, "an empty domain never started combination search");
        assert_eq!(diagnostic.packing.domains["C2"].1, 0);
        assert_eq!(diagnostic.references.len(), 3);
        let value = diagnostic.value(Some(false));
        assert_eq!(copper_json::parse(&copper_json::Dumps::new().dumps(&value)).unwrap(), value);
    }

    #[test]
    fn bounded_repair_observation_preserves_exact_search_and_reports_remaining_work() {
        let board = trapped_relative_unit();
        let options = repair_options();
        let scene = Scene::new(&board, options.policy());
        let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let held = without(&targets, "C");
        let mut diagnostic = RepairDiagnostics::default();
        let observed = exact_repair_observed(
            &scene,
            "C",
            &held,
            &["U".to_owned(), "X".to_owned()],
            &targets,
            &options,
            &mut diagnostic,
        )
        .unwrap()
        .unwrap();
        let base: Poses = held
            .iter()
            .filter(|(r, _)| !diagnostic.references.contains(*r))
            .map(|(r, p)| (r.clone(), p.clone()))
            .collect();
        let anchors: Vec<Placement> = diagnostic.references.iter().filter_map(|r| held.get(r).cloned()).collect();
        let unobserved =
            pack_local_components(&scene, &diagnostic.references, &base, &targets, &options, true, false, &anchors)
                .unwrap()
                .unwrap();
        assert_eq!(observed, unobserved, "observation adds no candidates or search decisions");
        assert_eq!(diagnostic.references.len(), 2);
        assert!(diagnostic.packing.passes >= 1);
        assert_eq!(diagnostic.packing.empty_reference, None);
        assert!(diagnostic.packing.search_remaining.is_some_and(|remaining| remaining >= 0));
        assert!(
            diagnostic.packing.domains.values().all(|(sampled, legal)| *sampled <= options.probe_limit() && *legal > 0)
        );
        let value = diagnostic.value(Some(true));
        assert_eq!(copper_json::parse(&copper_json::Dumps::new().dumps(&value)).unwrap(), value);
    }

    #[test]
    fn coupled_group_order_avoids_repeating_a_star_under_an_independent_narrow_neighbour() {
        let references: Vec<String> = ["U", "C1", "C2", "X"].into_iter().map(str::to_owned).collect();
        let mut board = board(
            "coupled-star-budget",
            240.0,
            220.0,
            vec![one_pad()],
            references.iter().map(|r| placement(r, "test/one-pad", 10.0, 10.0)).collect(),
            Vec::new(),
        );
        board.relative_rules =
            vec![max_distance(("U", "1"), ("C1", "1"), 1.7), max_distance(("U", "1"), ("C2", "1"), 1.7)];
        let options = PlannerOptions::default();
        let scene = Scene::new(&board, options.policy());
        let pose = |r: &str, x, y| placement(r, "test/one-pad", x, y);
        let mut domains = vec![
            vec![pose("U", 10.0, 10.0)],
            Vec::new(),
            Vec::new(),
            vec![pose("X", 10.0, 11.6), pose("X", 220.0, 200.0)],
        ];
        for index in 0..31 {
            let x = 20.0 + 3.0 * f64::from(index);
            domains[0].push(pose("U", x, 10.0));
            domains[1].push(pose("C1", x, 100.0));
            domains[2].push(pose("C2", x, 180.0));
        }
        domains[1].push(pose("C1", 10.0, 11.6));
        domains[2].push(pose("C2", 11.6, 10.0));
        let witness: Poses =
            [domains[0][0].clone(), domains[1][31].clone(), domains[2][31].clone(), domains[3][1].clone()]
                .into_iter()
                .map(|p| (p.reference.clone(), p))
                .collect();
        assert!(scene.checker.placement_solution_is_legal(&witness).unwrap());
        assert_eq!(scene.penalty(&witness).unwrap(), 0);
        let pair_rules: IndexMap<_, _> =
            [((0, 1), vec![&board.relative_rules[0]]), ((0, 2), vec![&board.relative_rules[1]])].into_iter().collect();
        let sizes = relative_group_sizes(references.len(), pair_rules.keys().copied());
        assert_eq!(sizes, [3, 3, 3, 1]);
        let initial: IndexMap<usize, Vec<usize>> =
            domains.iter().enumerate().map(|(i, poses)| (i, (0..poses.len()).collect())).collect();
        let make_search = |relative_group_sizes| Search {
            scene: &scene,
            references: &references,
            rules: scene.relative.clone(),
            polygons: domains.iter().map(|poses| poses.iter().map(|p| scene.polygon(p)).collect()).collect(),
            domains: domains.clone(),
            pair_rules: pair_rules.clone(),
            relative_group_sizes,
            clashes: IndexMap::new(),
            remaining: 300,
            // Ranking may already be exhausted in a deeper real search. That
            // does not grant more states or drop the remaining exact checks.
            heuristic_remaining: 0,
            exact_repair_candidates: options.exact_repair_candidates as usize,
            component_clearance_nm: options.component_clearance_nm,
        };
        let mut previous_mrv = make_search(vec![1; references.len()]);
        assert!(previous_mrv.solve(&initial, &Poses::new()).unwrap().is_none());
        assert_eq!(previous_mrv.remaining, 0, "the narrow independent choice repeats an impossible star branch");
        let mut grouped = make_search(sizes);
        let result = grouped.solve(&initial, &Poses::new()).unwrap().expect("same candidates and state allowance");
        assert!(grouped.remaining > 0);
        assert_eq!(result["X"], domains[3][1], "the independent blocker remains movable and is relocated");
        assert!(scene.checker.placement_solution_is_legal(&result).unwrap());
        assert_eq!(scene.penalty(&result).unwrap(), 0);
    }

    #[test]
    fn minimum_distance_peers_do_not_hide_a_distant_legal_region_from_greedy_or_joint_search() {
        for with_future_support in [false, true] {
            let mut board = board(
                "minimum-distance-wide-coverage",
                100.0,
                40.0,
                vec![one_pad()],
                vec![placement("A", "test/one-pad", 10.0, 5.0), placement("U", "test/one-pad", 20.0, 20.0)],
                Vec::new(),
            );
            // The held peer has a tiny legal pocket. Every local/ring proposal
            // around the target is blocked, but ordinary material beyond X=70
            // is legal and satisfies the exclusion distance.
            for (name, x, y, width, height) in [
                ("above", 0.0, 0.0, 70.0, 4.4),
                ("below", 0.0, 5.6, 70.0, 34.4),
                ("left", 0.0, 4.4, 9.4, 1.2),
                ("right", 10.6, 4.4, 59.4, 1.2),
            ] {
                board.keepouts.push(
                    copper_physical::PlacementKeepout::new(name, region(name, x, y, width, height).outline, None, None)
                        .unwrap(),
                );
            }
            let mut exclusion = max_distance(("U", "1"), ("A", "1"), 6.0);
            exclusion.kind = RelativePlacementKind::MinDistance;
            board.relative_rules.push(exclusion);
            if with_future_support {
                board.placements.push(placement("C", "test/one-pad", 22.0, 20.0));
                board.relative_rules.push(max_distance(("C", "1"), ("U", "1"), 2.0));
            }
            let options = PlannerOptions::default().validated().unwrap();
            let scene = Scene::new(&board, options.policy());
            let targets: Poses = board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            let held: Poses = [("A".to_owned(), targets["A"].clone())].into_iter().collect();
            let old_relative =
                relative_candidates(&scene, &scene.relative, "U", &targets["U"], &held, &options, 250_000).unwrap();
            let old_general =
                general_candidates(&scene, targets["U"].clone(), &options, scene.allowed_orientations("U"));
            assert!(
                !old_relative.into_iter().chain(old_general).take(options.probe_limit()).any(|pose| {
                    scene.legal(&pose, &held).unwrap() && scene.penalty(&with(&held, &pose)).unwrap() == 0
                }),
                "the old held-peer classification spends its whole budget in the blocked local area"
            );
            assert!(!bounded_by_held_peer(&scene.relative, "U", &held));
            let spacing =
                EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes).unwrap();
            let chosen = best_legal_choice(
                &scene,
                "U",
                &targets["U"],
                &held,
                &IndexMap::new(),
                &options,
                &spacing,
                0,
                Some(options.legalization_candidates as usize),
            )
            .unwrap()
            .unwrap();
            assert!(chosen.position.x_nm > mm(70.0));
            assert!(scene.legal(&chosen, &held).unwrap());
            assert_eq!(scene.penalty(&with(&held, &chosen)).unwrap(), 0);
            let references: Vec<String> =
                if with_future_support { vec!["U".into(), "C".into()] } else { vec!["U".into()] };
            let mut diagnostic = PackingDiagnostics::default();
            let packed =
                pack_local_components_observed(&scene, &references, &held, &targets, &options, &mut diagnostic)
                    .unwrap()
                    .expect("the same bounded joint domain retains broad coverage");
            assert_eq!(packed["A"], held["A"]);
            assert_eq!(scene.penalty(&packed).unwrap(), 0, "minimum and maximum distances still hold exactly");
            assert!(scene.checker.placement_solution_is_legal(&packed).unwrap());
            assert!(diagnostic.domains.values().all(|(sampled, _)| *sampled <= options.probe_limit()));
        }
    }

    // ------------------------------------------------------------------
    // `tests/test_region_blocker_repair.py`: region-only exact repair.

    fn region(name: &str, x: f64, y: f64, w: f64, h: f64) -> PlacementRegion {
        PlacementRegion {
            name: name.to_owned(),
            outline: BoardOutline::rectangle(mm(w), mm(h), Some(pt(x, y))).expect("outline"),
            side: Some(BoardSide::Back),
        }
    }

    fn back(reference: &str, footprint: &str, x: f64, y: f64) -> Placement {
        Placement { side: BoardSide::Back, ..placement(reference, footprint, x, y) }
    }

    struct RegionScene {
        board: PhysicalBoard,
        target: Placement,
        early: Placement,
        tails: Vec<Placement>,
    }

    impl RegionScene {
        fn new(prefix: &str) -> Self {
            let small = footprint("land", Vec::new(), 1.5, 1.5);
            let blocker = footprint("held-land", Vec::new(), 2.0, 2.0);
            let target = back(&format!("{prefix}-trapped"), "land", 13.0, 13.0);
            let early = back(&format!("{prefix}-early"), "held-land", 13.0, 13.0);
            let tails: Vec<Placement> =
                (0..6).map(|i| back(&format!("{prefix}-tail-{i}"), "land", 30.0 + 3.0 * i as f64, 30.0)).collect();
            let mut placements = vec![target.clone(), early.clone()];
            placements.extend(tails.iter().cloned());
            let mut board = board("RegionRepair", 60.0, 50.0, vec![small, blocker], placements, Vec::new());
            board.regions = vec![region("tight", 12.0, 12.0, 2.0, 2.0), region("flexible", 10.0, 10.0, 8.0, 8.0)];
            board.placement_rules = vec![
                ComponentPlacementRule {
                    region: Some("tight".to_owned()),
                    side: Some(BoardSide::Back),
                    ..ComponentPlacementRule::new(&target.reference)
                },
                ComponentPlacementRule {
                    region: Some("flexible".to_owned()),
                    side: Some(BoardSide::Back),
                    ..ComponentPlacementRule::new(&early.reference)
                },
            ];
            RegionScene { board, target, early, tails }
        }

        fn held(&self) -> (Poses, Vec<String>) {
            let held: Poses =
                std::iter::once(&self.early).chain(&self.tails).map(|p| (p.reference.clone(), p.clone())).collect();
            let order = held.keys().cloned().collect();
            (held, order)
        }

        fn repair(&self) -> Option<Poses> {
            let options = repair_options();
            let scene = Scene::new(&self.board, options.policy());
            let (held, order) = self.held();
            let targets: Poses = self.board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
            exact_repair(&scene, &self.target.reference, &held, &order, &targets, &options).expect("repair")
        }
    }

    fn repair_options() -> PlannerOptions {
        PlannerOptions { exact_repair_limit: 2, ..PlannerOptions::default() }.validated().expect("options")
    }

    fn solution_legal(board: &PhysicalBoard, result: &Poses) -> bool {
        Scene::new(board, repair_options().policy()).checker.placement_solution_is_legal(result).expect("legal")
    }

    #[test]
    fn region_only_repair_moves_actual_early_blocker_instead_of_unrelated_tail() {
        for prefix in ["renamed-a", "other"] {
            let s = RegionScene::new(prefix);
            let options = repair_options();
            let scene = Scene::new(&s.board, options.policy());
            let (held, _) = s.held();
            let probes =
                relative_candidates(&scene, &scene.relative, &s.target.reference, &s.target, &held, &options, 250_000)
                    .expect("probes");
            assert!(probes.is_empty());
            let result = s.repair().expect("a repair");
            assert_ne!(result[&s.early.reference].position, s.early.position, "{prefix}");
            assert!(s.tails.iter().all(|p| result[&p.reference] == *p));
            assert!(solution_legal(&s.board, &result));
        }
    }

    #[test]
    fn fixed_actual_blocker_is_never_relaxed_or_moved() {
        let mut s = RegionScene::new("fixed");
        s.board.placement_rules[1].fixed_position = Some(s.early.position);
        s.board.placement_rules[1].fixed_rotation_degrees = Some(Decimal::from_i64(0));
        assert!(s.repair().is_none());
    }

    #[test]
    fn nonrectangular_region_and_effective_rotation_side_remain_legal_authority() {
        let mut s = RegionScene::new("polygon");
        s.board.footprints.insert("triangle-land".to_owned(), footprint("triangle-land", Vec::new(), 0.4, 0.4));
        s.target = Placement { footprint: "triangle-land".to_owned(), position: pt(12.5, 12.5), ..s.target.clone() };
        s.early.position = s.target.position;
        s.board.placements[0] = s.target.clone();
        s.board.placements[1] = s.early.clone();
        s.board.placement_rules[0].allowed_orientations = vec![Decimal::from_i64(0)];
        let triangle = BoardOutline::new(vec![pt(12.0, 12.0), pt(14.0, 12.0), pt(12.0, 14.0)], Vec::new(), None, None)
            .expect("triangle");
        s.board.regions[0] =
            PlacementRegion { name: "tight".to_owned(), outline: triangle, side: Some(BoardSide::Back) };
        let result = s.repair().expect("a repair");
        assert!(solution_legal(&s.board, &result));
        let chosen = &result[&s.target.reference];
        assert!(chosen.side == BoardSide::Back && chosen.rotation_degrees == Decimal::from_i64(0));
        // A bounding rectangle alone would accept the opposite triangular corner.
        assert!(chosen.position.x_nm + chosen.position.y_nm + 400_000 <= 26_000_000, "{chosen:?}");
        assert!(s.tails.iter().all(|p| result[&p.reference] == *p));
    }

    /// Adapted: Python captured the references `_pack_local_components`
    /// received; here the domain facts are checked and the repair must move
    /// the boundary blocker while keeping the tail.
    #[test]
    fn spread_same_probe_count_detects_right_boundary_blocker_missed_by_prefix() {
        for prefix in ["wide-a", "renamed-wide"] {
            let mut s = RegionScene::new(prefix);
            s.board.footprints.insert("boundary-land".to_owned(), footprint("boundary-land", Vec::new(), 1.0, 1.0));
            s.target.position = pt(14.0, 14.0);
            s.early = Placement { footprint: "boundary-land".to_owned(), position: pt(17.0, 14.0), ..s.early.clone() };
            s.board.placements[0] = s.target.clone();
            s.board.placements[1] = s.early.clone();
            s.board.regions[0] = region("tight", 10.0, 10.0, 8.0, 8.0);
            let options = repair_options();
            let scene = Scene::new(&s.board, options.policy());
            let positions = candidate_positions(&scene, &s.target, &options);
            let angles = scene.allowed_orientations(&s.target.reference);
            let domain: Vec<Placement> =
                positions.iter().flat_map(|p| angles.iter().map(|a| posed(&s.target, *p, a))).collect();
            assert!(domain.len() as i64 > options.exact_repair_candidates);
            let early: Poses = [(s.early.reference.clone(), s.early.clone())].into_iter().collect();
            for candidate in domain.iter().take(options.exact_repair_candidates as usize) {
                assert!(scene.legal(candidate, &early).expect("legal"));
            }
            let result = s.repair().expect("a repair");
            assert!(s.tails.iter().all(|p| result[&p.reference] == *p), "{prefix}");
        }
    }
}

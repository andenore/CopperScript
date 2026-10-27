//! Placement legality as physical DRC checks it (`pcbir.placement`
//! `placement_solution_is_legal` and `placement_rejection_reasons`, with
//! `placement_holes`, `mechanical_assembly` and the reserved corridors).
//!
//! These are the planner's hard predicates at its default options. The
//! planner itself (seeding, legalization, refinement) is not here.

use indexmap::{IndexMap, IndexSet};

use copper_lang::mechanical::assembly::body_outline;
use copper_pcb::board::*;
use copper_pcb::hard_macros::{cluster_placement_matches, resolved_macro_geometry, via_span};
use copper_pcb::keepouts::{placement_map, resolved_cluster_keepouts};
use copper_pcb::transform::{placed_pad_shape, transform_local, transformed_local_point};
use copper_pcb::{Error, Result};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{
    RoundedConvexShape, polygon_triangles, ring_edges, segments_intersect, shapes_clear, shapes_touch_or_overlap,
};
use copper_physical::mechanical::{hole_shape, shape_in_outline, slot_shape, validated_ring};
use copper_physical::pyfloat::{big_ratio_to_f64, hypot, round_int};
use copper_physical::{BoardSide, CopperKeepout, CopperLayer, PlacementKeepout, Point};
use copper_units::Decimal;

use crate::{drill_shape, shape};

/// `PlacementPlannerOptions().edge_clearance_nm`.
pub const EDGE_CLEARANCE_NM: i64 = 2_000_000;
/// `PlacementPlannerOptions().component_clearance_nm`.
pub const COMPONENT_CLEARANCE_NM: i64 = DEFAULT_ADDITIONAL_COURTYARD_GAP_NM;
/// `tan(pi/8)`: a regular octagon with this half-side per unit inradius circumscribes the disc.
const OCTAGON_HALF_SIDE: f64 = 0.41421356237309515;

type Poses = IndexMap<String, Placement>;

fn rules(board: &PhysicalBoard) -> IndexMap<&str, &ComponentPlacementRule> {
    board.placement_rules.iter().map(|r| (r.reference.as_str(), r)).collect()
}

fn side_layer(side: BoardSide) -> CopperLayer {
    if side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }
}

/// `_allowed_orientations`.
pub fn allowed_orientations(board: &PhysicalBoard, reference: &str) -> Vec<Decimal> {
    match rules(board).get(reference) {
        Some(rule) => match &rule.fixed_rotation_degrees {
            Some(rotation) => vec![rotation.clone()],
            None => rule.allowed_orientations.clone(),
        },
        None => [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
    }
}

/// `component_height`: the declared height, else the footprint's.
pub fn component_height(board: &PhysicalBoard, pose: &Placement) -> Option<i64> {
    board
        .component_heights
        .iter()
        .find(|h| h.reference == pose.reference)
        .map(|h| h.height_nm)
        .or(board.footprints[&pose.footprint].height_nm)
}

/// `_placement_polygon`: the placed courtyard, or the conservative body rectangle.
pub fn placement_polygon(board: &PhysicalBoard, placement: &Placement) -> Vec<Point> {
    let footprint = &board.footprints[&placement.footprint];
    let courtyard = if footprint.courtyard.is_empty() {
        let (w, h) =
            ((footprint.body_size.width_nm + 1).div_euclid(2), (footprint.body_size.height_nm + 1).div_euclid(2));
        vec![Point::new(-w, -h), Point::new(w, -h), Point::new(w, h), Point::new(-w, h)]
    } else {
        footprint.courtyard.clone()
    };
    courtyard
        .iter()
        .map(|p| {
            let local = transform_local(*p, &placement.rotation_degrees, placement.side);
            Point::new(local.x_nm + placement.position.x_nm, local.y_nm + placement.position.y_nm)
        })
        .collect()
}

fn point_bounds(points: &[Point]) -> (i64, i64, i64, i64) {
    (
        points.iter().map(|p| p.x_nm).min().expect("points"),
        points.iter().map(|p| p.y_nm).min().expect("points"),
        points.iter().map(|p| p.x_nm).max().expect("points"),
        points.iter().map(|p| p.y_nm).max().expect("points"),
    )
}

/// Python's `int / int`: the correctly rounded float quotient.
fn true_divide(numerator: i128, denominator: i128) -> f64 {
    big_ratio_to_f64(&numerator.into(), &denominator.into())
}

/// `placement._point_segment_distance`: the float distance.
pub fn float_point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    let (dx, dy) = (i128::from(end.x_nm - start.x_nm), i128::from(end.y_nm - start.y_nm));
    let (px, py) = (point.x_nm as f64, point.y_nm as f64);
    if dx == 0 && dy == 0 {
        return hypot(px - start.x_nm as f64, py - start.y_nm as f64);
    }
    let dot = i128::from(point.x_nm - start.x_nm) * dx + i128::from(point.y_nm - start.y_nm) * dy;
    let fraction = true_divide(dot, dx * dx + dy * dy).clamp(0.0, 1.0);
    hypot(px - (start.x_nm as f64 + fraction * dx as f64), py - (start.y_nm as f64 + fraction * dy as f64))
}

/// `placement._point_in_polygon`: float crossing test, boundary inclusive.
pub fn float_point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    for (start, end) in ring_edges(polygon) {
        if float_point_segment_distance(point, start, end) == 0.0 {
            return true;
        }
        if (start.y_nm > point.y_nm) != (end.y_nm > point.y_nm) {
            let quotient = true_divide(
                i128::from(end.x_nm - start.x_nm) * i128::from(point.y_nm - start.y_nm),
                i128::from(end.y_nm - start.y_nm),
            );
            let crossing = quotient + start.x_nm as f64;
            if (point.x_nm as f64) < crossing {
                inside = !inside;
            }
        }
    }
    inside
}

fn polygons_intersect(first: &[Point], second: &[Point]) -> bool {
    let (a_edges, b_edges) = (ring_edges(first), ring_edges(second));
    if a_edges.iter().any(|&(a, b)| b_edges.iter().any(|&(c, d)| segments_intersect(a, b, c, d))) {
        return true;
    }
    float_point_in_polygon(first[0], second) || float_point_in_polygon(second[0], first)
}

fn is_axis_rectangle(polygon: &[Point]) -> bool {
    if polygon.len() != 4 {
        return false;
    }
    let (left, top, right, bottom) = point_bounds(polygon);
    let corners: IndexSet<Point> =
        [Point::new(left, top), Point::new(right, top), Point::new(right, bottom), Point::new(left, bottom)]
            .into_iter()
            .collect();
    let set: IndexSet<Point> = polygon.iter().copied().collect();
    set.len() == corners.len() && set.iter().all(|p| corners.contains(p))
}

/// `_polygons_too_close`: boundary-protecting proximity of two polygons.
pub fn polygons_too_close(first: &[Point], second: &[Point], clearance: i64) -> bool {
    let (a, b) = (point_bounds(first), point_bounds(second));
    if a.2 + clearance <= b.0 || b.2 + clearance <= a.0 || a.3 + clearance <= b.1 || b.3 + clearance <= a.1 {
        return false;
    }
    if is_axis_rectangle(first) && is_axis_rectangle(second) {
        let dx = i128::from((a.0 - b.2).max(b.0 - a.2).max(0));
        let dy = i128::from((a.1 - b.3).max(b.1 - a.3).max(0));
        let c = i128::from(clearance);
        return (dx == 0 && dy == 0) || dx * dx + dy * dy < c * c;
    }
    if polygons_intersect(first, second) {
        return true;
    }
    if clearance <= 0 {
        return false;
    }
    let minimum = [(first, second), (second, first)]
        .iter()
        .flat_map(|(polygon, other)| {
            polygon.iter().flat_map(move |p| {
                ring_edges(other).into_iter().map(move |(s, e)| float_point_segment_distance(*p, s, e))
            })
        })
        .fold(f64::INFINITY, f64::min);
    minimum < clearance as f64
}

fn courtyard_triangles(polygon: &[Point]) -> std::result::Result<Vec<[Point; 3]>, String> {
    polygon_triangles(&validated_ring(polygon.to_vec())?)
}

/// `_polygon_interiors_overlap`: positive-area overlap; boundary-only contact is legal.
fn polygon_interiors_overlap(first: &[Point], second: &[Point]) -> bool {
    let (Ok(left), Ok(right)) = (courtyard_triangles(first), courtyard_triangles(second)) else { return true };
    let degenerate = |t: &[Point; 3]| {
        let (a, b, c) = (t[0], t[1], t[2]);
        i128::from(b.x_nm - a.x_nm) * i128::from(c.y_nm - a.y_nm)
            == i128::from(b.y_nm - a.y_nm) * i128::from(c.x_nm - a.x_nm)
    };
    if left.iter().chain(right.iter()).any(degenerate) {
        return true;
    }
    for l in &left {
        for r in &right {
            let separated = [l, r].iter().any(|polygon| {
                ring_edges(&polygon[..]).iter().any(|(a, b)| {
                    let (nx, ny) = (i128::from(b.y_nm - a.y_nm), i128::from(a.x_nm - b.x_nm));
                    let project = |t: &[Point; 3]| {
                        let values = t.iter().map(|p| i128::from(p.x_nm) * nx + i128::from(p.y_nm) * ny);
                        (values.clone().min().expect("triangle"), values.max().expect("triangle"))
                    };
                    let ((lmin, lmax), (rmin, rmax)) = (project(l), project(r));
                    lmax <= rmin || rmax <= lmin
                })
            });
            if !separated {
                return true;
            }
        }
    }
    false
}

/// `_courtyards_too_close`: extra courtyard gaps use interior-only overlap at zero.
pub fn courtyards_too_close(first: &[Point], second: &[Point], clearance: i64) -> bool {
    if clearance != 0 {
        return polygons_too_close(first, second, clearance);
    }
    let (a, b) = (point_bounds(first), point_bounds(second));
    if a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1 {
        return false;
    }
    polygon_interiors_overlap(first, second)
}

/// `_polygon_inside`: vertices and edge midpoints inside, with clearance.
pub fn polygon_inside(inner: &[Point], outer: &[Point], clearance: i64) -> bool {
    let mut samples = inner.to_vec();
    samples.extend(
        ring_edges(inner)
            .into_iter()
            .map(|(s, e)| Point::new((s.x_nm + e.x_nm).div_euclid(2), (s.y_nm + e.y_nm).div_euclid(2))),
    );
    if !samples.iter().all(|p| float_point_in_polygon(*p, outer)) {
        return false;
    }
    if clearance != 0 {
        let edges = ring_edges(outer);
        return inner.iter().all(|p| {
            edges.iter().map(|(s, e)| float_point_segment_distance(*p, *s, *e)).fold(f64::INFINITY, f64::min)
                >= clearance as f64
        });
    }
    true
}

/// `shape_in_board`: inside the outline and clear of mechanical holes and slots.
pub fn shape_in_board(board: &PhysicalBoard, shape: &RoundedConvexShape, edge: i64, hole: i64) -> Result<bool> {
    Ok(shape_in_outline(shape, &board.outline, edge)?
        && board.mechanical_holes.iter().all(|h| shapes_clear(shape, &hole_shape(h), hole.max(1)))
        && board.mechanical_slots.iter().all(|s| shapes_clear(shape, &slot_shape(s), hole.max(1))))
}

/// `body_in_material`: board material containment, with an audited body overhang.
pub fn body_in_material(
    board: &PhysicalBoard,
    pose: &Placement,
    body: &RoundedConvexShape,
    clearance: i64,
) -> Result<bool> {
    let Some(policy) = board.body_overhangs.iter().find(|a| a.reference == pose.reference) else {
        return shape_in_board(board, body, clearance, 0);
    };
    let permitted = body_outline(&board.outline, &board.boundary_edges, policy)?;
    if !shape_in_outline(body, &permitted, 0)? {
        return Ok(false);
    }
    let edge = board.boundary_edges.iter().find(|e| e.id == policy.edge).expect("policy edge");
    let length = (edge.end.x_nm - edge.start.x_nm).abs() + (edge.end.y_nm - edge.start.y_nm).abs();
    let (ux, uy) =
        ((edge.end.x_nm - edge.start.x_nm).div_euclid(length), (edge.end.y_nm - edge.start.y_nm).div_euclid(length));
    let a = Point::new(edge.start.x_nm + ux * policy.start_nm, edge.start.y_nm + uy * policy.start_nm);
    let b = Point::new(edge.start.x_nm + ux * policy.end_nm, edge.start.y_nm + uy * policy.end_nm);
    let mut unaffected: Vec<(Point, Point)> = ring_edges(&board.outline.vertices)
        .into_iter()
        .filter(|(s, e)| {
            let pair: IndexSet<Point> = [*s, *e].into_iter().collect();
            let edge_pair: IndexSet<Point> = [edge.start, edge.end].into_iter().collect();
            pair != edge_pair
        })
        .collect();
    if a != edge.start {
        unaffected.push((edge.start, a));
    }
    if b != edge.end {
        unaffected.push((b, edge.end));
    }
    for (s, e) in unaffected {
        if !shapes_clear(body, &shape(vec![s, e], 0)?, clearance) {
            return Ok(false);
        }
    }
    for cutout in &board.outline.cutouts {
        if !shapes_clear(body, &shape(cutout.vertices.clone(), 0)?, clearance.max(1)) {
            return Ok(false);
        }
    }
    Ok(board.mechanical_holes.iter().all(|h| shapes_clear(body, &hole_shape(h), 1))
        && board.mechanical_slots.iter().all(|s| shapes_clear(body, &slot_shape(s), 1)))
}

/// `assembly_pose_legal`: overhang lands, enclosure heights and assembly access.
pub fn assembly_pose_legal(board: &PhysicalBoard, pose: &Placement, polygon: &[Point], placed: &Poses) -> Result<bool> {
    let footprint = &board.footprints[&pose.footprint];
    if board.body_overhangs.iter().any(|a| a.reference == pose.reference) {
        for pad in footprint.pads.iter().filter(|p| p.kind != PadKind::Aperture) {
            let land = placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose);
            if !shape_in_board(board, &land, board.rules.minimum_clearance_nm, 0)? {
                return Ok(false);
            }
        }
    }
    let height = component_height(board, pose);
    for enclosure in &board.assembly_envelopes {
        if enclosure.side == pose.side
            && polygons_too_close(polygon, &enclosure.outline.vertices, 0)
            && height.is_none_or(|h| h > enclosure.maximum_height_nm)
        {
            return Ok(false);
        }
    }
    let mut poses = placed.clone();
    poses.insert(pose.reference.clone(), pose.clone());
    for region in &board.assembly_access {
        let Some(owner) = poses.get(&region.reference) else { continue };
        let side = if region.side == "component" {
            owner.side
        } else if owner.side == BoardSide::Front {
            BoardSide::Back
        } else {
            BoardSide::Front
        };
        let reserved = shape(region.outline.vertices.iter().map(|p| transformed_local_point(owner, *p)).collect(), 0)?;
        for other in poses.values() {
            if other.reference != owner.reference
                && other.side == side
                && !shapes_clear(&reserved, &shape(placement_polygon(board, other), 0)?, 1)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `component_pair_courtyard_gap_nm`: the hard extra courtyard gap of two components.
pub fn component_pair_courtyard_gap_nm(board: &PhysicalBoard, first: &str, second: &str, fallback: i64) -> Result<i64> {
    let known =
        board.placements.iter().any(|p| p.reference == first) && board.placements.iter().any(|p| p.reference == second);
    if first == second || !known {
        return Err(Error::Value("courtyard gap requires two distinct physical components".to_owned()));
    }
    let explicit = |reference: &str| {
        board
            .placement_rules
            .iter()
            .rev()
            .find(|r| r.reference == reference)
            .and_then(|r| r.additional_courtyard_gap_nm)
    };
    let mut internal: Option<i64> = None;
    for cluster in &board.rigid_clusters {
        let Some(value) = cluster.internal_clearance_nm else { continue };
        let members: Vec<&str> = cluster.members.iter().map(|m| m.reference.as_str()).collect();
        if members.contains(&first) && members.contains(&second) {
            internal = Some(internal.unwrap_or(0).max(value));
        }
    }
    let (a, b) = (explicit(first), explicit(second));
    if let Some(gap) = internal {
        return Ok(gap.max(a.unwrap_or(0)).max(b.unwrap_or(0)));
    }
    Ok(a.unwrap_or(fallback).max(b.unwrap_or(fallback)))
}

/// `ReservedCorridor`: a placement keepout derived from a differential pair.
#[derive(Clone, Debug)]
pub struct ReservedCorridor {
    pub nets: (String, String),
    pub keepout: PlacementKeepout,
    pub terminal_references: Vec<String>,
    pub margin_nm: i64,
}

fn convex_hull(points: &[(i64, i64)]) -> Vec<Point> {
    let mut ordered: Vec<(i64, i64)> = points.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let cross = |o: (i64, i64), a: (i64, i64), b: (i64, i64)| {
        i128::from(a.0 - o.0) * i128::from(b.1 - o.1) - i128::from(a.1 - o.1) * i128::from(b.0 - o.0)
    };
    let mut lower: Vec<(i64, i64)> = Vec::new();
    for &point in &ordered {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0 {
            lower.pop();
        }
        lower.push(point);
    }
    let mut upper: Vec<(i64, i64)> = Vec::new();
    for &point in ordered.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0 {
            upper.pop();
        }
        upper.push(point);
    }
    lower.pop();
    upper.pop();
    lower.into_iter().chain(upper).map(|(x, y)| Point::new(x, y)).collect()
}

/// `_derive_corridor`: one pair's corridor, or the reason it cannot be derived.
fn derive_corridor(board: &PhysicalBoard, members: &[String]) -> Result<std::result::Result<ReservedCorridor, String>> {
    if members.len() != 2 {
        return Ok(Err("reserve_corridor requires a differential partner".to_owned()));
    }
    let nets: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    for net in members {
        let Some(physical) = nets.get(net.as_str()) else {
            return Ok(Err(format!("pair net {net} has no physical pads")));
        };
        if physical.pads.len() < 2 {
            return Ok(Err(format!("pair net {net} has fewer than two terminal lands")));
        }
    }
    let mut pads: Vec<&PadReference> = members.iter().flat_map(|n| nets[n.as_str()].pads.iter()).collect();
    pads.sort();
    pads.dedup();
    let mut references: Vec<String> = pads.iter().map(|p| p.component.clone()).collect();
    references.sort();
    references.dedup();
    let source = placement_map(board);
    let unplaced: Vec<&str> = references.iter().filter(|r| !source.contains_key(*r)).map(String::as_str).collect();
    if !unplaced.is_empty() {
        return Ok(Err(format!("terminal component(s) {} are not placed", unplaced.join(", "))));
    }
    let placement_rules = rules(board);
    let movable: Vec<&str> = references
        .iter()
        .filter(|r| {
            placement_rules
                .get(r.as_str())
                .is_none_or(|rule| rule.fixed_position.is_none() || rule.fixed_rotation_degrees.is_none())
        })
        .map(String::as_str)
        .collect();
    if !movable.is_empty() {
        return Ok(Err(format!(
            "terminal component(s) {} are not fixed; reserve_corridor needs fixed_placement with position and rotation on every terminal component",
            movable.join(", ")
        )));
    }
    let poses: IndexMap<&str, Placement> = references
        .iter()
        .map(|r| {
            let rule = placement_rules[r.as_str()];
            let base = &source[r];
            (
                r.as_str(),
                Placement {
                    position: rule.fixed_position.expect("fixed"),
                    rotation_degrees: rule.fixed_rotation_degrees.clone().expect("fixed"),
                    side: rule.side.unwrap_or(base.side),
                    ..base.clone()
                },
            )
        })
        .collect();
    let routing: IndexMap<&str, &NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let pair_rules: Vec<&NetRoutingRule> = members.iter().filter_map(|n| routing.get(n.as_str()).copied()).collect();
    let width = pair_rules
        .iter()
        .filter_map(|r| r.width_nm.filter(|w| *w != 0))
        .max()
        .unwrap_or(board.rules.default_track_width_nm);
    let gap = pair_rules
        .iter()
        .filter_map(|r| r.pair_gap_nm.filter(|g| *g != 0))
        .max()
        .unwrap_or(board.rules.minimum_clearance_nm);
    let clearance = pair_rules
        .iter()
        .filter_map(|r| r.clearance_nm.filter(|c| *c != 0))
        .fold(board.rules.minimum_clearance_nm, i64::max);
    let margin = width + gap + clearance;
    let mut points = Vec::new();
    for pad in &pads {
        let pose = &poses[pad.component.as_str()];
        let lands: Vec<_> = board.footprints[&pose.footprint].pads.iter().filter(|p| p.number == pad.pad).collect();
        if lands.is_empty() {
            return Ok(Err(format!("terminal pad {}.{} is not in its footprint", pad.component, pad.pad)));
        }
        for land in lands {
            let copper = placed_pad_shape(transformed_local_point(pose, land.position), land, pose);
            let radius = copper.radius_nm + margin;
            let half = (radius as f64 * OCTAGON_HALF_SIDE).ceil() as i64;
            for spine in &copper.spine {
                for (dx, dy) in [
                    (radius, half),
                    (half, radius),
                    (-half, radius),
                    (-radius, half),
                    (-radius, -half),
                    (-half, -radius),
                    (half, -radius),
                    (radius, -half),
                ] {
                    points.push((spine.x_nm + dx, spine.y_nm + dy));
                }
            }
        }
    }
    let sides: IndexSet<BoardSide> = poses.values().map(|p| p.side).collect();
    let name = format!("reserved-corridor:{}/{}", members[0], members[1]);
    let outline = copper_physical::BoardOutline::polygon(convex_hull(&points))?;
    let side = if sides.len() == 1 { sides.first().copied() } else { None };
    Ok(Ok(ReservedCorridor {
        nets: (members[0].clone(), members[1].clone()),
        keepout: PlacementKeepout::new(&name, outline, side, None)?,
        terminal_references: references,
        margin_nm: margin,
    }))
}

/// `reserved_corridors().corridors`: corridors of `reserve_corridor` pairs that can be derived.
pub fn reserved_corridors(board: &PhysicalBoard) -> Result<Vec<ReservedCorridor>> {
    Ok(corridor_reservation(board)?.0)
}

/// `reserved_corridors()`: the derived corridors, and each skipped pair with its reason.
#[allow(clippy::type_complexity)]
pub fn corridor_reservation(board: &PhysicalBoard) -> Result<(Vec<ReservedCorridor>, Vec<(Vec<String>, String)>)> {
    let mut pairs: Vec<Vec<String>> = Vec::new();
    for rule in board.net_routing_rules.iter().filter(|r| r.reserve_corridor) {
        let mut pair = match &rule.differential_partner {
            None => vec![rule.net.clone()],
            Some(partner) => vec![rule.net.clone(), partner.clone()],
        };
        pair.sort();
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    pairs.sort();
    let (mut result, mut skipped) = (Vec::new(), Vec::new());
    for members in pairs {
        match derive_corridor(board, &members)? {
            Ok(corridor) => result.push(corridor),
            Err(reason) => skipped.push((members, reason)),
        }
    }
    Ok((result, skipped))
}

/// `automatic_pair_corridors`: corridors of every fixed differential pair without `reserve_corridor`.
pub fn automatic_pair_corridors(board: &PhysicalBoard) -> Result<Vec<ReservedCorridor>> {
    let reserved: IndexSet<&str> =
        board.net_routing_rules.iter().filter(|r| r.reserve_corridor).map(|r| r.net.as_str()).collect();
    let mut pairs: Vec<Vec<String>> = Vec::new();
    for rule in &board.net_routing_rules {
        let Some(partner) = rule.differential_partner.as_ref().filter(|p| !p.is_empty()) else { continue };
        if reserved.contains(rule.net.as_str()) || reserved.contains(partner.as_str()) {
            continue;
        }
        let mut pair = vec![rule.net.clone(), partner.clone()];
        pair.sort();
        if !pairs.contains(&pair) {
            pairs.push(pair);
        }
    }
    pairs.sort();
    let mut result = Vec::new();
    for members in pairs {
        if let Ok(corridor) = derive_corridor(board, &members)? {
            result.push(corridor);
        }
    }
    Ok(result)
}

/// `_keepout_blocks`: a placement keepout excludes this placed body.
pub fn keepout_blocks(
    board: &PhysicalBoard,
    keepout: &PlacementKeepout,
    candidate: &Placement,
    polygon: &[Point],
) -> bool {
    if keepout.side.is_some_and(|side| side != candidate.side) {
        return false;
    }
    if let Some(maximum) = keepout.maximum_component_height_nm
        && component_height(board, candidate).is_some_and(|h| h <= maximum)
    {
        return false;
    }
    polygons_too_close(polygon, &keepout.outline.vertices, 0)
}

/// The non-aperture lands of a pose: `(pad, placed copper)`.
fn land_shapes<'a>(
    board: &'a PhysicalBoard,
    pose: &Placement,
) -> Vec<(&'a copper_physical::footprint::FootprintPad, RoundedConvexShape)> {
    board.footprints[&pose.footprint]
        .pads
        .iter()
        .filter(|p| p.kind != PadKind::Aperture)
        .map(|p| (p, placed_pad_shape(transformed_local_point(pose, p.position), p, pose)))
        .collect()
}

/// `_lands_meet_keepout`: pad-only isolation areas.
pub fn lands_meet_keepout(board: &PhysicalBoard, pose: &Placement, keepout: &CopperKeepout) -> Result<bool> {
    let region = shape(keepout.vertices.clone(), 0)?;
    let side = side_layer(pose.side);
    for (pad, copper) in land_shapes(board, pose) {
        if pad.kind == PadKind::NonPlatedThroughHole {
            continue;
        }
        let touches_layer = if pad.kind == PadKind::Smd {
            keepout.layers.contains(&side)
        } else {
            board.stackup.copper_layers.iter().any(|l| keepout.layers.contains(l))
        };
        if touches_layer && shapes_touch_or_overlap(&region, &copper) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `_lands_meet_macro_copper`: a non-member's lands violate a hard macro's copper.
pub fn lands_meet_macro_copper(
    board: &PhysicalBoard,
    pose: &Placement,
    tracks: &[TrackSegment],
    vias: &[Via],
) -> Result<bool> {
    if tracks.is_empty() && vias.is_empty() {
        return Ok(false);
    }
    let routing: IndexMap<&str, &NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let owners: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p, n.name.as_str()))).collect();
    let required = |first: Option<&str>, second: &str| {
        [first, Some(second)]
            .into_iter()
            .flatten()
            .filter_map(|net| routing.get(net).map(|r| r.clearance_nm.unwrap_or(0)))
            .fold(board.rules.minimum_clearance_nm, i64::max)
    };
    let stack = &board.stackup.copper_layers;
    let side = side_layer(pose.side);
    for (pad, land) in land_shapes(board, pose) {
        let net = owners.get(&PadReference::new(&pose.reference, &pad.number)).copied();
        let layers: Vec<CopperLayer> = if pad.kind == PadKind::Smd { vec![side] } else { stack.clone() };
        for track in tracks {
            if layers.contains(&track.layer)
                && Some(track.net.as_str()) != net
                && !shapes_clear(
                    &land,
                    &shape(vec![track.start, track.end], track.width_nm / 2)?,
                    required(net, &track.net),
                )
            {
                return Ok(true);
            }
        }
        for via in vias {
            let span = via_span(board, via)?;
            if Some(via.net.as_str()) == net || !layers.iter().any(|l| span.contains(l)) {
                continue;
            }
            if !shapes_clear(&land, &shape(vec![via.position], via.size_nm / 2)?, required(net, &via.net))
                || !shapes_clear(
                    &land,
                    &shape(vec![via.position], via.drill_nm / 2)?,
                    board.rules.minimum_hole_clearance_nm,
                )
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A footprint's placed drills, copper lands and their envelope (`placement_holes._transformed_geometry`).
struct FootprintGeometry {
    holes: Vec<(RoundedConvexShape, PadKind)>,
    copper: Vec<RoundedConvexShape>,
    lands: Vec<(String, PadKind, RoundedConvexShape)>,
    envelope: copper_physical::Bounds,
}

fn footprint_geometry(board: &PhysicalBoard, pose: &Placement) -> Result<FootprintGeometry> {
    let footprint = &board.footprints[&pose.footprint];
    let (mut holes, mut copper, mut lands) = (Vec::new(), Vec::new(), Vec::new());
    for pad in &footprint.pads {
        let center = transformed_local_point(pose, pad.position);
        if matches!(pad.kind, PadKind::ThroughHole | PadKind::NonPlatedThroughHole) {
            holes.push((drill_shape(center, pad, pose), pad.kind));
        }
        if !matches!(pad.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
            let land = placed_pad_shape(center, pad, pose);
            copper.push(land.clone());
            lands.push((pad.number.clone(), pad.kind, land));
        }
    }
    let mut boxes: Vec<copper_physical::Bounds> = copper.iter().map(|s| s.bounds).collect();
    boxes.extend(holes.iter().map(|(s, _)| s.bounds));
    boxes.push(shape(placement_polygon(board, pose), 0)?.bounds);
    let envelope = copper_physical::Bounds {
        min_x: boxes.iter().map(|b| b.min_x).min().expect("boxes"),
        min_y: boxes.iter().map(|b| b.min_y).min().expect("boxes"),
        max_x: boxes.iter().map(|b| b.max_x).max().expect("boxes"),
        max_y: boxes.iter().map(|b| b.max_y).max().expect("boxes"),
    };
    Ok(FootprintGeometry { holes, copper, lands, envelope })
}

fn has_drills(board: &PhysicalBoard, pose: &Placement) -> bool {
    board.footprints[&pose.footprint]
        .pads
        .iter()
        .any(|p| matches!(p.kind, PadKind::ThroughHole | PadKind::NonPlatedThroughHole))
}

fn drills_clear_courtyard(
    holes: &[(RoundedConvexShape, PadKind)],
    board: &PhysicalBoard,
    pose: &Placement,
) -> Result<bool> {
    let Ok(triangles) = courtyard_triangles(&placement_polygon(board, pose)) else { return Ok(false) };
    for triangle in triangles {
        let piece = shape(triangle.to_vec(), 0)?;
        if holes.iter().any(|(hole, _)| !shapes_clear(hole, &piece, 1)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn array_owners(board: &PhysicalBoard) -> bool {
    board.via_in_pad_rules.iter().any(|r| r.rows.is_some())
}

/// `footprint_holes_clear`: candidate and accepted peers, symmetrically on both faces.
pub fn footprint_holes_clear(board: &PhysicalBoard, candidate: &Placement, held: &Poses) -> Result<bool> {
    if held.iter().any(|(reference, pose)| *reference != pose.reference) {
        return Ok(false);
    }
    if array_owners(board) {
        return copper_pcb::not_implemented("via-in-pad via arrays");
    }
    let clearance = board.rules.minimum_hole_clearance_nm;
    let candidate_has_holes = has_drills(board, candidate);
    let mut candidate_geometry = None;
    if candidate_has_holes {
        let geometry = footprint_geometry(board, candidate)?;
        let own = board
            .component_hole_clearances
            .iter()
            .find(|r| r.reference == candidate.reference)
            .map_or(clearance, |r| r.clearance_nm);
        if geometry
            .holes
            .iter()
            .filter(|(_, kind)| *kind == PadKind::NonPlatedThroughHole)
            .any(|(hole, _)| geometry.copper.iter().any(|pad| !shapes_clear(hole, pad, own)))
        {
            return Ok(false);
        }
        candidate_geometry = Some(geometry);
    }
    let assigned: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p, n.name.as_str()))).collect();
    for (reference, other) in held {
        if *reference == candidate.reference {
            continue;
        }
        if !candidate_has_holes && !has_drills(board, other) {
            continue;
        }
        if candidate_geometry.is_none() {
            candidate_geometry = Some(footprint_geometry(board, candidate)?);
        }
        let geometry = candidate_geometry.as_ref().expect("candidate geometry");
        let other_geometry = footprint_geometry(board, other)?;
        let reach = clearance.max(board.rules.minimum_clearance_nm).max(1);
        if !geometry.envelope.intersects_expanded(other_geometry.envelope, reach) {
            continue;
        }
        if candidate.side != other.side {
            for (a_number, a_kind, a_shape) in &geometry.lands {
                for (b_number, b_kind, b_shape) in &other_geometry.lands {
                    if *a_kind != PadKind::ThroughHole && *b_kind != PadKind::ThroughHole {
                        continue;
                    }
                    let a_net = assigned.get(&PadReference::new(&candidate.reference, a_number));
                    let b_net = assigned.get(&PadReference::new(&other.reference, b_number));
                    if a_net.is_some() && a_net == b_net {
                        continue;
                    }
                    if !shapes_clear(a_shape, b_shape, board.rules.minimum_clearance_nm) {
                        return Ok(false);
                    }
                }
            }
        }
        if !geometry.holes.is_empty()
            && (!drills_clear_courtyard(&geometry.holes, board, other)?
                || geometry
                    .holes
                    .iter()
                    .any(|(hole, _)| other_geometry.copper.iter().any(|pad| !shapes_clear(hole, pad, clearance))))
        {
            return Ok(false);
        }
        if !other_geometry.holes.is_empty()
            && (!drills_clear_courtyard(&other_geometry.holes, board, candidate)?
                || other_geometry
                    .holes
                    .iter()
                    .any(|(hole, _)| geometry.copper.iter().any(|pad| !shapes_clear(hole, pad, clearance))))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The `PlacementPlannerOptions` fields the hard predicates read.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub edge_clearance_nm: i64,
    pub component_clearance_nm: i64,
    pub auto_pair_corridors: bool,
    /// Session locks: these poses must not change.
    pub fixed_references: IndexSet<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            edge_clearance_nm: EDGE_CLEARANCE_NM,
            component_clearance_nm: COMPONENT_CLEARANCE_NM,
            auto_pair_corridors: true,
            fixed_references: IndexSet::new(),
        }
    }
}

/// The hard placement predicates for one board and policy, with the
/// board-derived inputs (rules, gap policy, derived corridors) computed once.
pub struct Checker<'a> {
    pub board: &'a PhysicalBoard,
    pub policy: Policy,
    rules: IndexMap<&'a str, &'a ComponentPlacementRule>,
    known: IndexSet<&'a str>,
    explicit_gaps: IndexMap<&'a str, Option<i64>>,
    internal_gaps: IndexMap<(&'a str, &'a str), i64>,
    corridors: std::cell::OnceCell<Result<Vec<ReservedCorridor>>>,
    automatic: std::cell::OnceCell<Result<Vec<ReservedCorridor>>>,
}

impl<'a> Checker<'a> {
    pub fn new(board: &'a PhysicalBoard, policy: Policy) -> Self {
        let mut internal_gaps: IndexMap<(&str, &str), i64> = IndexMap::new();
        for cluster in &board.rigid_clusters {
            let Some(value) = cluster.internal_clearance_nm else { continue };
            for (index, first) in cluster.members.iter().enumerate() {
                for second in &cluster.members[index + 1..] {
                    let pair = ordered_pair(&first.reference, &second.reference);
                    let entry = internal_gaps.entry(pair).or_insert(0);
                    *entry = (*entry).max(value);
                }
            }
        }
        Checker {
            board,
            policy,
            rules: rules(board),
            known: board.placements.iter().map(|p| p.reference.as_str()).collect(),
            explicit_gaps: board
                .placement_rules
                .iter()
                .map(|r| (r.reference.as_str(), r.additional_courtyard_gap_nm))
                .collect(),
            internal_gaps,
            corridors: std::cell::OnceCell::new(),
            automatic: std::cell::OnceCell::new(),
        }
    }

    /// The placement rule of a component.
    pub fn rule(&self, reference: &str) -> Option<&'a ComponentPlacementRule> {
        self.rules.get(reference).copied()
    }

    /// `reserved_corridors(board).corridors`, derived once.
    pub fn reserved_corridors(&self) -> Result<&[ReservedCorridor]> {
        match self.corridors.get_or_init(|| reserved_corridors(self.board)) {
            Ok(corridors) => Ok(corridors),
            Err(error) => Err(error.clone()),
        }
    }

    /// `automatic_pair_corridors(board)`, derived once.
    pub fn automatic_pair_corridors(&self) -> Result<&[ReservedCorridor]> {
        match self.automatic.get_or_init(|| automatic_pair_corridors(self.board)) {
            Ok(corridors) => Ok(corridors),
            Err(error) => Err(error.clone()),
        }
    }

    /// `component_pair_courtyard_gap_nm` with an explicit fallback.
    pub fn courtyard_gap(&self, first: &str, second: &str, fallback: i64) -> Result<i64> {
        if first == second || !self.known.contains(first) || !self.known.contains(second) {
            return Err(Error::Value("courtyard gap requires two distinct physical components".to_owned()));
        }
        let (a, b) =
            (self.explicit_gaps.get(first).copied().flatten(), self.explicit_gaps.get(second).copied().flatten());
        if let Some(gap) = self.internal_gaps.get(&ordered_pair(first, second)) {
            return Ok((*gap).max(a.unwrap_or(0)).max(b.unwrap_or(0)));
        }
        Ok(a.unwrap_or(fallback).max(b.unwrap_or(fallback)))
    }

    /// `_legal`: one candidate pose against the accepted placements.
    pub fn legal(
        &self,
        candidate: &Placement,
        placed: &Poses,
        cluster_keepouts: Option<&[CopperKeepout]>,
    ) -> Result<bool> {
        let board = self.board;
        let rule = self.rule(&candidate.reference);
        let allowed = match rule {
            Some(rule) => match &rule.fixed_rotation_degrees {
                Some(rotation) => *rotation == candidate.rotation_degrees,
                None => rule.allowed_orientations.contains(&candidate.rotation_degrees),
            },
            None => [0, 90, 180, 270].iter().any(|v| Decimal::from_i64(*v) == candidate.rotation_degrees),
        };
        if !allowed {
            return Ok(false);
        }
        if rule.and_then(|r| r.side).is_some_and(|side| side != candidate.side) {
            return Ok(false);
        }
        let polygon = placement_polygon(board, candidate);
        // Reject local courtyard collisions before expensive board/cutout checks.
        for other in placed.values().filter(|o| o.side == candidate.side) {
            let gap = self.courtyard_gap(&candidate.reference, &other.reference, self.policy.component_clearance_nm)?;
            if courtyards_too_close(&polygon, &placement_polygon(board, other), gap) {
                return Ok(false);
            }
        }
        if !footprint_holes_clear(board, candidate, placed)? {
            return Ok(false);
        }
        if array_owners(board) {
            return copper_pcb::not_implemented("via-in-pad via arrays");
        }
        let edge = rule.and_then(|r| r.edge_clearance_nm).unwrap_or(self.policy.edge_clearance_nm);
        let outline = &board.outline;
        let inside = if !board.body_overhangs.is_empty() {
            body_in_material(board, candidate, &shape(polygon.clone(), 0)?, edge)?
        } else if outline.circular_boundary.is_some()
            || outline.boundary_path.is_some()
            || !outline.cutouts.is_empty()
            || !board.mechanical_holes.is_empty()
            || !board.mechanical_slots.is_empty()
        {
            shape_in_board(board, &shape(polygon.clone(), 0)?, edge, 0)?
        } else {
            polygon_inside(&polygon, &outline.vertices, edge)
        };
        if !inside {
            return Ok(false);
        }
        if (!board.body_overhangs.is_empty()
            || !board.assembly_envelopes.is_empty()
            || !board.assembly_access.is_empty())
            && !assembly_pose_legal(board, candidate, &polygon, placed)?
        {
            return Ok(false);
        }
        for hole in board.mechanical_holes.iter().filter(|h| h.head_clearance_radius_nm != 0) {
            if !shapes_clear(
                &shape(polygon.clone(), 0)?,
                &shape(vec![hole.position], hole.head_clearance_radius_nm)?,
                1,
            ) {
                return Ok(false);
            }
        }
        if let Some(name) = rule.and_then(|r| r.region.as_ref()) {
            let region = board.regions.iter().find(|r| r.name == *name).expect("validated region");
            if region.side.is_some_and(|side| side != candidate.side) {
                return Ok(false);
            }
            if !polygon_inside(&polygon, &region.outline.vertices, 0) {
                return Ok(false);
            }
        }
        if !board.hard_macros.is_empty() {
            let mut macro_poses = placed.clone();
            macro_poses.insert(candidate.reference.clone(), candidate.clone());
            for hard in &board.hard_macros {
                let cluster = board.rigid_clusters.iter().find(|c| c.name == hard.cluster).expect("macro cluster");
                if !macro_poses.contains_key(&cluster.anchor_reference) {
                    continue;
                }
                let members: IndexSet<&str> = cluster.members.iter().map(|m| m.reference.as_str()).collect();
                let outsiders: Vec<&Placement> = if members.contains(candidate.reference.as_str()) {
                    placed.iter().filter(|(r, _)| !members.contains(r.as_str())).map(|(_, p)| p).collect()
                } else {
                    vec![candidate]
                };
                let geometry = resolved_macro_geometry(board, hard, &macro_poses)?;
                for region in &geometry.regions {
                    for outsider in &outsiders {
                        if region.layers.contains(&side_layer(outsider.side))
                            && polygons_too_close(&placement_polygon(board, outsider), &region.vertices, 0)
                        {
                            return Ok(false);
                        }
                    }
                }
                for outsider in &outsiders {
                    if lands_meet_macro_copper(board, outsider, &geometry.tracks, &geometry.vias)? {
                        return Ok(false);
                    }
                }
            }
        }
        for keepout in &board.keepouts {
            if keepout_blocks(board, keepout, candidate, &polygon) {
                return Ok(false);
            }
        }
        for corridor in self.reserved_corridors()? {
            if !corridor.terminal_references.contains(&candidate.reference)
                && keepout_blocks(board, &corridor.keepout, candidate, &polygon)
            {
                return Ok(false);
            }
        }
        if self.policy.auto_pair_corridors && rule.is_none_or(|r| r.fixed_position.is_none()) {
            // Movable parts only: an authored fixed pose is the author's call.
            for corridor in self.automatic_pair_corridors()? {
                if !corridor.terminal_references.contains(&candidate.reference)
                    && keepout_blocks(board, &corridor.keepout, candidate, &polygon)
                {
                    return Ok(false);
                }
            }
        }
        let resolved;
        let cluster_keepouts = match cluster_keepouts {
            Some(keepouts) => keepouts,
            None if board.rigid_clusters.is_empty() => &[],
            None => {
                let mut macro_poses = placed.clone();
                macro_poses.insert(candidate.reference.clone(), candidate.clone());
                resolved = resolved_cluster_keepouts(board, &macro_poses);
                &resolved[..]
            }
        };
        let candidate_layer = side_layer(candidate.side);
        for keepout in board.copper_keepouts.iter().chain(cluster_keepouts.iter()) {
            if keepout.block_footprints
                && keepout.layers.contains(&candidate_layer)
                && polygons_too_close(&polygon, &keepout.vertices, 0)
            {
                return Ok(false);
            }
            if keepout.block_pads && lands_meet_keepout(board, candidate, keepout)? {
                return Ok(false);
            }
        }
        let footprint = &board.footprints[&candidate.footprint];
        for other in placed.values().filter(|o| o.side == candidate.side) {
            let other_footprint = &board.footprints[&other.footprint];
            if other_footprint.keepouts.is_empty() && footprint.keepouts.is_empty() {
                continue;
            }
            let other_polygon = placement_polygon(board, other);
            for local in other_footprint.keepouts.iter().filter(|k| k.block_footprints) {
                let area: Vec<Point> = local.vertices.iter().map(|p| transformed_local_point(other, *p)).collect();
                if polygons_too_close(&polygon, &area, 0) {
                    return Ok(false);
                }
            }
            for local in footprint.keepouts.iter().filter(|k| k.block_footprints) {
                let area: Vec<Point> = local.vertices.iter().map(|p| transformed_local_point(candidate, *p)).collect();
                if polygons_too_close(&other_polygon, &area, 0) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// `placement_solution_is_hard_legal`.
    pub fn placement_solution_is_hard_legal(&self, placements: &Poses) -> Result<bool> {
        let board = self.board;
        let original = placement_map(board);
        if placements.len() != original.len() || !placements.keys().all(|k| original.contains_key(k)) {
            return Ok(false);
        }
        if placements
            .iter()
            .any(|(reference, item)| item.reference != *reference || item.footprint != original[reference].footprint)
        {
            return Ok(false);
        }
        if !cluster_placement_matches(board, placements)? {
            return Ok(false);
        }
        if self.policy.fixed_references.iter().any(|r| placements.get(r) != original.get(r)) {
            return Ok(false);
        }
        for rule in &board.placement_rules {
            let pose = &placements[&rule.reference];
            if rule.fixed_position.is_some_and(|p| p != pose.position) {
                return Ok(false);
            }
            if rule.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != pose.rotation_degrees) {
                return Ok(false);
            }
        }
        let keepouts = resolved_cluster_keepouts(board, placements);
        let mut accepted: Poses = IndexMap::new();
        let mut references: Vec<&String> = placements.keys().collect();
        references.sort();
        for reference in references {
            let candidate = &placements[reference];
            if !self.legal(candidate, &accepted, Some(&keepouts))? {
                return Ok(false);
            }
            accepted.insert(reference.clone(), candidate.clone());
        }
        Ok(true)
    }

    /// `placement_solution_is_legal`: hard rules plus every relative rule.
    pub fn placement_solution_is_legal(&self, placements: &Poses) -> Result<bool> {
        Ok(self.placement_solution_is_hard_legal(placements)? && relative_penalty(self.board, placements)? == 0)
    }
}

fn ordered_pair<'b>(first: &'b str, second: &'b str) -> (&'b str, &'b str) {
    if first <= second { (first, second) } else { (second, first) }
}

/// `_legal` at the planner's default options.
pub fn legal(
    candidate: &Placement,
    placed: &Poses,
    board: &PhysicalBoard,
    cluster_keepouts: Option<&[CopperKeepout]>,
) -> Result<bool> {
    Checker::new(board, Policy::default()).legal(candidate, placed, cluster_keepouts)
}

fn target_point(board: &PhysicalBoard, placements: &Poses, target: &PlacementTarget) -> Result<Option<Point>> {
    let Some(placement) = placements.get(&target.reference) else { return Ok(None) };
    let Some(pad) = &target.pad else { return Ok(Some(placement.position)) };
    let Some(land) = board.footprints[&placement.footprint].pads.iter().find(|p| p.number == *pad) else {
        return Err(Error::Value(format!(
            "component {} has no pad {}",
            copper_json::python_str_repr(&placement.reference),
            copper_json::python_str_repr(pad)
        )));
    };
    Ok(Some(transformed_local_point(placement, land.position)))
}

fn rounded_distance(a: Point, b: Point) -> i64 {
    round_int(hypot((a.x_nm - b.x_nm) as f64, (a.y_nm - b.y_nm) as f64))
}

fn rule_points(board: &PhysicalBoard, placements: &Poses, rule: &RelativePlacementRule) -> Result<Option<Vec<Point>>> {
    let mut points = Vec::new();
    for target in &rule.targets {
        match target_point(board, placements, target)? {
            Some(point) => points.push(point),
            None => return Ok(None),
        }
    }
    Ok(Some(points))
}

/// `_relative_penalty`: the weighted excess of every relative rule.
pub fn relative_penalty(board: &PhysicalBoard, placements: &Poses) -> Result<i64> {
    let mut total = 0;
    for rule in &board.relative_rules {
        let Some(points) = rule_points(board, placements, rule)? else { continue };
        match rule.kind {
            RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance => {
                let limit = rule.distance_nm.unwrap_or(0);
                for pair in points.windows(2) {
                    let distance = rounded_distance(pair[0], pair[1]);
                    let excess = if rule.kind == RelativePlacementKind::MaxDistance {
                        distance - limit
                    } else {
                        limit - distance
                    };
                    total += excess.max(0) * rule.weight;
                }
            }
            RelativePlacementKind::Align => {
                let values: Vec<i64> =
                    points.iter().map(|p| if rule.axis == Some(AlignmentAxis::X) { p.x_nm } else { p.y_nm }).collect();
                let span = values.iter().max().expect("targets") - values.iter().min().expect("targets");
                total += (span - rule.tolerance_nm).max(0) * rule.weight;
            }
        }
    }
    Ok(total)
}

fn mm_text(value_nm: i64) -> String {
    let millimetres = Decimal::from_i64(value_nm).div(&Decimal::from_i64(1_000_000)).expect("finite length");
    copper_units::Quantity::of(copper_units::Dimension::Length, &millimetres, "mm").expect("a length in mm").to_string()
}

/// The `message` of each `relative_placement_violations` record.
pub fn relative_violation_messages(board: &PhysicalBoard, placements: &Poses) -> Result<Vec<String>> {
    let mut messages = Vec::new();
    for rule in &board.relative_rules {
        let Some(points) = rule_points(board, placements, rule)? else { continue };
        match rule.kind {
            RelativePlacementKind::MaxDistance | RelativePlacementKind::MinDistance => {
                let limit = rule.distance_nm.unwrap_or(0);
                for (index, pair) in points.windows(2).enumerate() {
                    let actual = rounded_distance(pair[0], pair[1]);
                    let maximum = rule.kind == RelativePlacementKind::MaxDistance;
                    let amount = if maximum { actual - limit } else { limit - actual }.max(0);
                    if amount == 0 {
                        continue;
                    }
                    let (left, right) = (&rule.targets[index], &rule.targets[index + 1]);
                    messages.push(format!(
                        "{} to {}: {} distance {}, actual {}",
                        left.reference,
                        right.reference,
                        if maximum { "maximum" } else { "minimum" },
                        mm_text(limit),
                        mm_text(actual)
                    ));
                }
            }
            RelativePlacementKind::Align => {
                let values: Vec<i64> =
                    points.iter().map(|p| if rule.axis == Some(AlignmentAxis::X) { p.x_nm } else { p.y_nm }).collect();
                let actual = values.iter().max().expect("targets") - values.iter().min().expect("targets");
                if (actual - rule.tolerance_nm).max(0) != 0 {
                    let references: Vec<&str> = rule.targets.iter().map(|t| t.reference.as_str()).collect();
                    messages.push(format!(
                        "{}: {} alignment tolerance {}, actual span {}",
                        references.join(", "),
                        rule.axis.map_or("axis", AlignmentAxis::value),
                        mm_text(rule.tolerance_nm),
                        mm_text(actual)
                    ));
                }
            }
        }
    }
    Ok(messages)
}

/// `placement_solution_is_hard_legal` at the default options.
pub fn placement_solution_is_hard_legal(board: &PhysicalBoard, placements: &Poses) -> Result<bool> {
    Checker::new(board, Policy::default()).placement_solution_is_hard_legal(placements)
}

/// `placement_solution_is_legal`: hard rules plus every relative rule.
pub fn placement_solution_is_legal(board: &PhysicalBoard, placements: &Poses) -> Result<bool> {
    Checker::new(board, Policy::default()).placement_solution_is_legal(placements)
}

/// `placement_rejection_reasons`: why a pose is rejected, by the planner's own predicates.
pub fn placement_rejection_reasons(board: &PhysicalBoard, placements: &Poses) -> Result<Vec<String>> {
    if placement_solution_is_legal(board, placements)? {
        return Ok(Vec::new());
    }
    let original = placement_map(board);
    if placements.len() != original.len() || !placements.keys().all(|k| original.contains_key(k)) {
        return Ok(vec!["component inventory differs from physical source".to_owned()]);
    }
    let mut errors: Vec<String> = copper_pcb::hard_macros::cluster_edge_opening_rejections(board, placements)?;
    let mut sorted: Vec<(&String, &Placement)> = placements.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let all_rules = rules(board);
    let empty: Poses = IndexMap::new();
    for (reference, pose) in &sorted {
        if !footprint_holes_clear(board, pose, &empty)? {
            errors.push(format!("{reference}: own non-plated drill violates pad clearance"));
        }
        for (other_reference, other) in &sorted {
            let single: Poses = [((*other_reference).clone(), (*other).clone())].into_iter().collect();
            if other_reference < reference && !footprint_holes_clear(board, pose, &single)? {
                errors.push(format!(
                    "{reference} / {other_reference}: footprint drill or required via array violates courtyard/copper clearance"
                ));
            }
        }
        let rule = all_rules.get(reference.as_str()).copied();
        if let Some(rule) = rule {
            if rule.fixed_position.is_some_and(|p| p != pose.position) {
                errors.push(format!("{reference}: source constraint fixes its position"));
            }
            if rule.fixed_rotation_degrees.as_ref().is_some_and(|r| *r != pose.rotation_degrees) {
                errors.push(format!("{reference}: source constraint fixes its rotation"));
            }
            if let Some(side) = rule.side
                && pose.side != side
            {
                errors.push(format!("{reference}: source constraint fixes side {}", side.value()));
            }
        }
        if !allowed_orientations(board, reference).contains(&pose.rotation_degrees) {
            errors.push(format!("{reference}: rotation {} is not allowed", pose.rotation_degrees.to_sci_string()));
        }
        let polygon = placement_polygon(board, pose);
        let body = shape(polygon.clone(), 0)?;
        let clearance = rule.and_then(|r| r.edge_clearance_nm).unwrap_or(EDGE_CLEARANCE_NM);
        if !body_in_material(board, pose, &body, clearance)? {
            errors.push(format!("{reference}: courtyard violates board material/cutout clearance ({clearance} nm)"));
        }
        for hole in &board.mechanical_holes {
            if !shapes_clear(&body, &hole_shape(hole), 1) {
                errors.push(format!("{reference}: courtyard intersects hole {}", hole.id));
            }
            if hole.head_clearance_radius_nm != 0
                && !shapes_clear(&body, &shape(vec![hole.position], hole.head_clearance_radius_nm)?, 1)
            {
                errors.push(format!("{reference}: courtyard intersects screw-head clearance {}", hole.id));
            }
        }
        let others: Poses =
            placements.iter().filter(|(k, _)| *k != *reference).map(|(k, v)| (k.clone(), v.clone())).collect();
        if !assembly_pose_legal(board, pose, &polygon, &others)? {
            let height = component_height(board, pose);
            let limits: Vec<String> = board
                .assembly_envelopes
                .iter()
                .filter(|e| {
                    e.side == pose.side
                        && polygons_too_close(&polygon, &e.outline.vertices, 0)
                        && height.is_none_or(|h| h > e.maximum_height_nm)
                })
                .map(|e| format!("{} ≤ {} nm", e.id, e.maximum_height_nm))
                .collect();
            if limits.is_empty() {
                errors.push(format!(
                    "{reference}: assembly-access or original-board pad clearance conflict (body overhang never waives copper)"
                ));
            } else {
                let height = height.map_or_else(|| "unknown".to_owned(), |h| format!("{h} nm"));
                errors.push(format!("{reference}: component height {height} violates enclosure {}", limits.join(", ")));
            }
        }
        if !legal(pose, &empty, board, None)? {
            let prefix = format!("{reference}:");
            if !errors.iter().any(|e| e.starts_with(&prefix)) {
                let mut names: Vec<String> = board
                    .keepouts
                    .iter()
                    .filter(|k| {
                        k.side.is_none_or(|side| side == pose.side)
                            && polygons_too_close(&polygon, &k.outline.vertices, 0)
                    })
                    .map(|k| k.name.clone())
                    .collect();
                for corridor in reserved_corridors(board)? {
                    if !corridor.terminal_references.contains(reference)
                        && keepout_blocks(board, &corridor.keepout, pose, &polygon)
                    {
                        names.push(corridor.keepout.name.clone());
                    }
                }
                if let Some(region) = rule.and_then(|r| r.region.as_ref()) {
                    names.push(region.clone());
                }
                names.extend(
                    board
                        .copper_keepouts
                        .iter()
                        .filter(|k| k.block_footprints && polygons_too_close(&polygon, &k.vertices, 0))
                        .map(|k| k.id.clone()),
                );
                let names = if names.is_empty() { "macro boundary".to_owned() } else { names.join(", ") };
                errors.push(format!("{reference}: pose violates region/keepout {names}"));
            }
        } else {
            for (other_reference, other) in &sorted {
                if other_reference >= reference || other.side != pose.side {
                    continue;
                }
                let single: Poses = [((*other_reference).clone(), (*other).clone())].into_iter().collect();
                if !legal(pose, &single, board, None)? {
                    errors.push(format!(
                        "{reference} / {other_reference}: courtyard or footprint keepout clearance conflict"
                    ));
                }
            }
        }
    }
    if !cluster_placement_matches(board, placements)? {
        errors.push("rigid cluster/hard-macro member poses must move together".to_owned());
    }
    if relative_penalty(board, placements)? != 0 {
        let named = relative_violation_messages(board, placements)?;
        if named.is_empty() {
            errors.push("relative placement constraint (alignment/distance) is not satisfied".to_owned());
        } else {
            errors.extend(named);
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for error in errors {
        if !unique.contains(&error) {
            unique.push(error);
        }
    }
    if unique.is_empty() {
        unique.push("pose violates a represented physical constraint".to_owned());
    }
    Ok(unique)
}

//! Exact polygon topology and board-material queries (`pcbir.mechanical`).

use std::f64::consts::PI;

use copper_json::python_str_repr as repr;

use crate::board::{
    BoardOutline, BoundarySegment, CircularBoardBoundary, DesignRules, MechanicalHole, MechanicalSlot, Result,
};
use crate::geometry::{
    Point, RoundedConvexShape, point_in_polygon, point_on_segment, ring_edges, segment_at_least_squared,
    segment_in_polygon, segments_intersect, shapes_clear,
};
use crate::pyfloat::round_int;

pub fn signed_area_twice(vertices: &[Point]) -> i128 {
    ring_edges(vertices)
        .into_iter()
        .map(|(a, b)| i128::from(a.x_nm) * i128::from(b.y_nm) - i128::from(b.x_nm) * i128::from(a.y_nm))
        .sum()
}

/// `circle_query_ring`: a bounded inscribed ring certified with exact predicates.
pub fn circle_query_ring(circle: &CircularBoardBoundary) -> Result<Vec<Point>> {
    let (radius, error) = (circle.radius_nm, circle.maximum_chord_error_nm);
    let ratio = (error - 2) as f64 / radius as f64;
    let mut count: i64 = if ratio > 1e-15 { 12.max((PI / (1.0 - ratio).max(-1.0).acos()).ceil() as i64) } else { 4097 };
    count = ((count + 3) / 4) * 4;
    let minimum = i128::from(radius - error).pow(2);
    let origin = Point::new(0, 0);
    while count <= 4096 {
        let scale = (radius - 1) as f64;
        let offsets: Vec<Point> = (0..count)
            .map(|i| {
                let angle = 2.0 * PI * i as f64 / count as f64;
                Point::new(round_int(scale * angle.cos()), round_int(scale * angle.sin()))
            })
            .collect();
        let mut distinct = offsets.clone();
        distinct.sort_unstable();
        distinct.dedup();
        let radius_squared = i128::from(radius).pow(2);
        if distinct.len() == offsets.len()
            && offsets.iter().all(|p| i128::from(p.x_nm).pow(2) + i128::from(p.y_nm).pow(2) <= radius_squared)
            && ring_edges(&offsets).into_iter().all(|(a, b)| segment_at_least_squared(origin, origin, a, b, minimum))
        {
            return Ok(offsets
                .into_iter()
                .map(|p| Point::new(circle.center.x_nm + p.x_nm, circle.center.y_nm + p.y_nm))
                .collect());
        }
        count += 4;
    }
    Err("circular board query tolerance requires more than 4096 distinct vertices".to_owned())
}

/// `boundary_line_pairs`: the straight boundary segments an edge may name.
pub fn boundary_line_pairs(outline: &BoardOutline) -> Vec<(Point, Point)> {
    if outline.circular_boundary.is_some() {
        return Vec::new();
    }
    if let Some(path) = &outline.boundary_path {
        return path
            .segments
            .iter()
            .filter_map(|segment| match segment {
                BoundarySegment::Line { start, end, .. } => Some((*start, *end)),
                BoundarySegment::Arc { .. } => None,
            })
            .collect();
    }
    ring_edges(&outline.vertices)
}

/// `validated_ring`: a simple ring of distinct vertices with nonzero area.
pub fn validated_ring(vertices: Vec<Point>) -> Result<Vec<Point>> {
    let mut vertices = vertices;
    if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
        vertices.pop();
    }
    let mut distinct = vertices.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if vertices.len() < 3 || distinct.len() != vertices.len() {
        return Err("mechanical boundary requires at least three distinct, unrepeated vertices".to_owned());
    }
    if signed_area_twice(&vertices) == 0 {
        return Err("mechanical boundary cannot have zero area".to_owned());
    }
    let edges = ring_edges(&vertices);
    let count = vertices.len();
    for (i, &(a, b)) in edges.iter().enumerate() {
        let c = vertices[(i + 2) % count];
        if point_on_segment(c, a, b) || point_on_segment(a, b, c) {
            return Err("mechanical boundary has overlapping adjacent edges".to_owned());
        }
        for (j, &(c, d)) in edges.iter().enumerate().skip(i + 1) {
            if j == i + 1 || (i == 0 && j == edges.len() - 1) {
                continue;
            }
            if segments_intersect(a, b, c, d) {
                return Err("mechanical boundary self-intersects".to_owned());
            }
        }
    }
    Ok(vertices)
}

fn polygon_shape(vertices: &[Point]) -> RoundedConvexShape {
    RoundedConvexShape::new(vertices.to_vec(), 0).expect("validated ring")
}

/// `validate_cutouts`.
pub fn validate_cutouts(outline: &BoardOutline) -> Result<()> {
    let mut ids: Vec<&str> = outline.cutouts.iter().map(|cutout| cutout.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != outline.cutouts.len() {
        return Err("board cutout ids must be unique".to_owned());
    }
    let bare = BoardOutline {
        vertices: outline.vertices.clone(),
        cutouts: Vec::new(),
        circular_boundary: outline.circular_boundary,
        boundary_path: outline.boundary_path.clone(),
    };
    for (i, cutout) in outline.cutouts.iter().enumerate() {
        let shape = polygon_shape(&cutout.vertices);
        if !shape_in_outline(&shape, &bare, 1)? {
            return Err(format!("cutout {} must lie strictly inside the board", repr(&cutout.id)));
        }
        for other in &outline.cutouts[..i] {
            if !shapes_clear(&shape, &polygon_shape(&other.vertices), 1) {
                return Err("board cutouts cannot touch, intersect or nest".to_owned());
            }
        }
    }
    Ok(())
}

/// `shape_in_outline`: the swept shape lies in board material with `clearance_nm`.
pub fn shape_in_outline(shape: &RoundedConvexShape, outline: &BoardOutline, clearance_nm: i64) -> Result<bool> {
    if clearance_nm < 0 {
        return Err("mechanical query clearance must be nonnegative".to_owned());
    }
    let required = i128::from(shape.radius_nm) + i128::from(clearance_nm);
    if let Some(circle) = &outline.circular_boundary {
        let available = i128::from(circle.radius_nm) - required;
        if available < 0
            || shape.spine.iter().any(|p| {
                (i128::from(p.x_nm) - i128::from(circle.center.x_nm)).pow(2)
                    + (i128::from(p.y_nm) - i128::from(circle.center.y_nm)).pow(2)
                    > available * available
            })
        {
            return Ok(false);
        }
    } else {
        if !shape.spine.iter().all(|&p| point_in_polygon(p, &outline.vertices)) {
            return Ok(false);
        }
        let spine = &shape.spine;
        let spine_edges = match spine.len() {
            1 => vec![(spine[0], spine[0])],
            2 => vec![(spine[0], spine[1])],
            _ => ring_edges(spine),
        };
        let outline_edges = ring_edges(&outline.vertices);
        for (a, b) in spine_edges {
            if !segment_in_polygon(a, b, &outline.vertices) {
                return Ok(false);
            }
            if required != 0
                && outline_edges.iter().any(|&(c, d)| !segment_at_least_squared(a, b, c, d, required * required))
            {
                return Ok(false);
            }
        }
    }
    let clearance = clearance_nm.max(1);
    Ok(outline.cutouts.iter().all(|cutout| shapes_clear(shape, &polygon_shape(&cutout.vertices), clearance)))
}

pub fn hole_shape(hole: &MechanicalHole) -> RoundedConvexShape {
    RoundedConvexShape::new(vec![hole.position], (hole.diameter_nm + 1) / 2).expect("positive diameter")
}

/// `slot_shape`: the outward 2 nm guard bounds oval quantization.
pub fn slot_shape(slot: &MechanicalSlot) -> RoundedConvexShape {
    RoundedConvexShape::new(vec![slot.start, slot.end], (slot.width_nm + 1) / 2 + 2).expect("positive width")
}

/// `validate_mechanical_holes` for a board with these holes, slots and rules.
pub fn validate_mechanical_holes(
    outline: &BoardOutline,
    holes: &[MechanicalHole],
    slots: &[MechanicalSlot],
    rules: &DesignRules,
) -> Result<()> {
    let mut ids: Vec<&str> = holes.iter().map(|hole| hole.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != holes.len() {
        return Err("mechanical hole ids must be unique".to_owned());
    }
    for (i, hole) in holes.iter().enumerate() {
        let shape = hole_shape(hole);
        if !shape_in_outline(&shape, outline, 1)? {
            return Err(format!("mechanical hole {} must lie strictly inside board material", repr(&hole.id)));
        }
        if holes[..i].iter().any(|other| !shapes_clear(&shape, &hole_shape(other), 1)) {
            return Err("mechanical holes cannot touch or overlap".to_owned());
        }
    }
    let mut occupied: Vec<RoundedConvexShape> = holes.iter().map(hole_shape).collect();
    if holes.iter().any(|hole| slots.iter().any(|slot| slot.id == hole.id)) {
        return Err("hole and slot IDs must not collide".to_owned());
    }
    for slot in slots {
        let shape = slot_shape(slot);
        if slot.width_nm < rules.minimum_slot_width_nm {
            return Err("routed slot width is below the board minimum slot/tool diameter".to_owned());
        }
        if !shape_in_outline(&shape, outline, 1)? {
            return Err(format!("slot {} must lie strictly inside board material", repr(&slot.id)));
        }
        let clearance = rules.minimum_hole_clearance_nm.max(1);
        if occupied.iter().any(|other| !shapes_clear(&shape, other, clearance)) {
            return Err("slots cannot touch/overlap holes or other slots or violate hole clearance".to_owned());
        }
        occupied.push(shape);
    }
    Ok(())
}

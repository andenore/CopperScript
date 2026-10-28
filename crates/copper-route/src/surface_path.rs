//! The board-containment test of `pcbir.surface_path` that terminal access
//! needs (`_track_inside_board`). The surface path router itself belongs to
//! the detailed routing streams.

use copper_drc::legality::shape_in_board;
use copper_pcb::Result;
use copper_pcb::board::PhysicalBoard;
use copper_physical::geometry::{point_in_polygon, segment_distance_squared};
use copper_physical::{Point, RoundedConvexShape};

/// `_rectangle_bounds`: only an actual axis-aligned rectangle, never its bounding box.
pub fn rectangle_bounds(outline: &[Point]) -> Option<(i64, i64, i64, i64)> {
    if outline.len() != 4 {
        return None;
    }
    let mut unique = outline.to_vec();
    unique.sort();
    unique.dedup();
    if unique.len() != 4 {
        return None;
    }
    for (index, first) in outline.iter().enumerate() {
        let second = outline[(index + 1) % 4];
        if (first.x_nm == second.x_nm) == (first.y_nm == second.y_nm) {
            return None;
        }
    }
    let mut xs: Vec<i64> = outline.iter().map(|p| p.x_nm).collect();
    let mut ys: Vec<i64> = outline.iter().map(|p| p.y_nm).collect();
    xs.sort();
    xs.dedup();
    ys.sort();
    ys.dedup();
    if xs.len() != 2 || ys.len() != 2 {
        return None;
    }
    Some((xs[0], ys[0], xs[1], ys[1]))
}

/// `_rectangle_capsule_inside`: exact erosion by half a diameter in doubled distances.
pub fn rectangle_capsule_inside(rectangle: (i64, i64, i64, i64), start: Point, end: Point, required_twice: i64) -> bool {
    let (left, top, right, bottom) = rectangle;
    let nearest = [
        start.x_nm - left,
        end.x_nm - left,
        right - start.x_nm,
        right - end.x_nm,
        start.y_nm - top,
        end.y_nm - top,
        bottom - start.y_nm,
        bottom - end.y_nm,
    ]
    .into_iter()
    .min()
    .expect("eight distances");
    2 * nearest >= required_twice
}

/// `_track_inside_board`: the track's copper, with the edge clearance, lies in board material.
pub fn track_inside_board(board: &PhysicalBoard, start: Point, end: Point, width_nm: i64) -> Result<bool> {
    let outline = &board.outline;
    if outline.circular_boundary.is_some()
        || outline.boundary_path.is_some()
        || !outline.cutouts.is_empty()
        || !board.mechanical_holes.is_empty()
        || !board.mechanical_slots.is_empty()
    {
        let shape = RoundedConvexShape::new(vec![start, end], (width_nm + 1).div_euclid(2))?;
        return shape_in_board(board, &shape, board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm);
    }
    let vertices = &outline.vertices;
    if let Some(rectangle) = rectangle_bounds(vertices) {
        return Ok(rectangle_capsule_inside(rectangle, start, end, width_nm + 2 * board.rules.minimum_clearance_nm));
    }
    if !point_in_polygon(start, vertices) || !point_in_polygon(end, vertices) {
        return Ok(false);
    }
    let required_twice = i128::from(width_nm + 2 * board.rules.minimum_clearance_nm);
    let limit = num_rational::BigRational::from_integer((required_twice * required_twice).into());
    Ok((0..vertices.len()).all(|index| {
        let (first, second) = (vertices[index], vertices[(index + 1) % vertices.len()]);
        let distance = segment_distance_squared(start, end, first, second);
        distance * num_rational::BigRational::from_integer(4.into()) >= limit
    }))
}

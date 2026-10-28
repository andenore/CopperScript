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
pub fn rectangle_capsule_inside(
    rectangle: (i64, i64, i64, i64),
    start: Point,
    end: Point,
    required_twice: i64,
) -> bool {
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

/// `surface_path` without detours: short 45-degree-first pad escapes with
/// exact-clearance fallbacks, reusing an endpoint-connected committed
/// same-layer chain. `None` when no candidate clears.
#[allow(clippy::too_many_arguments)]
pub fn surface_path(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    committed: &[copper_pcb::board::TrackSegment],
) -> Result<Option<Vec<copper_pcb::board::TrackSegment>>> {
    use std::collections::VecDeque;
    if start == end {
        return Ok(Some(Vec::new()));
    }
    let mut adjacency: indexmap::IndexMap<Point, Vec<(Point, &copper_pcb::board::TrackSegment)>> =
        indexmap::IndexMap::new();
    for track in committed {
        if track.net == net
            && track.layer == layer
            && track.width_nm >= clearance.breakout.required_width_nm(track, width_nm)
        {
            adjacency.entry(track.start).or_default().push((track.end, track));
            adjacency.entry(track.end).or_default().push((track.start, track));
        }
    }
    let mut reached: indexmap::IndexSet<Point> = indexmap::IndexSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(current) = queue.pop_front() {
        if reached.contains(&end) {
            break;
        }
        for (neighbor, track) in adjacency.get(&current).cloned().unwrap_or_default() {
            if reached.contains(&neighbor) {
                continue;
            }
            if !track_inside_board(board, track.start, track.end, track.width_nm)?
                || !clearance.can_track(net, track.start, track.end, track.width_nm, layer)?
            {
                continue;
            }
            reached.insert(neighbor);
            queue.push_back(neighbor);
        }
    }
    if reached.contains(&end) {
        return Ok(Some(Vec::new()));
    }
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    let diagonal = dx.abs().min(dy.abs());
    let mut paths: Vec<Vec<Point>> = Vec::new();
    if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
        paths.push(vec![start, end]);
    } else if dx.abs() > dy.abs() {
        let step = if dx > 0 { diagonal } else { -diagonal };
        paths.push(vec![start, Point::new(end.x_nm - step, start.y_nm), end]);
        paths.push(vec![start, Point::new(start.x_nm + step, end.y_nm), end]);
    } else {
        let step = if dy > 0 { diagonal } else { -diagonal };
        paths.push(vec![start, Point::new(start.x_nm, end.y_nm - step), end]);
        paths.push(vec![start, Point::new(end.x_nm, start.y_nm + step), end]);
    }
    if dx != 0 && dy != 0 {
        paths.push(vec![start, Point::new(start.x_nm, end.y_nm), end]);
        paths.push(vec![start, Point::new(end.x_nm, start.y_nm), end]);
    }
    let mut paths = clearance.terminal_preferences.ordered(net, layer, paths);
    if !paths.contains(&vec![start, end]) {
        paths.push(vec![start, end]);
    }
    'paths: for points in paths {
        let mut additions = Vec::new();
        for pair in points.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            if first == second {
                continue;
            }
            let pieces = clearance.route_pieces(net, first, second, width_nm, layer)?;
            for piece in &pieces {
                if !track_inside_board(board, piece.start, piece.end, piece.width_nm)?
                    || !clearance.can_track(net, piece.start, piece.end, piece.width_nm, layer)?
                {
                    continue 'paths;
                }
            }
            for piece in pieces {
                let mut reused = false;
                for old in committed {
                    if old.net == net
                        && old.layer == layer
                        && ((old.start == piece.start && old.end == piece.end)
                            || (old.start == piece.end && old.end == piece.start))
                        && old.width_nm >= piece.width_nm
                        && track_inside_board(board, old.start, old.end, old.width_nm)?
                        && clearance.can_track(net, old.start, old.end, old.width_nm, layer)?
                    {
                        reused = true;
                        break;
                    }
                }
                if !reused {
                    additions.push(piece);
                }
            }
        }
        return crate::route_style::chamfer_ordinary_corners(board, &additions, &[], clearance, Some(committed))
            .map(Some);
    }
    Ok(None)
}

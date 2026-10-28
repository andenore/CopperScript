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
    surface_path_detour(board, clearance, net, start, end, width_nm, layer, committed, 0, 500_000)
}

/// `surface_path(..., maximum_detour_nm=, detour_step_nm=)`: also three-leg
/// detours offset by multiples of the step.
#[allow(clippy::too_many_arguments)]
pub fn surface_path_detour(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    committed: &[copper_pcb::board::TrackSegment],
    maximum_detour_nm: i64,
    detour_step_nm: i64,
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
    if maximum_detour_nm != 0 {
        if maximum_detour_nm < 0 || detour_step_nm <= 0 {
            return Err(copper_pcb::Error::Value("surface-path detour bounds must be positive".to_owned()));
        }
        let mut offset = detour_step_nm;
        while offset <= maximum_detour_nm {
            for sign in [-1, 1] {
                if start.y_nm == end.y_nm {
                    let y = start.y_nm + sign * offset;
                    paths.push(vec![start, Point::new(start.x_nm, y), Point::new(end.x_nm, y), end]);
                } else if start.x_nm == end.x_nm {
                    let x = start.x_nm + sign * offset;
                    paths.push(vec![start, Point::new(x, start.y_nm), Point::new(x, end.y_nm), end]);
                } else {
                    let (x, y) = (start.x_nm + sign * offset, start.y_nm + sign * offset);
                    paths.push(vec![start, Point::new(x, start.y_nm), Point::new(x, end.y_nm), end]);
                    paths.push(vec![start, Point::new(start.x_nm, y), Point::new(end.x_nm, y), end]);
                }
            }
            offset += detour_step_nm;
        }
    }
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

/// `via_inside_board`: the via's copper, with the edge clearance, lies in board material.
pub fn via_inside_board(board: &PhysicalBoard, position: Point, size_nm: i64) -> Result<bool> {
    let outline = &board.outline;
    if outline.circular_boundary.is_some()
        || outline.boundary_path.is_some()
        || !outline.cutouts.is_empty()
        || !board.mechanical_holes.is_empty()
        || !board.mechanical_slots.is_empty()
    {
        let shape = RoundedConvexShape::new(vec![position], (size_nm + 1).div_euclid(2))?;
        return shape_in_board(board, &shape, board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm);
    }
    let vertices = &outline.vertices;
    if let Some(rectangle) = rectangle_bounds(vertices) {
        return Ok(rectangle_capsule_inside(
            rectangle,
            position,
            position,
            size_nm + 2 * board.rules.minimum_clearance_nm,
        ));
    }
    if !point_in_polygon(position, vertices) {
        return Ok(false);
    }
    let required_twice = i128::from(size_nm + 2 * board.rules.minimum_clearance_nm);
    let limit = num_rational::BigRational::from_integer((required_twice * required_twice).into());
    Ok((0..vertices.len()).all(|index| {
        let (first, second) = (vertices[index], vertices[(index + 1) % vertices.len()]);
        let distance = copper_physical::geometry::point_segment_distance_squared(position, first, second);
        distance * num_rational::BigRational::from_integer(4.into()) >= limit
    }))
}

/// The bounds of `surface_path_to_via`.
pub struct ViaSearch<'a> {
    pub step_nm: i64,
    pub radius_nm: i64,
    pub via_size_nm: i64,
    pub via_drill_nm: i64,
    pub via_layers: (copper_physical::CopperLayer, copper_physical::CopperLayer),
    pub accept_via: &'a dyn Fn(Point) -> bool,
    pub via_targets: &'a [Point],
    pub state_budget: i64,
}

/// `surface_path_to_via`: bounded octilinear package escape to any
/// exact-clearance legal via site.
#[allow(clippy::too_many_arguments)]
pub fn surface_path_to_via(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    search: &ViaSearch<'_>,
) -> Result<Option<(Vec<copper_pcb::board::TrackSegment>, Point)>> {
    use indexmap::IndexMap;
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let step = search.step_nm;
    if step <= 0 || search.radius_nm <= 0 || search.state_budget <= 0 {
        return Err(copper_pcb::Error::Value("surface via-search bounds must be positive".to_owned()));
    }
    const DIRECTIONS: [(i64, i64); 8] = [(1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1)];
    let targets: indexmap::IndexSet<Point> = search
        .via_targets
        .iter()
        .copied()
        .filter(|p| (p.x_nm - start.x_nm).rem_euclid(step) == 0 && (p.y_nm - start.y_nm).rem_euclid(step) == 0)
        .collect();
    let mut ordered: Vec<Point> = targets.iter().copied().collect();
    ordered.sort_by_key(|p| (p.x_nm, p.y_nm));
    let goals: Vec<(i64, i64)> = ordered
        .iter()
        .map(|p| ((p.x_nm - start.x_nm).div_euclid(step), (p.y_nm - start.y_nm).div_euclid(step)))
        .collect();
    let heuristic = |x: i64, y: i64| -> i64 {
        goals
            .iter()
            .map(|(gx, gy)| {
                let (a, b) = ((gx - x).abs(), (gy - y).abs());
                10 * a.max(b) + 4 * a.min(b)
            })
            .min()
            .unwrap_or(0)
    };
    let mut queue = BinaryHeap::from([Reverse((heuristic(0, 0), 0i64, 0i64, 0i64, 0i64, -1i64))]);
    let mut best: IndexMap<(i64, i64, i64), i64> = IndexMap::from_iter([((0, 0, -1), 0)]);
    let mut parent: IndexMap<(i64, i64, i64), Option<(i64, i64, i64)>> = IndexMap::from_iter([((0, 0, -1), None)]);
    let radius_squared = i128::from(search.radius_nm) * i128::from(search.radius_nm);
    let mut via_legal: IndexMap<(i64, i64), bool> = IndexMap::new();
    let mut edge_legal: IndexMap<(i64, i64, i64, i64), bool> = IndexMap::new();
    let at = |x: i64, y: i64| Point::new(start.x_nm + x * step, start.y_nm + y * step);
    let mut expanded = 0;
    while expanded < search.state_budget {
        let Some(Reverse((_, cost, _, x, y, heading))) = queue.pop() else { break };
        let state = (x, y, heading);
        if best.get(&state) != Some(&cost) {
            continue;
        }
        expanded += 1;
        let point = at(x, y);
        if (x != 0 || y != 0) && (targets.is_empty() || targets.contains(&point)) {
            let legal = match via_legal.get(&(x, y)) {
                Some(legal) => *legal,
                None => {
                    let legal = (search.accept_via)(point)
                        && via_inside_board(board, point, search.via_size_nm)?
                        && clearance.can_via(
                            net,
                            point,
                            search.via_size_nm,
                            search.via_layers.0,
                            search.via_layers.1,
                            Some(search.via_drill_nm),
                            false,
                            None,
                        )?;
                    via_legal.insert((x, y), legal);
                    legal
                }
            };
            if legal {
                let mut nodes: Vec<Point> = Vec::new();
                let mut cursor = Some(state);
                while let Some(current) = cursor {
                    nodes.push(at(current.0, current.1));
                    cursor = parent[&current];
                }
                nodes.reverse();
                let mut turns = vec![nodes[0]];
                for index in 1..nodes.len().saturating_sub(1) {
                    let (before, current, after) = (nodes[index - 1], nodes[index], nodes[index + 1]);
                    if (current.x_nm - before.x_nm) * (after.y_nm - current.y_nm)
                        != (current.y_nm - before.y_nm) * (after.x_nm - current.x_nm)
                    {
                        turns.push(current);
                    }
                }
                turns.push(nodes[nodes.len() - 1]);
                let mut pieces = Vec::new();
                for pair in turns.windows(2) {
                    pieces.extend(clearance.route_pieces(net, pair[0], pair[1], width_nm, layer)?);
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
                    return Ok(Some((pieces, point)));
                }
            }
        }
        for (next_heading, (dx, dy)) in DIRECTIONS.iter().enumerate() {
            let next_heading = next_heading as i64;
            let (nx, ny) = (x + dx, y + dy);
            if i128::from(nx * step) * i128::from(nx * step) + i128::from(ny * step) * i128::from(ny * step)
                > radius_squared
            {
                continue;
            }
            let edge = (x, y, *dx, *dy);
            let legal = match edge_legal.get(&edge) {
                Some(legal) => *legal,
                None => {
                    let neighbor = at(nx, ny);
                    let legal = track_inside_board(board, point, neighbor, width_nm)?
                        && clearance.can_route(net, point, neighbor, width_nm, layer)?;
                    edge_legal.insert(edge, legal);
                    legal
                }
            };
            if !legal {
                continue;
            }
            let turn_steps = if heading >= 0 {
                let difference = (next_heading - heading).abs();
                difference.min(8 - difference)
            } else {
                0
            };
            let step_cost = (if *dx != 0 && *dy != 0 { 14 } else { 10 }) + 7 * turn_steps;
            let candidate = cost + step_cost;
            let next_state = (nx, ny, next_heading);
            if candidate >= *best.get(&next_state).unwrap_or(&(1 << 60)) {
                continue;
            }
            best.insert(next_state, candidate);
            parent.insert(next_state, Some(state));
            queue.push(Reverse((candidate + heuristic(nx, ny), candidate, turn_steps, nx, ny, next_heading)));
        }
    }
    Ok(None)
}

/// `surface_path_between`: bounded local octilinear A* with exact, possibly
/// off-grid terminals (no vias, no movable-obstacle exemptions). The bounds
/// are computational, not electrical limits. Without a progress observer.
#[allow(clippy::too_many_arguments)]
pub fn surface_path_between(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    step_nm: i64,
    maximum_detour_nm: i64,
    state_budget: i64,
) -> Result<Option<Vec<copper_pcb::board::TrackSegment>>> {
    Ok(surface_path_between_counted(
        board,
        clearance,
        net,
        start,
        end,
        width_nm,
        layer,
        step_nm,
        maximum_detour_nm,
        state_budget,
    )?
    .0)
}

/// `surface_path_between` and the states it expanded (its finished event's
/// `expanded_states`).
#[allow(clippy::too_many_arguments)]
pub fn surface_path_between_counted(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    step_nm: i64,
    maximum_detour_nm: i64,
    state_budget: i64,
) -> Result<(Option<Vec<copper_pcb::board::TrackSegment>>, i64)> {
    let search = surface_path_between_observed(
        board,
        clearance,
        net,
        start,
        end,
        width_nm,
        layer,
        step_nm,
        maximum_detour_nm,
        state_budget,
        false,
    )?;
    Ok((search.path, search.expanded_states))
}

/// One `surface_path_between` search as its finished progress event reports it.
#[derive(Clone, Debug)]
pub struct SurfaceBridgeSearch {
    pub path: Option<Vec<copper_pcb::board::TrackSegment>>,
    pub expanded_states: i64,
    /// `found`, `state_budget_exhausted` or `search_exhausted`.
    pub outcome: &'static str,
    /// The first 32 sampled rejection blockers by identity (with an observer).
    pub blocker_records: Vec<crate::routing_clearance::RoutingBlocker>,
}

/// `surface_path_between` with an observer installed when `observed`: up to
/// 16 rejected moves name their exact blockers.
#[allow(clippy::too_many_arguments)]
pub fn surface_path_between_observed(
    board: &PhysicalBoard,
    clearance: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    width_nm: i64,
    layer: copper_physical::CopperLayer,
    step_nm: i64,
    maximum_detour_nm: i64,
    state_budget: i64,
    observed: bool,
) -> Result<SurfaceBridgeSearch> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    if step_nm <= 0 || maximum_detour_nm < 0 || state_budget <= 0 {
        return Err(crate::Error::Value("surface terminal-search bounds are invalid".to_owned()));
    }
    const DIRECTIONS: [(i64, i64); 8] = [(1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1)];
    let (left, right) = (start.x_nm.min(end.x_nm), start.x_nm.max(end.x_nm));
    let (top, bottom) = (start.y_nm.min(end.y_nm), start.y_nm.max(end.y_nm));
    let heuristic = |point: Point| {
        let (dx, dy) = ((point.x_nm - end.x_nm).abs(), (point.y_nm - end.y_nm).abs());
        10 * dx.max(dy) + 4 * dx.min(dy)
    };
    let at = |x: i64, y: i64| Point::new(start.x_nm + x * step_nm, start.y_nm + y * step_nm);
    type State = (i64, i64, i64);
    let initial: State = (0, 0, -1);
    let mut queue = BinaryHeap::new();
    queue.push(Reverse((heuristic(start), 0i64, 0i64, 0i64, -1i64)));
    let mut best: indexmap::IndexMap<State, i64> = indexmap::IndexMap::new();
    let mut parent: indexmap::IndexMap<State, Option<State>> = indexmap::IndexMap::new();
    best.insert(initial, 0);
    parent.insert(initial, None);
    let mut expanded = 0;
    let mut failure_blockers = std::collections::BTreeSet::new();
    let mut rejection_samples = 0;
    while expanded < state_budget {
        let Some(Reverse((_, cost, x, y, heading))) = queue.pop() else { break };
        let state = (x, y, heading);
        if best.get(&state) != Some(&cost) {
            continue;
        }
        expanded += 1;
        let point = at(x, y);
        if (point.x_nm - end.x_nm).abs().max((point.y_nm - end.y_nm).abs()) <= 2 * step_nm {
            let tail = crate::pin_escape::checked_access_paths(
                board, clearance, net, point, end, width_nm, layer, false, true, None, true,
            )?
            .into_iter()
            .next();
            if let Some(tail) = tail {
                let mut nodes = Vec::new();
                let mut cursor = Some(state);
                while let Some(current) = cursor {
                    nodes.push(at(current.0, current.1));
                    cursor = parent[&current];
                }
                nodes.reverse();
                let mut turns = vec![nodes[0]];
                for i in 1..nodes.len().saturating_sub(1) {
                    let (a, b, c) = (nodes[i - 1], nodes[i], nodes[i + 1]);
                    if i128::from(b.x_nm - a.x_nm) * i128::from(c.y_nm - b.y_nm)
                        != i128::from(b.y_nm - a.y_nm) * i128::from(c.x_nm - b.x_nm)
                    {
                        turns.push(b);
                    }
                }
                if nodes.last() != turns.last() {
                    turns.push(*nodes.last().expect("nodes"));
                }
                let mut pieces = Vec::new();
                for pair in turns.windows(2) {
                    pieces.extend(clearance.route_pieces(net, pair[0], pair[1], width_nm, layer)?);
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
                    pieces.extend(tail);
                    return Ok(SurfaceBridgeSearch {
                        path: Some(pieces),
                        expanded_states: expanded,
                        outcome: "found",
                        blocker_records: Vec::new(),
                    });
                }
            }
        }
        for (next_heading, (dx, dy)) in DIRECTIONS.iter().enumerate() {
            let next_heading = next_heading as i64;
            let (nx, ny) = (x + dx, y + dy);
            let neighbor = at(nx, ny);
            if !(left - maximum_detour_nm <= neighbor.x_nm
                && neighbor.x_nm <= right + maximum_detour_nm
                && top - maximum_detour_nm <= neighbor.y_nm
                && neighbor.y_nm <= bottom + maximum_detour_nm)
            {
                continue;
            }
            if !track_inside_board(board, point, neighbor, width_nm)?
                || !clearance.can_route(net, point, neighbor, width_nm, layer)?
            {
                if observed && rejection_samples < 16 {
                    rejection_samples += 1;
                    for piece in clearance.route_pieces(net, point, neighbor, width_nm, layer)? {
                        failure_blockers.extend(clearance.track_blocker_records(&piece)?);
                    }
                }
                continue;
            }
            let difference = if heading >= 0 { (next_heading - heading).abs() } else { 0 };
            let turn = difference.min(8 - difference);
            let candidate = cost + ((if *dx != 0 && *dy != 0 { 14 } else { 10 }) + 7 * turn) * step_nm;
            let next_state = (nx, ny, next_heading);
            if candidate >= *best.get(&next_state).unwrap_or(&(1 << 62)) {
                continue;
            }
            best.insert(next_state, candidate);
            parent.insert(next_state, Some(state));
            queue.push(Reverse((candidate + heuristic(neighbor), candidate, nx, ny, next_heading)));
        }
    }
    let mut blocker_records: Vec<_> = failure_blockers.into_iter().collect();
    blocker_records.sort_by(|a, b| a.identity.cmp(&b.identity).then_with(|| a.cmp(b)));
    blocker_records.truncate(32);
    Ok(SurfaceBridgeSearch {
        path: None,
        expanded_states: expanded,
        outcome: if queue.is_empty() { "search_exhausted" } else { "state_budget_exhausted" },
        blocker_records,
    })
}

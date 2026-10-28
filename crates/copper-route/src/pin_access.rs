//! Exact, bounded local copper paths from a pad to a coarse route guide
//! (`pcbir.pin_access`).
//!
//! This is a pin-access search, not the full-board detailed router. It only
//! examines a small window and never reserves copper; candidate sets are
//! negotiated by later routing stages.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::{Error, Result};
use copper_physical::{CopperLayer, Point};

use crate::routing_clearance::RoutingClearanceIndex;

/// `LocalAccessStats`: optional search observations; never acceptance or an allowance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalAccessStats {
    pub expanded_states: i64,
    pub discovered_nodes: i64,
    pub outcome: String,
}

/// `segment_cache`: memoised exact segment checks for one index state.
pub type SegmentCache = IndexMap<(String, CopperLayer, i64, Point, Point), bool>;

/// The search bounds of `local_access_path`.
#[derive(Clone, Copy, Debug)]
pub struct LocalAccessBounds {
    pub step_nm: i64,
    pub detour_nm: i64,
    pub maximum_states: i64,
}

fn distance(first: Point, second: Point) -> i64 {
    (first.x_nm - second.x_nm).abs() + (first.y_nm - second.y_nm).abs()
}

fn collinear(first: Point, middle: Point, last: Point) -> bool {
    i128::from(middle.x_nm - first.x_nm) * i128::from(last.y_nm - middle.y_nm)
        == i128::from(middle.y_nm - first.y_nm) * i128::from(last.x_nm - middle.x_nm)
}

/// Python's `sorted(set(range(start, stop, step)) | extra)`.
fn axis(low: i64, high: i64, step: i64, extra: [i64; 2]) -> Vec<i64> {
    let mut values: IndexSet<i64> = IndexSet::new();
    let mut value = low;
    while value < high + 1 {
        values.insert(value);
        value += step;
    }
    values.extend(extra);
    let mut sorted: Vec<i64> = values.into_iter().collect();
    sorted.sort_unstable();
    sorted
}

/// `_legal_segment`: an exact-clearance octilinear segment check.
pub fn legal_segment(
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    layer: CopperLayer,
    width_nm: i64,
) -> Result<bool> {
    if start == end {
        return Ok(true);
    }
    let (dx, dy) = ((end.x_nm - start.x_nm).abs(), (end.y_nm - start.y_nm).abs());
    // Endpoint coordinates split grid intervals unequally: adjacent indices do
    // not imply a physical 45-degree step. The direct shortcut obeys the same
    // eight-direction contract as every maze edge.
    if dx != 0 && dy != 0 && dx != dy {
        return Ok(false);
    }
    // can_route keeps the whole capsule inside the outline (shape_in_board).
    clearance.can_route(net, start, end, width_nm, layer)
}

/// `local_access_path`: an exact-clearance, eight-direction path in a bounded local window.
///
/// `segment_cache` memoises exact segment checks across calls; the caller must
/// drop it whenever `clearance` gains or loses copper.
#[allow(clippy::too_many_arguments)]
pub fn local_access_path(
    _board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    layer: CopperLayer,
    width_nm: i64,
    bounds: LocalAccessBounds,
    mut stats: Option<&mut LocalAccessStats>,
    mut segment_cache: Option<&mut SegmentCache>,
) -> Result<Option<Vec<TrackSegment>>> {
    let mut legal = |a: Point, b: Point| -> Result<bool> {
        match segment_cache.as_deref_mut() {
            None => legal_segment(clearance, net, a, b, layer, width_nm),
            Some(cache) => {
                let key = (net.to_owned(), layer, width_nm, a, b);
                if let Some(result) = cache.get(&key) {
                    return Ok(*result);
                }
                let result = legal_segment(clearance, net, a, b, layer, width_nm)?;
                cache.insert(key, result);
                Ok(result)
            }
        }
    };
    if start == end {
        if let Some(stats) = stats {
            stats.outcome = "same_point".to_owned();
        }
        return Ok(Some(Vec::new()));
    }
    if legal(start, end)? {
        if let Some(stats) = stats.as_deref_mut() {
            stats.outcome = "direct".to_owned();
        }
        return Ok(Some(clearance.route_pieces(net, start, end, width_nm, layer)?));
    }
    if bounds.step_nm.min(bounds.detour_nm).min(bounds.maximum_states) <= 0 {
        return Err(Error::Value("local access search bounds must be positive".to_owned()));
    }
    let xs = axis(
        start.x_nm.min(end.x_nm) - bounds.detour_nm,
        start.x_nm.max(end.x_nm) + bounds.detour_nm,
        bounds.step_nm,
        [start.x_nm, end.x_nm],
    );
    let ys = axis(
        start.y_nm.min(end.y_nm) - bounds.detour_nm,
        start.y_nm.max(end.y_nm) + bounds.detour_nm,
        bounds.step_nm,
        [start.y_nm, end.y_nm],
    );
    let index = |values: &[i64], value: i64| values.iter().position(|v| *v == value).expect("axis value");
    let start_node = (index(&xs, start.x_nm), index(&ys, start.y_nm));
    let target = (index(&xs, end.x_nm), index(&ys, end.y_nm));
    let mut best: IndexMap<(usize, usize), i64> = IndexMap::from([(start_node, 0)]);
    let mut previous: IndexMap<(usize, usize), Option<(usize, usize)>> = IndexMap::from([(start_node, None)]);
    // (estimate, remaining, cost, serial, node): a total order, so the pop
    // sequence is heapq's.
    let mut queue: BinaryHeap<Reverse<(i64, i64, i64, u64, (usize, usize))>> = BinaryHeap::new();
    let mut serial = 0;
    queue.push(Reverse((distance(start, end), distance(start, end), 0, serial, start_node)));
    let mut expanded = 0;
    while expanded < bounds.maximum_states {
        let Some(Reverse((_, _, cost, _, node))) = queue.pop() else { break };
        if best.get(&node) != Some(&cost) {
            continue;
        }
        if node == target {
            break;
        }
        expanded += 1;
        let current = Point::new(xs[node.0], ys[node.1]);
        for (dx, dy) in [(-1i64, 0i64), (0, -1), (0, 1), (1, 0), (-1, -1), (-1, 1), (1, -1), (1, 1)] {
            let (nx, ny) = (node.0 as i64 + dx, node.1 as i64 + dy);
            if nx < 0 || ny < 0 || nx >= xs.len() as i64 || ny >= ys.len() as i64 {
                continue;
            }
            let neighbor = (nx as usize, ny as usize);
            let point = Point::new(xs[neighbor.0], ys[neighbor.1]);
            if !legal(current, point)? {
                continue;
            }
            let candidate_cost = cost + distance(current, point);
            if candidate_cost >= best.get(&neighbor).copied().unwrap_or(1 << 62) {
                continue;
            }
            best.insert(neighbor, candidate_cost);
            previous.insert(neighbor, Some(node));
            serial += 1;
            // Equal estimated costs are common with the Manhattan metric: prefer
            // progress to the goal over flooding the shortest-path rectangle.
            let remaining = distance(point, end);
            queue.push(Reverse((candidate_cost + remaining, remaining, candidate_cost, serial, neighbor)));
        }
    }
    if let Some(stats) = stats {
        stats.expanded_states = expanded;
        stats.discovered_nodes = previous.len() as i64;
        stats.outcome = if previous.contains_key(&target) {
            "connected"
        } else if !queue.is_empty() {
            "state_limit"
        } else {
            "frontier_exhausted"
        }
        .to_owned();
    }
    if !previous.contains_key(&target) {
        return Ok(None);
    }
    let mut points = vec![end];
    let mut node = target;
    while node != start_node {
        node = previous[&node].expect("reached node has a parent");
        points.push(Point::new(xs[node.0], ys[node.1]));
    }
    points.reverse();
    // Compact collinear steps without changing their already-checked geometry.
    let mut compact = vec![points[0]];
    for point in &points[1..] {
        if compact.len() > 1 && collinear(compact[compact.len() - 2], compact[compact.len() - 1], *point) {
            *compact.last_mut().expect("compact") = *point;
        } else {
            compact.push(*point);
        }
    }
    let mut pieces = Vec::new();
    for pair in compact.windows(2) {
        pieces.extend(clearance.route_pieces(net, pair[0], pair[1], width_nm, layer)?);
    }
    Ok(Some(pieces))
}

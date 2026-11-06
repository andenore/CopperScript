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

use crate::routing_clearance::{RoutingBlocker, RoutingClearanceIndex};

/// A snapshot-local geometric match, never generated-mask ownership authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAccessSourceKeepout {
    pub source: &'static str,
    pub index: usize,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAccessBlockerSources {
    pub identity: String,
    pub occurrence: String,
    pub status: &'static str,
    pub match_count: usize,
    pub matches: Vec<LocalAccessSourceKeepout>,
}

struct SourceRegions {
    board_keepout_count: usize,
    resolved: Vec<copper_physical::CopperKeepout>,
    macro_regions: Vec<copper_physical::CopperKeepout>,
}

impl SourceRegions {
    fn from_board(board: &PhysicalBoard) -> Result<Self> {
        Ok(Self {
            board_keepout_count: board.copper_keepouts.len(),
            resolved: copper_pcb::keepouts::resolved_copper_keepouts(board)?,
            macro_regions: copper_pcb::hard_macros::macro_reservations(board)?,
        })
    }

    fn matches(&self, blocker: &RoutingBlocker) -> LocalAccessBlockerSources {
        let mut matches = Vec::new();
        let same_geometry = |item: &copper_physical::CopperKeepout| {
            let mut layers: Vec<_> = item.layers.iter().map(|layer| layer.value().to_owned()).collect();
            layers.sort();
            layers == blocker.layers
                && item.vertices.iter().map(|point| (point.x_nm, point.y_nm)).eq(blocker.positions_nm.iter().copied())
        };
        if blocker.identity.starts_with("region:") {
            for (index, item) in self.resolved.iter().enumerate() {
                if item.block_tracks && same_geometry(item) {
                    matches.push(LocalAccessSourceKeepout {
                        source: if index < self.board_keepout_count {
                            "board.copper_keepouts"
                        } else {
                            "resolved_copper_keepouts"
                        },
                        index,
                        id: item.id.clone(),
                    });
                }
            }
            // The index turns protected macro regions into track blockers
            // independently of their source block_tracks flag.
            for (index, item) in self.macro_regions.iter().enumerate() {
                if same_geometry(item) {
                    matches.push(LocalAccessSourceKeepout { source: "macro_reservations", index, id: item.id.clone() });
                }
            }
        }
        let match_count = matches.len();
        LocalAccessBlockerSources {
            identity: blocker.identity.clone(),
            occurrence: blocker.occurrence.clone(),
            status: if !blocker.identity.starts_with("region:") {
                "not_a_static_region"
            } else if match_count == 0 {
                "no_exact_geometry_match"
            } else if match_count == 1 {
                "unique_exact_geometry_match"
            } else {
                "ambiguous_exact_geometry_matches"
            },
            match_count,
            matches: matches.into_iter().take(4).collect(),
        }
    }
}

/// One already-rejected edge from a local search that admitted no successor.
/// Region identities describe geometry only; generated-mask ownership is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAccessRejection {
    pub start: Point,
    pub end: Point,
    pub layer: CopperLayer,
    pub requested_width_nm: i64,
    pub reason: &'static str,
    pub pieces: Vec<TrackSegment>,
    pub omitted_pieces: usize,
    pub blockers: Vec<RoutingBlocker>,
    pub omitted_blockers: usize,
    pub blocker_sources: Vec<LocalAccessBlockerSources>,
}

/// `LocalAccessStats`: optional search observations; never acceptance or an allowance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalAccessStats {
    pub expanded_states: i64,
    pub discovered_nodes: i64,
    pub outcome: String,
    pub first_step_rejections: Vec<LocalAccessRejection>,
}

impl LocalAccessStats {
    /// At most three new report records, each <= 900 UTF-8 bytes. All rejected
    /// directions are summarized; only the first two edges carry blocker
    /// geometry, at most two positions per blocker. Omission is explicit.
    pub fn first_step_diagnostics(&self, omitted_attempts: usize) -> Vec<String> {
        if self.first_step_rejections.is_empty() {
            return Vec::new();
        }
        let mut summary = format!(
            "first_step_scope=no_successor_first_local_attempt generated_region_role=unknown rejected={} omitted_other_attempts={} edges=[",
            self.first_step_rejections.len(),
            omitted_attempts,
        );
        let mut shown = 0;
        for edge in &self.first_step_rejections {
            let item = format!(
                "({},{})nm:{}:blockers={}:omitted_blockers={};",
                edge.end.x_nm,
                edge.end.y_nm,
                edge.reason,
                edge.blockers.len(),
                edge.omitted_blockers,
            );
            if summary.len() + item.len() > 800 {
                break;
            }
            summary.push_str(&item);
            shown += 1;
        }
        summary.push_str(&format!(
            "] omitted_edge_summaries={} detailed_edges={} omitted_edge_details={}",
            self.first_step_rejections.len() - shown,
            self.first_step_rejections.len().min(2),
            self.first_step_rejections.len().saturating_sub(2)
        ));
        let mut records = vec![summary];
        for edge in self.first_step_rejections.iter().take(2) {
            let mut record = format!(
                "first_step_edge=({},{})nm->({},{})nm@{} requested_width_nm={} kind={} piece_widths_nm={:?} omitted_pieces={} blockers=[",
                edge.start.x_nm,
                edge.start.y_nm,
                edge.end.x_nm,
                edge.end.y_nm,
                edge.layer.value(),
                edge.requested_width_nm,
                edge.reason,
                edge.pieces.iter().map(|piece| piece.width_nm).collect::<Vec<_>>(),
                edge.omitted_pieces,
            );
            let mut shown = 0;
            for blocker in &edge.blockers {
                let sources = edge
                    .blocker_sources
                    .iter()
                    .find(|sources| sources.identity == blocker.identity && sources.occurrence == blocker.occurrence);
                let source_text = sources.map_or_else(|| "source_status=unavailable".to_owned(), |sources| {
                    let rows: Vec<_> = sources.matches.iter().map(|item|
                        format!("{}[{}] id={:?}", item.source, item.index, item.id)).collect();
                    format!("source_context=passed_board source_status={} source_match_count={} sources={:?} omitted_source_matches={}",
                        sources.status, sources.match_count, rows, sources.match_count - sources.matches.len())
                });
                let item = format!(
                    "reason={:?} identity={:?} positions_nm={:?} omitted_positions={} {} ;",
                    blocker.reason,
                    blocker.identity,
                    blocker.positions_nm.iter().take(2).collect::<Vec<_>>(),
                    blocker.positions_nm.len().saturating_sub(2),
                    source_text,
                );
                if record.len() + item.len() > 800 {
                    break;
                }
                record.push_str(&item);
                shown += 1;
            }
            record.push_str(&format!("] omitted_blockers={}", edge.omitted_blockers + edge.blockers.len() - shown));
            records.push(record);
        }
        records
    }
}

fn first_step_rejection(
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    start: Point,
    end: Point,
    layer: CopperLayer,
    width_nm: i64,
    source_regions: Option<&SourceRegions>,
) -> Result<LocalAccessRejection> {
    let (dx, dy) = ((end.x_nm - start.x_nm).abs(), (end.y_nm - start.y_nm).abs());
    let octilinear = dx == 0 || dy == 0 || dx == dy;
    let pieces = if octilinear { clearance.route_pieces(net, start, end, width_nm, layer)? } else { Vec::new() };
    let mut blockers = IndexMap::new();
    for piece in &pieces {
        for blocker in clearance.track_blocker_records(piece)? {
            blockers.entry((blocker.identity.clone(), blocker.occurrence.clone())).or_insert(blocker);
        }
    }
    let reason = if !octilinear {
        "non_octilinear"
    } else if blockers.is_empty() {
        "exact_rejection_without_blocker_record"
    } else {
        "exact_clearance"
    };
    let omitted_blockers = blockers.len().saturating_sub(4);
    let blockers: Vec<_> = blockers.into_values().take(4).collect();
    let blocker_sources = source_regions
        .map_or_else(Vec::new, |sources| blockers.iter().map(|blocker| sources.matches(blocker)).collect());
    Ok(LocalAccessRejection {
        start,
        end,
        layer,
        requested_width_nm: width_nm,
        reason,
        omitted_pieces: pieces.len().saturating_sub(4),
        pieces: pieces.into_iter().take(4).collect(),
        omitted_blockers,
        blockers,
        blocker_sources,
    })
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
    board: &PhysicalBoard,
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
    if let Some(stats) = stats.as_deref_mut() {
        stats.first_step_rejections.clear();
    }
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
        if let Some(stats) = stats {
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
    // Cheaply remember actual rejected first endpoints. Materialize blocker
    // geometry only if the search ultimately admits no successor at all.
    let mut rejected_first_endpoints = Vec::new();
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
                if node == start_node && stats.is_some() && rejected_first_endpoints.len() < 8 {
                    rejected_first_endpoints.push(point);
                }
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
        if previous.len() == 1 && queue.is_empty() {
            // Resolve only for this no-successor diagnostic, once per call.
            // Any observation failure leaves search behavior untouched and
            // source_status explicitly unavailable.
            let source_regions = SourceRegions::from_board(board).ok();
            for end in rejected_first_endpoints {
                let evidence =
                    first_step_rejection(clearance, net, start, end, layer, width_nm, source_regions.as_ref())
                        .unwrap_or_else(|_| LocalAccessRejection {
                            start,
                            end,
                            layer,
                            requested_width_nm: width_nm,
                            reason: "blocker_observation_error",
                            pieces: Vec::new(),
                            omitted_pieces: 0,
                            blockers: Vec::new(),
                            omitted_blockers: 0,
                            blocker_sources: Vec::new(),
                        });
                stats.first_step_rejections.push(evidence);
            }
        }
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

#[cfg(test)]
mod first_step_tests {
    use super::*;
    use crate::test_support_routers as routers;

    #[test]
    fn nonsealed_search_does_not_materialize_first_step_blockers() {
        let mut board = routers::pad_board(&[("S", &[(10.0, 10.0), (20.0, 10.0)])], 2, Vec::new());
        // The east first step and direct shortcut are blocked, while the
        // north/south first steps admit a short detour around this area.
        board.copper_keepouts.push(routers::copper_keepout(
            "partial-first-step-wall",
            &[CopperLayer::Front],
            routers::rect(10.09, 9.9, 10.12, 10.1),
        ));
        let index = RoutingClearanceIndex::new(&board).expect("clearance");
        let start = routers::pt(10.0, 10.0);
        let end = routers::pt(12.0, 10.0);
        let width = routers::mm(0.15);
        assert!(
            !legal_segment(&index, "S", start, routers::pt(10.025, 10.0), CopperLayer::Front, width)
                .expect("blocked east first step")
        );
        let bounds =
            LocalAccessBounds { step_nm: routers::mm(0.025), detour_nm: routers::mm(0.5), maximum_states: 20_000 };
        let mut stats = LocalAccessStats::default();
        // Reused stats must not retain sealed-search evidence from an earlier
        // call, even if the next search is nonsealed or takes a direct shortcut.
        stats.first_step_rejections.push(LocalAccessRejection {
            start,
            end,
            layer: CopperLayer::Front,
            requested_width_nm: width,
            reason: "stale_test_record",
            pieces: Vec::new(),
            omitted_pieces: 0,
            blockers: Vec::new(),
            omitted_blockers: 0,
            blocker_sources: Vec::new(),
        });
        let observed = local_access_path(
            &board,
            &index,
            "S",
            start,
            end,
            CopperLayer::Front,
            width,
            bounds,
            Some(&mut stats),
            None,
        )
        .expect("observed detour");
        let unobserved =
            local_access_path(&board, &index, "S", start, end, CopperLayer::Front, width, bounds, None, None)
                .expect("unobserved detour");
        assert!(observed.is_some(), "the partial wall must not seal the start");
        assert_eq!(observed, unobserved);
        assert!(stats.discovered_nodes > 1);
        assert_eq!(stats.outcome, "connected");
        assert!(stats.first_step_rejections.is_empty());
        assert!(stats.first_step_diagnostics(0).is_empty());
    }

    #[test]
    fn first_step_evidence_observes_actual_region_or_copper_without_changing_search() {
        for region in [false, true] {
            let mut board = routers::pad_board(&[("S", &[(10.0, 10.0), (20.0, 10.0)]), ("OTHER", &[])], 2, Vec::new());
            if region {
                board.copper_keepouts.push(routers::copper_keepout(
                    "test-area",
                    &[CopperLayer::Front],
                    routers::rect(9.8, 9.8, 10.2, 10.2),
                ));
            } else {
                // A blocked snapshot is intentional: diagnostics must name
                // foreign physical copper even if it already seals the pad.
                board.tracks.push(routers::track("OTHER", (9.5, 10.0), (10.5, 10.0), 0.4, CopperLayer::Front));
            }
            let index = RoutingClearanceIndex::new(&board).expect("clearance");
            let bounds =
                LocalAccessBounds { step_nm: routers::mm(0.025), detour_nm: routers::mm(0.5), maximum_states: 100 };
            let mut stats = LocalAccessStats::default();
            let observed = local_access_path(
                &board,
                &index,
                "S",
                routers::pt(10.0, 10.0),
                routers::pt(12.0, 10.0),
                CopperLayer::Front,
                routers::mm(0.15),
                bounds,
                Some(&mut stats),
                None,
            )
            .expect("observed path");
            let unobserved = local_access_path(
                &board,
                &index,
                "S",
                routers::pt(10.0, 10.0),
                routers::pt(12.0, 10.0),
                CopperLayer::Front,
                routers::mm(0.15),
                bounds,
                None,
                None,
            )
            .expect("unobserved path");
            assert_eq!(observed, unobserved);
            assert!(observed.is_none());
            assert_eq!((stats.expanded_states, stats.discovered_nodes), (1, 1));
            assert_eq!(stats.outcome, "frontier_exhausted");
            assert_eq!(stats.first_step_rejections.len(), 8);
            let records = stats.first_step_diagnostics(3);
            assert_eq!(records.len(), 3);
            assert!(records.iter().all(|record| record.len() <= 900));
            assert!(records[0].contains("omitted_other_attempts=3"));
            assert!(records[0].contains("omitted_edge_details=6"));
            assert!(!records[1].contains("generated_region_role"));
            assert!(records[1].contains(if region { "region:" } else { "copper:" }));
            for edge in &stats.first_step_rejections {
                assert_eq!(edge.start, routers::pt(10.0, 10.0));
                assert_eq!(edge.layer, CopperLayer::Front);
                assert_eq!(edge.requested_width_nm, routers::mm(0.15));
                assert_eq!(edge.reason, "exact_clearance");
                assert_eq!(edge.omitted_pieces, 0);
                assert!(edge.blockers.len() <= 4 && !edge.blockers.is_empty());
                assert!(edge.blockers.iter().any(|blocker| if region {
                    blocker.identity.starts_with("region:")
                        && blocker.net.is_none()
                        && blocker.reason == "keepout or immutable macro region"
                } else {
                    blocker.net.as_deref() == Some("OTHER") && blocker.reason == "copper clearance"
                }));
                if region {
                    let source = edge
                        .blocker_sources
                        .iter()
                        .find(|source| source.identity.starts_with("region:"))
                        .expect("region source lookup");
                    assert_eq!(source.status, "unique_exact_geometry_match");
                    assert_eq!(source.match_count, 1, "board keepout is not counted twice through resolved keepouts");
                    assert_eq!(
                        source.matches,
                        vec![LocalAccessSourceKeepout {
                            source: "board.copper_keepouts",
                            index: 0,
                            id: "test-area".to_owned(),
                        }]
                    );
                }
                // Recorded endpoints are the actually tested first 25 um
                // neighbors, not a separate radial probe or invented target.
                let delta = (edge.end.x_nm - edge.start.x_nm).abs().max((edge.end.y_nm - edge.start.y_nm).abs());
                assert_eq!(delta, routers::mm(0.025));
            }
        }
    }

    #[test]
    fn equal_keepout_occurrences_are_ambiguous_and_ids_do_not_grant_roles() {
        let mut board = routers::pad_board(&[("S", &[(10.0, 10.0), (20.0, 10.0)])], 2, Vec::new());
        let area =
            routers::copper_keepout("escape-reserve-via-7", &[CopperLayer::Front], routers::rect(9.8, 9.8, 10.2, 10.2));
        board.copper_keepouts = vec![area.clone(), area];
        let index = RoutingClearanceIndex::new(&board).expect("clearance");
        let mut stats = LocalAccessStats::default();
        let result = local_access_path(
            &board,
            &index,
            "S",
            routers::pt(10.0, 10.0),
            routers::pt(12.0, 10.0),
            CopperLayer::Front,
            routers::mm(0.15),
            LocalAccessBounds { step_nm: routers::mm(0.025), detour_nm: routers::mm(0.5), maximum_states: 100 },
            Some(&mut stats),
            None,
        )
        .expect("observed search");
        assert!(result.is_none());
        for edge in &stats.first_step_rejections {
            let source = edge
                .blocker_sources
                .iter()
                .find(|source| source.identity.starts_with("region:"))
                .expect("region source lookup");
            assert_eq!(source.status, "ambiguous_exact_geometry_matches");
            assert_eq!(source.match_count, 2);
            assert_eq!(source.matches.iter().map(|row| row.index).collect::<Vec<_>>(), vec![0, 1]);
            assert!(
                source
                    .matches
                    .iter()
                    .all(|row| row.source == "board.copper_keepouts" && row.id == "escape-reserve-via-7")
            );
        }
        let records = stats.first_step_diagnostics(0);
        assert!(records[0].contains("generated_region_role=unknown"));
        assert!(records.iter().all(|record| record.len() <= 900));
        assert!(records.iter().any(|record| record.contains("ambiguous_exact_geometry_matches")));
    }
}

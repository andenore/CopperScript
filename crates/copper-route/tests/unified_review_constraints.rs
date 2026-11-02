//! Router review regressions: real copper, whole-net budgets and
//! cancellation (ported from the former
//! `tests/test_unified_review_constraints.py`). Rust has one search, so the
//! `search_backend` parametrization runs once; Python's patched
//! `perf_counter` is a short real budget that the progress observer sleeps
//! past.

mod support_routers;

use copper_json::Value;
use copper_pcb::board::{IslandPolicy, PhysicalBoard, PhysicalNet, ZoneConnection};
use copper_physical::{BoardSide, CopperLayer};
use copper_route::detailed::DetailedRoutingStatus;
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use copper_route::unified_router::router::Router;
use indexmap::IndexSet;
use support_routers::routes::{Routed, guides, route};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;

fn short(iterations: i64) -> UnifiedRouterOptions {
    UnifiedRouterOptions { maximum_iterations: iterations, commit_rounds: 1, ..UnifiedRouterOptions::default() }
}

fn by_code<'a>(routed: &'a Routed, key: &str) -> &'a Value {
    summary_value(&routed.summary, "drc", key)
}

fn has_code(routed: &Routed, key: &str, code: &str) -> bool {
    by_code(routed, key).get(code).is_some()
}

#[test]
fn opposite_face_terminals_respect_zero_and_positive_via_budgets() {
    for (limit, connected) in [(0, false), (1, true)] {
        let mut board = pad_board(&[("N", &[(2.0, 3.0), (8.0, 3.0)])], 2, vec![]);
        board.placements[1].side = BoardSide::Back;
        let mut rule = rule("N");
        rule.max_vias = Some(limit);
        board.net_routing_rules = vec![rule];
        let routed = route(&board, &short(5));
        assert_eq!(routed.result.nets[0].connected, connected, "max_vias={limit}");
        assert!(routed.result.board.vias.len() as i64 <= limit);
        assert!(!has_code(&routed, "errors_by_code", "DRC-MAX-VIAS"));
    }
}

fn plane_lead_board() -> PhysicalBoard {
    let mut board = pad_board(&[("G", &[(2.1, 3.0), (8.0, 3.0)])], 2, vec![]);
    for footprint in board.footprints.values_mut() {
        footprint.pads[0].size = copper_physical::footprint::Size { width_nm: mm(0.1), height_nm: mm(0.1) };
    }
    let mut plane = zone("g", "G", &[F], rect(5.25, 1.0, 24.0, 24.0));
    plane.pad_connection = ZoneConnection::Solid;
    plane.island_policy = IslandPolicy::RemoveAll;
    plane.minimum_island_area_nm2 = None;
    board.zones = vec![plane];
    board
}

#[test]
fn plane_target_requires_its_lead_in_to_be_committed() {
    let routed = route(&plane_lead_board(), &short(3));
    assert!(routed.result.nets[0].connected);
    assert_eq!(summary_int(&routed.summary, "plane", "estimated_opens"), 0);
    assert!(!routed.result.board.tracks.is_empty() && routed.result.board.vias.is_empty());
    let lead = pt(5.1, 6.0);
    assert!(routed.result.board.tracks.iter().any(|t| t.start == lead || t.end == lead));
}

fn multiset(tracks: &[copper_pcb::board::TrackSegment]) -> indexmap::IndexMap<String, usize> {
    let mut counts = indexmap::IndexMap::new();
    for track in tracks {
        *counts.entry(format!("{track:?}")).or_insert(0) += 1;
    }
    counts
}

#[test]
fn final_length_budget_includes_fixed_tracks() {
    for (limit, over) in [(8.0, true), (12.0, false)] {
        let fixed = track("N", (5.0, 6.0), (11.0, 6.0), 0.2, F);
        let mut board = pad_board(&[("N", &[(2.0, 3.0), (8.0, 3.0), (14.0, 3.0)])], 2, vec![fixed]);
        let mut rule = rule("N");
        rule.max_length_nm = Some(mm(limit));
        board.net_routing_rules = vec![rule];
        let routed = route(&board, &short(5));
        let net = &routed.result.nets[0];
        // Routable copper is kept either way; a length miss is a review warning.
        assert!(net.connected && net.diagnostics.is_empty(), "{net:?}");
        let (before, after) = (multiset(&board.tracks), multiset(&routed.result.board.tracks));
        assert!(before.iter().all(|(k, n)| after.get(k).is_some_and(|m| m >= n)));
        assert!(after.values().sum::<usize>() > before.values().sum::<usize>());
        assert!(!has_code(&routed, "errors_by_code", "DRC-MAX-LENGTH"));
        let budget = by_code(&routed, "budget_warnings_by_code");
        if over {
            assert!(net.warnings.join(" ").contains("12000000 nm exceeds 8000000 nm"), "{:?}", net.warnings);
            assert_eq!(int_of(budget.get("DRC-MAX-LENGTH").expect("length warning")), 1);
            assert_eq!(budget.as_object().expect("object").len(), 1);
        } else {
            assert!(net.warnings.is_empty());
            assert!(budget.as_object().expect("object").is_empty());
        }
    }
}

#[test]
fn already_connected_source_over_budget_is_preserved_with_a_warning() {
    let fixed = track("N", (5.0, 6.0), (11.0, 6.0), 0.2, F);
    let mut board = pad_board(&[("N", &[(2.0, 3.0), (8.0, 3.0)])], 2, vec![fixed]);
    let mut rule = rule("N");
    rule.max_length_nm = Some(mm(5.0));
    board.net_routing_rules = vec![rule];
    let routed = route(&board, &short(1));
    let net = &routed.result.nets[0];
    assert!(net.connected && routed.result.status == DetailedRoutingStatus::Success);
    assert_eq!(routed.result.board.tracks, board.tracks);
    assert_eq!(routed.result.board.vias, board.vias);
    assert!(net.warnings.join(" ").contains("6000000 nm exceeds 5000000 nm"), "{:?}", net.warnings);
}

/// Sleeps past the time budget at `expire_at` (Python advanced a patched clock).
struct Expire {
    events: Events,
    phase: &'static str,
    event: &'static str,
    sleep: std::time::Duration,
}

impl copper_route::Progress for Expire {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        self.events.observe(phase, event, details);
        if phase == self.phase && event == self.event && self.events.count(phase, event) == 1 {
            std::thread::sleep(self.sleep);
        }
    }
}

/// Adapted: only the `unified_setup` case. The router forwards negotiation
/// events after `negotiate` returns, so an observer cannot expire the budget
/// at `unified_iteration` start; `expired_negotiation_keeps_its_previous_incumbent`
/// covers a deadline that has passed when negotiation starts.
#[test]
fn expired_deadline_stops_before_starting_nets() {
    {
        let (phase, event) = ("unified_setup", "finished");
        let fixed = track("FIXED", (5.0, 11.0), (11.0, 11.0), 0.2, F);
        let board =
            pad_board(&[("N", &[(2.0, 3.0), (8.0, 3.0)]), ("FIXED", &[(2.0, 8.0), (8.0, 8.0)])], 2, vec![fixed]);
        let options = UnifiedRouterOptions { time_budget_s: Some(0.5), ..UnifiedRouterOptions::default() };
        let mut observer =
            Expire { events: Events::default(), phase, event, sleep: std::time::Duration::from_millis(600) };
        let result = route_unified(&board, &guides(&board), &options, None, None, None, None, Some(&mut observer))
            .expect("route");
        assert_eq!(result.board.tracks, board.tracks, "{phase}");
        assert_eq!(result.board.vias, board.vias);
        let net = |name: &str| result.nets.iter().find(|n| n.net == name).expect("net");
        assert!(net("FIXED").connected && !net("N").connected, "{phase}");
        assert!(net("N").diagnostics.join(" ").contains("time budget exhausted"), "{:?}", net("N").diagnostics);
        assert_eq!(observer.events.count("unified_iteration", "started"), 0);
    }
}

#[test]
fn expired_negotiation_keeps_its_previous_incumbent() {
    let board = pad_board(&[("N", &[(2.0, 3.0), (8.0, 3.0)])], 2, vec![]);
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let index = RoutingClearanceIndex::new(&board).expect("index");
    let options = UnifiedRouterOptions::default();
    let mut router = Router::new(&board, &options, &nets, IndexSet::new(), None, index).expect("router");
    let (previous, conflicts, _) = router.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    assert!(previous["N"].connected && conflicts.is_empty());
    let occupancy = router.occupancy.occ.clone();
    let expired = Some(std::time::Instant::now());
    let (current, conflicts, iterations) = router.negotiate(expired, None, &mut Vec::new()).expect("negotiate");
    assert_eq!(current, previous);
    assert!(conflicts.is_empty());
    assert_eq!(iterations, 0);
    assert_eq!(router.occupancy.occ, occupancy);
}

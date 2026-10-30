//! Physical-land closure and fill-backed routing completion, ported from
//! `tests/test_route_closure.py`.

mod support_planes;

use std::sync::OnceLock;

use copper_drc::digest::physical_board_digest;
use copper_drc::{Finding, Policy, Severity};
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::{CopperLayer, Point};
use copper_route::critical::{CriticalNetResult, CriticalRoutingStatus};
use copper_route::detailed::{DetailedNetResult, DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus};
use copper_route::drc_report::{DrcReport, run_physical_drc};
use copper_route::flow::{PhysicalFlowStatus, RoutingPipelineOptions, RoutingPipelineResult, run_routing_pipeline};
use copper_route::plane_verify::{PlaneVerification, kicad_export_digest};
use copper_route::route_closure::{close_detailed_lands, routing_complete_with_fill};
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use copper_route::{GlobalRouterOptions, GlobalRoutingResult};
use support_planes::*;

fn net_result(net: &str, connected: bool, tracks: i64, length_nm: i64, diagnostics: &[&str]) -> DetailedNetResult {
    DetailedNetResult {
        net: net.to_owned(),
        connected,
        track_count: tracks,
        via_count: 0,
        length_nm,
        guide_deviation_count: 0,
        diagnostics: diagnostics.iter().map(|item| (*item).to_owned()).collect(),
        orthogonal_mode_used: false,
        resumed_branch_count: 0,
        search_budget_exhausted: false,
        retry_heuristic_weight_percent: None,
        warnings: Vec::new(),
    }
}

/// `_result`: J1.1 and J2.1 of net A, 4 mm apart, joined by one track when
/// `copper`; the router claims success and reports A as `reported`.
fn closure_result(reported: bool, copper: bool) -> DetailedRoutingResult {
    let mut board = board("Closure", 10.0, 10.0);
    add_footprint(
        &mut board,
        footprint("test/terminal", vec![pad("1", Point::new(0, 0), size(1.0, 1.0))], size(1.0, 1.0)),
    );
    board.placements = vec![place("J1", "test/terminal", mm(3.0, 5.0)), place("J2", "test/terminal", mm(7.0, 5.0))];
    board.nets = vec![net("A", &[("J1", "1"), ("J2", "1")])];
    if copper {
        board.tracks = vec![track("A", mm(3.0, 5.0), mm(7.0, 5.0), nm(0.2), CopperLayer::Front)];
    }
    board.metadata.insert("detailed_routing".to_owned(), "complete".to_owned());
    let (count, length) = (board.tracks.len() as i64, if copper { nm(4.0) } else { 0 });
    let diagnostics: &[&str] = if reported { &[] } else { &["search budget exhausted"] };
    DetailedRoutingResult {
        status: DetailedRoutingStatus::Success,
        nets: vec![net_result("A", reported, count, length, diagnostics)],
        metrics: DetailedRoutingMetrics {
            routed_net_count: i64::from(reported),
            unrouted_net_count: i64::from(!reported),
            conflict_resource_count: 0,
            total_conflict_overflow: 0,
            track_count: count,
            via_count: 0,
            total_length_nm: length,
            passes: 1,
        },
        locked_track_count: 0,
        locked_via_count: 0,
        global_routing_fingerprint: "global".to_owned(),
        routing_fingerprint: "routing".to_owned(),
        congestion: None,
        board,
    }
}

/// A real pipeline result over the closure board, as the frame whose
/// fields each test sets (Python used a `SimpleNamespace`).
fn pipeline_frame() -> RoutingPipelineResult {
    static FRAME: OnceLock<RoutingPipelineResult> = OnceLock::new();
    FRAME
        .get_or_init(|| {
            let mut board = closure_result(true, false).board;
            board.metadata.clear();
            let placement = copper_place::options::PlannerOptions {
                candidate_count: 1,
                analytical_iterations: 0,
                refinement_passes: 0,
                fixed_references: ["J1", "J2"].iter().map(|r| r.to_string()).collect(),
                ..Default::default()
            }
            .validated()
            .unwrap();
            let options = RoutingPipelineOptions {
                placement_options: Some(placement),
                feedback_options: Some(PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }),
                ..Default::default()
            };
            run_routing_pipeline(board, options, None).unwrap()
        })
        .clone()
}

fn evidence_for(board: &PhysicalBoard) -> PlaneVerification {
    PlaneVerification {
        passed: true,
        board_digest: physical_board_digest(board),
        export_digest: kicad_export_digest(&copper_pcb::kicad_pcb::generate(board).unwrap()),
        filled_board_digest: "filled".to_owned(),
        report_digest: "report".to_owned(),
        kicad_version: "10.0.6".to_owned(),
        unconnected_count: 0,
        island_count: 0,
        other_violation_count: 0,
        findings: Vec::new(),
        dangling_track_uuids: Vec::new(),
        native_findings: Vec::new(),
        artifact_directory: None,
        filled_regions: Vec::new(),
        decoupling_assessments: Vec::new(),
    }
}

/// `_deferred_plane_fixture`: routed net A plus a GND zone net whose two
/// lands await verified fill and pad stitching, a failing pipeline whose
/// placement certified the full route, its DRC and matching fill evidence.
fn deferred_plane_fixture() -> (RoutingPipelineResult, PhysicalBoard, DrcReport, PlaneVerification) {
    let mut result = closure_result(true, true);
    let mut board = result.board.clone();
    board.placements.extend([place("G1", "test/terminal", mm(3.0, 3.0)), place("G2", "test/terminal", mm(7.0, 3.0))]);
    board.nets.push(net("GND", &[("G1", "1"), ("G2", "1")]));
    board.zones = vec![zone("ground", "GND", &[CopperLayer::Back], rect(1.0, 1.0, 9.0, 9.0))];
    board.metadata = [("detailed_routing".to_owned(), "partial".to_owned())].into_iter().collect();
    result.board = board.clone();
    result.nets.push(net_result("GND", false, 0, 0, &["zone net awaits verified fill and pad stitching"]));
    result.metrics.unrouted_net_count = 1;
    result.status = DetailedRoutingStatus::Partial;
    let mut pipeline = pipeline_frame();
    pipeline.status = PhysicalFlowStatus::Fail;
    pipeline.placement_and_global.full_route_certified = true;
    pipeline.critical.status = CriticalRoutingStatus::Success;
    pipeline.critical.nets.clear();
    pipeline.detailed = result;
    let drc = run_physical_drc(&board, Policy::default()).unwrap();
    let evidence = evidence_for(&board);
    (pipeline, board, drc, evidence)
}

fn complete(
    pipeline: &RoutingPipelineResult,
    board: &PhysicalBoard,
    drc: &DrcReport,
    evidence: Option<&PlaneVerification>,
) -> bool {
    routing_complete_with_fill(pipeline, board, drc, evidence).unwrap()
}

#[test]
fn external_fill_can_complete_routing_without_changing_physical_signoff() {
    let (pipeline, board, drc, evidence) = deferred_plane_fixture();
    let original = drc.token.clone();
    assert!(complete(&pipeline, &board, &drc, Some(&evidence)));
    assert_eq!(drc.decision(), copper_drc::Decision::Fail);
    assert_eq!(drc.token, original);
    assert!(!complete(&pipeline, &board, &drc, None));
}

#[test]
fn verified_fill_closes_a_zone_net_whatever_its_router_reported() {
    let (mut pipeline, board, drc, evidence) = deferred_plane_fixture();
    let diagnostics: [&[&str]; 2] = [&["unified: 2 of 150 pad groups cannot reach the plane"], &[]];
    for items in diagnostics {
        let last = pipeline.detailed.nets.last_mut().unwrap();
        last.diagnostics = items.iter().map(|item| (*item).to_owned()).collect();
        assert!(complete(&pipeline, &board, &drc, Some(&evidence)));
        let open = PlaneVerification { unconnected_count: 1, ..evidence.clone() };
        assert!(!complete(&pipeline, &board, &drc, Some(&open)));
        assert!(!complete(&pipeline, &board, &drc, None));
    }
}

/// The board's global guide with every route reported unconnected.
fn failed_guide(board: &PhysicalBoard, options: &GlobalRouterOptions) -> GlobalRoutingResult {
    let mut source = board.clone();
    source.tracks.clear();
    source.metadata.clear();
    let mut guide = copper_route::route_global(&source, options, None, None, None).unwrap();
    for route in &mut guide.routes {
        route.connected = false;
    }
    guide
}

#[test]
fn native_closure_reconciles_only_downstream_resolved_global_failures() {
    let (mut pipeline, board, drc, evidence) = deferred_plane_fixture();
    let options = GlobalRouterOptions { tile_size_nm: nm(2.0), ..GlobalRouterOptions::default() };
    let guide = failed_guide(&board, &options);
    pipeline.placement_and_global.full_route_certified = false;
    pipeline.placement_and_global.global_route = guide.clone();
    pipeline.critical.nets.clear();
    assert!(complete(&pipeline, &board, &drc, Some(&evidence)));
    assert!(!complete(&pipeline, &board, &drc, None));
    let stale = PlaneVerification { export_digest: "stale".to_owned(), ..evidence.clone() };
    assert!(!complete(&pipeline, &board, &drc, Some(&stale)));
    let mut overflowing = guide.clone();
    overflowing.metrics.total_overflow = 1;
    pipeline.placement_and_global.global_route = overflowing;
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
    let mut unaccounted = guide.clone();
    let mut extra = guide.routes[0].clone();
    extra.net = "unaccounted".to_owned();
    unaccounted.routes.push(extra);
    pipeline.placement_and_global.global_route = unaccounted;
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
    pipeline.placement_and_global.global_route = guide;
    for item in &mut pipeline.detailed.nets {
        item.connected = false;
    }
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
}

#[test]
fn native_global_reconciliation_accepts_completed_critical_owner() {
    let (mut pipeline, board, _, _) = deferred_plane_fixture();
    let mut board = board;
    board.net_routing_rules = vec![NetRoutingRule { kind: RouteKind::Clock, ..rule("A") }];
    let drc = run_physical_drc(&board, Policy::default()).unwrap();
    let evidence = evidence_for(&board);
    pipeline.placement_and_global.full_route_certified = false;
    pipeline.placement_and_global.global_route = failed_guide(&board, &GlobalRouterOptions::default());
    pipeline.detailed.nets.retain(|item| item.net != "A");
    let owner =
        |connected: bool| CriticalNetResult::new(vec!["A".to_owned()], connected, 0, 0, Vec::new(), 0, Vec::new());
    pipeline.critical.nets = vec![owner(true)];
    assert!(complete(&pipeline, &board, &drc, Some(&evidence)));
    pipeline.critical.nets = vec![owner(false)];
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
}

#[test]
fn routing_closure_rejects_stale_native_findings_and_search_failures() {
    let (mut pipeline, board, drc, evidence) = deferred_plane_fixture();
    let bad = [
        PlaneVerification { board_digest: "stale".to_owned(), ..evidence.clone() },
        PlaneVerification { export_digest: "stale".to_owned(), ..evidence.clone() },
        PlaneVerification { passed: false, ..evidence.clone() },
        PlaneVerification { unconnected_count: 1, ..evidence.clone() },
        PlaneVerification { other_violation_count: 1, ..evidence.clone() },
        PlaneVerification { findings: vec!["short".to_owned()], ..evidence.clone() },
    ];
    for item in &bad {
        assert!(!complete(&pipeline, &board, &drc, Some(item)), "{item:?}");
    }
    pipeline.critical.status = CriticalRoutingStatus::Failed;
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
    pipeline.critical.status = CriticalRoutingStatus::Success;
    let first = &mut pipeline.detailed.nets[0];
    first.connected = false;
    first.diagnostics = vec!["search budget exhausted".to_owned()];
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
    let (pipeline, board, mut drc, evidence) = deferred_plane_fixture();
    drc.report.findings.push(Finding {
        code: "DRC-CLEARANCE".to_owned(),
        severity: Severity::Error,
        message: "bad".to_owned(),
        objects: Vec::new(),
        nets: Vec::new(),
        layers: Vec::new(),
        required_nm: None,
        measured_nm: None,
    });
    assert!(!complete(&pipeline, &board, &drc, Some(&evidence)));
}

fn codes(drc: &DrcReport) -> std::collections::BTreeSet<String> {
    drc.findings().iter().map(|finding| finding.code.clone()).collect()
}

#[test]
fn closure_does_not_promote_failed_search_flags() {
    let initial = closure_result(false, true);
    let (closed, drc, _) = close_detailed_lands(&initial, Policy::default()).unwrap();
    assert!(!closed.nets[0].connected);
    assert_eq!(closed.nets[0].diagnostics, initial.nets[0].diagnostics);
    assert_eq!(closed.status, DetailedRoutingStatus::Partial);
    assert_eq!(closed.board.metadata["detailed_routing"], "partial");
    assert!(codes(&drc).contains("DRC-ROUTE-INCOMPLETE"), "{:?}", codes(&drc));
}

#[test]
fn closure_rejects_stale_success_without_copper() {
    let (closed, drc, _) = close_detailed_lands(&closure_result(true, false), Policy::default()).unwrap();
    assert!(!closed.nets[0].connected);
    assert_eq!(closed.metrics.unrouted_net_count, 1);
    assert_eq!(closed.status, DetailedRoutingStatus::Partial);
    let found = codes(&drc);
    assert!(found.contains("DRC-OPEN-NET") && found.contains("DRC-ROUTE-INCOMPLETE"), "{found:?}");
}

#[test]
fn default_zone_deferral_does_not_trigger_placement_repair() {
    let mut board = closure_result(false, false).board;
    board.zones = vec![zone("plane", "A", &[CopperLayer::Back], rect(1.0, 1.0, 9.0, 9.0))];
    board.metadata.clear();
    let placement = copper_place::options::PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        ..Default::default()
    }
    .validated()
    .unwrap();
    let options = RoutingPipelineOptions {
        placement_options: Some(placement),
        feedback_options: Some(PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }),
        detailed_feedback_trials: 2,
        ..Default::default()
    };
    let pipeline = run_routing_pipeline(board, options, None).unwrap();
    assert!(pipeline.placement_feedback.as_ref().is_none_or(|feedback| feedback.trials.is_empty()));
}

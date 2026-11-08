//! The bounded routing acceptance cases (ported from the former
//! `tests/test_routing_acceptance.py`). `route_case` lives in the `copper`
//! binary (`benchmark_routing.rs`); `route_case` below runs the same shared
//! pipeline with the same options.

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::CopperLayer;
use copper_route::flow::{PhysicalFlowStatus, RoutingPipelineOptions, run_routing_pipeline};
use copper_route::plane_verify::PlaneVerification;
use copper_route::routing_acceptance::{
    AcceptanceCase, AcceptanceError, RoutedCase, acceptance_cases, assess_case, run_acceptance,
};
use copper_route::routing_metrics::routing_quality;

#[path = "support_routers/boards.rs"]
mod boards;
use boards::*;

/// `benchmark_routing::route_case`.
fn route_case(board: &PhysicalBoard) -> copper_pcb::Result<RoutedCase> {
    let placement_options = copper_place::PlannerOptions {
        candidate_count: 1,
        analytical_iterations: 0,
        refinement_passes: 0,
        escape_spacing_passes: 0,
        fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
        ..copper_place::PlannerOptions::default()
    }
    .validated()?;
    let global_options =
        copper_route::routing::GlobalRouterOptions { tile_size_nm: 2_000_000, ..Default::default() }.validated()?;
    let feedback_options =
        copper_route::routeflow::PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }
            .validated()?;
    let fanout_options =
        copper_route::fanout::FanoutOptions { minimum_component_pads: 2, two_leg_escapes: false, ..Default::default() }
            .validated()?;
    let options = RoutingPipelineOptions {
        placement_options: Some(placement_options),
        global_options: Some(global_options),
        feedback_options: Some(feedback_options),
        fanout_options: Some(fanout_options),
        ..RoutingPipelineOptions::default()
    };
    let result = run_routing_pipeline(board.clone(), options, None)?;
    Ok(RoutedCase {
        board: result.board().clone(),
        status_pass: result.status == PhysicalFlowStatus::Pass,
        drc: result.drc,
    })
}

fn cases() -> Vec<AcceptanceCase> {
    acceptance_cases().expect("cases")
}

fn quality(board: &PhysicalBoard) -> Value {
    routing_quality(board, None).expect("quality")
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(n) => i64::try_from(n.clone()).expect("i64"),
        other => panic!("not an int: {other:?}"),
    }
}

fn decoupling<'a>(metrics: &'a Value, key: &str) -> &'a Value {
    let first = &metrics.get("decoupling").and_then(Value::as_array).expect("decoupling")[0];
    first.get(key).unwrap_or_else(|| panic!("{key}"))
}

#[test]
fn diverse_bounded_shared_pipeline() {
    for case in cases() {
        let result = route_case(&case.board).expect("route");
        let metrics = quality(&result.board);
        let assessed = assess_case(&case, &result, &metrics).expect("assess");
        assert_eq!(assessed.get("expectations_passed"), Some(&Value::Bool(true)), "{}: {assessed:?}", case.id);
        assert_eq!(metrics.get("native_fill_current"), Some(&Value::Bool(false)));
    }
}

#[test]
fn bypass_interior_junction_and_no_via_or_internal_shortcut() {
    let mut board = cases()[4].board.clone();
    // The capacitor joins the middle of the longer same-net segment.
    board.tracks = vec![track("POWER", (5.0, 7.0), (16.0, 7.0), 0.25, CopperLayer::Front)];
    let metrics = quality(&board);
    assert_eq!(decoupling(&metrics, "surface_connected"), &Value::Bool(true));
    assert_eq!(int(decoupling(&metrics, "surface_component_track_length_nm")), 11_000_000);
    assert_eq!(int(decoupling(&metrics, "pad_distance_nm")), 3_000_000);
    let mut bare = board.clone();
    bare.tracks.clear();
    assert_eq!(decoupling(&quality(&bare), "surface_connected"), &Value::Bool(false));
    let mut bottom = board.clone();
    bottom.tracks[0].layer = CopperLayer::Back;
    bottom.vias = vec![
        via("POWER", (5.0, 7.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back),
        via("POWER", (8.0, 7.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back),
    ];
    assert_eq!(decoupling(&quality(&bottom), "surface_connected"), &Value::Bool(false));
}

#[test]
fn added_copper_changes_actual_quality_not_pipeline_counters() {
    let board = cases()[0].board.clone();
    let original = quality(&board);
    let mut changed = board.clone();
    changed.tracks = vec![track("SIGNAL", (4.0, 7.0), (6.0, 7.0), 0.25, CopperLayer::Front)];
    changed.vias = vec![via("SIGNAL", (6.0, 7.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back)];
    let metrics = quality(&changed);
    assert_eq!(int(metrics.get("via_count").expect("vias")), int(original.get("via_count").expect("vias")) + 1);
    assert_eq!(int(metrics.get("track_length_nm").expect("length")), 2_000_000);
    assert_ne!(metrics.get("board_digest"), original.get("board_digest"));
}

#[test]
fn stale_native_reference_evidence_remains_incomplete() {
    let mut board = cases()[0].board.clone();
    board.net_routing_rules = vec![NetRoutingRule { kind: RouteKind::Clock, ..rule("SIGNAL") }];
    let stale = PlaneVerification {
        passed: true,
        board_digest: "different-geometry".to_owned(),
        export_digest: "different-export".to_owned(),
        filled_board_digest: "filled".to_owned(),
        report_digest: "report".to_owned(),
        kicad_version: "10.0.6".to_owned(),
        unconnected_count: 0,
        island_count: 0,
        other_violation_count: 0,
        findings: Vec::new(),
        dangling_track_uuids: Vec::new(),
        expected_antenna_open_ends: Vec::new(),
        expected_open_ends: Vec::new(),
        native_findings: Vec::new(),
        artifact_directory: None,
        filled_regions: Vec::new(),
        decoupling_assessments: Vec::new(),
    };
    let metrics = routing_quality(&board, Some(&stale)).expect("quality");
    assert_eq!(metrics.get("native_fill_current"), Some(&Value::Bool(false)));
    assert_eq!(metrics.get("native_report_digest"), Some(&Value::Null));
    let coverage = &metrics.get("reference_coverage").and_then(Value::as_array).expect("coverage")[0];
    assert_eq!(coverage.get("status").and_then(Value::as_str), Some("incomplete"));
}

#[test]
fn faster_geometry_cannot_silently_exceed_fixture_bounds() {
    let mut case = cases()[0].clone();
    let result = route_case(&case.board).expect("route");
    let metrics = quality(&result.board);
    case.maximum_length_nm = Some(1);
    let assessed = assess_case(&case, &result, &metrics).expect("assess");
    assert_eq!(assessed.get("expectations_passed"), Some(&Value::Bool(false)));
}

#[test]
fn fresh_output_and_missing_native_remain_incomplete() {
    let parent = std::env::temp_dir().join(format!("copper-route-acceptance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    let directory = parent.join("acceptance");
    let mut route = |board: &PhysicalBoard| route_case(board);
    let report = match run_acceptance(&directory, None, &mut route) {
        Ok(report) => report,
        Err(AcceptanceError::Message(m) | AcceptanceError::NotImplemented(m)) => panic!("{m}"),
    };
    assert_eq!(report.get("status").and_then(Value::as_str), Some("incomplete"));
    assert_eq!(report.get("software_expectations_passed"), Some(&Value::Bool(true)));
    assert_eq!(report.get("implementation_stable"), Some(&Value::Bool(true)));
    let rows = report.get("cases").and_then(Value::as_array).expect("cases");
    assert_eq!(rows.len(), 8);
    assert_eq!(rows.iter().filter(|r| r.get("expect_connected") == Some(&Value::Bool(true))).count(), 6);
    assert!(directory.join("acceptance.json").is_file());
    // The output directory must be fresh (Python's FileExistsError).
    assert!(matches!(run_acceptance(&directory, None, &mut route), Err(AcceptanceError::Message(_))));
    let _ = std::fs::remove_dir_all(parent);
}

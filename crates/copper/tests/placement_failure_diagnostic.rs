//! Failed placement geometry is inspectable but never a reusable checkpoint.

#[path = "../../copper-route/tests/support_routers/boards.rs"]
mod boards;

use boards::*;
use copper_json::{Object, Value};
use copper_place::planner::PlacementFailureDiagnostic;
use copper_place::scene::Poses;
use copper_route::iteration_checkpoints::{
    load_placement_checkpoint, remove_stale_placement_failure, write_error_record_with_diagnostic,
    write_placement_failure,
};
use std::path::Path;
use std::process::Command;

#[test]
fn failed_geometry_views_natively_and_cannot_be_reused() {
    let board = board(
        "failed-placement",
        30.0,
        30.0,
        vec![one_pad()],
        vec![
            placement("A", "test/one-pad", 10.0, 10.0),
            placement("B", "test/one-pad", 20.0, 20.0),
            placement("C", "test/one-pad", 25.0, 25.0),
        ],
        Vec::new(),
    );
    let held = placement("A", "test/one-pad", 5.0, 5.0);
    let proposal = placement("B", "test/one-pad", 5.2, 5.0);
    let packing = copper_json::parse(r#"{"selected_references":["B","A"],"outcome":"no_sampled_witness","passes":1,"final_pass":"coarse","domains":[{"reference":"B","sampled":64,"legal":0}],"empty_reference":"B","search_remaining":null}"#).unwrap();
    let diagnostic = PlacementFailureDiagnostic {
        seed: 1,
        failed_reference: Some("B".to_owned()),
        accepted: [("A".to_owned(), held)].into_iter().collect::<Poses>(),
        proposal: Some(proposal),
        reason: "cannot legalize component 'B' within the local repair budget".to_owned(),
        physical_rejection: None,
        bounded_packing: Some(packing.clone()),
    };
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("placement-failure-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    let artifact = write_placement_failure(&report, &board, &diagnostic).unwrap();
    assert_eq!(artifact.metadata["accepted_references"], Value::Array(vec![Value::from("A")]));
    assert_eq!(artifact.metadata["unaccepted_target_references"], Value::Array(vec![Value::from("B")]));
    assert_eq!(artifact.metadata["omitted_references"], Value::Array(vec![Value::from("C")]));
    let error = write_error_record_with_diagnostic(
        &report,
        &board.name,
        "ValueError",
        &diagnostic.reason,
        None,
        Object::new(),
        Some(&artifact),
    )
    .unwrap();
    let error_text = error.to_string_lossy();
    let mut run = copper_report::issues::load_run(&error_text).unwrap();
    let scene = run.scene(false).unwrap();
    assert_eq!(scene.footprints.len(), 2);
    assert!(scene.footprints.values().any(|p| p.reference == "A" && p.center == (5.0, 5.0)));
    assert!(!scene.footprints.contains_key("C"));
    let issues = copper_report::issues::run_issues(&mut run, 3.0).unwrap();
    assert_eq!(issues.len(), 1);
    assert!(!issues[0].points.is_empty(), "the rejected target locates the failure");
    assert_eq!(artifact.metadata["bounded_packing"], packing);
    let canonical = |value: &Value| copper_json::Dumps::new().sort_keys(true).dumps(value);
    assert_eq!(canonical(issues[0].details.get("records").unwrap()), canonical(&packing));
    let issue_output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["issues", error_text.as_ref(), "--issue", "E1", "--json"])
        .output()
        .unwrap();
    assert!(issue_output.status.success());
    let selected = copper_json::parse(&String::from_utf8(issue_output.stdout).unwrap()).unwrap();
    let detail = selected.get("issues").unwrap().as_array().unwrap()[0].get("details").unwrap();
    assert_eq!(
        canonical(detail.get("records").unwrap()),
        canonical(&packing),
        "native selected issue exposes every bounded field"
    );
    assert_eq!(detail.get("truncated"), Some(&Value::Bool(false)));
    let state = directory.join(&artifact.state_filename);
    let refusal = load_placement_checkpoint(&state, &state.to_string_lossy(), &Object::new()).unwrap();
    assert!(refusal.is_err(), "diagnostic PhysicalBoard is not PlacementRoutingResult");

    let svg = directory.join("failure.svg");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["view", error_text.as_ref(), "--issue", "E1", "-o", svg.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    assert!(String::from_utf8_lossy(&output.stdout).contains("INSPECTION ONLY"));
    assert!(std::fs::read_to_string(svg).unwrap().contains("<svg"));
}

#[test]
fn stale_cleanup_only_removes_a_generated_failure_artifact() {
    let source = board(
        "accepted-search",
        30.0,
        30.0,
        vec![one_pad()],
        vec![placement("A", "test/one-pad", 5.0, 5.0)],
        Vec::new(),
    );
    let diagnostic = PlacementFailureDiagnostic {
        seed: 1,
        failed_reference: Some("A".to_owned()),
        accepted: source.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect(),
        proposal: None,
        reason: "discarded physical seed".to_owned(),
        physical_rejection: None,
        bounded_packing: None,
    };
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("stale-seed-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    let artifact = write_placement_failure(&report, &source, &diagnostic).unwrap();
    let state = directory.join(&artifact.state_filename);
    assert!(remove_stale_placement_failure(&report).unwrap());
    assert!(!state.exists());
    assert!(!remove_stale_placement_failure(&report).unwrap());
    std::fs::write(&state, "unrelated user file").unwrap();
    assert!(!remove_stale_placement_failure(&report).unwrap());
    assert_eq!(std::fs::read_to_string(&state).unwrap(), "unrelated user file");
}

#[test]
fn complete_rejected_geometry_is_inspectable_but_never_a_checkpoint() {
    let board = board(
        "complete-rejected-placement",
        30.0,
        30.0,
        vec![one_pad()],
        vec![placement("A", "test/one-pad", 10.0, 10.0), placement("B", "test/one-pad", 10.2, 10.0)],
        Vec::new(),
    );
    let diagnostic = PlacementFailureDiagnostic {
        seed: 0,
        failed_reference: Some("B".to_owned()),
        accepted: board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect(),
        proposal: None,
        reason: "complete layout rejected by final placement constraints".to_owned(),
        physical_rejection: None,
        bounded_packing: None,
    };
    let directory =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("complete-placement-failure-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    let artifact = write_placement_failure(&report, &board, &diagnostic).unwrap();
    assert_eq!(artifact.metadata["inspection_only"], Value::Bool(true));
    assert_eq!(artifact.metadata["accepted_references"], Value::Array(vec![Value::from("A"), Value::from("B")]));
    assert_eq!(artifact.metadata["unaccepted_target_references"], Value::Array(Vec::new()));
    assert_eq!(artifact.metadata["omitted_references"], Value::Array(Vec::new()));
    assert!(artifact.metadata["note"].as_str().unwrap().contains("2 held components (not certified)"));
    let error = write_error_record_with_diagnostic(
        &report,
        &board.name,
        "ValueError",
        &diagnostic.reason,
        None,
        Object::new(),
        Some(&artifact),
    )
    .unwrap();
    let mut run = copper_report::issues::load_run(&error.to_string_lossy()).unwrap();
    assert_eq!(run.scene(false).unwrap().footprints.len(), 2);
    let issues = copper_report::issues::run_issues(&mut run, 3.0).unwrap();
    assert_eq!(issues.len(), 1);
    assert!(!issues[0].points.is_empty(), "the held violating component locates the issue");
    let state = directory.join(&artifact.state_filename);
    assert!(load_placement_checkpoint(&state, &state.to_string_lossy(), &Object::new()).unwrap().is_err());
    let svg = directory.join("complete-failure.svg");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["view", error.to_str().unwrap(), "--issue", "E1", "-o", svg.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    assert!(String::from_utf8_lossy(&output.stdout).contains("INSPECTION ONLY"));
    assert!(std::fs::read_to_string(svg).unwrap().contains("<svg"));
}

#[test]
fn rejected_macro_copper_views_natively_without_acquiring_a_certificate() {
    use copper_physical::CopperLayer::{Back, Front};
    let source = board(
        "macro-rejection",
        30.0,
        30.0,
        vec![one_pad()],
        vec![placement("A", "test/one-pad", 5.0, 5.0)],
        Vec::new(),
    );
    let mut trial = source.clone();
    trial.nets.extend([net("N1", &[]), net("N2", &[])]);
    trial.tracks.push(track("N1", (10.0, 10.0), (20.0, 10.0), 0.2, Front));
    trial.vias.push(via("N2", (15.0, 10.0), 0.6, 0.3, Front, Back));
    let findings = copper_drc::run_physical_drc(&trial, copper_drc::Policy::default()).unwrap().findings;
    assert!(findings.iter().any(|f| f.code == "DRC-HOLE-CLEARANCE"));
    let rejection = copper_drc::macros::MacroRejection { board: trial, findings };
    assert_eq!(rejection.failed_reference(), None, "the rejected copper has no named component owner");
    let diagnostic = PlacementFailureDiagnostic {
        seed: 1,
        failed_reference: rejection.failed_reference().map(str::to_owned),
        accepted: source.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect(),
        proposal: None,
        reason: rejection.message(),
        physical_rejection: Some(Box::new(rejection)),
        bounded_packing: None,
    };
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("macro-rejection-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("route-report.json");
    let artifact = write_placement_failure(&report, &source, &diagnostic).unwrap();
    assert_eq!(artifact.metadata["rejected_macro_copper"], Value::Bool(true));
    assert_eq!(artifact.metadata["failed_reference"], Value::Null);
    assert!(artifact.metadata["note"].as_str().unwrap().contains("every placement seed failed"));
    assert!(!artifact.metadata["physical_findings"].as_array().unwrap().is_empty());
    let error = write_error_record_with_diagnostic(
        &report,
        &source.name,
        "ValueError",
        &diagnostic.reason,
        None,
        Object::new(),
        Some(&artifact),
    )
    .unwrap();
    let document = copper_json::parse(&std::fs::read_to_string(&error).unwrap()).unwrap();
    assert_eq!(document.get("components"), Some(&Value::Array(Vec::new())));
    let mut run = copper_report::issues::load_run(&error.to_string_lossy()).unwrap();
    let scene = run.scene(false).unwrap();
    assert_eq!(scene.tracks.len(), 1);
    assert_eq!(scene.vias.len(), 1);
    let issues = copper_report::issues::run_issues(&mut run, 3.0).unwrap();
    assert_eq!(issues[0].code, Value::from("MACRO-TRIAL-REJECTED"));
    assert!(
        issues[0].points.iter().all(|p| p.0 >= 10.0 && p.1 == 10.0),
        "locate the actual rejected copper, not unrelated component A at (5,5)"
    );
    assert!(!issues[0].points.is_empty());
    let state = directory.join(&artifact.state_filename);
    assert!(load_placement_checkpoint(&state, &state.to_string_lossy(), &Object::new()).unwrap().is_err());
    let svg = directory.join("macro-failure.svg");
    let output = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["view", error.to_str().unwrap(), "--issue", "E1", "-o", svg.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    assert!(String::from_utf8_lossy(&output.stdout).contains("INSPECTION ONLY"));
    assert!(std::fs::read_to_string(svg).unwrap().contains("<svg"));
}

#[test]
fn only_a_successful_command_removes_its_generated_failure_sibling() {
    let source =
        board("stale-run", 30.0, 30.0, vec![one_pad()], vec![placement("A", "test/one-pad", 5.0, 5.0)], Vec::new());
    let diagnostic = PlacementFailureDiagnostic {
        seed: 0,
        failed_reference: Some("A".into()),
        accepted: source.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect(),
        proposal: None,
        reason: "prior placement failure".into(),
        physical_rejection: None,
        bounded_packing: None,
    };
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("failure-lifecycle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let report = directory.join("run.json");
    let artifact = write_placement_failure(&report, &source, &diagnostic).unwrap();
    let state = directory.join(&artifact.state_filename);
    let other_report = directory.join("other.json");
    let other_artifact = write_placement_failure(&other_report, &source, &diagnostic).unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let incomplete = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(["route-board", "--report", report.to_str().unwrap()])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(!incomplete.status.success());
    assert!(state.exists(), "incomplete arguments cannot remove prior evidence");
    let malformed = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args([
            "route-board",
            "examples/valid_board/board.copper",
            "--report",
            report.to_str().unwrap(),
            "--stop-after",
            "not-a-stage",
        ])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(!malformed.status.success());
    assert!(state.exists(), "malformed arguments cannot remove prior evidence");
    let output_path = directory.join("board.kicad_pcb");
    let completed = Command::new(env!("CARGO_BIN_EXE_copper"))
        .args([
            "route-board",
            "examples/valid_board/board.copper",
            "--locked",
            "--offline",
            "--allow-proxy-footprints",
            "--verify-plane-fill",
            "none",
            "--report",
            report.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
        ])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}{}",
        String::from_utf8_lossy(&completed.stdout),
        String::from_utf8_lossy(&completed.stderr)
    );
    assert!(!state.exists(), "successful rerun removes this report's generated diagnostic");
    assert!(directory.join(other_artifact.state_filename).exists(), "other report artifacts are untouched");
}

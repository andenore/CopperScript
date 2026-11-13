//! Direct unified mode stays direct through the complete `route-board`
//! path (ported from the former `tests/test_unified_direct_cli.py`). Python
//! also patched the legacy routers (`stitch_zone_pads`, `route_critical_nets`,
//! `_plan_decouplers`, `route_fanout`) to fail if called; those spies are
//! dropped, and the report's work limits and the saved state are checked.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::Value;

const BOARD: &str = r#"board DirectCli {
    silkscreen { branding { enabled = false; } }
    use library "tiny";
    component R1: RESISTOR { footprint="Fixture:Two"; }
    component R2: RESISTOR { footprint="Fixture:Two"; }
    net POWER { R1.1; R2.1; }
    net DATA { R1.2; R2.2; }
    mechanical { outline rectangle { width=20mm; height=12mm; } }
    constraint fixed_placement(R1) { x=3mm; y=6mm; rotation=0; side=front; }
    constraint fixed_placement(R2) { x=17mm; y=6mm; rotation=0; side=front; }
    constraint copper_zone(POWER) { layers="F.Cu"; x=1mm; y=1mm; width=18mm; height=10mm;
        pad_connection=solid; island_policy=remove_all; }
}
"#;

const FOOTPRINT: &str = r#"(footprint "Two" (layer "F.Cu")
    (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
    (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
    (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask")))
"#;

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("unified-direct-cli").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("footprints").join("Fixture.pretty")).expect("scratch");
    std::fs::write(directory.join("board.copper"), BOARD).expect("board");
    std::fs::write(directory.join("footprints").join("Fixture.pretty").join("Two.kicad_mod"), FOOTPRINT)
        .expect("footprint");
    directory
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn at<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(value, |v, key| v.get(key).unwrap_or_else(|| panic!("{key} in {path:?}")))
}

fn state(path: &Path) -> copper_state::StateValue {
    copper_state::StateDocument::read(&read(path)).expect("a state file").value
}

#[test]
fn direct_cli_keeps_direct_work_limits_and_reports_missing_verifier() {
    for search_states in [1i64, 2_000_000] {
        let directory = scratch(&search_states.to_string());
        let states = format!("maximum_expansions={search_states}");
        let output = Command::new(env!("CARGO_BIN_EXE_copper"))
            .args([
                "route-board",
                "board.copper",
                "--footprint-root",
                "footprints",
                "--unified-set",
                "direct=true",
                "--unified-time-budget-s",
                "10",
                "--unified-set",
                &states,
                "--unified-save-inputs",
                "inputs.state.json",
                "--checkpoint-dir",
                "checkpoint",
                "--candidates",
                "1",
                "--feedback-iterations",
                "1",
                "--router-iterations",
                "1",
                "--verify-plane-fill",
                "missing-kicad.exe",
                "--report",
                "route.json",
                "-o",
                "board.kicad_pcb",
            ])
            .current_dir(&directory)
            .output()
            .expect("the copper binary runs");
        // A missing verifier remains a failure, never a routing success.
        assert_ne!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stdout));
        // The console names the missing checker, not just the open zone nets.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().find(|line| line.starts_with("PLANE CHECK NOT RUN:")).unwrap_or_else(|| {
            panic!("no checker failure line in:\n{stdout}");
        });
        assert!(line.contains("kicad-cli missing-kicad.exe failed"), "{line}");
        assert!(line.contains("leave the option out"), "{line}");
        let data = copper_json::parse(&read(&directory.join("route.json"))).expect("report");
        assert_eq!(at(&data, &["work_limits", "router"]).as_str(), Some("unified"));
        assert_eq!(at(&data, &["work_limits", "unified_direct"]), &Value::Bool(true));
        assert_eq!(at(&data, &["work_limits", "filled_copper_trials"]), &Value::from(0i64));
        assert_eq!(at(&data, &["filled_copper_feedback", "stop_reason"]).as_str(), Some("missing_tool"));
        assert_eq!(at(&data, &["work_limits", "unified_search_states"]), &Value::from(search_states));
        let saved =
            copper_route::state_codec::load_inputs(&read(&directory.join("inputs.state.json")), "inputs.state.json")
                .expect("saved inputs");
        assert_eq!(saved.options.maximum_expansions, search_states);
        let final_board = copper_route::state_codec::physical_board(&state(
            &directory.join("checkpoint").join("final-board.state.json"),
        ))
        .expect("final board");
        let drc = copper_drc::run_physical_drc(&final_board, copper_drc::Policy::default()).expect("drc");
        let summary = copper_route::route_summary::connection_summary(&final_board, Some(&drc)).expect("summary");
        let signal = &summary.iter().find(|(k, _)| k == "signal").expect("signal").1;
        let sorted = |value: &Value| copper_json::Dumps::new().sort_keys(true).dumps(value);
        assert_eq!(sorted(signal), sorted(at(&data, &["summary", "signal"])));
        let opens = at(&data, &["summary", "signal", "opens"]);
        assert_eq!(*opens == Value::from(0i64), search_states > 1, "{search_states}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn unified_search_state_budget_must_be_positive() {
    let options = copper_route::unified_router::UnifiedRouterOptions {
        maximum_expansions: 0,
        ..copper_route::unified_router::UnifiedRouterOptions::default()
    };
    assert!(options.validated().expect_err("zero states").to_string().contains("search states"));
}

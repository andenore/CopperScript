//! `route-board` has one interface per setting and explains removed
//! options; ported from `tests/test_cli_router_flags.py`, run through the
//! `copper` binary. The parser-level halves (`_congestion_feedback_budget`,
//! `_unified_options`) are observed in the report's `work_limits`; the
//! in-crate `route_board::option_tests` cover every retired switch and the
//! direct-mode overrides directly.

use std::path::{Path, PathBuf};
use std::process::Command;

use copper_json::{Object, Value};

fn scratch(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-router-flags").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
}

fn copper(args: &[&str], directory: &Path) -> Run {
    let output =
        Command::new(env!("CARGO_BIN_EXE_copper")).args(args).current_dir(directory).output().expect("copper runs");
    Run {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `_board`: two fixed two-pad parts with a local footprint library.
fn router_board(directory: &Path) -> (PathBuf, PathBuf) {
    let source = directory.join("board.copper");
    std::fs::write(
        &source,
        r#"board RouterFlags {
        silkscreen { branding { enabled = false; } }
        use library "tiny";
        component R1: RESISTOR { footprint="Fixture:Two"; }
        component R2: RESISTOR { footprint="Fixture:Two"; }
        net POWER { R1.1; R2.1; }
        net DATA { R1.2; R2.2; }
        mechanical { outline rectangle { width=20mm; height=12mm; } }
        constraint fixed_placement(R1) { x=3mm; y=6mm; rotation=0; side=front; }
        constraint fixed_placement(R2) { x=17mm; y=6mm; rotation=0; side=front; }
    }"#,
    )
    .unwrap();
    let library = directory.join("footprints").join("Fixture.pretty");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("Two.kicad_mod"),
        r#"(footprint "Two" (layer "F.Cu")
        (fp_rect (start -0.5 -2.5) (end 0.5 0.5) (stroke (width 0.05) (type solid)) (layer "F.CrtYd"))
        (pad "1" smd rect (at 0 0) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask"))
        (pad "2" smd rect (at 0 -2) (size 0.6 0.6) (layers "F.Cu" "F.Paste" "F.Mask")))"#,
    )
    .unwrap();
    (source, directory.join("footprints"))
}

const BUDGET: [&str; 9] = [
    "--unified-time-budget-s",
    "5",
    "--candidates",
    "1",
    "--feedback-iterations",
    "1",
    "--router-iterations",
    "1",
    "--progress",
];

/// `_route`: a bounded `route-board` run and its JSON report.
fn route(directory: &Path, source: &Path, footprints: &Path, name: &str, extra: &[&str]) -> (Run, Object) {
    let report = directory.join(format!("{name}.json"));
    let (source, footprints, report_arg) = (s(source), s(footprints), s(&report));
    let mut args = vec!["route-board", &source, "--footprint-root", &footprints];
    args.extend_from_slice(&BUDGET);
    args.extend_from_slice(&["--report", &report_arg]);
    args.extend_from_slice(extra);
    let run = copper(&args, directory);
    assert_eq!(run.status, 0, "{name}: {}\n{}", run.stderr, run.stdout);
    let text = std::fs::read_to_string(&report).unwrap_or_else(|e| panic!("{name} report: {e}"));
    match copper_json::parse(&text) {
        Ok(Value::Object(object)) => (run, object),
        other => panic!("{name}: not a JSON object: {other:?}"),
    }
}

fn object<'a>(document: &'a Object, key: &str) -> &'a Object {
    match document.get(key) {
        Some(Value::Object(object)) => object,
        other => panic!("{key}: not an object: {other:?}"),
    }
}

/// `PROGRESS` events whose payload mentions `marker`.
fn progress_events(stdout: &str, marker: &str) -> Vec<Object> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("PROGRESS "))
        .filter(|payload| payload.contains(marker))
        .map(|payload| match copper_json::parse(payload) {
            Ok(Value::Object(object)) => object,
            other => panic!("progress event: {other:?}"),
        })
        .collect()
}

fn empty_list(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Array(items)) if items.is_empty())
}

#[test]
fn retired_options_explain_replacement_before_loading_source() {
    let directory = scratch("retired");
    for (flag, replacement) in [
        ("--unified-direct", "use --unified-set direct=true"),
        ("--no-route-smoothing", "use --unified-set route_smoothing=false"),
        ("--stitch-zone-pads", "declared copper zones are stitched automatically"),
    ] {
        let run = copper(&["route-board", "missing.copper", flag], &directory);
        assert_eq!(run.status, 2, "{flag}");
        assert_eq!(run.stderr.trim(), format!("copper route-board: error: {flag} was removed; {replacement}"));
    }
}

#[test]
fn unused_solver_controls_are_refused_with_their_fixed_value() {
    let directory = scratch("fixed");
    for (flag, fixed) in [
        ("--local-demand-pitch-mm", "local demand estimate is off"),
        ("--layer-preference-cost", "former default 4"),
        ("--fanout-step-mm", "every 0.5 mm"),
        ("--filled-copper-search-states", "use --routing-profile"),
    ] {
        let run = copper(&["route-board", "missing.copper", flag, "1"], &directory);
        assert_eq!(run.status, 2, "{flag}");
        assert!(run.stderr.contains(&format!("{flag} was removed; ")), "{flag}: {}", run.stderr);
        assert!(run.stderr.contains(fixed), "{flag}: {}", run.stderr);
    }
}

#[test]
fn expert_options_reach_effective_report() {
    let directory = scratch("expert");
    let (source, footprints) = router_board(&directory);
    let extra = ["--unified-set", "maximum_iterations=20", "--unified-set", "route_smoothing=false"];
    let (run, report) = route(&directory, &source, &footprints, "expert", &extra);
    let limits = object(&report, "work_limits");
    assert_eq!(limits.get("unified_iterations"), Some(&Value::from(20i64)));
    assert!(empty_list(limits.get("ignored_flags")), "{:?}", limits.get("ignored_flags"));
    assert!(empty_list(limits.get("mapped_flags")), "{:?}", limits.get("mapped_flags"));
    assert!(progress_events(&run.stdout, "\"router_options\"").is_empty());
}

#[test]
fn removed_router_flags_are_refused() {
    let directory = scratch("removed");
    for flag in [
        &["--router", "unified"][..],
        &["--passes", "2"],
        &["--soft-ripup"],
        &["--package-access-trials", "0"],
        &["--plan-power-areas"],
        &["--zone-escape-trials", "0"],
    ] {
        let mut args = vec!["route-board", "board.copper"];
        args.extend_from_slice(flag);
        let run = copper(&args, &directory);
        assert_eq!(run.status, 2, "{flag:?}");
        assert!(run.stderr.contains("unrecognized arguments"), "{flag:?}: {}", run.stderr);
    }
}

#[test]
fn one_congestion_budget_reaches_the_report() {
    // `_congestion_feedback_budget` (default 4, else the flag) as reported.
    let directory = scratch("budget");
    let (source, footprints) = router_board(&directory);
    for (extra, expected) in
        [(&[][..], 4i64), (&["--congestion-feedback-trials", "2"], 2), (&["--congestion-feedback-trials", "0"], 0)]
    {
        let (_, report) = route(&directory, &source, &footprints, &format!("budget-{expected}"), extra);
        let limits = object(&report, "work_limits");
        assert_eq!(limits.get("congestion_feedback_trials"), Some(&Value::from(expected)), "{extra:?}");
    }
}

const BYPASS_SOURCE: &str = r#"board MappedFlags {
    silkscreen { branding { enabled = false; } }
    use library "tiny";
    component U: REGULATOR_3V3 { footprint = "SOT-23-3"; }
    component C: CAPACITOR { footprint = "0402"; role = decoupling; decouples = "U.IN"; }
    component R: RESISTOR { footprint = "0402"; }
    net P { U.IN; C.1; R.1; } net G { U.GND; C.2; } net O { U.OUT; R.2; }
    supply GROUND { net = G; voltage = 0V; external = true; }
    supply VIN { net = P; voltage = 5V; external = true; }
    mechanical { outline rectangle { width=20mm; height=12mm; } }
}"#;

#[test]
fn congestion_budget_reaches_the_loop() {
    let directory = scratch("bypass");
    let source = directory.join("bypass.copper");
    std::fs::write(&source, BYPASS_SOURCE).unwrap();
    let report_path = directory.join("mapped.json");
    let (source_arg, report_arg) = (s(&source), s(&report_path));
    let mut args = vec!["route-board", &source_arg, "--allow-proxy-footprints"];
    args.extend_from_slice(&BUDGET);
    args.extend_from_slice(&["--congestion-feedback-trials", "2", "--report", &report_arg]);
    let run = copper(&args, &directory);
    assert_eq!(run.status, 0, "{}\n{}", run.stderr, run.stdout);
    let report = match copper_json::parse(&std::fs::read_to_string(&report_path).unwrap()) {
        Ok(Value::Object(object)) => object,
        other => panic!("{other:?}"),
    };
    let limits = object(&report, "work_limits");
    assert!(empty_list(limits.get("mapped_flags")));
    assert!(empty_list(limits.get("ignored_flags")));
    assert_eq!(limits.get("congestion_feedback_trials"), Some(&Value::from(2i64)));
    assert!(!report.contains_key("critical_placement_feedback"));
    let feedback = object(&report, "placement_feedback");
    assert_eq!(feedback.get("stop_reason"), Some(&Value::from("no_failed_nets")));
    assert!(empty_list(feedback.get("trials")));
    assert!(progress_events(&run.stdout, "\"router_options\"").is_empty());
}

#[test]
fn fine_regions_flag_reaches_the_unified_router_and_the_report() {
    let directory = scratch("fine-regions");
    let (source, footprints) = router_board(&directory);
    let (run, report) = route(&directory, &source, &footprints, "default-fine-regions", &[]);
    let limits = object(&report, "work_limits");
    assert_eq!(limits.get("fine_regions"), Some(&Value::from("arrays")));
    // No ball array: the plan has no patch, and the setup event has no patch fields.
    let block = object(&report, "fine_regions");
    assert_eq!(block.get("mode"), Some(&Value::from("arrays")));
    assert!(empty_list(block.get("patches")));
    assert!(matches!(block.get("base_pitch_mm"), Some(Value::Float(pitch)) if *pitch == 0.25));
    assert_eq!(block.get("total_nodes"), block.get("base_nodes"));
    assert_eq!(block.get("base_nodes"), block.get("uniform_nodes_at_finest"));
    let setups: Vec<Object> =
        progress_events(&run.stdout, "\"unified_setup\"").into_iter().filter(run_finished).collect();
    assert!(!setups.is_empty());
    for event in &setups {
        let details = object(event, "details");
        let keys: Vec<&str> = details.keys().map(String::as_str).collect();
        let mut keys = keys;
        keys.sort_unstable();
        assert_eq!(keys, ["grid", "nets", "seconds"]);
    }
    for mode in ["off", "arrays", "dense"] {
        let setting = format!("fine_regions={mode}");
        let (_, report) =
            route(&directory, &source, &footprints, &format!("{mode}-fine-regions"), &["--unified-set", &setting]);
        assert_eq!(object(&report, "work_limits").get("fine_regions"), Some(&Value::from(mode)));
        if mode == "off" {
            let block = object(&report, "fine_regions");
            assert_eq!(block.len(), 1);
            assert_eq!(block.get("mode"), Some(&Value::from("off")));
        }
    }
}

fn run_finished(event: &Object) -> bool {
    event.values().any(|value| value == &Value::from("finished"))
}

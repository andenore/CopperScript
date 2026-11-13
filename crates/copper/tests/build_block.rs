//! The board `build` block through the command line: its settings are
//! defaults for the commands' options, and the command line overrides them.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

const PARTS: &str = "    use library \"tiny\";\n    component R1: RESISTOR { footprint = \"0402\"; }\n    \
                     component R2: RESISTOR { footprint = \"0402\"; }\n    net N { R1.1; R2.1; }\n    \
                     no_connect R1.2, R2.2 { reason = \"build block fixture\"; }\n";

/// A project of its own with a board of built-in parts and the `build` block `block`.
fn project(name: &str, block: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("build-block-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    std::fs::write(directory.join("copper.mod"), "module example.com/build-block\n").unwrap();
    std::fs::write(directory.join("board.copper"), format!("board B {{\n{PARTS}{block}}}\n")).unwrap();
    directory
}

fn copper(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("the copper binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(path: &Path) -> Value {
    copper_json::parse(&std::fs::read_to_string(path).unwrap_or_else(|_| panic!("{} exists", path.display()))).unwrap()
}

/// Line 7 is `build {`; each setting below has a line of its own.
const BLOCK: &str = "    build {\n        fab_profile = \"jlcpcb-four-layer\";\n        placement {\n            \
                     candidates = 1;\n            feedback_trials = 5;\n        }\n        routing {\n            \
                     effort = quick;\n            filled_copper_trials = 3;\n            time_budget = 60s;\n        \
                     }\n        planes { contact_radius = 3mm; early_stitch = false; }\n        \
                     router { maximum_expansions = 200000; }\n    }\n";

const BLOCK_OPTIONS: &str = "--fab-profile jlcpcb-four-layer --candidates 1 --placement-feedback-trials 5 \
                             --routing-profile quick --filled-copper-trials 3 --unified-time-budget-s 60 \
                             --plane-contact-radius-mm 3 --no-early-plane-stitch \
                             --unified-set maximum_expansions=200000";

#[test]
fn the_compiled_design_holds_the_block_only_when_declared() {
    let with = project("compile-with", BLOCK);
    let output = copper(&with, &["compile", "board.copper", "--offline"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let compiled = copper_json::parse(&text(&output.stdout)).unwrap();
    let build = compiled.get("build").expect("a build member");
    assert_eq!(build.get("fab_profile"), Some(&Value::from("jlcpcb-four-layer")));
    assert_eq!(build.get("placement").and_then(|group| group.get("feedback_trials")), Some(&Value::from(5i64)));
    assert_eq!(build.get("routing").and_then(|group| group.get("time_budget_s")), Some(&Value::from(60i64)));
    assert_eq!(build.get("planes").and_then(|group| group.get("contact_radius_nm")), Some(&Value::from(3_000_000i64)));
    assert_eq!(build.get("planes").and_then(|group| group.get("early_stitch")), Some(&Value::Bool(false)));
    assert_eq!(build.get("router").and_then(|group| group.get("maximum_expansions")), Some(&Value::from(200_000i64)));

    let without = project("compile-without", "");
    let output = copper(&without, &["compile", "board.copper", "--offline"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(copper_json::parse(&text(&output.stdout)).unwrap().get("build").is_none());
}

#[test]
fn check_reports_wrong_values_and_unknown_router_keys_at_their_lines() {
    let cases = [
        ("    build {\n        placement {\n            candidates = 0;\n        }\n    }\n", ":9:13: BLD002:"),
        ("    build {\n        routing { time_budget = 600; }\n    }\n", ":8:19: BLD001:"),
        (
            "    build {\n        router {\n            maximum_expansion = 5;\n        }\n    }\n",
            ":9:13: BLD003: unknown router setting 'maximum_expansion'",
        ),
        ("    build {\n        speed = fast;\n    }\n", ":8:9: PAR021: unknown build setting 'speed'"),
    ];
    for (index, (block, expected)) in cases.iter().enumerate() {
        let directory = project(&format!("check-{index}"), block);
        let output = copper(&directory, &["check", "board.copper", "--offline"]);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{block}: {stderr}");
        assert!(stderr.contains("COMPILE ERROR:") && stderr.contains(expected), "{block}: {stderr}");
    }
    // The machine-readable report carries the same location.
    let directory = project("check-json", cases[2].0);
    let output = copper(&directory, &["check", "board.copper", "--offline", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let report = copper_json::parse(&text(&output.stdout)).unwrap();
    let diagnostic = &report.get("diagnostics").and_then(Value::as_array).expect("diagnostics")[0];
    assert_eq!(diagnostic.get("code"), Some(&Value::from("BLD003")));
    assert_eq!(diagnostic.get("location").and_then(|location| location.get("line")), Some(&Value::from(9i64)));

    let valid = project("check-valid", BLOCK);
    let output = copper(&valid, &["check", "board.copper", "--offline"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
}

#[test]
fn a_dry_run_shows_the_source_options_and_rejects_an_unknown_router_key() {
    let directory = project("dry-run", BLOCK);
    let output = copper(&directory, &["build", "board.copper", "--offline", "--dry-run", "--", "--candidates", "2"]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{}", text(&output.stderr));
    assert!(stdout.contains(&format!("Build settings from board.copper: {BLOCK_OPTIONS}")), "{stdout}");
    // The build always asks route-board for progress lines, once.
    assert!(stdout.contains("--offline --progress --candidates 2 --verify-plane-fill native"), "{stdout}");
    let repeated = copper(&directory, &["build", "board.copper", "--offline", "--dry-run", "--", "--progress"]);
    let stdout = text(&repeated.stdout);
    assert_eq!(repeated.status.code(), Some(0), "{stdout}");
    assert_eq!(stdout.matches("--progress").count(), 1, "{stdout}");
    assert!(!directory.join("build").exists());

    let plain = project("dry-run-plain", "");
    let stdout = text(&copper(&plain, &["build", "board.copper", "--offline", "--dry-run"]).stdout);
    assert!(stdout.contains("Steps:") && !stdout.contains("Build settings from"), "{stdout}");

    let unknown = project("dry-run-unknown", "    build {\n        router { nonsense = 1; }\n    }\n");
    for arguments in [
        &["build", "board.copper", "--offline", "--dry-run"][..],
        &["build", "board.copper", "--offline"],
        &["route-board", "board.copper", "--offline", "--allow-proxy-footprints"],
    ] {
        let output = copper(&unknown, arguments);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {stderr}");
        assert!(stderr.contains(":8:18: BLD003: unknown router setting 'nonsense'"), "{arguments:?}: {stderr}");
    }
    assert!(!unknown.join("build").exists());
}

#[test]
fn route_board_uses_the_source_settings_and_the_command_line_overrides_them() {
    let directory = project("route", BLOCK);
    let output = copper(
        &directory,
        &[
            "route-board",
            "board.copper",
            "--offline",
            "--allow-proxy-footprints",
            "--placement-feedback-trials",
            "7",
            "--routing-profile",
            "thorough",
            "--report",
            "report.json",
            "-o",
            "routed.kicad_pcb",
        ],
    );
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{}", text(&output.stderr));
    assert!(stdout.contains(&format!("Build settings from board.copper: {BLOCK_OPTIONS}\n")), "{stdout}");
    let report = json(&directory.join("report.json"));
    let limit = |name: &str| report.get("work_limits").and_then(|limits| limits.get(name)).cloned();
    // Given on the command line: the command line wins.
    assert_eq!(report.get("routing_profile"), Some(&Value::from("thorough")));
    assert_eq!(limit("placement_feedback_trials"), Some(Value::from(7i64)));
    // Given only in the source. An explicit setting also outranks what an effort preset implies.
    assert_eq!(limit("filled_copper_trials"), Some(Value::from(3i64)));
    assert_eq!(limit("unified_search_states"), Some(Value::from(200_000i64)));
    assert_eq!(limit("unified_time_budget_s"), Some(Value::from(60.0)));
    // Given nowhere: the thorough preset of the command line.
    assert_eq!(limit("decoupling_placement_trials"), Some(Value::from(64i64)));
    // The fabrication preset of the source selected the four-layer stack.
    let board = std::fs::read_to_string(directory.join("routed.kicad_pcb")).unwrap();
    assert!(board.contains("In1.Cu") && board.contains("In2.Cu"));

    // A board without a block prints no settings line.
    let plain = project("route-plain", "");
    let output = copper(
        &plain,
        &["route-board", "board.copper", "--offline", "--allow-proxy-footprints", "-o", "routed.kicad_pcb"],
    );
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{}", text(&output.stderr));
    assert!(!stdout.contains("Build settings from"), "{stdout}");
    assert!(!std::fs::read_to_string(plain.join("routed.kicad_pcb")).unwrap().contains("In1.Cu"));
}

#[test]
fn other_physical_commands_take_the_fabrication_preset_from_the_source() {
    let directory = project("preset", BLOCK);
    let common = ["board.copper", "--offline", "--allow-proxy-footprints"];
    let run = |command: &str, extra: &[&str]| {
        let mut arguments = vec![command];
        arguments.extend(common);
        arguments.extend(extra);
        let output = copper(&directory, &arguments);
        assert!(output.status.success(), "{command}: {}\n{}", text(&output.stdout), text(&output.stderr));
        text(&output.stdout)
    };
    assert!(run("si-check", &[]).contains("4 copper layers"));
    assert!(run("si-check", &["--fab-profile", "generic"]).contains("2 copper layers"));

    run("plan-layout", &["-o", "source.kicad_pcb"]);
    assert!(std::fs::read_to_string(directory.join("source.kicad_pcb")).unwrap().contains("In1.Cu"));
    run("plan-layout", &["-o", "generic.kicad_pcb", "--fab-profile", "generic"]);
    assert!(!std::fs::read_to_string(directory.join("generic.kicad_pcb")).unwrap().contains("In1.Cu"));

    // The four-layer preset of the source refuses two layers; an explicit
    // generic preset on the command line replaces it.
    for (command, extra) in [("edit-mechanical", &["--scene-output", "scene.json"][..]), ("synthesize-antenna", &[])] {
        let mut arguments = vec![command];
        arguments.extend(common);
        arguments.extend(["--layers", "2"]);
        arguments.extend(extra);
        let refused = copper(&directory, &arguments);
        let message = format!("{}{}", text(&refused.stdout), text(&refused.stderr));
        assert_eq!(refused.status.code(), Some(2), "{command}: {message}");
        assert!(message.contains("four-layer profile requires four copper layers"), "{command}: {message}");
        arguments.extend(["--fab-profile", "generic"]);
        let replaced = copper(&directory, &arguments);
        let message = format!("{}{}", text(&replaced.stdout), text(&replaced.stderr));
        assert!(!message.contains("four-layer profile"), "{command}: {message}");
    }
    assert!(directory.join("scene.json").is_file());
}

#[test]
fn a_build_records_the_source_options_in_run_json() {
    let directory = project("run", "    build {\n        routing { effort = quick; }\n    }\n");
    let output = copper(
        &directory,
        &["build", "board.copper", "--offline", "--output-dir", "build/board/run", "--", "--allow-proxy-footprints"],
    );
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{}", text(&output.stderr));
    assert!(stdout.contains("Build settings from board.copper: --routing-profile quick"), "{stdout}");
    let run = directory.join("build/board/run");
    let manifest = json(&run.join("run.json"));
    let recorded: Vec<&str> = manifest
        .get("source_build_options")
        .and_then(Value::as_array)
        .expect("source_build_options")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(recorded, ["--routing-profile", "quick"]);
    let command = manifest.get("routing_command").and_then(Value::as_str).expect("routing_command");
    assert!(command.contains(" --progress ") && !command.contains("--routing-profile"), "{command}");
    assert_eq!(json(&run.join("route-report.json")).get("routing_profile"), Some(&Value::from("quick")));
}

//! `copper build`: the plan, the refusals, and a real native build of a tiny
//! board whose `run.json` binds the files it names.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use copper_json::Value;

/// A project of its own with a board of built-in parts, so nothing is fetched.
fn project(name: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("build-cli-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    std::fs::write(directory.join("copper.mod"), "module example.com/build-cli\n").unwrap();
    std::fs::write(
        directory.join("board.copper"),
        "board B {\n    use library \"tiny\";\n    component R1: RESISTOR { footprint = \"0402\"; }\n    \
         component R2: RESISTOR { footprint = \"0402\"; }\n    net N { R1.1; R2.1; }\n}\n",
    )
    .unwrap();
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

/// Route options that suit the tiny board and need no footprint libraries.
const TINY: [&str; 1] = ["--allow-proxy-footprints"];

fn build_arguments<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut arguments = vec!["build", "board.copper", "--offline"];
    arguments.extend(extra);
    arguments.push("--");
    arguments.extend(TINY);
    arguments
}

fn sha256(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(std::fs::read(path).unwrap()).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn manifest(directory: &Path) -> Value {
    copper_json::parse(&std::fs::read_to_string(directory.join("run.json")).expect("run.json exists")).unwrap()
}

fn field<'a>(document: &'a Value, key: &str) -> &'a Value {
    document.get(key).unwrap_or_else(|| panic!("run.json has no {key}"))
}

fn run_directories(project: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(project.join("build").join("board"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

#[test]
fn dry_run_prints_the_plan_and_writes_nothing() {
    let directory = project("dry-run");
    let output = copper(&directory, &build_arguments(&["--dry-run"]));
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("(verify: native, dry run)"), "{stdout}");
    assert!(stdout.contains("Steps:"), "{stdout}");
    for step in ["1. check", "2. route", "3. fill", "4. drc", "5. gate", "6. manifest"] {
        assert!(stdout.contains(step), "{step}: {stdout}");
    }
    assert!(stdout.contains("copper route-board") && stdout.contains("--verify-plane-fill native"), "{stdout}");
    assert!(stdout.contains("--allow-proxy-footprints"), "options after -- reach route-board: {stdout}");
    assert!(stdout.contains("nothing was written"), "{stdout}");
    // Whether KiCad is installed must not change how footprints resolve.
    assert!(!stdout.contains("--footprint-root"), "a native plan names no footprint root unasked: {stdout}");
    assert!(!directory.join("build").exists(), "a dry run creates no build directory");

    let kicad = copper(&directory, &["build", "board.copper", "--offline", "--dry-run", "--verify", "kicad"]);
    let shown = text(&kicad.stdout);
    // Without KiCad the mode is refused; with it the plan names kicad-cli.
    match kicad.status.code() {
        Some(0) => assert!(shown.contains("kicad-cli") || shown.contains("pcb drc"), "{shown}"),
        Some(2) => assert!(text(&kicad.stderr).contains("KiCad CLI or footprint directory missing")),
        other => panic!("unexpected status {other:?}: {shown}"),
    }
    assert!(!directory.join("build").exists());
}

#[test]
fn runner_controlled_flags_after_the_separator_are_rejected() {
    let directory = project("controlled");
    for flag in [
        &["-o", "x.kicad_pcb"][..],
        &["--output=x"],
        &["--report", "r.json"],
        &["--verify-plane-fill", "none"],
        &["--locked"],
        &["--offline"],
        &["-ox.kicad_pcb"],
    ] {
        let mut arguments = vec!["build", "board.copper", "--dry-run", "--"];
        arguments.extend(flag);
        let output = copper(&directory, &arguments);
        assert_eq!(output.status.code(), Some(2), "{flag:?}: {}", text(&output.stdout));
        let stderr = text(&output.stderr);
        assert!(
            stderr.contains("SETUP ERROR: output, report, verification and resolution flags are controlled"),
            "{flag:?}: {stderr}"
        );
    }
    assert!(!directory.join("build").exists());
}

#[test]
fn bad_invocations_are_setup_errors() {
    let directory = project("setup");
    let cases: [(&[&str], &str); 7] = [
        (&["build", "missing.copper"], "source missing: missing.copper"),
        (&["build"], "a SOURCE board is required"),
        (&["build", "board.copper", "extra.copper"], "unexpected argument: extra.copper"),
        (&["build", "board.copper", "--verify", "magic"], "invalid choice: 'magic'"),
        (&["build", "board.copper", "--copper", "copper"], "--copper has no meaning"),
        (&["build", "board.copper", "--frobnicate"], "frobnicate"),
        (&["build", "board.copper", "--output-dir", "elsewhere/run"], "output must stay below project build/"),
    ];
    for (arguments, expected) in cases {
        let output = copper(&directory, arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {}", text(&output.stdout));
        assert!(text(&output.stderr).contains(expected), "{arguments:?}: {}", text(&output.stderr));
    }
    assert!(!directory.join("build").exists());
}

#[test]
fn a_source_outside_the_project_is_refused() {
    let directory = project("outside");
    let other = project("outside-other");
    let output = copper(
        &directory,
        &["build", other.join("board.copper").to_str().unwrap(), "--project-root", directory.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("source must be inside --project-root"), "{}", text(&output.stderr));
}

#[test]
fn a_native_build_writes_a_manifest_that_binds_its_files() {
    let directory = project("native");
    let output = copper(&directory, &build_arguments(&[]));
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(output.status.code(), Some(0), "{stdout}\n{stderr}");
    assert!(stdout.contains("Build exit code: 0; outputs: "), "{stdout}");

    let runs = run_directories(&directory);
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    for name in [
        "board.kicad_pcb",
        "board.kicad_pro",
        "board.editor-intent.json",
        "route-report.json",
        "native-drc.json",
        "routing.log",
        "fill.log",
        "run.json",
    ] {
        assert!(run.join(name).is_file(), "{name} is missing");
    }
    assert!(!run.join("board.filled.kicad_pcb").exists(), "the filled board replaced the draft");
    assert!(!run.join("run.json.tmp").exists());

    let document = manifest(run);
    assert_eq!(field(&document, "schema").as_str(), Some("copperscript-build/v1"));
    assert_eq!(field(&document, "status").as_str(), Some("passed"));
    assert_eq!(field(&document, "exit_code"), &Value::from(0i64));
    assert_eq!(field(&document, "verify").as_str(), Some("native"));
    assert_eq!(field(&document, "drc_report").as_str(), Some("native-drc.json"));
    assert_eq!(field(&document, "fabrication_ready"), &Value::Bool(false));
    for key in ["started_utc", "finished_utc", "routing_command", "fill_command", "drc_command", "elapsed_seconds"] {
        assert!(document.get(key).is_some(), "{key}");
    }
    assert!(field(&document, "routing_command").as_str().unwrap().contains("--verify-plane-fill native"));

    // Every recorded hash is the hash of the file on disk.
    assert_eq!(
        field(&document, "editor_intent_sha256").as_str(),
        Some(sha256(&run.join("board.editor-intent.json")).as_str())
    );
    assert_eq!(field(&document, "filled_board_sha256").as_str(), Some(sha256(&run.join("board.kicad_pcb")).as_str()));
    assert_eq!(field(&document, "native_drc_sha256").as_str(), Some(sha256(&run.join("native-drc.json")).as_str()));
    // The tiny board has no zones, so the refill leaves the draft's bytes as they were.
    assert_eq!(field(&document, "draft_board_sha256"), field(&document, "filled_board_sha256"));

    let provenance = field(&document, "provenance");
    let inputs = field(provenance, "input_sha256").as_object().expect("an input map");
    for name in ["board.copper", "copper.mod"] {
        let recorded =
            inputs.get(name).and_then(Value::as_str).unwrap_or_else(|| panic!("input {name} is not recorded"));
        assert_eq!(recorded, sha256(&directory.join(name)), "{name}");
    }
    assert!(inputs.keys().all(|name| !name.starts_with("build/")), "generated trees are not inputs: {inputs:?}");
    let compiler = field(provenance, "compiler");
    assert_eq!(
        field(compiler, "sha256").as_str(),
        Some(sha256(Path::new(env!("CARGO_BIN_EXE_copper"))).as_str()),
        "the compiler hash is the running binary's"
    );
    assert!(field(compiler, "version").as_str().unwrap().starts_with("copper "));

    // The gate's own inputs say what the status claims.
    let report = copper_json::parse(&std::fs::read_to_string(run.join("route-report.json")).unwrap()).unwrap();
    assert_eq!(field(&report, "status").as_str(), Some("pass"));
    let drc = copper_json::parse(&std::fs::read_to_string(run.join("native-drc.json")).unwrap()).unwrap();
    assert_eq!(field(&drc, "violations").as_array().map(<[Value]>::len), Some(0));
    // The overlay reads the unit from the report itself.
    assert_eq!(field(&drc, "coordinate_units").as_str(), Some("mm"));
}

#[test]
fn builds_never_share_or_overwrite_a_run_directory() {
    let directory = project("two");
    let first = copper(&directory, &build_arguments(&[]));
    assert_eq!(first.status.code(), Some(0), "{}\n{}", text(&first.stdout), text(&first.stderr));
    let runs = run_directories(&directory);
    assert_eq!(runs.len(), 1);
    let first_run = runs[0].clone();
    let before = std::fs::read(first_run.join("run.json")).unwrap();

    // A named directory that already holds a run is refused and left alone.
    let reused = copper(&directory, &build_arguments(&["--output-dir", runs[0].to_str().unwrap()]));
    assert_eq!(reused.status.code(), Some(2), "{}", text(&reused.stdout));
    assert!(text(&reused.stderr).contains("output is not empty"), "{}", text(&reused.stderr));
    assert_eq!(std::fs::read(runs[0].join("run.json")).unwrap(), before);

    // A second default build gets a directory of its own (the process id
    // differs), and the first run's files are untouched.
    let second = copper(&directory, &build_arguments(&[]));
    assert_eq!(second.status.code(), Some(0), "{}\n{}", text(&second.stdout), text(&second.stderr));
    let runs = run_directories(&directory);
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert_eq!(std::fs::read(first_run.join("run.json")).unwrap(), before, "the first run is untouched");
    let hashes: Vec<String> =
        runs.iter().map(|run| field(&manifest(run), "filled_board_sha256").as_str().unwrap().to_owned()).collect();
    for (run, hash) in runs.iter().zip(&hashes) {
        assert_eq!(&sha256(&run.join("board.kicad_pcb")), hash, "{}", run.display());
    }
    // The same inputs give the same board.
    assert_eq!(hashes[0], hashes[1]);
}

#[test]
fn a_failed_gate_is_recorded_with_exit_one() {
    let directory = project("gate");
    // A run that stops after placement is marked partial: it can never pass,
    // so the build records unmet gates and binds no filled board.
    let mut arguments = build_arguments(&[]);
    arguments.extend(["--stop-after", "placement"]);
    let output = copper(&directory, &arguments);
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(1),
        "{stdout}
{stderr}"
    );
    assert!(stdout.contains("Build exit code: 1; outputs: "), "{stdout}");
    let runs = run_directories(&directory);
    assert_eq!(runs.len(), 1);
    let document = manifest(&runs[0]);
    assert_eq!(field(&document, "status").as_str(), Some("unmet_gates"));
    assert_eq!(field(&document, "exit_code"), &Value::from(1i64));
    assert!(document.get("filled_board_sha256").is_none(), "a failed build binds no filled board");
    assert!(document.get("native_drc_sha256").is_none());
    assert!(!runs[0].join("fill.log").exists(), "the refill does not run after a failed route");
}

/// `kicad-cli` and the footprint libraries of an installed KiCad, if any.
fn kicad() -> Option<(PathBuf, PathBuf)> {
    let from_env = std::env::var_os("KICAD_CLI").map(PathBuf::from);
    let standard = PathBuf::from("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe");
    let cli = from_env.into_iter().chain([standard]).find(|path| path.is_file())?;
    let footprints = cli.parent()?.parent()?.join("share").join("kicad").join("footprints");
    footprints.is_dir().then_some((cli, footprints))
}

#[test]
fn a_kicad_build_binds_the_saved_board_and_kicad_report() {
    let Some((cli, footprints)) = kicad() else {
        eprintln!("skipped: no KiCad installation found (set KICAD_CLI to run this test)");
        return;
    };
    let directory = project("kicad");
    std::fs::write(
        directory.join("board.copper"),
        "board B {
    use library \"tiny\";
    component R1: RESISTOR { footprint = \"Resistor_SMD:R_0402_1005Metric\"; }
             component R2: RESISTOR { footprint = \"Resistor_SMD:R_0402_1005Metric\"; }
    net N { R1.1; R2.1; }
}
",
    )
    .unwrap();
    let output = copper(
        &directory,
        &[
            "build",
            "board.copper",
            "--offline",
            "--verify",
            "kicad",
            "--kicad-cli",
            cli.to_str().unwrap(),
            "--kicad-footprints",
            footprints.to_str().unwrap(),
        ],
    );
    let (stdout, stderr) = (text(&output.stdout), text(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{stdout}
{stderr}"
    );
    let runs = run_directories(&directory);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    let document = manifest(run);
    assert_eq!(field(&document, "status").as_str(), Some("passed"));
    assert_eq!(field(&document, "verify").as_str(), Some("kicad"));
    assert_eq!(field(&document, "drc_report").as_str(), Some("kicad-drc.json"));
    assert!(document.get("drc_command").is_none(), "KiCad refills and checks in one command");
    assert!(field(&document, "fill_command").as_str().unwrap().contains("pcb drc --refill-zones --save-board"));
    assert!(!run.join("native-drc.json").exists());
    assert_eq!(field(&document, "filled_board_sha256").as_str(), Some(sha256(&run.join("board.kicad_pcb")).as_str()));
    assert_eq!(field(&document, "native_drc_sha256").as_str(), Some(sha256(&run.join("kicad-drc.json")).as_str()));
    // KiCad's save rewrites the board, so the draft hash is the one before it.
    assert_ne!(field(&document, "draft_board_sha256"), field(&document, "filled_board_sha256"));
    let kicad_report = copper_json::parse(&std::fs::read_to_string(run.join("kicad-drc.json")).unwrap()).unwrap();
    assert_eq!(field(&kicad_report, "violations").as_array().map(<[Value]>::len), Some(0));
    assert!(std::fs::read_to_string(run.join("fill.log")).unwrap().contains("Saved board"));
}

#[test]
fn a_native_build_passes_a_footprint_root_only_when_asked() {
    let directory = project("roots");
    let libraries = directory.join("libraries");
    std::fs::create_dir_all(&libraries).unwrap();
    let asked = copper(
        &directory,
        &["build", "board.copper", "--offline", "--dry-run", "--kicad-footprints", libraries.to_str().unwrap()],
    );
    let stdout = text(&asked.stdout);
    assert_eq!(
        asked.status.code(),
        Some(0),
        "{stdout}
{}",
        text(&asked.stderr)
    );
    assert!(stdout.contains("--footprint-root") && stdout.contains("libraries"), "{stdout}");
    let missing = copper(&directory, &["build", "board.copper", "--dry-run", "--kicad-footprints", "no/such/dir"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(text(&missing.stderr).contains("no/such/dir"), "{}", text(&missing.stderr));
}

#[test]
fn an_explicit_kicad_cli_that_does_not_exist_is_an_error_in_both_modes() {
    let directory = project("no-cli");
    for mode in ["native", "kicad"] {
        let output = copper(
            &directory,
            &["build", "board.copper", "--dry-run", "--verify", mode, "--kicad-cli", "no/such/kicad-cli"],
        );
        assert_eq!(output.status.code(), Some(2), "{mode}: {}", text(&output.stdout));
        let stderr = text(&output.stderr);
        assert!(stderr.contains("kicad-cli missing: no/such/kicad-cli"), "{mode}: {stderr}");
    }
    assert!(!directory.join("build").exists());
}

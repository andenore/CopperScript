//! `sim spice` through the whole chain: board, part block, model file,
//! reader, solver and measures. The example's subcircuit is written for the
//! repository (examples/simulation/spice_amplifier/models/); the vendor
//! example is checked only for its offline failure (its model is not in the
//! repository and the default tests never use the network).

use std::fs;
use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation").join(path)
}

fn run_board(board: &Path) -> Vec<BenchRun> {
    let design = compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    simulate(board, &design, None).unwrap_or_else(|failure| panic!("{failure}"))
}

fn measure(run: &BenchRun, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[0].measures[k].value
}

fn bench<'a>(runs: &'a [BenchRun], name: &str) -> &'a BenchRun {
    runs.iter().find(|run| run.name == name).unwrap_or_else(|| panic!("{name}"))
}

#[test]
fn spice_amplifier_example_passes_op_ac_tran_and_noise() {
    let runs = run_board(&example("spice_amplifier/board.copper"));
    println!("{}", copper_simulate::results::text_table(&runs));
    assert_eq!(runs.len(), 4);
    for run in &runs {
        assert_eq!(run.error, None, "{}", run.name);
        assert_eq!(run.status(), Status::Pass, "{}", run.name);
        let u1 = run.models.iter().find(|m| m.name == "U1").unwrap();
        assert_eq!(u1.fidelity, "vendor");
        assert!(
            u1.model.starts_with("spice BEHAVIOURAL_OPAMP (../models/behavioural_opamp.lib, pspice dialect, sha256 "),
            "{}",
            u1.model
        );
    }
    // Hand values from the bench comments.
    let op = bench(&runs, "OperatingPoint");
    let a = 1e5 * (10e3 / 6.0) / (10e3 / 6.0 + 10.0);
    let vout = a / (1.0 + a / 10.0);
    assert!((measure(op, "output") - vout).abs() < 1e-6, "{}", measure(op, "output"));
    assert!((measure(op, "supply_current") - (1.5e-3 + vout / (10e3 / 6.0))).abs() < 1e-9);
    let tran = bench(&runs, "Step");
    assert!((measure(tran, "slew") / 10e6 - 1.0).abs() < 0.02, "{}", measure(tran, "slew"));
    let noise = bench(&runs, "Noise");
    let kt4 = 4.0 * copper_sim::noise::BOLTZMANN * copper_sim::noise::DEFAULT_TEMPERATURE;
    let gain = vout;
    let expect = (kt4 * 1.5e3 * gain * gain + kt4 * 9e3 + kt4 * 1e3 * 81.0).sqrt();
    assert!(
        (measure(noise, "en_out_10khz") / expect - 1.0).abs() < 2e-3,
        "{} vs {expect}",
        measure(noise, "en_out_10khz")
    );
}

#[test]
fn vendor_models_fail_clearly_without_the_cached_file() {
    let board = example("vendor_models/board.copper");
    let design = compile_design_file(&board, false, true).unwrap();
    let cache = copper_lang::spice_models::cache_directory(&board);
    let runs = simulate(&board, &design, Some("Frequency")).unwrap();
    let error = runs[0].error.clone().unwrap_or_default();
    if cache.join("bee7ec13330e01d291d3e05c3b703647af276f3b174a51d40680d36905c4a0a0").exists() {
        // Someone fetched it: then the bench runs.
        assert_eq!(runs[0].status(), Status::Pass, "{error}");
    } else {
        assert!(
            error.contains(
                "U_VENDOR: SIM077: the vendor SPICE model https://www.ti.com/lit/zip/SBOMB73 is not in the cache"
            ),
            "{error}"
        );
        assert!(error.contains("run copper models fetch"), "{error}");
    }
}

#[test]
fn netlist_and_file_errors_name_their_source() {
    let directory = std::env::temp_dir().join(format!("copper-spice-runs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
    fs::create_dir_all(directory.join("benches")).unwrap();
    let part = |file: &str| {
        format!(
            "part Amp {{
    category = \"amplifier.opamp\";
    pin INP {{ number = \"1\"; domains = \"analog\"; directions = \"input\"; }}
    pin INN {{ number = \"2\"; domains = \"analog\"; directions = \"input\"; }}
    pin OUT {{ number = \"3\"; domains = \"analog\"; directions = \"output\"; }}
    sim spice {{ file = \"{file}\"; subckt = \"AMP\"; terminals = INP, INN, OUT; }}
}}
"
        )
    };
    fs::write(
        directory.join("board.copper"),
        "board B {\n    use library \"tiny\";\n    import p \"./parts\";\n    component U1: p.Amp;\n    component R1: RESISTOR { value = 1kohm; }\n    net A { U1.INP; R1.1; }\n    net GND { U1.INN; R1.2; }\n    net OUT { U1.OUT; }\n    supply G { voltage = 0V; net = GND; external = true; }\n}\n",
    )
    .unwrap();
    fs::write(
        directory.join("benches").join("op.copper"),
        "bench Op for B {\n    analysis op;\n    measure out = V(OUT);\n}\n",
    )
    .unwrap();
    let run = |file: &str, netlist: Option<&str>| {
        fs::write(directory.join("parts").join("amp.copper"), part(file)).unwrap();
        if let Some(text) = netlist {
            fs::write(directory.join("parts").join(file), text).unwrap();
        }
        let board = directory.join("board.copper");
        let design = compile_design_file(&board, false, true).unwrap();
        simulate(&board, &design, None).unwrap().remove(0)
    };
    let missing = run("none.lib", None);
    assert!(
        missing.error.as_deref().unwrap_or_default().contains("U1: SIM077: the SPICE model file"),
        "{:?}",
        missing.error
    );
    let bad = run("bad.lib", Some(".SUBCKT AMP A B C\nE1 C 0 A B 1E5\nT1 A 0 B 0 Z0=50 TD=1N R=0.1\n.ENDS\n"));
    let message = bad.error.unwrap_or_default();
    assert!(
        message.contains("SIM078")
            && message.contains("bad.lib:3")
            && message.contains("U1.T1")
            && message.contains("transmission lines"),
        "{message}"
    );
    let count = run("count.lib", Some(".SUBCKT AMP A B\nR1 A B 1K\n.ENDS\n"));
    assert!(count.error.unwrap_or_default().contains("has 2 nodes (A B), but the sim spice block lists 3 terminals"));
    let good = run("good.lib", Some(".SUBCKT AMP A B C\nE1 C 0 A B 2\nRL C 0 1K\n.ENDS\n"));
    assert_eq!(good.status(), Status::Pass, "{:?}", good.error);
    let _ = fs::remove_dir_all(&directory);
}

/// A board whose part `SRC` is a local model file `models/root.lib` that
/// includes `models/sub.inc`, where a subcircuit drives `volts` into a 1 kohm
/// load; returns the board file.
fn included_model_project(name: &str, volts: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-spice-included-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    for sub in ["parts", "benches", "models"] {
        fs::create_dir_all(directory.join(sub)).unwrap();
    }
    fs::write(
        directory.join("parts/src.copper"),
        "part Src {
    category = \"power.reference\";
    pin OUT { number = \"1\"; domains = \"analog\"; directions = \"output\"; }
    pin REF { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
    sim spice { file = \"../models/root.lib\"; subckt = \"SRC\"; terminals = OUT, REF; }
}
",
    )
    .unwrap();
    fs::write(
        directory.join("board.copper"),
        "board B {\n    use library \"tiny\";\n    import p \"./parts\";\n    component U1: p.Src;\n    component R1: RESISTOR { value = 1kohm; }\n    net OUT { U1.OUT; R1.1; }\n    net GND { U1.REF; R1.2; }\n    supply G { voltage = 0V; net = GND; external = true; }\n}\n",
    )
    .unwrap();
    fs::write(
        directory.join("benches/op.copper"),
        "bench Op for B {\n    analysis op;\n    measure out = V(OUT);\n}\n",
    )
    .unwrap();
    fs::write(directory.join("models/root.lib"), "* the root file\n.INCLUDE sub.inc\n").unwrap();
    write_included(&directory.join("board.copper"), volts);
    directory.join("board.copper")
}

/// Rewrite the included file of [`included_model_project`] to drive `volts`.
fn write_included(board: &Path, volts: &str) {
    let file = board.parent().unwrap().join("models/sub.inc");
    fs::write(file, format!(".SUBCKT SRC OUT REF\nV1 OUT REF {volts}\nRS OUT REF 1MEG\n.ENDS\n")).unwrap();
}

#[test]
fn a_changed_included_model_file_is_read_again_in_the_same_process() {
    let board = included_model_project("cache", "1");
    let out = measure(&run_board(&board)[0], "out");
    assert!((out - 1.0).abs() < 1e-9, "{out}");
    // Only the included file changes; the root file is as it was.
    write_included(&board, "2");
    let out = measure(&run_board(&board)[0], "out");
    assert!((out - 2.0).abs() < 1e-9, "the included file's new subcircuit: {out}");
    let _ = fs::remove_dir_all(board.parent().unwrap());
}

#[test]
fn results_are_stale_once_an_included_model_file_changes() {
    use copper_simulate::evidence::{self, SourceIdentity};
    let board = included_model_project("evidence", "1");
    let design = compile_design_file(&board, false, true).unwrap();
    let runs = simulate(&board, &design, None).unwrap();
    let source = SourceIdentity::of(&board, &design);
    let document = copper_simulate::results::results_value_from(&runs, &design.electrical.name, Some(&source));
    let written = evidence::read("sim-results.json", &document).unwrap();
    assert_eq!(evidence::validate_source(&written, &board, &design), Ok(()));
    write_included(&board, "2");
    let reason = evidence::validate_source(&written, &board, &design).expect_err("the model changed");
    assert!(reason.contains("model") && reason.contains("sub.inc"), "{reason}");
    // As the root file does.
    write_included(&board, "1");
    assert_eq!(evidence::validate_source(&written, &board, &design), Ok(()));
    let root = board.parent().unwrap().join("models/root.lib");
    fs::write(&root, "* the root file, edited\n.INCLUDE sub.inc\n").unwrap();
    let reason = evidence::validate_source(&written, &board, &design).expect_err("the root changed");
    assert!(reason.contains("root.lib"), "{reason}");
    let _ = fs::remove_dir_all(board.parent().unwrap());
}

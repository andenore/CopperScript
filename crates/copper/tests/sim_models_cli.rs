//! Plan F6 on the command line: `copper sim --models`, `copper sim
//! --check-model` and `copper models add` for a file registered by hand. The
//! model file is written for these tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper"))
        .args(arguments)
        .current_dir(workspace())
        .output()
        .expect("the copper binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const AMP: &str = "\
* A self-written op amp: 100 dB, 1 MHz, 1 mA from a 10 V span.
.SUBCKT AMP INP INN VP VN OUT
G1 0 N1 INP INN 1m
R1 N1 0 100Meg
C1 N1 0 159.155p
E1 OUT 0 N1 0 1
RQ VP VN 10k
.ENDS
";

/// A project whose part `Amp` has a macro block and a vendor block registered by hand
/// (`hash`: the sha256 the part declares).
fn project(name: &str, hash: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-sim-models-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    std::fs::create_dir_all(directory.join("benches")).unwrap();
    std::fs::write(directory.join("copper.mod"), "module example.invalid/models-cli\n").unwrap();
    std::fs::write(
        directory.join("parts").join("amp.copper"),
        format!(
            "part Amp {{
    category = \"amplifier.opamp\";
    pin INP {{ number = \"1\"; domains = \"analog\"; directions = \"input\"; }}
    pin INN {{ number = \"2\"; domains = \"analog\"; directions = \"input\"; }}
    pin OUT {{ number = \"3\"; domains = \"analog\"; directions = \"output\"; }}
    pin VP {{ number = \"4\"; domains = \"power\"; directions = \"input\"; }}
    pin VN {{ number = \"5\"; domains = \"power\"; directions = \"input\"; }}
    sim opamp {{ inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; quiescent_current = 2mA; }}
    sim spice {{ sha256 = \"{hash}\"; download = \"the AMP model, written by the test\"; subckt = \"AMP\"; dialect = pspice; terminals = INP, INN, VP, VN, OUT; }}
}}
"
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("board.copper"),
        "board B {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.Amp;
    component R1: RESISTOR { value = 10kohm; }
    net IN { U1.INP; R1.1; }
    net GND { R1.2; }
    net OUT { U1.OUT; U1.INN; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    supply G { voltage = 0V; net = GND; external = true; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
}
",
    )
    .unwrap();
    std::fs::write(
        directory.join("benches").join("op.copper"),
        "bench Op for B {\n    analysis op;\n    measure iq = I(POS);\n}\n",
    )
    .unwrap();
    // The file the user "downloaded", outside the project.
    std::fs::write(directory.join("amp.cir"), AMP).unwrap();
    directory
}

fn sha256(bytes: &[u8]) -> String {
    copper_lang::remote_models::sha256_hex(bytes)
}

#[test]
fn models_add_registers_a_file_and_sim_runs_the_vendor_model() {
    let directory = project("add", &sha256(AMP.as_bytes()));
    let board = directory.join("board.copper");
    let board = board.to_str().unwrap();
    let out = directory.join("out");
    let out = out.to_str().unwrap();
    // The macro model runs by default, without the file.
    let output = copper(&["sim", board, "--out", out]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("iq") && stdout.contains("2 mA"), "{stdout}");
    // --models vendor without the file: SIM108 naming the download note and the command.
    let output = copper(&["sim", board, "--models", "vendor", "--out", out]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("SIM108") && stdout.contains("the AMP model, written by the test"), "{stdout}");
    assert!(stdout.contains("copper models add"), "{stdout}");
    // models list shows it missing with its note; fetch cannot fetch it.
    let output = copper(&["models", "list", board]);
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("missing  SPICE model registered by hand") && stdout.contains("download: the AMP model"),
        "{stdout}"
    );
    let output = copper(&["models", "fetch", board]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stdout).contains("download it yourself"), "{}", text(&output.stdout));
    // A file of another hash is refused, and its .SUBCKT lines are named.
    let other = directory.join("other.cir");
    std::fs::write(&other, AMP.replace("10k", "20k")).unwrap();
    let output = copper(&["models", "add", board, other.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("FAILED") && stdout.contains("nothing was stored"), "{stdout}");
    assert!(stdout.contains("the file defines .SUBCKT AMP INP INN VP VN OUT"), "{stdout}");
    // The right file is registered into the project's cache.
    let file = directory.join("amp.cir");
    let output = copper(&["models", "add", board, file.to_str().unwrap()]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("added") && stdout.contains("for p.Amp"), "{stdout}");
    let cached = directory.join(".copper-cache").join("spice").join(sha256(AMP.as_bytes())).join("amp.cir");
    assert!(cached.is_file(), "{}", cached.display());
    let output = copper(&["models", "list", board]);
    assert!(text(&output.stdout).contains("cached   SPICE model registered by hand"), "{}", text(&output.stdout));
    let output = copper(&["sim", board, "--models", "vendor", "--out", out]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("1 mA"), "{stdout}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn check_model_prints_both_models_and_its_verdict() {
    let directory = project("check", &sha256(AMP.as_bytes()));
    let board = directory.join("board.copper");
    let board = board.to_str().unwrap();
    // Without the file the vendor column is not run; the macro model decides.
    let output = copper(&["sim", "--check-model", "Amp", board, "--no-tran"]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.starts_with("CHECK MODEL p.Amp (sim opamp) at a 10 V supply (+/-5 V)"), "{stdout}");
    assert!(stdout.contains("vendor: spice AMP (registered by hand") && stdout.contains("not run: SIM108"), "{stdout}");
    assert!(stdout.contains("gain-bandwidth") && stdout.contains("PASS"), "{stdout}");
    assert!(!stdout.contains("slew rate"), "--no-tran leaves the step out: {stdout}");
    // With the file the vendor figures appear beside the macro ones.
    let file = directory.join("amp.cir");
    assert_eq!(copper(&["models", "add", board, file.to_str().unwrap()]).status.code(), Some(0));
    let output = copper(&["sim", "--check-model", "p.Amp", board, "--supply", "12V"]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
    assert!(stdout.contains("at a 12 V supply (+/-6 V)"), "{stdout}");
    assert!(stdout.contains("loads, operating points converge"), "{stdout}");
    let supply = stdout.lines().find(|line| line.trim_start().starts_with("supply current")).unwrap_or_default();
    assert!(supply.contains("2 mA") && supply.contains("1.2 mA") && supply.contains("DIFFERS"), "{supply}");
    assert!(stdout.contains("slew rate"), "{stdout}");
    // Usage errors.
    let output = copper(&["sim", "--check-model", "Nothing", board]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("SIM109"), "{}", text(&output.stderr));
    let output = copper(&["sim", board, "--supply", "5V"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("belong to --check-model"), "{}", text(&output.stderr));
    let output = copper(&["sim", board, "--models", "best"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("invalid choice: 'best' (choose from default, macro, vendor)"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn check_model_on_the_dual_opa810_example_part() {
    // The example's part has both models; the vendor archive is fetched only by
    // copper models fetch, so this checks the macro model and names the cache.
    let output = copper(&[
        "sim",
        "--check-model",
        "OPA810",
        "examples/simulation/vendor_models/board.copper",
        "--supply",
        "24V",
        "--no-tran",
    ]);
    let stdout = text(&output.stdout);
    assert!(stdout.starts_with("CHECK MODEL p.OPA810 (sim opamp) at a 24 V supply (+/-12 V)"), "{stdout}");
    assert!(stdout.contains("vendor: spice OPA810"), "{stdout}");
    assert_eq!(output.status.code(), Some(0), "{stdout}{}", text(&output.stderr));
}

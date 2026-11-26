//! Plan F6 through the whole chain: a part with a macro and a vendor model,
//! `copper sim --models`, a vendor file registered by hand, and the standard
//! bench set of `--check-model`. The subcircuit is written for these tests.

use std::fs;
use std::path::{Path, PathBuf};

use copper_lang::compiler::load_design;
use copper_simulate::check_model::{CheckOptions, Figure, Limits, ModelState, Tolerance, check_part, find_part, text};
use copper_simulate::{ModelChoice, Status, simulate_with};

/// A single-pole op amp, 100 dB and 1 MHz, that draws 1 mA from a 10 V span through RQ.
const AMP: &str = "\
* A self-written op amp for the model-library tests.
.SUBCKT AMP INP INN VP VN OUT
G1 0 N1 INP INN 1m
R1 N1 0 100Meg
C1 N1 0 159.155p
E1 OUT 0 N1 0 1
RQ VP VN 10k
.ENDS
";

const PINS: &str = "    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
";

const MACRO: &str = "sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; quiescent_current = 2mA; voltage_noise = 10nV/rtHz; slew_rate = 1V/us; }";

/// A project (copper.mod, board, bench, part `Amp` with `sim`) in a fresh directory.
fn project(name: &str, sim: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-model-library-runs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
    fs::create_dir_all(directory.join("benches")).unwrap();
    fs::write(directory.join("copper.mod"), "module example.invalid/models\n").unwrap();
    fs::write(directory.join("parts").join("amp.cir"), AMP).unwrap();
    fs::write(directory.join("parts").join("amp.copper"), format!("part Amp {{\n{PINS}{sim}\n}}\n")).unwrap();
    fs::write(
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
    fs::write(
        directory.join("benches").join("op.copper"),
        "bench Op for B {\n    analysis op;\n    measure iq = I(POS);\n}\n",
    )
    .unwrap();
    // Named as the loader resolves it: on Windows the temporary directory is an 8.3 path (RUNNER~1) that the
    // resolved one expands, and the tests compare cache paths with the loader's.
    copper_lang::paths::resolve(&directory)
}

fn run(directory: &Path, choice: ModelChoice) -> copper_simulate::BenchRun {
    let board = directory.join("board.copper");
    let design = load_design(&board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    simulate_with(&board, &design, None, choice).unwrap_or_else(|failure| panic!("{failure}")).remove(0)
}

fn iq(run: &copper_simulate::BenchRun) -> f64 {
    assert_eq!(run.error, None);
    run.steps[0].measures[0].value
}

const SPICE: &str =
    "sim spice { file = \"amp.cir\"; subckt = \"AMP\"; dialect = pspice; terminals = INP, INN, VP, VN, OUT; }";

#[test]
fn the_model_choice_picks_the_macro_or_the_vendor_model() {
    let directory = project("choice", &format!("{MACRO}\n{SPICE}"));
    let macro_run = run(&directory, ModelChoice::Default);
    assert!((iq(&macro_run) - 2e-3).abs() < 1e-9, "the macro model's quiescent current");
    let u1 = macro_run.models.iter().find(|m| m.name == "U1").unwrap();
    assert_eq!((u1.fidelity, u1.model.as_str()), ("macro", "opamp"));
    assert!((iq(&run(&directory, ModelChoice::Macro)) - 2e-3).abs() < 1e-9);
    let vendor_run = run(&directory, ModelChoice::Vendor);
    assert!((iq(&vendor_run) - 1e-3).abs() < 1e-9, "10 V across RQ = 10 kohm");
    let u1 = vendor_run.models.iter().find(|m| m.name == "U1").unwrap();
    assert_eq!(u1.fidelity, "vendor");
    assert!(u1.model.starts_with("spice AMP (amp.cir, pspice dialect"), "{}", u1.model);
}

#[test]
fn models_macro_refuses_a_part_with_only_a_vendor_model() {
    let directory = project("spice-only", SPICE);
    // Without --models macro the spice model runs.
    assert!((iq(&run(&directory, ModelChoice::Default)) - 1e-3).abs() < 1e-9);
    let refused = run(&directory, ModelChoice::Macro);
    assert_eq!(refused.status(), Status::Error);
    let error = refused.error.unwrap_or_default();
    assert!(error.contains("SIM107") && error.contains("U1") && error.contains("--models macro"), "{error}");
}

#[test]
fn a_file_registered_by_hand_runs_once_added_to_the_cache() {
    let hash = copper_lang::remote_models::sha256_hex(AMP.as_bytes());
    let block = format!(
        "sim spice {{ sha256 = \"{hash}\"; download = \"write amp.cir yourself\"; subckt = \"AMP\"; dialect = pspice; terminals = INP, INN, VP, VN, OUT; }}"
    );
    let directory = project("registered", &format!("{MACRO}\n{block}"));
    // The macro model runs without the file; the vendor model names what to download.
    assert!((iq(&run(&directory, ModelChoice::Default)) - 2e-3).abs() < 1e-9);
    let missing = run(&directory, ModelChoice::Vendor);
    let error = missing.error.unwrap_or_default();
    assert!(error.contains("SIM108") && error.contains("write amp.cir yourself"), "{error}");
    assert!(error.contains("copper models add"), "{error}");
    // Register it as copper models add does: into <project>/.copper-cache/spice/<sha256>/.
    let design = load_design(&directory.join("board.copper"), false, true).unwrap();
    let models = copper_lang::spice_models::library_spice_models(&design.electrical.library);
    assert_eq!(models.len(), 1);
    assert!(models[0].registered());
    let cache = models[0].cache();
    assert_eq!(cache, directory.join(".copper-cache").join("spice"));
    assert!(models[0].register(&cache, b"not the model", "amp.cir").unwrap_err().contains("nothing was stored"));
    let stored = models[0].register(&cache, AMP.as_bytes(), "amp.cir").unwrap();
    assert_eq!(stored, cache.join(&hash).join("amp.cir"));
    assert_eq!(models[0].cached(&cache).unwrap(), Some(stored.clone()));
    assert!((iq(&run(&directory, ModelChoice::Vendor)) - 1e-3).abs() < 1e-9);
    // Another file in that directory with the wrong content is a mismatch, not a silent pass.
    fs::write(&stored, "* changed\n").unwrap();
    assert!(models[0].cached(&cache).unwrap_err().contains("no file with the sha256"));
}

#[test]
fn check_model_runs_the_bench_set_on_both_models() {
    let directory = project("check", &format!("{MACRO}\n{SPICE}"));
    let design = load_design(&directory.join("board.copper"), false, true).unwrap();
    let part = find_part(&design.electrical.library, "Amp").unwrap();
    let checks = check_part(part, &CheckOptions { supply: Some(10.0), ..CheckOptions::default() }).unwrap();
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    println!("{}", text(&checks));
    assert_eq!((check.macro_state.clone(), check.vendor_state.clone()), (ModelState::Ran, ModelState::Ran));
    assert!(check.passed());
    let figure =
        |name: &str| check.figures.iter().find(|f| f.name.starts_with(name)).unwrap_or_else(|| panic!("{name}"));
    let supply = figure("supply current");
    assert!((supply.macro_value.unwrap() - 2e-3).abs() < 1e-9);
    assert!((supply.vendor_value.unwrap() - 1e-3).abs() < 1e-6);
    assert_eq!(supply.verdict(), Some(true));
    assert!(supply.differs(), "1 mA against 2 mA");
    // The self-written model is 100 dB and 1 MHz, as the macro block declares.
    for name in ["open-loop gain", "gain-bandwidth"] {
        let f = figure(name);
        assert_eq!(f.verdict(), Some(true), "{name}");
        assert!(!f.differs(), "{name}: {:?} {:?}", f.macro_value, f.vendor_value);
    }
    let slew = figure("slew rate");
    assert!((slew.macro_value.unwrap() / 1e6 - 1.0).abs() < 0.1, "{:?}", slew.macro_value);
    let report = text(&checks);
    assert!(report.contains("CHECK MODEL p.Amp (sim opamp) at a 10 V supply (+/-5 V)"), "{report}");
    assert!(report.contains("PASS"), "{report}");
}

#[test]
fn check_model_judges_both_models_against_the_data_sheet() {
    // The macro block adds a common-mode and a supply rejection with corners, and data-sheet
    // entries: the self-written model draws 1 mA, the macro 2 mA.
    let rejection = "cmrr = 80dB; cmrr_corner = 1kHz; psrr = 70dB; psrr_corner = 100Hz;";
    let sheet =
        |entries: &str| MACRO.replace("slew_rate = 1V/us;", &format!("slew_rate = 1V/us; {rejection} {entries}"));
    let check = |name: &str, entries: &str| {
        let directory = project(name, &format!("{}\n{SPICE}", sheet(entries)));
        let design = load_design(&directory.join("board.copper"), false, true).unwrap();
        let part = find_part(&design.electrical.library, "Amp").unwrap();
        check_part(part, &CheckOptions { supply: Some(10.0), ..CheckOptions::default() }).unwrap()
    };
    let checks = check(
        "sheet-pass",
        "datasheet quiescent_current { max = 2.5mA; } datasheet cmrr { min = 75dB; } \
         datasheet unity_gain_frequency { typ = 900kHz; } datasheet slew_rate { min = 0.8V/us; }",
    );
    let report = text(&checks);
    println!("{report}");
    let unit = &checks[0];
    assert!(unit.passed(), "{report}");
    let figure =
        |name: &str| unit.figures.iter().find(|f| f.name.starts_with(name)).unwrap_or_else(|| panic!("{name}"));
    // The rejections at DC (1 mHz, below a hundredth of either corner) and at the corners, 3 dB down.
    let cm = figure("common-mode rejection at 1 mHz");
    assert!((cm.macro_value.unwrap() - 80.0).abs() < 0.01 && cm.verdict() == Some(true), "{report}");
    let cm_corner = figure("common-mode rejection at 1 kHz");
    assert!((cm_corner.macro_value.unwrap() - 76.99).abs() < 0.01, "{report}");
    let ps_corner = figure("supply rejection at 100 Hz");
    assert!((ps_corner.macro_value.unwrap() - 66.99).abs() < 0.01, "{report}");
    let supply = figure("supply current");
    assert_eq!((supply.verdict(), supply.vendor_verdict()), (Some(true), Some(true)));
    // A typical is shown, not judged.
    assert_eq!(figure("unity-gain frequency").verdict(), None);
    assert!(
        report.contains("data sheet") && report.contains("2.5 mA max") && report.contains("typ 900 kHz"),
        "{report}"
    );
    // A maximum the macro model exceeds fails the unit; a minimum only the vendor model misses is printed.
    let checks = check("sheet-macro", "datasheet quiescent_current { max = 1.5mA; }");
    assert!(!checks[0].passed(), "{}", text(&checks));
    let checks = check("sheet-vendor", "datasheet quiescent_current { min = 1.5mA; }");
    let report = text(&checks);
    assert!(checks[0].passed(), "{report}");
    assert!(
        report.contains("note: the vendor model is outside the data sheet: supply current 1 mA (1.5 mA min)"),
        "{report}"
    );
}

#[test]
fn check_model_checks_a_rail_model() {
    let pins = "    pin VIN { number = \"1\"; domains = \"power\"; directions = \"input\"; }
    pin GND { number = \"2\"; domains = \"ground\"; directions = \"input\"; }
    pin EN { number = \"3\"; domains = \"digital\"; directions = \"input\"; }
    pin VOUT { number = \"4\"; domains = \"power\"; directions = \"output\"; }
";
    let sim = "sim regulator { in = VIN; out = VOUT; gnd = GND; en = EN; output_voltage = 3.3V; dropout = 0.25V;
        output_resistance = 0.1ohm; psrr = 60dB; psrr_corner = 1kHz; quiescent_current = 50uA;
        enable_threshold = 1V; soft_start = 100us; voltage_noise = 100nV/rtHz; }";
    let directory = project("rail", MACRO);
    fs::write(directory.join("parts").join("ldo.copper"), format!("part Ldo {{\n{pins}{sim}\n}}\n")).unwrap();
    let design = load_design(&directory.join("board.copper"), false, true).unwrap();
    let part = find_part(&design.electrical.library, "Ldo").unwrap();
    let checks = check_part(part, &CheckOptions::default()).unwrap();
    println!("{}", text(&checks));
    let check = &checks[0];
    assert!(check.passed(), "{}", text(&checks));
    assert!((check.supply - 4.55).abs() < 1e-12, "|V_out| + dropout + 1 V");
    let names: Vec<&str> = check.figures.iter().map(|f| f.name.as_str()).collect();
    for wanted in [
        "quiescent current",
        "output voltage at 1 mA",
        "output resistance at 1 Hz",
        "line rejection at 1 Hz",
        "line rejection at 1 kHz",
        "dropout at 1 mA",
        "output noise at 1 kHz",
        "start-up to 90 %",
    ] {
        assert!(names.contains(&wanted), "{wanted} in {names:?}");
    }
    assert!(check.figures.iter().all(|f| f.verdict() == Some(true)), "{}", text(&checks));
}

#[test]
fn check_model_refuses_what_it_cannot_check() {
    let directory = project("refuse", SPICE);
    let design = load_design(&directory.join("board.copper"), false, true).unwrap();
    let message = find_part(&design.electrical.library, "Nothing").unwrap_err().0;
    assert!(message.starts_with("SIM109") && message.contains("p.Amp"), "{message}");
    let part = find_part(&design.electrical.library, "p.Amp").unwrap();
    let message = check_part(part, &CheckOptions::default()).unwrap_err().0;
    assert!(message.contains("SIM109") && message.contains("only a sim spice block"), "{message}");
    let options = CheckOptions { ties: vec![("NOPE".into(), "vn".into())], ..CheckOptions::default() };
    let message = check_part(part, &options).unwrap_err().0;
    assert!(message.contains("SIM109") && message.contains("NOPE"), "{message}");
}

#[test]
fn figures_are_judged_against_their_tolerance() {
    let figure = |declared: f64, tolerance: Tolerance, value: f64| Figure {
        name: "x".into(),
        unit: "Hz",
        declared: Some(declared),
        tolerance: Some(tolerance),
        macro_value: Some(value),
        vendor_value: None,
        datasheet: None,
    };
    assert_eq!(figure(1e6, Tolerance::Relative(0.05), 1.04e6).verdict(), Some(true));
    assert_eq!(figure(1e6, Tolerance::Relative(0.05), 1.06e6).verdict(), Some(false));
    assert_eq!(figure(100.0, Tolerance::Absolute(1.0), 100.9).verdict(), Some(true));
    assert_eq!(figure(100.0, Tolerance::Absolute(1.0), f64::NAN).verdict(), Some(false));
    let mut unmeasured = figure(1.0, Tolerance::Relative(0.05), 1.0);
    unmeasured.macro_value = None;
    assert_eq!(unmeasured.verdict(), Some(false), "a declared figure the model did not produce fails");
    let mut shown = figure(1.0, Tolerance::Relative(0.05), 1.0);
    shown.declared = None;
    assert_eq!(shown.verdict(), None);
    shown.vendor_value = Some(1.4);
    assert!(shown.differs());
    shown.vendor_value = Some(1.2);
    assert!(!shown.differs());
    // Data-sheet limits judge both models; a typical alone judges neither.
    let limits = |min: Option<f64>, typ: Option<f64>, max: Option<f64>| Limits { min, typ, max, magnitude: false };
    let mut sheet = figure(1e6, Tolerance::Relative(0.05), 1.04e6);
    sheet.datasheet = Some(limits(Some(1.05e6), None, None));
    assert_eq!(sheet.verdict(), Some(false), "within the block's tolerance, below the data sheet's minimum");
    sheet.vendor_value = Some(1.2e6);
    assert_eq!(sheet.vendor_verdict(), Some(true));
    sheet.datasheet = Some(limits(None, None, Some(1.1e6)));
    assert_eq!((sheet.verdict(), sheet.vendor_verdict()), (Some(true), Some(false)));
    sheet.declared = None;
    sheet.datasheet = Some(limits(None, Some(2e6), None));
    assert_eq!((sheet.verdict(), sheet.vendor_verdict()), (None, None));
    sheet.vendor_value = None;
    sheet.datasheet = Some(limits(Some(1.0), None, None));
    assert_eq!(sheet.vendor_verdict(), None, "a figure the vendor model did not produce is not judged");
    // An offset stated as +/-: its magnitude.
    let offset = Limits { magnitude: true, ..limits(None, None, Some(5e-6)) };
    assert_eq!(offset.verdict(Some(-4e-6)), Some(true));
    assert_eq!(offset.verdict(Some(-6e-6)), Some(false));
    assert_eq!(offset.verdict(Some(f64::NAN)), Some(false));
}

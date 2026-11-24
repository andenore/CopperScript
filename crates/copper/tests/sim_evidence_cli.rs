//! Simulation results as evidence for the other checks (plans F11 and I4):
//! `copper check --sim-results` fails a pin whose simulated voltage exceeds its
//! absolute rating and a component beyond its stress rating, naming the bench;
//! `copper power-check --sim-results` lists the supplies' simulated currents
//! with their source and fails a rail above its declared budget; both refuse
//! results made from other sources.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copper(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper")).args(arguments).current_dir(directory).output().expect("copper runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}
";

/// An amplifier whose inputs may go 0.3 V beyond its rails (the usual data-sheet form).
const OPAMP: &str = "part OPAMP {
    category = \"amplifier.opamp\";
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; absolute_min = \"VN-0.3V\"; absolute_max = \"VP+0.3V\"; }
    pin INP { number = \"3\"; domains = \"analog\"; directions = \"input\"; absolute_min = \"VN-0.3V\"; absolute_max = \"VP+0.3V\"; }
    pin VN  { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"6\"; domains = \"analog\"; directions = \"output\"; }
    pin VP  { number = \"7\"; domains = \"power\"; directions = \"input\"; absolute_max = 12V; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB;
        output_swing_from_rail = 0.5V; quiescent_current = 1mA; }
}
";

/// A non-inverting amplifier of gain 2 on +-5 V, driven from J2; ERC alone passes.
const BOARD: &str = "board Amp {
    use library \"tiny\";
    import p \"./parts\";
    component J2: p.HEADER;
    component U1: p.OPAMP;
    component RF: RESISTOR { value = 10kohm; }
    component RG: RESISTOR { value = 10kohm; }
    net IN { J2.1; U1.INP; }
    net OUT { U1.OUT; RF.1; }
    net FB { U1.INN; RF.2; RG.1; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { J2.2; RG.2; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}
";

/// 6 V into the input: 1 V over VP + 0.3 V at the operating point.
const OVERDRIVE: &str = "bench Overdrive for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 6V; }
    analysis op;
    measure out = V(OUT);
}
";

/// A 6 V sine: below VN - 0.3 V in its negative half-wave.
const SWING: &str = "bench Swing for Amp {
    replace J2 with source { out = J2.1; ref = GND; waveform = sine { amplitude = 6V; frequency = 1kHz; }; }
    analysis tran { stop = 1ms; max_step = 10us; }
    measure low = min(V(IN));
}
";

fn project(name: &str) -> PathBuf {
    project_with(
        name,
        &[
            ("board.copper", BOARD),
            ("parts/header.copper", HEADER),
            ("parts/opamp.copper", OPAMP),
            ("benches/overdrive.copper", OVERDRIVE),
            ("benches/swing.copper", SWING),
        ],
    )
}

fn project_with(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-sim-evidence-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for &(path, text) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, text).unwrap();
    }
    directory
}

/// `copper sim` into `build/sim`; returns the results file, relative to the project.
fn simulate(directory: &Path) -> &'static str {
    let output = copper(directory, &["sim", "board.copper", "--out", "build/sim"]);
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    "build/sim/sim-results.json"
}

#[test]
fn a_simulated_pin_beyond_its_absolute_rating_fails_erc_with_the_bench_named() {
    let directory = project("erc");
    // The declared supplies alone are within every rating.
    let plain = copper(&directory, &["check", "board.copper"]);
    assert_eq!(plain.status.code(), Some(0), "{}{}", stdout(&plain), stderr(&plain));
    let results = simulate(&directory);
    let output = copper(&directory, &["check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}{}", stderr(&output));
    let line = |code: &str, subject: &str| {
        text.lines()
            .find(|line| line.contains(&format!("ERROR {code} [{subject}]")))
            .unwrap_or_else(|| panic!("no {code} for {subject} in:\n{text}"))
            .to_owned()
    };
    // The operating point: 6 V on INP against VP = 5 V, 0.7 V beyond VP + 0.3 V.
    let high = line("SIM_PIN_ABSOLUTE_HIGH", "U1.INP");
    assert!(high.contains("bench Overdrive, step 'nominal'"), "{high}");
    assert!(high.contains("V(U1.INP) - V(U1.VP) reaches 1 V at the operating point"), "{high}");
    assert!(high.contains("above the absolute maximum VP+0.3 V by 700 mV"), "{high}");
    assert!(high.contains("(2 of 2 simulated steps exceed it)"), "{high}");
    // The transient run: the negative half-wave (-1 V against VN at its sampled bottom, near 750 us),
    // with where it was reached.
    let low = line("SIM_PIN_ABSOLUTE_LOW", "U1.INP");
    assert!(low.contains("bench Swing, step 'nominal'"), "{low}");
    assert!(low.contains("V(U1.INP) - V(U1.VN) reaches -99"), "{low}");
    assert!(low.contains(" mV at t = 75"), "{low}");
    assert!(low.contains("below the absolute minimum VN-0.3 V by 69"), "{low}");
    assert!(low.contains("(1 of 2 simulated steps exceed it)"), "{low}");
    // The feedback input follows the output (gain 2, half of it): 6 V at the operating point too.
    line("SIM_PIN_ABSOLUTE_HIGH", "U1.INN");
    // VP stays at 5 V, inside its fixed 12 V rating; the output pin declares none and says so.
    assert!(!text.contains("[U1.VP]"), "{text}");
    assert!(text.contains("WARNING SIM_PIN_NOT_CHECKED [Amp]"), "{text}");
    assert!(text.contains("U1: OUT, VN"), "{text}");
    assert!(text.contains("FAILED: 3 errors"), "{text}");
    // The JSON document carries the same findings.
    let json = copper(&directory, &["check", "board.copper", "--json", "--sim-results", results]);
    assert_eq!(json.status.code(), Some(1));
    assert!(stdout(&json).contains("\"code\":\"SIM_PIN_ABSOLUTE_HIGH\""), "{}", stdout(&json));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn results_of_other_sources_are_refused() {
    let directory = project("stale");
    let results = simulate(&directory);
    let check = |directory: &Path| copper(directory, &["check", "board.copper", "--sim-results", results]);
    // A comment does not change the compiled design: the results still apply.
    let board = directory.join("board.copper");
    std::fs::write(&board, format!("// a note\n{BOARD}")).unwrap();
    let output = check(&directory);
    assert!(stdout(&output).contains("SIM_PIN_ABSOLUTE_HIGH"), "{}", stdout(&output));
    assert!(!stdout(&output).contains("SIM_RESULTS_STALE"), "{}", stdout(&output));
    // A changed component value does.
    std::fs::write(&board, BOARD.replace("RG: RESISTOR { value = 10kohm; }", "RG: RESISTOR { value = 4.7kohm; }"))
        .unwrap();
    let output = check(&directory);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("ERROR SIM_RESULTS_STALE"), "{text}");
    assert!(
        text.contains("the board (or a part, package or rating it uses) changed since they were written"),
        "{text}"
    );
    assert!(!text.contains("SIM_PIN_ABSOLUTE"), "stale results are not judged: {text}");
    std::fs::write(&board, BOARD).unwrap();
    // So does a changed bench.
    std::fs::write(directory.join("benches/overdrive.copper"), OVERDRIVE.replace("dc = 6V", "dc = 5.5V")).unwrap();
    let text = stdout(&check(&directory));
    assert!(text.contains("SIM_RESULTS_STALE") && text.contains("a bench source changed"), "{text}");
    std::fs::write(directory.join("benches/overdrive.copper"), OVERDRIVE).unwrap();
    // Results without a source identity (written before it existed) are refused too.
    let path = directory.join(results);
    let document = std::fs::read_to_string(&path).unwrap();
    let start = document.find("  \"source\": {").expect("a source identity");
    let end = start + document[start..].find("},\n").unwrap() + 3;
    let anonymous = directory.join("anonymous.json");
    std::fs::write(&anonymous, format!("{}{}", &document[..start], &document[end..])).unwrap();
    let output = copper(&directory, &["check", "board.copper", "--sim-results", "anonymous.json"]);
    assert!(stdout(&output).contains("they carry no source identity"), "{}{}", stdout(&output), stderr(&output));
    // Something that is not a results document is a usage error.
    std::fs::write(directory.join("other.json"), "{\"schema\": \"copperscript.check/v1\"}").unwrap();
    let output = copper(&directory, &["check", "board.copper", "--sim-results", "other.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("argument --sim-results"), "{}", stderr(&output));
    let missing = copper(&directory, &["power-check", "board.copper", "--sim-results", "missing.json"]);
    assert_eq!(missing.status.code(), Some(2), "{}", stderr(&missing));
    let _ = std::fs::remove_dir_all(&directory);
}

/// A source part whose model is the local SPICE file `models/src.lib`.
const SPICE_SOURCE: &str = "part SRC {
    category = \"power.reference\";
    pin OUT { number = \"1\"; domains = \"analog\"; directions = \"output\"; }
    pin REF { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
    sim spice { file = \"../models/src.lib\"; subckt = \"SRC\"; terminals = OUT, REF; }
}
";

const SPICE_BOARD: &str = "board Ref {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.SRC;
    component R1: RESISTOR { value = 1kohm; }
    net OUT { U1.OUT; R1.1; }
    net GND { U1.REF; R1.2; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}
";

fn spice_source_model(volts: &str) -> String {
    format!(".SUBCKT SRC OUT REF\nV1 OUT REF {volts}\n.ENDS\n")
}

#[test]
fn results_are_stale_once_a_local_model_file_changes() {
    let model = spice_source_model("2");
    let directory = project_with(
        "model",
        &[
            ("board.copper", SPICE_BOARD),
            ("parts/src.copper", SPICE_SOURCE),
            ("models/src.lib", &model),
            ("benches/op.copper", "bench Op for Ref {\n    analysis op;\n    measure out = V(OUT);\n}\n"),
        ],
    );
    let results = simulate(&directory);
    let document = std::fs::read_to_string(directory.join(results)).unwrap();
    assert!(document.contains("\"file\": \"models/src.lib\""), "the model file is fingerprinted: {document}");
    let check = || {
        let output = copper(&directory, &["check", "board.copper", "--sim-results", results]);
        format!("{}{}", stdout(&output), stderr(&output))
    };
    assert!(!check().contains("SIM_RESULTS_STALE"), "{}", check());
    // The model now produces 3 V: the 2 V results are not evidence for it.
    std::fs::write(directory.join("models/src.lib"), spice_source_model("3")).unwrap();
    let text = check();
    assert!(text.contains("ERROR SIM_RESULTS_STALE"), "{text}");
    assert!(text.contains("the SPICE model file models/src.lib changed since they were written"), "{text}");
    // Results written before the models were fingerprinted cannot vouch for one.
    std::fs::write(directory.join("models/src.lib"), &model).unwrap();
    let start = document.find(",\n    \"models\": [").expect("source.models");
    let end = start + document[start..].find("]").unwrap() + 1;
    std::fs::write(directory.join("old.json"), format!("{}{}", &document[..start], &document[end..])).unwrap();
    let output = copper(&directory, &["check", "board.copper", "--sim-results", "old.json"]);
    let text = stdout(&output);
    assert!(text.contains("SIM_RESULTS_STALE") && text.contains("carry no digests of the SPICE model files"), "{text}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_power_check_lists_simulated_rail_currents_with_their_source() {
    let directory = project("power");
    let results = simulate(&directory);
    let output = copper(&directory, &["power-check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}{}", stderr(&output));
    assert!(text.contains("OK: Amp passed power-state analysis"), "{text}");
    // No supply declares a budget: the figures are listed, not compared, and that is said.
    assert!(text.contains("WARNING SIM_RAIL_NOT_CHECKED [Amp]"), "{text}");
    assert!(text.contains("simulated rails without a budget: POS, NEG"), "{text}");
    assert!(text.contains("  budget: none declared"), "{text}");
    // The quiescent 1 mA plus the 0.6 mA the output (12 V) drives into RF + RG, from the positive rail;
    // the negative rail sinks the quiescent current alone.
    let rails: Vec<&str> = text.lines().collect();
    let after = |header: &str| {
        let at = rails.iter().position(|line| *line == header).unwrap_or_else(|| panic!("no {header} in:\n{text}"));
        rails[at + 1..].iter().take_while(|line| line.starts_with("  ")).copied().collect::<Vec<_>>()
    };
    let positive = after("RAIL POS (5 V, net VP):");
    assert!(
        positive.iter().any(|line| line.contains("1.6 mA (operating point, 1 step): bench Overdrive, supply POS")),
        "{text}"
    );
    assert!(
        positive.iter().any(|line| line.contains("(step 'nominal'; transient, 1 step): bench Swing, supply POS")),
        "{text}"
    );
    let negative = after("RAIL NEG (-5 V, net VN):");
    assert!(negative.iter().any(|line| line.contains("-1 mA (operating point, 1 step): bench Overdrive")), "{text}");
    assert!(text.contains("Not simulated (no bench drove their nets): GROUND"), "{text}");
    // Without --sim-results the check is unchanged.
    let plain = stdout(&copper(&directory, &["power-check", "board.copper"]));
    assert!(!plain.contains("RAIL"), "{plain}");
    let _ = std::fs::remove_dir_all(&directory);
}

/// A resistor part with a power rating.
const RATED_RESISTOR: &str = "part RES_0402 {
    category = \"passive.resistor\";
    rated_power = 1mW;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}
";

/// A regulator with a rated output (2 mA) and a current limit (1.2 mA).
const LDO: &str = "part LDO {
    category = \"power.regulator.ldo\";
    pin IN { number = \"1\"; domains = \"power\"; directions = \"input\"; }
    pin GND { number = \"2\"; domains = \"ground\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"power\"; directions = \"output\"; current_max = 2mA; }
    sim regulator { in = IN; out = OUT; gnd = GND; output_voltage = 5V; dropout = 0.2V;
        output_resistance = 0.1ohm; output_current_limit = 1.2mA; }
}
";

/// The amplifier with its positive rail from the regulator and budgets on both rails; RF is rated 1 mW.
const RATED_BOARD: &str = "board Amp {
    use library \"tiny\";
    import p \"./parts\";
    component J2: p.HEADER;
    component U1: p.OPAMP;
    component U2: p.LDO;
    component RF: p.RES_0402 { value = 10kohm; }
    component RG: RESISTOR { value = 10kohm; }
    net IN { J2.1; U1.INP; }
    net OUT { U1.OUT; RF.1; }
    net FB { U1.INN; RF.2; RG.1; }
    net VIN { U2.IN; }
    net VP { U1.VP; U2.OUT; }
    net VN { U1.VN; }
    net GND { J2.2; RG.2; U2.GND; }
    supply VIN { voltage = 7V; external = true; }
    supply POS { voltage = 5V; net = VP; source = U2.OUT; current_max = 1.5mA; }
    supply NEG { voltage = -5V; net = VN; external = true; current_max = 5mA; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}
";

/// 6 V in, with an ideal 5 V rail standing in for the regulator: 1.6 mA from POS.
const RATED_OVERDRIVE: &str = "bench Overdrive for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 6V; }
    ignore U2;
    replace POS with source { out = VP; ref = GND; dc = 5V; }
    analysis op;
    measure out = V(OUT);
}
";

/// The regulator itself drives POS; the bias point of this ac bench is recorded once for both steps
/// (1 V in: 1.1 mA from the regulator).
const RATED_GAIN: &str = "bench Gain for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = 1V; ac = A_IN; }
    param A_IN = 1V;
    step A_IN in 1V, 2V;
    analysis ac { from = 10Hz; to = 1MHz; points_per_decade = 2; }
    measure gain = mag(at(V(OUT), 1kHz));
}
";

/// The regulator itself drives POS with its own limit active: a 25 kohm load added on VP. At 1 V in the
/// amplifier draws I_q + V(OUT) / 20 kohm = 1.1 mA whatever its rail, so the 1.2 mA limit leaves
/// 0.1 mA for the load and VP = 0.1 mA x 25 kohm = 2.5 V; at 6 V in it draws 1.6 mA, more than the
/// limit at any voltage, and the rail collapses onto GND (-(1.6 - 1.2) mA x 0.1 ohm = -40 uV).
const RATED_LIMITED: &str = "bench Limited for Amp {
    replace J2 with source { out = J2.1; ref = GND; dc = V_DRIVE; }
    param V_DRIVE = 1V;
    step V_DRIVE in 1V, 6V;
    component RLOAD: RESISTOR { value = 25kohm; }
    net VP { RLOAD.1; }
    net GND { RLOAD.2; }
    analysis op;
    measure vp = V(VP);
    measure out = V(OUT);
}
";

fn rated_project(name: &str) -> PathBuf {
    project_with(
        name,
        &[
            ("board.copper", RATED_BOARD),
            ("parts/header.copper", HEADER),
            ("parts/opamp.copper", OPAMP),
            ("parts/res.copper", RATED_RESISTOR),
            ("parts/ldo.copper", LDO),
            ("benches/overdrive.copper", RATED_OVERDRIVE),
            ("benches/gain.copper", RATED_GAIN),
        ],
    )
}

fn error_line(text: &str, code: &str, subject: &str) -> String {
    text.lines()
        .find(|line| line.contains(&format!("ERROR {code} [{subject}]")))
        .unwrap_or_else(|| panic!("no {code} for {subject} in:\n{text}"))
        .to_owned()
}

#[test]
fn a_rail_above_its_budget_fails_the_power_check_with_the_bench_named() {
    let directory = rated_project("budget");
    let results = simulate(&directory);
    let output = copper(&directory, &["power-check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}{}", stderr(&output));
    assert!(!text.contains("OK: Amp passed"), "{text}");
    // 1.6 mA at the operating point of Overdrive: above the supply's 1.5 mA and the regulator's 1.2 mA
    // limit; the larger excess is named.
    let mean = error_line(&text, "SIM_RAIL_MEAN_HIGH", "POS");
    assert!(
        mean.contains("rail POS: the mean current 1.6 mA (board level, bench Overdrive: step 'nominal')"),
        "{mean}"
    );
    assert!(
        mean.contains("above its budget 1.2 mA (output_current_limit of U2's sim regulator block) by 400 uA"),
        "{mean}"
    );
    // current_max bounds the peak too.
    error_line(&text, "SIM_RAIL_PEAK_HIGH", "POS");
    // The budgets and their sources are listed, the regulator's rated output among them.
    assert!(
        text.contains(
            "  budget: 1.5 mA mean and peak (current_max of supply POS); 2 mA mean (current_max of U2.OUT, its \
             source); 1.2 mA mean and peak (output_current_limit of U2's sim regulator block)"
        ),
        "{text}"
    );
    // Where the regulator drives the rail itself, its output current is the rail's: the bias point of the ac
    // bench, the same at both steps.
    assert!(
        text.contains("1.1 mA (bias point of the ac analysis, 2 steps): bench Gain, supply POS (driven by U2.OUT)"),
        "{text}"
    );
    // NEG stays inside its 5 mA; VIN has no budget, which is said.
    assert!(!text.contains("[NEG]"), "{text}");
    assert!(text.contains("simulated rails without a budget: VIN"), "{text}");
    assert!(text.contains("covers 1 of 1 loads on VP"), "{text}");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_component_beyond_its_rating_fails_erc_with_the_bench_named() {
    let directory = rated_project("stress");
    let results = simulate(&directory);
    let output = copper(&directory, &["check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}{}", stderr(&output));
    // RF carries 6 V and 0.6 mA at the operating point: 3.6 mW against 1 mW.
    let power = error_line(&text, "SIM_STRESS_POWER_HIGH", "RF");
    assert!(
        power.contains("bench Overdrive, step 'nominal': the power into RF is 3.6 mW at the operating point"),
        "{power}"
    );
    assert!(power.contains("above its rated_power 1 mW by 2.6 mW (1 of 3 simulated steps exceed it)"), "{power}");
    // The regulator's output is within its rating; RG declares none and is counted, not assumed.
    assert!(!text.contains("[U2.OUT]"), "{text}");
    let summary = text.lines().find(|line| line.contains("WARNING SIM_STRESS_NOT_CHECKED")).expect("a summary");
    assert!(
        summary.contains(
            "2 ratings of simulated components checked (highest: RF power at 360 % of 1 mW; U2.OUT current at 55 % \
             of 2 mA)"
        ),
        "{summary}"
    );
    assert!(
        summary
            .contains("1 simulated component was not checked, their parts declare no rating: 1 resistor (rated_power)"),
        "{summary}"
    );
    // The bias point of the ac bench is judged like an operating point: 1 V on INP is inside VP + 0.3 V.
    let high = error_line(&text, "SIM_PIN_ABSOLUTE_HIGH", "U1.INP");
    assert!(high.contains("(1 of 3 simulated steps exceed it)"), "{high}");
    let _ = std::fs::remove_dir_all(&directory);
}

/// The values of measure `name` of bench `bench` per step, from a results file.
fn measure_values(results: &Path, bench: &str, name: &str) -> Vec<f64> {
    let document = copper_json::parse(&std::fs::read_to_string(results).expect("the results")).expect("json");
    let benches = document.get("benches").and_then(copper_json::Value::as_array).expect("benches");
    let bench =
        benches.iter().find(|b| b.get("name").and_then(copper_json::Value::as_str) == Some(bench)).expect("bench");
    let measures = bench.get("measures").and_then(copper_json::Value::as_array).expect("measures");
    let measure =
        measures.iter().find(|m| m.get("name").and_then(copper_json::Value::as_str) == Some(name)).expect("measure");
    let steps = measure.get("steps").and_then(copper_json::Value::as_array).expect("steps");
    steps
        .iter()
        .map(|step| match step.get("value") {
            Some(copper_json::Value::Float(v)) => *v,
            other => panic!("value {other:?}"),
        })
        .collect()
}

#[test]
fn a_regulator_in_its_own_current_limit_solves_and_its_rail_is_checked() {
    let directory = project_with(
        "limited",
        &[
            ("board.copper", RATED_BOARD),
            ("parts/header.copper", HEADER),
            ("parts/opamp.copper", OPAMP),
            ("parts/res.copper", RATED_RESISTOR),
            ("parts/ldo.copper", LDO),
            ("benches/limited.copper", RATED_LIMITED),
        ],
    );
    let results = simulate(&directory);
    let vp = measure_values(&directory.join(results), "Limited", "vp");
    let out = measure_values(&directory.join(results), "Limited", "out");
    // The amplifier's own closed loop: V(OUT) = 2 V A / (1 + A / 2) x (1 / 2) with A = 1e5, and
    // VP = (1.2 mA - 1 mA - V(OUT) / 20 kohm) x 25 kohm.
    let a = 1e5;
    let want_out = 1.0 / (1.0 / a + 0.5);
    assert!((out[0] - want_out).abs() < 1e-9, "{out:?}");
    let want_vp = (1.2e-3 - 1e-3 - want_out / 20e3) * 25e3;
    assert!((vp[0] - want_vp).abs() < 1e-6, "{vp:?} vs {want_vp}");
    // Collapsed: the amplifier's 1.6 mA (and the load's -1.6 nA) against the limit, through R_out.
    let want_collapsed = -(1e-3 + 6.0 / (1.0 / a + 0.5) / 20e3 - 1.2e-3) * 0.1;
    assert!((vp[1] - want_collapsed).abs() < 1e-8, "{vp:?} vs {want_collapsed}");
    // The power check sees the regulator's own current: within its limit at 1 V, above it once collapsed.
    let output = copper(&directory, &["power-check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(1), "{text}{}", stderr(&output));
    assert!(
        text.contains("1.2 mA to 1.6 mA (operating point, 2 steps): bench Limited, supply POS (driven by U2.OUT)"),
        "{text}"
    );
    let mean = error_line(&text, "SIM_RAIL_MEAN_HIGH", "POS");
    assert!(mean.contains("bench Limited: step 'V_DRIVE = 6 V'"), "{mean}");
    let _ = std::fs::remove_dir_all(&directory);
}

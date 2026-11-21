//! Simulation results as evidence for the other checks (plan F11): `copper
//! check --sim-results` fails a pin whose simulated voltage exceeds its
//! absolute rating and names the bench, `copper power-check --sim-results`
//! lists the supplies' simulated currents with their source, and both refuse
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
    let directory = std::env::temp_dir().join(format!("copper-sim-evidence-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, text) in [
        ("board.copper", BOARD),
        ("parts/header.copper", HEADER),
        ("parts/opamp.copper", OPAMP),
        ("benches/overdrive.copper", OVERDRIVE),
        ("benches/swing.copper", SWING),
    ] {
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

#[test]
fn the_power_check_lists_simulated_rail_currents_with_their_source() {
    let directory = project("power");
    let results = simulate(&directory);
    let output = copper(&directory, &["power-check", "board.copper", "--sim-results", results]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}{}", stderr(&output));
    assert!(text.contains("OK: Amp passed power-state analysis"), "{text}");
    assert!(text.contains("no part or supply declares a rail current, so none is compared"), "{text}");
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

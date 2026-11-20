//! Supply current and supply rejection through benches: rail currents of a
//! built-in follower in op, dc, tran and ac runs against closed forms, the
//! PSRR referred to the bench's rails, an unsupplied rail (`SIM102`), and
//! `I()` of the rails of a SPICE subcircuit.

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-supply-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

fn run(board: &Path, bench: &str) -> BenchRun {
    let design = compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    let mut runs = simulate(board, &design, Some(bench)).unwrap_or_else(|failure| panic!("{failure}"));
    println!("{}", copper_simulate::results::text_table(&runs));
    runs.remove(0)
}

fn measure(run: &BenchRun, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[0].measures[k].value
}

const BOARD: &str = "board Follower {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.AMP;
    component R_LOAD: RESISTOR { value = 1kohm; }
    component J_IN: p.HEADER;
    net VIN { J_IN.1; U1.INP; }
    net OUT { U1.OUT; U1.INN; R_LOAD.1; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { J_IN.2; R_LOAD.2; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

const AMP: &str = "part AMP {
    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
    sim opamp {
        inp = INP; inn = INN; out = OUT; vp = VP; vn = VN;
        gbw = 10MHz; open_loop_gain = 100dB;
        quiescent_current = 2mA; psrr = 80dB; psrr_corner = 1kHz;
    }
}";

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

const BENCHES: [&str; 8] = ["bench Op for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 1V; }
    analysis op;
    measure pos = I(POS);
    measure neg = I(NEG);
    measure vp_pin = I(U1.VP);
    measure vn_pin = I(U1.VN);
    measure out_pin = I(U1.OUT);
}", "bench Sweep for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 0V; }
    analysis dc { sweep = J_IN; from = -2V; to = 2V; step = 0.5V; }
    measure pos_sourcing = at(I(POS), 1V);
    measure neg_sourcing = at(I(NEG), 1V);
    measure pos_sinking = at(I(POS), -1V);
    measure neg_sinking = at(I(NEG), -1V);
    plot I(POS), I(NEG);
}", "bench Sine for Follower {
    replace J_IN with source { out = VIN; ref = GND; waveform = sine { amplitude = 1V; frequency = 1kHz; }; }
    analysis tran { stop = 3ms; start = 1ms; max_step = 2us; }
    measure pos_mean = mean(I(POS));
    measure neg_mean = mean(I(NEG));
    measure pos_peak = max(I(POS));
}", "bench Rejection for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 0V; }
    replace POS with source { out = VP; ref = GND; dc = 5V; ac = 1V; }
    analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }
    measure at_100hz = db(at(V(OUT), 100Hz));
    measure at_10khz = db(at(V(OUT), 10kHz));
    measure rail_current = mag(at(I(POS), 1kHz));
}", "bench SupplySweep for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 0V; }
    analysis dc { sweep = POS; from = 4V; to = 6V; step = 0.5V; }
    measure at_nominal = at(V(OUT), 5V);
    measure slope = (at(V(OUT), 6V) - at(V(OUT), 4V)) / 2V;
}", "bench Unsupplied for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 1V; }
    ignore POS;
    analysis op;
    measure out = V(OUT);
}", "bench Replaced for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 1V; }
    replace U1 with opamp {
        inp = VIN; inn = OUT; out = OUT; vp = VP; vn = VN;
        gbw = 10MHz; open_loop_gain = 100dB; quiescent_current = 1mA; output_resistance = 10ohm;
    }
    analysis op;
    measure vp_role = I(U1.vp);
    measure vn_role = I(U1.vn);
}", "bench Floating for Follower {
    replace J_IN with source { out = VIN; ref = GND; dc = 1V; }
    replace U1 with opamp { inp = VIN; inn = OUT; out = OUT; vp = VP; vn = VN; gbw = 10MHz; open_loop_gain = 100dB; quiescent_current = 1mA; }
    ignore POS;
    analysis op;
    measure out = V(OUT);
}"];

/// The follower project, one bench per file, in a directory of its own per test.
fn follower(name: &str) -> PathBuf {
    let names: Vec<String> = (0..BENCHES.len()).map(|k| format!("benches/b{k}.copper")).collect();
    let mut files = vec![("board.copper", BOARD), ("parts/amp.copper", AMP), ("parts/header.copper", HEADER)];
    files.extend(names.iter().map(String::as_str).zip(BENCHES));
    project(name, &files)
}

const A0: f64 = 1e5;
const IQ: f64 = 2e-3;

#[test]
fn rail_currents_in_op_dc_and_tran() {
    let board = follower("follower1");
    // Op: V_out = A0 / (1 + A0) into 1 kohm; VP delivers I_q plus the load current, VN takes back I_q.
    let op = run(&board, "Op");
    assert_eq!((op.error.as_deref(), op.status()), (None, Status::Pass));
    let i_out = A0 / (1.0 + A0) / 1e3;
    assert!((measure(&op, "pos") - (IQ + i_out)).abs() < 1e-12, "{}", measure(&op, "pos"));
    assert!((measure(&op, "neg") + IQ).abs() < 1e-12, "{}", measure(&op, "neg"));
    assert!((measure(&op, "vp_pin") - (IQ + i_out)).abs() < 1e-12);
    assert!((measure(&op, "vn_pin") + IQ).abs() < 1e-12);
    // The pins balance: what flows in at VP leaves at VN and OUT.
    let sum = measure(&op, "vp_pin") + measure(&op, "vn_pin") + measure(&op, "out_pin");
    assert!(sum.abs() < 1e-12, "{sum}");

    // Dc sweep: the output current comes from the rail on its side.
    let dc = run(&board, "Sweep");
    assert_eq!(dc.error, None);
    assert!((measure(&dc, "pos_sourcing") - (IQ + i_out)).abs() < 1e-12);
    assert!((measure(&dc, "neg_sourcing") + IQ).abs() < 1e-12);
    assert!((measure(&dc, "pos_sinking") - IQ).abs() < 1e-12);
    assert!((measure(&dc, "neg_sinking") + (IQ + i_out)).abs() < 1e-12);

    // Tran, 1 V sine into 1 kohm: each rail carries one half-wave, mean I_peak / pi above I_q.
    let tran = run(&board, "Sine");
    assert_eq!(tran.error, None);
    let half_wave = i_out / std::f64::consts::PI;
    let (pos, neg) = (measure(&tran, "pos_mean"), measure(&tran, "neg_mean"));
    println!("mean rail currents {pos:.6e} / {neg:.6e} A, closed form {:.6e} A", IQ + half_wave);
    assert!((pos - (IQ + half_wave)).abs() < 1e-3 * half_wave, "{pos}");
    assert!((neg + IQ + half_wave).abs() < 1e-3 * half_wave, "{neg}");
    assert!((measure(&tran, "pos_peak") - (IQ + i_out)).abs() < 1e-3 * i_out);
}

#[test]
fn supply_rejection_against_the_bench_rails() {
    let board = follower("follower2");
    // AC tone on VP: V_out = A / (1 + A) / PSRR(f), PSRR(f) = 1e4 / (1 + j f / 1 kHz).
    let ac = run(&board, "Rejection");
    assert_eq!(ac.error, None);
    let want = |f: f64| -80.0 + 10.0 * (1.0 + (f / 1e3).powi(2)).log10();
    assert!((measure(&ac, "at_100hz") - want(100.0)).abs() < 1e-3, "{}", measure(&ac, "at_100hz"));
    assert!((measure(&ac, "at_10khz") - want(1e4)).abs() < 1e-3, "{}", measure(&ac, "at_10khz"));
    // The follower's output carries no load current at the 0 V input, so the rail tone draws almost nothing.
    assert!(measure(&ac, "rail_current") < 1e-9);
    // Dc sweep of the supply: zero at the nominal 10 V span, 1 / PSRR per volt around it.
    let dc = run(&board, "SupplySweep");
    assert_eq!(dc.error, None);
    assert!(measure(&dc, "at_nominal").abs() < 1e-12, "{}", measure(&dc, "at_nominal"));
    assert!((measure(&dc, "slope") / (A0 / (1.0 + A0) * 1e-4) - 1.0).abs() < 1e-9, "{}", measure(&dc, "slope"));
}

#[test]
fn an_unsupplied_rail_and_replaced_models() {
    let board = follower("follower3");
    // The part's PSRR cannot be referred to a rail that nothing sets.
    let unsupplied = run(&board, "Unsupplied");
    assert_eq!(unsupplied.status(), Status::Error);
    let error = unsupplied.error.as_deref().unwrap();
    assert!(error.contains("SIM102: U1: psrr refers the supply rejection to the supply span"), "{error}");
    assert!(error.contains("its vp pin (net 'VP') is set by no supply, DC source or rail model"), "{error}");
    // Without a PSRR the supply current still needs the rail.
    let floating = run(&board, "Floating");
    assert_eq!(floating.status(), Status::Error);
    let error = floating.error.as_deref().unwrap();
    assert!(
        error.contains(
            "SIM102: U1: its model draws supply current through its vp pin, but net 'VP' (U1) has no DC path"
        ),
        "{error}"
    );
    // A replacement with a quiescent current: I(X.vp) and I(X.vn) read its rails.
    let replaced = run(&board, "Replaced");
    assert_eq!(replaced.error, None);
    let a = A0 * 1e3 / (1e3 + 10.0);
    let i_out = a / (1.0 + a) / 1e3;
    assert!((measure(&replaced, "vp_role") - (1e-3 + i_out)).abs() < 1e-12, "{}", measure(&replaced, "vp_role"));
    assert!((measure(&replaced, "vn_role") + 1e-3).abs() < 1e-12);
}

#[test]
fn rails_of_a_spice_subcircuit() {
    // The repository's own behavioural subcircuit (spice_amplifier/models/): 1.5 mA quiescent plus
    // the output current from the rail that sources it, the same convention as the built-in model.
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation/spice_amplifier");
    let read = |file: &str| std::fs::read_to_string(example.join(file)).unwrap();
    let bench = "bench Rails for SpiceAmplifier {
    replace J_IN with source { out = J_IN.1; ref = GND; dc = 1V; }
    ignore J_OUT, J_PWR;
    step V_IN in 1V, -1V;
    analysis op;
    measure pos = I(POS);
    measure neg = I(NEG);
    measure output = V(OUT);
}";
    let bench = bench.replace("dc = 1V;", "dc = V_IN;").replace("    step V_IN", "    param V_IN = 1V;\n    step V_IN");
    let board = project(
        "spice",
        &[
            ("board.copper", &read("board.copper")),
            ("parts/opamp.copper", &read("parts/opamp.copper")),
            ("parts/connector.copper", &read("parts/connector.copper")),
            ("parts/power_header.copper", &read("parts/power_header.copper")),
            ("models/behavioural_opamp.lib", &read("models/behavioural_opamp.lib")),
            ("benches/rails.copper", &bench),
        ],
    );
    let rails = run(&board, "Rails");
    assert_eq!(rails.error, None);
    let k = |name: &str| rails.measures.iter().position(|m| m.name == name).unwrap();
    for (step, sign) in [(0, 1.0), (1, -1.0)] {
        let at = |name: &str| rails.steps[step].measures[k(name)].value;
        // R_LOAD || (R_F + R_G) = 1.667 kohm takes V_out / 1.667 kohm.
        let i_out = at("output") / (10e3 / 6.0);
        assert!((i_out * sign - 6.0e-3).abs() < 1e-5, "{i_out}");
        let (pos, neg) = (at("pos"), at("neg"));
        if sign > 0.0 {
            assert!((pos - (1.5e-3 + i_out)).abs() < 1e-9 && (neg + 1.5e-3).abs() < 1e-9, "{pos} {neg}");
        } else {
            assert!((pos - 1.5e-3).abs() < 1e-9 && (neg + 1.5e-3 - i_out).abs() < 1e-9, "{pos} {neg}");
        }
    }
}

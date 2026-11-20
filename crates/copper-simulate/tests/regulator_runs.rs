//! The rail models through benches, against closed forms: a part with a
//! `sim regulator` block in a dc sweep of its input (regulation, dropout, line
//! regulation referred to the bench's nominal input), at an operating point
//! (load regulation, input current), in ac (output impedance, line
//! rejection) and in tran (enable and soft-start); an averaged converter's
//! input power; a regulator supplying an amplifier that draws supply current
//! and rejects its rail; a reference's output noise; `SIM104`.

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-regulator-{name}-{}", std::process::id()));
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
    measure_at(run, 0, name)
}

fn measure_at(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

/// A 3.3 V LDO from a 5 V input into a load, an amplifier on a 5 V rail
/// from a second LDO, and a reference.
const BOARD: &str = "board Rails {
    use library \"tiny\";
    import p \"./parts\";
    param R_LOAD_VALUE = 100ohm;
    component U1: p.LDO;
    component R_LOAD: RESISTOR { value = R_LOAD_VALUE; }
    component C_OUT: CAPACITOR { value = 10uF; }
    net VIN { U1.IN; U1.EN; }
    net VOUT { U1.OUT; R_LOAD.1; C_OUT.1; }
    net GND { U1.GND; R_LOAD.2; C_OUT.2; }
    supply INPUT { voltage = 5V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

// dropout 0.2 V, R_out 0.1 ohm with a 10 kHz corner, PSRR 60 dB with a 1 kHz
// corner, 0.5 A limit, 1 ms soft-start, EN at 1.2 V, 50 uA quiescent.
const LDO: &str = "part LDO {
    category = \"power.regulator.ldo\";
    pin IN { number = \"1\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"2\"; domains = \"power\"; directions = \"output\"; }
    pin GND { number = \"3\"; domains = \"ground\"; directions = \"input\"; }
    pin EN { number = \"4\"; domains = \"digital\"; directions = \"input\"; }
    sim regulator {
        in = IN; out = OUT; gnd = GND; en = EN;
        output_voltage = 3.3V; dropout = 0.2V;
        output_resistance = 0.1ohm; output_impedance_corner = 10kHz;
        psrr = 60dB; psrr_corner = 1kHz;
        output_current_limit = 0.5A;
        soft_start = 1ms; enable_threshold = 1.2V;
        quiescent_current = 50uA;
    }
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
        gbw = 10MHz; open_loop_gain = 140dB;
        quiescent_current = 10mA; psrr = 60dB;
        supply 3.3V { quiescent_current = 8mA; }
        supply 10V { quiescent_current = 10mA; }
    }
}";

const BENCHES: [&str; 9] = ["bench Line for Rails {
    replace INPUT with source { out = VIN; ref = GND; dc = 5V; }
    analysis dc { sweep = INPUT; from = 0V; to = 6V; step = 0.25V; }
    measure off = at(V(VOUT), 1V);
    measure dropout_at_3v = at(V(VOUT), 3V);
    measure nominal = at(V(VOUT), 5V);
    measure high = at(V(VOUT), 6V);
    measure input_at_5v = at(I(U1.IN), 5V);
}", "bench Load for Rails {
    step R_LOAD_VALUE in 100ohm, 10ohm, 1ohm;
    analysis op;
    measure out = V(VOUT);
    measure input = I(INPUT);
    measure in_pin = I(U1.IN);
    measure out_pin = I(U1.OUT);
    measure gnd_pin = I(U1.GND);
}", "bench Impedance for Rails {
    model PROBE: source { out = VOUT; ref = GND; output = current; ac = 1A; }
    ignore C_OUT;
    analysis ac { from = 100Hz; to = 1MHz; points_per_decade = 10; }
    measure z_100hz = mag(at(V(VOUT), 100Hz));
    measure z_10khz = mag(at(V(VOUT), 10kHz));
    measure z_100khz = mag(at(V(VOUT), 100kHz));
}", "bench Rejection for Rails {
    replace INPUT with source { out = VIN; ref = GND; dc = 5V; ac = 1V; }
    ignore C_OUT;
    analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }
    measure at_100hz = db(at(V(VOUT), 100Hz));
    measure at_10khz = db(at(V(VOUT), 10kHz));
}", "bench Start for Rails {
    replace INPUT with source { out = VIN; ref = GND; dc = 5V; waveform = step { final = 5V; at = 0.5ms; }; }
    analysis tran { stop = 3ms; max_step = 10us; }
    measure before = at(V(VOUT), 0.4ms);
    measure half = at(V(VOUT), 1ms);
    measure ninety = cross(V(VOUT), 2.97V);
    measure final = at(V(VOUT), 3ms);
    measure charging = at(I(INPUT), 1ms);
}", "bench Converter for Rails {
    replace U1 with regulator {
        in = VIN; out = VOUT; gnd = GND;
        output_voltage = -5.24V; efficiency = 85%; quiescent_current = 1mA;
    }
    analysis op;
    measure out = V(VOUT);
    measure input = I(INPUT);
}", "bench Amplifier for Rails {
    // A second rail, 5 V behind 1 ohm, for a follower with its own -5 V rail.
    model REG5: regulator { in = VIN; out = VP; gnd = GND; output_voltage = 5V; dropout = 0.3V; output_resistance = 1ohm; }
    replace INPUT with source { out = VIN; ref = GND; dc = 7V; }
    model NEG: source { out = VN; ref = GND; dc = -5V; }
    model SIG: source { out = SIG_IN; ref = GND; dc = 1V; }
    component U2: p.AMP;
    component R_AMP: RESISTOR { value = 100ohm; }
    component C_VP: CAPACITOR { value = 1uF; }
    component C_VN: CAPACITOR { value = 1uF; }
    component C_SIG: CAPACITOR { value = 1nF; }
    net VP { U2.VP; C_VP.1; }
    net VN { U2.VN; C_VN.1; }
    net SIG_IN { U2.INP; C_SIG.1; }
    net AMP_OUT { U2.OUT; U2.INN; R_AMP.1; }
    net GND { R_AMP.2; C_VP.2; C_VN.2; C_SIG.2; }
    analysis op;
    measure vp = V(VP);
    measure amp_out = V(AMP_OUT);
    measure rail = I(REG5.out);
}", "bench Unreferred for Rails {
    replace INPUT with source { out = VIN; ref = GND; dc = 5V; }
    component R_FEED: RESISTOR { value = 1ohm; }
    component C_FEED: CAPACITOR { value = 1uF; }
    net FED { R_FEED.2; C_FEED.1; }
    net VIN { R_FEED.1; }
    net GND { C_FEED.2; }
    model REG: regulator { in = FED; out = VOUT; gnd = GND; output_voltage = 3.3V; psrr = 60dB; }
    ignore U1;
    analysis op;
    measure out = V(VOUT);
}", "bench Noise for Rails {
    model REF: reference { in = VIN; out = VREF; gnd = GND; output_voltage = 2.5V; voltage_noise = 100nV/rtHz; voltage_noise_corner = 10Hz; }
    component C_REF: CAPACITOR { value = 1nF; }
    component C_REF2: CAPACITOR { value = 1nF; }
    net VREF { C_REF.1; C_REF2.1; }
    net GND { C_REF.2; C_REF2.2; }
    analysis noise { from = 1Hz; to = 10kHz; points_per_decade = 10; output = V(VREF); }
    measure at_1hz = at(noise(), 1Hz);
    measure at_1khz = at(noise(), 1kHz);
}"];

fn rails(name: &str) -> PathBuf {
    let names: Vec<String> = (0..BENCHES.len()).map(|k| format!("benches/b{k}.copper")).collect();
    let mut files = vec![("board.copper", BOARD), ("parts/ldo.copper", LDO), ("parts/amp.copper", AMP)];
    files.extend(names.iter().map(String::as_str).zip(BENCHES));
    project(name, &files)
}

#[test]
fn regulation_dropout_and_load() {
    let board = rails("line");
    // DC sweep of the input (EN tied to it, 1.2 V): off below the threshold, in
    // dropout V_in - 0.2 V, regulating 3.3 V + (V_in - 5 V) / 1000 (60 dB referred
    // to the bench's 5 V); all behind 0.1 ohm into 100 ohm.
    let line = run(&board, "Line");
    assert_eq!(line.error, None);
    let divider = 100.0 / 100.1;
    assert!(measure(&line, "off").abs() < 1e-12, "{}", measure(&line, "off"));
    assert!((measure(&line, "dropout_at_3v") - 2.8 * divider).abs() < 1e-9);
    assert!((measure(&line, "nominal") - 3.3 * divider).abs() < 1e-9);
    assert!((measure(&line, "high") - (3.3 + 1e-3) * divider).abs() < 1e-9, "{}", measure(&line, "high"));
    let i_load = 3.3 * divider / 100.0;
    assert!((measure(&line, "input_at_5v") - (i_load + 50e-6)).abs() < 1e-12);

    // Load regulation: V_out = 3.3 RL / (RL + 0.1); at 1 ohm the 0.5 A limit holds.
    let load = run(&board, "Load");
    assert_eq!(load.error, None);
    for (step, rl) in [(0, 100.0), (1, 10.0)] {
        let i = 3.3 / (rl + 0.1);
        assert!((measure_at(&load, step, "out") - 3.3 * rl / (rl + 0.1)).abs() < 1e-12);
        assert!((measure_at(&load, step, "input") - (i + 50e-6)).abs() < 1e-12);
        // in, out and gnd balance; gnd returns the quiescent current.
        let sum: f64 = ["in_pin", "out_pin", "gnd_pin"].iter().map(|name| measure_at(&load, step, name)).sum();
        assert!(sum.abs() < 1e-12, "{sum}");
        assert!((measure_at(&load, step, "gnd_pin") + 50e-6).abs() < 1e-12);
    }
    assert!((measure_at(&load, 2, "out") - 0.5).abs() < 1e-12, "{}", measure_at(&load, 2, "out"));
    assert!((measure_at(&load, 2, "input") - 0.50005).abs() < 1e-12);
}

#[test]
fn impedance_and_rejection_against_frequency() {
    let board = rails("ac");
    // |Z| = 0.1 ohm sqrt(1 + (f / 10 kHz)^2) in parallel with the 100 ohm load.
    let ac = run(&board, "Impedance");
    assert_eq!(ac.error, None);
    for (name, f) in [("z_100hz", 100.0), ("z_10khz", 1e4), ("z_100khz", 1e5)] {
        let z = num_complex(0.1, 0.1 * f / 1e4);
        let parallel = div(mul(z, (100.0, 0.0)), (z.0 + 100.0, z.1));
        let want = (parallel.0 * parallel.0 + parallel.1 * parallel.1).sqrt();
        assert!((measure(&ac, name) / want - 1.0).abs() < 1e-9, "{name}: {} vs {want}", measure(&ac, name));
    }
    // Line rejection: -60 dB + 10 log10(1 + (f / 1 kHz)^2), and the divider into the load.
    let rejection = run(&board, "Rejection");
    assert_eq!(rejection.error, None);
    let divider_db = 20.0 * (100.0f64 / 100.1).log10();
    for (name, f) in [("at_100hz", 100.0), ("at_10khz", 1e4)] {
        let want = -60.0 + 10.0 * (1.0 + (f / 1e3f64).powi(2)).log10() + divider_db;
        assert!((measure(&rejection, name) - want).abs() < 1e-3, "{name}: {}", measure(&rejection, name));
    }
}

fn num_complex(re: f64, im: f64) -> (f64, f64) {
    (re, im)
}
fn mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}
fn div(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let d = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

#[test]
fn soft_start_in_a_transient_run() {
    // The input (and EN) steps to 5 V at 0.5 ms: the command ramps 3.3 V in 1 ms,
    // 3.3 V (t - 0.5 ms) / 1 ms. Behind 0.1 ohm into 100 ohm || 10 uF the output
    // follows it less 0.1 ohm times the load and charging current (3.3 mV for the
    // 33 mA into 10 uF at 3.3 V/ms), and the input delivers V / RL + C dV/dt + I_q:
    // at 1 ms 1.65 V / 100 ohm + 33 mA + 50 uA = 49.5 mA; 2.97 V is passed at
    // 0.5 ms + (2.97 V 100.1 / 100 + 3.3 mV) / 3.3 V/ms = 1.402 ms.
    let start = run(&rails("tran"), "Start");
    assert_eq!(start.error, None);
    assert!(measure(&start, "before").abs() < 1e-9, "{}", measure(&start, "before"));
    let half = measure(&start, "half");
    assert!((half - 1.65 * 100.0 / 100.1).abs() < 5e-3, "{half}");
    let ninety = measure(&start, "ninety");
    assert!((ninety - 1.402e-3).abs() < 5e-6, "{ninety}");
    assert!((measure(&start, "final") - 3.3 * 100.0 / 100.1).abs() < 1e-4);
    let charging = measure(&start, "charging");
    assert!((charging - 49.5e-3).abs() < 0.2e-3, "{charging}");
}

#[test]
fn converter_power_and_a_supplied_amplifier() {
    let board = rails("power");
    // An inverter to -5.24 V at 85 % from 5 V into 100 ohm: P_in = P_out / 0.85 + 5 V 1 mA.
    let converter = run(&board, "Converter");
    assert_eq!(converter.error, None);
    assert!((measure(&converter, "out") + 5.24).abs() < 1e-12);
    let p_in = 5.0 * measure(&converter, "input");
    assert!((p_in - (5.24f64.powi(2) / 100.0 / 0.85 + 5e-3)).abs() < 1e-12, "{p_in}");

    // A 5 V regulator behind 1 ohm supplies VP of a follower (I_q 10 mA from its
    // 10 V parameter set, chosen from the regulator's set point; PSRR 60 dB
    // referred to the 10 V span), 1 V into 100 ohm:
    //   VP = 5 - 1 ohm (10 mA + V_out / 100 ohm) = 4.98 V,
    //   V_out = 1 V + (VP - 5 V) / 1000 = 1 V - 20 uV.
    let amplifier = run(&board, "Amplifier");
    assert_eq!(amplifier.error, None);
    let (mut vp, mut out) = (5.0, 1.0);
    let a = 1e7 / (1.0 + 1e7);
    for _ in 0..30 {
        vp = 5.0 - (10e-3 + out / 100.0);
        out = a * (1.0 + (vp - 5.0) / 1000.0);
    }
    assert!((measure(&amplifier, "vp") - vp).abs() < 1e-12, "{}", measure(&amplifier, "vp"));
    assert!((measure(&amplifier, "amp_out") - out).abs() < 1e-12, "{}", measure(&amplifier, "amp_out"));
    assert!((vp - 4.98).abs() < 1e-6 && (out - (1.0 - 20e-6)).abs() < 1e-6);
    assert!((measure(&amplifier, "rail") + (10e-3 + out / 100.0)).abs() < 1e-12);
    let model = amplifier.models.iter().find(|model| model.name == "U2").unwrap();
    assert!(model.model.contains("parameter set for a 10 V supply"), "{}", model.model);
}

#[test]
fn reference_noise_and_an_unreferred_rejection() {
    let board = rails("noise");
    // e_n^2 (1 + 10 Hz / f): 316 nV/rtHz at 1 Hz, 100.5 nV/rtHz at 1 kHz.
    let noise = run(&board, "Noise");
    assert_eq!(noise.error, None);
    assert!((measure(&noise, "at_1hz") / (100e-9 * 11f64.sqrt()) - 1.0).abs() < 1e-6, "{}", measure(&noise, "at_1hz"));
    assert!((measure(&noise, "at_1khz") / (100e-9 * 1.01f64.sqrt()) - 1.0).abs() < 1e-6);
    // A PSRR with an input that no supply, DC source or rail model sets.
    let unreferred = run(&board, "Unreferred");
    assert_eq!(unreferred.status(), Status::Error);
    let error = unreferred.error.as_deref().unwrap();
    assert!(error.contains("SIM104: REG: psrr refers the line rejection to the nominal input"), "{error}");
    assert!(error.contains("its in pin (net 'FED')"), "{error}");
}

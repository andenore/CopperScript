//! Lowering, expression evaluation, stepping and the failure paths of a
//! bench run, on small projects written to a temporary directory.

use std::path::{Path, PathBuf};

use copper_lang::bench::load_benches;
use copper_lang::compiler::{Design, compile_design_file};
use copper_sim::ElementKind;
use copper_simulate::lower::{self, Params, base};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

/// A project directory with `files` (relative path, source).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

fn compile(board: &Path) -> Design {
    compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"))
}

fn run(board: &Path) -> Vec<BenchRun> {
    simulate(board, &compile(board), None).unwrap_or_else(|failure| panic!("{failure}"))
}

fn value(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance * b.abs().max(1e-30)
}

const DIVIDER: &str = "board Divider {
    use library \"tiny\";
    component R1: RESISTOR { value = 1kohm; }
    component R2: RESISTOR { value = 3kohm; }
    net VIN { R1.1; }
    net VOUT { R1.2; R2.1; }
    net GND { R2.2; }
    supply IN { voltage = 2V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

#[test]
fn lowering_and_the_sign_of_pin_currents() {
    let bench = "bench Op for Divider {
        analysis op;
        measure v_out = V(VOUT);
        measure v_r1 = V(VIN, VOUT);
        measure i_r1 = I(R1);
        measure i_r1_second = I(R1.2);
        measure i_r2 = I(R2.1);
    }";
    let board = project("divider", &[("board.copper", DIVIDER), ("benches/op.copper", bench)]);
    let design = compile(&board);
    // The lowered circuit: R1 from VIN (its first pin) to VOUT, the supply as a DC source to ground.
    let benches = load_benches(&board, &design).unwrap();
    let view = copper_lang::elaborate::elaborate(&design.electrical).unwrap();
    let wiring = lower::wiring(&view, &benches[0]).unwrap();
    let params: Params = Params::new();
    let lowered = lower::lower(&design, &benches[0], &wiring, &params).unwrap();
    let circuit = &lowered.circuit;
    assert_eq!(circuit.ground_name(), "GND");
    let r1 = circuit.element(circuit.find_element("R1").unwrap());
    let ElementKind::Resistor { a, b, ohms } = *r1.kind() else { panic!("R1 is a resistor") };
    assert_eq!((circuit.node_name(a), circuit.node_name(b), ohms), ("VIN", "VOUT", 1e3));
    let supply = circuit.element(circuit.find_element("supply IN").unwrap());
    assert!(matches!(supply.kind(), ElementKind::VoltageSource { value, .. } if value.dc == 2.0));
    assert_eq!(
        lowered.models.iter().map(|m| (m.name.as_str(), m.fidelity)).collect::<Vec<_>>(),
        [("R1", "ideal"), ("R2", "ideal"), ("IN", "ideal"), ("GROUND", "ground")]
    );

    let runs = run(&board);
    let run = &runs[0];
    assert_eq!(run.error, None);
    assert!(close(value(run, 0, "v_out"), 1.5, 1e-12));
    assert!(close(value(run, 0, "v_r1"), 0.5, 1e-12));
    // I(X) is the current into X's first pin: 0.5 mA flows from VIN into R1.1 and out of R1.2.
    assert!(close(value(run, 0, "i_r1"), 0.5e-3, 1e-12), "{}", value(run, 0, "i_r1"));
    assert!(close(value(run, 0, "i_r1_second"), -0.5e-3, 1e-12));
    assert!(close(value(run, 0, "i_r2"), 0.5e-3, 1e-12));
    assert_eq!(run.status(), Status::Pass, "no limits, nothing fails");
}

const RC: &str = "board RC {
    use library \"tiny\";
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    net VIN { R1.1; }
    net VOUT { R1.2; C1.1; }
    net GND { C1.2; }
    supply INPUT { voltage = 1V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn rc_bench(body: &str) -> String {
    format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; ac = 1V; }}\n \
         analysis ac {{ from = 10Hz; to = 100kHz; points_per_decade = 50; }}\n let h = V(VOUT) / V(VIN);\n{body}\n}}"
    )
}

#[test]
fn expression_functions_over_an_ac_result() {
    let body = "step R_VALUE in 1kohm, 2kohm;
        measure re_1k = re(at(h, 1kHz));
        measure im_1k = im(at(h, 1kHz));
        measure mag_1k = mag(at(h, 1kHz));
        measure abs_1k = abs(at(h, 1kHz));
        measure phase_1k = phase(at(h, 1kHz));
        measure db_1k = db(at(h, 1kHz));
        measure peak = max(db(h)) { max = 0dB; }
        measure floor = min(phase(h));
        measure average = mean(mag(h));
        measure tau = R_VALUE * 100nF;
        measure i_c = mag(at(I(C1), 1kHz));
        measure corner_gain = db(at(h, 1.5915kHz)) { min = -3.1dB; max = -2.9dB; }
        plot db(h), phase(h);";
    let board = project("rc-expressions", &[("board.copper", RC), ("benches/b.copper", &rc_bench(body))]);
    let runs = run(&board);
    let run = &runs[0];
    assert_eq!(run.error, None);
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[1].label, "R_VALUE = 2 kohm");
    for (step, r) in [(0, 1e3), (1, 2e3)] {
        let x = 2.0 * std::f64::consts::PI * 1e3 * r * 100e-9;
        let d = 1.0 + x * x;
        assert!(close(value(run, step, "re_1k"), 1.0 / d, 1e-9), "{step}");
        assert!(close(value(run, step, "im_1k"), -x / d, 1e-9), "{step}");
        assert!(close(value(run, step, "mag_1k"), d.sqrt().recip(), 1e-9));
        assert_eq!(value(run, step, "abs_1k"), value(run, step, "mag_1k"));
        assert!(close(value(run, step, "phase_1k"), -x.atan().to_degrees(), 1e-9));
        assert!(close(value(run, step, "db_1k"), -10.0 * d.log10(), 1e-9));
        // The params of the step feed expressions: tau = R C.
        assert!(close(value(run, step, "tau"), r * 100e-9, 1e-12));
        // Current into C1's first pin: V(VOUT) j w C.
        let w = 2.0 * std::f64::consts::PI * 1e3;
        assert!(close(value(run, step, "i_c"), w * 100e-9 / d.sqrt(), 1e-9));
        let axis = &run.steps[step].axis;
        let last = axis[axis.len() - 1];
        let floor = -(2.0 * std::f64::consts::PI * last * r * 100e-9).atan().to_degrees();
        assert!(close(value(run, step, "floor"), floor, 1e-9));
        let mean: f64 = axis
            .iter()
            .map(|f| (1.0 + (2.0 * std::f64::consts::PI * f * r * 100e-9).powi(2)).sqrt().recip())
            .sum::<f64>()
            / axis.len() as f64;
        assert!(close(value(run, step, "average"), mean, 1e-9));
        assert!(value(run, step, "peak") < 0.0 && value(run, step, "peak") > -0.01);
    }
    // The 2 kohm step moves the corner to 796 Hz: corner_gain fails there, and that is the worst step.
    let k = run.measures.iter().position(|m| m.name == "corner_gain").unwrap();
    assert_eq!(run.steps[0].measures[k].pass, Some(true));
    assert_eq!(run.steps[1].measures[k].pass, Some(false));
    assert_eq!(run.measures[k].worst, Some(1));
    assert_eq!(run.status(), Status::Fail);
    assert_eq!(run.counts(), (2, 1));
    // The window of corner_gain is drawn on the db(h) panel, the peak's limit as a line.
    let svg = copper_simulate::report::figure(run).unwrap();
    assert!(svg.contains("class=\"limit-window\" data-frequency-hz=\"1591.5\""), "window at 1.5915 kHz");
    assert!(svg.contains("class=\"limit\""), "max(db(h)) draws its limit line");
    assert_eq!(svg.matches("class=\"curve\"").count(), 4, "two panels, two steps");
    let table = copper_simulate::results::text_table(&runs);
    assert!(table.contains("corner_gain") && table.contains("FAIL") && table.contains("R_VALUE = 2 kohm"), "{table}");
    let json = copper_simulate::results::results_json(&runs, "RC");
    let parsed = copper_json::parse(&json).unwrap();
    assert_eq!(parsed.get("schema").and_then(|v| v.as_str()), Some("copperscript-sim/v1"));
    assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("fail"));
    let csv = copper_simulate::results::plots_csv(run);
    assert!(csv.starts_with("step,label,frequency_hz,db(h) [dB],phase(h) [deg]\n"), "{csv}");
    assert_eq!(csv.lines().count(), 1 + 2 * run.steps[0].axis.len());
}

#[test]
fn a_floating_net_is_reported_by_name_with_its_component_pins() {
    let bench = "bench Floating for RC {
        replace INPUT with source { out = VIN; ref = GND; dc = 1V; }
        ignore R1;
        analysis op;
        measure v = V(VOUT);
    }";
    let board = project("floating", &[("board.copper", RC), ("benches/f.copper", bench)]);
    let runs = run(&board);
    let run = &runs[0];
    assert_eq!(run.status(), Status::Error);
    let error = run.error.as_deref().unwrap();
    assert!(
        error.contains("net 'VOUT' (C1.1) has no DC path to the ground net 'GND' (capacitors are open at DC)"),
        "{error}"
    );
    assert!(error.starts_with("bench Floating: the circuit cannot be solved (operating point)"), "{error}");
    assert!(copper_simulate::failed(&runs));
    assert!(copper_simulate::results::text_table(&runs).contains("ERROR: bench Floating"));
    // The page says so instead of plotting.
    let page = copper_simulate::report::html(run, "RC");
    assert!(page.contains("ERROR: bench Floating"), "{page}");
}

#[test]
fn a_loop_of_supplies_names_them() {
    let board_source =
        DIVIDER.replace("supply GROUND", "supply IN2 { voltage = 3V; net = VIN; external = true; }\n    supply GROUND");
    let bench = "bench Loop for Divider { analysis op; measure v = V(VOUT); }";
    let board = project("loop", &[("board.copper", &board_source), ("benches/l.copper", bench)]);
    let runs = run(&board);
    let error = runs[0].error.as_deref().unwrap();
    assert!(error.contains("a loop of voltage sources through supply IN, supply IN2"), "{error}");
}

const DEVICE: &str = "device DUAL_DIE {
    pad A_INP { domains = \"analog\"; directions = \"input\"; }
    pad A_INN { domains = \"analog\"; directions = \"input\"; }
    pad A_OUT { domains = \"analog\"; directions = \"output\"; }
    pad B_INP { domains = \"analog\"; directions = \"input\"; }
    pad B_INN { domains = \"analog\"; directions = \"input\"; }
    pad B_OUT { domains = \"analog\"; directions = \"output\"; }
    pad VP { domains = \"power\"; directions = \"input\"; }
    pad VN { domains = \"power\"; directions = \"input\"; }
    unit A: std.opamp { INP = A_INP; INN = A_INN; OUT = A_OUT; }
    unit B: std.opamp { INP = B_INP; INN = B_INN; OUT = B_OUT; }
    unit POWER: std.power { shared = true; VP = VP; VN = VN; }
}";

const DUAL: &str = "part DUAL {
    category = \"amplifier.opamp\";
    device = DUAL_DIE;
    pin OUTA { number = \"1\"; bond = A_OUT; }
    pin INA_N { number = \"2\"; bond = A_INN; }
    pin INA_P { number = \"3\"; bond = A_INP; }
    pin VN { number = \"4\"; bond = VN; }
    pin INB_P { number = \"5\"; bond = B_INP; }
    pin INB_N { number = \"6\"; bond = B_INN; }
    pin OUTB { number = \"7\"; bond = B_OUT; }
    pin VP { number = \"8\"; bond = VP; }
    sim opamp A { inp = INA_P; inn = INA_N; out = OUTA; vp = VP; vn = VN; gbw = 10MHz; open_loop_gain = 100dB; }
    sim opamp B { inp = INB_P; inn = INB_N; out = OUTB; vp = VP; vn = VN; gbw = 10MHz; open_loop_gain = 100dB; }
}";

const FOLLOWER: &str = "board Follower {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.DUAL;
    component R1: RESISTOR { value = 1kohm; }
    net VIN { U1.A.INP; }
    net OUT { U1.A.OUT; U1.A.INN; R1.1; }
    net GND { R1.2; U1.B.INP; }
    net BOUT { U1.B.OUT; U1.B.INN; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    supply SIGNAL { voltage = 1V; net = VIN; external = true; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

const FOLLOWER_BENCH: &str = "bench Gain for Follower {
    replace SIGNAL with source { out = VIN; ref = GND; ac = 1V; }
    analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 10; }
    let g = V(OUT) / V(VIN);
    measure gain = mag(at(g, 1kHz)) { min = 0.999; }
    measure i_out_pin = re(at(I(U1.OUTA), 1kHz));
    measure i_load = re(at(I(R1), 1kHz));
    measure i_supply = mag(at(I(U1.VP), 1kHz));
}";

#[test]
fn unit_terminals_of_a_device_backed_part_are_its_package_pins() {
    let board = project(
        "device",
        &[
            ("board.copper", FOLLOWER),
            ("parts/die.copper", DEVICE),
            ("parts/dual.copper", DUAL),
            ("benches/gain.copper", FOLLOWER_BENCH),
        ],
    );
    let runs = run(&board);
    let run = &runs[0];
    assert_eq!(run.error, None);
    // A0 = 1e5, GBW 10 MHz: a follower's gain at 1 kHz is A/(1+A) to 1e-5.
    let gain = value(run, 0, "gain");
    assert!((gain - 1.0).abs() < 2e-5, "{gain}");
    // Current into the output pin is minus what the amplifier drives into the 1 kohm load.
    let into_load = value(run, 0, "i_load");
    assert!(close(into_load, 1e-3, 1e-4), "{into_load}");
    assert!(close(value(run, 0, "i_out_pin"), -into_load, 1e-9));
    assert_eq!(value(run, 0, "i_supply"), 0.0, "supply pins carry no current in the linear model");
    let used: Vec<(&str, &str, &str)> =
        run.models.iter().map(|m| (m.name.as_str(), m.fidelity, m.model.as_str())).collect();
    assert_eq!(used[0], ("U1", "macro", "opamp"), "one name per model kind");
}

#[test]
fn a_unit_terminal_bonded_to_two_pins_is_refused() {
    let doubled = DUAL.replace(
        "    pin VP { number = \"8\"; bond = VP; }",
        "    pin VP { number = \"8\"; bond = VP; }\n    pin OUTA2 { number = \"9\"; bond = A_OUT; }",
    );
    let doubled = doubled.replace(
        "open_loop_gain = 100dB; }\n    sim opamp B",
        "open_loop_gain = 100dB; unmodelled = OUTA2; }\n    sim opamp B",
    );
    let board = project(
        "device-doubled",
        &[
            ("board.copper", FOLLOWER),
            ("parts/die.copper", DEVICE),
            ("parts/dual.copper", &doubled),
            ("benches/gain.copper", FOLLOWER_BENCH),
        ],
    );
    let design = match compile_design_file(&board, false, true) {
        Ok(design) => design,
        // The compiler may already refuse an ambiguous terminal; then nothing reaches the simulator.
        Err(failure) => {
            println!("compile refused it: {failure}");
            return;
        }
    };
    let runs = simulate(&board, &design, None).unwrap();
    let error = runs[0].error.as_deref().unwrap_or_else(|| panic!("expected an error: {:?}", runs[0].steps.len()));
    assert!(error.contains("U1.A.OUT on net 'OUT'") && error.contains("bonded to 2 package pins"), "{error}");
}

#[test]
fn stepped_values_reach_the_circuit() {
    let body = "step R_VALUE in 1kohm, 10kohm; measure g = mag(at(h, 1kHz));";
    let board = project("rc-steps", &[("board.copper", RC), ("benches/b.copper", &rc_bench(body))]);
    let design = compile(&board);
    let benches = load_benches(&board, &design).unwrap();
    let view = copper_lang::elaborate::elaborate(&design.electrical).unwrap();
    let wiring = lower::wiring(&view, &benches[0]).unwrap();
    let mut params: Params = benches[0].params.iter().map(|(name, p)| (name.clone(), base(&p.value))).collect();
    params.insert("R_VALUE".into(), 10e3);
    let lowered = lower::lower(&design, &benches[0], &wiring, &params).unwrap();
    let r1 = lowered.circuit.element(lowered.circuit.find_element("R1").unwrap());
    assert!(matches!(r1.kind(), ElementKind::Resistor { ohms, .. } if *ohms == 10e3));
    let combinations = copper_simulate::run::combinations(&benches[0]);
    assert_eq!(combinations.len(), 2);
}

#[test]
fn derived_params_zipped_steps_and_a_reference_run() {
    // R2 = R_VALUE * SCALE from VOUT to ground: at 10 Hz the RC is a divider
    // R2 / (R1 + R2) = SCALE / (1 + SCALE). The zipped step moves R_VALUE and SCALE
    // together; the reference re-runs the bench at SCALE = 100 %.
    let body = "param SCALE = 100%;
    param R_SCALED = R_VALUE * SCALE;
    component R2: RESISTOR { value = R_SCALED; }
    net VOUT { R2.1; }
    net GND { R2.2; }
    step R_VALUE, SCALE in (1kohm, 100%), (2kohm, 300%);
    reference EVEN { SCALE = 100%; }
    measure divider = mag(at(h, 10Hz));
    measure r2 = R_SCALED;
    measure relative = mag(at(h / reference(h, EVEN), 10Hz));";
    let board = project("derived", &[("board.copper", RC), ("benches/b.copper", &rc_bench(body))]);
    let runs = run(&board);
    let run = &runs[0];
    assert_eq!(run.status(), Status::Pass, "{:?}", run.error);
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.steps[1].label, "R_VALUE = 2 kohm, SCALE = 300 %");
    assert!(close(value(run, 0, "r2"), 1e3, 1e-12));
    assert!(close(value(run, 1, "r2"), 6e3, 1e-12), "the derived param follows both stepped ones");
    assert!(close(run.steps[1].params["R_SCALED"], 6e3, 1e-12));
    // 10 Hz is far below the corner (C1 sees R1 || R2 >= 500 ohm): divider to 1e-4.
    assert!(close(value(run, 0, "divider"), 0.5, 1e-4));
    assert!(close(value(run, 1, "divider"), 0.75, 1e-4));
    // The reference keeps the step's R_VALUE and re-derives R_SCALED at SCALE = 100 %.
    assert!(close(value(run, 0, "relative"), 1.0, 1e-9));
    assert!(close(value(run, 1, "relative"), 1.5, 1e-4));
}

const RECEIVER: &str = "part Rx {
    category = \"amplifier.differential_receiver\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin REF { number = \"3\"; domains = \"analog\"; directions = \"input\"; }
    pin FB { number = \"4\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"5\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"6\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"7\"; domains = \"power\"; directions = \"input\"; }
    sim diffamp {
        inp = INP; inn = INN; ref = REF; fb = FB; out = OUT; vp = VP; vn = VN;
        gbw = 270MHz; open_loop_gain = 74dB; cmrr = 90dB; cmrr_corner = 630kHz;
    }
}";

const RECEIVER_BOARD: &str = "board Receiver {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.Rx;
    component R_IN: RESISTOR { value = 1kohm; }
    net IN { R_IN.1; U1.INP; U1.INN; }
    net OUT { U1.OUT; U1.FB; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { R_IN.2; U1.REF; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

#[test]
fn a_differential_receiver_rejects_common_mode_by_its_cmrr() {
    // Both inputs on one node driven with 1 V: V_out = A (V_cm / CMRR(f) - V_out),
    // so V_out / V_cm = (A / (1 + A)) / CMRR(f), CMRR(f) = 90 dB / (1 + j f / 630 kHz).
    let bench = "bench Cm for Receiver {
        replace R_IN with source { out = IN; ref = GND; ac = 1V; }
        analysis ac { from = 1kHz; to = 10MHz; points_per_decade = 10; }
        measure cm_1khz = mag(at(V(OUT) / V(IN), 1kHz));
        measure cm_630khz = mag(at(V(OUT) / V(IN), 630kHz));
    }";
    let board = project(
        "receiver",
        &[("board.copper", RECEIVER_BOARD), ("parts/rx.copper", RECEIVER), ("benches/cm.copper", bench)],
    );
    let design = compile(&board);
    let benches = load_benches(&board, &design).unwrap();
    let view = copper_lang::elaborate::elaborate(&design.electrical).unwrap();
    let wiring = lower::wiring(&view, &benches[0]).unwrap();
    let lowered = lower::lower(&design, &benches[0], &wiring, &Params::new()).unwrap();
    for element in ["U1.gain", "U1.gain_fb", "U1.cmp", "U1.cmn", "U1.rcm", "U1.lcm", "U1.cmg"] {
        assert!(lowered.circuit.find_element(element).is_some(), "{element}");
    }
    assert_eq!(lowered.models.iter().find(|m| m.name == "U1").unwrap().model, "diffamp");
    let runs = run(&board);
    let cmrr = 10f64.powf(-90.0 / 20.0);
    let a0 = 10f64.powf(74.0 / 20.0);
    assert!(close(value(&runs[0], 0, "cm_1khz"), cmrr * a0 / (1.0 + a0), 1e-3));
    // At the corner the rejection is 3 dB worse (the loop gain is still large there).
    assert!(close(value(&runs[0], 0, "cm_630khz"), cmrr * 2f64.sqrt(), 2e-2));
}

/// An RC low-pass as a module (a param and a port named apart from its net), and a board that uses it.
const RC_MODULE: &str = "module RcStage {
    use library \"tiny\";
    port IN: input;
    port OUT: output;
    port GND: power_in;
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    net VIN { port.IN; R1.1; }
    net VOUT { port.OUT; R1.2; C1.1; }
    net GND { port.GND; C1.2; }
}";

const RC_TOP: &str = "board Top {
    use library \"tiny\";
    import p \"./parts\";
    module F: p.RcStage;
    component R_LOAD: RESISTOR { value = 10ohm; }
    net A { F.IN; }
    net B { F.OUT; R_LOAD.1; }
    net GND { F.GND; R_LOAD.2; }
    supply GROUND { voltage = 0V; net = GND; }
}";

#[test]
fn a_module_bench_runs_the_module_alone_with_its_port_conditions() {
    // The board loads the module's output with 10 ohm; a bench of the module does not see it.
    let bench = "bench Corner for RcStage {
        supply GROUND { voltage = 0V; net = GND; }
        model SRC: source { out = IN; ref = GND; ac = 1V; }
        analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 20; }
        step R_VALUE in 1kohm, 2kohm;
        measure gain_at_1592 = mag(at(V(OUT), 1591.55Hz));
        measure input = mag(at(V(VIN), 1591.55Hz));
        measure i_r1 = mag(at(I(R1), 1591.55Hz));
        plot mag(V(OUT));
    }";
    let board = project(
        "module-run",
        &[("board.copper", RC_TOP), ("parts/rc.copper", RC_MODULE), ("benches/corner.copper", bench)],
    );
    let runs = run(&board);
    assert_eq!(runs.len(), 1);
    let corner = &runs[0];
    assert!(corner.error.is_none(), "{:?}", corner.error);
    assert_eq!((corner.board.as_str(), corner.module.as_deref()), ("Top", Some("p.RcStage")));
    assert_eq!(corner.target(), "module RcStage of Top");
    // 1 kohm and 100 nF: the corner is 1591.55 Hz, 1/sqrt(2) there; twice R halves the corner.
    let expected = |r: f64| 1.0 / (1.0 + (2.0 * std::f64::consts::PI * 1591.55 * r * 100e-9).powi(2)).sqrt();
    assert!(close(value(corner, 0, "gain_at_1592"), expected(1e3), 1e-3), "{}", value(corner, 0, "gain_at_1592"));
    assert!(close(value(corner, 1, "gain_at_1592"), expected(2e3), 1e-3), "{}", value(corner, 1, "gain_at_1592"));
    // The ideal source drives the port's net, which is VIN inside the module.
    assert!(close(value(corner, 0, "input"), 1.0, 1e-12));
    let i = 1.0 / (1e3_f64.powi(2) + (1.0 / (2.0 * std::f64::consts::PI * 1591.55 * 100e-9)).powi(2)).sqrt();
    assert!(close(value(corner, 0, "i_r1"), i, 1e-3), "{}", value(corner, 0, "i_r1"));
    let models: Vec<(&str, &str, &str, bool)> =
        corner.models.iter().map(|model| (model.name.as_str(), model.kind, model.fidelity, model.bench)).collect();
    assert_eq!(
        models,
        [
            ("R1", "component", "ideal", false),
            ("C1", "component", "ideal", false),
            ("SRC", "model", "macro", true),
            ("GROUND", "supply", "ground", true)
        ]
    );
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("BENCH Corner (module RcStage of Top): ac 10 Hz to 100 kHz"), "{text}");
    let json = copper_simulate::results::results_json(&runs, "Top");
    assert!(json.contains("\"module\": \"p.RcStage\""), "{json}");
}

#[test]
fn a_module_bench_gives_the_numbers_of_the_same_circuit_drawn_flat() {
    // The module with a 3 kohm load and a DC bias added by its bench ...
    let module_bench = "bench Loaded for RcStage {
        supply GROUND { voltage = 0V; net = GND; }
        supply BIAS { voltage = 1V; net = IN; }
        component R2: RESISTOR { value = 3kohm; }
        net OUT { R2.1; }
        net GND { R2.2; }
        analysis op;
        measure v_out = V(OUT);
    }";
    let module_board = project(
        "module-flat-a",
        &[("board.copper", RC_TOP), ("parts/rc.copper", RC_MODULE), ("benches/loaded.copper", module_bench)],
    );
    // ... against the same circuit drawn as a flat board.
    let flat = "board Flat {
        use library \"tiny\";
        component R1: RESISTOR { value = 1kohm; }
        component C1: CAPACITOR { value = 100nF; }
        component R2: RESISTOR { value = 3kohm; }
        net VIN { R1.1; }
        net VOUT { R1.2; C1.1; R2.1; }
        net GND { C1.2; R2.2; }
        supply BIAS { voltage = 1V; net = VIN; external = true; }
        supply GROUND { voltage = 0V; net = GND; external = true; }
    }";
    let flat_bench = "bench Loaded for Flat {
        analysis op;
        measure v_out = V(VOUT);
    }";
    let flat_board = project("module-flat-b", &[("board.copper", flat), ("benches/loaded.copper", flat_bench)]);
    let a = value(&run(&module_board)[0], 0, "v_out");
    let b = value(&run(&flat_board)[0], 0, "v_out");
    assert!(close(a, 0.75, 1e-12) && a == b, "{a} {b}");
}

#[test]
fn a_reference_on_another_analysis_axis_is_an_error() {
    // Both runs have two frequencies, but the reference's are 1 kHz and 10 kHz:
    // pairing them by index would divide H(1 kHz) by the reference's H(10 kHz).
    let bench = "bench B for RC {
    param F_LO = 100Hz;
    param F_HI = 1kHz;
    replace INPUT with source { out = VIN; ref = GND; ac = 1V; }
    analysis ac { from = F_LO; to = F_HI; points_per_decade = 1; }
    reference WIDE { F_LO = 1kHz; F_HI = 10kHz; }
    let h = V(VOUT) / V(VIN);
    measure ratio = mag(at(h / reference(h, WIDE), 1kHz));
}";
    let board = project("reference-axis", &[("board.copper", RC), ("benches/b.copper", bench)]);
    let runs = run(&board);
    let error = runs[0].error.clone().unwrap_or_else(|| panic!("no error; ratio {}", value(&runs[0], 0, "ratio")));
    assert!(error.contains("SIM121") && error.contains("reference WIDE"), "{error}");
    assert!(error.contains("1 kHz") && error.contains("100 Hz"), "{error}");
    // A reference that leaves the axis alone still runs.
    let same = bench.replace("reference WIDE { F_LO = 1kHz; F_HI = 10kHz; }", "reference WIDE { F_LO = 100Hz; }");
    let board = project("reference-same-axis", &[("board.copper", RC), ("benches/b.copper", &same)]);
    let runs = run(&board);
    assert_eq!(runs[0].error, None);
    assert!(close(value(&runs[0], 0, "ratio"), 1.0, 1e-12));
}

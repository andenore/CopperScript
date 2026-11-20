//! The two simulation examples through the library entry point, against
//! closed forms: the RC corner and the Howland pump's load current.

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_sim::Complex;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation").join(path)
}

fn run_example(path: &str) -> Vec<BenchRun> {
    let board = example(path);
    let design = compile_design_file(&board, false, true).unwrap_or_else(|failure| panic!("{path}: {failure}"));
    simulate(&board, &design, None).unwrap_or_else(|failure| panic!("{path}: {failure}"))
}

fn measure(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

fn plot(run: &BenchRun, step: usize, expression: &str) -> Vec<f64> {
    let k = run.plots.iter().position(|plot| plot.expression == expression).unwrap_or_else(|| panic!("{expression}"));
    run.steps[step].plots[k].clone()
}

#[test]
fn rc_corner_is_minus_three_db_and_minus_45_degrees() {
    let runs = run_example("rc_filter.copper");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    println!("{}", copper_simulate::results::text_table(&runs));
    assert_eq!(run.error, None);
    assert_eq!(run.status(), Status::Pass);
    let gain = measure(run, 0, "gain_at_corner");
    let phase = measure(run, 0, "phase_at_corner");
    // 1.5915 kHz is the corner 1/(2 pi 1 kohm 100 nF) = 1591.549 Hz to 3e-5.
    let x = 1591.5 / (1.0 / (2.0 * std::f64::consts::PI * 1e3 * 100e-9));
    let exact_gain = -10.0 * (1.0 + x * x).log10();
    let exact_phase = -x.atan().to_degrees();
    // at() interpolates between the 50-per-decade samples (log magnitude linear in log frequency): the
    // curvature of the response at the corner, 23 dB per decade squared, bounds that error by
    // 23 * 0.02^2 / 8 = 1.2e-3 dB; the phase is interpolated linearly in log frequency too.
    assert!((gain - exact_gain).abs() < 1.2e-3, "{gain} vs {exact_gain}");
    assert!((phase - exact_phase).abs() < 1e-2, "{phase} vs {exact_phase}");
    assert!((gain + 3.0103).abs() < 2e-3 && (phase + 45.0).abs() < 2e-2, "{gain} dB {phase} deg");
    // The plotted response is the closed form at every frequency.
    let db = plot(run, 0, "db(h)");
    let deg = plot(run, 0, "phase(h)");
    for (k, f) in run.steps[0].axis.iter().enumerate() {
        let h = Complex::ONE / Complex::new(1.0, 2.0 * std::f64::consts::PI * f * 1e3 * 100e-9);
        assert!((db[k] - 20.0 * h.abs().log10()).abs() < 1e-9, "{f}");
        assert!((deg[k] - h.arg().to_degrees()).abs() < 1e-9, "{f}");
    }
    // Fidelity of every component and supply.
    let fidelity: Vec<(&str, &str)> = run.models.iter().map(|m| (m.name.as_str(), m.fidelity)).collect();
    assert_eq!(fidelity, [("R1", "ideal"), ("C1", "ideal"), ("INPUT", "replaced"), ("GROUND", "ground")]);
}

/// The buffered Howland pump's load current from the closed form of
/// `copper-sim/tests/closed_forms.rs`: matched resistors R, Rs = R_SET, the
/// same op-amp (A0, GBW, ideal inputs) as driver and follower buffer.
fn howland_current(f: f64, vin: f64, rs: f64, rl: f64, a0: f64, gbw: f64) -> Complex {
    let a = Complex::real(a0) / Complex::new(1.0, f / (gbw / a0));
    let (alpha, p, q) = (2.0, 0.5, 0.5);
    let den = Complex::ONE + a.scale(1.0 / alpha);
    let b = a / (Complex::ONE + a);
    let g_in = a.scale(q) / den.scale(rs);
    let g_out = (Complex::ONE - a.scale(p) * b / den).scale(1.0 / rs);
    g_in.scale(vin) / (Complex::ONE + g_out.scale(rl))
}

#[test]
fn howland_example_load_current() {
    let runs = run_example("howland_pump/board.copper");
    println!("{}", copper_simulate::results::text_table(&runs));
    let run = &runs[0];
    assert_eq!(run.error, None);
    assert_eq!(run.steps.len(), 3);
    let a0 = 10f64.powf(65.0 / 20.0);
    for (step, rl) in [100.0, 1e3, 10e3].into_iter().enumerate() {
        let mag = plot(run, step, "mag(i_load)");
        let axis = &run.steps[step].axis;
        // At 1 kHz the 1.3 pF input capacitances are 122 Mohm: the ideal-input closed form holds to 2e-3.
        let exact = howland_current(axis[0], 0.15, 10e3, rl, a0, 400e6);
        assert!((mag[0] / exact.abs() - 1.0).abs() < 2e-3, "{rl}: {} vs {}", mag[0], exact.abs());
        // 15 uA nominal, low by 2 / (2 + A0) = 0.112 % (the finite open-loop gain) and by the output
        // impedance Rs A0 / 3 = 5.9 Mohm against the load (0.017 % for 1 kohm, 0.17 % for 10 kohm).
        let zout = 10e3 * (2.0 + a0) * (1.0 + a0) / (2.0 + 3.0 * a0);
        let estimate = 15e-6 * a0 / (2.0 + a0) / (1.0 + rl / zout);
        assert!((mag[0] / estimate - 1.0).abs() < 2e-3, "{rl}: {} vs {estimate}", mag[0]);
        println!(
            "R_LOAD {rl}: |I| at 1 kHz {:.6e}, at 1 MHz {:.6e}; flatness {:.5}, nominal_error {:.6}, phase_error {:.4}",
            mag[0],
            mag[mag.len() - 1],
            measure(run, step, "flatness"),
            measure(run, step, "nominal_error"),
            measure(run, step, "phase_error")
        );
    }
    let models: Vec<(&str, &str)> = run.models.iter().map(|m| (m.name.as_str(), m.fidelity)).collect();
    assert!(models.contains(&("U1", "macro")) && models.contains(&("J_IN", "replaced")));
    assert!(models.contains(&("J_OUT", "ignored")) && models.contains(&("C_U1P", "ideal")));
    assert!(models.contains(&("POS", "ideal")) && models.contains(&("GROUND", "ground")));
    let j_in = run.models.iter().find(|m| m.name == "J_IN").unwrap();
    assert_eq!(j_in.inactive, ["waveform"], "the sine waveform plays no part in an ac run");
}

/// The Howland board with ideal op-amp inputs (no input capacitance): the
/// load current equals the closed form at every frequency and step.
#[test]
fn howland_without_input_capacitance_matches_the_closed_form_everywhere() {
    let directory = std::env::temp_dir().join(format!("copper-simulate-howland-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for file in [
        "board.copper",
        "parts/connector.copper",
        "parts/opamp.copper",
        "parts/power_header.copper",
        "benches/flatness.copper",
        "benches/loaded_pump.copper",
    ] {
        let source = std::fs::read_to_string(example("howland_pump").join(file)).unwrap();
        let source = source.replace("        input_capacitance_common = 1.3pF;\n", "");
        let target = directory.join(file);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    let board = directory.join("board.copper");
    let design = compile_design_file(&board, false, true).unwrap();
    let runs = simulate(&board, &design, Some("CurrentFlatness")).unwrap();
    let run = &runs[0];
    assert_eq!(run.error, None);
    let a0 = 10f64.powf(65.0 / 20.0);
    for (step, rl) in [100.0, 1e3, 10e3].into_iter().enumerate() {
        let mag = plot(run, step, "mag(i_load)");
        let deg = plot(run, step, "phase(i_load)");
        for (k, f) in run.steps[step].axis.iter().enumerate() {
            let exact = howland_current(*f, 0.15, 10e3, rl, a0, 400e6);
            assert!((mag[k] / exact.abs() - 1.0).abs() < 1e-9, "{rl} {f}: {} vs {}", mag[k], exact.abs());
            assert!((deg[k] - exact.arg().to_degrees()).abs() < 1e-7, "{rl} {f}");
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

//! Noise analysis, Monte Carlo and worst case through the whole chain
//! (board, bench, lowering, solver, expressions), against closed forms.

use std::path::{Path, PathBuf};

use copper_lang::bench::compile_bench_source;
use copper_lang::compiler::{Design, compile_design_file};
use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-p1-{name}-{}", std::process::id()));
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

fn index(run: &BenchRun, name: &str) -> usize {
    run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"))
}

fn value(run: &BenchRun, step: usize, name: &str) -> f64 {
    run.steps[step].measures[index(run, name)].value
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance * b.abs().max(1e-30)
}

/// A divider of R1 (top) and R2 (bottom) from a 2 V supply, with component attributes.
fn divider(r1: &str, r2: &str) -> String {
    format!(
        "board Divider {{
    use library \"tiny\";
    component R1: RESISTOR {{ {r1} }}
    component R2: RESISTOR {{ {r2} }}
    net VIN {{ R1.1; }}
    net VOUT {{ R1.2; R2.1; }}
    net GND {{ R2.2; }}
    supply IN {{ voltage = 2V; net = VIN; external = true; }}
    supply GROUND {{ voltage = 0V; net = GND; external = true; }}
}}"
    )
}

#[test]
fn worst_case_of_a_divider_is_the_analytic_corner() {
    // V = 2 V R2 / (R1 + R2), R1 = 1 kohm and R2 = 3 kohm at 1 %: the high
    // corner is R1 low and R2 high, 2 * 3.03 / (0.99 + 3.03); the low corner
    // the reverse, 2 * 2.97 / (1.01 + 2.97).
    let board = divider("value = 1kohm; tolerance = 1%;", "value = 3kohm; tolerance = 1%;");
    let bench = "bench Corners for Divider {
        analysis op;
        worstcase;
        measure v_out = V(VOUT) { min = 1.49V; max = 1.51V; }
        measure i_r2 = I(R2);
    }";
    let path = project("worstcase", &[("board.copper", &board), ("benches/corners.copper", bench)]);
    let runs = run(&path);
    let run = &runs[0];
    assert_eq!(run.status(), Status::Pass, "{:?}", run.error);
    let variation = run.variation.as_ref().expect("a worst-case bench has variation results");
    assert_eq!(variation.variables, ["R1", "R2"]);
    let v = &variation.steps[0].measures[index(run, "v_out")];
    let (high, low) = (2.0 * 3.03 / (0.99 + 3.03), 2.0 * 2.97 / (1.01 + 2.97));
    assert!(close(v.nominal, 1.5, 1e-12));
    let (low_value, low_x) = v.low_corner.as_ref().unwrap();
    let (high_value, high_x) = v.high_corner.as_ref().unwrap();
    assert!(close(*high_value, high, 1e-12), "{high_value} vs {high}");
    assert!(close(*low_value, low, 1e-12), "{low_value} vs {low}");
    assert_eq!((low_x.as_slice(), high_x.as_slice()), (&[1i8, -1][..], &[-1i8, 1][..]));
    assert!(close(v.min, low, 1e-12) && close(v.max, high, 1e-12));
    // The limited measure reports its least-margin run: the low corner (1.4925 V is
    // nearer 1.49 V than 1.5075 V is to 1.51 V).
    assert!(close(value(run, 0, "v_out"), low, 1e-12));
    // R2's current 2 V / (R1 + R2): both resistors low is the high corner.
    let i = &variation.steps[0].measures[index(run, "i_r2")];
    assert!(close(i.high_corner.as_ref().unwrap().0, 2.0 / (0.99e3 + 2.97e3), 1e-12));
    assert!(close(i.low_corner.as_ref().unwrap().0, 2.0 / (1.01e3 + 3.03e3), 1e-12));
    // Tighter limits fail at the corners while the nominal value passes.
    let tight = bench.replace("min = 1.49V; max = 1.51V;", "min = 1.495V; max = 1.505V;");
    let path = project("worstcase-tight", &[("board.copper", &board), ("benches/corners.copper", &tight)]);
    let failing = run_one(&path);
    assert_eq!(failing.status(), Status::Fail);
    assert!(close(value(&failing, 0, "v_out"), low, 1e-12));
}

#[test]
fn a_matched_pair_keeps_its_ratio_within_the_match_tolerance() {
    // R1 = R2 = 10 kohm, 1 % absolute and 0.1 % ratio: each member is
    // nominal (1 + 0.95 % x_common + 0.05 % x_own). The ratio V = V_IN R2 / (R1 + R2)
    // does not see the common part: its corners are 2 * 1.0005 / 2 and 2 * 0.9995 / 2.
    // The current sees it: both members low by 1 % is 2 V / (2 * 9.9 kohm).
    let attributes = "value = 10kohm; tolerance = 1%; match = \"PAIR\"; match_tolerance = 0.1%;";
    let board = divider(attributes, attributes);
    let bench = "bench Pair for Divider {
        analysis op;
        worstcase;
        measure v_out = V(VOUT);
        measure i_r2 = I(R2);
    }";
    let path = project("matched", &[("board.copper", &board), ("benches/pair.copper", bench)]);
    let runs = run(&path);
    let run = &runs[0];
    assert_eq!(run.status(), Status::Pass, "{:?}", run.error);
    let variation = run.variation.as_ref().unwrap();
    assert_eq!(variation.variables, ["PAIR common", "PAIR R1", "PAIR R2"]);
    let v = &variation.steps[0].measures[index(run, "v_out")];
    assert!(close(v.high_corner.as_ref().unwrap().0, 1.0005, 1e-12), "{:?}", v.high_corner);
    assert!(close(v.low_corner.as_ref().unwrap().0, 0.9995, 1e-12));
    // The common variable does not move the ratio (to rounding).
    assert!(v.sensitivity[0].abs() < 1e-12, "{:?}", v.sensitivity);
    let i = &variation.steps[0].measures[index(run, "i_r2")];
    assert!(close(i.high_corner.as_ref().unwrap().0, 2.0 / 19.8e3, 1e-12));
    assert!(close(i.low_corner.as_ref().unwrap().0, 2.0 / 20.2e3, 1e-12));
}

#[test]
fn monte_carlo_divider_statistics_yield_and_determinism() {
    // R1 = R2 = 10 kohm, uniform within 1 %: V = 1 V + (d2 - d1) / 2 V to
    // first order, a triangular distribution of half-width a = 10 mV with
    // standard deviation a / sqrt(6) = 4.082 mV. Within +-sigma lies
    // 1 - (1 - 1/sqrt 6)^2 = 65.0 % of it.
    let sigma = 0.01 / 6f64.sqrt();
    let board = divider("value = 10kohm; tolerance = 1%;", "value = 10kohm; tolerance = 1%;");
    let bench = format!(
        "bench Spread for Divider {{
        analysis op;
        montecarlo {{ runs = 2000; seed = 3; }}
        measure v_out = V(VOUT) {{ min = {}V; max = {}V; }}
    }}",
        1.0 - sigma,
        1.0 + sigma
    );
    let path = project("montecarlo", &[("board.copper", &board), ("benches/spread.copper", &bench)]);
    let runs = run(&path);
    let run = &runs[0];
    let variation = run.variation.as_ref().unwrap();
    let m = &variation.steps[0].measures[0];
    assert_eq!(m.values.len(), 2000);
    // Mean within 4 standard errors, sigma within 5 %, every run inside the corners.
    assert!((m.mean.unwrap() - 1.0).abs() < 4.0 * sigma / 2000f64.sqrt(), "{:?}", m.mean);
    assert!(close(m.std.unwrap(), sigma, 0.05), "{:?} vs {sigma}", m.std);
    assert!(m.min >= 2.0 * 0.99 / 2.0 && m.max <= 2.0 * 1.01 / 2.0);
    let y = m.yield_fraction.unwrap();
    assert!((y - 0.650).abs() < 0.035, "{y}");
    // Limits apply to the worst run: the bench fails, its value is the extreme run.
    assert_eq!(run.status(), Status::Fail);
    let worst = value(run, 0, "v_out");
    assert!(worst == m.min || worst == m.max);
    assert_eq!(variation.yield_fraction(), Some(y));
    // The same seed gives the same runs, bit for bit; another seed others.
    let again = &simulate(&path, &compile(&path), None).unwrap()[0];
    let values = |run: &BenchRun| run.variation.as_ref().unwrap().steps[0].measures[0].values.clone();
    assert!(values(again).iter().zip(&m.values).all(|(a, b)| a.to_bits() == b.to_bits()));
    let other = bench.replace("seed = 3", "seed = 4");
    let path = project("montecarlo-seed", &[("board.copper", &board), ("benches/spread.copper", &other)]);
    let other = &run_one(&path);
    assert!(values(other).iter().zip(&m.values).any(|(a, b)| a.to_bits() != b.to_bits()));
}

fn run_one(path: &Path) -> BenchRun {
    run(path).remove(0)
}

#[test]
fn monte_carlo_runs_are_the_same_boards_at_every_step() {
    // A stepped load on the stiff input does not change V(VOUT): every run k
    // gives the same value at both steps. A param tolerance and a gaussian draw.
    let board = divider("value = R_TOP;", "value = 10kohm; tolerance = 1%;")
        .replace("use library \"tiny\";", "use library \"tiny\";\n    param R_TOP = 10kohm;");
    let bench = "bench Steps for Divider {
        param R_X = 1kohm;
        component RX: RESISTOR { value = R_X; }
        net VIN { RX.1; }
        net GND { RX.2; }
        analysis op;
        step R_X in 1kohm, 2kohm;
        tolerance R_TOP = 2%;
        montecarlo { runs = 50; seed = 11; distribution = gaussian; }
        measure v_out = V(VOUT);
    }";
    let path = project("steps", &[("board.copper", &board), ("benches/steps.copper", bench)]);
    let run = run_one(&path);
    assert!(run.error.is_none(), "{:?}", run.error);
    let variation = run.variation.as_ref().unwrap();
    assert_eq!(variation.variables, ["R2", "R_TOP"]);
    let (a, b) = (&variation.steps[0].measures[0].values, &variation.steps[1].measures[0].values);
    assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
    // Gaussian with the tolerance at 3 sigma, truncated: V moves by
    // (d2 - d1) / 2 with sigma sqrt(1 %^2 + 2 %^2) / 3 / 2 V... every run within the corners.
    let low = 2.0 * 0.99 / (0.99 + 1.02);
    let high = 2.0 * 1.01 / (1.01 + 0.98);
    assert!(a.iter().all(|v| *v >= low && *v <= high));
}

const RC: &str = "board RC {
    use library \"tiny\";
    component R1: RESISTOR { value = 1kohm; }
    component C1: CAPACITOR { value = 1nF; }
    net VIN { R1.1; }
    net VOUT { R1.2; C1.1; }
    net GND { C1.2; }
    supply INPUT { voltage = 1V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

#[test]
fn noise_of_an_rc_lowpass_through_the_chain() {
    // R1 = 1 kohm, C1 = 1 nF: fc = 159.2 kHz, density sqrt(4kTR) = 4.07 nV/rtHz
    // below it, sqrt(kT/C) = 2.035 uV over all frequencies, all of it from R1.
    // Referred to the source (|H| = 1 at low frequency) the density is the same.
    let bench = "bench Noise for RC {
        replace INPUT with source { out = VIN; ref = GND; dc = 1V; ac = 1V; }
        analysis noise { from = 1Hz; to = 1.6GHz; points_per_decade = 50; output = V(VOUT); input = INPUT; }
        measure density = at(noise(), 10Hz);
        measure referred = at(input_noise(), 10Hz);
        measure total = noise_rms(noise(), 1Hz, 1.6GHz);
        measure share = noise_share(noise(R1), 1Hz, 1.6GHz);
        measure capacitor = noise_rms(noise(C1), 1Hz, 1.6GHz);
        measure gain = mag(at(V(VOUT), 10Hz));
        plot noise();
    }";
    let path = project("noise", &[("board.copper", RC), ("benches/noise.copper", bench)]);
    let run = run_one(&path);
    assert!(run.error.is_none(), "{:?}", run.error);
    let kt = BOLTZMANN * DEFAULT_TEMPERATURE;
    // At 10 Hz the pole lowers the density by (10 Hz / fc)^2 / 2 = 2e-9.
    assert!(close(value(&run, 0, "density"), (4.0 * kt * 1e3).sqrt(), 1e-8));
    assert!(close(value(&run, 0, "referred"), (4.0 * kt * 1e3).sqrt(), 1e-6));
    // The band misses 4kTR * 1 Hz below and the tail above 1.6 GHz: about 3.6e-5 low.
    assert!(close(value(&run, 0, "total"), (kt / 1e-9).sqrt(), 1e-4), "{}", value(&run, 0, "total"));
    assert!(close(value(&run, 0, "share"), 1.0, 1e-12));
    assert_eq!(value(&run, 0, "capacitor"), 0.0);
    assert!(close(value(&run, 0, "gain"), 1.0, 1e-6));
    let noise = run.steps[0].noise.as_ref().expect("a noise step summary");
    assert_eq!(noise.contributors.len(), 1);
    assert_eq!((noise.contributors[0].label.as_str(), noise.contributors[0].owner.as_str()), ("R1 thermal", "R1"));
    // JSON: the noise summary and the analysis are in the results; a nominal bench has no variation.
    let json = copper_simulate::results::results_json(std::slice::from_ref(&run), "RC");
    assert!(json.contains("\"kind\": \"noise\"") && json.contains("\"total_rms_v\""));
    assert!(!json.contains("\"variation\""));
    // A hotter board: density as sqrt(T).
    let hot = bench.replace("input = INPUT;", "input = INPUT; temperature = 85degC;");
    let path = project("noise-hot", &[("board.copper", RC), ("benches/noise.copper", &hot)]);
    let run = run_one(&path);
    assert!(close(value(&run, 0, "density"), (4.0 * BOLTZMANN * 358.15 * 1e3).sqrt(), 1e-8));
}

#[test]
fn noise_and_tolerance_mistakes_are_named() {
    let board = divider("value = 1kohm; tolerance = 1%;", "value = 3kohm;");
    let path = project("mistakes", &[("board.copper", &board)]);
    let design = compile(&path);
    let check = |body: &str, code: &str| {
        let source = format!("bench B for Divider {{\n{body}\n}}");
        match compile_bench_source(&source, "b.copper", &design) {
            Ok(_) => panic!("{body}: compiled"),
            Err(failure) => assert!(failure.to_string().contains(code), "{body}: {failure}"),
        }
    };
    check("analysis op; measure n = at(noise(), 1kHz);", "SIM085");
    check(
        "analysis ac { from = 1Hz; to = 1kHz; points_per_decade = 5; } measure n = noise_rms(mag(V(VOUT)), 1Hz, 1kHz);",
        "SIM085",
    );
    check(
        "analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; output = V(VOUT); } measure n = at(input_noise(), 1kHz);",
        "SIM086",
    );
    check(
        "analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; output = V(VOUT); } measure n = at(noise(R9), 1kHz);",
        "SIM087",
    );
    check(
        "analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; output = V(VOUT); } measure n = noise_share(noise(R1), 1Hz, V(VOUT));",
        "SIM088",
    );
    check("analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; }", "SIM094");
    check(
        "analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; output = V(VOUT); temperature = 27; }",
        "SIM094",
    );
    check("analysis noise { from = 1Hz; to = 1kHz; points_per_decade = 5; output = V(VOUT); input = R1; }", "SIM094");
    check("analysis op; tolerance R2 = 1%;", "SIM090");
    check("analysis op; montecarlo { seed = 1; }", "SIM090");
    check("analysis op; montecarlo { runs = 10; } worstcase;", "SIM090");
    check("analysis op; worstcase; tolerance R9 = 1%;", "SIM091");
    check("analysis op; worstcase; tolerance R2 = 1V;", "SIM091");
    check("analysis op; worstcase; tolerance R1 = 1%; tolerance R1 = 2%;", "SIM091");
    let plain = divider("value = 1kohm;", "value = 3kohm;");
    let path = project("mistakes-plain", &[("board.copper", &plain)]);
    let plain_design = compile(&path);
    let source = "bench B for Divider { analysis op; worstcase; measure v = V(VOUT); }";
    let failure = compile_bench_source(source, "b.copper", &plain_design).unwrap_err();
    assert!(failure.to_string().contains("SIM093"), "{failure}");
    // A match group of one simulated member.
    let lone = divider("value = 1kohm; tolerance = 1%; match = \"G\"; match_tolerance = 0.1%;", "value = 3kohm;");
    let path = project("mistakes-lone", &[("board.copper", &lone)]);
    let lone_design = compile(&path);
    let failure = compile_bench_source(source, "b.copper", &lone_design).unwrap_err();
    assert!(failure.to_string().contains("SIM092"), "{failure}");
    // Component attributes are checked when the board compiles.
    for attributes in [
        "value = 1kohm; tolerance = 1V;",
        "value = 1kohm; tolerance = 0%;",
        "value = 1kohm; match = \"G\";",
        "value = 1kohm; tolerance = 1%; match_tolerance = 0.1%;",
        "value = 1kohm; tolerance = 0.1%; match = \"G\"; match_tolerance = 1%;",
    ] {
        let path = project("mistakes-attributes", &[("board.copper", &divider(attributes, "value = 3kohm;"))]);
        match compile_design_file(&path, false, true) {
            Ok(_) => panic!("{attributes}: compiled"),
            Err(failure) => assert!(failure.to_string().contains("SIM089"), "{attributes}: {failure}"),
        }
    }
}

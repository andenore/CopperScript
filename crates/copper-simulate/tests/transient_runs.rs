//! Transient benches through the library entry point: the precision
//! rectifier example against the ideal rectifier's closed forms, an RC step
//! against its time constant, and the outputs and errors of a tran run.

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation").join(path)
}

fn run(board: &Path) -> Vec<BenchRun> {
    let design = compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    simulate(board, &design, None).unwrap_or_else(|failure| panic!("{failure}"))
}

fn measure(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

/// A project directory with `files` (relative path, source).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-tran-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

#[test]
fn precision_rectifier_example() {
    let runs = run(&example("precision_rectifier/board.copper"));
    println!("{}", copper_simulate::results::text_table(&runs));
    let r = &runs[0];
    assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass));
    // The ideal half-wave rectifier of a 1 V sine: peak 1 V, mean 1/pi V, and
    // harmonics 1/2, 2/(3 pi), 2/(15 pi), 2/(35 pi), 2/(63 pi) of 1 V (even ones only).
    let ideal_thd =
        [3.0, 15.0, 35.0, 63.0].iter().map(|k: &f64| (2.0 / (k * std::f64::consts::PI)).powi(2)).sum::<f64>().sqrt()
            / 0.5;
    let (peak, mean, thd) = (measure(r, 0, "peak"), measure(r, 0, "average"), measure(r, 0, "distortion"));
    println!("peak {peak}, mean {mean} (ideal {}), thd {thd} (ideal {ideal_thd})", 1.0 / std::f64::consts::PI);
    assert!((peak - 1.0).abs() < 1e-3);
    assert!((mean * std::f64::consts::PI - 1.0).abs() < 2e-3);
    assert!((thd / ideal_thd - 1.0).abs() < 1e-2);
    // The op-amp's limits are active in tran, so none of its parameters is listed inactive.
    let u1 = r.models.iter().find(|m| m.name == "U1").unwrap();
    assert!(u1.inactive.is_empty(), "{:?}", u1.inactive);
    let stats = r.steps[0].transient.as_ref().unwrap();
    assert!(stats.accepted_steps > 2000 && stats.largest_step <= 1e-6);
}

const RC: &str = "board RC {
    use library \"tiny\";
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    component J1: HEADER;
    net VIN { J1.1; R1.1; }
    net VOUT { R1.2; C1.1; }
    net GND { J1.2; C1.2; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

#[test]
fn rc_step_time_constant_outputs_and_errors() {
    let bench = "bench Step for RC {
        replace J1 with source { out = J1.1; ref = GND; waveform = pulse { initial = 0V; pulsed = 1V; delay = 10us; width = 1ms; }; }
        analysis tran { stop = 800us; }
        step R_VALUE in 1kohm, 2kohm;
        let v = V(VOUT);
        measure tau = cross(v, 0.632121V) - 10us;
        measure final = at(v, 800us);
        measure ripple = pp(v, 700us, 800us);
        measure charge = mean(I(C1), 10us, 800us) * 790us;
        plot v, I(R1);
    }";
    let board = project(
        "rc",
        &[
            (
                "board.copper",
                &RC.replace("HEADER;", "p.HEADER;")
                    .replace("use library \"tiny\";", "use library \"tiny\"; import p \"./parts\";"),
            ),
            ("parts/header.copper", HEADER),
            ("benches/step.copper", bench),
        ],
    );
    let runs = run(&board);
    let r = &runs[0];
    assert_eq!(r.error, None, "{}", copper_simulate::results::text_table(&runs));
    for (step, tau) in [(0, 100e-6), (1, 200e-6)] {
        let measured = measure(r, step, "tau");
        assert!((measured / tau - 1.0).abs() < 2e-3, "step {step}: {measured} vs {tau}");
        // The charge that entered C1 is C (V(800 us) - 0), to the quadrature of the 1 mA jump at 10 us.
        let charge = measure(r, step, "charge");
        let want = 100e-9 * measure(r, step, "final");
        assert!((charge / want - 1.0).abs() < 5e-3, "{charge} vs {want}");
    }
    assert!((measure(r, 0, "final") - (1.0 - (-7.9f64).exp())).abs() < 1e-3);
    // Outputs: the time axis in JSON and CSV, per-step solver work, the time figure.
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(
        json.contains("\"kind\": \"tran\"")
            && json.contains("\"quantity\": \"time\"")
            && json.contains("\"accepted_steps\"")
    );
    assert!(!json.contains("\"quantity\": \"frequency\""));
    let csv = copper_simulate::results::plots_csv(r);
    assert!(csv.starts_with("step,label,time_s,"), "{}", &csv[..80]);
    let html = copper_simulate::report::html(r, "RC");
    assert!(html.contains("data-axis=\"time\"") && html.contains(">time<") && html.contains("us<"));
    assert!(copper_simulate::results::text_table(&runs).contains("tran 0 to 800 us"));

    // A capacitor-only node has no DC path at the initial operating point: named in source terms.
    let floating = "bench Floating for RC {
        replace J1 with source { out = J1.1; ref = GND; waveform = step { final = 1V; }; }
        replace R1 with load { inp = VIN; ref = VOUT; capacitance = 1nF; }
        analysis tran { stop = 1ms; }
        measure v = max(V(VOUT));
    }";
    let board = project(
        "floating",
        &[
            (
                "board.copper",
                &RC.replace("HEADER;", "p.HEADER;")
                    .replace("use library \"tiny\";", "use library \"tiny\"; import p \"./parts\";"),
            ),
            ("parts/header.copper", HEADER),
            ("benches/floating.copper", floating),
        ],
    );
    let runs = run(&board);
    let error = runs[0].error.clone().unwrap_or_default();
    assert!(
        error.contains("initial operating point") && error.contains("VOUT") && error.contains("no DC path"),
        "{error}"
    );
    assert_eq!(runs[0].status(), Status::Error);
}

#[test]
fn parallel_steps_equal_the_serial_run_and_report_the_first_failing_step() {
    let bench = "bench Steps for RC {
        replace J1 with source { out = J1.1; ref = GND; waveform = pulse { initial = 0V; pulsed = 1V; delay = 10us; width = 1ms; }; }
        analysis tran { stop = 400us; }
        step R_VALUE in 1kohm, 2kohm, 500ohm, 3kohm;
        measure final = at(V(VOUT), 400us);
        measure tau = cross(V(VOUT), 0.632121V) - 10us;
        plot V(VOUT);
    }";
    let files = |bench: &str| {
        vec![
            (
                "board.copper".to_owned(),
                RC.replace("HEADER;", "p.HEADER;")
                    .replace("use library \"tiny\";", "use library \"tiny\"; import p \"./parts\";"),
            ),
            ("parts/header.copper".to_owned(), HEADER.to_owned()),
            ("benches/steps.copper".to_owned(), bench.to_owned()),
        ]
    };
    // The same project, serially and on threads.
    let both = |name: &str, bench: &str| {
        let files = files(bench);
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
        let board = project(name, &refs);
        let design = compile_design_file(&board, false, true).unwrap();
        let serial = copper_simulate::parallel::with_jobs(1, || simulate(&board, &design, None).unwrap());
        let parallel = copper_simulate::parallel::with_jobs(3, || simulate(&board, &design, None).unwrap());
        (serial, parallel)
    };
    let (serial, parallel) = both("parallel", bench);
    assert_eq!(serial[0].error, None);
    assert_eq!(serial[0].steps.len(), 4);
    assert_eq!(serial, parallel);
    // At 3 kohm (tau 300 us) V(VOUT) never reaches 0.99 V in 400 us, nor at 1 kohm and
    // 2 kohm: the bench stops at its first step either way.
    let failing = bench.replace("0.632121V", "0.99V");
    let (serial, parallel) = both("parallel-failing", &failing);
    assert!(
        serial[0].error.as_deref().is_some_and(|e| e.contains("at step R_VALUE = 1 kohm")),
        "{:?}",
        serial[0].error
    );
    assert!(serial[0].steps.is_empty());
    assert_eq!(serial, parallel);
}

#[test]
fn accuracy_high_tightens_the_step_control_where_it_is_not_given() {
    // The RC step (tau 100 us) at the default, at accuracy = high, and at high
    // with an explicit reltol and max_step, which win. The error of the 63 %
    // crossing against tau: at high it is within the preset's level, 2e-5 of
    // the range (0.02 sqrt(1e-6)); at the default within twice its 2e-4 (the
    // rule holds within a factor of 2; here 2.5e-4, the crossing read between
    // steps of 16 us).
    let bench = |extra: &str| {
        format!(
            "bench Step for RC {{
        replace J1 with source {{ out = J1.1; ref = GND; waveform = pulse {{ initial = 0V; pulsed = 1V; delay = 10us; width = 1ms; }}; }}
        analysis tran {{ stop = 800us; {extra} }}
        measure tau = cross(V(VOUT), 0.632121V) - 10us;
        measure final = at(V(VOUT), 800us);
    }}"
        )
    };
    let run_with = |name: &str, extra: &str| {
        let board = project(
            name,
            &[
                (
                    "board.copper",
                    &RC.replace("HEADER;", "p.HEADER;")
                        .replace("use library \"tiny\";", "use library \"tiny\"; import p \"./parts\";"),
                ),
                ("parts/header.copper", HEADER),
                ("benches/step.copper", &bench(extra)),
            ],
        );
        let runs = run(&board);
        assert_eq!(runs[0].error, None, "{}", copper_simulate::results::text_table(&runs));
        runs.into_iter().next().unwrap()
    };
    let solver = |r: &BenchRun| match r.steps[0].analysis {
        copper_simulate::run::AnalysisRun::Tran { max_step, reltol, .. } => (max_step, reltol),
        _ => unreachable!("a tran bench"),
    };
    let normal = run_with("accuracy-normal", "");
    let high = run_with("accuracy-high", "accuracy = high;");
    let explicit = run_with("accuracy-explicit", "accuracy = high; reltol = 0.001%; max_step = 4us;");
    assert_eq!(solver(&normal), (None, 1e-4));
    assert_eq!(solver(&high), (Some(800e-6 / 500.0), 1e-6));
    let (max_step, reltol) = solver(&explicit);
    assert_eq!(max_step, Some(4e-6));
    assert!((reltol - 1e-5).abs() < 1e-18, "{reltol}");
    // The crossing's error, in time over tau, is the waveform's error over its slope there (1/e of the range per tau).
    let error = |r: &BenchRun| (measure(r, 0, "tau") / 100e-6 - 1.0).abs() / std::f64::consts::E;
    let (e_normal, e_high) = (error(&normal), error(&high));
    println!(
        "RC step: tau error {e_normal:.2e} of the range at the default ({} steps), {e_high:.2e} at high ({} steps)",
        normal.steps[0].transient.as_ref().unwrap().accepted_steps,
        high.steps[0].transient.as_ref().unwrap().accepted_steps
    );
    assert!(e_high <= copper_sim::Accuracy::High.error_level(), "{e_high}");
    assert!(e_normal <= 2.0 * copper_sim::Accuracy::Normal.error_level(), "{e_normal}");
    assert!(e_high < e_normal);
}

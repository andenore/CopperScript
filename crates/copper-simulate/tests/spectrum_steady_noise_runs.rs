//! Phase F9 through the library entry point: the precision rectifier's
//! periodic steady state against a long transient run, transient noise
//! against the noise analysis of the same bench, the spectrum outputs, and
//! stepped sweeps around `analysis dc` and `analysis ac`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use copper_lang::compiler::compile_design_file;
use copper_simulate::run::BenchRun;
use copper_simulate::simulate;

fn run(board: &Path, only: Option<&str>) -> Vec<BenchRun> {
    let design = compile_design_file(board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    simulate(board, &design, only).unwrap_or_else(|failure| panic!("{failure}"))
}

fn measure(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

fn bench<'a>(runs: &'a [BenchRun], name: &str) -> &'a BenchRun {
    runs.iter().find(|run| run.name == name).unwrap_or_else(|| panic!("no bench {name}"))
}

/// A project directory with `files` (relative path, source).
fn project(name: &str, files: &[(String, String)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-f9-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, source) in files {
        let target = directory.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, source).unwrap();
    }
    directory.join("board.copper")
}

/// The precision rectifier example's board and parts, with `benches` instead of its own.
fn rectifier(name: &str, benches: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation/precision_rectifier");
    let mut files = vec![("board.copper".to_owned(), std::fs::read_to_string(root.join("board.copper")).unwrap())];
    for entry in std::fs::read_dir(root.join("parts")).unwrap() {
        let path = entry.unwrap().path();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        files.push((format!("parts/{file}"), std::fs::read_to_string(&path).unwrap()));
    }
    files.extend(benches.iter().map(|(file, source)| (format!("benches/{file}"), (*source).to_owned())));
    project(name, &files)
}

const RECTIFIER_MEASURES: &str = "
    measure peak = max(V(OUT));
    measure turn_off_kick = min(V(OUT));
    measure average = mean(V(OUT));
    measure amp_clip = min(V(AMP));
    measure distortion = thd(V(OUT), 1kHz);
    measure h2 = harmonic(V(OUT), 1kHz, 2);
    plot V(OUT);";

#[test]
fn precision_rectifier_steady_state_matches_a_long_run() {
    // The rectifier settles within its first period (its only slow states are the
    // rail decoupling capacitors, held by the supplies), so a run of 20 periods
    // records the steady state over its last one; the steady state records one.
    let source =
        "replace J_IN with source { out = J_IN.1; ref = GND; waveform = sine { amplitude = 1V; frequency = 1kHz; }; }
    ignore J_PWR;";
    let steady = format!(
        "bench Steady for PrecisionRectifier {{ {source}
        analysis tran {{ stop = 1ms; max_step = 1us; steady = 1kHz; }}{RECTIFIER_MEASURES}
        plot spectrum(V(OUT), 1kHz);
    }}"
    );
    let long = format!(
        "bench Long for PrecisionRectifier {{ {source}
        analysis tran {{ stop = 20ms; start = 19ms; max_step = 1us; }}{RECTIFIER_MEASURES}
    }}"
    );
    let board = rectifier("rectifier", &[("steady.copper", &steady), ("long.copper", &long)]);
    let clock = Instant::now();
    let steady_runs = run(&board, Some("Steady"));
    let shooting = clock.elapsed().as_secs_f64();
    let clock = Instant::now();
    let long_runs = run(&board, Some("Long"));
    let settling = clock.elapsed().as_secs_f64();
    let (s, l) = (&steady_runs[0], &long_runs[0]);
    assert_eq!((s.error.as_deref(), l.error.as_deref()), (None, None));
    let stats = s.steps[0].transient.as_ref().unwrap();
    println!(
        "precision rectifier: steady state in {} Newton iterations and {} periods, {shooting:.2} s; 20 periods {settling:.2} s ({:.1} times as long)",
        stats.steady_iterations,
        stats.steady_periods,
        settling / shooting
    );
    assert!(stats.steady_fallback.is_none(), "{:?}", stats.steady_fallback);
    for (name, tolerance) in [
        ("peak", 1e-4),
        ("turn_off_kick", 1e-4),
        ("average", 1e-4),
        ("amp_clip", 1e-3),
        ("distortion", 1e-4),
        ("h2", 1e-5),
    ] {
        let (a, b) = (measure(s, 0, name), measure(l, 0, name));
        println!("  {name}: steady {a:.6e}, long run {b:.6e}");
        assert!((a - b).abs() <= tolerance, "{name}: {a} vs {b}");
    }
    // The same waveform: the long run's last period against the steady one, point by point.
    let (ts, vs) = (&s.steps[0].axis, &s.steps[0].plots[0]);
    let (tl, vl) = (&l.steps[0].axis, &l.steps[0].plots[0]);
    let worst = tl
        .iter()
        .zip(vl)
        .map(|(t, v)| {
            let at = (t - 19e-3).clamp(0.0, 1e-3);
            (copper_sim::time_measure::value_at(ts, vs, at).unwrap() - v).abs()
        })
        .fold(0.0, f64::max);
    println!("  largest difference of V(OUT) over the period: {worst:.2e} V");
    // Within the error control's reach: reltol 1e-4 of the 5 V the amplifier swings, at
    // the fast edges where the two runs' time points differ.
    assert!(worst < 5e-3, "{worst}");
    // The spectrum: harmonic 2 of the spectrum is the harmonic measure, odd ones vanish.
    let spectrum = &s.steps[0].spectra[0];
    assert_eq!(spectrum.fundamental, Some(1e3));
    assert!((spectrum.harmonic(2).unwrap() / measure(s, 0, "h2") - 1.0).abs() < 1e-3);
    let json = copper_simulate::results::results_json(&steady_runs, "PrecisionRectifier");
    assert!(
        json.contains("\"spectra\"") && json.contains("\"steady_period_s\"") && json.contains("\"steady_iterations\"")
    );
    let csv = copper_simulate::results::spectra_csv(s);
    assert!(csv.starts_with("step,label,spectrum,frequency_hz,harmonic,amplitude,level_db\n"));
    let html = copper_simulate::report::html(s, "PrecisionRectifier");
    assert!(html.contains("class=\"sim-spectrum\"") && html.contains("integration floor"), "spectrum figure");
    assert!(copper_simulate::results::text_table(&steady_runs).contains("periodic steady state of 1 ms"));
}

const RC: &str = "board RC {
    use library \"tiny\";
    import p \"./parts\";
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    component J1: p.HEADER;
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

fn rc(name: &str, benches: &[(&str, &str)]) -> PathBuf {
    let mut files =
        vec![("board.copper".to_owned(), RC.to_owned()), ("parts/header.copper".to_owned(), HEADER.to_owned())];
    files.extend(benches.iter().map(|(file, source)| (format!("benches/{file}"), (*source).to_owned())));
    project(name, &files)
}

#[test]
fn transient_noise_matches_the_noise_analysis_of_the_same_bench() {
    // 100 ohm into 100 nF (tau 10 us): kT/C, 0.203 uV RMS at 27 degC, whatever R.
    // The noise section integrates the small-signal analysis over the whole band;
    // the transient section samples the noise every tau / 50 for 40 ms: the RMS of
    // a record 4000 tau long scatters by about 1 % (one standard deviation).
    let source = "bench Noise for RC {
        param R_VALUE = 100ohm;
        replace J1 with source { out = J1.1; ref = GND; dc = 0V; ac = 1V; }
        analysis small: noise { from = 1Hz; to = 1GHz; points_per_decade = 10; output = V(VOUT); }
        measure total = noise_rms(noise(), 1Hz, 1GHz);
        analysis time: tran { stop = 40ms; start = 0.5ms; max_step = 200ns; noise = true; seed = 7; }
        measure rms = rms(V(VOUT));
        measure mean = mean(V(VOUT));
        measure snr = snr(V(VOUT), 1kHz, 100Hz, 1MHz);
        plot spectrum(V(VOUT));
    }";
    let runs = run(&rc("noise", &[("noise.copper", source)]), None);
    let (small, time) = (bench(&runs, "Noise.small"), bench(&runs, "Noise.time"));
    assert_eq!((small.error.as_deref(), time.error.as_deref()), (None, None));
    let (total, rms) = (measure(small, 0, "total"), measure(time, 0, "rms"));
    let kt_c = (copper_sim::noise::BOLTZMANN * 300.15 / 100e-9f64).sqrt();
    println!(
        "rc noise: analysis {total:.4e} V, transient {rms:.4e} V ({:+.2} %), kT/C {kt_c:.4e} V",
        (rms / total - 1.0) * 100.0
    );
    // The band misses the tail above 1 GHz, (2 / pi) f_c / 1 GHz of the power (1e-4).
    assert!((total / kt_c - 1.0).abs() < 3e-3, "{total} vs {kt_c}");
    assert!((rms / total - 1.0).abs() < 0.05, "{rms} vs {total}");
    assert!(measure(time, 0, "mean").abs() < 0.1 * total);
    let stats = time.steps[0].transient.as_ref().unwrap();
    assert_eq!(stats.noise_generators, 1, "R1's thermal noise; the source is ideal");
    // A noise record has no 1 kHz tone: its "SNR" against one is very negative.
    assert!(measure(time, 0, "snr") < -20.0);
    // A windowed spectrum of the noise: flat at low frequency, falling past the corner.
    let s = &time.steps[0].spectra[0];
    assert!(s.fundamental.is_none() && s.window == copper_sim::spectrum::Window::BlackmanHarris);
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"noise_seed\": 7") && json.contains("\"noise_generators\": 1"));
    assert!(copper_simulate::results::text_table(&runs).contains("with noise (seed 7)"));
}

#[test]
fn steady_state_refusals_are_named() {
    // A source that does not repeat with the period.
    let source = "bench Bad for RC {
        replace J1 with source { out = J1.1; ref = GND; waveform = pulse { initial = 0V; pulsed = 1V; width = 10us; }; }
        analysis tran { stop = 1ms; steady = 1ms; }
        measure v = max(V(VOUT));
    }";
    let runs = run(&rc("refusal", &[("bad.copper", source)]), None);
    let error = runs[0].error.clone().unwrap_or_default();
    assert!(error.contains("SIM112") && error.contains("J1") && error.contains("repeat"), "{error}");
}

#[test]
fn stepped_bias_around_ac_and_dc_sweeps() {
    // A diode on VOUT biased through R1 from a source whose dc is a param: stepping
    // the param around analysis ac is a dc sweep as the outer loop of an ac
    // analysis. At bias current I the diode is r_d = n V_T / I, so the low-frequency
    // gain is r_d / (R + r_d), with I from the operating point of the same step.
    let source = "bench Bias for RC {
        param VB = 1V;
        replace J1 with source { out = J1.1; ref = GND; dc = VB; ac = 1V; }
        model D: diode { anode = VOUT; cathode = GND; }
        step VB from 0.8V to 1.6V by 0.4V;
        analysis bias: op;
        measure v_out = V(VOUT);
        analysis gain: ac { from = 1Hz; to = 1kHz; points_per_decade = 5; }
        measure low = mag(at(V(VOUT), 1Hz));
        analysis sweep: dc { sweep = J1; from = 0V; to = 2V; points = 21; }
        measure at_1v = at(V(VOUT), 1V);
        plot V(VOUT);
    }";
    let runs = run(&rc("bias", &[("bias.copper", source)]), None);
    let (op, ac, dc) = (bench(&runs, "Bias.bias"), bench(&runs, "Bias.gain"), bench(&runs, "Bias.sweep"));
    for r in [op, ac, dc] {
        assert_eq!(r.error, None, "{}", r.name);
        assert_eq!(r.steps.len(), 3);
    }
    let vt = 1.380649e-23 * 300.15 / 1.602176634e-19;
    for (step, vb) in [0.8, 1.2, 1.6].into_iter().enumerate() {
        let v = measure(op, step, "v_out");
        let current = (vb - v) / 1e3;
        let rd = vt / current;
        let want = rd / (1e3 + rd);
        let gain = measure(ac, step, "low");
        println!("bias {vb} V: V(VOUT) {v:.4} V, {:.3} mA, gain {gain:.5} against {want:.5}", current * 1e3);
        assert!((gain / want - 1.0).abs() < 2e-3, "{gain} vs {want}");
        // The dc sweep of the source overrides the stepped dc: the same curve at every step.
        assert!((measure(dc, step, "at_1v") - measure(dc, 0, "at_1v")).abs() < 1e-12);
    }
    // A nested dc sweep: the source swept inside, R1 stepped outside.
    let nested = "bench Nested for RC {
        replace J1 with source { out = J1.1; ref = GND; dc = 0V; }
        model D: diode { anode = VOUT; cathode = GND; }
        step R_VALUE from 1kohm to 3kohm points 3;
        analysis dc { sweep = J1; from = 0V; to = 2V; points = 21; }
        measure at_2v = at(V(VOUT), 2V);
        plot V(VOUT);
    }";
    let runs = run(&rc("nested", &[("nested.copper", nested)]), None);
    let r = &runs[0];
    assert_eq!(r.error, None);
    let levels: Vec<f64> = (0..3).map(|step| measure(r, step, "at_2v")).collect();
    assert!(levels[0] > levels[1] && levels[1] > levels[2], "{levels:?}");
    assert_eq!(r.steps[2].label, "R_VALUE = 3 kohm");
}

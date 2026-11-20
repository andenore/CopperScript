//! The stability analyses of phase F3 through the whole chain: `analysis
//! loop` at an amplifier's output pin against the closed forms of a
//! single-pole amplifier, `analysis pz` against an RC pole and on an
//! amplifier wired for positive feedback (unstable: the bench fails), the
//! results JSON, the text table and the HTML page.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use copper_lang::compiler::{Design, compile_design_file};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-f3-{name}-{}", std::process::id()));
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

fn bench<'a>(runs: &'a [BenchRun], name: &str) -> &'a BenchRun {
    runs.iter().find(|run| run.name == name).unwrap_or_else(|| panic!("no bench {name}"))
}

fn value(run: &BenchRun, step: usize, name: &str) -> f64 {
    let k = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[k].value
}

const OPAMP: &str = "part OPAMP {
    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 10MHz; open_loop_gain = 80dB; }
}";

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

/// A non-inverting amplifier of gain 1 + RF / RG. With `positive`, the
/// divider feeds the non-inverting input instead (and the input drives the
/// inverting one): positive feedback.
fn amplifier(positive: bool) -> String {
    let (plus, minus) = if positive { ("FB", "IN") } else { ("IN", "FB") };
    format!(
        "board Amp {{
    use library \"tiny\";
    import p \"./parts\";
    param RF_VALUE = 9kohm;
    component U1: p.OPAMP;
    component RF: RESISTOR {{ value = RF_VALUE; }}
    component RG: RESISTOR {{ value = 1kohm; }}
    component J1: p.HEADER;
    net {plus} {{ U1.INP; }}
    net {minus} {{ U1.INN; }}
    net IN {{ J1.1; }}
    net FB {{ RF.2; RG.1; }}
    net OUT {{ U1.OUT; RF.1; }}
    net GND {{ J1.2; RG.2; }}
    net VP {{ U1.VP; }}
    net VN {{ U1.VN; }}
    supply POS {{ voltage = 5V; net = VP; external = true; }}
    supply NEG {{ voltage = -5V; net = VN; external = true; }}
    supply GROUND {{ voltage = 0V; net = GND; external = true; }}
}}"
    )
}

const BENCH: &str = "bench Margins for Amp {
    replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
    step RF_VALUE in 0.001ohm, 9kohm;
    analysis gain: loop { from = 10Hz; to = 1GHz; points_per_decade = 50; probe = U1.OUT; }
    measure phase_margin = phase_margin(loop_gain()) { min = 45deg; }
    measure crossover = crossover(loop_gain());
    measure gain_margin = gain_margin(loop_gain());
    measure low = mag(at(loop_gain(), 10Hz));
    analysis poles: pz { input = J1; output = V(OUT); }
    measure damping = damping();
}";

#[test]
fn loop_gain_and_poles_of_a_single_pole_amplifier() {
    let board = project(
        "amp",
        &[
            ("board.copper", &amplifier(false)),
            ("parts/opamp.copper", OPAMP),
            ("parts/header.copper", HEADER),
            ("benches/margins.copper", BENCH),
        ],
    );
    let runs = run(&board);
    let gain = bench(&runs, "Margins.gain");
    assert_eq!(gain.status(), Status::Pass, "{:?}", gain.error);
    let (a0, gbw): (f64, f64) = (1e4, 10e6);
    for (step, rf) in [(0, 1e-3f64), (1, 9e3)] {
        let beta = 1e3 / (1e3 + rf);
        let k = a0 * beta;
        let fc = gbw / a0 * (k * k - 1.0).sqrt();
        let pm = 180.0 - (k * k - 1.0).sqrt().atan().to_degrees();
        let crossover = value(gain, step, "crossover");
        assert!((crossover / fc - 1.0).abs() < 1e-4, "step {step}: {crossover} {fc}");
        assert!((value(gain, step, "phase_margin") - pm).abs() < 0.01);
        assert!(gain.steps[step].measures[2].never, "one pole: no phase crossover");
        // T at 10 Hz: A0 beta / (1 + j f / f1).
        let f1 = gbw / a0;
        let want = k / (1.0 + (10.0 / f1).powi(2)).sqrt();
        assert!((value(gain, step, "low") / want - 1.0).abs() < 1e-9);
    }
    // The default Bode plot.
    assert_eq!(gain.plots.len(), 2);
    assert_eq!(gain.plots[0].expression, "db(loop_gain())");
    // Poles: the closed loop of a single pole is one real pole at -2 pi f1 (1 + A0 beta).
    let poles = bench(&runs, "Margins.poles");
    assert_eq!(poles.status(), Status::Pass, "{:?}", poles.error);
    for (step, rf) in [(0, 1e-3f64), (1, 9e3)] {
        let pz = poles.steps[step].pole_zero.as_ref().unwrap();
        let beta = 1e3 / (1e3 + rf);
        let want = -2.0 * PI * gbw / a0 * (1.0 + a0 * beta);
        let real: Vec<f64> = pz.result.poles.iter().map(|p| p.value.re).collect();
        assert_eq!(real.len(), 1, "{:?}", pz.result.poles);
        assert!((real[0] / want - 1.0).abs() < 1e-9, "{real:?} {want}");
        assert!(pz.result.zeros.is_empty(), "V(OUT) / J1 of this model has no finite zero");
        assert_eq!(value(poles, step, "stable"), 1.0);
    }
    let out = std::env::temp_dir().join(format!("copper-simulate-f3-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    copper_simulate::write_outputs(&runs, "Amp", &out).unwrap();
    let json = std::fs::read_to_string(out.join("sim-results.json")).unwrap();
    assert!(json.contains("\"kind\": \"loop\"") && json.contains("\"probe\": \"U1.OUT\""), "loop analysis");
    assert!(json.contains("\"pole_zero\"") && json.contains("\"right_half_plane\": false"), "pole table");
    let page = std::fs::read_to_string(out.join("Margins.poles.html")).unwrap();
    assert!(page.contains("pole map") && page.contains("<h2>Poles at"), "the pole map and table");
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("poles at RF_VALUE = 9 kohm: 1 finite, none in the right half-plane"), "{text}");
}

#[test]
fn positive_feedback_is_unstable() {
    let bench_source = "bench Latch for Amp {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
        analysis pz;
    }";
    let board = project(
        "latch",
        &[
            ("board.copper", &amplifier(true)),
            ("parts/opamp.copper", OPAMP),
            ("parts/header.copper", HEADER),
            ("benches/latch.copper", bench_source),
        ],
    );
    let runs = run(&board);
    let latch = bench(&runs, "Latch");
    assert_eq!(latch.status(), Status::Fail);
    assert_eq!(value(latch, 0, "stable"), 0.0);
    let pz = latch.steps[0].pole_zero.as_ref().unwrap();
    let pole = pz.result.poles.iter().find(|p| p.right_half_plane()).unwrap();
    // s = w1 (A0 beta - 1) with beta = 0.1.
    let want = 2.0 * PI * 1e3 * (1e4 * 0.1 - 1.0);
    assert!((pole.value.re / want - 1.0).abs() < 1e-9, "{pole:?}");
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("UNSTABLE") && text.contains("right half-plane: real, +"), "{text}");
}

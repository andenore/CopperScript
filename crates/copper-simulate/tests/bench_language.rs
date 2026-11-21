//! The bench language of phase F2 through the whole chain: "never" from
//! `upto` and `downto`, optional fixture components (nets of one name, 0 F
//! capacitors, fixtures that use fixtures), `if(...)` and lets that are not
//! read, several analyses in one bench, several noise outputs and the noise
//! of a derived quantity, matched groups of model parameters, parameter sets
//! per supply, `at(x, f)` solved at `f`, `I()` of a supply, the stepped
//! analysis bounds and `Gohm`. Expected values are closed forms written out
//! beside them.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use copper_lang::bench::load_benches;
use copper_lang::compiler::{Design, compile_design_file};
use copper_sim::noise::{BOLTZMANN, DEFAULT_TEMPERATURE};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

/// A project directory with `files` (relative path, source).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-f2-{name}-{}", std::process::id()));
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

fn index(run: &BenchRun, name: &str) -> usize {
    run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"))
}

fn value(run: &BenchRun, step: usize, name: &str) -> f64 {
    run.steps[step].measures[index(run, name)].value
}

fn rel(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

/// R1 = 1 kohm from VIN to VOUT, C1 = 100 nF from VOUT to ground; J1 is the input.
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

fn rc_project(name: &str, benches: &[(&str, &str)]) -> PathBuf {
    let mut files = vec![("board.copper", RC), ("parts/header.copper", HEADER)];
    files.extend_from_slice(benches);
    project(name, &files)
}

/// The RC response at `f`: `1 / (1 + j 2 pi f R C)`.
fn rc(f: f64) -> (f64, f64) {
    let x = 2.0 * PI * f * 1e3 * 100e-9;
    (1.0 / (1.0 + x * x).sqrt(), -x.atan().to_degrees())
}

const SOURCE: &str = "replace J1 with source { out = J1.1; ref = GND; ac = 1V; }";

#[test]
fn upto_that_never_holds_is_never() {
    // |H| > 2 holds nowhere, so it fails at the first sample: "never", not an error.
    let body = format!(
        "bench Never for RC {{
        {SOURCE}
        analysis ac {{ from = 10Hz; to = 100kHz; points_per_decade = 10; }}
        let h = V(VOUT) / V(VIN);
        measure bandwidth = upto(mag(h) > 0.70710678);
        measure never_up = upto(mag(h) > 2);
        measure never_down = downto(mag(h) > 0.9);
        measure scaled = upto(mag(h) > 2) / 2;
        plot mag(h);
    }}"
    );
    let board = rc_project("never", &[("benches/never.copper", &body)]);
    let runs = run(&board);
    let r = bench(&runs, "Never");
    assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass));
    assert!(rel(value(r, 0, "bandwidth"), 1.0 / (2.0 * PI * 1e-4)) < 2e-3);
    for name in ["never_up", "never_down", "scaled"] {
        let m = &r.steps[0].measures[index(r, name)];
        assert!(m.never && m.value.is_nan() && m.pass.is_none(), "{name}: {m:?}");
    }
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("never_up    never"), "{text}");
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"never\": true") && json.contains("\"value\": null"), "{json}");
    // A limit fails on "never": the step has no frequency where the condition holds.
    let limited =
        body.replace("measure never_up = upto(mag(h) > 2);", "measure never_up = upto(mag(h) > 2) { min = 1kHz; }");
    let board = rc_project("never-limited", &[("benches/never.copper", &limited)]);
    let runs = run(&board);
    let r = bench(&runs, "Never");
    assert_eq!(r.status(), Status::Fail);
    assert_eq!(r.steps[0].measures[index(r, "never_up")].pass, Some(false));
    assert!(copper_simulate::results::text_table(&runs).contains("FAIL"), "never fails its limit");
}

#[test]
fn optional_components_in_fixtures() {
    // A fixture holds a stray C_X on VOUT and its own net; a bench adds a pin to
    // the fixture's net (C_Y on VOUT) and sets C_X to 0 F (absent).
    let fixture = "fixture Strays for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
        param C_X = 100nF;
        component CX: CAPACITOR { value = C_X; }
        net VOUT { CX.1; }
        net GND { CX.2; }
    }";
    let wider = "fixture Wider for RC using Strays {
        param C_X = 0pF;                     // overrides the fixture it uses
    }";
    let added = "bench Added for RC using Wider {
        component CY: CAPACITOR { value = 100nF; }
        net VOUT { CY.1; }                   // the fixture's net VOUT, one more pin
        net GND { CY.2; }
        analysis ac { from = 100Hz; to = 10kHz; points_per_decade = 10; }
        step C_X in 0pF, 100nF;
        let h = V(VOUT) / V(VIN);
        measure gain = mag(at(h, 1kHz));
    }";
    let both = "bench Both for RC using Strays, Wider {
        analysis ac { from = 100Hz; to = 10kHz; points_per_decade = 10; }
        let h = V(VOUT) / V(VIN);
        measure gain = mag(at(h, 1kHz));
    }";
    let board = rc_project(
        "fixtures",
        &[
            ("benches/strays.copper", fixture),
            ("benches/wider.copper", wider),
            ("benches/added.copper", added),
            ("benches/both.copper", both),
        ],
    );
    let runs = run(&board);
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
    }
    // C_X = 0: C1 + CY = 200 nF; C_X = 100 nF: 300 nF.
    let gain = |c: f64| 1.0 / (1.0 + (2.0 * PI * 1e3 * 1e3 * c).powi(2)).sqrt();
    let r = bench(&runs, "Added");
    assert!(rel(value(r, 0, "gain"), gain(200e-9)) < 1e-9, "{}", value(r, 0, "gain"));
    assert!(rel(value(r, 1, "gain"), gain(300e-9)) < 1e-9);
    assert_eq!(r.models.iter().find(|m| m.name == "CX").map(|m| m.model.as_str()), Some("capacitor (0 F: absent)"));
    // Strays then Wider: Strays once, Wider's C_X in its place.
    let r = bench(&runs, "Both");
    assert!(rel(value(r, 0, "gain"), gain(100e-9)) < 1e-9);
    let benches = load_benches(&board, &compile(&board)).unwrap();
    let added = benches.iter().find(|b| b.name == "Added").unwrap();
    assert_eq!(added.fixtures, ["Strays", "Wider"]);
    // A fixture that uses itself through another is an error.
    let cycle = rc_project(
        "fixtures-cycle",
        &[
            ("benches/a.copper", "fixture A for RC using B { param X = 1V; }"),
            ("benches/b.copper", "fixture B for RC using A { param Y = 1V; }"),
            ("benches/c.copper", "bench C for RC using A { analysis op; }"),
        ],
    );
    let failure = simulate(&cycle, &compile(&cycle), None).unwrap_err().to_string();
    assert!(failure.contains("SIM073") && failure.contains("uses itself"), "{failure}");
}

#[test]
fn conditionals_and_lets_that_nothing_reads() {
    let body = format!(
        "bench Choose for RC {{
        {SOURCE}
        analysis ac {{ from = 10Hz; to = 100kHz; points_per_decade = 10; }}
        let h = V(VOUT) / V(VIN);
        let broken = V(VIN) / (V(VOUT) - V(VOUT));     // 1 / 0: never read, never evaluated
        measure passband = max(if(mag(h) > 0.5, mag(h), 0));
        measure stopband = max(if(mag(h) > 0.5, 0, mag(h)));
        measure lazy = if(R_VALUE > 2kohm, R_VALUE / (R_VALUE - R_VALUE), 2);
        measure guarded = max(if(mag(h) < 2, 1, mag(V(VOUT) / (V(VOUT) - V(VOUT)))));
        plot mag(h);
    }}"
    );
    let board = rc_project("choose", &[("benches/choose.copper", &body)]);
    let runs = run(&board);
    let r = bench(&runs, "Choose");
    assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass));
    assert!(rel(value(r, 0, "passband"), rc(10.0).0) < 1e-9);
    // The largest gain below 0.5: the first sample past the corner where |H| < 0.5.
    assert!(value(r, 0, "stopband") < 0.5 && value(r, 0, "stopband") > 0.4);
    assert_eq!(value(r, 0, "lazy"), 2.0);
    // The other branch's 0 / 0 is never chosen.
    assert_eq!(value(r, 0, "guarded"), 1.0);
    // A let that a measure reads is evaluated, and its failure is the measure's.
    let reading = body.replace("plot mag(h);", "plot mag(h);\n measure bad = max(mag(broken));");
    let board = rc_project("choose-read", &[("benches/choose.copper", &reading)]);
    let runs = run(&board);
    let error = bench(&runs, "Choose").error.clone().unwrap_or_default();
    assert!(error.contains("measure bad: let broken:") && error.contains("not finite"), "{error}");
}

#[test]
fn several_analyses_in_one_bench() {
    let body = format!(
        "bench Response for RC {{
        {SOURCE}
        let h = V(VOUT) / V(VIN);           // shared
        let unused_dc = V(VOUT) * 2;         // read by no section
        analysis op;
        measure dc_out = V(VOUT);
        analysis wide: ac {{ from = 10Hz; to = 100kHz; points_per_decade = 10; }}
        measure corner = cross(db(h), -3.0103dB);
        plot mag(h);
        analysis narrow: ac {{ from = 1kHz; to = 2kHz; points_per_decade = 50; }}
        measure gain_1k = mag(at(h, 1kHz));
    }}"
    );
    let board = rc_project("sections", &[("benches/response.copper", &body)]);
    let runs = run(&board);
    let names: Vec<&str> = runs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Response.op", "Response.wide", "Response.narrow"]);
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
        assert_eq!(r.bench_name(), "Response");
    }
    assert_eq!(bench(&runs, "Response.op").measures.len(), 1);
    assert!(rel(value(bench(&runs, "Response.wide"), 0, "corner"), 1.0 / (2.0 * PI * 1e-4)) < 1e-2);
    assert!(rel(value(bench(&runs, "Response.narrow"), 0, "gain_1k"), rc(1e3).0) < 1e-9);
    // `--bench Response` runs every section.
    let design = compile(&board);
    assert_eq!(simulate(&board, &design, Some("Response")).unwrap().len(), 3);
    assert_eq!(simulate(&board, &design, Some("Response.wide")).unwrap().len(), 1);
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"section\": \"wide\"") && json.contains("\"bench\": \"Response\""), "{json}");
    // Mistakes: two unnamed analyses of one kind; a measure before the first analysis.
    for (bad, code) in [
        ("analysis op; measure a = V(VOUT); analysis op; measure b = V(VOUT);", "SIM097"),
        (
            "measure a = V(VOUT); analysis op; analysis wide: ac { from = 1Hz; to = 1kHz; points_per_decade = 1; }",
            "SIM097",
        ),
        ("ic C1 = 1V; analysis op; analysis wide: ac { from = 1Hz; to = 1kHz; points_per_decade = 1; }", "SIM075"),
    ] {
        let source = format!("bench B for RC {{ {SOURCE} {bad} }}");
        let failure = copper_lang::bench::compile_benches_source(&source, "b.copper", &design).unwrap_err().to_string();
        assert!(failure.contains(code), "{bad}: {failure}");
    }
}

#[test]
fn several_noise_outputs_and_the_noise_of_a_ratio() {
    // The source's 1 kohm output resistance and R1 are the generators. Every
    // generator at VIN reaches VOUT through H, so the ratio V(VOUT) / V(VIN)
    // does not see the source's noise at all; R1's noise it does.
    let single = "bench One for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; output_resistance = 1kohm; }
        analysis noise { from = 10Hz; to = 1MHz; points_per_decade = 20; output = V(VOUT); input = J1; }
        measure out_rms = noise_rms(noise(), 100Hz, 100kHz);
        measure r1_share = noise_share(noise(R1), 100Hz, 100kHz);
    }";
    let several = "bench Two for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; output_resistance = 1kohm; }
        analysis noise { from = 10Hz; to = 1MHz; points_per_decade = 20; output = V(VOUT), V(VIN); input = J1; }
        let h = V(VOUT) / V(VIN);
        let same = V(VOUT) * 1;
        measure out_rms = noise_rms(noise(V(VOUT)), 100Hz, 100kHz);
        measure r1_share = noise_share(noise(V(VOUT), R1), 100Hz, 100kHz);
        measure in_rms = noise_rms(noise(V(VIN)), 100Hz, 100kHz);
        measure same_rms = noise_rms(noise(same), 100Hz, 100kHz);
        measure ratio_rms = noise_rms(noise(h), 100Hz, 100kHz);
        measure ratio_source = noise_rms(noise(h, J1), 100Hz, 100kHz);
        measure ratio_r1 = noise_rms(noise(h, R1), 100Hz, 100kHz);
        measure ratio_at_1k = at(noise(h), 1kHz);
        measure ratio_between = at(noise(h), 1.234kHz);
        measure referred = at(input_noise(V(VOUT)), 1kHz);
        plot noise(V(VOUT)), noise(h);
    }";
    let board = rc_project("noise", &[("benches/one.copper", single), ("benches/two.copper", several)]);
    let runs = run(&board);
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
    }
    let (one, two) = (bench(&runs, "One"), bench(&runs, "Two"));
    // A declared output gives the numbers of the single-output bench, bit for bit.
    assert_eq!(value(one, 0, "out_rms"), value(two, 0, "out_rms"));
    assert_eq!(value(one, 0, "r1_share"), value(two, 0, "r1_share"));
    assert!(rel(value(two, 0, "same_rms"), value(two, 0, "out_rms")) < 1e-12);
    // The source's noise cancels in the ratio (round-off only); R1's is all of it.
    assert!(value(two, 0, "ratio_source") < 1e-9 * value(two, 0, "ratio_rms"), "{}", value(two, 0, "ratio_source"));
    assert!(rel(value(two, 0, "ratio_r1"), value(two, 0, "ratio_rms")) < 1e-9);
    // By hand, with Rs = R1 = 1 kohm and C = 100 nF: the signal is Vin = Vs (R1 + Zc) / T,
    // Vout = Vs Zc / T with T = Rs + R1 + Zc, so H = Zc / (R1 + Zc). R1's noise, as a
    // current i across it, drives the loop Rs -> R1 -> C with i R1 / T: dVin = -i R1 Rs / T,
    // dVout = i R1 Zc / T, and dH = (dVout - H dVin) / Vin, times sqrt(4 k T / R1).
    let kt = BOLTZMANN * DEFAULT_TEMPERATURE;
    let f = 1e3;
    let w = 2.0 * PI * f;
    let (rs, r1, c) = (1e3, 1e3, 100e-9);
    let zc = num_complex(0.0, -1.0 / (w * c));
    let total = num_complex(rs + r1, 0.0).add(zc);
    let i_loop = num_complex(r1, 0.0).div(total);
    let d_vin = i_loop.scale(-rs);
    let d_vout = i_loop.mul(zc);
    let vin = num_complex(r1, 0.0).add(zc).div(total);
    let vout = zc.div(total);
    let h = vout.div(vin);
    let dh = d_vout.div(vin).sub(h.mul(d_vin).div(vin));
    let expected = dh.abs() * (4.0 * kt / r1).sqrt();
    assert!(rel(value(two, 0, "ratio_at_1k"), expected) < 1e-6, "{} vs {expected}", value(two, 0, "ratio_at_1k"));
    // Between samples, at(x, f) solves the noise at f: the same closed form at 1.234 kHz.
    let w = 2.0 * PI * 1.234e3;
    let zc = num_complex(0.0, -1.0 / (w * c));
    let total = num_complex(rs + r1, 0.0).add(zc);
    let i_loop = num_complex(r1, 0.0).div(total);
    let (d_vin, d_vout) = (i_loop.scale(-rs), i_loop.mul(zc));
    let (vin, vout) = (num_complex(r1, 0.0).add(zc).div(total), zc.div(total));
    let h = vout.div(vin);
    let dh = d_vout.div(vin).sub(h.mul(d_vin).div(vin));
    let expected = dh.abs() * (4.0 * kt / r1).sqrt();
    assert!(rel(value(two, 0, "ratio_between"), expected) < 1e-9, "{} vs {expected}", value(two, 0, "ratio_between"));
    // Referred to the source: Rs and R1 in series, 2 kohm, through the same divider as the
    // signal, so sqrt(4 k T 2 kohm) at every frequency (the ratio h has no gain from the
    // source at all, and the bench reads its noise all the same: it is not referred to it).
    assert!(rel(value(two, 0, "referred"), (4.0 * kt * 2e3).sqrt()) < 1e-9, "{}", value(two, 0, "referred"));
    // One contributor table per output.
    assert_eq!(two.steps[0].noise.iter().map(|n| n.output.as_str()).collect::<Vec<_>>(), ["V(VOUT)", "V(VIN)"]);
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"output\": \"V(VIN)\""), "{json}");
    // With several outputs, noise() must name its quantity.
    let design = compile(&board);
    let source = several.replace("measure out_rms = noise_rms(noise(V(VOUT))", "measure out_rms = noise_rms(noise()");
    let failure = copper_lang::bench::compile_bench_source(&source, "b.copper", &design).unwrap_err().to_string();
    assert!(failure.contains("SIM095"), "{failure}");
    let source = several.replace("let same = V(VOUT) * 1;", "let same = mag(V(VOUT));");
    let failure = copper_lang::bench::compile_bench_source(&source, "b.copper", &design).unwrap_err().to_string();
    assert!(failure.contains("SIM095") && failure.contains("mag(...)"), "{failure}");
}

/// A small complex type for the closed forms.
#[derive(Clone, Copy)]
struct C(f64, f64);

fn num_complex(re: f64, im: f64) -> C {
    C(re, im)
}

impl C {
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn sub(self, o: C) -> C {
        C(self.0 - o.0, self.1 - o.1)
    }
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn scale(self, k: f64) -> C {
        C(self.0 * k, self.1 * k)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// A two-resistor network as one part: two `load` blocks.
const NETWORK: &str = "part NET2 {
    category = \"passive.network\";
    pin A1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin B1 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
    pin A2 { number = \"3\"; domains = \"analog\"; directions = \"passive\"; }
    pin B2 { number = \"4\"; domains = \"analog\"; directions = \"passive\"; }
    sim load R1 { inp = A1; ref = B1; resistance = 10kohm; }
    sim load R2 { inp = A2; ref = B2; resistance = 10kohm; }
}";

fn divider_board(attributes: &str) -> String {
    format!(
        "board Div {{
    use library \"tiny\";
    import p \"./parts\";
    component RN: p.NET2 {{ {attributes} }}
    net VIN {{ RN.A1; }}
    net MID {{ RN.B1; RN.A2; }}
    net GND {{ RN.B2; }}
    supply IN {{ voltage = 2V; net = VIN; external = true; }}
    supply GROUND {{ voltage = 0V; net = GND; external = true; }}
}}"
    )
}

#[test]
fn matched_groups_of_model_parameters() {
    // A divider of the network's two units: 10 % absolute, 0.1 % ratio. The
    // ratio of the units moves at most 0.1 %, so V(MID) = 2 V R2 / (R1 + R2)
    // stays within 1 V (1 +- 0.0005).
    let statement = "bench Bench for Div {
        analysis op;
        tolerance RN.resistance = 10% match 0.1%;
        worstcase;
        measure mid = V(MID);
        measure current = I(IN);
    }";
    let attribute = "bench Attribute for Div {
        analysis op;
        worstcase;
        measure mid = V(MID);
    }";
    let board = divider_board("");
    let path = project(
        "matched",
        &[("board.copper", &board), ("parts/net2.copper", NETWORK), ("benches/statement.copper", statement)],
    );
    let runs = run(&path);
    let r = bench(&runs, "Bench");
    assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass));
    let variation = r.variation.as_ref().unwrap();
    assert_eq!(
        variation.variables,
        ["RN.resistance common", "RN.resistance RN.R1.resistance", "RN.resistance RN.R2.resistance"]
    );
    let stats = &variation.steps[0].measures[index(r, "mid")];
    // The worst corners: R1 0.05 % below the common deviation and R2 0.05 % above (and
    // back): V(MID) moves by 0.05 % over (1 +- 9.95 %), never by the 10 % absolute.
    let corner = |a: f64, b: f64| 2.0 * (1.0 + b) / ((1.0 + a) + (1.0 + b));
    for (extreme, sign) in [(stats.max, 1.0), (stats.min, -1.0)] {
        let shift = sign * (extreme - 1.0);
        assert!(shift > 0.0005 / 1.0995 * (1.0 - 1e-9) && shift < 0.0005 / 0.9005 * (1.0 + 1e-9), "{extreme}");
    }
    // The current follows the absolute tolerance: 2 V / 20 kohm at +-10 %.
    let current = &variation.steps[0].measures[index(r, "current")];
    assert!(rel(current.max, 2.0 / (20e3 * 0.9)) < 1e-9 && rel(current.min, 2.0 / (20e3 * 1.1)) < 1e-9);
    // The same group declared on the component: its units are the members.
    let board = divider_board("tolerance = 10%; match = \"NET\"; match_tolerance = 0.1%;");
    let path = project(
        "matched-attribute",
        &[("board.copper", &board), ("parts/net2.copper", NETWORK), ("benches/attribute.copper", attribute)],
    );
    let runs = run(&path);
    let a = bench(&runs, "Attribute");
    assert_eq!((a.error.as_deref(), a.status()), (None, Status::Pass));
    let stats_a = &a.variation.as_ref().unwrap().steps[0].measures[0];
    assert!(rel(stats_a.max, stats.max) < 1e-12 && rel(stats_a.min, stats.min) < 1e-12);
    // Without match, each unit varies on its own at 10 %.
    let board = divider_board("tolerance = 10%;");
    let path = project(
        "unmatched-attribute",
        &[("board.copper", &board), ("parts/net2.copper", NETWORK), ("benches/attribute.copper", attribute)],
    );
    let runs = run(&path);
    let stats = &bench(&runs, "Attribute").variation.as_ref().unwrap().steps[0].measures[0];
    assert!(rel(stats.max, corner(-0.1, 0.1)) < 1e-9, "{}", stats.max);
    // Mistakes: an absolute matched tolerance, a match above the tolerance, a group of one.
    let board = divider_board("");
    let path = project("matched-mistakes", &[("board.copper", &board), ("parts/net2.copper", NETWORK)]);
    let design = compile(&path);
    for (statement, code) in [
        ("tolerance RN.resistance = 1kohm match 0.1%;", "SIM091"),
        ("tolerance RN.resistance = 1% match 2%;", "SIM092"),
        ("tolerance RN.R1.resistance = 1% match 0.1%;", "SIM092"),
        ("tolerance RN.R1.resistance, RN.R2.resistance = 1%;", "SIM091"),
    ] {
        let source = format!("bench B for Div {{ analysis op; worstcase; {statement} measure m = V(MID); }}");
        let failure = copper_lang::bench::compile_bench_source(&source, "b.copper", &design).unwrap_err().to_string();
        assert!(failure.contains(code), "{statement}: {failure}");
    }
}

/// An op-amp whose output swing depends on its supply.
const SWING_AMP: &str = "part AMP {
    category = \"amplifier.opamp\";
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin INP { number = \"3\"; domains = \"analog\"; directions = \"input\"; }
    pin VN  { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"6\"; domains = \"analog\"; directions = \"output\"; }
    pin VP  { number = \"7\"; domains = \"power\"; directions = \"input\"; }
    sim opamp {
        inp = INP; inn = INN; out = OUT; vp = VP; vn = VN;
        gbw = 1MHz; open_loop_gain = 100dB; output_swing_from_rail = 0.5V;
        supply 3.3V { output_swing_from_rail = 0.1V; }
        supply 10V { output_swing_from_rail = 1V; }
    }
}";

/// A follower U1 on VP and VN, driven from J2.
const FOLLOWER: &str = "board Follower {
    use library \"tiny\";
    import p \"./parts\";
    component U1: p.AMP;
    component J2: p.HEADER;
    net IN { J2.1; U1.INP; }
    net OUT { U1.OUT; U1.INN; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { J2.2; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

#[test]
fn parameter_sets_per_supply() {
    let split = "bench Split for Follower {
        replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
        analysis dc { sweep = J2; from = 0V; to = 6V; step = 0.05V; }
        measure high = max(V(OUT));
    }";
    let single = "bench Single for Follower {
        replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
        replace POS with source { out = VP; ref = GND; dc = 3.3V; }
        replace NEG with source { out = VN; ref = GND; dc = 0V; }
        analysis dc { sweep = J2; from = 0V; to = 4V; step = 0.05V; }
        measure high = max(V(OUT));
    }";
    let path = project(
        "sets",
        &[
            ("board.copper", FOLLOWER),
            ("parts/header.copper", HEADER),
            ("parts/amp.copper", SWING_AMP),
            ("benches/split.copper", split),
            ("benches/single.copper", single),
        ],
    );
    let runs = run(&path);
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
    }
    // +-5 V (span 10 V): 1 V from the rail; 3.3 V: 0.1 V (the clamp's knee: 20 mV).
    let high = |name: &str| value(bench(&runs, name), 0, "high");
    assert!((high("Split") - 4.0).abs() < 0.02, "{}", high("Split"));
    assert!((high("Single") - 3.2).abs() < 0.02, "{}", high("Single"));
    let model = |name: &str| bench(&runs, name).models.iter().find(|m| m.name == "U1").unwrap().model.clone();
    assert_eq!(model("Split"), "opamp (parameter set for a 10 V supply)");
    assert_eq!(model("Single"), "opamp (parameter set for a 3.3 V supply)");
    // The part JSON lists the sets; a set on a kind without supply pins is an error.
    let design = compile(&path);
    let part = design.electrical.library.values().find(|part| part.name.ends_with("AMP")).unwrap();
    assert_eq!(part.sim[0].supply_sets.len(), 2);
    let bad = "part LOADED {
    category = \"passive.network\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
    sim load { inp = 1; ref = 2; resistance = 1kohm; supply 3.3V { resistance = 2kohm; } }
}";
    let board = FOLLOWER
        .replace("component J2: p.HEADER;", "component J2: p.HEADER;\n    component RX: p.LOADED;")
        .replace("net GND { J2.2; }", "net GND { J2.2; RX.1; RX.2; }");
    let path = project(
        "sets-bad",
        &[
            ("board.copper", &board),
            ("parts/header.copper", HEADER),
            ("parts/amp.copper", SWING_AMP),
            ("parts/loaded.copper", bad),
        ],
    );
    let failure = compile_design_file(&path, false, true).unwrap_err().to_string();
    assert!(failure.contains("SIM096"), "{failure}");
}

#[test]
fn at_solves_between_samples_and_supplies_deliver_current() {
    // 10 points per decade: 1.5915 kHz lies between samples; at() solves there.
    let body = "bench At for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
        analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }
        let h = V(VOUT) / V(VIN);
        measure gain = db(at(h, 1.5915kHz));
        measure phase_between = at(phase(h), 1.5915kHz);
        measure on_sample = mag(at(h, 1kHz));
    }";
    let current = "bench Current for RC {
        replace J1 with source { out = J1.1; ref = GND; dc = 2V; }
        component RL: RESISTOR { value = 1Gohm; }
        net VOUT { RL.1; }
        net GND { RL.2; }
        supply LOAD { voltage = 1V; net = VOUT; }
        analysis op;
        measure delivered = I(LOAD);
        measure through_r1 = I(R1);
        measure gigaohm = I(RL);
    }";
    let board = rc_project("at", &[("benches/at.copper", body), ("benches/current.copper", current)]);
    let runs = run(&board);
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
    }
    let r = bench(&runs, "At");
    let (mag, phase) = rc(1591.5);
    assert!((value(r, 0, "gain") - 20.0 * mag.log10()).abs() < 1e-12, "{}", value(r, 0, "gain"));
    assert!((value(r, 0, "phase_between") - phase).abs() < 1e-10, "{}", value(r, 0, "phase_between"));
    assert!(rel(value(r, 0, "on_sample"), rc(1e3).0) < 1e-12);
    // 2 V into R1 against the 1 V supply on VOUT: 1 mA flows into the supply, so it delivers -1 mA.
    let r = bench(&runs, "Current");
    assert!(rel(value(r, 0, "through_r1"), 1e-3) < 1e-12);
    assert!(rel(value(r, 0, "delivered"), -1e-3 + 1e-9) < 1e-9, "{}", value(r, 0, "delivered"));
    assert!(rel(value(r, 0, "gigaohm"), 1e-9) < 1e-9);
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"expression\": \"I(LOAD)\""), "{json}");
}

#[test]
fn stepped_analysis_bounds_are_shown() {
    let body = "bench Bounds for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
        param F_TOP = 100kHz;
        analysis ac { from = 10Hz; to = F_TOP; points_per_decade = 5; }
        step F_TOP in 1kHz, 10kHz;
        measure top = max(mag(V(VOUT)));
    }";
    let board = rc_project("bounds", &[("benches/bounds.copper", body)]);
    let runs = run(&board);
    let r = bench(&runs, "Bounds");
    assert_eq!(r.error, None);
    assert_eq!(r.steps[1].axis.last().copied(), Some(1e4));
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("ac 10 Hz to 1 kHz..10 kHz, 5 points per decade, 2 steps"), "{text}");
    let json = copper_simulate::results::results_json(&runs, "RC");
    let compact: String = json.split_whitespace().collect();
    // A bound that differs between steps is null on the bench, with each step's value on the step.
    assert!(compact.contains("\"to_hz\":null") && compact.contains("\"stepped\":[\"to_hz\"]"), "{json}");
    assert!(
        compact.contains("\"analysis\":{\"to_hz\":1000.0}") && compact.contains("\"analysis\":{\"to_hz\":10000.0}"),
        "{json}"
    );
}

//! Limits scoped to steps (`where`), fixtures, `upto`/`downto` and windowed
//! reductions, dc sweeps, initial conditions, `I(...)` of models a bench
//! places, noise of a diode and the report's limit lines and noise axes,
//! through the whole chain on small projects written to a temporary
//! directory. Expected values are closed forms written out beside them.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};

use copper_lang::compiler::{Design, compile_design_file};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

/// A project directory with `files` (relative path, source).
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-scopes-{name}-{}", std::process::id()));
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

fn rel(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

const HEADER: &str = "part HEADER {
    category = \"connector.header\";
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"ground\"; directions = \"passive\"; }
}";

const DIODE: &str = "part DIODE {
    category = \"semiconductor.diode\";
    pin A { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin K { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
    sim diode { anode = A; cathode = K; saturation_current = 10fA; series_resistance = 10ohm; }
}";

const OPAMP: &str = "part OPAMP {
    category = \"amplifier.opamp\";
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin INP { number = \"3\"; domains = \"analog\"; directions = \"input\"; }
    pin VN  { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"6\"; domains = \"analog\"; directions = \"output\"; }
    pin VP  { number = \"7\"; domains = \"power\"; directions = \"input\"; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; output_swing_from_rail = 0.5V; }
}";

/// R1 from VIN to VOUT, C1 = 100 nF from VOUT to ground; J1 is the input connector.
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

/// A diode D1 from A to ground, driven through J1, and a non-inverting
/// amplifier U1 of gain 1 + R_F / R_G on +-5 V from J2.
const ANALOG: &str = "board Analog {
    use library \"tiny\";
    import p \"./parts\";
    param R_F = 10kohm;
    component D1: p.DIODE;
    component J1: p.HEADER;
    component J2: p.HEADER;
    component U1: p.OPAMP;
    component RF: RESISTOR { value = R_F; }
    component RG: RESISTOR { value = 10kohm; }
    net A { J1.1; D1.A; }
    net IN { J2.1; U1.INP; }
    net OUT { U1.OUT; RF.1; }
    net FB { U1.INN; RF.2; RG.1; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { J1.2; J2.2; D1.K; RG.2; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn rc_project(name: &str, benches: &[(&str, &str)]) -> PathBuf {
    let mut files = vec![("board.copper", RC), ("parts/header.copper", HEADER)];
    files.extend_from_slice(benches);
    project(name, &files)
}

fn analog_project(name: &str, benches: &[(&str, &str)]) -> PathBuf {
    let mut files = vec![
        ("board.copper", ANALOG),
        ("parts/header.copper", HEADER),
        ("parts/diode.copper", DIODE),
        ("parts/opamp.copper", OPAMP),
    ];
    files.extend_from_slice(benches);
    project(name, &files)
}

#[test]
fn limits_scoped_to_steps_and_the_bandwidth_functions() {
    // The RC low-pass corner is fc = 1 / (2 pi R C); its -3 dB point (not
    // -3.0103 dB) is fc sqrt(10^0.3 - 1). R = 1, 2, 4 kohm: the limit 700 Hz
    // to 1.7 kHz holds for the first two, so it is scoped to R_VALUE <= 2kohm.
    let scoped = "bench Corner for RC {
        replace J1 with source { out = J1.1; ref = GND; ac = 1V; }
        analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 20; }
        step R_VALUE in 1kohm, 2kohm, 4kohm;
        param PEAK_LIMIT = R_VALUE / 1kohm * 110%;
        let h = mag(V(VOUT)) / 1V;
        measure corner = upto(db(V(VOUT) / 1V) > -3dB) { min = 700Hz; max = 1.7kHz; where R_VALUE <= 2kohm; }
        measure corner_down = downto(db(V(VOUT) / 1V) < -3dB);
        measure band_max = max(h, 1kHz, 10kHz);
        measure band_mean = mean(h, 1kHz, 1kHz);
        measure peak = max(h) { max = PEAK_LIMIT; }
        measure flat = max(h) { max = 110%; }
        plot h;
    }";
    let unscoped = scoped.replace("bench Corner", "bench Unscoped").replace(" where R_VALUE <= 2kohm;", "");
    let nowhere = scoped.replace("bench Corner", "bench Nowhere").replace("R_VALUE <= 2kohm", "R_VALUE > 5kohm");
    let board = rc_project(
        "corner",
        &[
            ("benches/corner.copper", scoped),
            ("benches/unscoped.copper", &unscoped),
            ("benches/nowhere.copper", &nowhere),
        ],
    );
    let runs = run(&board);
    let r = bench(&runs, "Corner");
    assert_eq!(
        (r.error.as_deref(), r.status()),
        (None, Status::Pass),
        "{}",
        copper_simulate::results::text_table(&runs)
    );
    for (step, ohms) in [(0, 1e3), (1, 2e3), (2, 4e3)] {
        let fc = 1.0 / (2.0 * PI * ohms * 100e-9);
        let f3 = fc * (10f64.powf(0.3) - 1.0).sqrt();
        let (up, down) = (value(r, step, "corner"), value(r, step, "corner_down"));
        println!("R = {ohms} ohm: upto {up:.3} Hz, downto {down:.3} Hz, closed form {f3:.3} Hz");
        assert!(rel(up, f3) < 2e-4 && rel(down, f3) < 2e-4, "{up} {down} vs {f3}");
        // 1 kHz and 10 kHz are sweep points; the low-pass is largest at the window's start.
        let at_1k = 1.0 / (1.0 + (1e3 / fc).powi(2)).sqrt();
        assert!(rel(value(r, step, "band_max"), at_1k) < 1e-12 && rel(value(r, step, "band_mean"), at_1k) < 1e-12);
    }
    let k = r.measures.iter().position(|m| m.name == "corner").unwrap();
    let covered: Vec<bool> = r.steps.iter().map(|step| step.measures[k].covered).collect();
    assert_eq!(covered, [true, true, false]);
    assert_eq!(r.steps[2].measures[k].pass, None);
    assert_eq!(r.measures[k].scope.as_deref(), Some("R_VALUE <= 2kohm"));
    assert!(matches!(r.measures[k].worst, Some(0 | 1)));
    // Text, JSON and HTML name the steps the limit covers.
    let text = copper_simulate::results::text_table(&runs);
    assert!(text.contains("min 700 Hz, max 1.7 kHz where R_VALUE <= 2kohm (2 of 3 steps)"), "{text}");
    let json = copper_simulate::results::results_json(&runs, "RC");
    assert!(json.contains("\"where\": \"R_VALUE <= 2kohm\"") && json.contains("\"covered_steps\": [\n"), "{json}");
    assert!(json.contains("\"covered\": false"));
    let html = copper_simulate::report::html(r, "RC");
    assert!(html.contains("where R_VALUE &lt;= 2kohm (2 of 3 steps)") && html.contains("(no limit)"));
    // A limit that follows a param draws one line per step, an equal one a single line.
    let lines = html.matches("class=\"limit\"").count();
    assert_eq!(lines, 3 + 1, "{lines} limit lines");
    // Without the where the third step fails; a where that holds nowhere is an error.
    let unscoped = bench(&runs, "Unscoped");
    assert_eq!(unscoped.status(), Status::Fail);
    assert_eq!(unscoped.steps[2].measures[k].pass, Some(false));
    let nowhere = bench(&runs, "Nowhere");
    assert_eq!(nowhere.status(), Status::Error);
    assert!(nowhere.error.as_deref().unwrap_or_default().contains("holds at no step"), "{:?}", nowhere.error);
}

#[test]
fn fixtures_share_setup_and_benches_override_it() {
    // Source 1 V behind R1 = 1 kohm into a load R_D: V(VOUT) = R_D / (R1 + R_D).
    let fixture = "fixture Driven for RC {
        replace J1 with source { out = J1.1; ref = GND; dc = 1V; }
        param R_LOAD = 1kohm;
        param R_HALF = R_LOAD / 2;
        component R_D: RESISTOR { value = R_LOAD; }
        net VOUT { R_D.1; }
        net GND { R_D.2; }
    }";
    let plain = "bench Plain for RC using Driven { analysis op; measure v = V(VOUT); measure half = R_HALF; }";
    let load = "bench Load for RC using Driven { param R_LOAD = 3kohm; analysis op; measure v = V(VOUT); measure half = R_HALF; }";
    let drive = "bench Drive for RC using Driven {
        replace J1 with source { out = J1.1; ref = GND; dc = 2V; }
        analysis op;
        measure v = V(VOUT);
    }";
    let written = "bench Written for RC {
        replace J1 with source { out = J1.1; ref = GND; dc = 1V; }
        param R_LOAD = 1kohm;
        component R_D: RESISTOR { value = R_LOAD; }
        net VOUT { R_D.1; }
        net GND { R_D.2; }
        analysis op;
        measure v = V(VOUT);
    }";
    let board = rc_project(
        "fixtures",
        &[
            ("benches/driven.copper", fixture),
            ("benches/plain.copper", plain),
            ("benches/load.copper", load),
            ("benches/drive.copper", drive),
            ("benches/written.copper", written),
        ],
    );
    let runs = run(&board);
    assert_eq!(runs.len(), 4, "a fixture is not a bench");
    assert_eq!(value(bench(&runs, "Plain"), 0, "v"), 0.5);
    assert_eq!(value(bench(&runs, "Plain"), 0, "v"), value(bench(&runs, "Written"), 0, "v"));
    // The bench's R_LOAD takes the fixture's place: the derived R_HALF follows it.
    assert!(rel(value(bench(&runs, "Load"), 0, "v"), 0.75) < 1e-12);
    assert!(rel(value(bench(&runs, "Load"), 0, "half"), 1.5e3) < 1e-12);
    assert!(rel(value(bench(&runs, "Drive"), 0, "v"), 1.0) < 1e-12);

    // A fixture holds setup only; a bench names fixtures that exist.
    let design = compile(&board);
    let broken = fixture.replace("param R_LOAD", "analysis op;\n        param R_LOAD");
    let failures = [
        (broken.as_str(), plain, "SIM073", "analysis belongs in the bench"),
        (fixture, "bench Missing for RC using Drivn { analysis op; measure v = V(VOUT); }", "SIM073", "Driven"),
    ];
    for (fixture_source, bench_source, code, words) in failures {
        let fixture_document = copper_lang::parse(fixture_source, "fixture.copper").unwrap();
        let bench_document = copper_lang::parse(bench_source, "bench.copper").unwrap();
        let failure =
            copper_lang::bench::compile_bench_with_fixtures(&bench_document, &[fixture_document], &design).unwrap_err();
        let text = failure.to_string();
        assert!(
            text.contains(code) && (text.contains(words) || failure.hint.as_deref().unwrap_or("").contains(words)),
            "{text}"
        );
    }
}

#[test]
fn dc_sweeps_of_a_diode_an_amplifier_and_a_param() {
    let iv = "bench IV for Analog {
        replace J1 with source { out = J1.1; ref = GND; dc = 0V; }
        replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
        analysis dc { sweep = J1; from = 0.3V; to = 0.7V; step = 50mV; }
        measure i_600 = at(I(D1), 0.6V);
        measure i_max = max(I(D1));
        plot I(D1);
    }";
    let transfer = "bench Transfer for Analog {
        replace J1 with source { out = J1.1; ref = GND; dc = 0V; }
        replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
        analysis dc { sweep = J2; from = -4V; to = 4V; step = 0.1V; }
        measure gain = at(V(OUT), 1V) / 1V;
        measure high = max(V(OUT)) { min = 4.48V; max = 4.52V; }
        measure low = min(V(OUT));
        measure linear_to = upto(V(OUT) < 4.4V);
        plot V(OUT);
    }";
    let downward =
        transfer.replace("bench Transfer", "bench Downward").replace("from = -4V; to = 4V", "from = 4V; to = -4V");
    let param = "bench Gain for Analog {
        replace J1 with source { out = J1.1; ref = GND; dc = 0V; }
        replace J2 with source { out = J2.1; ref = GND; dc = 1V; }
        analysis dc { sweep = R_F; from = 10kohm; to = 30kohm; points = 3; }
        measure at_20k = at(V(OUT), 20kohm);
        plot V(OUT);
    }";
    let board = analog_project(
        "dc",
        &[
            ("benches/iv.copper", iv),
            ("benches/transfer.copper", transfer),
            ("benches/downward.copper", &downward),
            ("benches/gain.copper", param),
        ],
    );
    let runs = run(&board);
    println!("{}", copper_simulate::results::text_table(&runs));
    for r in &runs {
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{}", r.name);
    }
    // The diode behind its 10 ohm series resistance: I solves v = I R_S + V_T ln(1 + I / I_S).
    let vt = copper_sim::devices::THERMAL_VOLTAGE;
    let solve = |v: f64| {
        let (mut lo, mut hi): (f64, f64) = (0.0, v / 10.0);
        for _ in 0..200 {
            let i = 0.5 * (lo + hi);
            if i * 10.0 + vt * (i / 10e-15 + 1.0).ln() > v { hi = i } else { lo = i }
        }
        0.5 * (lo + hi)
    };
    let r = bench(&runs, "IV");
    assert!(rel(value(r, 0, "i_600"), solve(0.6)) < 1e-6, "{} vs {}", value(r, 0, "i_600"), solve(0.6));
    assert!(rel(value(r, 0, "i_max"), solve(0.7)) < 1e-6);
    assert_eq!(r.steps[0].axis.len(), 9);
    // Gain 2 / (1 + 2 / A0) inside the rails, 5 V - 0.5 V at the top (the clamp's knee: 20 mV).
    let gain = 2.0 / (1.0 + 2.0 / 1e5);
    for name in ["Transfer", "Downward"] {
        let r = bench(&runs, name);
        assert!(rel(value(r, 0, "gain"), gain) < 1e-6, "{name}");
        assert!((value(r, 0, "high") - 4.5).abs() < 0.02 && (value(r, 0, "low") + 4.5).abs() < 0.02, "{name}");
        // V(OUT) = 4.4 V at 4.4 V / gain, between samples 0.1 V apart where the next one is clipped.
        assert!(rel(value(r, 0, "linear_to"), 4.4 / gain) < 1e-4, "{name}: {}", value(r, 0, "linear_to"));
        assert!(r.steps[0].axis.windows(2).all(|w| w[1] > w[0]), "an increasing axis");
    }
    // A param sweep: gain 1 + R_F / 10 kohm at 1 V in.
    let r = bench(&runs, "Gain");
    assert!(rel(value(r, 0, "at_20k"), 3.0 / (1.0 + 3.0 / 1e5)) < 1e-6);
    // Outputs: the swept value as the axis.
    let json = copper_simulate::results::results_json(&runs, "Analog");
    assert!(json.contains("\"kind\": \"dc\"") && json.contains("\"quantity\": \"sweep\""));
    let csv = copper_simulate::results::plots_csv(bench(&runs, "IV"));
    assert!(csv.starts_with("step,label,sweep,"), "{}", &csv[..60]);
    let html = copper_simulate::report::html(bench(&runs, "IV"), "Analog");
    assert!(html.contains("data-axis=\"sweep\"") && html.contains(">J1<"));
}

#[test]
fn initial_conditions_of_a_transient_run() {
    // C1 = 100 nF with R_D = 1 kohm across it: from 1 V it discharges with tau = 100 us.
    let discharge = "bench Discharge for RC {
        ignore J1;
        component R_D: RESISTOR { value = 1kohm; }
        net VOUT { R_D.1; }
        net GND { R_D.2; }
        ic C1 = 1V;
        analysis tran { stop = 300us; }
        measure at_tau = at(V(VOUT), 100us);
        measure charge = integrate(I(R_D)) { min = 0.0949uC; max = 0.0951uC; }
        plot V(VOUT);
    }";
    let by_node = discharge.replace("bench Discharge", "bench ByNode").replace("ic C1 = 1V;", "ic V(VOUT) = 1V;");
    let uic = discharge.replace("bench Discharge", "bench Uic").replace("stop = 300us;", "stop = 300us; uic = true;");
    // L1 = 1 mH across C1 = 100 nF from 1 V, no operating point: v = cos(w t), a
    // quarter period to the first zero.
    let tank = "bench Tank for RC {
        ignore J1;
        component L1: INDUCTOR { value = 1mH; }
        net VOUT { L1.1; }
        net GND { L1.2; }
        ic C1 = 1V;
        analysis tran { stop = 200us; max_step = 100ns; reltol = 0.0001%; uic = true; }
        measure quarter = cross(V(VOUT), 0V);
        measure low = min(V(VOUT));
        plot V(VOUT), I(L1);
    }";
    let board = rc_project(
        "ic",
        &[
            ("benches/discharge.copper", discharge),
            ("benches/by_node.copper", &by_node),
            ("benches/uic.copper", &uic),
            ("benches/tank.copper", tank),
        ],
    );
    let runs = run(&board);
    println!("{}", copper_simulate::results::text_table(&runs));
    for name in ["Discharge", "ByNode", "Uic"] {
        let r = bench(&runs, name);
        assert_eq!((r.error.as_deref(), r.status()), (None, Status::Pass), "{name}");
        assert!((value(r, 0, "at_tau") - (-1f64).exp()).abs() < 1e-3, "{name}: {}", value(r, 0, "at_tau"));
        // The charge through R_D is C (1 V - v(300 us)) = 100 nF (1 - e^-3).
        assert!(rel(value(r, 0, "charge"), 100e-9 * (1.0 - (-3f64).exp())) < 2e-3, "{name}");
    }
    let r = bench(&runs, "Tank");
    assert_eq!(r.error, None);
    let quarter = 2.0 * PI * (1e-3f64 * 100e-9).sqrt() / 4.0;
    assert!(rel(value(r, 0, "quarter"), quarter) < 1e-3, "{} vs {quarter}", value(r, 0, "quarter"));
    assert!((value(r, 0, "low") + 1.0).abs() < 2e-3);
    // An ic needs a transient run; a node ic is no state with uic.
    let design = compile(&board);
    for (source, words) in [
        ("bench B for RC { ignore J1; ic C1 = 1V; analysis op; measure v = V(VOUT); }", "transient run"),
        (
            "bench B for RC { ignore J1; ic V(VOUT) = 1V; analysis tran { stop = 1us; uic = true; } measure v = max(V(VOUT)); }",
            "uic",
        ),
        (
            "bench B for RC { ignore J1; ic R1 = 1V; analysis tran { stop = 1us; } measure v = max(V(VOUT)); }",
            "neither",
        ),
    ] {
        let failure = copper_lang::bench::compile_bench_source(source, "b.copper", &design).unwrap_err();
        assert!(failure.code == "SIM075" && failure.message.contains(words), "{failure}");
    }
}

#[test]
fn currents_of_models_the_bench_places() {
    // A 1 V source behind 1 kohm, R1 = 1 kohm and R_D = 1 kohm in series: 1 / 3 mA
    // leaves the source's out terminal, so the current into it is -1/3 mA.
    let replaced = "bench Replaced for RC {
        replace J1 with source { out = J1.1; ref = GND; dc = 1V; output_resistance = 1kohm; }
        component R_D: RESISTOR { value = 1kohm; }
        net VOUT { R_D.1; }
        net GND { R_D.2; }
        model PROBE: load { inp = VOUT; ref = GND; resistance = 1000Mohm; }
        analysis op;
        measure i_source = I(J1);
        measure i_out = I(J1.out);
        measure i_ref = I(J1.ref);
        measure i_probe = I(PROBE);
    }";
    let board = rc_project("currents", &[("benches/replaced.copper", replaced)]);
    let runs = run(&board);
    let r = &runs[0];
    assert_eq!(r.error, None);
    // With the 1 Gohm probe across R_D: the load is R_D || 1 Gohm.
    let load = 1e3 * 1e9 / (1e3 + 1e9);
    let total = 1.0 / (2e3 + load);
    assert!(rel(value(r, 0, "i_source"), -total) < 1e-9, "{}", value(r, 0, "i_source"));
    assert_eq!(value(r, 0, "i_source"), value(r, 0, "i_out"));
    let names: Vec<&str> = r.measures.iter().map(|m| m.expression.as_str()).collect();
    assert_eq!(names, ["I(J1)", "I(J1)", "I(J1.ref)", "I(PROBE)"]);
    assert!(rel(value(r, 0, "i_ref"), total) < 1e-9);
    assert!(rel(value(r, 0, "i_probe"), total * load / 1e9) < 1e-6, "{}", value(r, 0, "i_probe"));
    // A part pin of a replaced component is no terminal of its model: the message says what to write.
    let design = compile(&board);
    let source = replaced.replace("measure i_out = I(J1.out);", "measure i_out = I(J1.1);");
    let failure = copper_lang::bench::compile_bench_source(&source, "b.copper", &design).unwrap_err();
    assert!(
        failure.code == "SIM045" && failure.message.contains("I(J1.ROLE)") && failure.message.contains("out, ref"),
        "{failure}"
    );
}

#[test]
fn noise_of_a_diode_and_the_noise_figure_axes() {
    // 1 mA into the diode (I_S = 10 fA, R_S = 10 ohm): the noise at A is
    // sqrt(2 q I r_d^2 + 4 k T R_S), r_d = V_T / (I + I_S) (G_MIN aside).
    let bench_source = "bench Shot for Analog {
        replace J1 with source { out = J1.1; ref = GND; output = current; dc = 1mA; }
        replace J2 with source { out = J2.1; ref = GND; dc = 0V; }
        analysis noise { from = 10Hz; to = 100Hz; points_per_decade = 10; output = V(A); }
        measure density = at(noise(), 50Hz);
        measure shot = at(noise(D1), 50Hz);
        plot noise(), noise(D1);
    }";
    let board = analog_project("noise", &[("benches/shot.copper", bench_source)]);
    let runs = run(&board);
    let r = &runs[0];
    assert_eq!(r.error, None);
    let (q, k, t) = (1.602_176_634e-19, 1.380649e-23, 300.15);
    let vt = copper_sim::devices::THERMAL_VOLTAGE;
    let rd = vt / (1e-3 + 10e-15);
    let want = (2.0 * q * 1e-3 * rd * rd + 4.0 * k * t * 10.0).sqrt();
    assert!(rel(value(r, 0, "density"), want) < 1e-5, "{} vs {want}", value(r, 0, "density"));
    // noise(D1) holds the junction's shot noise and its series resistance's thermal noise.
    assert!(rel(value(r, 0, "shot"), want) < 1e-5);
    let labels: Vec<&str> = r.steps[0].noise.first().unwrap().contributors.iter().map(|c| c.label.as_str()).collect();
    assert!(labels.contains(&"D1 shot noise") && labels.contains(&"D1 thermal"), "{labels:?}");
    // Noise densities are drawn log-log, even over a flat stretch.
    let html = copper_simulate::report::html(r, "Analog");
    assert!(html.contains("\"log\":true"), "noise panels on a log axis");
}

#[test]
fn harmonics_of_a_square_wave() {
    // A +-1 V square wave of 1 kHz (1 us edges): odd harmonics of 4 / (n pi);
    // thd over 2 to 9 is sqrt(1/9 + 1/25 + 1/49 + 1/81), to 11 adds 1/121.
    let square = "bench Square for RC {
        replace J1 with source {
            out = J1.1; ref = GND;
            waveform = pulse { initial = -1V; pulsed = 1V; rise = 1us; fall = 1us; width = 499us; period = 1ms; };
        }
        analysis tran { stop = 5ms; start = 1ms; max_step = 2us; }
        measure h1 = harmonic(V(VIN), 1kHz, 1);
        measure h3 = harmonic(V(VIN), 1kHz, 3);
        measure h2 = harmonic(V(VIN), 1kHz, 2);
        measure thd9 = thd(V(VIN), 1kHz);
        measure thd11 = thd(V(VIN), 1kHz, 11);
    }";
    let board = rc_project("square", &[("benches/square.copper", square)]);
    let runs = run(&board);
    let r = &runs[0];
    assert_eq!(r.error, None, "{}", copper_simulate::results::text_table(&runs));
    let a1 = 4.0 / PI;
    assert!(rel(value(r, 0, "h1"), a1) < 2e-3, "{}", value(r, 0, "h1"));
    assert!(rel(value(r, 0, "h3"), a1 / 3.0) < 2e-3, "{}", value(r, 0, "h3"));
    assert!(value(r, 0, "h2") < 1e-3 * a1);
    let nine = (1.0f64 / 9.0 + 1.0 / 25.0 + 1.0 / 49.0 + 1.0 / 81.0).sqrt();
    let eleven = (nine * nine + 1.0 / 121.0).sqrt();
    assert!(rel(value(r, 0, "thd9"), nine) < 2e-3 && rel(value(r, 0, "thd11"), eleven) < 2e-3);
}

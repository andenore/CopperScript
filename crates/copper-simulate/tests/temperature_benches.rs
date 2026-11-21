//! The bench temperature (plan phase F8) through the whole chain: `param
//! TEMP` stepped like any param, passives' `tempco`, a capacitor's ESR and
//! ESL and an inductor's DC resistance from their parts, the built-in diode,
//! op-amp offset drift and noise, a reference's temperature coefficient, and
//! resistor thermal noise, each against its closed form.

use std::path::{Path, PathBuf};

use copper_lang::compiler::{Design, compile_design_file};
use copper_sim::devices::GMIN;
use copper_sim::noise::BOLTZMANN;
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-f8-{name}-{}", std::process::id()));
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

fn value(run: &BenchRun, step: usize, name: &str) -> f64 {
    let index = run.measures.iter().position(|measure| measure.name == name).unwrap_or_else(|| panic!("{name}"));
    run.steps[step].measures[index].value
}

fn rel(a: f64, b: f64) -> f64 {
    (a / b - 1.0).abs()
}

fn kelvin(celsius: f64) -> f64 {
    celsius + 273.15
}

/// The parts, one document per file.
const PARTS: [(&str, &str); 3] = [
    (
        "parts/cap_low_esl.copper",
        "part CAP_LOW_ESL {
    category = \"passive.capacitor\";
    esr = 0.1ohm;
    esl = 1nH;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}",
    ),
    (
        "parts/cap_srf.copper",
        "part CAP_SRF {
    category = \"passive.capacitor\";
    esr = 20mohm;
    self_resonance = 5MHz;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}",
    ),
    (
        "parts/choke.copper",
        "part CHOKE {
    category = \"passive.inductor\";
    esr = 2ohm;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}",
    ),
];

/// A divider R1 (with a tempco) over R2 from 2 V; a capacitor, an inductor
/// and four loads on nets of their own for bench models to drive.
const BOARD: &str = "board Thermal {
    use library \"tiny\";
    import p \"./parts\";
    component R1: RESISTOR { value = 10kohm; tempco = 100ppm/degC; tempco2 = 0.5ppm/degC2; }
    component R2: RESISTOR { value = 10kohm; }
    component C1: p.CAP_LOW_ESL { value = 10uF; }
    component C2: p.CAP_SRF { value = 100nF; }
    component L1: p.CHOKE { value = 10uH; }
    component RL: RESISTOR { value = 1kohm; }
    component RR: RESISTOR { value = 10kohm; }
    component RD: RESISTOR { value = 1Mohm; }
    component RP: RESISTOR { value = 10kohm; }
    component RN: RESISTOR { value = 10kohm; }
    net VIN { R1.1; }
    net MID { R1.2; R2.1; }
    net CIN { C1.1; }
    net C2IN { C2.1; }
    net LIN { L1.1; }
    net OUTA { RL.1; }
    net REFOUT { RR.1; }
    net DA { RD.1; }
    net VP { RP.1; }
    net VN { RN.1; }
    net GND { R2.2; C1.2; C2.2; L1.2; RL.2; RR.2; RD.2; RP.2; RN.2; }
    supply IN { voltage = 2V; net = VIN; external = true; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn runs(name: &str, benches: &[(&str, &str)]) -> Vec<BenchRun> {
    let mut files = vec![("board.copper", BOARD)];
    files.extend_from_slice(&PARTS);
    files.extend_from_slice(benches);
    let path = project(name, &files);
    simulate(&path, &compile(&path), None).unwrap_or_else(|failure| panic!("{failure}"))
}

fn bench<'a>(runs: &'a [BenchRun], name: &str) -> &'a BenchRun {
    let run = runs.iter().find(|run| run.name == name).unwrap_or_else(|| panic!("no bench {name}"));
    assert_eq!(run.status(), Status::Pass, "{name}: {:?}", run.error);
    run
}

/// R1 at `celsius`: 10 kohm (1 + 100e-6 dT + 0.5e-6 dT^2), dT from 27 degC.
fn r1(celsius: f64) -> f64 {
    let dt = celsius - 27.0;
    10e3 * (1.0 + 100e-6 * dt + 0.5e-6 * dt * dt)
}

#[test]
fn resistor_tempco_and_thermal_noise_over_a_stepped_temperature() {
    let divider = "bench Divider for Thermal {
        ignore C1, C2;
        param TEMP = 27degC;
        analysis op;
        step TEMP in -40degC, 27degC, 125degC;
        measure v_mid = V(MID);
    }";
    let nominal = "bench Nominal for Thermal {
        ignore C1, C2;
        analysis op;
        measure v_mid = V(MID);
    }";
    let noise = "bench Johnson for Thermal {
        ignore C1, C2;
        param TEMP = 27degC;
        analysis noise { from = 1kHz; to = 10kHz; points_per_decade = 2; output = V(MID); }
        step TEMP in -40degC, 125degC;
        measure density = at(noise(), 1kHz);
        measure r2_share = at(noise(R2), 1kHz);
    }";
    let all = runs(
        "tempco",
        &[("benches/divider.copper", divider), ("benches/nominal.copper", nominal), ("benches/noise.copper", noise)],
    );
    let run = bench(&all, "Divider");
    for (step, celsius) in [-40.0, 27.0, 125.0].into_iter().enumerate() {
        let expect = 2.0 * 10e3 / (r1(celsius) + 10e3);
        let v = value(run, step, "v_mid");
        println!("divider at {celsius} degC: {v:.9} V (closed form {expect:.9} V)");
        assert!(rel(v, expect) < 1e-12, "{celsius}: {v} vs {expect}");
    }
    // Without TEMP the bench runs at 27 degC: the value of that step, to the last bit.
    assert_eq!(value(bench(&all, "Nominal"), 0, "v_mid"), value(run, 1, "v_mid"));
    // 4 k T (R1 || R2), with R1 at its own temperature.
    let run = bench(&all, "Johnson");
    for (step, celsius) in [-40.0, 125.0].into_iter().enumerate() {
        let parallel = r1(celsius) * 10e3 / (r1(celsius) + 10e3);
        let expect = (4.0 * BOLTZMANN * kelvin(celsius) * parallel).sqrt();
        let density = value(run, step, "density");
        println!("thermal noise at {celsius} degC: {:.6} nV/rtHz (4kTR {:.6})", density * 1e9, expect * 1e9);
        assert!(rel(density, expect) < 1e-9, "{celsius}: {density} vs {expect}");
        let share = (4.0 * BOLTZMANN * kelvin(celsius) / 10e3).sqrt() * parallel;
        assert!(rel(value(run, step, "r2_share"), share) < 1e-9);
    }
}

#[test]
fn capacitor_esr_esl_and_inductor_dc_resistance_from_the_parts() {
    let parasitics = "bench Parasitics for Thermal {
        model SC: source { out = CIN; ref = GND; output = current; ac = 1A; }
        model SC2: source { out = C2IN; ref = GND; output = current; ac = 1A; }
        model SL: source { out = LIN; ref = GND; output = current; ac = 1A; }
        analysis ac { from = 10kHz; to = 100MHz; points_per_decade = 1; }
        measure zc_low = mag(at(V(CIN), 10kHz));
        measure zc_resonance = mag(at(V(CIN), 1591549.4309189534Hz));
        measure zc_high = mag(at(V(CIN), 100MHz));
        measure zc2_resonance = mag(at(V(C2IN), 5MHz));
        measure zl = mag(at(V(LIN), 10kHz));
    }";
    let all = runs("parasitics", &[("benches/parasitics.copper", parasitics)]);
    let run = bench(&all, "Parasitics");
    let w = |f: f64| 2.0 * std::f64::consts::PI * f;
    let z = |f: f64, r: f64, l: f64, c: f64| r.hypot(w(f) * l - 1.0 / (w(f) * c));
    assert!(rel(value(run, 0, "zc_low"), z(1e4, 0.1, 1e-9, 10e-6)) < 1e-9);
    assert!(rel(value(run, 0, "zc_high"), z(1e8, 0.1, 1e-9, 10e-6)) < 1e-9);
    // At the series resonance only the ESR is left.
    assert!(rel(value(run, 0, "zc_resonance"), 0.1) < 1e-6, "{}", value(run, 0, "zc_resonance"));
    // The ESL a declared self-resonance gives: the impedance there is the ESR.
    assert!(rel(value(run, 0, "zc2_resonance"), 0.02) < 1e-6, "{}", value(run, 0, "zc2_resonance"));
    assert!(rel(value(run, 0, "zl"), 2.0f64.hypot(w(1e4) * 10e-6)) < 1e-9);
    let model = |name: &str| run.models.iter().find(|m| m.name == name).unwrap().model.clone();
    assert_eq!(model("C1"), "capacitor (ESR 0.1 ohm, ESL 1 nH)");
    assert_eq!(model("L1"), "inductor (DC resistance 2 ohm)");
    assert!(model("C2").starts_with("capacitor (ESR 20 mohm, ESL "), "{}", model("C2"));
    assert_eq!(model("R1"), "resistor (tempco 100 ppm/degC, tempco2 0.5 ppm/degC2)");
}

/// The voltage at which `is (exp(v / vt) - 1) + GMIN v + v / r` carries `i`.
fn loaded_junction(is: f64, vt: f64, r: f64, i: f64) -> f64 {
    let mut v = vt * (i / is).ln();
    for _ in 0..60 {
        let f = is * ((v / vt).exp() - 1.0) + GMIN * v + v / r - i;
        v -= f / (is / vt * (v / vt).exp() + GMIN + 1.0 / r);
    }
    v
}

#[test]
fn macro_models_over_temperature() {
    let drift = "bench Drift for Thermal {
        ignore C1, C2;
        param TEMP = 27degC;
        model U1: opamp {
            inp = GND; inn = OUTA; out = OUTA; vp = VP; vn = VN;
            gbw = 1MHz; open_loop_gain = 200dB;
            input_offset_voltage = 100uV; input_offset_drift = 2uV/degC;
        }
        model REF: reference { in = VP; out = REFOUT; gnd = GND; output_voltage = 2.5V; temperature_coefficient = -10ppm/degC; }
        analysis op;
        step TEMP in -40degC, 27degC, 85degC;
        measure vos = V(OUTA);
        measure vref = V(REFOUT);
    }";
    let noise = "bench AmpNoise for Thermal {
        ignore C1, C2;
        param TEMP = 27degC;
        model U1: opamp {
            inp = GND; inn = OUTA; out = OUTA; vp = VP; vn = VN;
            gbw = 1MHz; open_loop_gain = 200dB; voltage_noise = 10nV/rtHz; current_noise = 1pA/rtHz;
        }
        analysis noise { from = 1Hz; to = 100Hz; points_per_decade = 1; output = V(OUTA); }
        step TEMP in 27degC, 125degC;
        measure en = at(noise(U1), 10Hz);
    }";
    let diode = "bench Junction for Thermal {
        ignore C1, C2;
        param TEMP = 27degC;
        model D1: diode { anode = DA; cathode = GND; saturation_current = 10fA; }
        model SI: source { out = DA; ref = GND; output = current; dc = 1mA; }
        analysis op;
        step TEMP in -40degC, 27degC, 100degC;
        measure vd = V(DA);
    }";
    let all = runs(
        "macro",
        &[("benches/drift.copper", drift), ("benches/noise.copper", noise), ("benches/diode.copper", diode)],
    );
    let run = bench(&all, "Drift");
    for (step, celsius) in [-40.0, 27.0, 85.0].into_iter().enumerate() {
        let vos = 100e-6 + 2e-6 * (celsius - 27.0);
        assert!((value(run, step, "vos") - vos).abs() < 1e-12, "{celsius}: {}", value(run, step, "vos"));
        let vref = 2.5 * (1.0 - 10e-6 * (celsius - 27.0));
        assert!(rel(value(run, step, "vref"), vref) < 1e-12, "{celsius}: {}", value(run, step, "vref"));
    }
    // The voltage noise is thermal: e_n sqrt(T / 300.15 K); the current noise meets 0 ohm and is gone.
    let run = bench(&all, "AmpNoise");
    assert!(rel(value(run, 0, "en"), 10e-9) < 1e-9);
    assert!(rel(value(run, 1, "en"), 10e-9 * (kelvin(125.0) / 300.15).sqrt()) < 1e-9);
    // The built-in diode at SPICE's defaults (EG 1.11 eV, XTI 3) from 27 degC, beside RD = 1 Mohm.
    let run = bench(&all, "Junction");
    for (step, celsius) in [-40.0, 27.0, 100.0].into_iter().enumerate() {
        let t = kelvin(celsius);
        let vt = 1.380649e-23 * t / 1.602_176_634e-19;
        let is = 1e-14 * ((t / 300.15 - 1.0) * 1.11 / vt + 3.0 * (t / 300.15).ln()).exp();
        let expect = loaded_junction(is, vt, 1e6, 1e-3);
        let v = value(run, step, "vd");
        println!("built-in diode at {celsius} degC: {v:.9} V (closed form {expect:.9} V)");
        assert!((v - expect).abs() < 1e-9, "{celsius}: {v} vs {expect}");
    }
}

#[test]
fn temperature_diagnostics() {
    let mut files = vec![("board.copper", BOARD)];
    files.extend_from_slice(&PARTS);
    let design_path = project("diagnostics", &files);
    let design = compile(&design_path);
    let compile_bench = |source: &str| copper_lang::bench::compile_benches_source(source, "b.copper", &design);
    for (bad, code, says) in [
        ("param TEMP = 5V; analysis op; measure v = V(MID);", "SIM110", "degC"),
        (
            "param TEMP = 27degC; step TEMP in 27degC, -300degC; analysis op; measure v = V(MID);",
            "SIM110",
            "absolute zero",
        ),
        (
            "param TEMP = 85degC; analysis noise { from = 1kHz; to = 10kHz; points_per_decade = 2; output = V(MID); temperature = 27degC; } measure n = at(noise(), 1kHz);",
            "SIM110",
            "remove temperature",
        ),
    ] {
        let source = format!("bench B for Thermal {{ {bad} }}");
        let failure = compile_bench(&source).unwrap_err().to_string();
        assert!(failure.contains(code) && failure.contains(says), "{bad}: {failure}");
    }
    // A tempco must be in ppm/degC (ppm/degC2 for tempco2).
    let board = BOARD.replace("tempco = 100ppm/degC;", "tempco = 100%;");
    let mut files = vec![("board.copper", board.as_str())];
    files.extend_from_slice(&PARTS);
    let path = project("tempco-unit", &files);
    let failure = compile_design_file(&path, false, true).unwrap_err().to_string();
    assert!(failure.contains("SIM111") && failure.contains("TemperatureCoefficient"), "{failure}");
}

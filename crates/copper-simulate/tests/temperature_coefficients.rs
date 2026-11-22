//! Temperature coefficients of plan I7: a two-terminal part's default
//! `tempco`, and an amplifier's `input_bias_current_tempco`, `gbw_tempco` and
//! `slew_rate_tempco`, each against its closed form, and their diagnostics.

use std::path::{Path, PathBuf};

use copper_lang::compiler::{Design, compile_design_file};
use copper_simulate::run::BenchRun;
use copper_simulate::{Status, simulate};

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulate-i7-{name}-{}", std::process::id()));
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

/// A resistor part whose components take its temperature coefficients.
const PART: &str = "part RES_TC {
    category = \"passive.resistor\";
    tempco = 50ppm/degC;
    tempco2 = 0.2ppm/degC2;
    pin 1 { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin 2 { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
}";

/// R1 takes the part's coefficients, R2 declares its own tempco (and the part's tempco2), R3 is a
/// plain resistor: two dividers from 2 V. RB is the source resistance of an amplifier input.
const BOARD: &str = "board Coefficients {
    use library \"tiny\";
    import p \"./parts\";
    component R1: p.RES_TC { value = 10kohm; }
    component R2: p.RES_TC { value = 10kohm; tempco = -100ppm/degC; }
    component R3: RESISTOR { value = 10kohm; }
    component R4: RESISTOR { value = 10kohm; }
    component RB: RESISTOR { value = 1Mohm; }
    component RL: RESISTOR { value = 10kohm; }
    component RP: RESISTOR { value = 10kohm; }
    component RN: RESISTOR { value = 10kohm; }
    net VIN { R1.1; R2.1; }
    net MID1 { R1.2; R3.1; }
    net MID2 { R2.2; R4.1; }
    net BIAS { RB.1; }
    net OUTA { RL.1; }
    net AIN { }
    net VP { RP.1; }
    net VN { RN.1; }
    net GND { R3.2; R4.2; RB.2; RL.2; RP.2; RN.2; }
    supply IN { voltage = 2V; net = VIN; external = true; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn runs(name: &str, board: &str, benches: &[(&str, &str)]) -> Result<Vec<BenchRun>, String> {
    let mut files = vec![("board.copper", board), ("parts/res_tc.copper", PART)];
    files.extend_from_slice(benches);
    let path = project(name, &files);
    let design = compile_design_file(&path, false, true).map_err(|failure| failure.to_string())?;
    simulate(&path, &design, None).map_err(|failure| failure.to_string())
}

fn bench<'a>(runs: &'a [BenchRun], name: &str) -> &'a BenchRun {
    let run = runs.iter().find(|run| run.name == name).unwrap_or_else(|| panic!("no bench {name}"));
    assert_eq!(run.status(), Status::Pass, "{name}: {:?}", run.error);
    run
}

#[test]
fn a_part_tempco_is_the_default_of_its_components() {
    let divider = "bench Dividers for Coefficients {
        ignore RB, RL, RP, RN;
        param TEMP = 27degC;
        analysis op;
        step TEMP in -40degC, 27degC, 125degC;
        measure v1 = V(MID1);
        measure v2 = V(MID2);
    }";
    let all = runs("part-tempco", BOARD, &[("benches/dividers.copper", divider)]).unwrap();
    let run = bench(&all, "Dividers");
    for (step, celsius) in [-40.0, 27.0, 125.0].into_iter().enumerate() {
        let dt = celsius - 27.0;
        // R1: the part's 50 ppm/degC and 0.2 ppm/degC2; R2: its own -100 ppm/degC with the part's tempco2.
        let r1 = 10e3 * (1.0 + 50e-6 * dt + 0.2e-6 * dt * dt);
        let r2 = 10e3 * (1.0 - 100e-6 * dt + 0.2e-6 * dt * dt);
        let (v1, v2) = (value(run, step, "v1"), value(run, step, "v2"));
        assert!(rel(v1, 2.0 * 10e3 / (r1 + 10e3)) < 1e-12, "{celsius}: {v1}");
        assert!(rel(v2, 2.0 * 10e3 / (r2 + 10e3)) < 1e-12, "{celsius}: {v2}");
    }
    let model = |name: &str| run.models.iter().find(|m| m.name == name).unwrap().model.clone();
    assert_eq!(model("R1"), "resistor (tempco 50 ppm/degC, tempco2 0.2 ppm/degC2)");
    assert_eq!(model("R2"), "resistor (tempco -100 ppm/degC, tempco2 0.2 ppm/degC2)");
}

#[test]
fn amplifier_bias_current_gain_bandwidth_and_slew_over_temperature() {
    // Bias: a follower whose input sits on RB = 1 Mohm to ground, V(BIAS) = -I_b(T) RB with
    // I_b(T) = 1 nA exp(69315 ppm/degC dT): it doubles every 10 degC (to 3e-6 of ln 2).
    let bias = "bench Bias for Coefficients {
        ignore R1, R2, R3, R4;
        param TEMP = 27degC;
        model U1: opamp {
            inp = BIAS; inn = OUTA; out = OUTA; vp = VP; vn = VN;
            gbw = 1MHz; open_loop_gain = 120dB; input_bias_current = 1nA; input_bias_current_tempco = 69315ppm/degC;
        }
        analysis op;
        step TEMP in -3degC, 27degC, 37degC, 87degC;
        measure v = V(BIAS);
    }";
    // GBW: in open loop at 100 kHz the gain is |A0 / (1 + j f A0 / GBW(T))|, GBW(T) = 1 MHz (1 - 2000 ppm/degC dT).
    let gbw = "bench Bandwidth for Coefficients {
        ignore R1, R2, R3, R4, RB;
        param TEMP = 27degC;
        model SIN: source { out = AIN; ref = GND; ac = 1V; }
        model U1: opamp { inp = AIN; inn = GND; out = OUTA; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 60dB; gbw_tempco = -2000ppm/degC; }
        analysis ac { from = 10kHz; to = 100kHz; points_per_decade = 1; }
        step TEMP in -40degC, 27degC, 125degC;
        measure gain = mag(at(V(OUTA), 100kHz));
    }";
    // Slew: a follower step of 4 V at SR(T) = 1 V/us (1 + 3000 ppm/degC dT).
    let slew = "bench Slew for Coefficients {
        ignore R1, R2, R3, R4, RB;
        param TEMP = 27degC;
        model SIN: source { out = AIN; ref = GND; waveform = pulse { initial = -2V; pulsed = 2V; delay = 1us; rise = 1ns; fall = 1ns; width = 50us; }; }
        model U1: opamp { inp = AIN; inn = OUTA; out = OUTA; vp = VP; vn = VN; gbw = 10MHz; open_loop_gain = 100dB; slew_rate = 1V/us; slew_rate_tempco = 3000ppm/degC; }
        analysis tran { stop = 20us; max_step = 10ns; }
        step TEMP in -40degC, 125degC;
        measure t20 = cross(V(OUTA), -1.2V);
        measure t80 = cross(V(OUTA), 1.2V);
    }";
    let all = runs(
        "amplifier-tempco",
        BOARD,
        &[("benches/bias.copper", bias), ("benches/gbw.copper", gbw), ("benches/slew.copper", slew)],
    )
    .unwrap();
    let run = bench(&all, "Bias");
    for (step, celsius) in [-3.0, 27.0, 37.0, 87.0].into_iter().enumerate() {
        let ib = 1e-9 * (69315e-6_f64 * (celsius - 27.0)).exp();
        // The follower holds the input: the 1 Mohm carries the bias current alone.
        let v = value(run, step, "v");
        assert!(rel(v, -ib * 1e6) < 1e-6, "{celsius}: {v} vs {}", -ib * 1e6);
    }
    assert!(rel(value(run, 2, "v"), 2.0 * value(run, 1, "v")) < 1e-5, "doubles in 10 degC");
    let run = bench(&all, "Bandwidth");
    for (step, celsius) in [-40.0, 27.0, 125.0].into_iter().enumerate() {
        let gbw = 1e6 * (1.0 - 2000e-6 * (celsius - 27.0));
        let a0: f64 = 1000.0;
        let expect = a0 / (1.0 + (1e5 * a0 / gbw).powi(2)).sqrt();
        assert!(rel(value(run, step, "gain"), expect) < 1e-9, "{celsius}: {}", value(run, step, "gain"));
    }
    let run = bench(&all, "Slew");
    for (step, celsius) in [-40.0, 125.0].into_iter().enumerate() {
        let sr = 1e6 * (1.0 + 3000e-6 * (celsius - 27.0));
        let rate = 2.4 / (value(run, step, "t80") - value(run, step, "t20"));
        assert!(rel(rate, sr) < 0.02, "{celsius}: {rate} vs {sr}");
    }
}

#[test]
fn temperature_coefficient_diagnostics() {
    // A linear coefficient that leaves the gain-bandwidth at or below zero at the bench temperature.
    let bench = "bench Hot for Coefficients {
        ignore R1, R2, R3, R4, RB;
        param TEMP = 150degC;
        model U1: opamp { inp = GND; inn = OUTA; out = OUTA; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 60dB; gbw_tempco = -10000ppm/degC; }
        analysis op;
        measure v = V(OUTA);
    }";
    let all = runs("hot", BOARD, &[("benches/hot.copper", bench)]).unwrap();
    let run = all.iter().find(|run| run.name == "Hot").unwrap();
    let error = run.error.clone().unwrap_or_default();
    assert!(error.contains("SIM116") && error.contains("gbw_tempco"), "{error}");
    // The coefficients are temperature coefficients.
    let typed = BOARD.replace(
        "component RB: RESISTOR { value = 1Mohm; }",
        "component RB: RESISTOR { value = 1Mohm; }\n    component U9: p.AMP_BAD;",
    );
    let amp = "part AMP_BAD {
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = INP; vn = INN; gbw = 1MHz; gbw_tempco = 1MHz; }
}";
    let path =
        project("typed", &[("board.copper", typed.as_str()), ("parts/res_tc.copper", PART), ("parts/amp.copper", amp)]);
    let failure = compile_design_file(&path, false, true).unwrap_err().to_string();
    assert!(failure.contains("SIM018") && failure.contains("gbw_tempco"), "{failure}");
    // A part tempco belongs to a two-terminal part without sim blocks, in ppm/degC.
    for (part, says) in [
        (PART.replace("tempco = 50ppm/degC;", "tempco = 5%;"), "part tempco must be a TemperatureCoefficient"),
        (
            PART.replace(
                "pin 2 {",
                "pin 3 { number = \"3\"; domains = \"analog\"; directions = \"passive\"; }\n    pin 2 {",
            ),
            "it has 3 pins",
        ),
    ] {
        let path = project("part-tempco-bad", &[("board.copper", BOARD), ("parts/res_tc.copper", part.as_str())]);
        let failure = compile_design_file(&path, false, true).unwrap_err().to_string();
        assert!(failure.contains("SIM111") && failure.contains(says), "{failure}");
    }
    let _ = compile;
}

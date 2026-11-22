//! The transient frontend (phase P2 of `docs/integrated-simulation-plan.md`):
//! `analysis tran`, source waveforms, `sim diode` and `sim switch` blocks,
//! the op-amp's transient parameters and the time-domain functions. Nothing
//! here simulates.

use copper_lang::bench::{Analysis, Bench, bench_json, compile_bench_source};
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::expression::Units;
use copper_lang::simulation::{SimKind, Waveform};
use copper_units::Dimension;

fn error_of<T: std::fmt::Debug>(result: Result<T, copper_lang::CopperScriptError>) -> String {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(failure) => failure.to_string(),
    }
}

const RC_BOARD: &str = "board RC {
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

fn rc() -> Design {
    compile_design_source(RC_BOARD, "rc.copper", false, true).unwrap()
}

/// A bench for `RC` with `source` as the input's waveform, `analysis`, then `body`.
fn tran_bench(waveform: &str, analysis: &str, body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; waveform = {waveform}; }}\n \
         {analysis}\n{body}\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc())
}

fn tran(body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    tran_bench("step { final = 1V; at = 1us; }", "analysis tran { stop = 1ms; }", body)
}

#[test]
fn analysis_tran_and_waveforms() {
    let bench = tran_bench(
        "pulse { initial = 0V; pulsed = 1V; delay = 1us; rise = 10ns; fall = 10ns; width = 5us; period = 10us; cycles = 3; }",
        "analysis tran { stop = 50us; start = 10us; max_step = 100ns; reltol = 0.001%; }",
        " measure v_max = max(V(VOUT));\n plot V(VOUT);",
    )
    .unwrap();
    let Analysis::Tran(tran) = &bench.analysis else { panic!("tran") };
    assert_eq!(tran.stop.value.to_string(), "50 us");
    assert_eq!(tran.start.as_ref().unwrap().value.to_string(), "10 us");
    assert_eq!(tran.max_step.as_ref().unwrap().value.to_string(), "100 ns");
    assert_eq!(tran.reltol.as_ref().unwrap().value.base_value().to_string(), "0.00001");
    let Some(Waveform::Pulse { cycles, period, .. }) = &bench.replacements[0].spec.waveform else { panic!("pulse") };
    assert_eq!((*cycles, period.as_ref().unwrap().value.to_string()), (Some(3), "10 us".to_owned()));
    // Probes are real and swept over time; plots and reductions take them.
    assert!(!bench.measures[0].expression.ty.complex);
    assert!(bench.plots[0].expressions[0].ty.swept && !bench.plots[0].expressions[0].ty.complex);
    let json = bench_json(&bench);
    assert!(json.contains("\"kind\": \"tran\"") && json.contains("\"kind\": \"pulse\""), "{json}");

    let pwl =
        tran_bench("pwl { points = 0s, 0V, 1us, 1V, 1us, 0.5V, 5us, 0.5V; }", "analysis tran { stop = 10us; }", "")
            .unwrap();
    let Some(Waveform::Pwl { points }) = &pwl.replacements[0].spec.waveform else { panic!("pwl") };
    assert_eq!(points.len(), 4);
    let sine = tran_bench(
        "sine { offset = 0.5V; amplitude = 0.1V; frequency = 1kHz; delay = 1ms; }",
        "analysis tran { stop = 3ms; }",
        "",
    )
    .unwrap();
    assert!(matches!(&sine.replacements[0].spec.waveform, Some(Waveform::Sine { delay: Some(_), .. })));
    // A param may set a waveform value and the analysis.
    let param = tran_bench(
        "step { final = V_STEP; }",
        "analysis tran { stop = T_STOP; }",
        " param V_STEP = 2V;\n param T_STOP = 5ms;\n step V_STEP in 1V, 2V;",
    )
    .unwrap();
    let Analysis::Tran(tran) = &param.analysis else { panic!("tran") };
    assert_eq!(tran.stop.param.as_deref(), Some("T_STOP"));
}

#[test]
fn analysis_and_waveform_errors() {
    let analysis = |text: &str| error_of(tran_bench("step { final = 1V; }", text, ""));
    assert!(analysis("analysis tran { }").contains("SIM069: analysis tran needs a stop time"));
    assert!(analysis("analysis tran { stop = 1V; }").contains("SIM018: analysis tran stop must be a Time"));
    assert!(analysis("analysis tran { stop = 0s; }").contains("SIM069: analysis tran stop must be positive"));
    assert!(
        analysis("analysis tran { stop = 1ms; start = 2ms; }")
            .contains("SIM069: analysis tran needs 0 <= start < stop")
    );
    assert!(analysis("analysis tran { stop = 1ms; max_step = 0s; }").contains("SIM069"));
    assert!(analysis("analysis tran { stop = 1ms; reltol = 200%; }").contains("SIM069"));
    let unknown = analysis("analysis tran { stop = 1ms; maxstep = 1us; }");
    assert!(
        unknown.contains("SIM069: unknown analysis tran property 'maxstep'") && unknown.contains("max_step"),
        "{unknown}"
    );
    let waveform = |text: &str| error_of(tran_bench(text, "analysis tran { stop = 1ms; }", ""));
    assert!(waveform("pulse { initial = 0V; pulsed = 1V; }").contains("SIM020: pulse needs width"));
    assert!(
        waveform("pulse { initial = 0V; pulsed = 1V; width = 5us; rise = 1us; period = 5us; }")
            .contains("SIM020: pulse period must be positive and at least rise + width + fall")
    );
    assert!(
        waveform("pulse { initial = 0V; pulsed = 1V; width = 5us; cycles = 2; }")
            .contains("pulse cycles needs a period")
    );
    assert!(waveform("pulse { initial = 0V; pulsed = 1V; width = 5us; cycles = 0; period = 6us; }").contains("SIM020"));
    assert!(waveform("pwl { points = 0s, 0V, 1us; }").contains("SIM020: pwl points takes time, value pairs"));
    assert!(waveform("pwl { points = 1us, 0V, 0s, 1V; }").contains("SIM020: pwl times must not decrease"));
    assert!(waveform("pwl { points = 0s, 1A; }").contains("SIM018: pwl value must be a Voltage"));
    assert!(waveform("step { at = 1us; }").contains("SIM020: step needs final"));
    assert!(waveform("step { final = 1V; at = -1us; }").contains("SIM020"));
    assert!(waveform("dc { value = 1V; }").contains("dc property"));
    let typo = waveform("puls { initial = 0V; }");
    assert!(typo.contains("SIM020: unknown waveform 'puls'") && typo.contains("did you mean 'pulse'?"), "{typo}");
    assert!(waveform("sine { amplitude = 1V; frequency = 1kHz; delay = 1V; }").contains("SIM018"));
}

#[test]
fn analysis_tran_accuracy_preset() {
    use copper_lang::transient::TranAccuracy;
    let accuracy = |text: &str| tran_bench("step { final = 1V; }", text, "");
    let bench = accuracy("analysis tran { stop = 1ms; accuracy = high; }").unwrap();
    let Analysis::Tran(tran) = &bench.analysis else { panic!("tran") };
    assert_eq!(tran.accuracy, Some(TranAccuracy::High));
    assert!(bench_json(&bench).contains("\"accuracy\": \"high\""));
    // Beside an explicit reltol and max_step (which win when the bench is run).
    let bench = accuracy("analysis tran { stop = 1ms; accuracy = normal; reltol = 0.001%; max_step = 1us; }").unwrap();
    let Analysis::Tran(tran) = &bench.analysis else { panic!("tran") };
    assert_eq!(tran.accuracy, Some(TranAccuracy::Normal));
    assert!(tran.reltol.is_some() && tran.max_step.is_some());
    let Analysis::Tran(plain) = &accuracy("analysis tran { stop = 1ms; }").unwrap().analysis else { panic!("tran") };
    assert_eq!(plain.accuracy, None);
    let typo = error_of(accuracy("analysis tran { stop = 1ms; accuracy = hihg; }"));
    assert!(
        typo.contains("SIM069: analysis tran accuracy is a preset of reltol and max_step, one of: normal, high")
            && typo.contains("did you mean 'high'?"),
        "{typo}"
    );
    assert!(error_of(accuracy("analysis tran { stop = 1ms; accuracy = 2; }")).contains("SIM069"));
}

#[test]
fn time_domain_functions_type_check() {
    let ok = |body: &str| tran(body).unwrap_or_else(|failure| panic!("{body}: {failure}"));
    let bench = ok(" let v = V(VOUT);
 measure a = max(v, 0.1ms, 0.5ms);
 measure b = rms(v);
 measure c = pp(v, 0s, 1ms);
 measure d = mean(I(R1));
 measure e = at(v, 0.5ms);
 measure f = cross(v, 0.5V) - 1us;
 measure g = settle(v, 10mV);
 measure h = overshoot(v);
 measure i = harmonic(v, 1kHz, 3);
 measure j = thd(v, 1kHz);");
    let units: Vec<Units> = bench.measures.iter().map(|m| m.expression.ty.units).collect();
    let volt = Units::of(Dimension::Voltage);
    let second = Units::of(Dimension::Time);
    assert_eq!(
        units,
        [
            volt,
            volt,
            volt,
            Units::of(Dimension::Current),
            volt,
            second,
            second,
            Units::DIMENSIONLESS,
            volt,
            Units::DIMENSIONLESS
        ]
    );
    let error = |body: &str| error_of(tran(body));
    assert!(error(" measure m = V(VOUT);").contains("SIM043: measure m varies over time"));
    assert!(error(" measure m = at(V(VOUT), 1kHz);").contains("SIM068: at(x, t) takes one time"));
    assert!(error(" measure m = max(V(VOUT), 1kHz, 2kHz);").contains("SIM070: max(x, from, to) takes two times"));
    assert!(error(" measure m = max(V(VOUT), 1ms);").contains("SIM064: max takes 1 argument, or 3"));
    assert!(error(" measure m = cross(V(VOUT), 1A);").contains("SIM071: cross takes a level"));
    assert!(error(" measure m = settle(V(VOUT), 1us);").contains("SIM071"));
    assert!(
        error(" measure m = harmonic(V(VOUT), 1ms, 2);").contains("SIM071: harmonic takes a fundamental frequency")
    );
    assert!(error(" measure m = thd(1V, 1kHz);").contains("SIM071: thd takes a real value that varies over time"));
    assert!(error(" measure m = max(1V);").contains("SIM066: max reduces over the tran time axis"));
    assert!(error(" plot 1V;").contains("SIM044: plot needs a swept value, but it does not vary with time"));
    // Outside tran: windows and cross on the frequency axis; the waveform measures need tran.
    let source = "bench B for RC {\n replace INPUT with source { out = VIN; ref = GND; ac = 1V; }\n \
         analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }\n BODY\n}";
    let ac = |body: &str| compile_bench_source(&source.replace("BODY", body), "b.copper", &rc());
    let bench = ac(" measure f = cross(db(V(VOUT)), -3dB);\n measure m = max(mag(V(VOUT)), 1kHz, 10kHz);\n measure r = pp(phase(V(VOUT)));").unwrap();
    assert_eq!(bench.measures[0].expression.ty.units, Units::of(Dimension::Frequency));
    assert!(
        error_of(ac(" measure m = overshoot(mag(V(VOUT)));")).contains("SIM070: overshoot measures a waveform in time")
    );
    assert!(
        error_of(ac(" measure m = max(mag(V(VOUT)), 1ms, 2ms);"))
            .contains("SIM070: max(x, from, to) takes two frequencies")
    );
}

const DEVICES_BOARD: &str = r#"board Devices {
    use library "tiny";
    import p "./parts";
    component D1: p.DIODE_X;
    component S1: p.SWITCH_X;
    component U1: p.AMP_X;
    component R1: RESISTOR { value = 1kohm; }
    net A { D1.A; S1.P; U1.INP; }
    net K { D1.K; R1.1; U1.OUT; U1.INN; }
    net CTL { S1.C; }
    net VP { U1.VP; }
    net VN { U1.VN; }
    net GND { R1.2; S1.N; S1.CR; }
    supply POS { voltage = 5V; net = VP; external = true; }
    supply NEG { voltage = -5V; net = VN; external = true; }
    supply CONTROL { voltage = 1V; net = CTL; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}"#;

const DIODE_PART: &str = r#"part DIODE_X {
    category = "semiconductor.diode";
    pin A { number = "1"; domains = "analog"; directions = "passive"; }
    pin K { number = "2"; domains = "analog"; directions = "passive"; }
    sim diode { anode = A; cathode = K; saturation_current = 10fA; emission_coefficient = 1.8; series_resistance = 1ohm;
        junction_capacitance = 2pF; grading_coefficient = 0.33; transit_time = 5ns; breakdown_voltage = 75V; }
}"#;

const SWITCH_PART: &str = r#"part SWITCH_X {
    category = "switch.analog";
    pin P { number = "1"; domains = "analog"; directions = "passive"; }
    pin N { number = "2"; domains = "analog"; directions = "passive"; }
    pin C { number = "3"; domains = "analog"; directions = "passive"; }
    pin CR { number = "4"; domains = "analog"; directions = "passive"; }
    sim switch { a = P; b = N; ctrl = C; ctrl_ref = CR; on_resistance = 5ohm; off_resistance = 100Mohm;
        threshold = 1.5V; hysteresis = 0.2V; }
}"#;

const AMP_PART: &str = r#"part AMP_X {
    category = "amplifier.opamp";
    pin INP { number = "1"; domains = "analog"; directions = "passive"; } pin INN { number = "2"; domains = "analog"; directions = "passive"; } pin OUT { number = "3"; domains = "analog"; directions = "passive"; } pin VP { number = "4"; domains = "analog"; directions = "passive"; } pin VN { number = "5"; domains = "analog"; directions = "passive"; }
    sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB; slew_rate = 1V/us;
        output_resistance = 50ohm; output_swing_from_positive_rail = 0.1V; output_swing_from_negative_rail = 0.3V;
        output_current_limit = 20mA; }
}"#;

/// A board in a temporary directory with one part per file in `parts/`.
fn project(name: &str, board: &str, parts: &[&str]) -> Result<Design, copper_lang::CopperScriptError> {
    let directory = std::env::temp_dir().join(format!("copper-transient-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    for (index, source) in parts.iter().enumerate() {
        std::fs::write(directory.join("parts").join(format!("p{index}.copper")), source).unwrap();
    }
    std::fs::write(directory.join("board.copper"), board).unwrap();
    copper_lang::compiler::compile_design_file(&directory.join("board.copper"), false, true)
}

#[test]
fn diode_switch_and_opamp_limit_blocks() {
    let design =
        project("devices", DEVICES_BOARD, &[DIODE_PART, SWITCH_PART, AMP_PART]).unwrap_or_else(|f| panic!("{f}"));
    let library = &design.electrical.library;
    let diode = &library["p.DIODE_X"].sim[0];
    assert_eq!(diode.spec.kind, SimKind::Diode);
    assert_eq!(diode.spec.parameters["emission_coefficient"].value.base_value().to_string(), "1.8");
    assert_eq!(diode.spec.parameters["saturation_current"].value.to_string(), "10 fA");
    let switch = &library["p.SWITCH_X"].sim[0];
    assert_eq!((switch.spec.kind, switch.pins["ctrl_ref"].as_str()), (SimKind::Switch, "CR"));
    let amp = &library["p.AMP_X"].sim[0];
    assert_eq!(amp.spec.parameters["output_current_limit"].value.to_string(), "20 mA");
    // A bench can replace with them.
    let bench = compile_bench_source(
        "bench B for Devices {\n replace D1 with diode { anode = A; cathode = K; saturation_current = 1nA; }\n analysis op;\n measure v = V(K);\n}",
        "b.copper",
        &design,
    )
    .unwrap();
    assert_eq!(bench.replacements[0].spec.kind, SimKind::Diode);
    // Range and shape checks.
    let board = r#"board B { use library "tiny"; import p "./parts"; component D1: p.DX;
        component R1: RESISTOR { value = 1kohm; } net A { D1.A; R1.1; } net GND { D1.K; R1.2; }
        supply GROUND { voltage = 0V; net = GND; external = true; } }"#;
    let part = |block: &str| {
        let part = format!(
            r#"part DX {{ category = "semiconductor.diode"; pin A {{ number = "1"; domains = "analog"; directions = "passive"; }} pin K {{ number = "2"; domains = "analog"; directions = "passive"; }} {block} }}"#
        );
        error_of(project("range", board, &[&part]))
    };
    assert!(
        part("sim diode { anode = A; cathode = K; grading_coefficient = 1; }")
            .contains("SIM019: sim diode grading_coefficient must be below 1")
    );
    assert!(part("sim diode { anode = A; cathode = K; saturation_current = 0A; }").contains("SIM019"));
    assert!(part("sim diode { anode = A; cathode = K; emission_coefficient = 1V; }").contains("SIM018"));
    assert!(part("sim diode { anode = A; }").contains("SIM013: sim diode needs the 'cathode' role"));
    assert!(
        part("sim diode { anode = A; cathode = K; breakdown = 5V; }")
            .contains("SIM012: unknown sim diode property 'breakdown'")
    );
}

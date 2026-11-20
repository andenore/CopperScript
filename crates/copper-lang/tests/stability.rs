//! The compile-time side of the stability analyses (phase F3): `analysis
//! loop` with its probe, `loop_gain()` and the margins, `analysis pz` with
//! its built-in `stable` measure and `damping()`, and their diagnostics
//! (`SIM098` to `SIM101`). `copper-simulate/tests/stability_runs.rs` runs them.

use copper_lang::bench::{Analysis, Bench, bench_json, compile_bench_source, compile_benches_source};
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::expression::{ExpressionKind, Function, Probe, Units};

const RC_BOARD: &str = "board RC {
    use library \"tiny\";
    component R1: RESISTOR { value = 1kohm; }
    component C1: CAPACITOR { value = 100nF; }
    component R2: RESISTOR { value = 10kohm; }
    component R3: RESISTOR { value = 10kohm; }
    net VIN { R1.1; }
    net VOUT { R1.2; C1.1; R2.1; }
    net GND { C1.2; R2.2; }
    supply INPUT { voltage = 1V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn rc() -> Design {
    compile_design_source(RC_BOARD, "rc.copper", false, true).unwrap()
}

fn bench_with(analysis: &str, body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; ac = 1V; }}\n ignore R3;\n {analysis}\n{body}\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc())
}

fn error(analysis: &str, body: &str) -> String {
    match bench_with(analysis, body) {
        Ok(_) => panic!("{analysis} {body}: compiled"),
        Err(failure) => failure.to_string(),
    }
}

const LOOP: &str = "analysis loop { from = 1kHz; to = 1MHz; points_per_decade = 10; probe = R1.2; }";

#[test]
fn a_loop_analysis_names_its_probe_and_reads_the_loop_gain() {
    let bench = bench_with(
        LOOP,
        "measure pm = phase_margin(loop_gain()) { min = 45deg; }
         measure gm = gain_margin(loop_gain());
         measure fc = crossover(loop_gain());
         measure f180 = phase_crossover(loop_gain());",
    )
    .unwrap();
    let Analysis::Loop(analysis) = &bench.analysis else { panic!("a loop analysis") };
    assert_eq!(analysis.probe.to_string(), "R1.2");
    assert_eq!(analysis.net.to_string(), "VOUT");
    let units: Vec<String> = bench.measures.iter().map(|m| m.expression.ty.units.to_string()).collect();
    assert_eq!(units, ["Angle", "Gain", "Frequency", "Frequency"]);
    // No plot: the Bode plot of the loop gain.
    let plotted: Vec<&ExpressionKind> = bench.plots[0].expressions.iter().map(|e| &e.kind).collect();
    assert!(matches!(plotted[0], ExpressionKind::Call { function: Function::Db, .. }));
    assert!(matches!(plotted[1], ExpressionKind::Call { function: Function::Phase, .. }));
    let ExpressionKind::Call { arguments, .. } = plotted[0] else { unreachable!() };
    assert!(matches!(arguments[0].kind, ExpressionKind::Probe(Probe::LoopGain)));
    let json = bench_json(&bench);
    assert!(json.contains("\"kind\": \"loop\"") && json.contains("\"probe\": \"R1.2\""), "{json}");
    // A plot of its own replaces the default.
    let bench = bench_with(LOOP, "plot mag(loop_gain());").unwrap();
    assert_eq!(bench.plots.len(), 1);
    assert_eq!(bench.plots[0].expressions.len(), 1);
}

#[test]
fn loop_probe_diagnostics() {
    let with = |probe: &str| format!("analysis loop {{ from = 1kHz; to = 1MHz; points_per_decade = 10; {probe} }}");
    let missing = error(&with(""), "");
    assert!(missing.contains("SIM098") && missing.contains("needs its probe"), "{missing}");
    let unknown = error(&with("probe = R9.1;"), "");
    assert!(unknown.contains("SIM098") && unknown.contains("no component"), "{unknown}");
    let ignored = error(&with("probe = R3.1;"), "");
    assert!(ignored.contains("SIM098") && ignored.contains("ignored"), "{ignored}");
    let net = error(&with("probe = VOUT;"), "");
    assert!(net.contains("SIM098") && net.contains("no component pin"), "{net}");
    let pin = error(&with("probe = R1.3;"), "");
    assert!(pin.contains("SIM039") && pin.contains("no pin"), "{pin}");
    let bounds = error("analysis loop { from = 1kHz; to = 1MHz; points_per_decade = 0; probe = R1.2; }", "");
    assert!(bounds.contains("SIM040") && bounds.contains("points_per_decade"), "{bounds}");
    let order = error("analysis loop { from = 1MHz; to = 1kHz; points_per_decade = 10; probe = R1.2; }", "");
    assert!(order.contains("SIM040") && order.contains("0 < from < to"), "{order}");
    let unknown_key =
        error("analysis loop { from = 1kHz; to = 1MHz; points_per_decade = 10; probe = R1.2; prob = 1; }", "");
    assert!(unknown_key.contains("SIM040"), "{unknown_key}");
}

#[test]
fn loop_functions_need_a_loop_gain() {
    let ac = "analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 10; }";
    let outside = error(ac, "measure t = mag(at(loop_gain(), 10kHz));");
    assert!(outside.contains("SIM099") && outside.contains("analysis loop"), "{outside}");
    // The margins of a loop gain built by hand in an ac analysis: allowed.
    let bench = bench_with(ac, "measure pm = phase_margin(-V(VOUT) / V(VIN));").unwrap();
    assert_eq!(bench.measures[0].expression.ty.units.to_string(), "Angle");
    let dimension = error(ac, "measure pm = phase_margin(V(VOUT));");
    assert!(dimension.contains("SIM099") && dimension.contains("dimensionless"), "{dimension}");
    let real = error(LOOP, "measure pm = phase_margin(mag(loop_gain()));");
    assert!(real.contains("SIM099") && real.contains("complex"), "{real}");
    let probes = error(LOOP, "measure v = mag(at(V(VOUT), 10kHz));");
    assert!(probes.contains("SIM101") && probes.contains("analysis loop"), "{probes}");
    let args = error(LOOP, "measure t = mag(at(loop_gain(R1), 10kHz));");
    assert!(args.contains("SIM064"), "{args}");
}

#[test]
fn a_pole_zero_analysis_has_the_built_in_stable_measure() {
    let bench = bench_with("analysis pz;", "measure d = damping();").unwrap();
    assert!(matches!(bench.analysis, Analysis::Pz(_)));
    let names: Vec<&str> = bench.measures.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["d", "stable"]);
    let stable = &bench.measures[1];
    assert_eq!(stable.expression.ty.units, Units::DIMENSIONLESS);
    assert!(stable.min.is_some() && stable.max.is_none());
    let bench = bench_with("analysis pz { input = INPUT; output = V(VOUT); }", "").unwrap();
    let Analysis::Pz(pz) = &bench.analysis else { panic!("pz") };
    assert_eq!(pz.input.as_ref().unwrap().instance, "supply INPUT");
    assert_eq!(pz.output.as_ref().unwrap().0.to_string(), "VOUT");
    let json = bench_json(&bench);
    assert!(json.contains("\"kind\": \"pz\""), "{json}");
}

#[test]
fn pole_zero_diagnostics() {
    let own = error("analysis pz;", "measure stable = damping();");
    assert!(own.contains("SIM100") && own.contains("built-in"), "{own}");
    let half = error("analysis pz { output = V(VOUT); }", "");
    assert!(half.contains("SIM100") && half.contains("together"), "{half}");
    let input = error("analysis pz { input = R1; output = V(VOUT); }", "");
    assert!(input.contains("SIM100") && input.contains("no source"), "{input}");
    let ac = "analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 10; }";
    let outside = error(ac, "measure d = damping();");
    assert!(outside.contains("SIM100") && outside.contains("analysis pz"), "{outside}");
    let probes = error("analysis pz;", "measure v = V(VOUT);");
    assert!(probes.contains("SIM101") && probes.contains("analysis pz"), "{probes}");
    let plot = error("analysis pz;", "plot damping();");
    assert!(plot.contains("SIM044") && plot.contains("analysis pz"), "{plot}");
}

#[test]
fn stability_sections_in_one_bench() {
    let source = "bench B for RC {
        replace INPUT with source { out = VIN; ref = GND; ac = 1V; }
        ignore R3;
        analysis gain: loop { from = 1kHz; to = 1MHz; points_per_decade = 10; probe = R2.1; }
        measure pm = phase_margin(loop_gain());
        analysis poles: pz;
        measure d = damping();
        analysis ac { from = 1kHz; to = 1MHz; points_per_decade = 10; }
        measure v = mag(at(V(VOUT), 10kHz));
    }";
    let benches = compile_benches_source(source, "b.copper", &rc()).unwrap();
    let names: Vec<&str> = benches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["B.gain", "B.poles", "B.ac"]);
    assert_eq!(benches[1].measures.len(), 2, "d and the built-in stable");
}

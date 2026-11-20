//! The compile-time side of the phase F2 bench language: `if(...)`, the
//! noise of named and derived quantities, named analyses, nets of one name,
//! fixtures that use fixtures, matched tolerance statements, parameter sets
//! per supply, `I()` of a supply and `Gohm`. Nothing here simulates
//! (`copper-simulate/tests/bench_language.rs` runs the same features).

use copper_lang::bench::{Bench, bench_json, compile_bench_sections, compile_bench_source, compile_benches_source};
use copper_lang::compiler::{Design, compile_design_source};
use copper_lang::expression::{ExpressionKind, Function, Probe};
use copper_lang::parse;
use copper_lang::syntax::{BenchStatement, Declaration};

const RC_BOARD: &str = "board RC {
    use library \"tiny\";
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    component R_BIG: RESISTOR { value = 1.5Gohm; }
    net VIN { R1.1; }
    net VOUT { R1.2; C1.1; R_BIG.1; }
    net GND { C1.2; R_BIG.2; }
    supply INPUT { voltage = 1V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

fn rc() -> Design {
    compile_design_source(RC_BOARD, "rc.copper", false, true).unwrap()
}

fn bench_with(analysis: &str, body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; ac = 1V; output_resistance = 50ohm; }}\n \
         {analysis}\n{body}\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc())
}

const AC: &str = "analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }";
const NOISE: &str =
    "analysis noise { from = 10Hz; to = 100kHz; points_per_decade = 10; output = V(VOUT), V(VIN); input = INPUT; }";

fn error(analysis: &str, body: &str) -> String {
    match bench_with(analysis, body) {
        Ok(_) => panic!("{body}: compiled"),
        Err(failure) => failure.to_string(),
    }
}

#[test]
fn gohm_is_a_resistance_unit() {
    let design = rc();
    let bench = bench_with(AC, "measure v = mag(at(V(VOUT), 1kHz));").unwrap();
    let big = bench.components.iter().find(|component| component.reference == "R_BIG").unwrap();
    let copper_lang::bench::Treatment::Passive { value, .. } = &big.treatment else { panic!("a passive") };
    assert_eq!(
        (value.value.to_string(), value.value.base_value().to_sci_string()),
        ("1.5 Gohm".into(), "1500000000.0".into())
    );
    drop(design);
}

#[test]
fn conditionals_are_typed() {
    let bench = bench_with(AC, "measure m = max(if(mag(V(VOUT)) > 0.5V, mag(V(VOUT)), 0V));").unwrap();
    let ExpressionKind::Call { function: Function::Max, arguments } = &bench.measures[0].expression.kind else {
        panic!("max")
    };
    assert!(matches!(arguments[0].kind, ExpressionKind::Call { function: Function::If, .. }));
    assert!(arguments[0].ty.swept && !arguments[0].ty.complex);
    assert!(error(AC, "measure m = max(if(mag(V(VOUT)), 1, 0));").contains("SIM072: if takes a comparison"));
    assert!(error(AC, "measure m = max(if(mag(V(VOUT)) > 0.5V, mag(V(VOUT)), 0));").contains("SIM065"));
    assert!(error(AC, "measure m = max(if(V(VOUT) > 0.5V, 1, 0));").contains("SIM072: a comparison needs real values"));
    assert!(error(AC, "measure m = if(1 > 0, 1);").contains("SIM064: if takes 3 arguments"));
}

#[test]
fn noise_of_named_and_derived_quantities() {
    let bench = bench_with(
        NOISE,
        "let h = V(VOUT) / V(VIN);
         measure a = noise_rms(noise(V(VOUT)), 10Hz, 1kHz);
         measure b = noise_share(noise(V(VIN), R1), 10Hz, 1kHz);
         measure c = noise_rms(noise(h), 10Hz, 1kHz);
         measure d = noise_share(noise(h, R1), 10Hz, 1kHz);
         measure e = at(input_noise(V(VOUT)), 1kHz);",
    )
    .unwrap();
    let probe = |k: usize| -> Probe {
        let ExpressionKind::Call { arguments, .. } = &bench.measures[k].expression.kind else { panic!("a call") };
        let ExpressionKind::Probe(probe) = &arguments[0].kind else { panic!("a probe") };
        probe.clone()
    };
    assert!(matches!(probe(0), Probe::Noise { contributor: None, quantity: Some(_) }));
    assert!(matches!(probe(1), Probe::Noise { contributor: Some(ref name), quantity: Some(_) } if name == "R1"));
    // The noise of h = V(VOUT) / V(VIN) is dimensionless per root hertz: noise_rms of it is dimensionless.
    assert!(bench.measures[2].expression.ty.units.is_dimensionless());
    let json = bench_json(&bench);
    assert!(json.contains("\"outputs\": [") && json.contains("\"quantity\": {"), "{json}");
    // With two outputs the quantity must be named; a quantity must be linear in V probes.
    assert!(error(NOISE, "measure a = noise_rms(noise(), 10Hz, 1kHz);").contains("SIM095"));
    assert!(error(NOISE, "measure a = noise_rms(noise(R1), 10Hz, 1kHz);").contains("SIM095"));
    assert!(
        error(NOISE, "measure a = noise_share(noise(V(VOUT), R1) + noise(V(VIN), R1), 10Hz, 1kHz);").contains("SIM088")
    );
    assert!(
        error(NOISE, "measure a = noise_rms(noise(I(R1)), 10Hz, 1kHz);").contains("SIM095: noise(Q) needs Q built")
    );
    assert!(error(NOISE, "let m = mag(V(VOUT)); measure a = noise_rms(noise(m), 10Hz, 1kHz);").contains("SIM095"));
    assert!(error(NOISE, "measure a = noise_rms(noise(V(VOUT), R9), 10Hz, 1kHz);").contains("SIM087"));
    assert!(error(NOISE, "measure a = at(input_noise(), 1kHz);").contains("SIM095"));
    // One output: noise() and noise(X) as before, and noise(LET) of a derived quantity.
    let one = NOISE.replace("output = V(VOUT), V(VIN);", "output = V(VOUT);");
    bench_with(&one, "let h = V(VOUT) / V(VIN); measure a = noise_rms(noise(), 10Hz, 1kHz); measure b = noise_rms(noise(h), 10Hz, 1kHz); measure c = noise_share(noise(R1), 10Hz, 1kHz);").unwrap();
    assert!(error(&one, "let R1 = V(VOUT); measure a = noise_rms(noise(R1), 10Hz, 1kHz);").contains("SIM087"));
}

#[test]
fn named_analyses_and_sections() {
    let source = "bench S for RC {
        replace INPUT with source { out = VIN; ref = GND; dc = 1V; ac = 1V; }
        let h = V(VOUT) / V(VIN);
        analysis op;
        measure dc = V(VOUT);
        analysis wide: ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }
        measure gain = mag(at(h, 1kHz));
        plot mag(h);
    }";
    let document = parse(source, "s.copper").unwrap();
    let names: Vec<Option<String>> = document
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Bench(BenchStatement::Analysis(item)) => Some(item.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, [None, Some("wide".to_owned())]);
    let benches = compile_bench_sections(&document, &[], &rc()).unwrap();
    let summary: Vec<(&str, Option<&str>, usize, usize)> = benches
        .iter()
        .map(|bench| (bench.name.as_str(), bench.section.as_deref(), bench.measures.len(), bench.plots.len()))
        .collect();
    assert_eq!(summary, [("S.op", Some("op"), 1, 0), ("S.wide", Some("wide"), 1, 1)]);
    assert_eq!(benches[1].bench_name(), "S");
    // The op section has no use for h: it is not in that section.
    assert!(benches[0].lets.is_empty() && benches[1].lets.len() == 1);
    assert!(bench_json(&benches[1]).contains("\"section\": \"wide\""));
    // A bench with several analyses is several benches: the single-bench call says so.
    let failure = compile_bench_source(source, "s.copper", &rc()).unwrap_err().to_string();
    assert!(failure.contains("SIM097") && failure.contains("2 analyses"), "{failure}");
    assert_eq!(compile_benches_source(source, "s.copper", &rc()).unwrap().len(), 2);
}

#[test]
fn nets_of_one_name_are_one_net_and_supplies_have_currents() {
    let bench = bench_with(
        AC,
        "component CA: CAPACITOR { value = 1nF; }
         component CB: CAPACITOR { value = 0pF; }
         net VOUT { CA.1; }
         net GND { CA.2; }
         net VOUT { CB.1; }
         net GND { CB.2; }
         measure i = mag(at(I(INPUT), 1kHz));
         measure j = mag(at(I(INPUT.out), 1kHz));",
    )
    .unwrap();
    let vout = bench.nets.iter().find(|net| net.name == "VOUT").unwrap();
    assert_eq!(vout.endpoints.iter().map(|e| e.to_string()).collect::<Vec<_>>(), ["CA.1", "CB.1"]);
    assert_eq!(bench.nets.len(), 2);
    let probe = |k: usize| -> (String, String) {
        let ExpressionKind::Call { arguments, .. } = &bench.measures[k].expression.kind else { panic!("mag") };
        let ExpressionKind::Call { arguments, .. } = &arguments[0].kind else { panic!("at") };
        let ExpressionKind::Probe(Probe::Current { component, pin }) = &arguments[0].kind else { panic!("I") };
        (component.clone(), pin.clone())
    };
    assert_eq!(probe(0), ("supply INPUT".to_owned(), copper_lang::bench::SUPPLY_NET.to_owned()));
    assert_eq!(probe(1), ("supply INPUT".to_owned(), "out".to_owned()));
    assert!(error(AC, "measure i = mag(at(I(GROUND), 1kHz));").contains("SIM045: I(GROUND)"));
}

#[test]
fn matched_tolerance_statements_parse_and_check() {
    let document = parse("bench B for RC { tolerance R1, C1 = 1% match 0.1%; }", "b.copper").unwrap();
    let Some(Declaration::Bench(BenchStatement::Variation(item))) = document.declarations.get(1) else {
        panic!("a tolerance statement")
    };
    assert_eq!((item.target.as_deref(), item.others.as_slice()), (Some("R1"), ["C1".to_owned()].as_slice()));
    assert!(item.match_value.is_some());
    // Two passives matched by the bench: one group, its members the two values.
    let bench =
        bench_with(AC, "tolerance R1, R_BIG = 1% match 0.1%; worstcase; measure m = mag(at(V(VOUT), 1kHz));").unwrap();
    let variation = bench.variation.unwrap();
    assert_eq!(variation.groups.len(), 1);
    assert_eq!(variation.groups[0].members.iter().map(|m| m.text()).collect::<Vec<_>>(), ["R1", "R_BIG"]);
    assert!(
        error(
            AC,
            "tolerance R1 = 1%; tolerance R1, R_BIG = 1% match 0.1%; worstcase; measure m = mag(at(V(VOUT), 1kHz));"
        )
        .contains("SIM091")
    );
    assert!(
        error(AC, "tolerance R_VALUE, R1 = 1% match 0.1%; worstcase; measure m = mag(at(V(VOUT), 1kHz));")
            .contains("SIM091: 'R_VALUE' is a param")
    );
}

#[test]
fn parameter_sets_per_supply_are_checked() {
    let part = |set: &str| {
        format!(
            "part AMP {{
    category = \"amplifier.opamp\";
    pin INN {{ number = \"2\"; domains = \"analog\"; directions = \"input\"; }}
    pin INP {{ number = \"3\"; domains = \"analog\"; directions = \"input\"; }}
    pin VN {{ number = \"4\"; domains = \"power\"; directions = \"input\"; }}
    pin OUT {{ number = \"6\"; domains = \"analog\"; directions = \"output\"; }}
    pin VP {{ number = \"7\"; domains = \"power\"; directions = \"input\"; }}
    sim opamp {{ inp = INP; inn = INN; out = OUT; vp = VP; vn = VN; gbw = 1MHz; open_loop_gain = 100dB;
        slew_rate = 10V/us; {set} }}
}}"
        )
    };
    let board = "board Amp {
    import p \"./parts\";
    component U1: p.AMP;
    net VP { U1.VP; }
    net VN { U1.VN; }
    net IO { U1.INP; U1.INN; U1.OUT; }
}";
    let compile = |set: &str| {
        let directory = std::env::temp_dir().join(format!("copper-f2-sets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("parts")).unwrap();
        std::fs::write(directory.join("parts/amp.copper"), part(set)).unwrap();
        std::fs::write(directory.join("board.copper"), board).unwrap();
        copper_lang::compiler::compile_design_file(&directory.join("board.copper"), false, true)
    };
    let design = compile("supply 3.3V { slew_rate = 5V/us; } supply 10V { }").unwrap();
    let model = &design.electrical.library.values().find(|part| part.name.ends_with("AMP")).unwrap().sim[0];
    assert_eq!(model.supply_sets.iter().map(|set| set.span.to_string()).collect::<Vec<_>>(), ["3.3 V", "10 V"]);
    assert_eq!(model.supply_sets[0].parameters["slew_rate"].value.to_string(), "5 V/us");
    let json = copper_lang::serializer::design_json(&design);
    assert!(json.contains("\"supply_sets\""), "{json}");
    for (set, code) in [
        ("supply 3.3V { gain = 1; }", "SIM096"),
        ("supply 3.3V { } supply 3.3V { }", "SIM096"),
        ("supply 3.3A { }", "SIM096"),
        ("supply -1V { }", "SIM096"),
        ("supply 3.3V { gbw = 0Hz; }", "SIM019"),
    ] {
        let failure = compile(set).unwrap_err().to_string();
        assert!(failure.contains(code), "{set}: {failure}");
    }
}

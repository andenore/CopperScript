//! The simulation frontend (phase P0 of `docs/integrated-simulation-plan.md`):
//! board `param`, part `sim` blocks, bench documents and typed bench
//! expressions. Nothing here simulates; these tests check parsing, the
//! checks that are decidable before a simulation and the plain-data result.

use std::fs;
use std::path::{Path, PathBuf};

use copper_lang::bench::{
    self, Analysis, Bench, NetJoin, SupplyTreatment, Treatment, bench_json, compile_bench_file, compile_bench_source,
    load_benches,
};
use copper_lang::compiler::{Design, compile_design_file, compile_design_source};
use copper_lang::expression::{ExpressionKind, Function, Node, Probe, Units};
use copper_lang::parse;
use copper_lang::serializer::design_json;
use copper_lang::simulation::SimKind;
use copper_lang::syntax::{BenchStatement, BinaryOperator, Declaration, DocumentKind, ExprKind};
use copper_units::Dimension;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn example(path: &str) -> PathBuf {
    repository().join("examples/simulation").join(path)
}

fn compile_example(path: &str) -> Design {
    compile_design_file(&example(path), false, true).unwrap_or_else(|failure| panic!("{path}: {failure}"))
}

/// A temporary project: `board.copper`, `parts/*.copper` (one part per file) and `benches/*.copper`.
fn project(name: &str, parts: &[&str], board: &str, benches: &[&str]) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-simulation-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
    for (index, source) in parts.iter().enumerate() {
        fs::write(directory.join("parts").join(format!("p{index}.copper")), source).unwrap();
    }
    if !benches.is_empty() {
        fs::create_dir_all(directory.join("benches")).unwrap();
        for (index, source) in benches.iter().enumerate() {
            fs::write(directory.join("benches").join(format!("b{index}.copper")), source).unwrap();
        }
    }
    fs::write(directory.join("board.copper"), board).unwrap();
    directory.join("board.copper")
}

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

/// A bench for `RC` with `body` after an AC source on VIN and an analysis.
fn rc_bench(body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let source = format!(
        "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; ac = 1V; }}\n \
         analysis ac {{ from = 10Hz; to = 100kHz; points_per_decade = 10; }}\n{body}\n}}"
    );
    compile_bench_source(&source, "b.copper", &rc())
}

fn rc_error(body: &str) -> String {
    error_of(rc_bench(body))
}

#[test]
fn rc_filter_example_bench() {
    let board = example("rc_filter.copper");
    let design = compile_example("rc_filter.copper");
    let benches = load_benches(&board, &design).unwrap();
    assert_eq!(benches.len(), 1);
    let bench = &benches[0];
    assert_eq!(
        (bench.name.as_str(), bench.board.as_str(), bench.ground.as_str()),
        ("RcCorner", "SimulationRCFilter", "GND")
    );
    let Analysis::Ac { from, to, points_per_decade } = &bench.analysis else { panic!("ac") };
    assert_eq!(
        (from.value.to_string(), to.value.to_string(), *points_per_decade),
        ("10 Hz".into(), "100 kHz".into(), 50)
    );
    let treatments: Vec<(&str, &str)> =
        bench.components.iter().map(|component| (component.reference.as_str(), component.treatment.name())).collect();
    assert_eq!(treatments, [("R1", "passive"), ("C1", "passive")]);
    let supplies: Vec<(&str, &SupplyTreatment)> =
        bench.supplies.iter().map(|supply| (supply.name.as_str(), &supply.treatment)).collect();
    assert_eq!(supplies, [("INPUT", &SupplyTreatment::Replaced(0)), ("GROUND", &SupplyTreatment::Ground)]);
    let replacement = &bench.replacements[0];
    assert!(replacement.supply);
    assert_eq!(replacement.spec.kind, SimKind::Source);
    assert_eq!(replacement.nodes["out"], Node::Net("VIN".into()));
    assert_eq!(replacement.spec.parameters["ac"].value.to_string(), "1 V");
    let names: Vec<&str> = bench.measures.iter().map(|measure| measure.name.as_str()).collect();
    assert_eq!(names, ["gain_at_corner", "phase_at_corner"]);
    let gain = &bench.measures[0];
    assert_eq!(gain.expression.ty.units, Units::of(Dimension::Gain));
    assert_eq!(gain.min.as_ref().unwrap().value.to_string(), "-3.1 dB");
    let phase = &bench.measures[1];
    assert_eq!(phase.expression.ty.units, Units::of(Dimension::Angle));
    assert_eq!(bench.lets[0].expression.ty.units, Units::DIMENSIONLESS);
    assert!(bench.lets[0].expression.ty.complex && bench.lets[0].expression.ty.swept);
    assert_eq!(bench.plots[0].expressions.len(), 2);
    let json = bench_json(bench);
    assert!(json.contains("\"schema\": \"copperscript-bench/v0.1\""), "{json}");
}

#[test]
fn howland_example_board_and_bench() {
    let board = example("howland_pump/board.copper");
    let design = compile_example("howland_pump/board.copper");
    // Board params and their uses.
    let params = &design.electrical.params;
    assert_eq!(params.keys().collect::<Vec<_>>(), ["R_GAIN", "R_SET"]);
    let gain_users: Vec<&str> = params["R_GAIN"].uses.iter().map(|item| item.component.as_str()).collect();
    assert_eq!(gain_users, ["R1", "R2", "R3", "R4"]);
    assert_eq!(params["R_SET"].uses[0].component, "R_SENSE");
    let sense = design.electrical.components.iter().find(|component| component.reference == "R_SENSE").unwrap();
    assert_eq!(
        format!("{:?}", sense.value),
        format!("{:?}", Some(copper_lang::model::ComponentValue::Quantity(params["R_SET"].value.clone())))
    );
    // The op-amp part's sim block.
    let part = design.electrical.library.values().find(|part| part.name.ends_with("OPAMP_ADA4817_LIKE")).unwrap();
    assert_eq!(part.sim.len(), 1);
    let model = &part.sim[0];
    assert_eq!(model.spec.kind, SimKind::Opamp);
    assert_eq!(
        model.pins.iter().map(|(role, pin)| format!("{role}={pin}")).collect::<Vec<_>>(),
        ["inp=INP", "inn=INN", "out=OUT", "vp=VP", "vn=VN"]
    );
    assert_eq!(model.unmodelled, ["NC"]);
    assert_eq!(model.spec.parameters["gbw"].value.to_string(), "400 MHz");
    assert_eq!(model.spec.parameters["open_loop_gain"].value.to_string(), "65 dB");
    assert_eq!(model.spec.parameters["input_capacitance_common"].value.to_string(), "1.3 pF");
    let json = design_json(&design);
    assert!(json.contains("\"params\": {"), "board JSON names its params");
    assert!(json.contains("\"sim\": ["), "part JSON carries its sim block");

    let benches = load_benches(&board, &design).unwrap();
    let names: Vec<&str> = benches.iter().map(|bench| bench.name.as_str()).collect();
    assert_eq!(names, ["CurrentFlatness", "MeasurementNoise", "OutputImpedance", "ResistorMatching", "StepResponse"]);
    let bench = &benches[0];
    assert_eq!(bench.name, "CurrentFlatness");
    let treatment = |reference: &str| {
        bench.components.iter().find(|component| component.reference == reference).unwrap().treatment.clone()
    };
    assert_eq!(treatment("U1"), Treatment::PartModel);
    assert_eq!(treatment("J_IN"), Treatment::Replaced(0));
    assert_eq!(treatment("J_OUT"), Treatment::Ignored);
    let Treatment::Passive { value, .. } = treatment("R_SENSE") else { panic!("passive") };
    assert_eq!((value.value.to_string(), value.param.as_deref()), ("10 kohm".into(), Some("R_SET")));
    let Treatment::Passive { value, .. } = treatment("R_LOAD") else { panic!("passive") };
    assert_eq!(value.param.as_deref(), Some("R_LOAD_VALUE"));
    let load_net = bench.nets.iter().find(|net| net.name == "J_OUT.1").unwrap();
    assert_eq!(load_net.join, NetJoin::BoardNet("LOAD".into()));
    assert_eq!(
        bench.steps[0].values.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["100 ohm", "1 kohm", "10 kohm"]
    );
    let i_nominal = &bench.lets[1].expression;
    assert_eq!(i_nominal.ty.units, Units::of(Dimension::Current), "V / ohm is a current");
    let flatness = &bench.measures[0];
    assert_eq!(flatness.expression.ty.units, Units::DIMENSIONLESS);
    assert_eq!(flatness.max.as_ref().unwrap().value.to_string(), "1 %");
    assert!(flatness.min.is_none());
    let ExpressionKind::Call { function: Function::Max, .. } = &flatness.expression.kind else { panic!("max") };
    let ExpressionKind::Probe(Probe::Current { component, pin }) = &bench.lets[0].expression.kind else { panic!() };
    assert_eq!((component.as_str(), pin.as_str()), ("R_LOAD", "1"));
    assert_eq!(bench.measures[2].max.as_ref().unwrap().value.to_string(), "10 deg");
    let replacement = &bench.replacements[0];
    assert!(!replacement.supply);
    assert_eq!(replacement.nodes["out"], Node::Net("VIN".into()));
    assert!(replacement.spec.waveform.is_some());
    // The models of the circuit, for the lowering step.
    let u1 = bench.components.iter().find(|component| component.reference == "U1").unwrap();
    assert_eq!(bench.models(&design, u1).len(), 1);
    bench_json(bench);
}

#[test]
fn existing_boards_gain_no_keys() {
    // A board without params or sim parts serializes exactly as before: no new keys.
    let json = design_json(&compile_example("rc_filter.copper"));
    assert!(!json.contains("\"params\""));
    assert!(!json.contains("\"sim\""));
}

#[test]
fn bench_documents_parse() {
    let source = std::fs::read_to_string(example("howland_pump/benches/flatness.copper")).unwrap();
    let document = parse(&source, "flatness.copper").unwrap();
    assert_eq!(document.kind, DocumentKind::Bench);
    let Some(Declaration::Bench(BenchStatement::Target(target))) = document.declarations.first() else { panic!() };
    assert_eq!(target.board, "HowlandPump");
    // `sim = value;` in a part stays a property, as `build` does.
    assert!(parse("part P { sim = 3; }", "p.copper").is_ok());
    // A misspelled bench statement suggests one.
    let failure = error_of(parse("bench B for X {\n mesure m = 1;\n}", "b.copper"));
    assert_eq!(failure, "b.copper:2:2: PAR002: unknown declaration 'mesure' (did you mean 'measure'?)");
    // Params are board, module and bench declarations.
    assert!(parse("module M {\n param X = 1V;\n}", "m.copper").is_ok());
}

fn let_expression(text: &str) -> copper_lang::syntax::Expr {
    let document = parse(&format!("bench B for X {{ let x = {text}; }}"), "b.copper").unwrap();
    let Some(Declaration::Bench(BenchStatement::Let(item))) = document.declarations.get(1) else { panic!() };
    item.expression.clone()
}

/// The expression tree in prefix notation.
fn shape(expression: &copper_lang::syntax::Expr) -> String {
    match &expression.kind {
        ExprKind::Number(text) => text.clone(),
        ExprKind::Quantity { value, unit } => format!("{value}{unit}"),
        ExprKind::Name(name) => name.clone(),
        ExprKind::Probe { function, targets, .. } => format!("{function}({})", targets.join(",")),
        ExprKind::Call { function, arguments } => {
            format!("{function}({})", arguments.iter().map(shape).collect::<Vec<_>>().join(","))
        }
        ExprKind::Negate(operand) => format!("(neg {})", shape(operand)),
        ExprKind::Binary { operator, left, right } => {
            format!("({} {} {})", operator.symbol(), shape(left), shape(right))
        }
        ExprKind::Compare { operator, left, right } => {
            format!("({} {} {})", operator.symbol(), shape(left), shape(right))
        }
    }
}

#[test]
fn expression_precedence() {
    assert_eq!(shape(&let_expression("1 + 2 * 3")), "(+ 1 (* 2 3))");
    assert_eq!(shape(&let_expression("(1 + 2) * 3")), "(* (+ 1 2) 3)");
    assert_eq!(shape(&let_expression("1 - 2 - 3")), "(- (- 1 2) 3)");
    assert_eq!(shape(&let_expression("8 / 4 / 2")), "(/ (/ 8 4) 2)");
    assert_eq!(shape(&let_expression("-2 * 3")), "(* (neg 2) 3)");
    assert_eq!(shape(&let_expression("a - -b")), "(- a (neg b))");
    assert_eq!(shape(&let_expression("(V(P) - V(N)) / V(M, GND) * R_TIA")), "(* (/ (- V(P) V(N)) V(M,GND)) R_TIA)");
    assert_eq!(shape(&let_expression("max(abs(mag(I(R1.1)) / 15uA - 1))")), "max(abs((- (/ mag(I(R1.1)) 15uA) 1)))");
    assert_eq!(shape(&let_expression("at(x, 1.5kHz) + 3%")), "(+ at(x,1.5kHz) 3%)");
    assert_eq!(shape(&let_expression("4nV/rtHz * 2")), "(* 4nV/rtHz 2)");
    let Some(ExprKind::Binary { operator: BinaryOperator::Divide, .. }) = Some(let_expression("V(A)/V(B)").kind) else {
        panic!("an adjacent slash between probes divides")
    };
}

#[test]
fn expression_types_and_dimensions() {
    let bench = rc_bench(
        " let z = (V(VIN) - V(VOUT)) / I(R1);\n let tau = R_VALUE * 100nF;\n \
         measure zmax = max(mag(z)) { max = 2kohm; }\n measure t = tau { min = 99us; max = 101us; }",
    )
    .unwrap();
    assert_eq!(bench.lets[0].expression.ty.units, Units::of(Dimension::Resistance), "V / A is ohm");
    assert_eq!(bench.lets[1].expression.ty.units, Units::of(Dimension::Time), "ohm * F is s");
    assert!(!bench.lets[1].expression.ty.swept);
    assert_eq!(rc_error(" let bad = V(VOUT) + I(R1);"), "b.copper:4:20: SIM065: cannot add Voltage and Current");
    assert_eq!(rc_error(" let bad = 1 - 1V;"), "b.copper:4:14: SIM065: cannot subtract Voltage from dimensionless");
    assert!(
        rc_error(" measure m = max(mag(V(VOUT))) { max = 1A; }")
            .contains("SIM043: measure m is Voltage, but its max limit 1 A is Current")
    );
    assert!(rc_error(" measure m = V(VOUT);").contains("SIM043: measure m is swept over frequency"));
    assert!(rc_error(" measure m = at(V(VOUT), 1kHz);").contains("SIM043: measure m is complex"));
    assert!(rc_error(" measure m = max(V(VOUT));").contains("SIM067: max needs a real value"));
    assert!(rc_error(" measure m = max(R_VALUE);").contains("SIM066: max reduces over the ac frequency axis"));
    assert!(rc_error(" measure m = mag(at(V(VOUT), 1V));").contains("SIM068"));
    assert!(rc_error(" plot V(VOUT);").contains("SIM044: plot needs a real value"));
    assert!(
        rc_error(" let m = mean(mag(V(VOUT)), 2);")
            .contains("SIM064: mean takes 1 argument, or 3 (x, from, to), found 2")
    );
    let unknown = rc_error(" let m = magn(V(VOUT));");
    assert!(
        unknown.contains("SIM063: unknown function 'magn'") && unknown.contains("did you mean 'mag'?"),
        "{unknown}"
    );
    let name = rc_error(" let m = R_VALU;");
    assert!(name.contains("SIM062") && name.contains("did you mean 'R_VALUE'?"), "{name}");
    // `-` is part of identifiers, so subtraction needs spaces.
    let joined = rc_error(" let m = 1kohm-R_VALUE;");
    assert!(joined.contains("SIM061") && joined.contains("write spaces around '-'"), "{joined}");
    // Names: lets are visible after they are declared, and names are unique.
    assert!(rc_error(" let a = b;\n let b = 1;").contains("SIM062"));
    assert!(rc_error(" let a = 1;\n let a = 2;").contains("SIM042"));
    assert!(rc_error(" let R_VALUE = 1;").contains("SIM042: 'R_VALUE' is already a param"));
}

#[test]
fn op_analysis_has_no_axis() {
    let source = "bench B for RC {\n analysis op;\n measure v = V(VOUT) { min = 0.9V; max = 1.1V; }\n}";
    let bench = compile_bench_source(source, "b.copper", &rc()).unwrap();
    assert_eq!(bench.analysis, Analysis::Op);
    assert!(!bench.measures[0].expression.ty.swept && !bench.measures[0].expression.ty.complex);
    let failure = error_of(compile_bench_source(
        "bench B for RC {\n analysis op;\n measure v = max(V(VOUT));\n}",
        "b.copper",
        &rc(),
    ));
    assert!(
        failure.contains("SIM066: max reduces over the ac frequency axis, but analysis op has no axis"),
        "{failure}"
    );
    let failure =
        error_of(compile_bench_source("bench B for RC {\n analysis op;\n plot V(VOUT);\n}", "b.copper", &rc()));
    assert!(failure.contains("SIM044: plot needs a swept value, but analysis op gives single values"), "{failure}");
}

#[test]
fn unknown_probe_targets() {
    let failure = rc_error(" let v = V(VOUTT);");
    assert!(failure.starts_with("b.copper:4:12: SIM039: unknown node 'VOUTT'"), "{failure}");
    assert!(failure.contains("did you mean 'VOUT'?"), "{failure}");
    assert!(rc_error(" let v = V(R1.3);").contains("SIM039: 'R1' has no pin '3'"));
    assert!(rc_error(" let v = V(R9.1);").contains("SIM039: unknown node 'R9.1': 'R9' is no board or bench component"));
    let current = rc_error(" let i = I(RR1);");
    assert!(
        current.contains("SIM045: unknown component or supply 'RR1' in I(...)")
            && current.contains("did you mean 'R1'?"),
        "{current}"
    );
    assert!(rc_error(" let i = I(R1.9);").contains("SIM045: 'R1' has no pin '9'"));
    assert!(rc_error(" let v = V(A, B, C);").contains("SIM051"));
    // Node names resolve: a board pin's node is its net.
    let bench = rc_bench(" let v = V(C1.1, R1.1);").unwrap();
    let ExpressionKind::Probe(Probe::Voltage { node, reference }) = &bench.lets[0].expression.kind else { panic!() };
    assert_eq!((node, reference.as_ref()), (&Node::Net("VOUT".into()), Some(&Node::Net("VIN".into()))));
}

#[test]
fn benches_name_their_board() {
    let failure = error_of(compile_bench_source("bench B for Other {\n analysis op;\n}", "b.copper", &rc()));
    assert_eq!(failure, "b.copper:1:13: SIM030: bench 'B' is for board 'Other', but the board is 'RC'");
    // In a board's benches/ directory, a bench for a board no sibling source declares is an error ...
    let board = project("missing-board", &[], RC_BOARD, &["bench Lost for RCC {\n analysis op;\n}"]);
    let design = compile_design_file(&board, false, true).unwrap();
    let failure = error_of(load_benches(&board, &design));
    assert!(failure.contains("SIM030: bench 'Lost' is for board 'RCC', which no source in"), "{failure}");
    assert!(failure.contains("did you mean 'RC'?"), "{failure}");
    // ... and a bench for a sibling board is that board's.
    let board = project("sibling-board", &[], RC_BOARD, &["bench Other for Sibling {\n analysis op;\n}"]);
    fs::write(board.with_file_name("sibling.copper"), "board Sibling {\n}\n").unwrap();
    let design = compile_design_file(&board, false, true).unwrap();
    assert!(load_benches(&board, &design).unwrap().is_empty());
    // A board document is not a bench.
    let failure = error_of(compile_bench_source(RC_BOARD, "rc.copper", &rc()));
    assert!(failure.contains("SIM030: expected a bench document"), "{failure}");
}

#[test]
fn bench_circuit_checks() {
    let design = rc();
    let bench = |body: &str| {
        compile_bench_source(&format!("bench B for RC {{\n{body}\n analysis op;\n}}"), "b.copper", &design)
    };
    // Every board component needs a model; tiny's VOLTAGE_SOURCE has none.
    let board = "board M {\n use library \"tiny\";\n component V1: VOLTAGE_SOURCE;\n component R1: RESISTOR { value = 1kohm; }\n \
                 net A { V1.OUT; R1.1; }\n net GND { V1.GND; R1.2; }\n supply G { voltage = 0V; net = GND; external = true; }\n}";
    let model_less = compile_design_source(board, "m.copper", false, true).unwrap();
    let failure = error_of(compile_bench_source("bench B for M {\n analysis op;\n}", "b.copper", &model_less));
    assert!(
        failure.contains("SIM034: board component 'V1' (part 'VOLTAGE_SOURCE') has no simulation model"),
        "{failure}"
    );
    assert!(compile_bench_source("bench B for M {\n ignore V1;\n analysis op;\n}", "b.copper", &model_less).is_ok());
    // Bench components and nets.
    assert!(error_of(bench(" component R1: RESISTOR { value = 1kohm; }")).contains("SIM032"));
    assert!(error_of(bench(" component RX: RESISTR { value = 1kohm; }")).contains("did you mean 'RESISTOR'?"));
    assert!(
        error_of(bench(" component RX: RESISTOR { value = 1kohm; }\n net VOUT { RX.1; }"))
            .contains("SIM036: bench component pin RX.2 is on no bench net")
    );
    assert!(
        error_of(bench(" component RX: RESISTOR { value = 1kohm; }\n net VOUT { RX.1; R1.1; }")).contains("SIM035")
    );
    let new_net =
        error_of(bench(" component RX: RESISTOR { value = 1kohm; }\n net VOUT { RX.1; }\n net GDN { RX.2; }"));
    assert!(new_net.contains("SIM037") && new_net.contains("did you mean 'GND'?"), "{new_net}");
    assert!(
        error_of(bench(" component RX: RESISTOR { value = 1kohm; }\n net VOUT { RX.1; }\n net C1.1 { RX.2; }"))
            .contains("SIM037: bench nets 'VOUT' and 'C1.1' both join VOUT")
    );
    let joined = bench(" component RX: RESISTOR { value = 1kohm; }\n net C1.1 { RX.1; }\n net GND { RX.2; }").unwrap();
    assert_eq!(joined.nets[0].join, NetJoin::BoardNet("VOUT".into()));
    // Ignore and replace targets.
    assert!(error_of(bench(" ignore R9;")).contains("SIM038: cannot ignore 'R9'"));
    assert!(error_of(bench(" ignore R1, R1;")).contains("SIM038: 'R1' is ignored twice"));
    assert!(
        error_of(bench(" ignore R1;\n replace R1 with load { inp = VOUT; ref = GND; resistance = 1kohm; }"))
            .contains("both replaced and ignored")
    );
    assert!(error_of(bench(" ignore GROUND;")).contains("SIM046"));
    assert!(error_of(bench(" replace R1 with load { inp = VOUT; ref = GND; }")).contains("SIM021"));
    assert!(
        error_of(bench(" replace R1 with load { inp = VOUT; ref = GNDD; resistance = 1kohm; }"))
            .contains("SIM039: unknown node 'GNDD'")
    );
    assert!(
        error_of(bench(" replace R1 with spice { }")).contains("SIM011: sim spice models belong to a part's sim block")
    );
    // Analyses.
    let only = |text: &str| compile_bench_source(&format!("bench B for RC {{\n{text}\n}}"), "b.copper", &design);
    assert!(error_of(only(" let x = 1;")).contains("SIM040: a bench needs an analysis"));
    assert!(error_of(only(" analysis op;\n analysis op;")).contains("SIM097: bench 'B' has two analyses named 'op'"));
    assert!(error_of(only(" analysis tran { }")).contains("SIM069: analysis tran needs a stop time"));
    assert!(
        error_of(only(" analysis ac { from = 1kHz; to = 10Hz; points_per_decade = 10; }")).contains("0 < from < to")
    );
    assert!(
        error_of(only(" analysis ac { from = 1V; to = 10Hz; points_per_decade = 10; }"))
            .contains("SIM018: analysis ac from must be a Frequency")
    );
    assert!(error_of(only(" analysis ac { from = 1Hz; to = 10Hz; }")).contains("needs from, to and points_per_decade"));
}

#[test]
fn param_type_mismatches() {
    // A board param stands in for component quantities of its dimension only.
    let board = |param: &str, use_: &str| {
        format!("board P {{\n use library \"tiny\";\n param X = {param};\n component C1: CAPACITOR {{ {use_} }}\n}}")
    };
    let compile = |source: String| compile_design_source(&source, "p.copper", false, true);
    assert!(compile(board("100nF", "value = X;")).is_ok());
    let failure = error_of(compile(board("10dB", "value = X;")));
    assert!(failure.contains("SIM003: component C1 value takes a Voltage or Resistance"), "{failure}");
    let failure = error_of(compile(board("10kohm", "value = 100nF; capacitance_at_bias = X;")));
    assert!(
        failure.contains("SIM003: component C1 capacitance_at_bias takes a Capacitance, but param 'X' is a Resistance"),
        "{failure}"
    );
    assert!(error_of(compile(board("3", "value = X;"))).contains("SIM001: param 'X' must be a quantity"));
    assert!(error_of(compile(board("\"text\"", "value = X;"))).contains("SIM001"));
    assert!(error_of(compile(board("3parsecs", "value = X;"))).contains("SIM001: unknown unit 'parsecs'"));
    let duplicate = "board P {\n param X = 1V;\n param X = 2V;\n}";
    assert!(error_of(compile(duplicate.to_owned())).contains("SIM002: duplicate param 'X'"));
    // A name that is no param stays text, as before.
    let text = compile(board("1V", "value = DNP;")).unwrap();
    assert_eq!(format!("{:?}", text.electrical.components[0].value), "Some(Text(\"DNP\"))");
    // Benches keep a board param's dimension, step it with its dimension, and type its uses.
    assert!(
        rc_error(" param R_VALUE = 10pF;")
            .contains("SIM003: param 'R_VALUE' is a Resistance on the board; the bench cannot make it a Capacitance")
    );
    assert!(
        rc_error(" step R_VALUE in 1kohm, 2pF;")
            .contains("SIM018: step R_VALUE must be a Resistance, found 2 pF (Capacitance)")
    );
    assert!(rc_error(" step R_VALU in 1kohm;").contains("SIM041"));
    assert!(
        rc_error(" step R_VALUE in 1kohm;\n step R_VALUE in 2kohm;")
            .contains("SIM041: param 'R_VALUE' is stepped twice")
    );
    let override_ = rc_bench(" param R_VALUE = 2kohm;").unwrap();
    let r1 = override_.components.iter().find(|component| component.reference == "R1").unwrap();
    let Treatment::Passive { value, .. } = &r1.treatment else { panic!() };
    assert_eq!((value.value.to_string(), value.param.as_deref()), ("2 kohm".into(), Some("R_VALUE")));
    let failure = error_of(compile_bench_source(
        "bench B for RC {\n param F = 1kohm;\n replace INPUT with source { out = VIN; ref = GND; ac = F; }\n analysis op;\n}",
        "b.copper",
        &rc(),
    ));
    assert!(failure.contains("SIM003: sim source ac must be a Voltage, but param 'F' is a Resistance"), "{failure}");
}

const OPAMP_PINS: &str = "    category = \"amplifier.opamp\";
    pin INP { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INN { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUT { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"5\"; domains = \"power\"; directions = \"input\"; }
";

fn opamp_part(name: &str, sim: &str) -> Result<Design, copper_lang::CopperScriptError> {
    let part = format!("part Amp {{\n{OPAMP_PINS}{sim}\n}}\n");
    let board = "board A {\n    import p \"./parts\";\n    component U1: p.Amp;\n}\n";
    compile_design_file(&project(name, &[&part], board, &[]), false, true)
}

#[test]
fn sim_blocks_check_their_pins() {
    let roles = "inp = INP; inn = INN; out = OUT; vp = VP; vn = VN;";
    assert!(opamp_part("ok", &format!("sim opamp {{ {roles} gbw = 1MHz; }}")).is_ok());
    let missing = error_of(opamp_part("missing", "sim opamp { inp = INPP; inn = INN; out = OUT; vp = VP; vn = VN; }"));
    assert!(
        missing.contains("SIM014: sim opamp role inp names 'INPP', which is not a pin of part 'p.Amp'"),
        "{missing}"
    );
    assert!(missing.contains("did you mean 'INP'?"), "{missing}");
    let role = error_of(opamp_part("role", "sim opamp { inp = INP; inn = INN; out = OUT; vp = VP; }"));
    assert!(role.contains("SIM013: sim opamp needs the 'vn' role"), "{role}");
    let twice = error_of(opamp_part("twice", "sim opamp { inp = INP; inn = INP; out = OUT; vp = VP; vn = VN; }"));
    assert!(twice.contains("SIM015"), "{twice}");
    let uncovered = error_of(opamp_part("uncovered2", "sim load { inp = INP; ref = INN; resistance = 1Mohm; }"));
    assert!(uncovered.contains("SIM016: pin 'OUT' of part 'p.Amp' has no sim role"), "{uncovered}");
    assert!(
        opamp_part("unmodelled", "sim load { inp = INP; ref = INN; resistance = 1Mohm; unmodelled = OUT, VP, VN; }")
            .is_ok()
    );
    let unmodelled = error_of(opamp_part("unmodelled-bad", &format!("sim opamp {{ {roles} unmodelled = NC; }}")));
    assert!(unmodelled.contains("SIM014: unmodelled names 'NC'"), "{unmodelled}");
    let property = error_of(opamp_part("property", &format!("sim opamp {{ {roles} gwb = 1MHz; }}")));
    assert!(
        property.contains("SIM012: unknown sim opamp property 'gwb'") && property.contains("did you mean 'gbw'?"),
        "{property}"
    );
    let typed = error_of(opamp_part("typed", &format!("sim opamp {{ {roles} gbw = 65dB; }}")));
    assert!(typed.contains("SIM018: sim opamp gbw must be a Frequency, found 65 dB (Gain)"), "{typed}");
    assert!(error_of(opamp_part("negative", &format!("sim opamp {{ {roles} gbw = -1MHz; }}"))).contains("SIM019"));
    let spice = error_of(opamp_part(
        "spice",
        "sim spice { source = \"https://example.com/amp.cir\"; subckt = \"AMP\"; terminals = INP, INN, VP, VN, OUT; }",
    ));
    assert!(spice.contains("SIM076: sim spice source needs its sha256"), "{spice}");
    assert!(error_of(opamp_part("kind", &format!("sim opamp2 {{ {roles} }}"))).contains("did you mean 'opamp'?"));
    // Multi-unit parts name each block's unit; units share supply pins in the same role.
    let dual = "part Dual {\n    category = \"amplifier.opamp\";
    pin INA_P { number = \"1\"; domains = \"analog\"; directions = \"input\"; }
    pin INA_N { number = \"2\"; domains = \"analog\"; directions = \"input\"; }
    pin OUTA { number = \"3\"; domains = \"analog\"; directions = \"output\"; }
    pin INB_P { number = \"5\"; domains = \"analog\"; directions = \"input\"; }
    pin INB_N { number = \"6\"; domains = \"analog\"; directions = \"input\"; }
    pin OUTB { number = \"7\"; domains = \"analog\"; directions = \"output\"; }
    pin VP { number = \"8\"; domains = \"power\"; directions = \"input\"; }
    pin VN { number = \"4\"; domains = \"power\"; directions = \"input\"; }
    sim opamp A { inp = INA_P; inn = INA_N; out = OUTA; vp = VP; vn = VN; gbw = 10MHz; }
    sim opamp B { inp = INB_P; inn = INB_N; out = OUTB; vp = VP; vn = VN; gbw = 10MHz; }\n}\n";
    let board = "board A {\n    import p \"./parts\";\n    component U1: p.Dual;\n}\n";
    let design = compile_design_file(&project("dual", &[dual], board, &[]), false, true).unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Dual")).unwrap();
    let units: Vec<Option<&str>> = part.sim.iter().map(|model| model.unit.as_deref()).collect();
    assert_eq!(units, [Some("A"), Some("B")]);
    let unnamed = dual.replace("sim opamp B {", "sim opamp {");
    let failure = error_of(compile_design_file(&project("dual-unnamed", &[&unnamed], board, &[]), false, true));
    assert!(failure.contains("SIM017"), "{failure}");
    let crossed = dual.replace("vp = VP; vn = VN; gbw = 10MHz; }\n}", "vp = VN; vn = VP; gbw = 10MHz; }\n}");
    let failure = error_of(compile_design_file(&project("dual-crossed", &[&crossed], board, &[]), false, true));
    assert!(failure.contains("SIM015: pin 'VN' is role vn of another sim block and cannot also be vp"), "{failure}");
}

#[test]
fn bench_file_api() {
    let board = example("howland_pump/board.copper");
    let design = compile_example("howland_pump/board.copper");
    let sources = bench::bench_sources(&board);
    // Five benches and the LoadedPump fixture they share.
    assert_eq!(sources.len(), 6);
    // flatness.copper uses the fixture; compile_bench_file finds it beside the bench.
    let bench = compile_bench_file(&sources[0], &design).unwrap();
    assert_eq!(bench.board, "HowlandPump");
    assert_eq!(bench.fixtures, ["LoadedPump"]);
    assert!(bench.components.iter().any(|component| component.reference == "R_LOAD" && component.bench));
}

#[test]
fn diffamp_kind_and_new_opamp_parameters() {
    let roles = "inp = INP; inn = INN; out = OUT; vp = VP; vn = VN;";
    let design = opamp_part(
        "cmrr",
        &format!("sim opamp {{ {roles} gbw = 1MHz; second_pole = 5MHz; cmrr = 90dB; cmrr_corner = 300kHz; }}"),
    )
    .unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Amp")).unwrap();
    let keys: Vec<&str> = part.sim[0].spec.parameters.keys().map(String::as_str).collect();
    assert_eq!(keys, ["gbw", "second_pole", "cmrr", "cmrr_corner"]);
    let typed = error_of(opamp_part("cmrr-typed", &format!("sim opamp {{ {roles} cmrr = 90kHz; }}")));
    assert!(typed.contains("SIM018: sim opamp cmrr must be a Gain"), "{typed}");
    assert!(
        error_of(opamp_part("pole-zero", &format!("sim opamp {{ {roles} second_pole = 0Hz; }}"))).contains("SIM019")
    );
    // A differential amplifier has the feedback pair as two more roles.
    let receiver = format!(
        "part Rx {{\n{OPAMP_PINS}    pin REF {{ number = \"6\"; domains = \"analog\"; directions = \"input\"; }}
    pin FB {{ number = \"7\"; domains = \"analog\"; directions = \"input\"; }}
    sim diffamp {{ {roles} ref = REF; fb = FB; gbw = 270MHz; open_loop_gain = 74dB; cmrr = 90dB; }}\n}}\n"
    );
    let board = "board A {\n    import p \"./parts\";\n    component U1: p.Rx;\n}\n";
    let design = compile_design_file(&project("diffamp", &[&receiver], board, &[]), false, true).unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Rx")).unwrap();
    assert_eq!(part.sim[0].spec.kind, SimKind::Diffamp);
    let roles: Vec<&str> = part.sim[0].pins.keys().map(String::as_str).collect();
    assert_eq!(roles, ["inp", "inn", "ref", "fb", "out", "vp", "vn"]);
    let missing = receiver.replace(" fb = FB;", "");
    let failure = error_of(compile_design_file(&project("diffamp-fb", &[&missing], board, &[]), false, true));
    assert!(failure.contains("SIM013: sim diffamp needs the 'fb' role"), "{failure}");
}

#[test]
fn load_blocks_share_pins_across_roles() {
    // A resistor network: each resistor and the capacitance of each of its ends to the
    // exposed pad are load blocks, so a pin is `inp` of one block and `ref` of another.
    let network = "part Net {\n    category = \"passive.network\";
    pin A { number = \"1\"; domains = \"analog\"; directions = \"passive\"; }
    pin B { number = \"2\"; domains = \"analog\"; directions = \"passive\"; }
    pin EP { number = \"3\"; domains = \"ground\"; directions = \"passive\"; }
    sim load R { inp = A; ref = B; resistance = 10kohm; }
    sim load CA { inp = A; ref = EP; capacitance = 2pF; }
    sim load CB { inp = B; ref = EP; capacitance = 2pF; }\n}\n";
    let board = "board A {\n    import p \"./parts\";\n    component N1: p.Net;\n}\n";
    let design = compile_design_file(&project("network", &[network], board, &[]), false, true).unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Net")).unwrap();
    assert_eq!(part.sim.len(), 3);
    // Other kinds still keep one role per pin.
    let mixed = network
        .replace("sim load CB { inp = B;", "sim source CB { out = B; ref = A; ac = 1V; } sim load CC { inp = B;");
    let failure = error_of(compile_design_file(&project("network-mixed", &[&mixed], board, &[]), false, true));
    assert!(failure.contains("SIM015"), "{failure}");
}

#[test]
fn derived_params_zipped_steps_and_references() {
    let bench = rc_bench(
        " param SCALE = 200%;\n param R_TWICE = R_VALUE * SCALE;\n param TAU = R_VALUE * 100nF;\n \
         component R2: RESISTOR { value = R_TWICE; }\n net VOUT { R2.1; }\n net GND { R2.2; }\n \
         step R_VALUE, SCALE in (1kohm, 100%), (2kohm, 300%);\n \
         reference SHORT { R_VALUE = 1ohm; }\n \
         let gain = V(VOUT) / V(VIN);\n let relative = gain / reference(gain, SHORT);\n \
         measure g = mag(at(relative, 1kHz)) { max = 1; }",
    )
    .unwrap();
    let derived = &bench.params["R_TWICE"];
    assert_eq!(derived.value.to_string(), "2 kohm");
    assert!(derived.derived.is_some());
    assert_eq!(bench.params["TAU"].value.dimension(), Dimension::Time);
    assert_eq!(bench.steps.len(), 1);
    assert_eq!(bench.steps[0].params(), ["R_VALUE", "SCALE"]);
    assert_eq!(bench.steps[0].entry(1)[1].1.to_string(), "300 %");
    assert_eq!(bench.references.len(), 1);
    assert_eq!(bench.references[0].overrides[0].0, "R_VALUE");
    let relative = bench.lets.iter().find(|item| item.name == "relative").unwrap();
    assert!(relative.uses_reference);
    assert!(!bench.lets.iter().find(|item| item.name == "gain").unwrap().uses_reference);
    let ExpressionKind::Binary { right, .. } = &relative.expression.kind else { panic!() };
    let ExpressionKind::Call { function: Function::Reference, arguments } = &right.kind else { panic!() };
    assert_eq!(arguments[1].kind, ExpressionKind::Reference("SHORT".into()));
    let json = bench_json(&bench);
    assert!(json.contains("\"derived\"") && json.contains("\"zip\"") && json.contains("\"overrides\""), "{json}");

    // Derived params: earlier params and arithmetic only, with a quantity's units.
    assert!(rc_error(" param X = Y * 2;").contains("SIM062: unknown name 'Y'"));
    assert!(rc_error(" param X = mag(R_VALUE);").contains("SIM001: derived param 'X' may use numbers"));
    assert!(rc_error(" param X = V(VOUT) * 2;").contains("SIM001: a derived param is computed from params"));
    assert!(rc_error(" param X = R_VALUE * R_VALUE;").contains("SIM001: derived param 'X' has units V^2/A^2"));
    assert!(rc_error(" param X = R_VALUE * 2;\n step X in 1kohm;").contains("SIM041: param 'X' is derived"));
    // Zipped steps: one value per param in every tuple, each param stepped once.
    assert!(
        rc_error(" param S = 1%;\n step R_VALUE, S in (1kohm, 1%), (2kohm);")
            .contains("SIM041: a zipped step of 2 params needs 2 values per tuple, found 1")
    );
    assert!(
        rc_error(" param S = 1%;\n step R_VALUE, S in (1kohm, 1%);\n step S in 2%;")
            .contains("SIM041: param 'S' is stepped twice")
    );
    // References: known params of the right dimension; reference() names a declared one.
    assert!(rc_error(" reference CARD { R_VALUE = 1pF; }").contains("SIM018"));
    assert!(
        rc_error(" reference CARD { NOPE = 1kohm; }")
            .contains("SIM047: reference 'CARD' sets 'NOPE', which is no param")
    );
    assert!(rc_error(" reference CARD { }").contains("SIM047"));
    assert!(rc_error(" let x = reference(V(VOUT), CARD);").contains("SIM047: unknown reference 'CARD'"));
    let nested = rc_error(
        " reference CARD { R_VALUE = 1ohm; }\n let x = reference(V(VOUT), CARD);\n let y = reference(x, CARD);",
    );
    assert!(nested.contains("SIM047: reference(x, NAME) reads x from a reference run"), "{nested}");
}

/// A module with a param and a port whose name is not its net's, in the board's `parts/` package.
const RC_MODULE: &str = "module RcStage {
    use library \"tiny\";
    port IN: input;
    port OUT: output;
    port GND: power_in;
    param R_VALUE = 1kohm;
    component R1: RESISTOR { value = R_VALUE; }
    component C1: CAPACITOR { value = 100nF; }
    net VIN { port.IN; R1.1; }
    net VOUT { port.OUT; R1.2; C1.1; }
    net GND { port.GND; C1.2; }
}";

/// A board that instantiates `RcStage` and has no supplies of its own for it.
const RC_TOP: &str = "board Top {
    use library \"tiny\";
    import p \"./parts\";
    module F: p.RcStage;
    net A { F.IN; }
    net B { F.OUT; }
    net GND { F.GND; }
}";

fn module_design(name: &str, benches: &[&str]) -> (PathBuf, Design) {
    let board = project(name, &[RC_MODULE], RC_TOP, benches);
    let design = compile_design_file(&board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    (board, design)
}

fn module_bench(body: &str) -> Result<Bench, copper_lang::CopperScriptError> {
    let (_, design) = module_design("module-bench", &[]);
    compile_bench_source(&format!("bench B for RcStage {{\n{body}\n}}"), "b.copper", &design)
}

const MODULE_PORTS: &str = " supply GROUND { voltage = 0V; net = GND; }
 model SRC: source { out = IN; ref = GND; ac = 1V; output_resistance = 50ohm; }
 analysis ac { from = 10Hz; to = 100kHz; points_per_decade = 10; }";

#[test]
fn module_params_compile_into_the_module_definition() {
    let (_, design) = module_design("module-params", &[]);
    let module = &design.electrical.module_definitions["p.RcStage"];
    assert_eq!(module.params["R_VALUE"].value.to_string(), "1 kohm");
    assert_eq!(module.params["R_VALUE"].uses[0].component, "R1");
    let json = design_json(&design);
    assert!(json.contains("\"R_VALUE\""), "{json}");
    assert!(design.electrical.params.is_empty());
}

#[test]
fn a_bench_simulates_a_module_on_its_own() {
    let (_, design) = module_design("module-own", &[]);
    let source = format!(
        "bench B for RcStage {{\n{MODULE_PORTS}\n param R_VALUE = 2kohm;\n measure gain = mag(at(V(OUT), 796Hz));\n}}"
    );
    let bench = compile_bench_source(&source, "b.copper", &design).unwrap();
    assert_eq!((bench.board.as_str(), bench.module.as_deref()), ("Top", Some("p.RcStage")));
    assert_eq!(bench.ground, "GND");
    // The module's own components under their local names; nothing of the board.
    let references: Vec<&str> = bench.components.iter().map(|component| component.reference.as_str()).collect();
    assert_eq!(references, ["R1", "C1"]);
    let Treatment::Passive { value, .. } = &bench.components[0].treatment else { panic!("R1 is a passive") };
    assert_eq!((value.value.to_string(), value.param.as_deref()), ("2 kohm".into(), Some("R_VALUE")));
    assert_eq!(bench.params["R_VALUE"].origin, bench::ParamOrigin::BenchOverride);
    // The bench supplies the ports.
    let supplies: Vec<(&str, &str, &SupplyTreatment, bool)> = bench
        .supplies
        .iter()
        .map(|supply| (supply.name.as_str(), supply.net.as_str(), &supply.treatment, supply.bench))
        .collect();
    assert_eq!(supplies, [("GROUND", "GND", &SupplyTreatment::Ground, true)]);
    let model = &bench.models[0];
    assert_eq!((model.name.as_str(), model.spec.kind), ("SRC", SimKind::Source));
    // Port IN is an alias of its net VIN.
    assert_eq!(model.nodes["out"], Node::Net("VIN".into()));
    assert!(bench.components.iter().all(|component| bench.part(&design, component).is_some()));
    let json = bench_json(&bench);
    assert!(json.contains("\"module\": \"p.RcStage\"") && json.contains("\"models\""), "{json}");
}

#[test]
fn module_benches_are_found_beside_the_board() {
    let source = format!("bench Corner for RcStage {{\n{MODULE_PORTS}\n measure gain = mag(at(V(OUT), 1592Hz));\n}}");
    let (board, design) = module_design("module-discovery", &[&source]);
    let benches = load_benches(&board, &design).unwrap();
    assert_eq!(benches.len(), 1);
    assert_eq!(benches[0].module.as_deref(), Some("p.RcStage"));
    // A target that is neither the board nor one of its modules.
    let failure = error_of(compile_bench_source("bench B for RcStge {\n analysis op;\n}", "b.copper", &design));
    assert!(
        failure.contains(
            "SIM030: bench 'B' is for 'RcStge', but the board is 'Top' and has no module of that name (its modules: p.RcStage)"
        ),
        "{failure}"
    );
    assert!(failure.contains("did you mean 'RcStage'?"), "{failure}");
    let (board, design) = module_design("module-lost", &["bench Lost for Nothing {\n analysis op;\n}"]);
    let failure = error_of(load_benches(&board, &design));
    assert!(failure.contains("which no source in") && failure.contains("is no module of board 'Top'"), "{failure}");
}

#[test]
fn module_bench_supplies_and_models_are_checked() {
    let analysis = " analysis op;";
    let ground = " supply GROUND { voltage = 0V; net = GND; }";
    let failure = error_of(module_bench(analysis));
    assert!(failure.contains("SIM046: a bench for a module needs a 0 V supply"), "{failure}");
    let failure = error_of(module_bench(&format!("{ground}\n supply S {{ voltage = 1V; net = VOUTT; }}\n{analysis}")));
    assert!(failure.contains("SIM048: bench supply 'S' is on 'VOUTT', which is no module net"), "{failure}");
    assert!(failure.contains("did you mean 'VOUT'?"), "{failure}");
    let failure = error_of(module_bench(&format!(
        "{ground}\n supply S {{ voltage = 1V; net = VIN; external = true; }}\n{analysis}"
    )));
    assert!(
        failure
            .contains("SIM048: a bench supply is an ideal DC source on a net; it takes voltage and net, not external"),
        "{failure}"
    );
    let failure = error_of(module_bench(&format!("{ground}\n supply S {{ voltage = 1V; net = GND; }}\n{analysis}")));
    assert!(failure.contains("SIM048: bench supply 'S' is on net 'GND', which supply 'GROUND' drives"), "{failure}");
    let failure = error_of(module_bench(&format!("{ground}\n supply R1 {{ voltage = 1V; net = VIN; }}\n{analysis}")));
    assert!(failure.contains("SIM048: bench supply 'R1' has the name of a supply or component"), "{failure}");
    let failure =
        error_of(module_bench(&format!("{ground}\n supply S {{ voltage = 1V; net = IN; }}\n ignore S;\n{analysis}")));
    assert!(
        failure.contains("SIM038: 'S' is a bench supply; change it in the bench instead of ignore it"),
        "{failure}"
    );
    let failure = error_of(module_bench(&format!(
        "{ground}\n model C1: source {{ out = VIN; ref = GND; dc = 1V; }}\n{analysis}"
    )));
    assert!(failure.contains("SIM049: bench model 'C1' has the name of a component, supply or model"), "{failure}");
    let failure = error_of(module_bench(&format!(
        "{ground}\n model M: source {{ out = NOWHERE; ref = GND; dc = 1V; }}\n{analysis}"
    )));
    assert!(failure.contains("SIM039: unknown node 'NOWHERE'"), "{failure}");
    // A port name is a node, in supplies and probes.
    let bench =
        module_bench(&format!("{ground}\n supply S {{ voltage = 1V; net = IN; }}\n{analysis}\n measure v = V(OUT);"));
    assert_eq!(bench.unwrap().supplies[1].net, "VIN");
}

#[test]
fn bench_models_and_supplies_parse() {
    let document = parse(
        "bench B for RC {\n supply S { voltage = 1V; net = VIN; }\n model M: load { inp = VOUT; ref = GND; resistance = 1kohm; }\n analysis op;\n}",
        "b.copper",
    )
    .unwrap();
    assert!(matches!(&document.declarations[1], Declaration::Supply(block) if block.name == "S"));
    let Declaration::Bench(BenchStatement::Model(model)) = &document.declarations[2] else { panic!("a model") };
    assert_eq!((model.name.as_str(), model.kind.as_str(), model.entries.len()), ("M", "load", 3));
    // In a board bench, a model joins the board's circuit.
    let bench = rc_bench(" model M: load { inp = VOUT; ref = GND; resistance = 1kohm; }").unwrap();
    assert_eq!(bench.models[0].nodes["inp"], Node::Net("VOUT".into()));
    assert!(bench.module.is_none());
}

#[test]
fn scoped_limits_and_comparisons_are_checked() {
    // A where of params scopes the limits; it is kept on the measure.
    let bench = rc_bench(
        " step R_VALUE in 1kohm, 2kohm;\n measure corner = upto(db(V(VOUT) / 1V) > -3dB) { min = 700Hz; where R_VALUE <= 1.5kohm and R_VALUE != 3kohm; }",
    )
    .unwrap();
    assert_eq!(bench.measures[0].conditions.len(), 2);
    for (body, words) in [
        (" measure m = max(mag(V(VOUT))) { where R_VALUE < 1kohm; }", "no min or max"),
        (" measure m = max(mag(V(VOUT))) { max = 1V; where R_VALUE < 1V; }", "cannot compare"),
        (" measure m = max(mag(V(VOUT))) { max = 1V; where max(mag(V(VOUT))) < 1V; }", "cannot probe"),
        (" measure m = max(mag(V(VOUT))) { max = 1V; where R_VALUE < 1kohm; where R_VALUE > 0ohm; }", "one where"),
        (" measure m = max(mag(V(VOUT)) < 1V);", "comparison"),
        (" measure m = upto(mag(V(VOUT)));", "takes a comparison"),
        (" measure m = upto(mag(V(VOUT)) == 1V);", "equality"),
        (" measure m = upto(V(VOUT) > 1V);", "real values"),
        (" measure m = upto(R_VALUE > 1kohm);", "varies over the axis"),
    ] {
        let failure = rc_error(body);
        assert!(failure.contains("SIM072") && failure.contains(words), "{body}: {failure}");
    }
    // Comparisons parse in call arguments and limits only; elsewhere `<` ends the expression.
    let failure = rc_error(" let x = mag(V(VOUT)) < 1V;");
    assert!(failure.contains("PAR"), "{failure}");
    // upto needs a swept analysis.
    let source = "bench B for RC { analysis op; measure m = upto(V(VOUT) > 1V); }";
    let failure = error_of(compile_bench_source(source, "b.copper", &rc()));
    assert!(failure.contains("SIM072") && failure.contains("swept analysis"), "{failure}");
}

#[test]
fn dc_sweep_analysis_is_checked() {
    let dc = |analysis: &str, extra: &str| {
        let source = format!(
            "bench B for RC {{\n replace INPUT with source {{ out = VIN; ref = GND; dc = 1V; }}\n {analysis}\n{extra}\n measure v = max(V(VOUT));\n}}"
        );
        compile_bench_source(&source, "b.copper", &rc())
    };
    let bench = dc("analysis dc { sweep = INPUT; from = 0V; to = 2V; step = 0.1V; }", "").unwrap();
    let copper_lang::bench::Analysis::Dc(analysis) = &bench.analysis else { panic!("a dc analysis") };
    assert_eq!(analysis.sweep, copper_lang::bench::DcSweep::ReplacedSupply("INPUT".to_owned()));
    let bench = dc("analysis dc { sweep = R_VALUE; from = 1kohm; to = 10kohm; points = 10; }", "").unwrap();
    assert!(
        matches!(&bench.analysis, copper_lang::bench::Analysis::Dc(dc) if dc.dimension == copper_units::Dimension::Resistance)
    );
    for (analysis, words) in [
        ("analysis dc { from = 0V; to = 2V; step = 0.1V; }", "needs what it sweeps"),
        ("analysis dc { sweep = R1; from = 0V; to = 2V; step = 0.1V; }", "no param, source model"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2V; }", "step (the spacing) or points"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2V; step = 0.1V; points = 3; }", "not both"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2V; step = -0.1V; }", "positive"),
        ("analysis dc { sweep = INPUT; from = 1V; to = 1V; step = 0.1V; }", "differ"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2kohm; step = 0.1V; }", "Voltage"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2V; points = 1; }", "from 2 to 100000"),
        ("analysis dc { sweep = INPUT; from = 0V; to = 2V; step = 0.1V; stride = 2; }", "unknown analysis dc property"),
    ] {
        let failure = error_of(dc(analysis, ""));
        assert!(
            (failure.contains("SIM074") || failure.contains("SIM018")) && failure.contains(words),
            "{analysis}: {failure}"
        );
    }
    // Probes are real over the sweep; a measure reduces them.
    let failure =
        error_of(dc("analysis dc { sweep = INPUT; from = 0V; to = 2V; step = 0.1V; }", " measure w = V(VOUT);"));
    assert!(failure.contains("SIM043") && failure.contains("dc sweep"), "{failure}");
    let failure = error_of(dc(
        "analysis dc { sweep = INPUT; from = 0V; to = 2V; step = 0.1V; }",
        " measure w = mag(at(V(VOUT), 1kHz));",
    ));
    assert!(failure.contains("SIM068") && failure.contains("dc sweep"), "{failure}");
}

#[test]
fn fixtures_and_initial_conditions_parse() {
    let fixture = copper_lang::parse("fixture Common for RC { param R_X = 1kohm; ignore C1; }", "f.copper").unwrap();
    assert_eq!(fixture.kind, copper_lang::syntax::DocumentKind::Fixture);
    let bench = copper_lang::parse(
        "bench B for RC using Common, Other { ic V(VOUT) = 1V; ic C1 = 0.5V; analysis tran { stop = 1ms; uic = false; } }",
        "b.copper",
    )
    .unwrap();
    let Some(copper_lang::syntax::Declaration::Bench(copper_lang::syntax::BenchStatement::Target(target))) =
        bench.declarations.first()
    else {
        panic!("a target first")
    };
    assert_eq!(target.fixtures, ["Common", "Other"]);
    // A fixture may use other fixtures.
    let fixture = copper_lang::parse("fixture F for RC using G { }", "f.copper").unwrap();
    let Some(copper_lang::syntax::Declaration::Bench(copper_lang::syntax::BenchStatement::Target(target))) =
        fixture.declarations.first()
    else {
        panic!("a target first")
    };
    assert_eq!(target.fixtures, ["G"]);
    let failure = error_of(copper_lang::parse("bench B for RC { ic V(A, B) = 1V; }", "b.copper"));
    assert!(failure.contains("SIM075"), "{failure}");
}

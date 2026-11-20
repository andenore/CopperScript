//! The rail models `sim regulator` and `sim reference` in the frontend: part
//! blocks with the optional `en` role, the checks that need several entries
//! (`SIM103`), ranges, bench models and the terminals `I(...)` reads.
//! `copper-simulate/tests/regulator_runs.rs` runs them.

use std::fs;
use std::path::{Path, PathBuf};

use copper_lang::bench::{compile_bench_source, load_benches};
use copper_lang::compiler::{compile_design_file, compile_design_source};
use copper_lang::simulation::SimKind;

fn project(name: &str, parts: &[&str], board: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("copper-regulators-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(directory.join("parts")).unwrap();
    for (index, source) in parts.iter().enumerate() {
        fs::write(directory.join("parts").join(format!("p{index}.copper")), source).unwrap();
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

const LDO_PINS: &str = "    category = \"power.regulator.ldo\";
    pin IN { number = \"1\"; domains = \"power\"; directions = \"input\"; }
    pin OUT { number = \"2\"; domains = \"power\"; directions = \"output\"; }
    pin GND { number = \"3\"; domains = \"ground\"; directions = \"input\"; }
    pin EN { number = \"4\"; domains = \"digital\"; directions = \"input\"; }
";

fn ldo_part(name: &str, sim: &str) -> Result<copper_lang::compiler::Design, copper_lang::CopperScriptError> {
    let part = format!("part Ldo {{\n{LDO_PINS}{sim}\n}}\n");
    let board = "board A {\n    import p \"./parts\";\n    component U1: p.Ldo;\n}\n";
    compile_design_file(&project(name, &[&part], board), false, true)
}

#[test]
fn regulator_part_blocks_and_their_checks() {
    let roles = "in = IN; out = OUT; gnd = GND;";
    let design = ldo_part(
        "ok",
        &format!(
            "sim regulator {{ {roles} en = EN; output_voltage = 4.5V; dropout = 260mV; output_resistance = 20mohm;
                output_impedance_corner = 30kHz; psrr = 70dB; psrr_corner = 10kHz; output_current_limit = 200mA;
                soft_start = 14ms; enable_threshold = 2.1V; quiescent_current = 49uA; voltage_noise = 50nV/rtHz; }}"
        ),
    )
    .unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Ldo")).unwrap();
    let block = &part.sim[0];
    assert_eq!(block.spec.kind, SimKind::Regulator);
    let assigned: Vec<(&str, &str)> = block.pins.iter().map(|(role, pin)| (role.as_str(), pin.as_str())).collect();
    assert_eq!(assigned, [("in", "IN"), ("out", "OUT"), ("gnd", "GND"), ("en", "EN")]);
    assert_eq!(block.spec.parameters["soft_start"].value.to_string(), "14 ms");
    // Without en, the pin is unmodelled; a negative set point and an efficiency are accepted.
    let converter = format!(
        "sim regulator {{ {roles} unmodelled = EN; output_voltage = -5.24V; efficiency = 85%; quiescent_current = 1mA; }}"
    );
    let design = ldo_part("converter", &converter).unwrap();
    let part = design.electrical.library.values().find(|part| part.name.ends_with("Ldo")).unwrap();
    assert_eq!(part.sim[0].spec.parameters["efficiency"].value.base_value().to_string(), "0.85");
    assert!(!part.sim[0].pins.contains_key("en"));
    // SIM103: the checks across entries.
    for (case, body, wanted) in [
        ("no-set-point", "dropout = 0.2V;", "SIM103: sim regulator needs an output_voltage"),
        ("zero", "output_voltage = 0V;", "SIM103: sim regulator output_voltage must not be zero"),
        (
            "corner",
            "output_voltage = 3.3V; output_impedance_corner = 1kHz;",
            "SIM103: sim regulator output_impedance_corner needs an output_resistance above 0 ohm",
        ),
        (
            "limit",
            "output_voltage = 3.3V; output_current_limit = 0.1A;",
            "SIM103: sim regulator output_current_limit needs an output_resistance above 0 ohm",
        ),
        ("psrr", "output_voltage = 3.3V; psrr_corner = 1kHz;", "SIM103: sim regulator psrr_corner needs a psrr"),
        (
            "threshold",
            "output_voltage = 3.3V; enable_threshold = 1V;",
            "SIM103: sim regulator enable_threshold needs the en role",
        ),
        ("efficiency", "output_voltage = 3.3V; efficiency = 120%;", "SIM103: sim regulator efficiency must be above 0"),
    ] {
        let failure = error_of(ldo_part(case, &format!("sim regulator {{ {roles} unmodelled = EN; {body} }}")));
        assert!(failure.contains(wanted), "{case}: {failure}");
    }
    let failure = error_of(ldo_part("en", &format!("sim regulator {{ {roles} en = EN; output_voltage = 3.3V; }}")));
    assert!(failure.contains("SIM103: sim regulator en needs an enable_threshold"), "{failure}");
    // Ranges (SIM019) and kinds.
    let failure = error_of(ldo_part(
        "dropout",
        &format!("sim regulator {{ {roles} unmodelled = EN; output_voltage = 3.3V; dropout = -1V; }}"),
    ));
    assert!(failure.contains("SIM019: sim regulator dropout must be zero or positive"), "{failure}");
    let failure = error_of(ldo_part(
        "start",
        &format!("sim regulator {{ {roles} unmodelled = EN; output_voltage = 3.3V; soft_start = 0s; }}"),
    ));
    assert!(failure.contains("SIM019: sim regulator soft_start must be positive"), "{failure}");
    let failure = error_of(ldo_part(
        "reference-efficiency",
        &format!("sim reference {{ {roles} unmodelled = EN; output_voltage = 2.5V; efficiency = 90%; }}"),
    ));
    assert!(failure.contains("SIM012: unknown sim reference property 'efficiency'"), "{failure}");
    let failure = error_of(ldo_part(
        "missing-role",
        "sim regulator { in = IN; out = OUT; unmodelled = EN, GND; output_voltage = 3.3V; }",
    ));
    assert!(failure.contains("SIM013: sim regulator needs the 'gnd' role"), "{failure}");
}

#[test]
fn the_analyzer_reference_has_two_units() {
    let board = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation/impedance_analyzer/board.copper");
    let design = compile_design_file(&board, false, true).unwrap_or_else(|failure| panic!("{failure}"));
    let part = design.electrical.library.values().find(|part| part.name.ends_with("REF2033AIDDCR")).unwrap();
    let units: Vec<(Option<&str>, SimKind, &str)> =
        part.sim.iter().map(|block| (block.unit.as_deref(), block.spec.kind, block.pins["out"].as_str())).collect();
    assert_eq!(units, [(Some("VREF"), SimKind::Reference, "VREF"), (Some("VBIAS"), SimKind::Reference, "VBIAS")]);
    assert_eq!(part.sim[1].spec.parameters["dropout"].value.to_string(), "1.67 V");
}

const RAIL_BOARD: &str = "board R {
    use library \"tiny\";
    component R1: RESISTOR { value = 100ohm; }
    component R2: RESISTOR { value = 1Mohm; }
    net VIN { R2.1; }
    net VOUT { R1.1; }
    net GND { R1.2; R2.2; }
    supply INPUT { voltage = 5V; net = VIN; external = true; }
    supply GROUND { voltage = 0V; net = GND; external = true; }
}";

#[test]
fn bench_rail_models_and_their_terminals() {
    let design = compile_design_source(RAIL_BOARD, "r.copper", false, true).unwrap();
    let bench = |body: &str| {
        let source = format!(
            "bench B for R {{\n model REG: regulator {{ in = VIN; out = VOUT; gnd = GND; output_voltage = 3.3V; dropout = 0.2V; }}\n analysis op;\n{body}\n}}"
        );
        compile_bench_source(&source, "b.copper", &design)
    };
    let ok = bench("measure i_in = I(REG.in); measure i_out = I(REG.out); measure i_gnd = I(REG.gnd);").unwrap();
    assert_eq!(ok.models[0].spec.kind, SimKind::Regulator);
    let failure = error_of(bench("measure i = I(REG);"));
    assert!(failure.contains("SIM045") && failure.contains("which has 3 terminals"), "{failure}");
    let enabled = "bench B for R {
 model EN_SRC: source { out = VIN; ref = GND; dc = 5V; }
 model REF: reference { in = VIN; out = VOUT; gnd = GND; en = VIN; output_voltage = 2.5V; enable_threshold = 2V; startup_time = 1ms; }
 analysis op;
 measure i = I(REF.en);
}";
    let failure = error_of(compile_bench_source(enabled, "b.copper", &design));
    assert!(
        failure.contains("SIM045: the en terminal of the reference model of 'REF' carries no current"),
        "{failure}"
    );
    // A soft_start belongs to a regulator; a reference has a startup_time.
    let failure = error_of(bench(
        "model X: reference { in = VIN; out = VOUT; gnd = GND; output_voltage = 1V; soft_start = 1ms; }",
    ));
    assert!(failure.contains("SIM012: unknown sim reference property 'soft_start'"), "{failure}");
    // The analyzer's Startup bench compiles with its rail models and the REF2033 part.
    let board = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/simulation/impedance_analyzer/board.copper");
    let design = compile_design_file(&board, false, true).unwrap();
    let benches = load_benches(&board, &design).unwrap();
    let startup = benches.iter().find(|bench| bench.name == "Startup").unwrap();
    let kinds: Vec<(&str, SimKind)> = startup
        .models
        .iter()
        .filter(|model| model.spec.kind == SimKind::Regulator)
        .map(|m| (m.name.as_str(), m.spec.kind))
        .collect();
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(startup.components.iter().any(|component| component.reference == "U_REF"));
}

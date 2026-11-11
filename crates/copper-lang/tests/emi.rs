//! EMI roles and switching metadata: attribute parsing, the IR, the derived
//! switching-cell model and the source-level EMI warnings.

use std::fs;
use std::path::PathBuf;

use copper_lang::compiler::{Design, compile_design_file};
use copper_lang::elaborate::elaborate;
use copper_lang::emi::{self, AggressorClass, FrequencySource, Provenance};
use copper_lang::erc::{self, Diagnostic, Severity};
use copper_lang::model::{EmiRole, SwitchingFrequency, Topology};
use copper_lang::serializer::{design_json, part_value};

/// A project with parts (one per file) and a board.
fn project(name: &str, parts: &str, board_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-emi-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    // The importer reads one declaration per file.
    let mut files: Vec<String> = Vec::new();
    for line in parts.lines() {
        if line.starts_with("part ") || line.starts_with("device ") {
            files.push(String::new());
        }
        let Some(file) = files.last_mut() else { continue };
        file.push_str(line);
        file.push('\n');
    }
    for (index, source) in files.iter().enumerate() {
        fs::write(dir.join("parts").join(format!("p{index}.copper")), source).unwrap();
    }
    fs::write(dir.join("board.copper"), format!("board B {{\n    import p \"./parts\";\n{board_body}\n}}\n")).unwrap();
    dir.join("board.copper")
}

fn compile(name: &str, parts: &str, board_body: &str) -> Result<Design, String> {
    compile_design_file(&project(name, parts, board_body), false, false).map_err(|failure| failure.to_string())
}

fn design(name: &str, parts: &str, board_body: &str) -> Design {
    compile(name, parts, board_body).unwrap_or_else(|failure| panic!("compiles: {failure}"))
}

fn model(design: &Design) -> emi::EmiModel {
    emi::analyze(&elaborate(&design.electrical).expect("elaborates"))
}

fn emi_diagnostics(design: &Design) -> Vec<Diagnostic> {
    erc::check(&design.electrical, false)
        .expect("checks")
        .into_iter()
        .filter(|item| item.code.starts_with("EMI_"))
        .collect()
}

fn codes(found: &[Diagnostic]) -> Vec<&str> {
    found.iter().map(|item| item.code.as_str()).collect()
}

const PARTS: &str = r#"
part Buck {
    category = "power.regulator.buck";
    switching_frequency = 500kHz;
    pin VIN  { number = "1"; domains = "power"; directions = "input"; emi_role = hot_supply; }
    pin BOOT { number = "2"; domains = "power"; directions = "passive"; emi_role = bootstrap; }
    pin SW   { number = "3"; domains = "power"; directions = "output"; emi_role = switch; rise_time = 5ns; current_peak = 4.5A; }
    pin FB   { number = "4"; domains = "analog"; directions = "input"; emi_role = feedback; }
    pin GND  { number = "5"; domains = "ground"; directions = "input"; emi_role = hot_return; }
}
part Inductor {
    category = "passive.inductor";
    shielding = shielded;
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin B { number = "2"; domains = "power"; directions = "passive"; }
}
part Diode {
    category = "semiconductor.diode.schottky";
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin K { number = "2"; domains = "power"; directions = "passive"; }
}
part Cap {
    category = "passive.capacitor";
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin B { number = "2"; domains = "power"; directions = "passive"; }
}
part Res {
    category = "passive.resistor";
    pin A { number = "1"; domains = "analog"; directions = "passive"; }
    pin B { number = "2"; domains = "analog"; directions = "passive"; }
}
part Conn {
    category = "connector.header";
    external_cable = true;
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin B { number = "2"; domains = "power"; directions = "passive"; }
}
part Reg {
    category = "power.regulator.buck";
    pin VIN { number = "1"; domains = "power"; directions = "input"; }
    pin SW  { number = "2"; domains = "power"; directions = "output"; }
    pin GND { number = "3"; domains = "ground"; directions = "input"; }
}
"#;

/// An asynchronous buck with a declared input capacitor and a bootstrap capacitor.
const BUCK: &str = r#"
    component U1: p.Buck;
    component L1: p.Inductor { value = 4.7uH; }
    component D1: p.Diode;
    component CIN: p.Cap { value = 10uF; role = decoupling; decouples = U1.VIN; }
    component CBOOT: p.Cap { value = 100nF; }
    component COUT: p.Cap { value = 22uF; }
    component R1: p.Res { value = 100kohm; }
    component R2: p.Res { value = 33kohm; }
    supply VIN { voltage = 12V; external = true; }
    net VIN {
        U1.VIN;
        CIN.A;
    }
    net GND {
        U1.GND;
        CIN.B;
        D1.A;
        COUT.B;
        R2.B;
    }
    net SW {
        U1.SW;
        L1.A;
        D1.K;
        CBOOT.B;
    }
    net BOOT {
        U1.BOOT;
        CBOOT.A;
    }
    net VOUT {
        L1.B;
        COUT.A;
        R1.A;
    }
    net FB {
        U1.FB;
        R1.B;
        R2.A;
    }
"#;

/// `body` without the lines that mention any of `tokens`.
fn without(body: &str, tokens: &[&str]) -> String {
    body.lines().filter(|line| !tokens.iter().any(|token| line.contains(token))).collect::<Vec<_>>().join("\n")
}

#[test]
fn attributes_compile_and_serialize_only_when_declared() {
    let compiled = design("attributes", PARTS, BUCK);
    let parts: Vec<_> = compiled.electrical.library.values().collect();
    let buck = parts.iter().find(|part| part.name.ends_with("Buck")).expect("Buck");
    let sw = &buck.pins["SW"].profile.as_ref().expect("profile").emi.clone().expect("emi");
    assert_eq!(sw.role, Some(EmiRole::Switch));
    assert_eq!(sw.rise_time.as_ref().map(ToString::to_string).as_deref(), Some("5 ns"));
    assert_eq!(sw.fall().map(ToString::to_string).as_deref(), Some("5 ns"));
    assert_eq!(sw.current_peak.as_ref().map(ToString::to_string).as_deref(), Some("4.5 A"));
    let part_emi = buck.emi.as_ref().expect("part emi");
    assert_eq!(part_emi.switching_frequency, Some(SwitchingFrequency::Fixed(quantity_khz("500"))));

    let json = part_value(buck);
    let pins = json.get("pins").and_then(|pins| pins.as_array()).expect("pins");
    let profile_of = |name: &str| {
        let pin = pins.iter().find(|pin| pin.get("name").and_then(|v| v.as_str()) == Some(name)).expect("pin");
        pin.get("profile").expect("profile").clone()
    };
    let emi = profile_of("SW").get("emi").expect("emi object").clone();
    assert_eq!(emi.get("role").and_then(|v| v.as_str()), Some("switch"));
    assert_eq!(emi.get("rise_time").and_then(|v| v.get("unit")).and_then(|v| v.as_str()), Some("ns"));
    assert_eq!(emi.get("rise_time").and_then(|v| v.get("base_value")).and_then(|v| v.as_str()), Some("0.000000005"));
    assert!(emi.get("fall_time").is_none(), "undeclared keys are absent");
    let feedback = profile_of("FB").get("emi").expect("emi object").clone();
    assert_eq!(feedback.as_object().map(|object| object.len()), Some(1));
    assert_eq!(
        json.get("emi").and_then(|v| v.get("switching_frequency")).and_then(|v| v.get("unit")).and_then(|v| v.as_str()),
        Some("kHz")
    );
    let inductor = parts.iter().find(|part| part.name.ends_with("Inductor")).expect("Inductor");
    assert_eq!(
        part_value(inductor).get("emi").and_then(|v| v.get("shielding")).and_then(|v| v.as_str()),
        Some("shielded")
    );
}

fn quantity_khz(value: &str) -> copper_units::Quantity {
    copper_units::Quantity::of(copper_units::Dimension::Frequency, &copper_units::Decimal::parse(value).unwrap(), "kHz")
        .unwrap()
}

#[test]
fn unannotated_design_has_an_unchanged_ir_and_no_diagnostics() {
    let parts = r#"
part Chip {
    category = "power.regulator.buck";
    pin VIN { number = "1"; domains = "power"; directions = "input"; }
    pin SW  { number = "2"; domains = "power"; directions = "output"; }
}
"#;
    let compiled = design("plain", parts, "    component U1: p.Chip;\n    net X {\n        U1.SW;\n    }\n");
    let json = design_json(&compiled);
    assert!(!json.contains("\"emi\""), "{json}");
    assert!(!json.contains("switching_frequency"), "{json}");
    assert!(emi_diagnostics(&compiled).is_empty());
    let found = model(&compiled);
    assert!(found.cells.is_empty(), "{:?}", found.cells);
    assert!(found.nets.is_empty());
}

#[test]
fn declared_asynchronous_buck_gives_a_complete_cell_and_no_warnings() {
    let compiled = design("buck", PARTS, BUCK);
    let found = emi_diagnostics(&compiled);
    assert!(found.is_empty(), "{found:?}");
    let model = model(&compiled);
    assert_eq!(model.cells.len(), 1, "{:?}", model.cells);
    let cell = model.cell("U1").expect("U1 cell");
    assert_eq!(cell.provenance, Provenance::Declared);
    assert_eq!(cell.topology, Some(Topology::Buck));
    let frequency = cell.frequency.as_ref().expect("frequency");
    assert_eq!(frequency.source, FrequencySource::Part);
    assert_eq!(cell.fixed_frequency().map(ToString::to_string).as_deref(), Some("500 kHz"));
    assert_eq!(cell.rise_time.as_ref().map(ToString::to_string).as_deref(), Some("5 ns"));
    assert_eq!(cell.current_peak.as_ref().map(ToString::to_string).as_deref(), Some("4.5 A"));
    let endpoints = |pins: &[copper_lang::model::Endpoint]| pins.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(endpoints(&cell.switch_pins), ["U1.SW"]);
    assert_eq!(endpoints(&cell.hot_supply_pins), ["U1.VIN"]);
    assert_eq!(endpoints(&cell.hot_return_pins), ["U1.GND"]);
    assert_eq!(endpoints(&cell.bootstrap_pins), ["U1.BOOT"]);
    assert_eq!(cell.switch_nets, ["SW"]);
    assert_eq!(cell.hot_supply_net.as_deref(), Some("VIN"));
    assert_eq!(cell.hot_return_net.as_deref(), Some("GND"));
    let magnetic = cell.magnetic.as_ref().expect("magnetic");
    assert_eq!(magnetic.component, "L1");
    assert_eq!(magnetic.switch_pin.to_string(), "L1.A");
    assert_eq!(magnetic.output_pin.as_ref().map(ToString::to_string).as_deref(), Some("L1.B"));
    assert_eq!(magnetic.output_net.as_deref(), Some("VOUT"));
    let diode = cell.catch_diode.as_ref().expect("catch diode");
    assert_eq!(
        (diode.component.as_str(), diode.switch_pin.to_string(), diode.rail_pin.to_string()),
        ("D1", "D1.K".into(), "D1.A".into())
    );
    assert_eq!(cell.hot_capacitors.len(), 1);
    let capacitor = &cell.hot_capacitors[0];
    assert_eq!((capacitor.component.as_str(), capacitor.declared), ("CIN", true));
    assert_eq!((capacitor.supply_pin.to_string(), capacitor.return_pin.to_string()), ("CIN.A".into(), "CIN.B".into()));
    let bootstrap = cell.bootstrap_capacitor.as_ref().expect("bootstrap capacitor");
    assert_eq!(bootstrap.component, "CBOOT");
    assert_eq!(
        (bootstrap.bootstrap_pin.to_string(), bootstrap.switch_pin.to_string()),
        ("CBOOT.A".into(), "CBOOT.B".into())
    );

    let sw = &model.nets["SW"];
    assert_eq!((sw.aggressor, sw.provenance), (Some(AggressorClass::SwitchNode), Provenance::Declared));
    assert_eq!(model.nets["VIN"].aggressor, Some(AggressorClass::PulsedPower));
    assert_eq!(model.nets["GND"].aggressor, Some(AggressorClass::PulsedPower));
    assert_eq!(model.nets["BOOT"].aggressor, Some(AggressorClass::Bootstrap));
    assert!(model.nets["FB"].sensitive);
    assert_eq!(model.nets["FB"].sensitive_pins[0].to_string(), "U1.FB");
    assert!(!model.nets.contains_key("VOUT"));
}

#[test]
fn model_json_is_deterministic_and_versioned() {
    let compiled = design("json", PARTS, BUCK);
    let first = model(&compiled).to_json();
    assert_eq!(first, model(&compiled).to_json());
    assert!(first.contains("\"schema\": \"copperscript.emi-model/v0.1\""), "{first}");
    assert!(first.contains("\"controller\": \"U1\""));
    assert!(first.contains("\"output_net\": \"VOUT\""));
    assert!(first.ends_with("}\n"));
}

#[test]
fn component_frequency_overrides_the_part_and_a_range_warns_without_it() {
    let ranged = PARTS.replace(
        "switching_frequency = 500kHz;",
        "switching_frequency_min = 100kHz; switching_frequency_max = 2.5MHz;",
    );
    let compiled = design("range", &ranged, BUCK);
    let found = emi_diagnostics(&compiled);
    assert_eq!(codes(&found), ["EMI_SWITCHING_FREQUENCY_UNKNOWN"], "{found:?}");
    assert_eq!((found[0].severity, found[0].subject.as_deref()), (Severity::Warning, Some("U1")));
    let cell = model(&compiled).cells.remove(0);
    assert!(matches!(cell.frequency.as_ref().map(|f| &f.spec), Some(SwitchingFrequency::Range { .. })));

    let board = BUCK.replace("component U1: p.Buck;", "component U1: p.Buck { switching_frequency = 800kHz; }");
    let compiled = design("override", &ranged, &board);
    assert!(emi_diagnostics(&compiled).is_empty());
    let cell = model(&compiled).cells.remove(0);
    assert_eq!(cell.frequency.as_ref().map(|f| f.source), Some(FrequencySource::Component));
    assert_eq!(cell.fixed_frequency().map(ToString::to_string).as_deref(), Some("800 kHz"));
    assert_eq!(
        compiled.electrical.components[0].properties.get("switching_frequency").map(String::as_str),
        Some("800 kHz")
    );

    let unknown = PARTS.replace("    switching_frequency = 500kHz;\n", "");
    let compiled = design("unknown", &unknown, BUCK);
    assert_eq!(codes(&emi_diagnostics(&compiled)), ["EMI_SWITCHING_FREQUENCY_UNKNOWN"]);
}

fn expect_error(name: &str, parts: &str, board: &str, code: &str) {
    match compile(name, parts, board) {
        Ok(_) => panic!("{name}: expected {code}"),
        Err(failure) => assert!(failure.contains(code), "{name}: expected {code}, got {failure}"),
    }
}

#[test]
fn pin_attribute_errors_are_cmp130() {
    for (name, from, to) in [
        ("role", "emi_role = switch;", "emi_role = nope;"),
        ("role-number", "emi_role = switch;", "emi_role = 3;"),
        ("zero-rise", "rise_time = 5ns;", "rise_time = 0ns;"),
        ("voltage-rise", "rise_time = 5ns;", "rise_time = 5V;"),
        ("bare-rise", "rise_time = 5ns;", "rise_time = 5;"),
        ("length-fall", "rise_time = 5ns;", "fall_time = 5mm;"),
        ("negative-fall", "rise_time = 5ns;", "fall_time = -1ns;"),
        ("zero-peak", "current_peak = 4.5A;", "current_peak = 0A;"),
        ("time-peak", "current_peak = 4.5A;", "current_peak = 3ns;"),
    ] {
        expect_error(name, &PARTS.replace(from, to), BUCK, "CMP130");
    }
    let unknown = PARTS.replace("emi_role = switch;", "emi_rol = switch;");
    expect_error("typo", &unknown, BUCK, "CMP018");
}

#[test]
fn edge_data_without_an_aggressor_role_is_allowed() {
    let parts = r#"
part Osc {
    category = "clock.oscillator";
    pin OUT { number = "1"; domains = "clock"; directions = "output"; rise_time = 1ns; fall_time = 2ns; }
}
"#;
    let compiled = design("clock", parts, "    component X1: p.Osc;\n    net CLK {\n        X1.OUT;\n    }\n");
    let found = model(&compiled);
    assert_eq!(found.nets["CLK"].aggressor, Some(AggressorClass::FastEdge));
    assert!(found.cells.is_empty());
    let emi = compiled.electrical.library.values().next().unwrap().pins["OUT"].profile.as_ref().unwrap().emi.clone();
    let emi = emi.expect("emi");
    assert_eq!(emi.role, None);
    assert_eq!(emi.fall().map(ToString::to_string).as_deref(), Some("2 ns"));
}

#[test]
fn part_attribute_errors_are_cmp130() {
    for (name, attribute) in [
        ("freq-voltage", "switching_frequency = 5V;"),
        ("freq-zero", "switching_frequency = 0kHz;"),
        ("freq-bare", "switching_frequency = 500;"),
        ("both", "switching_frequency_min = 1kHz;"),
        ("topology", "topology = nope;"),
        ("shielding", "shielding = nope;"),
        ("esl-unit", "esl = 5nF;"),
        ("esr-negative", "esr = -1ohm;"),
        ("sfr", "self_resonance = 5uH;"),
        ("coss", "output_capacitance = 0pF;"),
        ("cable", "external_cable = 1;"),
    ] {
        let parts = PARTS.replace(
            "    switching_frequency = 500kHz;\n",
            &format!("    switching_frequency = 500kHz; {attribute}\n"),
        );
        let parts =
            if name.starts_with("freq") { PARTS.replace("switching_frequency = 500kHz;", attribute) } else { parts };
        expect_error(name, &parts, BUCK, "CMP130");
    }
    let inverted = PARTS
        .replace("switching_frequency = 500kHz;", "switching_frequency_min = 2MHz; switching_frequency_max = 100kHz;");
    expect_error("min-max", &inverted, BUCK, "CMP130");
}

#[test]
fn component_frequency_must_lie_in_the_part_range() {
    let ranged = PARTS.replace(
        "switching_frequency = 500kHz;",
        "switching_frequency_min = 100kHz; switching_frequency_max = 2.5MHz;",
    );
    let outside = BUCK.replace("component U1: p.Buck;", "component U1: p.Buck { switching_frequency = 5MHz; }");
    expect_error("outside", &ranged, &outside, "CMP130");
    let wrong = BUCK.replace("component U1: p.Buck;", "component U1: p.Buck { switching_frequency = 5V; }");
    expect_error("wrong-unit", &ranged, &wrong, "CMP130");
    let fixed_mismatch =
        BUCK.replace("component U1: p.Buck;", "component U1: p.Buck { switching_frequency = 600kHz; }");
    expect_error("fixed", PARTS, &fixed_mismatch, "CMP130");
    let inside = BUCK.replace("component U1: p.Buck;", "component U1: p.Buck { switching_frequency = 100kHz; }");
    design("inside", &ranged, &inside);
}

#[test]
fn device_pad_attributes_reach_the_package_pin() {
    let parts = r#"
device DevBuck {
    pad VIN { domains = "power"; directions = "input"; emi_role = hot_supply; }
    pad SW { domains = "power"; directions = "output"; emi_role = switch; rise_time = 4ns; fall_time = 6ns; current_peak = 3A; }
    pad GND { domains = "ground"; directions = "input"; emi_role = hot_return; }
}
part Packaged {
    category = "power.regulator.buck";
    device = DevBuck;
    switching_frequency = 1MHz;
    pin VIN { number = "1"; bond = VIN; }
    pin SW { number = "2"; bond = SW; }
    pin GND { number = "3"; bond = GND; }
}
part Inductor {
    category = "passive.inductor";
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin B { number = "2"; domains = "power"; directions = "passive"; }
}
part Cap {
    category = "passive.capacitor";
    pin A { number = "1"; domains = "power"; directions = "passive"; }
    pin B { number = "2"; domains = "power"; directions = "passive"; }
}
"#;
    let board = r#"
    component U1: p.Packaged;
    component L1: p.Inductor;
    component CIN: p.Cap { role = decoupling; decouples = U1.VIN; }
    net VIN {
        U1.VIN;
        CIN.A;
    }
    net GND {
        U1.GND;
        CIN.B;
    }
    net SW {
        U1.SW;
        L1.A;
    }
"#;
    let compiled = design("device", parts, board);
    let found = emi_diagnostics(&compiled);
    assert!(found.is_empty(), "{found:?}");
    let cell = model(&compiled).cells.remove(0);
    assert_eq!(cell.topology, Some(Topology::Buck));
    assert_eq!(cell.fall_time.as_ref().map(ToString::to_string).as_deref(), Some("6 ns"));
    assert_eq!(cell.hot_capacitors[0].component, "CIN");
    expect_error("pad-role", &parts.replace("emi_role = switch;", "emi_role = nope;"), board, "CMP130");
}

fn constraint_board(constraint: &str) -> String {
    format!("{BUCK}\n    component J1: p.Conn;\n    net CABLE {{\n        J1.A;\n    }}\n    {constraint}\n")
}

#[test]
fn emission_constraint_is_validated() {
    let ok = constraint_board(
        "constraint emission(J1) { standard = \"cispr32_class_b\"; cable_length = 1m; mode = target; }",
    );
    let compiled = design("emission", PARTS, &ok);
    assert!(compiled.electrical.constraints.iter().any(|item| item.kind.value() == "emission"));
    assert!(emi_diagnostics(&compiled).is_empty());
    for (name, constraint) in [
        ("no-standard", "constraint emission(J1) { cable_length = 1m; mode = target; }"),
        ("bad-standard", "constraint emission(J1) { standard = \"nope\"; mode = target; }"),
        ("bad-length", "constraint emission(J1) { standard = \"fcc15_class_a\"; cable_length = 0m; mode = target; }"),
        ("length-unit", "constraint emission(J1) { standard = \"fcc15_class_a\"; cable_length = 1V; mode = target; }"),
        ("pin-target", "constraint emission(J1.A) { standard = \"fcc15_class_a\"; mode = target; }"),
        ("unknown-parameter", "constraint emission(J1) { standard = \"fcc15_class_a\"; band = 1; mode = target; }"),
    ] {
        expect_error(name, PARTS, &constraint_board(constraint), "CMP131");
    }
    let twice = constraint_board(
        "constraint emission(J1) { standard = \"fcc15_class_a\"; mode = target; }\n    constraint emission(J1) { standard = \"fcc15_class_b\"; mode = target; }",
    );
    expect_error("twice", PARTS, &twice, "CMP131");
    let unknown = constraint_board("constraint emission(J9) { standard = \"fcc15_class_a\"; mode = target; }");
    let compiled = design("unknown-target", PARTS, &unknown);
    let found = erc::check(&compiled.electrical, false).expect("checks");
    assert!(found.iter().any(|item| item.code == "UNKNOWN_CONSTRAINT_TARGET"), "{found:?}");
}

#[test]
fn emi_accept_constraint_is_validated() {
    let ok = constraint_board(
        "constraint emi_accept(U1.SW) { code = \"EMI-SWITCH-NODE\"; reason = \"fixed by the thermal pad\"; mode = target; }\n    constraint emi_accept(U1) { code = \"EMI-HOT-LOOP\"; reason = \"reviewed\"; mode = target; }",
    );
    let compiled = design("accept", PARTS, &ok);
    assert_eq!(compiled.electrical.constraints.iter().filter(|item| item.kind.value() == "emi_accept").count(), 2);
    let found = erc::check(&compiled.electrical, false).expect("checks");
    assert!(!found.iter().any(|item| item.code == "UNKNOWN_CONSTRAINT_TARGET"), "{found:?}");
    for (name, constraint) in [
        ("no-code", "constraint emi_accept(U1.SW) { reason = \"x\"; mode = target; }"),
        ("no-reason", "constraint emi_accept(U1.SW) { code = \"EMI-SWITCH-NODE\"; mode = target; }"),
        ("blank-reason", "constraint emi_accept(U1.SW) { code = \"A\"; reason = \"  \"; mode = target; }"),
        ("empty-code", "constraint emi_accept(U1.SW) { code = \"\"; reason = \"x\"; mode = target; }"),
        ("two-targets", "constraint emi_accept(U1.SW, U1) { code = \"A\"; reason = \"x\"; mode = target; }"),
        ("extra", "constraint emi_accept(U1.SW) { code = \"A\"; reason = \"x\"; limit = 3; mode = target; }"),
    ] {
        expect_error(name, PARTS, &constraint_board(constraint), "CMP132");
    }
    let bad_pin = constraint_board("constraint emi_accept(U1.NOPE) { code = \"A\"; reason = \"x\"; mode = target; }");
    let compiled = design("accept-pin", PARTS, &bad_pin);
    let found = erc::check(&compiled.electrical, false).expect("checks");
    assert!(found.iter().any(|item| item.code == "UNKNOWN_CONSTRAINT_TARGET"), "{found:?}");
}

#[test]
fn hot_loop_without_a_capacitor_warns() {
    let board = without(BUCK, &["CIN"]);
    let found = emi_diagnostics(&design("no-cap", PARTS, &board));
    assert_eq!(codes(&found), ["EMI_HOT_LOOP_NO_CAPACITOR"], "{found:?}");
    assert_eq!((found[0].severity, found[0].subject.as_deref()), (Severity::Warning, Some("U1")));
    assert!(found[0].message.contains("VIN") && found[0].message.contains("GND"), "{}", found[0].message);
}

#[test]
fn undeclared_hot_capacitor_warns() {
    let board = BUCK.replace(" role = decoupling; decouples = U1.VIN;", "");
    let compiled = design("undeclared", PARTS, &board);
    let found = emi_diagnostics(&compiled);
    assert_eq!(codes(&found), ["EMI_HOT_CAPACITOR_UNDECLARED"], "{found:?}");
    assert!(found[0].message.contains("decouples = U1.VIN"), "{}", found[0].message);
    let cell = model(&compiled).cells.remove(0);
    assert!(!cell.hot_capacitors[0].declared);
}

#[test]
fn declared_hot_capacitors_sort_first() {
    let board = BUCK
        .replace(
            "component COUT: p.Cap { value = 22uF; }",
            "component COUT: p.Cap { value = 22uF; }\n    component CBULK: p.Cap { value = 47uF; }",
        )
        .replace("        CIN.A;\n", "        CBULK.A;\n        CIN.A;\n")
        .replace("        CIN.B;\n", "        CBULK.B;\n        CIN.B;\n");
    let cell = model(&design("order", PARTS, &board)).cells.remove(0);
    let order: Vec<_> = cell.hot_capacitors.iter().map(|c| (c.component.as_str(), c.declared)).collect();
    assert_eq!(order, [("CIN", true), ("CBULK", false)]);
}

#[test]
fn switch_node_without_a_magnetic_warns() {
    let board = without(BUCK, &["L1"]);
    let found = emi_diagnostics(&design("no-magnetic", PARTS, &board));
    assert_eq!(codes(&found), ["EMI_SWITCH_NODE_NO_MAGNETIC"], "{found:?}");
    assert_eq!(found[0].subject.as_deref(), Some("SW"));
}

#[test]
fn charge_pump_switch_node_needs_no_magnetic() {
    let parts = PARTS.replace("switching_frequency = 500kHz;", "switching_frequency = 500kHz; topology = charge_pump;");
    let board = without(BUCK, &["L1"]);
    assert!(emi_diagnostics(&design("pump", &parts, &board)).is_empty());
}

#[test]
fn bootstrap_pin_without_a_capacitor_warns() {
    let board = without(BUCK, &["CBOOT"]);
    let found = emi_diagnostics(&design("no-bootstrap", PARTS, &board));
    assert_eq!(codes(&found), ["EMI_BOOTSTRAP_NO_CAPACITOR"], "{found:?}");
    assert_eq!(found[0].subject.as_deref(), Some("U1.BOOT"));
    // A capacitor from BOOT to ground is not a bootstrap capacitor.
    let wrong = BUCK.replace("        CBOOT.B;\n", "").replace("        D1.A;\n", "        D1.A;\n        CBOOT.B;\n");
    let found = emi_diagnostics(&design("wrong-bootstrap", PARTS, &wrong));
    assert_eq!(codes(&found), ["EMI_BOOTSTRAP_NO_CAPACITOR"], "{found:?}");
}

#[test]
fn sensitive_pin_on_the_switch_node_warns() {
    let board = BUCK
        .replace("        U1.FB;\n        R1.B;", "        R1.B;")
        .replace("        U1.SW;\n", "        U1.SW;\n        U1.FB;\n");
    let found = emi_diagnostics(&design("sensitive", PARTS, &board));
    assert_eq!(codes(&found), ["EMI_SENSITIVE_ON_SWITCH_NODE"], "{found:?}");
    assert_eq!(found[0].subject.as_deref(), Some("U1.FB"));
}

#[test]
fn switch_node_reaching_a_cable_connector_warns() {
    let board =
        format!("{BUCK}\n    component J1: p.Conn;\n").replace("        U1.SW;\n", "        U1.SW;\n        J1.A;\n");
    let compiled = design("external", PARTS, &board);
    let found = emi_diagnostics(&compiled);
    assert_eq!(codes(&found), ["EMI_SWITCH_NODE_EXTERNAL"], "{found:?}");
    assert!(found[0].message.contains("J1"), "{}", found[0].message);
    assert_eq!(model(&compiled).nets["SW"].external, ["J1"]);

    // Through a series resistor to the cable pin.
    let series = format!(
        "{BUCK}\n    component J1: p.Conn;\n    component R9: p.Res {{ value = 10ohm; }}\n    net CABLE {{\n        J1.A;\n        R9.B;\n    }}\n"
    )
    .replace("        U1.SW;\n        L1.A;", "        U1.SW;\n        R9.A;\n        L1.A;");
    let found = emi_diagnostics(&design("external-series", PARTS, &series));
    assert_eq!(codes(&found), ["EMI_SWITCH_NODE_EXTERNAL"], "{found:?}");

    // The output side of the inductor may leave the board.
    let output =
        format!("{BUCK}\n    component J1: p.Conn;\n    net CABLE {{\n        J1.A;\n        COUT.A;\n    }}\n");
    let output = output.replace("        COUT.A;\n        R1.A;", "        R1.A;");
    let compiled = design("external-output", PARTS, &output);
    assert!(emi_diagnostics(&compiled).is_empty());
}

#[test]
fn emission_targets_mark_a_connector_external_without_the_part_flag() {
    let parts = PARTS.replace("    external_cable = true;\n", "");
    let board = format!(
        "{BUCK}\n    component J1: p.Conn;\n    constraint emission(J1) {{ standard = \"cispr32_class_b\"; mode = target; }}\n"
    )
    .replace("        U1.SW;\n", "        U1.SW;\n        J1.A;\n");
    let found = emi_diagnostics(&design("emission-external", &parts, &board));
    assert_eq!(codes(&found), ["EMI_SWITCH_NODE_EXTERNAL"], "{found:?}");
}

#[test]
fn structural_fallback_infers_a_cell_without_diagnostics() {
    let board = r#"
    component U1: p.Reg;
    component L1: p.Inductor { value = 4.7uH; }
    component C1: p.Cap { value = 10uF; }
    supply VIN { voltage = 12V; external = true; }
    net VIN {
        U1.VIN;
        C1.A;
    }
    net GND {
        U1.GND;
        C1.B;
    }
    net SW {
        U1.SW;
        L1.A;
    }
    net VOUT {
        L1.B;
    }
"#;
    let compiled = design("inferred", PARTS, board);
    assert!(emi_diagnostics(&compiled).is_empty());
    let found = model(&compiled);
    let cell = found.cell("U1").expect("inferred cell");
    assert_eq!(cell.provenance, Provenance::Inferred);
    assert_eq!(cell.topology, Some(Topology::Buck));
    assert_eq!(cell.switch_pins[0].to_string(), "U1.SW");
    assert_eq!(cell.hot_supply_pins[0].to_string(), "U1.VIN");
    assert_eq!(cell.hot_return_pins[0].to_string(), "U1.GND");
    assert_eq!(cell.magnetic.as_ref().map(|m| m.component.as_str()), Some("L1"));
    assert_eq!(cell.hot_capacitors[0].component, "C1");
    assert!(cell.frequency.is_none());
    assert_eq!(found.declared_cells().count(), 0);
    assert_eq!(found.nets["SW"].aggressor, Some(AggressorClass::SwitchNode));
    assert_eq!(found.nets["SW"].provenance, Provenance::Inferred);
    assert!(found.to_json().contains("\"provenance\": \"inferred\""));
}

#[test]
fn a_regulator_with_any_role_is_not_inferred() {
    // Only the feedback pin is annotated: no switch pin, so no cell, and no fallback.
    let parts = PARTS.replace(
        "pin SW  { number = \"2\"; domains = \"power\"; directions = \"output\"; }",
        "pin SW  { number = \"2\"; domains = \"power\"; directions = \"output\"; }\n    pin FB { number = \"4\"; domains = \"analog\"; directions = \"input\"; emi_role = feedback; }",
    );
    let board = "    component U1: p.Reg;\n    component L1: p.Inductor;\n    supply VIN { voltage = 12V; external = true; }\n    net VIN {\n        U1.VIN;\n    }\n    net SW {\n        U1.SW;\n        L1.A;\n    }\n";
    let compiled = design("partial", &parts, board);
    assert!(model(&compiled).cells.is_empty());
    assert!(emi_diagnostics(&compiled).is_empty());
}

#[test]
fn query_emi_prints_the_model() {
    let compiled = design("query", PARTS, BUCK);
    let view = elaborate(&compiled.electrical).expect("elaborates");
    let result = copper_lang::query::run_query(view, "emi", None).expect("query");
    assert_eq!(result.get("schema").and_then(|v| v.as_str()), Some(emi::SCHEMA));
    assert_eq!(result.get("cells").and_then(|v| v.as_array()).map(<[_]>::len), Some(1));
    assert!(copper_lang::query::render_query(&result).contains("\"controller\": \"U1\""));
}

//! The cable attributes of the EMI model (P4 of `docs/emi-signal-cable-plan.md`): `emi_role = shield`,
//! `output_impedance` with `open_circuit_swing`, `cable_shielded`, `cable_common_mode_impedance` and
//! `common_mode_impedance` with `common_mode_impedance_frequency`. All optional; a design that declares none of them
//! compiles to the IR it had before.

use std::fs;
use std::path::PathBuf;

use copper_lang::compiler::{Design, compile_design_file};
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_lang::model::{CommonModeValue, EmiRole};
use copper_lang::serializer::{design_json, part_value};

fn project(name: &str, parts: &str, board_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-emi-cable-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
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

fn expect_error(name: &str, parts: &str, board: &str, code: &str) {
    match compile(name, parts, board) {
        Ok(_) => panic!("{name}: expected {code}"),
        Err(failure) => assert!(failure.contains(code), "{name}: expected {code}, got {failure}"),
    }
}

const PARTS: &str = r#"
part Conn {
    category = "connector.header";
    external_cable = true;
    cable_shielded = true;
    cable_common_mode_impedance = 150ohm;
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
    pin SH { number = "3"; domains = "ground"; directions = "passive"; emi_role = shield; }
}
part Choke {
    category = "passive.inductor.common_mode";
    common_mode_impedance = "30 ohm, 120+j300 ohm, 0.4 kohm";
    common_mode_impedance_frequency = "10 MHz, 100 MHz, 1 GHz";
    pin IN1 { number = "1"; domains = "digital"; directions = "passive"; passes_to = OUT1; }
    pin IN2 { number = "2"; domains = "digital"; directions = "passive"; passes_to = OUT2; }
    pin OUT1 { number = "4"; domains = "digital"; directions = "passive"; }
    pin OUT2 { number = "3"; domains = "digital"; directions = "passive"; }
}
part Ferrite {
    category = "passive.ferrite";
    common_mode_impedance = 600ohm;
    common_mode_impedance_frequency = 100MHz;
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Plain {
    category = "passive.ferrite";
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Thevenin {
    category = "interface.serdes";
    pin TX { number = "1"; domains = "digital"; directions = "output"; open_circuit_swing = 2V; output_impedance = 50ohm; data_rate = 1Gbps; }
}
part Loaded {
    category = "interface.serdes";
    pin TX { number = "1"; domains = "digital"; directions = "output"; signal_swing = 1V; data_rate = 1Gbps; }
}
"#;

fn part<'a>(compiled: &'a Design, suffix: &str) -> &'a copper_lang::model::PartDefinition {
    compiled.electrical.library.values().find(|part| part.name.ends_with(suffix)).unwrap()
}

#[test]
fn the_cable_attributes_are_accepted_and_reach_the_ir() {
    let compiled = design(
        "accept",
        PARTS,
        "    component J1: p.Conn;\n    component L1: p.Choke;\n    component F1: p.Ferrite;\n    component U1: p.Thevenin;\n    net X { U1.TX; J1.A; }\n",
    );
    let conn = part(&compiled, "Conn").emi.clone().unwrap();
    assert_eq!(conn.cable_shielded, Some(true));
    assert_eq!(conn.cable_common_mode_impedance.as_ref().map(ToString::to_string).as_deref(), Some("150 ohm"));
    let shield = part(&compiled, "Conn").pins["SH"].profile.as_ref().unwrap().emi.clone().unwrap();
    assert_eq!(shield.role, Some(EmiRole::Shield));
    assert!(!EmiRole::Shield.is_aggressor(), "a shield pin is a victim-side role");

    let choke = part(&compiled, "Choke").emi.clone().unwrap();
    let points = &choke.common_mode_impedance;
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].frequency.to_string(), "10 MHz");
    assert!(matches!(&points[0].impedance, CommonModeValue::Magnitude(m) if m.to_string() == "30 ohm"));
    match &points[1].impedance {
        CommonModeValue::Complex { resistance, reactance } => {
            assert_eq!(resistance.base_value().decimal_text(), "120");
            assert_eq!(reactance.base_value().decimal_text(), "300");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&points[2].impedance, CommonModeValue::Magnitude(m) if m.base_value().decimal_text() == "400"));
    let ferrite = part(&compiled, "Ferrite").emi.clone().unwrap();
    assert_eq!(ferrite.common_mode_impedance.len(), 1);
    assert_eq!(ferrite.common_mode_impedance[0].frequency.to_string(), "100 MHz");

    let tx = part(&compiled, "Thevenin").pins["TX"].profile.as_ref().unwrap().emi.clone().unwrap();
    assert_eq!(tx.open_circuit_swing.as_ref().map(ToString::to_string).as_deref(), Some("2 V"));
    assert_eq!(tx.output_impedance.as_ref().map(ToString::to_string).as_deref(), Some("50 ohm"));

    // The IR carries the data, and only on what declared it.
    let dump = |value: &copper_json::Value| copper_json::Dumps::compact().sort_keys(true).dumps(value);
    let text = dump(&part_value(part(&compiled, "Choke")));
    assert!(text.contains("common_mode_impedance") && text.contains("\"reactance\"") && text.contains("\"magnitude\""));
    let connector = dump(&part_value(part(&compiled, "Conn")));
    assert!(connector.contains("cable_shielded") && connector.contains("cable_common_mode_impedance"));
    assert!(connector.contains("\"role\":\"shield\""));
    assert!(!connector.contains("\"common_mode_impedance\""));
    let plain = dump(&part_value(part(&compiled, "Plain")));
    assert!(!plain.contains("common_mode_impedance") && !plain.contains("cable_"));
}

#[test]
fn an_unannotated_design_has_none_of_the_new_keys() {
    let parts = r#"
part Chip {
    category = "component.generic";
    pin A { number = "1"; domains = "digital"; directions = "output"; rise_time = 1ns; }
    pin B { number = "2"; domains = "digital"; directions = "input"; }
}
"#;
    let compiled = design("plain", parts, "    component U1: p.Chip;\n    net X { U1.A; U1.B; }\n");
    let json = design_json(&compiled);
    for key in [
        "output_impedance",
        "open_circuit_swing",
        "cable_shielded",
        "cable_common_mode_impedance",
        "common_mode_impedance",
        "shield",
    ] {
        assert!(!json.contains(key), "{key} in {json}");
    }
}

#[test]
fn a_shield_pin_makes_no_switching_cell_and_no_aggressor() {
    let compiled = design("shield-model", PARTS, "    component J1: p.Conn;\n    net X { J1.A; }\n");
    let model = emi::analyze(&elaborate(&compiled.electrical).expect("elaborates"));
    assert!(model.cells.is_empty());
    assert!(model.nets.values().all(|classes| classes.aggressor.is_none()));
}

#[test]
fn the_thevenin_source_reaches_the_signal_source_and_is_listed_only_when_declared() {
    let compiled = design(
        "thevenin",
        PARTS,
        "    component U1: p.Thevenin;\n    component U2: p.Loaded;\n    component J1: p.Conn;\n    net X { U1.TX; J1.A; }\n    net Y { U2.TX; J1.B; }\n",
    );
    let model = emi::analyze(&elaborate(&compiled.electrical).expect("elaborates"));
    let x = &model.signals["X"];
    assert_eq!(x.open_circuit_swing.as_ref().map(|n| n.value.to_string()).as_deref(), Some("2 V"));
    assert_eq!(x.output_impedance.as_ref().map(|n| n.value.to_string()).as_deref(), Some("50 ohm"));
    assert!(x.swing.is_none());
    let y = &model.signals["Y"];
    assert!(y.open_circuit_swing.is_none() && y.output_impedance.is_none());
    assert_eq!(y.swing.as_ref().map(|n| n.value.to_string()).as_deref(), Some("1 V"));
    let json = model.to_json();
    assert!(json.contains("open_circuit_swing"));
    // The Loaded source alone lists neither key.
    let loaded = design("loaded-only", PARTS, "    component U2: p.Loaded;\n    net Y { U2.TX; }\n");
    let model = emi::analyze(&elaborate(&loaded.electrical).expect("elaborates"));
    assert!(!model.to_json().contains("open_circuit_swing") && !model.to_json().contains("output_impedance"));
}

#[test]
fn the_source_representation_errors_are_cmp130() {
    let board = "    component U1: p.Thevenin;\n";
    for (name, from, to) in [
        ("impedance-alone", "open_circuit_swing = 2V; ", ""),
        ("swing-alone", "output_impedance = 50ohm; ", ""),
        ("both-representations", "open_circuit_swing = 2V;", "open_circuit_swing = 2V; signal_swing = 1V;"),
        ("impedance-zero", "output_impedance = 50ohm;", "output_impedance = 0ohm;"),
        ("impedance-voltage", "output_impedance = 50ohm;", "output_impedance = 50V;"),
        ("impedance-bare", "output_impedance = 50ohm;", "output_impedance = 50;"),
        ("swing-zero", "open_circuit_swing = 2V;", "open_circuit_swing = 0V;"),
        ("swing-bare", "open_circuit_swing = 2V;", "open_circuit_swing = 2;"),
    ] {
        let source = PARTS.replacen(from, to, 1);
        assert_ne!(source, PARTS, "{name}");
        expect_error(name, &source, board, "CMP130");
    }
    // `signal_swing` alone with an `output_impedance` is the double application the definition excludes.
    expect_error(
        "loaded-with-impedance",
        &PARTS.replacen("signal_swing = 1V;", "signal_swing = 1V; output_impedance = 50ohm;", 1),
        "    component U2: p.Loaded;\n",
        "CMP130",
    );
}

#[test]
fn the_choke_and_connector_attribute_errors_are_cmp130() {
    let board = "    component L1: p.Choke;\n    component F1: p.Ferrite;\n    component J1: p.Conn;\n";
    for (name, from, to) in [
        (
            "count",
            "common_mode_impedance_frequency = \"10 MHz, 100 MHz, 1 GHz\";",
            "common_mode_impedance_frequency = \"10 MHz, 100 MHz\";",
        ),
        ("order", "\"10 MHz, 100 MHz, 1 GHz\"", "\"100 MHz, 10 MHz, 1 GHz\""),
        ("equal-frequencies", "\"10 MHz, 100 MHz, 1 GHz\"", "\"10 MHz, 10 MHz, 1 GHz\""),
        ("frequency-unit", "\"10 MHz, 100 MHz, 1 GHz\"", "\"10 MHz, 100 ns, 1 GHz\""),
        ("frequency-zero", "\"10 MHz, 100 MHz, 1 GHz\"", "\"0 MHz, 100 MHz, 1 GHz\""),
        ("impedance-unit", "\"30 ohm, 120+j300 ohm, 0.4 kohm\"", "\"30 ohm, 120+j300 ohm, 0.4 V\""),
        ("impedance-negative", "\"30 ohm, 120+j300 ohm, 0.4 kohm\"", "\"-30 ohm, 120+j300 ohm, 0.4 kohm\""),
        ("resistance-negative", "\"30 ohm, 120+j300 ohm, 0.4 kohm\"", "\"30 ohm, -5+j300 ohm, 0.4 kohm\""),
        ("impedance-text", "\"30 ohm, 120+j300 ohm, 0.4 kohm\"", "\"30 ohm, big, 0.4 kohm\""),
        ("impedance-no-unit", "\"30 ohm, 120+j300 ohm, 0.4 kohm\"", "\"30, 120+j300 ohm, 0.4 kohm\""),
        ("ferrite-no-frequency", "common_mode_impedance_frequency = 100MHz;", ""),
        ("ferrite-bool", "common_mode_impedance = 600ohm;", "common_mode_impedance = true;"),
        ("shielded-text", "cable_shielded = true;", "cable_shielded = \"yes\";"),
        ("cable-impedance-zero", "cable_common_mode_impedance = 150ohm;", "cable_common_mode_impedance = 0ohm;"),
        ("cable-impedance-unit", "cable_common_mode_impedance = 150ohm;", "cable_common_mode_impedance = 150nF;"),
    ] {
        let source = PARTS.replacen(from, to, 1);
        assert_ne!(source, PARTS, "{name}");
        expect_error(name, &source, board, "CMP130");
    }
    // An impedance declared at a frequency only.
    let alone = PARTS.replacen("common_mode_impedance = 600ohm;", "", 1);
    expect_error("frequency-alone", &alone, board, "CMP130");
    // A zero impedance is a legitimate declaration: it changes nothing.
    design("zero-choke", &PARTS.replacen("common_mode_impedance = 600ohm;", "common_mode_impedance = 0ohm;", 1), board);
    // A single point may also be written as a one-entry list.
    design(
        "one-entry-list",
        &PARTS.replacen("common_mode_impedance = 600ohm;", "common_mode_impedance = \"600 ohm\";", 1),
        board,
    );
}

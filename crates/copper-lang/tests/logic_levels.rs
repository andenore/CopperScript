//! ERC logic-level checks on signal nets: `io_supply`, `vih`/`vil`/`voh`/`vol`
//! and pull-up levels.

use std::fs;
use std::path::PathBuf;

use copper_json::Dumps;
use copper_lang::compiler::compile_design_file;
use copper_lang::erc::{self, Diagnostic, Severity};
use copper_lang::serializer::part_value;

/// A project with one part or device per file and a board.
fn project(name: &str, files: &[(&str, &str)], board_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-logic-levels-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    for (file, source) in files {
        fs::write(dir.join("parts").join(format!("{file}.copper")), source).unwrap();
    }
    fs::write(dir.join("board.copper"), format!("board B {{\n    import p \"./parts\";\n{board_body}\n}}\n")).unwrap();
    dir.join("board.copper")
}

fn logic_diagnostics(name: &str, files: &[(&str, &str)], board_body: &str) -> Vec<Diagnostic> {
    let design = compile_design_file(&project(name, files, board_body), false, false)
        .unwrap_or_else(|failure| panic!("compiles: {failure}"));
    erc::check(&design.electrical, false)
        .expect("checks")
        .into_iter()
        .filter(|item| item.code.starts_with("LOGIC_"))
        .collect()
}

fn compile_error(name: &str, files: &[(&str, &str)]) -> String {
    match compile_design_file(&project(name, files, "    component U1: p.Rx;"), false, false) {
        Ok(_) => panic!("expected a compile error"),
        Err(failure) => failure.to_string(),
    }
}

const DRIVER: (&str, &str) = (
    "driver",
    r#"part Tx {
    pin OUT { number = "1"; domains = "digital"; directions = "output"; drive_modes = "push_pull"; io_supply = VDD; }
    pin VDD { number = "2"; domains = "power"; directions = "input"; }
}"#,
);

fn receiver(pin_properties: &str) -> String {
    format!(
        "part Rx {{\n    pin IN {{ number = \"1\"; domains = \"digital\"; directions = \"input\"; {pin_properties} }}\n    pin VDD {{ number = \"2\"; domains = \"power\"; directions = \"input\"; }}\n}}"
    )
}

/// A 3.3 V driver and a receiver on one signal net.
const BOARD_33: &str = r#"
    component U1: p.Tx;
    component U2: p.Rx;
    supply V3V3 { voltage = 3.3V; external = true; }
    net V3V3 { U1.VDD; U2.VDD; }
    net SIG { U1.OUT; U2.IN; }
"#;

#[test]
fn driver_above_receiver_maximum_is_an_error() {
    let rx = receiver("voltage_max = 1.8V;");
    let found = logic_diagnostics("over-error", &[DRIVER, ("rx", &rx)], BOARD_33);
    assert_eq!(found.len(), 1, "{found:?}");
    let item = &found[0];
    assert_eq!((item.code.as_str(), item.severity), ("LOGIC_HIGH_OVERVOLTAGE", Severity::Error));
    assert_eq!(item.subject.as_deref(), Some("U2.IN"));
    for needle in ["U1.OUT", "SIG", "3.3 V", "io_supply VDD = 3.3 V", "1.8 V", "U2.IN"] {
        assert!(item.message.contains(needle), "{needle}: {}", item.message);
    }
}

#[test]
fn driver_inside_a_declared_absolute_maximum_is_a_warning() {
    let rx = receiver("voltage_max = 1.8V; absolute_max = 3.6V;");
    let found = logic_diagnostics("over-warning", &[DRIVER, ("rx", &rx)], BOARD_33);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), ("LOGIC_HIGH_OVERVOLTAGE", Severity::Warning));
    assert!(found[0].message.contains("within its absolute maximum 3.6 V"), "{}", found[0].message);
}

#[test]
fn driver_above_the_absolute_maximum_is_an_error() {
    let rx = receiver("voltage_max = 1.8V; absolute_max = 3.0V;");
    let found = logic_diagnostics("over-absolute", &[DRIVER, ("rx", &rx)], BOARD_33);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), ("LOGIC_HIGH_OVERVOLTAGE", Severity::Error));
    assert!(found[0].message.contains("absolute maximum 3 V"), "{}", found[0].message);
}

#[test]
fn relative_absolute_maximum_follows_the_receiver_rail() {
    // The receiver's absolute maximum is VDD + 0.3 V; its own VDD is a 1.8 V rail.
    let rx = receiver("absolute_max = \"VDD+0.3V\";");
    let board = r#"
    component U1: p.Tx;
    component U2: p.Rx;
    supply V3V3 { voltage = 3.3V; external = true; }
    supply V1V8 { voltage = 1.8V; external = true; }
    net V3V3 { U1.VDD; }
    net V1V8 { U2.VDD; }
    net SIG { U1.OUT; U2.IN; }
"#;
    let found = logic_diagnostics("over-relative", &[DRIVER, ("rx", &rx)], board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].message.contains("2.1 V"), "{}", found[0].message);
}

#[test]
fn output_high_below_a_rail_relative_vih_is_an_error() {
    let rx = receiver("vih = \"0.7*VDD\";");
    let board = r#"
    component U1: p.Tx;
    component U2: p.Rx;
    supply V1V8 { voltage = 1.8V; external = true; }
    supply V3V3 { voltage = 3.3V; external = true; }
    net V1V8 { U1.VDD; }
    net V3V3 { U2.VDD; }
    net SIG { U1.OUT; U2.IN; }
"#;
    let found = logic_diagnostics("too-low", &[DRIVER, ("rx", &rx)], board);
    assert_eq!(found.len(), 1, "{found:?}");
    let item = &found[0];
    assert_eq!((item.code.as_str(), item.severity), ("LOGIC_HIGH_TOO_LOW", Severity::Error));
    for needle in ["U1.OUT", "1.8 V", "2.31 V", "0.7*VDD", "U2.IN"] {
        assert!(item.message.contains(needle), "{needle}: {}", item.message);
    }
}

#[test]
fn declared_voh_replaces_the_rail_as_the_output_high_level() {
    let driver = DRIVER.1.replace("io_supply = VDD;", "io_supply = VDD; voh = \"VDD-0.4V\";");
    let rx = receiver("vih = 3.0V;");
    let found = logic_diagnostics("voh", &[("driver", &driver), ("rx", &rx)], BOARD_33);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "LOGIC_HIGH_TOO_LOW");
    assert!(found[0].message.contains("2.9 V"), "{}", found[0].message);
}

#[test]
fn output_low_above_input_low_is_an_error() {
    let driver = DRIVER.1.replace("io_supply = VDD;", "io_supply = VDD; vol = 1.2V;");
    let rx = receiver("vil = \"0.3*VDD\";");
    let found = logic_diagnostics("vol-vil", &[("driver", &driver), ("rx", &rx)], BOARD_33);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), ("LOGIC_LOW_TOO_HIGH", Severity::Error));
    assert!(found[0].message.contains("1.2 V") && found[0].message.contains("0.99 V"), "{}", found[0].message);
}

#[test]
fn level_compatible_pair_is_clean() {
    let driver = DRIVER.1.replace("io_supply = VDD;", "io_supply = VDD; voh = \"VDD-0.4V\"; vol = 0.4V;");
    let rx = receiver("vih = \"0.7*VDD\"; vil = \"0.3*VDD\"; voltage_max = 3.6V;");
    let found = logic_diagnostics("clean", &[("driver", &driver), ("rx", &rx)], BOARD_33);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn pull_up_is_checked_as_a_high_level() {
    let open_drain = r#"part Od {
    pin IO { number = "1"; domains = "digital"; directions = "bidirectional"; drive_modes = "open_drain"; absolute_max = 3.6V; }
}"#;
    let resistor = r#"part Res {
    category = "passive.resistor";
    pin A { number = "1"; domains = "analog"; directions = "passive"; }
    pin B { number = "2"; domains = "analog"; directions = "passive"; }
}"#;
    let board = r#"
    component U1: p.Od;
    component R1: p.Res;
    supply V5V { voltage = 5V; external = true; }
    net V5V { R1.A; }
    net SIG { U1.IO; R1.B; }
"#;
    let files = [("od", open_drain), ("res", resistor)];
    let found = logic_diagnostics("pull-up", &files, board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), ("LOGIC_HIGH_OVERVOLTAGE", Severity::Error));
    assert_eq!(found[0].subject.as_deref(), Some("U1.IO"));
    assert!(found[0].message.contains("pull-up resistor to V5V"), "{}", found[0].message);

    // A resistor to ground as well makes a bias network, which has no single high level.
    let divider = board.replace(
        "net SIG { U1.IO; R1.B; }",
        "supply GND { voltage = 0V; external = true; }\n    component R2: p.Res;\n    net GND { R2.A; }\n    net SIG { U1.IO; R1.B; R2.B; }",
    );
    assert!(logic_diagnostics("divider", &files, &divider).is_empty());
}

#[test]
fn device_pad_power_domain_gives_the_rail() {
    let device = r#"device Mcu {
    power_domain IOD { supply_pads = "VDDIO"; }
    pad VDDIO { domains = "power"; directions = "input"; }
    pad P1 { domains = "digital"; directions = "output"; drive_modes = "push_pull"; power_domain = IOD; }
}"#;
    let part = r#"part McuPart {
    footprint = "X";
    device = Mcu;
    pin VDDIO { number = "1"; bond = VDDIO; }
    pin P1 { number = "2"; bond = P1; }
}"#;
    let rx = receiver("voltage_max = 1.8V;");
    let board = r#"
    component U1: p.McuPart;
    component U2: p.Rx;
    supply V3V3 { voltage = 3.3V; external = true; }
    net V3V3 { U1.VDDIO; }
    net SIG { U1.P1; U2.IN; }
"#;
    let found = logic_diagnostics("domain", &[("device", device), ("part", part), ("rx", &rx)], board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "LOGIC_HIGH_OVERVOLTAGE");
    assert!(found[0].message.contains("power domain IOD supply pad VDDIO = 3.3 V"), "{}", found[0].message);
}

#[test]
fn unannotated_parts_are_not_checked() {
    let driver = r#"part Tx {
    pin OUT { number = "1"; domains = "digital"; directions = "output"; drive_modes = "push_pull"; }
}"#;
    let rx = receiver("voltage_max = 1.8V; vih = 1.5V; vil = 0.3V;");
    let board = "    component U1: p.Tx;\n    component U2: p.Rx;\n    net SIG { U1.OUT; U2.IN; }";
    assert!(logic_diagnostics("unannotated", &[("driver", driver), ("rx", &rx)], board).is_empty());
}

#[test]
fn a_driver_without_declared_drive_modes_is_not_assumed_push_pull() {
    let driver = DRIVER.1.replace("drive_modes = \"push_pull\"; ", "");
    let rx = receiver("voltage_max = 1.8V;");
    assert!(logic_diagnostics("no-drive-mode", &[("driver", &driver), ("rx", &rx)], BOARD_33).is_empty());
}

#[test]
fn pins_of_one_component_on_the_same_rail_are_skipped() {
    let part = r#"part Loop {
    pin OUT { number = "1"; domains = "digital"; directions = "output"; drive_modes = "push_pull"; io_supply = VDD; }
    pin IN { number = "2"; domains = "digital"; directions = "input"; io_supply = VDD; voltage_max = 1.8V; }
    pin VDD { number = "3"; domains = "power"; directions = "input"; }
}"#;
    let board = r#"
    component U1: p.Loop;
    supply V3V3 { voltage = 3.3V; external = true; }
    net V3V3 { U1.VDD; }
    net SIG { U1.OUT; U1.IN; }
"#;
    assert!(logic_diagnostics("same-rail", &[("loop", part)], board).is_empty());
}

#[test]
fn unresolved_explicit_rail_is_an_error() {
    let board = "    component U1: p.Tx;\n    component U2: p.Rx;\n    net SIG { U1.OUT; U2.IN; }";
    let rx = receiver("voltage_max = 1.8V;");
    let found = logic_diagnostics("unresolved-io", &[DRIVER, ("rx", &rx)], board);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), ("LOGIC_LEVEL_UNRESOLVED", Severity::Error));
    assert_eq!(found[0].subject.as_deref(), Some("U1.OUT"));
    assert!(found[0].message.contains("io_supply VDD"), "{}", found[0].message);

    let rx = receiver("vih = \"0.7*VDD\";");
    let found = logic_diagnostics("unresolved-vih", &[DRIVER, ("rx", &rx)], BOARD_33.replace("U2.VDD; ", "").as_str());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code, "LOGIC_LEVEL_UNRESOLVED");
    assert_eq!(found[0].subject.as_deref(), Some("U2.IN"));
    assert!(found[0].message.contains("vih 0.7*VDD"), "{}", found[0].message);
}

#[test]
fn supply_nets_are_left_to_the_supply_checks() {
    let rx = receiver("voltage_max = 1.8V;");
    let board = r#"
    component U1: p.Tx;
    component U2: p.Rx;
    supply V3V3 { voltage = 3.3V; external = true; }
    net V3V3 { U1.VDD; U2.VDD; U1.OUT; U2.IN; }
"#;
    assert!(logic_diagnostics("supply-net", &[DRIVER, ("rx", &rx)], board).is_empty());
}

#[test]
fn bad_syntax_is_a_compile_error() {
    let cases = [
        ("unknown-io-supply", "io_supply = NOPE;", "CMP127"),
        ("unknown-rail-vih", "vih = \"0.7*NOPE\";", "CMP127"),
        ("trailing-star", "vih = \"0.7*\";", "CMP120"),
        ("bare-number", "vih = 2;", "CMP120"),
        ("wrong-unit", "vil = 2.0A;", "CMP120"),
        ("zero-scale", "vih = \"0*VDD\";", "CMP120"),
        ("word", "vol = \"0.7 VDD\";", "CMP120"),
        ("inverted", "vih = 0.8V; vil = 1.2V;", "CMP121"),
    ];
    for (name, properties, code) in cases {
        let rx = format!(
            "part Rx {{\n    pin IN {{ number = \"1\"; domains = \"digital\"; directions = \"input\"; {properties} }}\n    pin VDD {{ number = \"2\"; domains = \"power\"; directions = \"input\"; }}\n}}"
        );
        let message = compile_error(name, &[("rx", &rx)]);
        assert!(message.contains(code), "{name}: {message}");
    }
}

#[test]
fn declared_fields_reach_the_ir_and_absent_fields_do_not() {
    let rx = receiver("io_supply = VDD; vih = \"0.7*VDD\"; vil = 0.4V;");
    let design = compile_design_file(&project("ir", &[("rx", &rx)], "    component U1: p.Rx;"), false, false).unwrap();
    let part = design.electrical.library.values().next().expect("one part");
    let json = Dumps::compact().dumps(&part_value(part));
    assert!(json.contains("\"io_supply\":\"VDD\""), "{json}");
    assert!(json.contains("\"logic_levels\":{\"vih\":{\"reference\":\"VDD\",\"scale\":\"0.7\""), "{json}");

    let plain = receiver("");
    let design =
        compile_design_file(&project("ir-plain", &[("rx", &plain)], "    component U1: p.Rx;"), false, false).unwrap();
    let part = design.electrical.library.values().next().expect("one part");
    let json = Dumps::compact().dumps(&part_value(part));
    assert!(!json.contains("io_supply") && !json.contains("logic_levels"), "{json}");
}

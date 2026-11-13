//! ERC pull checks: `requires_pull`, `internal_pull`, undriven inputs and
//! open-drain nets without a pull-up.

use std::fs;
use std::path::PathBuf;

use copper_json::Dumps;
use copper_lang::compiler::compile_design_file;
use copper_lang::erc::{self, Diagnostic, Severity};
use copper_lang::serializer::part_value;

/// A project with one part per file and a board.
fn project(name: &str, files: &[(&str, &str)], board_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-pull-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    for (file, source) in files {
        fs::write(dir.join("parts").join(format!("{file}.copper")), source).unwrap();
    }
    fs::write(dir.join("board.copper"), format!("board B {{\n    import p \"./parts\";\n{board_body}\n}}\n")).unwrap();
    dir.join("board.copper")
}

const PULL_CODES: [&str; 3] = ["MISSING_PULL", "UNDRIVEN_INPUT", "OPEN_DRAIN_NO_PULLUP"];

fn all_diagnostics(name: &str, files: &[(&str, &str)], board_body: &str) -> Vec<Diagnostic> {
    let design = compile_design_file(&project(name, files, board_body), false, false)
        .unwrap_or_else(|failure| panic!("compiles: {failure}"));
    erc::check(&design.electrical, false).expect("checks")
}

fn pull_diagnostics(name: &str, files: &[(&str, &str)], board_body: &str) -> Vec<Diagnostic> {
    all_diagnostics(name, files, board_body)
        .into_iter()
        .filter(|item| PULL_CODES.contains(&item.code.as_str()))
        .collect()
}

fn compile_error(name: &str, files: &[(&str, &str)]) -> String {
    match compile_design_file(&project(name, files, "    component U1: p.In;"), false, false) {
        Ok(_) => panic!("expected a compile error"),
        Err(failure) => failure.to_string(),
    }
}

fn input(pin_properties: &str) -> String {
    format!(
        "part In {{\n    pin IN {{ number = \"1\"; domains = \"digital\"; directions = \"input\"; {pin_properties} }}\n}}"
    )
}

const OUTPUT: (&str, &str) = (
    "out",
    r#"part Out {
    pin OUT { number = "1"; domains = "digital"; directions = "output"; drive_modes = "push_pull"; }
}"#,
);

const BIDIRECTIONAL: (&str, &str) = (
    "bidi",
    r#"part Bidi {
    pin IO { number = "1"; domains = "digital"; directions = "bidirectional"; drive_modes = "push_pull"; }
}"#,
);

const CONNECTOR: (&str, &str) = (
    "conn",
    r#"part Conn {
    pin P1 { number = "1"; domains = "analog"; directions = "passive"; }
}"#,
);

const OPEN_DRAIN: (&str, &str) = (
    "od",
    r#"part Od {
    pin IO { number = "1"; domains = "digital"; directions = "bidirectional"; drive_modes = "open_drain"; }
}"#,
);

const RESISTOR: (&str, &str) = (
    "res",
    r#"part Res {
    category = "passive.resistor";
    pin A { number = "1"; domains = "analog"; directions = "passive"; }
    pin B { number = "2"; domains = "analog"; directions = "passive"; }
}"#,
);

const SUPPLIES: &str = r#"
    supply V3V3 { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
"#;

fn board(body: &str) -> String {
    format!("{SUPPLIES}{body}")
}

fn only<'a>(found: &'a [Diagnostic], code: &str, severity: Severity) -> &'a Diagnostic {
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].code.as_str(), found[0].severity), (code, severity), "{found:?}");
    &found[0]
}

// MISSING_PULL

#[test]
fn a_required_pull_up_without_a_resistor_is_an_error() {
    let rx = input("requires_pull = up;");
    let found = pull_diagnostics(
        "missing-up",
        &[("rx", &rx), OUTPUT],
        &board("    component U1: p.In;\n    component U2: p.Out;\n    net SIG { U1.IN; U2.OUT; }"),
    );
    let item = only(&found, "MISSING_PULL", Severity::Error);
    assert_eq!(item.subject.as_deref(), Some("U1.IN"));
    for needle in ["U1.IN", "SIG", "pull-up"] {
        assert!(item.message.contains(needle), "{needle}: {}", item.message);
    }
    assert!(item.hint.as_deref().is_some_and(|hint| hint.contains("resistor")), "{item:?}");
}

#[test]
fn a_required_pull_down_without_a_resistor_is_an_error() {
    let rx = input("requires_pull = down;");
    let found = pull_diagnostics(
        "missing-down",
        &[("rx", &rx), OUTPUT],
        &board("    component U1: p.In;\n    component U2: p.Out;\n    net SIG { U1.IN; U2.OUT; }"),
    );
    let item = only(&found, "MISSING_PULL", Severity::Error);
    assert!(item.message.contains("pull-down"), "{}", item.message);
}

#[test]
fn a_pull_up_resistor_to_a_supply_satisfies_requires_pull() {
    let rx = input("requires_pull = up;");
    let found = pull_diagnostics(
        "up-resistor",
        &[("rx", &rx), RESISTOR, OUTPUT],
        &board(
            "    component U1: p.In;\n    component U2: p.Out;\n    component R1: p.Res;\n    net V3V3 { R1.A; }\n    net SIG { U1.IN; U2.OUT; R1.B; }",
        ),
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_pull_down_resistor_to_ground_satisfies_requires_pull() {
    let rx = input("requires_pull = down;");
    let found = pull_diagnostics(
        "down-resistor",
        &[("rx", &rx), RESISTOR, OUTPUT],
        &board(
            "    component U1: p.In;\n    component U2: p.Out;\n    component R1: p.Res;\n    net GND { R1.A; }\n    net SIG { U1.IN; U2.OUT; R1.B; }",
        ),
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_resistor_must_go_the_required_way() {
    // A pull-down does not satisfy requires_pull = up.
    let rx = input("requires_pull = up;");
    let found = pull_diagnostics(
        "wrong-way",
        &[("rx", &rx), RESISTOR, OUTPUT],
        &board(
            "    component U1: p.In;\n    component U2: p.Out;\n    component R1: p.Res;\n    net GND { R1.A; }\n    net SIG { U1.IN; U2.OUT; R1.B; }",
        ),
    );
    only(&found, "MISSING_PULL", Severity::Error);
}

#[test]
fn a_direct_supply_tie_satisfies_requires_pull() {
    let rx = input("requires_pull = up;");
    let found = pull_diagnostics("tie-up", &[("rx", &rx)], &board("    component U1: p.In;\n    net V3V3 { U1.IN; }"));
    assert!(found.is_empty(), "{found:?}");
    let rx = input("requires_pull = down;");
    let found = pull_diagnostics("tie-down", &[("rx", &rx)], &board("    component U1: p.In;\n    net GND { U1.IN; }"));
    assert!(found.is_empty(), "{found:?}");
    // A tie of the wrong polarity does not count.
    let found =
        pull_diagnostics("tie-wrong", &[("rx", &rx)], &board("    component U1: p.In;\n    net V3V3 { U1.IN; }"));
    only(&found, "MISSING_PULL", Severity::Error);
}

#[test]
fn an_internal_pull_does_not_satisfy_requires_pull() {
    let rx = input("requires_pull = up; internal_pull = \"up\";");
    let found = pull_diagnostics(
        "internal-not-enough",
        &[("rx", &rx), OUTPUT],
        &board("    component U1: p.In;\n    component U2: p.Out;\n    net SIG { U1.IN; U2.OUT; }"),
    );
    only(&found, "MISSING_PULL", Severity::Error);
}

#[test]
fn requires_pull_on_an_unconnected_pin_is_left_to_the_connection_checks() {
    let rx = input("requires_pull = up;");
    let found = pull_diagnostics("unconnected", &[("rx", &rx)], &board("    component U1: p.In;"));
    assert!(found.is_empty(), "{found:?}");
}

// UNDRIVEN_INPUT

#[test]
fn a_net_of_only_inputs_is_undriven() {
    let rx = input("");
    let found = pull_diagnostics(
        "undriven",
        &[("rx", &rx)],
        &board("    component U1: p.In;\n    component U2: p.In;\n    net SIG { U1.IN; U2.IN; }"),
    );
    let item = only(&found, "UNDRIVEN_INPUT", Severity::Error);
    assert_eq!(item.subject.as_deref(), Some("SIG"));
    for needle in ["U1.IN", "U2.IN"] {
        assert!(item.message.contains(needle), "{needle}: {}", item.message);
    }
}

#[test]
fn a_single_input_net_is_undriven() {
    let rx = input("");
    let found = pull_diagnostics("single", &[("rx", &rx)], &board("    component U1: p.In;\n    net SIG { U1.IN; }"));
    only(&found, "UNDRIVEN_INPUT", Severity::Warning);
}

#[test]
fn a_clock_input_is_checked_like_a_digital_one() {
    let clock = "part Clk {\n    pin CLK { number = \"1\"; domains = \"clock\"; directions = \"input\"; }\n}";
    let found =
        pull_diagnostics("clock", &[("clk", clock)], &board("    component U1: p.Clk;\n    net SIG { U1.CLK; }"));
    only(&found, "UNDRIVEN_INPUT", Severity::Warning);
}

#[test]
fn a_pull_resistor_or_a_supply_tie_drives_an_input_net() {
    let rx = input("");
    let up = pull_diagnostics(
        "undriven-up",
        &[("rx", &rx), RESISTOR],
        &board(
            "    component U1: p.In;\n    component R1: p.Res;\n    net V3V3 { R1.A; }\n    net SIG { U1.IN; R1.B; }",
        ),
    );
    assert!(up.is_empty(), "{up:?}");
    let down = pull_diagnostics(
        "undriven-down",
        &[("rx", &rx), RESISTOR],
        &board(
            "    component U1: p.In;\n    component R1: p.Res;\n    net GND { R1.A; }\n    net SIG { U1.IN; R1.B; }",
        ),
    );
    assert!(down.is_empty(), "{down:?}");
    let tied =
        pull_diagnostics("undriven-tie", &[("rx", &rx)], &board("    component U1: p.In;\n    net GND { U1.IN; }"));
    assert!(tied.is_empty(), "{tied:?}");
}

#[test]
fn an_internal_pull_makes_an_input_net_acceptable() {
    let rx = input("internal_pull = \"up,down\";");
    let found = pull_diagnostics("internal", &[("rx", &rx)], &board("    component U1: p.In;\n    net SIG { U1.IN; }"));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_input_driven_by_an_output_a_passive_pin_or_a_bidirectional_pin_is_clean() {
    let rx = input("");
    for (name, driver, part, pin) in [
        ("by-output", OUTPUT, "Out", "OUT"),
        ("by-passive", CONNECTOR, "Conn", "P1"),
        ("by-bidirectional", BIDIRECTIONAL, "Bidi", "IO"),
    ] {
        let found = pull_diagnostics(
            name,
            &[("rx", &rx), driver],
            &board(&format!(
                "    component U1: p.In;\n    component U2: p.{part};\n    net SIG {{ U1.IN; U2.{pin}; }}"
            )),
        );
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}

#[test]
fn an_unresolvable_pin_counts_as_a_driver() {
    let rx = input("");
    let design = compile_design_file(
        &project(
            "unresolved",
            &[("rx", &rx)],
            &board(
                "    component U1: p.In;
    net SIG { U1.IN; U1.NOPE; }",
            ),
        ),
        false,
        false,
    );
    // Compilation may reject the unknown pin; if it passes, the net must count as driven.
    if let Ok(design) = design {
        let found: Vec<_> = erc::check(&design.electrical, false)
            .expect("checks")
            .into_iter()
            .filter(|item| item.code == "UNDRIVEN_INPUT")
            .collect();
        assert!(found.is_empty(), "{found:?}");
    }
}

#[test]
fn analog_inputs_are_ignored() {
    let analog = "part Adc {\n    pin AIN { number = \"1\"; domains = \"analog\"; directions = \"input\"; }\n}";
    let found =
        pull_diagnostics("analog", &[("adc", analog)], &board("    component U1: p.Adc;\n    net SIG { U1.AIN; }"));
    assert!(found.is_empty(), "{found:?}");
}

// OPEN_DRAIN_NO_PULLUP

#[test]
fn open_drain_drivers_without_a_pull_up_are_a_warning() {
    let found = pull_diagnostics(
        "od-none",
        &[OPEN_DRAIN, ("rx", &input(""))],
        &board("    component U1: p.Od;\n    component U2: p.In;\n    net SIG { U1.IO; U2.IN; }"),
    );
    let item = only(&found, "OPEN_DRAIN_NO_PULLUP", Severity::Warning);
    assert_eq!(item.subject.as_deref(), Some("SIG"));
    assert!(item.message.contains("U1.IO"), "{}", item.message);
    assert!(item.hint.as_deref().is_some_and(|hint| hint.contains("pull-up")), "{item:?}");
}

#[test]
fn a_pull_up_resistor_satisfies_the_open_drain_check() {
    let found = pull_diagnostics(
        "od-resistor",
        &[OPEN_DRAIN, RESISTOR],
        &board(
            "    component U1: p.Od;\n    component R1: p.Res;\n    net V3V3 { R1.A; }\n    net SIG { U1.IO; R1.B; }",
        ),
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_pull_down_does_not_satisfy_the_open_drain_check() {
    let found = pull_diagnostics(
        "od-down",
        &[OPEN_DRAIN, RESISTOR],
        &board(
            "    component U1: p.Od;\n    component R1: p.Res;\n    net GND { R1.A; }\n    net SIG { U1.IO; R1.B; }",
        ),
    );
    only(&found, "OPEN_DRAIN_NO_PULLUP", Severity::Warning);
}

#[test]
fn an_internal_pull_up_satisfies_the_open_drain_check() {
    let rx = input("internal_pull = \"up\";");
    let found = pull_diagnostics(
        "od-internal",
        &[OPEN_DRAIN, ("rx", &rx)],
        &board("    component U1: p.Od;\n    component U2: p.In;\n    net SIG { U1.IO; U2.IN; }"),
    );
    assert!(found.is_empty(), "{found:?}");
    // An internal pull-down alone does not.
    let rx = input("internal_pull = \"down\";");
    let found = pull_diagnostics(
        "od-internal-down",
        &[OPEN_DRAIN, ("rx", &rx)],
        &board("    component U1: p.Od;\n    component U2: p.In;\n    net SIG { U1.IO; U2.IN; }"),
    );
    only(&found, "OPEN_DRAIN_NO_PULLUP", Severity::Warning);
}

#[test]
fn a_supply_tied_net_is_not_checked_for_a_pull_up() {
    let found = pull_diagnostics("od-tied", &[OPEN_DRAIN], &board("    component U1: p.Od;\n    net V3V3 { U1.IO; }"));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_push_pull_driver_on_the_net_means_no_pull_up_is_needed() {
    let found = pull_diagnostics(
        "od-and-push-pull",
        &[OPEN_DRAIN, OUTPUT],
        &board("    component U1: p.Od;\n    component U2: p.Out;\n    net SIG { U1.IO; U2.OUT; }"),
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_i2c_net_is_left_to_the_i2c_check() {
    let i2c = r#"part Dev {
    pin SDA { number = "1"; domains = "digital"; directions = "bidirectional"; drive_modes = "open_drain"; }
    pin SCL { number = "2"; domains = "digital"; directions = "bidirectional"; drive_modes = "open_drain"; }
}"#;
    let body = board(
        r#"    component U1: p.Dev;
    component U2: p.Dev;
    net I2C_SDA { U1.SDA; U2.SDA; }
    net I2C_SCL { U1.SCL; U2.SCL; }
    interface BUS: i2c {
        sda = I2C_SDA;
        scl = I2C_SCL;
        pullup = V3V3;
        bind U1 { sda = SDA; scl = SCL; }
        bind U2 { sda = SDA; scl = SCL; }
    }"#,
    );
    let found = all_diagnostics("i2c", &[("dev", i2c)], &body);
    assert!(found.iter().any(|item| item.code == "I2C_MISSING_PULLUP"), "{found:?}");
    assert!(found.iter().all(|item| item.code != "OPEN_DRAIN_NO_PULLUP"), "{found:?}");
}

#[test]
fn requires_pull_up_replaces_the_open_drain_warning() {
    let od = r#"part Od {
    pin IO { number = "1"; domains = "digital"; directions = "bidirectional"; drive_modes = "open_drain"; requires_pull = up; }
}"#;
    let found =
        pull_diagnostics("od-requires", &[("od", od)], &board("    component U1: p.Od;\n    net SIG { U1.IO; }"));
    only(&found, "MISSING_PULL", Severity::Error);
}

// Syntax and IR

#[test]
fn bad_pull_values_are_compile_errors() {
    for (name, properties) in [
        ("bad-requires", "requires_pull = sideways;"),
        ("bad-internal", "internal_pull = \"up,sideways\";"),
        ("bad-requires-number", "requires_pull = 5;"),
    ] {
        let message = compile_error(name, &[("rx", &input(properties))]);
        assert!(message.contains("CMP120") || message.contains("CMP086"), "{name}: {message}");
    }
    let message = compile_error("bad-requires-code", &[("rx", &input("requires_pull = sideways;"))]);
    assert!(message.contains("CMP120") && message.contains("sideways"), "{message}");
}

#[test]
fn pull_properties_on_device_pads_reach_the_package_pin() {
    let device = r#"device Dev {
    pad P { domains = "digital"; directions = "input"; requires_pull = up; internal_pull = "down"; }
}"#;
    let part = r#"part Pkg {
    footprint = "X";
    device = Dev;
    pin IN { number = "1"; bond = P; }
}"#;
    let found = pull_diagnostics(
        "pad",
        &[("dev", device), ("pkg", part)],
        &board(
            "    component U1: p.Pkg;
    net SIG { U1.IN; }",
        ),
    );
    // requires_pull comes from the pad; the internal pull-down keeps UNDRIVEN_INPUT quiet.
    only(&found, "MISSING_PULL", Severity::Error);
}

#[test]
fn declared_fields_reach_the_ir_and_absent_fields_do_not() {
    let rx = input("requires_pull = down; internal_pull = \"up,down\";");
    let design = compile_design_file(&project("ir", &[("rx", &rx)], "    component U1: p.In;"), false, false).unwrap();
    let part = design.electrical.library.values().next().expect("one part");
    let json = Dumps::compact().dumps(&part_value(part));
    assert!(json.contains("\"requires_pull\":\"down\""), "{json}");
    assert!(json.contains("\"internal_pull\":[\"down\",\"up\"]"), "{json}");

    let plain = input("");
    let design =
        compile_design_file(&project("ir-plain", &[("rx", &plain)], "    component U1: p.In;"), false, false).unwrap();
    let part = design.electrical.library.values().next().expect("one part");
    let json = Dumps::compact().dumps(&part_value(part));
    assert!(!json.contains("requires_pull") && !json.contains("internal_pull"), "{json}");
}

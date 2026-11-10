//! `COMPONENT.PREFIX*` net endpoints expand to the matching pins of the
//! component's part, in the part's pin order.

use std::fs;

use copper_lang::compiler::{Design, compile_design_file};
use copper_lang::model::Endpoint;

const PART: &str = "part Module {\n\
    pin VCC { number = \"1\"; domains = \"power\"; directions = \"input\"; }\n\
    pin GND { number = \"2\"; domains = \"power\"; directions = \"input\"; }\n\
    pin GND_7 { number = \"7\"; domains = \"power\"; directions = \"input\"; }\n\
    pin GND_3 { number = \"3\"; domains = \"power\"; directions = \"input\"; }\n\
    pin GNSS { number = \"4\"; domains = \"rf\"; directions = \"passive\"; }\n\
}\n";

/// Compile a board whose body is `body`, with `p.Module` importable.
fn compile_body(name: &str, body: &str) -> Result<Design, String> {
    let dir = std::env::temp_dir().join(format!("copper-pin-patterns-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    fs::write(dir.join("parts/module.copper"), PART).unwrap();
    fs::write(dir.join("board.copper"), format!("board B {{\n    import p \"./parts\";\n{body}\n}}\n")).unwrap();
    let result = compile_design_file(&dir.join("board.copper"), false, false).map_err(|error| error.to_string());
    let _ = fs::remove_dir_all(&dir);
    result
}

fn compile(name: &str, nets: &str) -> Result<Design, String> {
    compile_body(name, &format!("    component U1: p.Module;\n{nets}"))
}

fn net<'a>(design: &'a Design, name: &str) -> Vec<&'a Endpoint> {
    let net = design.electrical.nets.iter().find(|net| net.name == name).expect("the net");
    net.endpoints.iter().collect()
}

#[test]
fn a_prefix_pattern_expands_in_part_order_without_repeats() {
    let design = compile("order", "    net GND { U1.GND_3; U1.GND_*; }\n    net RF { U1.GNS*; }").expect("compiles");
    let pins: Vec<&str> = net(&design, "GND").iter().map(|endpoint| endpoint.pin.as_str()).collect();
    assert_eq!(pins, ["GND_3", "GND_7"]);
    let pins: Vec<&str> = net(&design, "RF").iter().map(|endpoint| endpoint.pin.as_str()).collect();
    assert_eq!(pins, ["GNSS"]);
}

#[test]
fn a_pattern_may_name_a_component_declared_below_the_net() {
    let design = compile_body("below", "    net GND { U1.GN*; }\n    component U1: p.Module;").expect("compiles");
    let pins: Vec<&str> = net(&design, "GND").iter().map(|endpoint| endpoint.pin.as_str()).collect();
    assert_eq!(pins, ["GND", "GND_7", "GND_3", "GNSS"]);
}

#[test]
fn a_pattern_that_matches_nothing_or_names_no_part_is_an_error() {
    let message = compile("none", "    net N { U1.VDD*; }").unwrap_err();
    assert!(message.contains("CMP211") && message.contains("matches no pin of part"), "{message}");
    let message = compile("unknown", "    net N { U9.GND*; }").unwrap_err();
    assert!(message.contains("CMP212"), "{message}");
}

#[test]
fn the_star_must_follow_its_prefix() {
    let message = compile("spaced", "    net N { U1.GND *; }").unwrap_err();
    assert!(message.contains("PAR017"), "{message}");
}

//! Signal sources of the EMI model (P1 of `docs/emi-signal-cable-plan.md`): the pin attributes
//! (`data_rate`, `clock_frequency`, `signal_swing`, `spread_spectrum`, `passes_to`), the IR, `EmiModel::signals`
//! and the channel walk from a driver to the first cable connector.

use std::fs;
use std::path::PathBuf;

use copper_lang::compiler::{Design, compile_design_file};
use copper_lang::elaborate::elaborate;
use copper_lang::emi::{self, AggressorClass};
use copper_lang::emi_signals::{ChannelEnd, ChannelStop, HopKind, SignalKind, SignalSource, StopReason};
use copper_lang::serializer::{design_json, part_value};

/// A project with parts (one per file) and a board.
fn project(name: &str, parts: &str, board_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-emi-signals-{name}-{}", std::process::id()));
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

fn model(design: &Design) -> emi::EmiModel {
    emi::analyze(&elaborate(&design.electrical).expect("elaborates"))
}

fn expect_error(name: &str, parts: &str, board: &str, code: &str) {
    match compile(name, parts, board) {
        Ok(_) => panic!("{name}: expected {code}"),
        Err(failure) => assert!(failure.contains(code), "{name}: expected {code}, got {failure}"),
    }
}

const PARTS: &str = r#"
part Phy {
    category = "interface.serdes";
    pin TXP { number = "1"; domains = "digital"; directions = "output"; data_rate = 2.5Gbps; rise_time = 0.1ns; fall_time = 0.12ns; signal_swing = 400mV; }
    pin TXN { number = "2"; domains = "digital"; directions = "output"; data_rate = 2.5Gbps; rise_time = 0.1ns; fall_time = 0.12ns; }
    pin GND { number = "3"; domains = "ground"; directions = "input"; }
}
part Osc {
    category = "clock.oscillator";
    pin OUT { number = "1"; domains = "clock"; directions = "output"; clock_frequency = 24MHz; rise_time = 1ns; spread_spectrum = 0.5%; signal_swing = 3.3V; }
    pin GND { number = "2"; domains = "ground"; directions = "input"; }
}
part RateOnly {
    category = "interface.serdes";
    pin TX { number = "1"; domains = "digital"; directions = "output"; data_rate = 100Mbps; }
}
part Cap {
    category = "passive.capacitor";
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Res {
    category = "passive.resistor";
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Ferrite {
    category = "passive.ferrite";
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Choke {
    category = "passive.inductor.common_mode";
    pin IN1 { number = "1"; domains = "digital"; directions = "passive"; passes_to = OUT1; }
    pin IN2 { number = "2"; domains = "digital"; directions = "passive"; passes_to = OUT2; }
    pin OUT1 { number = "4"; domains = "digital"; directions = "passive"; }
    pin OUT2 { number = "3"; domains = "digital"; directions = "passive"; }
}
part BareChoke {
    category = "passive.inductor.common_mode";
    pin IN1 { number = "1"; domains = "digital"; directions = "passive"; }
    pin IN2 { number = "2"; domains = "digital"; directions = "passive"; }
    pin OUT1 { number = "4"; domains = "digital"; directions = "passive"; }
    pin OUT2 { number = "3"; domains = "digital"; directions = "passive"; }
}
part Inductor {
    category = "passive.inductor";
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
part Conn {
    category = "connector.header";
    external_cable = true;
    pin A { number = "1"; domains = "digital"; directions = "passive"; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
    pin C { number = "3"; domains = "digital"; directions = "passive"; }
    pin D { number = "4"; domains = "digital"; directions = "passive"; }
}
"#;

const PAIR_RULES: &str = r#"
    constraint routing(TX_P) { kind = differential; partner = TX_N; }
    constraint routing(TX_N) { kind = differential; partner = TX_P; }
"#;

/// A differential driver wired straight to a cable connector.
fn direct() -> String {
    format!(
        "    component U1: p.Phy;\n    component J1: p.Conn;\n    net TX_P {{ U1.TXP; J1.A; }}\n    net TX_N {{ U1.TXN; J1.B; }}\n    net GND {{ U1.GND; }}\n{PAIR_RULES}"
    )
}

fn only(found: &emi::EmiModel) -> &SignalSource {
    assert_eq!(found.signals.len(), 1, "{:?}", found.signals.keys().collect::<Vec<_>>());
    found.signals.values().next().unwrap()
}

fn parts_of(source: &SignalSource, net: &str) -> Vec<(String, HopKind)> {
    source.member(net).unwrap().channel.iter().map(|hop| (hop.component.clone(), hop.kind)).collect()
}

fn connector_of(source: &SignalSource, net: &str) -> Option<(String, String)> {
    match &source.member(net).unwrap().end {
        ChannelEnd::Connector { component, pin, .. } => Some((component.clone(), pin.clone())),
        ChannelEnd::Open { .. } => None,
    }
}

// ----------------------------------------------------------------------------------------------
// Language
// ----------------------------------------------------------------------------------------------

#[test]
fn signal_attributes_are_accepted_and_reach_the_ir_only_when_declared() {
    let compiled = design("accept", PARTS, &direct());
    let phy = compiled.electrical.library.values().find(|part| part.name.ends_with("Phy")).unwrap();
    let emi = phy.pins["TXP"].profile.as_ref().unwrap().emi.clone().unwrap();
    assert_eq!(emi.data_rate.as_ref().map(ToString::to_string).as_deref(), Some("2.5 Gbps"));
    assert_eq!(emi.signal_swing.as_ref().map(ToString::to_string).as_deref(), Some("400 mV"));
    assert!(emi.clock_frequency.is_none() && emi.spread_spectrum.is_none());
    let osc = compiled.electrical.library.values().find(|part| part.name.ends_with("Osc")).unwrap();
    let emi = osc.pins["OUT"].profile.as_ref().unwrap().emi.clone().unwrap();
    assert_eq!(emi.clock_frequency.as_ref().map(ToString::to_string).as_deref(), Some("24 MHz"));
    assert_eq!(emi.spread_spectrum.as_ref().map(ToString::to_string).as_deref(), Some("0.5 %"));
    assert_eq!(emi.spread_spectrum.as_ref().unwrap().base_value().decimal_text(), "0.005");

    let json = part_value(phy);
    let pins = json.get("pins").and_then(|pins| pins.as_array()).unwrap();
    let emi_of = |name: &str| {
        let pin = pins.iter().find(|pin| pin.get("name").and_then(|v| v.as_str()) == Some(name)).unwrap();
        pin.get("profile").and_then(|profile| profile.get("emi")).cloned()
    };
    let transmit = emi_of("TXP").expect("emi object");
    assert_eq!(transmit.get("data_rate").and_then(|v| v.get("unit")).and_then(|v| v.as_str()), Some("Gbps"));
    assert_eq!(
        transmit.get("data_rate").and_then(|v| v.get("base_value")).and_then(|v| v.as_str()),
        Some("2500000000")
    );
    assert_eq!(transmit.get("signal_swing").and_then(|v| v.get("unit")).and_then(|v| v.as_str()), Some("mV"));
    assert!(transmit.get("clock_frequency").is_none() && transmit.get("spread_spectrum").is_none());
    assert!(emi_of("TXN").unwrap().get("signal_swing").is_none(), "undeclared keys are absent");
    assert!(emi_of("GND").is_none());
    let dump = |value: &copper_json::Value| copper_json::Dumps::compact().sort_keys(true).dumps(value);
    let text = dump(&part_value(osc));
    assert!(text.contains("spread_spectrum") && text.contains("clock_frequency"));
    // No pin declares `passes_to`: the key is absent from every pin.
    assert!(!dump(&json).contains("passes_to"));
    assert!(!text.contains("passes_to"));
}

#[test]
fn passes_to_reaches_the_ir_only_on_the_pins_that_declare_it() {
    let compiled = design("passes", PARTS, &format!("{}    component L1: p.Choke;\n", direct()));
    let choke = compiled.electrical.library.values().find(|part| part.name.ends_with("Choke")).unwrap();
    assert_eq!(choke.pins["IN1"].passes_to.as_deref(), Some("OUT1"));
    assert_eq!(choke.pins["OUT1"].passes_to, None);
    let json = part_value(choke);
    let pins = json.get("pins").and_then(|pins| pins.as_array()).unwrap();
    let key = |name: &str| {
        let pin = pins.iter().find(|pin| pin.get("name").and_then(|v| v.as_str()) == Some(name)).unwrap();
        pin.get("passes_to").and_then(|v| v.as_str()).map(str::to_owned)
    };
    assert_eq!(key("IN1").as_deref(), Some("OUT1"));
    assert_eq!(key("IN2").as_deref(), Some("OUT2"));
    assert_eq!(key("OUT1"), None);
}

#[test]
fn unannotated_design_ir_has_none_of_the_new_keys() {
    let parts = r#"
part Chip {
    category = "component.generic";
    pin A { number = "1"; domains = "digital"; directions = "output"; rise_time = 1ns; }
    pin B { number = "2"; domains = "digital"; directions = "input"; }
    pin C { number = "3"; domains = "digital"; directions = "input"; }
}
"#;
    let compiled = design("plain", parts, "    component U1: p.Chip;\n    net X { U1.A; U1.B; }\n");
    let json = design_json(&compiled);
    for key in ["data_rate", "clock_frequency", "signal_swing", "spread_spectrum", "passes_to"] {
        assert!(!json.contains(key), "{key} in {json}");
    }
}

#[test]
fn signal_attribute_errors_are_cmp130() {
    for (name, from, to) in [
        ("rate-time", "data_rate = 2.5Gbps;", "data_rate = 2.5ns;"),
        ("rate-frequency", "data_rate = 2.5Gbps;", "data_rate = 2.5GHz;"),
        ("rate-zero", "data_rate = 2.5Gbps;", "data_rate = 0Gbps;"),
        ("rate-negative", "data_rate = 2.5Gbps;", "data_rate = -1Mbps;"),
        ("rate-bare", "data_rate = 2.5Gbps;", "data_rate = 2500;"),
        ("rate-text", "data_rate = 2.5Gbps;", "data_rate = fast;"),
        ("swing-length", "signal_swing = 400mV;", "signal_swing = 400mm;"),
        ("swing-zero", "signal_swing = 400mV;", "signal_swing = 0V;"),
        ("swing-bare", "signal_swing = 400mV;", "signal_swing = 400;"),
    ] {
        expect_error(name, &PARTS.replace(from, to), &direct(), "CMP130");
    }
    for (name, from, to) in [
        ("clock-rate", "clock_frequency = 24MHz;", "clock_frequency = 24Mbps;"),
        ("clock-zero", "clock_frequency = 24MHz;", "clock_frequency = 0Hz;"),
        ("clock-time", "clock_frequency = 24MHz;", "clock_frequency = 24ns;"),
        ("spread-time", "spread_spectrum = 0.5%;", "spread_spectrum = 0.5ns;"),
        ("spread-zero", "spread_spectrum = 0.5%;", "spread_spectrum = 0%;"),
        ("spread-bare", "spread_spectrum = 0.5%;", "spread_spectrum = 0.5;"),
        ("spread-negative", "spread_spectrum = 0.5%;", "spread_spectrum = -0.5%;"),
    ] {
        expect_error(name, &PARTS.replace(from, to), &direct(), "CMP130");
    }
    // `%` is a unit only of a ratio, and only when it follows the number directly.
    expect_error("percent-time", &PARTS.replace("rise_time = 1ns;", "rise_time = 5%;"), &direct(), "CMP130");
    expect_error("percent-rate", &PARTS.replace("data_rate = 2.5Gbps;", "data_rate = 2.5%;"), &direct(), "CMP130");
}

#[test]
fn signal_attributes_reach_the_package_pin_from_a_device_pad() {
    let parts = r#"
device DevPhy {
    pad TX { domains = "digital"; directions = "output"; data_rate = 5Gbps; signal_swing = 0.8V; rise_time = 0.05ns; spread_spectrum = 0.5%; clock_frequency = 100MHz; }
}
part Packaged {
    category = "interface.serdes";
    device = DevPhy;
    pin TX { number = "1"; bond = TX; }
}
"#;
    let compiled = design("pad", parts, "    component U1: p.Packaged;\n    net X { U1.TX; }\n");
    let found = model(&compiled);
    let source = only(&found);
    assert_eq!(source.data_rate.as_ref().map(|n| n.value.to_string()).as_deref(), Some("5 Gbps"));
    assert_eq!(source.swing.as_ref().map(|n| n.value.to_string()).as_deref(), Some("0.8 V"));
    assert_eq!(source.pins.iter().map(ToString::to_string).collect::<Vec<_>>(), ["U1.TX"]);
    expect_error(
        "pad-rate",
        &parts.replace("data_rate = 5Gbps;", "data_rate = 5ns;"),
        "    component U1: p.Packaged;\n",
        "CMP130",
    );
}

#[test]
fn passes_to_errors_are_cmp130() {
    let board = "    component L1: p.Choke;\n";
    for (name, from, to) in [
        ("unknown", "passes_to = OUT1;", "passes_to = NOPE;"),
        (
            "itself",
            "pin IN1 { number = \"1\"; domains = \"digital\"; directions = \"passive\"; passes_to = OUT1; }",
            "pin IN1 { number = \"1\"; domains = \"digital\"; directions = \"passive\"; passes_to = IN1; }",
        ),
        ("number", "passes_to = OUT1;", "passes_to = 3;"),
        // IN1 names OUT1, but OUT1 names IN2: the pair disagrees in its two directions.
        (
            "inconsistent",
            "pin OUT1 { number = \"4\"; domains = \"digital\"; directions = \"passive\"; }",
            "pin OUT1 { number = \"4\"; domains = \"digital\"; directions = \"passive\"; passes_to = IN2; }",
        ),
        // IN1 and IN2 both name OUT1.
        ("shared-target", "passes_to = OUT2;", "passes_to = OUT1;"),
    ] {
        // Both parts of a source are replaced only in the first (Choke) declaration.
        let source = PARTS.replacen(from, to, 1);
        assert_ne!(source, PARTS, "{name}");
        expect_error(name, &source, board, "CMP130");
    }
    // A part with two pins is not a "more than two pins" part.
    let two = r#"
part Two {
    category = "passive.resistor";
    pin A { number = "1"; domains = "digital"; directions = "passive"; passes_to = B; }
    pin B { number = "2"; domains = "digital"; directions = "passive"; }
}
"#;
    expect_error("two-pins", two, "    component R1: p.Two;\n", "CMP130");
    // Agreeing in both directions is accepted.
    let both = PARTS.replacen(
        "pin OUT1 { number = \"4\"; domains = \"digital\"; directions = \"passive\"; }",
        "pin OUT1 { number = \"4\"; domains = \"digital\"; directions = \"passive\"; passes_to = IN1; }",
        1,
    );
    design("agree", &both, board);
    // `passes_to` is a pin attribute only: not a device pad attribute.
    let pad = "device D { pad X { domains = \"digital\"; directions = \"output\"; passes_to = Y; } }\npart P { category = \"x\"; device = D; pin X { number = \"1\"; bond = X; } }\n";
    assert!(compile("pad-passes", pad, "    component U1: p.P;\n").is_err());
}

// ----------------------------------------------------------------------------------------------
// Signal sources
// ----------------------------------------------------------------------------------------------

#[test]
fn a_differential_pair_is_one_source_keyed_by_its_p_net() {
    let found = model(&design("pair", PARTS, &direct()));
    let source = only(&found);
    assert_eq!(source.net, "TX_P");
    assert_eq!(source.kind, SignalKind::Differential);
    assert_eq!(source.partner.as_deref(), Some("TX_N"));
    assert!(source.fast_edge);
    assert_eq!(found.nets["TX_P"].aggressor, Some(AggressorClass::FastEdge));
    assert_eq!(source.pins.iter().map(ToString::to_string).collect::<Vec<_>>(), ["U1.TXP", "U1.TXN"]);
    // Numbers come from the first driving pin that declares them, with that pin as provenance.
    let number = |value: &Option<copper_lang::emi_signals::SignalNumber>| {
        value.as_ref().map(|number| (number.value.to_string(), number.from.to_string()))
    };
    assert_eq!(number(&source.data_rate), Some(("2.5 Gbps".to_owned(), "U1.TXP".to_owned())));
    assert_eq!(number(&source.rise_time), Some(("0.1 ns".to_owned(), "U1.TXP".to_owned())));
    assert_eq!(number(&source.fall_time), Some(("0.12 ns".to_owned(), "U1.TXP".to_owned())));
    assert_eq!(number(&source.swing), Some(("400 mV".to_owned(), "U1.TXP".to_owned())));
    assert!(source.clock_frequency.is_none() && source.spread_spectrum.is_none());
    assert_eq!(source.members.iter().map(|member| member.net.as_str()).collect::<Vec<_>>(), ["TX_P", "TX_N"]);
}

#[test]
fn the_pair_key_does_not_depend_on_which_net_declares_the_pair() {
    // Only the N net carries the routing rule.
    let body = direct().replace("    constraint routing(TX_P) { kind = differential; partner = TX_N; }\n", "");
    let found = model(&design("pair-n-only", PARTS, &body));
    assert_eq!(only(&found).net, "TX_P");
    assert_eq!(only(&found).partner.as_deref(), Some("TX_N"));
    // `DP`/`DM` names: the pair is keyed by the DP net although `DM` sorts first.
    let renamed = direct().replace("TX_P", "USB_DP").replace("TX_N", "USB_DM");
    let found = model(&design("pair-usb", PARTS, &renamed));
    assert_eq!(only(&found).net, "USB_DP");
    assert_eq!(only(&found).partner.as_deref(), Some("USB_DM"));
    // Names that do not say fall back to the first in name order.
    let neutral = direct().replace("TX_P", "LANE_A").replace("TX_N", "LANE_B");
    let found = model(&design("pair-neutral", PARTS, &neutral));
    assert_eq!(only(&found).net, "LANE_A");
    // A partner that names a different net is not a pair.
    let broken = direct().replace(
        "    constraint routing(TX_N) { kind = differential; partner = TX_P; }\n",
        "    constraint routing(TX_N) { kind = differential; partner = GND; }\n",
    );
    let found = model(&design("pair-mismatch", PARTS, &broken));
    assert_eq!(found.signals.len(), 2, "two single-ended sources");
    assert!(found.signals.values().all(|source| source.kind == SignalKind::SingleEnded));
}

#[test]
fn a_clock_is_a_single_ended_source() {
    let board = "    component X1: p.Osc;\n    component R1: p.Res { value = 33ohm; }\n    component J1: p.Conn;\n    net CLK { X1.OUT; R1.A; }\n    net CLK_OUT { R1.B; J1.A; }\n    net GND { X1.GND; }\n";
    let found = model(&design("clock", PARTS, board));
    let source = only(&found);
    assert_eq!((source.net.as_str(), source.kind, source.partner.as_deref()), ("CLK", SignalKind::SingleEnded, None));
    assert_eq!(source.clock_frequency.as_ref().map(|n| n.value.to_string()).as_deref(), Some("24 MHz"));
    assert_eq!(source.spread_spectrum.as_ref().map(|n| n.value.to_string()).as_deref(), Some("0.5 %"));
    assert_eq!(source.rise_time.as_ref().map(|n| n.value.to_string()).as_deref(), Some("1 ns"));
    // A pin that declares only `rise_time` gives the fall time the same value.
    assert_eq!(source.fall_time.as_ref().map(|n| n.value.to_string()).as_deref(), Some("1 ns"));
    assert_eq!(source.swing.as_ref().map(|n| n.value.to_string()).as_deref(), Some("3.3 V"));
    assert!(source.data_rate.is_none());
    assert_eq!(parts_of(source, "CLK"), [("R1".to_owned(), HopKind::SeriesPart)]);
    assert_eq!(connector_of(source, "CLK"), Some(("J1".to_owned(), "A".to_owned())));
}

#[test]
fn a_clock_routing_rule_makes_a_source_with_no_numbers() {
    let parts = r#"
part Gate {
    category = "component.generic";
    pin Y { number = "1"; domains = "digital"; directions = "output"; }
}
"#;
    let board = "    component U1: p.Gate;\n    net CK { U1.Y; }\n    constraint routing(CK) { kind = clock; }\n";
    let found = model(&design("clock-rule", parts, board));
    let source = only(&found);
    assert!(source.fast_edge && source.pins.is_empty());
    assert!(source.rise_time.is_none() && source.data_rate.is_none() && source.swing.is_none());
    assert!(matches!(source.members[0].end, ChannelEnd::Open { .. }));
}

#[test]
fn a_rate_without_an_edge_is_a_source_that_is_not_a_fast_edge_net() {
    let board = "    component U1: p.RateOnly;\n    net X { U1.TX; }\n";
    let found = model(&design("rate-only", PARTS, board));
    let source = only(&found);
    assert!(!source.fast_edge && !found.nets.contains_key("X"));
    assert_eq!(source.data_rate.as_ref().map(|n| n.value.to_string()).as_deref(), Some("100 Mbps"));
    assert!(source.rise_time.is_none() && source.fall_time.is_none());
}

#[test]
fn the_query_lists_signals_only_when_there_are_any() {
    let with = design("query-with", PARTS, &direct());
    let result = copper_lang::query::run_query(elaborate(&with.electrical).unwrap(), "emi", None).expect("query");
    let signals = result.get("signals").and_then(|v| v.as_array()).expect("signals");
    assert_eq!(signals.len(), 1);
    let first = &signals[0];
    assert_eq!(first.get("net").and_then(|v| v.as_str()), Some("TX_P"));
    assert_eq!(first.get("partner").and_then(|v| v.as_str()), Some("TX_N"));
    assert_eq!(first.get("data_rate").and_then(|v| v.get("declared_on")).and_then(|v| v.as_str()), Some("U1.TXP"));
    let members = first.get("members").and_then(|v| v.as_array()).unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].get("end").and_then(|v| v.get("kind")).and_then(|v| v.as_str()), Some("connector"));
    let text = copper_lang::query::render_query(&result);
    assert!(text.contains("\"signals\"") && text.contains("\"declared_on\": \"U1.TXP\""), "{text}");

    // A design without signals: the key is absent.
    let parts = "part Cap {\n    category = \"passive.capacitor\";\n    pin A { number = \"1\"; domains = \"power\"; directions = \"passive\"; }\n    pin B { number = \"2\"; domains = \"power\"; directions = \"passive\"; }\n}\n";
    let plain = design("query-without", parts, "    component C1: p.Cap;\n    net X { C1.A; }\n");
    let result = copper_lang::query::run_query(elaborate(&plain.electrical).unwrap(), "emi", None).unwrap();
    assert!(result.get("signals").is_none());
    assert!(!copper_lang::query::render_query(&result).contains("signals"));
}

// ----------------------------------------------------------------------------------------------
// Channel walk
// ----------------------------------------------------------------------------------------------

/// A pair through the given series parts of each leg, ending at the connector pins A and B.
fn coupled(legs: &str) -> String {
    format!("    component U1: p.Phy;\n    component J1: p.Conn;\n{legs}    net GND {{ U1.GND; }}\n{PAIR_RULES}")
}

#[test]
fn direct_connection_has_an_empty_channel_that_ends_at_the_connector() {
    let found = model(&design("direct", PARTS, &direct()));
    let source = only(&found);
    for (net, pin) in [("TX_P", "A"), ("TX_N", "B")] {
        assert!(source.member(net).unwrap().channel.is_empty());
        assert_eq!(connector_of(source, net), Some(("J1".to_owned(), pin.to_owned())));
    }
    // The existing net classification reaches the connector the same way.
    assert_eq!(found.nets["TX_P"].external, ["J1"]);
}

#[test]
fn one_and_two_coupling_capacitors_are_hops() {
    let one = coupled(
        "    component C1: p.Cap { value = 100nF; }\n    component C2: p.Cap { value = 100nF; }\n    net TX_P { U1.TXP; C1.A; }\n    net TX_N { U1.TXN; C2.A; }\n    net LINK_P { C1.B; J1.A; }\n    net LINK_N { C2.B; J1.B; }\n",
    );
    let found = model(&design("one-cap", PARTS, &one));
    let source = only(&found);
    assert_eq!(parts_of(source, "TX_P"), [("C1".to_owned(), HopKind::CouplingCapacitor)]);
    assert_eq!(parts_of(source, "TX_N"), [("C2".to_owned(), HopKind::CouplingCapacitor)]);
    let hop = &source.member("TX_P").unwrap().channel[0];
    assert_eq!(
        (hop.from_net.as_str(), hop.to_net.as_str(), hop.from_pin.as_str(), hop.to_pin.as_str()),
        ("TX_P", "LINK_P", "A", "B")
    );
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
    assert_eq!(connector_of(source, "TX_N"), Some(("J1".to_owned(), "B".to_owned())));
    // The existing discovery (one resistor or ferrite only) does not see through a capacitor.
    assert!(found.nets["TX_P"].external.is_empty());

    let two = coupled(
        "    component C1: p.Cap { value = 100nF; }\n    component C2: p.Cap { value = 100nF; }\n    component C3: p.Cap { value = 100nF; }\n    component C4: p.Cap { value = 100nF; }\n    net TX_P { U1.TXP; C1.A; }\n    net TX_N { U1.TXN; C2.A; }\n    net MID_P { C1.B; C3.A; }\n    net MID_N { C2.B; C4.A; }\n    net LINK_P { C3.B; J1.A; }\n    net LINK_N { C4.B; J1.B; }\n",
    );
    let found = model(&design("two-caps", PARTS, &two));
    let source = only(&found);
    let components: Vec<String> =
        source.member("TX_P").unwrap().channel.iter().map(|hop| hop.component.clone()).collect();
    assert_eq!(components, ["C1", "C3"], "ordered from the driver");
    assert_eq!(connector_of(source, "TX_N"), Some(("J1".to_owned(), "B".to_owned())));
}

#[test]
fn resistors_ferrites_and_capacitors_chain_in_any_order() {
    let board = coupled(
        "    component R1: p.Res { value = 22ohm; }\n    component F1: p.Ferrite;\n    component C1: p.Cap { value = 100nF; }\n    component R2: p.Res { value = 22ohm; }\n    component C2: p.Cap { value = 100nF; }\n    net TX_P { U1.TXP; R1.A; }\n    net TX_N { U1.TXN; R2.A; }\n    net A1 { R1.B; F1.A; }\n    net A2 { F1.B; C1.A; }\n    net B1 { R2.B; C2.A; }\n    net LINK_P { C1.B; J1.A; }\n    net LINK_N { C2.B; J1.B; }\n",
    );
    let found = model(&design("chain", PARTS, &board));
    let source = only(&found);
    assert_eq!(
        parts_of(source, "TX_P"),
        [
            ("R1".to_owned(), HopKind::SeriesPart),
            ("F1".to_owned(), HopKind::SeriesPart),
            ("C1".to_owned(), HopKind::CouplingCapacitor)
        ]
    );
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
    assert_eq!(connector_of(source, "TX_N"), Some(("J1".to_owned(), "B".to_owned())));
}

fn choke_board(choke_part: &str, wiring: &str) -> String {
    coupled(&format!("    component L1: p.{choke_part};\n{wiring}"))
}

const CHOKE_WIRING: &str = "    net TX_P { U1.TXP; L1.IN1; }\n    net TX_N { U1.TXN; L1.IN2; }\n    net LINK_P { L1.OUT1; J1.A; }\n    net LINK_N { L1.OUT2; J1.B; }\n";

#[test]
fn a_four_pin_choke_is_crossed_along_its_declared_pairs() {
    let found = model(&design("choke", PARTS, &choke_board("Choke", CHOKE_WIRING)));
    let source = only(&found);
    assert_eq!(parts_of(source, "TX_P"), [("L1".to_owned(), HopKind::DeclaredPair)]);
    let (p, n) = (&source.member("TX_P").unwrap().channel[0], &source.member("TX_N").unwrap().channel[0]);
    assert_eq!((p.from_pin.as_str(), p.to_pin.as_str(), p.to_net.as_str()), ("IN1", "OUT1", "LINK_P"));
    assert_eq!((n.from_pin.as_str(), n.to_pin.as_str(), n.to_net.as_str()), ("IN2", "OUT2", "LINK_N"));
    assert!(!p.swaps_polarity && !n.swaps_polarity);
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
    assert_eq!(connector_of(source, "TX_N"), Some(("J1".to_owned(), "B".to_owned())));
}

#[test]
fn a_pair_declared_on_one_side_works_from_either_direction() {
    // Walk from the connector side's point of view: the same part, entered at OUT1, passes to IN1.
    let board = coupled(
        "    component L1: p.Choke;\n    net TX_P { U1.TXP; L1.OUT1; }\n    net TX_N { U1.TXN; L1.OUT2; }\n    net LINK_P { L1.IN1; J1.A; }\n    net LINK_N { L1.IN2; J1.B; }\n",
    );
    let found = model(&design("choke-reverse", PARTS, &board));
    let source = only(&found);
    let hop = &source.member("TX_P").unwrap().channel[0];
    assert_eq!((hop.from_pin.as_str(), hop.to_pin.as_str()), ("OUT1", "IN1"));
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
}

#[test]
fn without_passes_to_the_walk_stops_at_the_choke_and_says_where_and_why() {
    let found = model(&design("bare-choke", PARTS, &choke_board("BareChoke", CHOKE_WIRING)));
    let source = only(&found);
    for (net, pin) in [("TX_P", "IN1"), ("TX_N", "IN2")] {
        let member = source.member(net).unwrap();
        assert!(member.channel.is_empty(), "no hop is invented");
        let expected = ChannelStop {
            at: copper_lang::model::Endpoint::new("L1", pin),
            net: net.to_owned(),
            reason: StopReason::NoPassesTo,
        };
        match &member.end {
            ChannelEnd::Open { stops } => assert_eq!(stops, &[expected], "{net}"),
            other => panic!("{net}: {other:?}"),
        }
    }
}

#[test]
fn coupling_capacitors_and_a_choke_combine_in_order() {
    let board = coupled(
        "    component C1: p.Cap { value = 100nF; }\n    component C2: p.Cap { value = 100nF; }\n    component L1: p.Choke;\n    component R1: p.Res { value = 0ohm; }\n    component R2: p.Res { value = 0ohm; }\n    net TX_P { U1.TXP; C1.A; }\n    net TX_N { U1.TXN; C2.A; }\n    net AC_P { C1.B; L1.IN1; }\n    net AC_N { C2.B; L1.IN2; }\n    net CM_P { L1.OUT1; R1.A; }\n    net CM_N { L1.OUT2; R2.A; }\n    net LINK_P { R1.B; J1.A; }\n    net LINK_N { R2.B; J1.B; }\n",
    );
    let found = model(&design("combined", PARTS, &board));
    let source = only(&found);
    assert_eq!(
        parts_of(source, "TX_P"),
        [
            ("C1".to_owned(), HopKind::CouplingCapacitor),
            ("L1".to_owned(), HopKind::DeclaredPair),
            ("R1".to_owned(), HopKind::SeriesPart)
        ]
    );
    assert_eq!(
        parts_of(source, "TX_N"),
        [
            ("C2".to_owned(), HopKind::CouplingCapacitor),
            ("L1".to_owned(), HopKind::DeclaredPair),
            ("R2".to_owned(), HopKind::SeriesPart)
        ]
    );
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
    assert_eq!(connector_of(source, "TX_N"), Some(("J1".to_owned(), "B".to_owned())));
}

#[test]
fn p_and_n_identity_is_kept_and_a_part_that_swaps_them_is_reported() {
    // Wired straight: P stays P through every part.
    let found = model(&design("identity", PARTS, &choke_board("Choke", CHOKE_WIRING)));
    let source = only(&found);
    assert!(source.members.iter().flat_map(|member| &member.channel).all(|hop| !hop.swaps_polarity));
    // The choke leaves the P member on the net named for N and the N member on P's: reported on both members.
    let swapped = "    net TX_P { U1.TXP; L1.IN1; }\n    net TX_N { U1.TXN; L1.IN2; }\n    net LINK_N { L1.OUT1; J1.A; }\n    net LINK_P { L1.OUT2; J1.B; }\n";
    let found = model(&design("swap", PARTS, &choke_board("Choke", swapped)));
    let source = only(&found);
    assert_eq!(source.net, "TX_P", "the pair is still keyed by the P net");
    for net in ["TX_P", "TX_N"] {
        let hops = &source.member(net).unwrap().channel;
        assert_eq!(hops.len(), 1);
        assert!(hops[0].swaps_polarity, "{net}");
    }
    // The P member still reaches the connector pin its net is wired to; the walk does not "correct" the swap.
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
    // Net names that do not say P from N report nothing.
    let neutral = swapped.replace("LINK_N", "LINK_X").replace("LINK_P", "LINK_Y");
    let found = model(&design("swap-neutral", PARTS, &choke_board("Choke", &neutral)));
    assert!(only(&found).members.iter().flat_map(|member| &member.channel).all(|hop| !hop.swaps_polarity));
}

#[test]
fn the_walk_never_enters_a_declared_supply_rail() {
    // The only way to the connector is through a resistor into a net that is a declared supply.
    let board = "    component U1: p.Phy;\n    component J1: p.Conn;\n    component R1: p.Res { value = 10kohm; }\n    supply VDD { net = VDD; voltage = 3.3V; }\n    net TX_P { U1.TXP; R1.A; }\n    net VDD { R1.B; J1.A; }\n    net GND { U1.GND; }\n    net TX_N { U1.TXN; }\n";
    let found = model(&design("supply", PARTS, board));
    let source = found.signals.get("TX_P").expect("TX_P");
    let member = source.member("TX_P").unwrap();
    assert!(member.channel.is_empty());
    match &member.end {
        ChannelEnd::Open { stops } => {
            assert_eq!(stops.len(), 1, "{stops:?}");
            assert_eq!(stops[0].reason, StopReason::SupplyRail);
            assert_eq!(stops[0].at.to_string(), "R1.A");
        }
        other => panic!("{other:?}"),
    }
    // The existing discovery also refuses to enter the supply, so the nets agree.
    assert!(found.nets["TX_P"].external.is_empty());
}

#[test]
fn a_capacitor_to_a_reference_is_a_shunt_not_a_hop() {
    // C1 shunts TX_P to GND and the connector's ground pin sits on GND: there is no series path to the cable.
    let board = "    component U1: p.Phy;\n    component J1: p.Conn;\n    component C1: p.Cap { value = 10pF; }\n    net TX_P { U1.TXP; C1.A; }\n    net TX_N { U1.TXN; }\n    net GND { U1.GND; C1.B; J1.C; }\n";
    let found = model(&design("shunt", PARTS, board));
    let source = found.signals.get("TX_P").expect("TX_P");
    let member = source.member("TX_P").unwrap();
    assert!(member.channel.is_empty());
    assert_eq!(member.end, ChannelEnd::Open { stops: Vec::new() });

    // A resistor into a net that carries a ground pin is refused and says so.
    let resistor = board.replace("component C1: p.Cap { value = 10pF; }", "component C1: p.Res { value = 50ohm; }");
    let found = model(&design("shunt-resistor", PARTS, &resistor));
    match &found.signals["TX_P"].member("TX_P").unwrap().end {
        ChannelEnd::Open { stops } => {
            assert_eq!(stops.iter().map(|stop| stop.reason).collect::<Vec<_>>(), [StopReason::ReferenceNet])
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_walk_stops_at_the_first_connector() {
    // A second connector behind the first one is not reached.
    let board = coupled(
        "    component J2: p.Conn;\n    component R1: p.Res { value = 0ohm; }\n    component R2: p.Res { value = 0ohm; }\n    net TX_P { U1.TXP; J1.A; R1.A; }\n    net TX_N { U1.TXN; J1.B; R2.A; }\n    net FAR_P { R1.B; J2.A; }\n    net FAR_N { R2.B; J2.B; }\n",
    );
    let found = model(&design("first-connector", PARTS, &board));
    let source = only(&found);
    assert!(source.member("TX_P").unwrap().channel.is_empty());
    assert_eq!(connector_of(source, "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
}

#[test]
fn an_emission_constraint_names_a_connector_without_the_part_flag() {
    let parts = PARTS.replace("    external_cable = true;\n", "");
    let board =
        format!("{}    constraint emission(J1) {{ standard = \"cispr32_class_b\"; mode = target; }}\n", direct());
    let found = model(&design("emission", &parts, &board));
    assert_eq!(connector_of(only(&found), "TX_P"), Some(("J1".to_owned(), "A".to_owned())));
}

#[test]
fn a_series_part_the_walk_does_not_follow_is_a_recorded_stop_and_a_shunt_is_not() {
    // A series inductor between the driver and the connector: not followed, and the stop says where and why.
    let series = "    component U1: p.Phy;\n    component J1: p.Conn;\n    component L1: p.Inductor { value = 10nH; }\n    net TX_P { U1.TXP; L1.A; }\n    net LINK { L1.B; J1.A; }\n    net GND { U1.GND; }\n    net TX_N { U1.TXN; }\n";
    let found = model(&design("series_inductor", PARTS, series));
    let member = found.signals.get("TX_P").expect("TX_P").member("TX_P").unwrap();
    assert!(member.channel.is_empty());
    match &member.end {
        ChannelEnd::Open { stops } => {
            assert_eq!(stops.len(), 1, "{stops:?}");
            assert_eq!(stops[0].reason, StopReason::UnsupportedSeriesPart);
            assert_eq!(stops[0].at.to_string(), "L1.A");
        }
        other => panic!("{other:?}"),
    }
    // The same part from the signal to ground is a shunt: no stop, and the connector on the net is reached.
    let shunt = "    component U1: p.Phy;\n    component J1: p.Conn;\n    component L1: p.Inductor { value = 10nH; }\n    net TX_P { U1.TXP; L1.A; J1.A; }\n    net GND { U1.GND; L1.B; }\n    net TX_N { U1.TXN; }\n";
    let found = model(&design("shunt_inductor", PARTS, shunt));
    let member = found.signals.get("TX_P").expect("TX_P").member("TX_P").unwrap();
    assert!(matches!(member.end, ChannelEnd::Connector { .. }), "{:?}", member.end);
}

//! `copper explain`: what a diagnostic code means and its usual fix
//! (`pcbir.diagnostics.explain`).

use copper_json::{Object, Value, python_isdigit, python_isspace};

/// Each entry: code, summary, typical fix. Kept short: an agent reads one
/// entry at a time. In the Python registry's order.
pub const EXPLANATIONS: &[(&str, &str, &str)] = &[
    (
        "UNKNOWN_COMPONENT",
        "An endpoint, no_connect or binding names a component reference that is not declared.",
        "Fix the reference spelling, or declare the component. Module internals are reached through the instance's ports (INST.PORT), not INST/REF.",
    ),
    (
        "UNKNOWN_PIN",
        "The component's part has no pin (or device unit terminal) with this name.",
        "Use the exact pin name from the part definition: `copper query BOARD pins REF` lists them. Do not infer names from footprint pad numbers.",
    ),
    (
        "UNKNOWN_CONSTRAINT_TARGET",
        "A placement constraint (max_distance, align, placement_region, ...) names a component or pin that does not exist, so placement would silently drop it.",
        "Use the component reference (and exact pin name) as declared; `copper query BOARD components` and `copper query BOARD pins REF` list them.",
    ),
    (
        "UNKNOWN_PART",
        "A component uses a part that no `use library` or import provides.",
        "Check the import alias and exported name with `copper library search` / `inspect`.",
    ),
    (
        "UNKNOWN_NET",
        "A supply or interface refers to a net that is not declared.",
        "Declare the net, or point the supply at an existing net with `net = NAME`.",
    ),
    (
        "UNKNOWN_DEVICE",
        "A part references a device definition that is not loaded.",
        "Import the package that defines the device.",
    ),
    (
        "UNKNOWN_DEVICE_PAD",
        "A package pin bonds to a pad the device does not define.",
        "Repair the part/device definition in its library.",
    ),
    (
        "UNMODELED_PIN",
        "The pin has no electrical profile, so its connection cannot be checked.",
        "Add domains/directions to the pin in the part or device definition.",
    ),
    ("DUPLICATE_COMPONENT", "Two components share a reference.", "Rename one of them."),
    ("DUPLICATE_NET", "Two nets share a name.", "Merge their endpoints into one net or rename one."),
    ("DUPLICATE_SUPPLY", "Two supplies share a name.", "Remove or rename one."),
    ("DUPLICATE_INTERFACE", "Two interfaces share a name.", "Rename one."),
    ("DUPLICATE_PERIPHERAL_CONFIGURATION", "Two `configure` declarations share a name.", "Rename one."),
    (
        "PIN_ON_MULTIPLE_NETS",
        "One pin appears in more than one net, which would short those nets.",
        "Keep the pin in exactly one net; merge the nets if they really are one node.",
    ),
    (
        "INTERNAL_PAD_NET_CONFLICT",
        "Pads that the part connects internally are assigned to different nets.",
        "Put internally joined pads on the same net.",
    ),
    (
        "OUTPUT_CONFLICT",
        "More than one push-pull output drives the same net.",
        "Split the net or make all but one driver an input/open-drain.",
    ),
    ("UNKNOWN_SUPPLY_SOURCE", "A supply's `source` does not resolve to a known pin.", "Fix the endpoint."),
    (
        "SUPPLY_SOURCE_NOT_OUTPUT",
        "A supply's `source` pin cannot output power.",
        "Name the regulator or connector pin that actually sources the rail.",
    ),
    (
        "SUPPLY_SOURCE_NOT_ON_NET",
        "A supply's `source` pin is not on the supply's net.",
        "Connect the source pin to the net or correct the source.",
    ),
    (
        "UNSOURCED_POWER_INPUT",
        "A power input is unconnected, or its net has no output, external supply or DC path.",
        "Connect it to a powered net, mark the rail `external = true`, or name a `source`. A no_connect does not silence this: power inputs need an explicit decision in the part.",
    ),
    (
        "SUPPLY_VOLTAGE_HIGH",
        "The supply voltage exceeds the pin's operating maximum.",
        "Use a lower rail, a level shifter, or a part rated for the voltage.",
    ),
    (
        "SUPPLY_VOLTAGE_LOW",
        "The supply voltage is below the pin's operating minimum.",
        "Raise the rail or change part.",
    ),
    (
        "SUPPLY_VOLTAGE_ABSOLUTE_HIGH",
        "The supply exceeds the pin's absolute maximum rating.",
        "This can damage the part; change the rail or the part.",
    ),
    (
        "SUPPLY_VOLTAGE_ABSOLUTE_LOW",
        "The supply is below the pin's absolute minimum rating.",
        "Change the rail or the part.",
    ),
    ("POWER_DOMAIN_VOLTAGE_HIGH", "A device power domain is supplied above its maximum.", "Change the rail."),
    ("POWER_DOMAIN_VOLTAGE_LOW", "A device power domain is supplied below its minimum.", "Change the rail."),
    (
        "VOLTAGE_LIMIT_UNRESOLVED",
        "A pin voltage limit could not be evaluated (often relative to an unknown rail).",
        "Connect the reference pin to a declared supply.",
    ),
    (
        "EMI_HOT_LOOP_NO_CAPACITOR",
        "A switching cell declares a hot supply and a hot return, but no capacitor joins their nets, so the pulsed switch current has no local loop and flows through the supply.",
        "Add a ceramic input (or output, for a boost) capacitor between the hot supply and hot return pins, and declare it with `role = decoupling; decouples = U.PIN;`.",
    ),
    (
        "EMI_HOT_CAPACITOR_UNDECLARED",
        "Capacitors join the hot supply and hot return of a switching cell, but none declares `decouples` of the hot-supply pin, so placement and routing cannot treat the hot loop as a priority.",
        "On the capacitor nearest the pins, add `role = decoupling; decouples = U.PIN;` naming the hot-supply pin.",
    ),
    (
        "EMI_SWITCH_NODE_NO_MAGNETIC",
        "A pin declared emi_role = switch is on a net that reaches no inductor or transformer terminal (charge pumps are excluded).",
        "Connect the inductor or transformer, or correct the pin's emi_role or the part's topology (charge_pump).",
    ),
    (
        "EMI_BOOTSTRAP_NO_CAPACITOR",
        "A pin declared emi_role = bootstrap has no capacitor between its net and a switch net of the same component.",
        "Add the bootstrap capacitor between the BOOT pin and the switch pin.",
    ),
    (
        "EMI_SENSITIVE_ON_SWITCH_NODE",
        "A pin declared emi_role = feedback or sense is directly on a switch-node net, which swings rail to rail.",
        "Take the feedback or sense connection from the output side of the inductor, not from the switch node.",
    ),
    (
        "EMI_SWITCH_NODE_EXTERNAL",
        "A switch-node net also reaches a cable connector directly or through a series resistor, ferrite or dc_power_path part, so the unfiltered switching waveform can leave the board.",
        "Separate the nets, or filter after the inductor and output capacitor; the switch node should stay on the board.",
    ),
    (
        "EMI_SWITCHING_FREQUENCY_UNKNOWN",
        "A declared switching cell has no fixed switching frequency on its part or component (a range alone does not fix it), so emission estimates cannot use a single frequency.",
        "Declare `switching_frequency` on the part, or on the component when the part gives only a range or an external resistor sets it.",
    ),
    (
        "EMI_OPERATING_POINT_NO_CELL",
        "A component declares `load_current`, the operating point of a switching converter, but it is not a switching cell (no pin declares emi_role = switch and the structural fallback found none), so the operating point is not used.",
        "Remove `load_current` from the component, or give the converter part `emi_role = switch` on its switch pin and the other EMI roles.",
    ),
    (
        "EMI-HOT-LOOP",
        "`copper emi-check` found the pulsed-current loop of a switching cell (hot capacitor, hot-supply pin, switch pin, catch diode or hot-return pin, back to the capacitor) and reports its area, path length, a partial-inductance estimate and the radiated-field estimate in the same scenario as `EMI-CONDUCTED` (`measures.scenario`: `declared_operating_point` when the converter's component declares `load_current`, using the harmonic lines of the ramp-on-pedestal high-side switch current, with the switch current limit as `measures.secondary_scenario`; `current_limit` otherwise or outside continuous conduction, the trapezoid envelope at the limit, a worst case; the stored energy and overshoot are figured at the turn-off current I_out + dI/2 with the turn-on valley and the limit also given) (when the edge time is not declared, a default slew band gives the field as a nominal value with a [slow .. fast] band and the margin from the fast bound; declare `slew_rate` or `rise_time` on the switch pin to replace it); larger area and inductance mean more differential-mode emission and a higher ringing energy. The field estimate is an absolute screening estimate for a small loop in the far field; its validity (loop path at most a tenth of a wavelength, distance beyond lambda/2pi, distance at least 3 x the loop size) is stated with it, and the sweep stops where the loop is no longer electrically small. What it does not model (common-mode currents on cables, ground-plane reflection, the inductor's field) is listed in `estimate.not_modelled`. Screening evidence from pad centres; checked against hand calculations, not against measurements.",
        "Place the hot capacitor against the hot-supply and hot-return pins, keep the diode on the same side, and close the loop on the layer next to a ground plane. Review with `copper emi-check`; acknowledge a reviewed result with `constraint emi_accept(U1) { code = \"EMI-HOT-LOOP\"; reason = \"...\"; }`.",
    ),
    (
        "EMI-SWITCH-NODE",
        "`copper emi-check` measured the copper of a switch net: its area, the estimated capacitance to the nearest ground plane, the distance to the board edge and to cable connectors, and a voltage-driven common-mode coupling-risk SCORE (dB relative to f*C*V = 100 MHz x 1 pF x 1 V; a band when the edge time is a rule-of-thumb default). The score ranks switch nets by their drive (island capacitance, swing, edge, frequency); it is not a field, has no distance and no margin, and is never compared with an emission limit, because the board model has no cable, chassis or return-path structure (`estimate.not_modelled`). A pads-only result is a lower bound until the net is routed.",
        "Keep the switch-node copper as small as the current and thermal needs allow, away from the board edge and cable connectors, with no ground plane under a large pour that is not needed. Acknowledge a reviewed result with `emi_accept(U1.SW)`.",
    ),
    (
        "EMI-CONDUCTED",
        "`copper emi-check` predicted the conducted emission of a buck cell at its input (dBuV at a 50 uH V-network, 150 kHz to 30 MHz, differential mode only): the harmonic currents of the input pulse train of a fixed-frequency buck in continuous conduction, pushed through the input network in which every capacitor on the hot net and on the supply side of a recognised series input filter is its own R-L-C branch with its own connection inductance (the hot-loop inductance sits in the converter's branch and does not change the LISN level). Two scenarios exist, named in `measures.scenario`: `declared_operating_point` when the cell's component declares `load_current` (the input current is a ramp from I_out - dI/2 to I_out + dI/2 in the on-time, dI = (Vin - Vout) D / (L f) from the power inductor, or a rule-of-thumb ripple ratio, tag `rule_of_thumb_ripple`, when its value is not found; the switch current limit is then a secondary worst case), and `current_limit` otherwise (the limit `current_peak` as the amplitude of a flat-top pulse: a worst case that bounds operation, not a prediction; declare `load_current = <current>;` on the component to give the cell an operating point). A declared load current outside continuous conduction (I_out < dI/2) is outside the model: only the current-limit scenario is reported (tag `operating_point_outside_model`). The current commutation time is `current_rise_time`/`current_fall_time` on the switch pin, else the voltage edge stands in for it (tag `current_edge_from_voltage_edge`, an assumption). Capacitor parasitics default by package when the part declares none, the report names the capacitor that carries the largest share of the ripple current at the fundamental and at the worst harmonic and any anti-resonance of the input network between 150 kHz and 30 MHz, and common mode is not modelled. The input network's anti-resonances are all listed in `measures.anti_resonances`; only one that raises a reported harmonic at the LISN by at least 6 dB over the level with the hot node held by its best single capacitor branch alone is tagged `anti_resonance_in_band` and named in the headline. Capacitor branch currents are ratios |I_branch| / |I_source|, not a partition, and can add to more than 100 %. Rule-of-thumb defaults fill gaps and are reported as bands, never as datasheet values: a missing edge time becomes a default slew band of 3 V/ns nominal, 10 V/ns fast and 1 V/ns slow on the hot-supply voltage (`rule_of_thumb_edge`; a declared `slew_rate` gives an exact edge, tag `edge_from_slew_rate`), and a ceramic capacitor loses capacitance to DC bias by package (`rule_of_thumb_dc_bias`) unless its component declares `capacitance_at_bias`. Every harmonic carries `low_dbuv` and `high_dbuv`; the headline and the margin use the high end. With a cispr32 `emission` standard the margins are against the mains-port quasi-peak and average lines, never a pass or fail. The margin exists only where the isolated-line detector assumption holds (`measures.detector_applicability = isolated_lines`: one constant switching frequency whose harmonic spacing exceeds twice the receiver bandwidth, 9 kHz in Band B), or at the harmonics a receiver emulation read (`receiver_emulation`: one constant frequency with closely spaced harmonics; at most 48 harmonics are run through a time-domain receiver model with Gaussian IF filter and quasi-peak time constants that are remembered CISPR 16-1-1 values and unverified, the others stay line amplitudes). Otherwise (`not_applicable`: a declared switching-frequency range) the levels are line amplitudes, not detector readings, and no limit or margin is stated; a declared range is evaluated as two fixed-frequency scenarios (its lower and upper bound), and the swept or modulated case is not modelled. Declare `switching_frequency` on the component to fix the frequency.",
        "Put a ceramic capacitor with low ESL against the hot-supply and hot-return pins, keep the hot loop small, add an input filter (a series inductor with a capacitor on its supply side), and declare `esr`, `esl` or `self_resonance` on the capacitor parts and the filter inductor. Acknowledge a reviewed result with `constraint emi_accept(U1) { code = \"EMI-CONDUCTED\"; reason = \"...\"; }`.",
    ),
    (
        "EMI-PROXIMITY",
        "`copper emi-check` found an inductor or switch-node pad closer to a sensitive net (feedback, sense, RF feed) or a cable connector than the screening distance, a multiple of the inductor's size that depends on its `shielding`.",
        "Move the sensitive pads or the connector away, choose a shielded inductor (`shielding = shielded` on the part), or put the sensitive trace on the far side of a ground plane. Acknowledge a reviewed result with `emi_accept`.",
    ),
    (
        "EMI-ACCEPT-UNUSED",
        "An `emi_accept` constraint matched no `copper emi-check` finding: the finding is gone, or the target pin or component does not take part in it.",
        "Remove the constraint, or correct its target and `code` to the finding it acknowledges.",
    ),
    (
        "EMI-RETURN-PATH",
        "Coverage entry of `copper emi-check`: the return-path check needs filled copper and traced routed copper and is not implemented, so no result says anything about plane gaps or layer changes.",
        "Review reference-plane continuity under aggressor and sensitive tracks in the filled board.",
    ),
    (
        "EMI-DECOUPLING-PATH",
        "Coverage entry of `copper emi-check`: the copper path length from each decoupling capacitor to its pin and return via needs routed copper and is not implemented.",
        "Check the decoupling paths after routing.",
    ),
    (
        "LOGIC_HIGH_OVERVOLTAGE",
        "A push-pull output, or a pull-up resistor, takes a signal net above the voltage limit of a pin on it.",
        "Use a level shifter, a lower I/O rail, or a part whose input is rated for the driver's rail. Above an absolute maximum is an error; above only the operating maximum is a warning when an absolute maximum is declared.",
    ),
    (
        "LOGIC_HIGH_TOO_LOW",
        "A driver's output-high level is below the receiver's declared input-high threshold (vih).",
        "Use a level shifter or a driver on a higher I/O rail, or check the thresholds against the datasheet.",
    ),
    (
        "LOGIC_LOW_TOO_HIGH",
        "A driver's declared output-low level (vol) is above the receiver's declared input-low threshold (vil).",
        "Use a driver with a lower output-low level or a receiver with a higher vil.",
    ),
    (
        "LOGIC_LEVEL_UNRESOLVED",
        "A pin's explicit io_supply or rail-relative logic threshold could not be evaluated.",
        "Declare a supply on the net of the named reference pin, or fix the reference. Until then the levels of those pins are not checked.",
    ),
    (
        "MISSING_PULL",
        "A pin declares requires_pull, but its net has no resistor to a declared supply (pull-up) or 0 V net (pull-down) and is not tied to one directly. The resistor value is not checked.",
        "Add the pull resistor the datasheet demands, or tie the net to the rail if that is allowed.",
    ),
    (
        "UNDRIVEN_INPUT",
        "Every pin on the net is a digital or clock input, and nothing drives it: no declared supply, no pull resistor to a supply or ground, and no pin with an internal_pull. Passive, bidirectional, output, power and unmodeled pins count as drivers.",
        "Connect an output, add a pull resistor, or declare internal_pull on the pin if firmware enables it.",
    ),
    (
        "OPEN_DRAIN_NO_PULLUP",
        "Every pin that can drive the net is open-drain, but the net has no pull-up resistor to a declared supply and no pin declares an internal pull-up. I2C nets are covered by I2C_MISSING_PULLUP.",
        "Add a pull-up resistor, or declare internal_pull on a pin that has a pull-up you will enable in firmware.",
    ),
    ("I2C_MISSING_SIGNAL", "An I2C interface does not map SDA or SCL.", "Add `sda = NET; scl = NET;`."),
    ("I2C_TOO_FEW_PARTICIPANTS", "An I2C interface binds fewer than two components.", "Bind the host and each target."),
    ("I2C_MISSING_BINDING", "A bound component has no SDA or SCL pin binding.", "Add the pin to its `bind` block."),
    ("I2C_UNKNOWN_PIN", "An I2C binding names a pin the part does not have.", "Fix the pin name."),
    ("I2C_BINDING_NOT_ON_NET", "A bound pin is not on the mapped SDA/SCL net.", "Add the pin to that net."),
    ("I2C_INCOMPATIBLE_PIN", "A bound pin cannot act as an open-drain I2C line.", "Choose an I2C-capable pin."),
    ("I2C_NO_PULLUP_SUPPLY", "The interface does not name a pull-up supply.", "Add `pullup = RAIL;`."),
    ("I2C_UNKNOWN_PULLUP_SUPPLY", "The named pull-up supply is not declared.", "Declare it or fix the name."),
    (
        "I2C_MISSING_PULLUP",
        "SDA or SCL has no resistor to the pull-up rail.",
        "Add one resistor from each line to the pull-up supply net.",
    ),
    ("UNKNOWN_CONFIGURATION_COMPONENT", "A `configure` names an unknown component.", "Fix the reference."),
    (
        "PART_HAS_NO_DEVICE",
        "Peripheral configuration needs a part backed by a device definition.",
        "Use a part whose definition names a device.",
    ),
    ("UNKNOWN_PERIPHERAL", "The device has no peripheral with this name.", "Inspect the device's peripherals."),
    ("MISSING_PERIPHERAL_SIGNAL", "A required peripheral signal is not assigned a pin.", "Assign it in `configure`."),
    ("UNKNOWN_PERIPHERAL_SIGNAL", "The peripheral has no signal with this name.", "Fix the signal name."),
    ("UNKNOWN_MUX_PIN", "A configuration selects a physical pin the part does not have.", "Fix the pin name."),
    (
        "INVALID_MUX_OPTION",
        "The selected pin cannot carry that peripheral signal.",
        "Pick a pin with a mux option for it.",
    ),
    ("MUX_SELECTOR_MISMATCH", "Resolved mux metadata disagrees with the device definition.", "Repair the definition."),
    ("MUX_INCOMPATIBLE_PIN", "The pin's electrical profile is incompatible with the signal.", "Pick another pin."),
    ("PERIPHERAL_CONFLICT", "One peripheral instance is configured more than once.", "Keep one configuration."),
    ("PIN_MUX_CONFLICT", "One physical pin is selected by several signals.", "Give each signal its own pin."),
    ("MUX_RESOURCE_CONFLICT", "Selections need incompatible settings of a shared resource.", "Choose compatible pins."),
    ("DO_NOT_CONNECT", "The part marks this pin do-not-connect, but it is on a net.", "Remove it from the net."),
    (
        "REQUIRED_PIN_UNCONNECTED",
        "The part requires this pin to be connected.",
        "Connect it as the datasheet requires.",
    ),
    (
        "REQUIRED_NET_TRAIT",
        "The pin requires net traits (for example a ground) that its net lacks.",
        "Connect it to an appropriate net.",
    ),
    ("UNKNOWN_MODE_GROUP", "A component selects a mode group the device does not define.", "Fix the name."),
    ("MODE_NOT_SELECTED", "A device mode group has neither a default nor a selection.", "Select a mode."),
    ("UNKNOWN_MODE_CHOICE", "The selected mode is not a choice of that group.", "Use one of the declared choices."),
    (
        "INCOMPLETE_DIFFERENTIAL_PAIR",
        "Only one polarity of a differential pair is connected.",
        "Connect both or neither.",
    ),
    (
        "NO_CONNECT_CONNECTED",
        "A pin is declared `no_connect` but also appears in a net.",
        "Remove the pin from the net, or delete the no_connect if the connection is intended.",
    ),
    (
        "NO_CONNECT_REQUIRED_PIN",
        "A pin the part requires to be connected is declared `no_connect`.",
        "Connect the pin as the datasheet requires and remove the no_connect.",
    ),
    ("DUPLICATE_NO_CONNECT", "The same pin is declared `no_connect` more than once.", "Remove the duplicate."),
    (
        "NO_CONNECT_MODULE_PORT",
        "A `no_connect` names a module instance port; it applies only to component pins.",
        "Connect the port in the parent (every port must be connected), or declare the spare pin `no_connect` inside the module source.",
    ),
    (
        "UNMARKED_PIN",
        "Ordinary pins are neither on a net nor declared `no_connect`, so an omission cannot be told apart from a decision. Power inputs, required, optional and do-not-connect pins and pins without an electrical profile are excluded because they are reported or decided elsewhere. Components inside module instances are left to the module's own source.",
        "Connect each pin, or declare it: `no_connect U1.PA0, U1.PA1 { reason = \"unused GPIO\"; }`. `copper query BOARD unconnected` lists them; `copper check --pins error` makes this blocking.",
    ),
];

/// Compiler error families: `PAR002` and friends.
const FAMILIES: &[(&str, &str)] = &[
    ("PAR", "Syntax error from the parser. The location points at the offending token."),
    ("LEX", "Lexical error (unterminated string, invalid character). The location points at it."),
    ("CMP", "Compile/lowering error: the source parsed but a declaration is invalid. Read the message."),
    ("MEC", "Mechanical declaration error."),
    ("SLK", "Silkscreen declaration error."),
    ("MAC", "Physical macro definition or binding error."),
    (
        "BLD",
        "Board `build` block error: a setting has the wrong type, unit or range, or names an unknown router field.",
    ),
];

const FAMILY_FIX: &str = "Use the message and hint; `copper check --json` gives the exact location.";

/// The family prefixes in the order `--list --json` reports them.
pub const FAMILY_NAMES: [&str; 7] = ["BLD", "CMP", "LEX", "MAC", "MEC", "PAR", "SLK"];

/// Python's `code.strip().upper()`.
fn normalize(code: &str) -> String {
    code.trim_matches(python_isspace).to_uppercase()
}

/// The entry for `code`, as `{"code", "summary", "fix"}`; `None` when unknown.
pub fn explain(code: &str) -> Option<Value> {
    let code = normalize(code);
    let entry = |summary: &str, fix: &str| {
        Value::Object(Object::from([
            ("code".to_owned(), Value::from(code.as_str())),
            ("summary".to_owned(), Value::from(summary)),
            ("fix".to_owned(), Value::from(fix)),
        ]))
    };
    if let Some((_, summary, fix)) = EXPLANATIONS.iter().find(|(name, _, _)| *name == code) {
        return Some(entry(summary, fix));
    }
    let family: String = code.chars().take(3).collect();
    let number: String = code.chars().skip(3).collect();
    let (_, summary) = FAMILIES.iter().find(|(name, _)| *name == family)?;
    (!number.is_empty() && number.chars().all(python_isdigit)).then(|| entry(summary, FAMILY_FIX))
}

/// The closest explained code, as `difflib.get_close_matches(..., n=1, cutoff=0.6)`.
pub fn suggest_code(code: &str) -> Option<String> {
    let codes: Vec<&str> = EXPLANATIONS.iter().map(|(name, _, _)| *name).collect();
    copper_lang::close_match(&normalize(code), &codes)
}

/// Every explained code, sorted.
pub fn codes() -> Vec<&'static str> {
    let mut codes: Vec<&str> = EXPLANATIONS.iter().map(|(name, _, _)| *name).collect();
    codes.sort_unstable();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_families_and_suggestions() {
        assert_eq!(explain(" unknown_pin\n").unwrap().get("code"), Some(&Value::from("UNKNOWN_PIN")));
        assert_eq!(explain("par002").unwrap().get("fix"), Some(&Value::from(FAMILY_FIX)));
        assert!(explain("PAR").is_none());
        assert!(explain("PAR0x").is_none());
        assert_eq!(suggest_code("UNKNOWN_PINS").as_deref(), Some("UNKNOWN_PIN"));
    }

    #[test]
    fn the_operating_point_warning_and_the_scenarios_are_explained() {
        let warning = explain("emi_operating_point_no_cell").expect("explained");
        assert!(warning.get("fix").and_then(Value::as_str).is_some_and(|fix| fix.contains("load_current")));
        let conducted = explain("EMI-CONDUCTED").expect("explained");
        let text = copper_json::Dumps::new().dumps(&conducted);
        for needle in ["declared_operating_point", "current_limit", "load_current", "current_edge_from_voltage_edge"] {
            assert!(text.contains(needle), "{needle}");
        }
        assert!(!text.contains("0.4 x the limit") && !text.contains("rule_of_thumb_current"));
    }
}

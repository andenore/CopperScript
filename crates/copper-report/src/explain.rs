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
];

const FAMILY_FIX: &str = "Use the message and hint; `copper check --json` gives the exact location.";

/// The family prefixes in the order `--list --json` reports them.
pub const FAMILY_NAMES: [&str; 6] = ["CMP", "LEX", "MAC", "MEC", "PAR", "SLK"];

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
}

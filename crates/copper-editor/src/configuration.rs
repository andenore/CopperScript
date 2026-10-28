//! `pcbir.editor.configuration`: the editor's copper stack, chosen from
//! authoritative source intent and explicit settings.

use std::collections::BTreeSet;

use copper_json::Value;
use copper_lang::compiler::Design;
use copper_lang::model::ParameterValue;
use copper_pcb::physicalize::PhysicalOptions;

use crate::{Error, Result, value};

/// `In([1-9][0-9]*)\.Cu` -> the index plus two.
fn inner_minimum(layer: &str) -> Option<i64> {
    let digits = layer.strip_prefix("In")?.strip_suffix(".Cu")?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Python ints are unbounded; such a stack is refused below either way.
    Some(digits.parse::<i64>().unwrap_or(i64::MAX - 2) + 2)
}

fn minimum(required: &BTreeSet<String>) -> i64 {
    required.iter().filter_map(|layer| inner_minimum(layer)).max().unwrap_or(2)
}

/// `physical_options`: an explicit count stays strict; automatic selection
/// takes the smallest supported stack.
pub fn physical_options(design: &Design, layers: Option<i64>, fab_profile: &str) -> Result<PhysicalOptions> {
    let flat = copper_lang::elaborate::elaborate(&design.electrical).map_err(Error::Value)?;
    let mut required: BTreeSet<String> = ["F.Cu", "B.Cu"].iter().map(|s| (*s).to_owned()).collect();
    for constraint in &flat.constraints {
        if !["routing", "copper_zone"].contains(&constraint.kind.value()) {
            continue;
        }
        for key in ["layers", "allowed_layers", "shared_reference_layer"] {
            if let Some(ParameterValue::Text(text)) = constraint.parameters.get(key) {
                required.extend(
                    text.split(',')
                        .map(|x| x.trim_matches(copper_json::python_isspace))
                        .filter(|x| !x.is_empty())
                        .map(str::to_owned),
                );
            }
        }
    }
    if let Some(mechanical) = &design.mechanical {
        for keepout in &mechanical.copper_keepouts {
            required.extend(keepout.layers.iter().map(|l| l.value().to_owned()));
        }
    }
    let mut lowest = minimum(&required);
    let layers = match layers {
        Some(layers) => layers,
        None => match design.mechanical.as_ref().and_then(|m| m.stackup.as_ref()) {
            Some(stackup) => stackup.layers.iter().filter(|l| l.copper_layer.is_some()).count() as i64,
            None => {
                if lowest < 6 {
                    let macros =
                        copper_lang::macros::prepare_macro_bindings(&flat, true, true).map_err(Error::Value)?;
                    for prepared in macros {
                        if let Some(Value::Array(items)) = prepared.data.get("required_layers") {
                            required.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
                        }
                    }
                    lowest = minimum(&required);
                }
                lowest = lowest.max(match fab_profile {
                    "jlcpcb-four-layer" => 4,
                    "jlcpcb-six-layer" => 6,
                    _ => 2,
                });
                match [2, 4, 6].into_iter().find(|count| *count >= lowest) {
                    Some(count) => count,
                    None => return value("source requires more than six copper layers; the editor supports 2, 4 or 6"),
                }
            }
        },
    };
    if lowest > layers {
        let names: Vec<&str> = required.iter().map(String::as_str).collect();
        return value(format!(
            "source requires {}, outside the selected {layers}-layer stack; omit --layers for automatic selection or choose a compatible explicit stack",
            names.join(", ")
        ));
    }
    PhysicalOptions::new(100.0, 80.0, layers, fab_profile).map_err(Error::from)
}

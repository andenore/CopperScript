//! Resolve physical construction from source and explicit invocation settings.
//!
//! Shared by physical CLI commands and the editor. An inferred layer count is
//! not a physical dielectric stackup or fabrication qualification.

use std::collections::BTreeSet;

use copper_json::Value;
use copper_lang::compiler::Design;
use copper_lang::model::ParameterValue;

use crate::physicalize::PhysicalOptions;
use crate::{Error, Result};

/// The minimum stack size that can contain an `InN.Cu` layer.
fn inner_minimum(layer: &str) -> Option<i64> {
    let digits = layer.strip_prefix("In")?.strip_suffix(".Cu")?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(digits.parse::<i64>().unwrap_or(i64::MAX).saturating_add(2))
}

fn minimum(required: &BTreeSet<String>) -> i64 {
    required.iter().filter_map(|layer| inner_minimum(layer)).max().unwrap_or(2)
}

/// Select an exact source stack, an explicit compatible count, or the
/// smallest supported stack accommodating source requirements. Macro inputs
/// use the caller's dependency policy, just as subsequent physicalization does.
pub fn physical_options(
    design: &Design,
    dimensions_mm: (f64, f64),
    layers: Option<i64>,
    fab_profile: &str,
    locked: bool,
    offline: bool,
) -> Result<PhysicalOptions> {
    let declared = design.mechanical.as_ref().and_then(|m| m.stackup.as_ref());
    let declared_count = declared.map(|stack| stack.layers.iter().filter(|l| l.copper_layer.is_some()).count() as i64);
    if let (Some(selected), Some(count)) = (layers, declared_count)
        && selected != count
    {
        return Err(Error::Value(format!(
            "{}: stackup declares {count} copper layers but --layers selects {selected}; omit --layers or select {count}",
            declared.expect("declared count").location
        )));
    }

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
    let selected = match layers.or(declared_count) {
        Some(count) => count,
        None => {
            if lowest < 6 {
                let macros =
                    copper_lang::macros::prepare_macro_bindings(&flat, locked, offline).map_err(Error::Value)?;
                for prepared in macros {
                    if let Some(Value::Array(items)) = prepared.data.get("required_layers") {
                        required.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
                    }
                }
                lowest = minimum(&required);
            }
            let needed = lowest.max(match fab_profile {
                "jlcpcb-four-layer" => 4,
                "jlcpcb-six-layer" => 6,
                _ => 2,
            });
            [2, 4, 6].into_iter().find(|count| *count >= needed).ok_or_else(|| {
                Error::Value("source requires more than six copper layers; supported stacks are 2, 4 or 6".to_owned())
            })?
        }
    };
    if lowest > selected {
        let names: Vec<&str> = required.iter().map(String::as_str).collect();
        return Err(Error::Value(format!(
            "source requires {}, outside the selected {selected}-layer stack; declare a compatible stackup or select a compatible --layers count",
            names.join(", ")
        )));
    }
    PhysicalOptions::new(dimensions_mm.0, dimensions_mm.1, selected, fab_profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_lang::compiler::compile_design_source;

    fn design(body: &str) -> Design {
        compile_design_source(&format!("board Config {{ {body} }}"), "config.copper", true, true).unwrap()
    }

    fn select(design: &Design, layers: Option<i64>, profile: &str) -> Result<PhysicalOptions> {
        physical_options(design, (100.0, 80.0), layers, profile, true, true)
    }

    const STACK: &str = "mechanical {
        outline rectangle { width = 40mm; height = 30mm; }
        stackup {
            copper F.Cu { thickness = 0.035mm; }
            dielectric top { thickness = 0.2mm; er = 4.1; }
            copper In1.Cu { thickness = 0.035mm; }
            dielectric core { thickness = 1mm; er = 4.1; }
            copper In2.Cu { thickness = 0.035mm; }
            dielectric bottom { thickness = 0.2mm; er = 4.1; }
            copper B.Cu { thickness = 0.035mm; }
        }
    }";

    #[test]
    fn declared_stack_is_authoritative_and_survives_physicalization() {
        let design = design(STACK);
        let inferred = select(&design, None, "generic").unwrap();
        assert_eq!(inferred.copper_layers, 4);
        let explicit = select(&design, Some(4), "generic").unwrap();
        let lower = |options: &PhysicalOptions| {
            crate::physicalize::physicalize(&design, crate::physicalize::Footprints::Proxy, options, true, true)
                .unwrap()
        };
        let board = lower(&inferred);
        assert_eq!(board, lower(&explicit));
        assert_eq!(board.stackup.physical_layers.len(), 7);
        assert_eq!(board.stackup.thickness_nm, 1_540_000);
        for count in [2, 6] {
            let error = select(&design, Some(count), "generic").unwrap_err().to_string();
            assert!(error.contains("config.copper:") && error.contains("stackup declares 4"), "{error}");
        }
        assert!(select(&design, None, "jlcpcb-six-layer").is_err());
    }

    #[test]
    fn automatic_stacks_cover_constraints_and_profiles_without_inventing_materials() {
        let empty = design("");
        assert_eq!(select(&empty, None, "generic").unwrap().copper_layers, 2);
        assert_eq!(select(&empty, None, "jlcpcb-four-layer").unwrap().copper_layers, 4);
        assert_eq!(select(&empty, None, "jlcpcb-six-layer").unwrap().copper_layers, 6);
        assert!(select(&empty, Some(2), "jlcpcb-four-layer").is_err());
        for (layer, count) in [("In1.Cu", 4), ("In2.Cu", 4), ("In3.Cu", 6), ("In4.Cu", 6)] {
            let design = design(&format!("constraint routing(N) {{ allowed_layers = \"{layer}\"; }}"));
            let options = select(&design, None, "generic").unwrap();
            assert_eq!(options.copper_layers, count);
            assert!(select(&design, Some(2), "generic").is_err());
        }
        for source in [
            "constraint routing(N) { shared_reference_layer = \"In1.Cu\"; }",
            "constraint copper_zone(N) { layers = \"In2.Cu\"; }",
            "mechanical {
                outline rectangle { width = 40mm; height = 30mm; }
                copper_keepout inner rectangle {
                    origin = (10mm, 10mm); width = 5mm; height = 5mm; layers = \"In2.Cu\";
                }
            }",
        ] {
            assert_eq!(select(&design(source), None, "generic").unwrap().copper_layers, 4);
        }
        let options = select(&empty, None, "generic").unwrap();
        let board =
            crate::physicalize::physicalize(&empty, crate::physicalize::Footprints::Proxy, &options, true, true)
                .unwrap();
        assert!(board.stackup.physical_layers.is_empty());
        assert!(select(&design("constraint routing(N) { allowed_layers = \"In5.Cu\"; }"), None, "generic").is_err());
        assert_eq!(inner_minimum("In9223372036854775807.Cu"), Some(i64::MAX));
    }
}

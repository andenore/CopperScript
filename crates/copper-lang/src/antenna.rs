//! `constraint antenna(COMPONENT.PIN) { ... }`: source validation and the
//! shared frequency-band grammar for synthesized trace antennas. Geometry
//! generation and binding live in `copper_pcb::antenna`; see
//! `docs/pcb-antenna-synthesis-research.md`.
//!
//! The declaration is intent, not evidence: accepting it proves nothing about
//! resonance, matching, radiation or regulatory compliance.

use indexmap::IndexMap;

use copper_json::{python_isspace, python_str_repr as repr};
use copper_units::{Decimal, Dimension, Quantity};

use crate::error::CopperScriptError;
use crate::lower::{error, is_length, quantity_dimension};
use crate::model::ParameterValue;
use crate::syntax::ConstraintDecl;

type Result<T> = std::result::Result<T, CopperScriptError>;

/// The diagnostic code of every antenna declaration error.
pub const CODE: &str = "CMP126";

/// Every parameter an antenna declaration accepts (besides the generic
/// `id`, `mode`, `consumers` and `verifier`).
pub const PARAMETERS: [&str; 17] = [
    "name",
    "region",
    "polygon_mm",
    "x",
    "y",
    "width",
    "height",
    "side",
    "ground",
    "bands",
    "topology",
    "track_width",
    "keepout_margin",
    "max_return_loss_db",
    "geometry",
    "tuned_for",
    "short",
];

/// Accepted `topology` words. `inverted_f` needs `short`, a two-pad link
/// component that ties the arm to ground. `loop` is reserved: physical
/// lowering reports it as not implemented rather than approximating.
pub const TOPOLOGIES: [&str; 6] = ["auto", "monopole", "meander", "pixel", "inverted_f", "loop"];

/// One declared operating band, `LOW-HIGH`, in hertz.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub low_hz: Decimal,
    pub high_hz: Decimal,
    /// The band as written, trimmed.
    pub text: String,
}

impl Band {
    /// The arithmetic centre frequency in hertz.
    pub fn centre_hz(&self) -> Decimal {
        self.low_hz.add(&self.high_hz).and_then(|sum| sum.div(&Decimal::from_i64(2))).expect("finite band limits")
    }
}

fn strip(text: &str) -> &str {
    text.trim_matches(python_isspace)
}

/// `"2400 MHz"` or `"2.45GHz"` to hertz.
fn frequency_hz(text: &str) -> std::result::Result<Decimal, String> {
    let text = strip(text);
    let split = text.rfind(|c: char| !c.is_ascii_alphabetic()).map_or(0, |index| index + 1);
    let (number, unit) = (strip(&text[..split]), &text[split..]);
    if number.is_empty() || unit.is_empty() {
        return Err(format!("{} is not a frequency with a unit", repr(text)));
    }
    if quantity_dimension(unit) != Some(Dimension::Frequency) {
        return Err(format!("{} is not a frequency unit", repr(unit)));
    }
    let value = Decimal::parse(number).map_err(|_| format!("{} is not a frequency", repr(text)))?;
    let quantity = Quantity::of(Dimension::Frequency, &value, unit).map_err(|e| e.to_string())?;
    let hertz = quantity.base_value().clone();
    if hertz <= Decimal::zero() {
        return Err(format!("frequency {} must be positive", repr(text)));
    }
    Ok(hertz)
}

/// `"2400 MHz-2483.5 MHz; 5150 MHz-5850 MHz"`: semicolon-separated bands,
/// each `LOW-HIGH` with frequency units on both limits.
pub fn parse_bands(text: &str) -> std::result::Result<Vec<Band>, String> {
    let mut bands = Vec::new();
    for item in text.split(';') {
        let item = strip(item);
        if item.is_empty() {
            continue;
        }
        let Some((low, high)) = item.split_once('-') else {
            return Err(format!("band {} must be written LOW-HIGH with units, like 2400 MHz-2483.5 MHz", repr(item)));
        };
        let (low_hz, high_hz) = (frequency_hz(low)?, frequency_hz(high)?);
        if low_hz >= high_hz {
            return Err(format!("band {} must have its lower limit first", repr(item)));
        }
        bands.push(Band { low_hz, high_hz, text: item.to_owned() });
    }
    if bands.is_empty() {
        return Err("at least one band is required".to_owned());
    }
    Ok(bands)
}

fn text<'a>(parameters: &'a IndexMap<String, ParameterValue>, name: &str) -> Option<&'a str> {
    match parameters.get(name) {
        Some(ParameterValue::Text(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn length<'a>(parameters: &'a IndexMap<String, ParameterValue>, name: &str) -> Option<&'a Quantity> {
    match parameters.get(name) {
        Some(ParameterValue::Quantity(quantity)) if is_length(quantity) => Some(quantity),
        _ => None,
    }
}

/// `_validate_antenna`: the declaration shape every antenna must have before
/// physical lowering sees it.
pub fn validate_antenna(declaration: &ConstraintDecl, parameters: &IndexMap<String, ParameterValue>) -> Result<()> {
    let location = &declaration.location;
    let fail = |message: String| Err(error(CODE, message, location));
    if declaration.targets.len() != 1 || !declaration.targets[0].contains('.') {
        return fail("antenna requires exactly one COMPONENT.PIN feed target".to_owned());
    }
    let mut unknown: Vec<&String> = parameters.keys().filter(|name| !PARAMETERS.contains(&name.as_str())).collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return fail(format!("unknown antenna parameter {}", repr(name)));
    }
    let rectangle = ["x", "y", "width", "height"];
    let rectangular = rectangle.iter().any(|name| parameters.contains_key(*name));
    let modes = usize::from(rectangular)
        + usize::from(parameters.contains_key("region"))
        + usize::from(parameters.contains_key("polygon_mm"));
    if modes != 1 {
        return fail("antenna requires exactly one region form: region, polygon_mm, or x/y/width/height".to_owned());
    }
    if rectangular {
        for name in rectangle {
            if length(parameters, name).is_none() {
                return fail(format!("antenna rectangle requires length {}", repr(name)));
            }
        }
        for name in ["width", "height"] {
            if *length(parameters, name).expect("checked").base_value() <= Decimal::zero() {
                return fail(format!("antenna {} must be positive", repr(name)));
            }
        }
    }
    for name in ["region", "polygon_mm", "ground", "name", "geometry", "tuned_for", "short"] {
        if parameters.contains_key(name) && text(parameters, name).is_none_or(|value| strip(value).is_empty()) {
            return fail(format!("antenna parameter {} must be a nonempty string", repr(name)));
        }
    }
    if !parameters.contains_key("ground") {
        return fail("antenna requires 'ground', the reference ground net".to_owned());
    }
    let Some(bands) = text(parameters, "bands") else {
        return fail("antenna requires 'bands', like \"2400 MHz-2483.5 MHz\"".to_owned());
    };
    if let Err(message) = parse_bands(bands) {
        return fail(format!("invalid antenna bands: {message}"));
    }
    if let Some(side) = parameters.get("side")
        && !matches!(side, ParameterValue::Text(value) if value == "front" || value == "back")
    {
        return fail("antenna side must be front or back".to_owned());
    }
    if let Some(short) = text(parameters, "short")
        && (short.contains('.') || short.contains('/'))
    {
        return fail(
            "antenna short must be a component reference, the two-pad link that ties the arm to ground".to_owned(),
        );
    }
    if text(parameters, "topology") == Some("inverted_f") && !parameters.contains_key("short") {
        return fail("antenna topology inverted_f requires short, the two-pad link component".to_owned());
    }
    if parameters.contains_key("topology") && !text(parameters, "topology").is_some_and(|v| TOPOLOGIES.contains(&v)) {
        return fail(format!("antenna topology must be one of {}", TOPOLOGIES.join(", ")));
    }
    if let Some(digest) = text(parameters, "tuned_for")
        && !(digest.len() == 71
            && digest.starts_with("sha256:")
            && digest[7..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
    {
        return fail("antenna tuned_for must be sha256: followed by 64 lowercase hex digits".to_owned());
    }
    if parameters.contains_key("track_width")
        && !length(parameters, "track_width").is_some_and(|value| *value.base_value() > Decimal::zero())
    {
        return fail("antenna track_width must be a positive length".to_owned());
    }
    if parameters.contains_key("keepout_margin")
        && !length(parameters, "keepout_margin").is_some_and(|value| !value.base_value().is_negative())
    {
        return fail("antenna keepout_margin must be a nonnegative length".to_owned());
    }
    if parameters.contains_key("max_return_loss_db")
        && !matches!(parameters.get("max_return_loss_db"), Some(ParameterValue::Int(_) | ParameterValue::Float(_)))
    {
        return fail("antenna max_return_loss_db must be a number".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_parse_with_units_and_reject_malformed_text() {
        let bands = parse_bands("2400 MHz-2483.5 MHz; 5.15GHz - 5.85 GHz").expect("two bands");
        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].low_hz, Decimal::parse("2400000000").unwrap());
        assert_eq!(bands[0].high_hz, Decimal::parse("2483500000").unwrap());
        assert_eq!(bands[0].centre_hz(), Decimal::parse("2441750000").unwrap());
        assert_eq!(bands[1].low_hz, Decimal::parse("5150000000").unwrap());
        assert!(parse_bands("2400-2483.5 MHz").is_err());
        assert!(parse_bands("2483.5 MHz-2400 MHz").is_err());
        assert!(parse_bands("2400 mm-2483 mm").is_err());
        assert!(parse_bands("").is_err());
    }
}

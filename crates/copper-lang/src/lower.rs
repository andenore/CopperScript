//! Lowering of parts, devices and unit declarations (`pcbir.compiler`).
//!
//! Every check, its order and its diagnostic follow the Python compiler, so
//! the first error a source produces is the same in both implementations.

use std::collections::BTreeSet;

use copper_json::{python_isspace, python_str_repr as repr};
use copper_units::{Decimal, Dimension, Quantity, Rounding};
use indexmap::IndexMap;

use crate::error::CopperScriptError;
use crate::model::*;
use crate::suggest::close_matches;
use crate::syntax::*;

pub type Result<T> = std::result::Result<T, CopperScriptError>;

pub fn error(code: &str, message: impl Into<String>, location: &Location) -> CopperScriptError {
    CopperScriptError::new(code, message, location.clone())
}

/// `QUANTITY_TYPES`: the units a source quantity may use. `ohm` resolves to
/// resistance, as the later dict entry wins in Python.
pub fn quantity_dimension(unit: &str) -> Option<Dimension> {
    [
        Dimension::Voltage,
        Dimension::Resistance,
        Dimension::Capacitance,
        Dimension::Inductance,
        Dimension::Length,
        Dimension::Current,
        Dimension::Frequency,
    ]
    .into_iter()
    .find(|dimension| dimension.units().iter().any(|(name, _)| *name == unit))
}

fn quantity_units() -> Vec<&'static str> {
    let mut units: Vec<&str> = [
        Dimension::Voltage,
        Dimension::Resistance,
        Dimension::Capacitance,
        Dimension::Inductance,
        Dimension::Length,
        Dimension::Current,
        Dimension::Frequency,
    ]
    .iter()
    .flat_map(|dimension| dimension.units().iter().map(|(name, _)| *name))
    .collect();
    units.sort();
    units.dedup();
    units
}

/// `Kind.of(text, unit)` for a known dimension.
pub fn quantity_of(dimension: Dimension, value: &str, unit: &str) -> Quantity {
    let number = Decimal::parse(value).expect("source numbers are valid decimals");
    Quantity::of(dimension, &number, unit).expect("the unit belongs to the dimension")
}

/// `_quantity`.
pub fn quantity(value: &Scalar, location: &Location) -> Result<Quantity> {
    let Scalar::Quantity { value, unit } = value else {
        return Err(error("CMP015", "expected a quantity with a unit", location));
    };
    let Some(dimension) = quantity_dimension(unit) else {
        return Err(error(
            "CMP016",
            format!("unknown unit {}; expected one of: {}", repr(unit), quantity_units().join(", ")),
            location,
        ));
    };
    Ok(quantity_of(dimension, value, unit))
}

pub fn is_length(quantity: &Quantity) -> bool {
    quantity.dimension() == Dimension::Length
}

pub fn is_voltage(quantity: &Quantity) -> bool {
    quantity.dimension() == Dimension::Voltage
}

/// `_string_value`.
pub fn string_value<'a>(value: &'a Scalar, label: &str, location: &Location) -> Result<&'a str> {
    match value {
        Scalar::Str(text) => Ok(text),
        _ => Err(error("CMP086", format!("{label} must be a string or identifier"), location)),
    }
}

fn optional_string(value: Option<&Scalar>, label: &str, location: &Location) -> Result<Option<String>> {
    value.map(|value| string_value(value, label, location).map(str::to_owned)).transpose()
}

/// `_csv_names`.
pub fn csv_names(value: &Scalar, label: &str, location: &Location) -> Result<Vec<String>> {
    let text = string_value(value, label, location)?;
    if text.chars().all(python_isspace) {
        return Ok(Vec::new());
    }
    let names: Vec<String> = text
        .split(',')
        .map(|item| item.trim_matches(python_isspace))
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    let unique: BTreeSet<&String> = names.iter().collect();
    if unique.len() != names.len() {
        return Err(error("CMP085", format!("{label} contains a duplicate name"), location));
    }
    Ok(names)
}

fn empty() -> Scalar {
    Scalar::Str(String::new())
}

/// `_key_values`: `NAME=VALUE` entries.
pub fn key_values(value: &Scalar, label: &str, location: &Location) -> Result<IndexMap<String, String>> {
    let mut result = IndexMap::new();
    for item in csv_names(value, label, location)? {
        let (key, selected) = item.split_once('=').unwrap_or((&item, ""));
        if !item.contains('=') || key.is_empty() || selected.is_empty() {
            return Err(error("CMP101", format!("{label} entries must use NAME=VALUE"), location));
        }
        if result.contains_key(key) {
            return Err(error("CMP101", format!("{label} contains duplicate key {}", repr(key)), location));
        }
        result.insert(key.to_owned(), selected.to_owned());
    }
    Ok(result)
}

pub fn condition(value: Option<&Scalar>, location: &Location) -> Result<Option<Condition>> {
    value.map(|value| key_values(value, "condition", location).map(|selections| Condition { selections })).transpose()
}

/// `_required`.
pub fn required<'a>(attributes: &'a Attributes, name: &str, location: &Location) -> Result<&'a Scalar> {
    attributes.get(name).ok_or_else(|| error("CMP017", format!("missing required property {}", repr(name)), location))
}

/// `_reject_unknown_attributes`, with a "did you mean" or the allowed list.
pub fn reject_unknown<K: AsRef<str>>(
    keys: impl IntoIterator<Item = K>,
    allowed: &[&str],
    location: &Location,
) -> Result<()> {
    let mut unknown: Vec<String> =
        keys.into_iter().map(|key| key.as_ref().to_owned()).filter(|key| !allowed.contains(&key.as_str())).collect();
    unknown.sort();
    let Some(first) = unknown.first() else { return Ok(()) };
    let mut sorted: Vec<&str> = allowed.to_vec();
    sorted.sort();
    sorted.dedup();
    let suggestion = close_matches(first, &sorted, 1, 0.6).into_iter().next();
    let mut failure = error("CMP018", format!("unknown property {}", repr(first)), location);
    failure.hint = match suggestion {
        Some(word) => Some(format!("did you mean {}?", repr(&word))),
        None if sorted.len() <= 12 => Some(format!("allowed: {}", sorted.join(", "))),
        None => None,
    };
    Err(failure)
}

fn optional_voltage(value: Option<&Scalar>, location: &Location) -> Result<Option<Quantity>> {
    let Some(value) = value else { return Ok(None) };
    let compiled = quantity(value, location)?;
    if !is_voltage(&compiled) {
        return Err(error("CMP051", "pin voltage limits must use V or mV", location));
    }
    Ok(Some(compiled))
}

/// `_voltage_range`.
pub fn voltage_range(attributes: &Attributes, location: &Location) -> Result<Option<QuantityRange>> {
    let minimum = optional_voltage(attributes.get("voltage_min"), location)?;
    let maximum = optional_voltage(attributes.get("voltage_max"), location)?;
    if minimum.is_none() && maximum.is_none() {
        return Ok(None);
    }
    Ok(Some(QuantityRange::new(minimum.map(Bound::Fixed), maximum.map(Bound::Fixed), "operating")))
}

fn trim_space(text: &str) -> &str {
    text.trim_matches(python_isspace)
}

/// `[+-]?\d+(?:\.\d+)?` at the start of `text`: (number, rest).
fn leading_number(text: &str, signed: bool) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut index = 0;
    if signed && matches!(bytes.first(), Some(b'+' | b'-')) {
        index = 1;
    }
    let digits_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    if index == digits_start {
        return None;
    }
    if bytes.get(index) == Some(&b'.') && bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    Some((&text[..index], &text[index..]))
}

/// `\s*(?P<number>[+-]?\d+(?:\.\d+)?)\s*(?P<unit>[A-Za-z]+)\s*` (fullmatch).
fn fixed_limit(term: &str) -> Option<(String, String)> {
    let (number, rest) = leading_number(trim_space(term), true)?;
    let unit = rest.trim_start_matches(python_isspace);
    (!unit.is_empty() && unit.bytes().all(|b| b.is_ascii_alphabetic())).then(|| (number.to_owned(), unit.to_owned()))
}

/// A relative limit's signed offset: (sign, number, unit).
type Offset = (char, String, String);

/// The optional `[+-] number unit` tail of a relative limit.
fn relative_tail(rest: &str) -> Option<Option<Offset>> {
    let rest = trim_space(rest);
    if rest.is_empty() {
        return Some(None);
    }
    let sign = rest.chars().next()?;
    if sign != '+' && sign != '-' {
        return None;
    }
    let (number, after) = leading_number(rest[1..].trim_start_matches(python_isspace), false)?;
    let unit = after.trim_start_matches(python_isspace);
    (!unit.is_empty() && unit.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| Some((sign, number.to_owned(), unit.to_owned())))
}

/// `_RELATIVE_LIMIT`: the shortest pad reference whose remainder is a valid tail.
fn relative_limit(term: &str) -> Option<(String, Option<Offset>)> {
    let text = term.trim_start_matches(python_isspace);
    let mut characters = text.char_indices();
    let (_, first) = characters.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut end = first.len_utf8();
    loop {
        if let Some(tail) = relative_tail(&text[end..]) {
            return Some((text[..end].to_owned(), tail));
        }
        match text[end..].chars().next() {
            Some(next) if next.is_ascii_alphanumeric() || next == '_' || next == '-' => end += next.len_utf8(),
            _ => return None,
        }
    }
}

/// `_voltage_limit`: a fixed voltage or `"PAD[+|-]OFFSET"`, optionally capped.
fn voltage_limit(value: Option<&Scalar>, label: &str, location: &Location) -> Result<Option<Bound>> {
    let Some(value) = value else { return Ok(None) };
    if let Scalar::Quantity { .. } = value {
        let compiled = quantity(value, location)?;
        if !is_voltage(&compiled) {
            return Err(error("CMP120", format!("{label} must use V or mV"), location));
        }
        return Ok(Some(Bound::Fixed(compiled)));
    }
    let usage = format!(
        "{label} must be a voltage such as 1.35V or a pad-relative limit such as \"VTERM+0.1V\", optionally capped as \"VTERM+0.1V, 1.36V\""
    );
    let Scalar::Str(text) = value else { return Err(error("CMP120", usage, location)) };
    let terms: Vec<&str> = text.split(',').collect();
    if terms.len() > 2 {
        return Err(error("CMP120", usage, location));
    }
    let mut relative: Option<(String, Quantity)> = None;
    let mut fixed: Option<Quantity> = None;
    for term in terms {
        if let Some((number, unit)) = fixed_limit(term) {
            if fixed.is_some() || !Dimension::Voltage.units().iter().any(|(name, _)| *name == unit) {
                return Err(error("CMP120", usage, location));
            }
            fixed = Some(quantity_of(Dimension::Voltage, &number, &unit));
        } else if let Some((reference, tail)) = relative_limit(term) {
            if relative.is_some() {
                return Err(error("CMP120", usage, location));
            }
            let offset = match tail {
                None => quantity_of(Dimension::Voltage, "0", "V"),
                Some((sign, number, unit)) => {
                    if !Dimension::Voltage.units().iter().any(|(name, _)| *name == unit) {
                        return Err(error("CMP120", format!("{label} offset must use V or mV"), location));
                    }
                    let signed = if sign == '+' { number } else { format!("-{number}") };
                    quantity_of(Dimension::Voltage, &signed, &unit)
                }
            };
            relative = Some((reference, offset));
        } else {
            return Err(error("CMP120", usage, location));
        }
    }
    Ok(match relative {
        None => fixed.map(Bound::Fixed),
        Some((reference, offset)) => Some(Bound::Relative(RelativeVoltage { reference, offset, limit: fixed })),
    })
}

/// `_absolute_voltage_range`.
fn absolute_voltage_range(
    attributes: &Attributes,
    operating: Option<&QuantityRange>,
    location: &Location,
) -> Result<Option<QuantityRange>> {
    let minimum = voltage_limit(attributes.get("absolute_min"), "absolute_min", location)?;
    let maximum = voltage_limit(attributes.get("absolute_max"), "absolute_max", location)?;
    if minimum.is_none() && maximum.is_none() {
        return Ok(None);
    }
    let fixed = |bound: &Option<Bound>| match bound {
        Some(Bound::Fixed(quantity)) => Some(quantity.clone()),
        Some(Bound::Relative(relative)) => relative.limit.clone(),
        None => None,
    };
    let (fixed_minimum, fixed_maximum) = (fixed(&minimum), fixed(&maximum));
    if let (Some(low), Some(high)) = (&fixed_minimum, &fixed_maximum)
        && high.base_value() < low.base_value()
    {
        return Err(error("CMP121", format!("absolute_min {low} exceeds absolute_max {high}"), location));
    }
    if let Some(operating) = operating {
        if let (Some(low), Some(Bound::Fixed(minimum))) = (&fixed_minimum, &operating.minimum)
            && minimum.base_value() < low.base_value()
        {
            return Err(error("CMP121", format!("voltage_min {minimum} is below absolute_min {low}"), location));
        }
        if let (Some(high), Some(Bound::Fixed(maximum))) = (&fixed_maximum, &operating.maximum)
            && high.base_value() < maximum.base_value()
        {
            return Err(error("CMP121", format!("voltage_max {maximum} exceeds absolute_max {high}"), location));
        }
    }
    Ok(Some(QuantityRange::new(minimum, maximum, "absolute")))
}

pub fn relative_limit_references(profile: Option<&ElectricalProfile>) -> Vec<String> {
    let Some(absolute) = profile.and_then(|profile| profile.absolute_voltage.as_ref()) else { return Vec::new() };
    [&absolute.minimum, &absolute.maximum]
        .into_iter()
        .filter_map(|bound| match bound {
            Some(Bound::Relative(relative)) => Some(relative.reference.clone()),
            _ => None,
        })
        .collect()
}

fn parse_enum<T>(
    names: &[String],
    parse: fn(&str) -> Option<T>,
    invalid: fn(&str) -> String,
) -> std::result::Result<Vec<T>, String> {
    names.iter().map(|name| parse(name).ok_or_else(|| invalid(name))).collect()
}

/// `_electrical_profile`.
pub fn electrical_profile(attributes: &Attributes, location: &Location) -> Result<ElectricalProfile> {
    let enums = (|| -> Result<std::result::Result<_, String>> {
        let domains = csv_names(required(attributes, "domains", location)?, "signal domains", location)?;
        let domains = match parse_enum(&domains, SignalDomain::parse, SignalDomain::invalid) {
            Ok(values) => values,
            Err(message) => return Ok(Err(message)),
        };
        let directions = csv_names(required(attributes, "directions", location)?, "directions", location)?;
        let directions = match parse_enum(&directions, Direction::parse, Direction::invalid) {
            Ok(values) => values,
            Err(message) => return Ok(Err(message)),
        };
        let drive = csv_names(attributes.get("drive_modes").unwrap_or(&empty()), "drive modes", location)?;
        let drive = match parse_enum(&drive, DriveMode::parse, DriveMode::invalid) {
            Ok(values) => values,
            Err(message) => return Ok(Err(message)),
        };
        Ok(Ok((domains, directions, drive)))
    })()?;
    let (domains, directions, drive_modes) = enums.map_err(|message| error("CMP084", message, location))?;
    let operating = voltage_range(attributes, location)?;
    let traits = csv_names(attributes.get("traits").unwrap_or(&empty()), "electrical traits", location)?;
    let absolute_voltage = absolute_voltage_range(attributes, operating.as_ref(), location)?;
    Ok(ElectricalProfile {
        domains: domains.into_iter().collect(),
        directions: directions.into_iter().collect(),
        drive_modes: drive_modes.into_iter().collect(),
        traits: traits.into_iter().collect(),
        voltage: operating,
        current: None,
        absolute_voltage,
    })
}

/// `_profile_for_pin_type`.
fn profile_for_pin_type(value: &str, location: &Location) -> Result<ElectricalProfile> {
    use {Direction as D, DriveMode as M, SignalDomain as S};
    Ok(match PinType::parse(value) {
        Some(PinType::Passive) => ElectricalProfile::simple(&[S::Analog], &[D::Passive], &[]),
        Some(PinType::Input) => ElectricalProfile::simple(&[S::Digital], &[D::Input], &[]),
        Some(PinType::Output) => ElectricalProfile::simple(&[S::Digital], &[D::Output], &[M::PushPull]),
        Some(PinType::Bidirectional) => ElectricalProfile::simple(&[S::Digital], &[D::Bidirectional], &[M::PushPull]),
        Some(PinType::OpenDrain) => ElectricalProfile::simple(&[S::Digital], &[D::Bidirectional], &[M::OpenDrain]),
        Some(PinType::PowerIn) => ElectricalProfile::simple(&[S::Power], &[D::Input], &[]),
        Some(PinType::PowerOut) => ElectricalProfile::simple(&[S::Power], &[D::Output], &[]),
        None => return Err(error("CMP060", format!("unknown signal electrical profile {}", repr(value)), location)),
    })
}

/// `_source_reference` from `source_*` properties.
fn source_reference(properties: &IndexMap<String, Scalar>, location: &Location) -> Result<Option<SourceReference>> {
    let fields = ["document", "revision", "location", "url", "checksum"];
    let values: Vec<Option<&Scalar>> = fields.iter().map(|field| properties.get(&format!("source_{field}"))).collect();
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let mut texts = Vec::new();
    for (field, value) in fields.iter().zip(values) {
        texts.push(optional_string(value, &format!("source_{field}"), location)?);
    }
    let mut texts = texts.into_iter();
    Ok(Some(SourceReference {
        document: texts.next().flatten(),
        revision: texts.next().flatten(),
        location: texts.next().flatten(),
        url: texts.next().flatten(),
        checksum: texts.next().flatten(),
    }))
}

/// `_compile_device`.
pub fn compile_device(document: &Document, qualified_name: &str) -> Result<DeviceDefinition> {
    let mut properties: IndexMap<String, String> = IndexMap::new();
    let mut raw_properties: IndexMap<String, Scalar> = IndexMap::new();
    let mut pads: IndexMap<String, DevicePadDefinition> = IndexMap::new();
    let mut power_domains: IndexMap<String, PowerDomainDefinition> = IndexMap::new();
    let mut peripherals: IndexMap<String, PeripheralDefinition> = IndexMap::new();
    let mut resources: Vec<String> = Vec::new();
    let mut mux_options: Vec<MuxOption> = Vec::new();
    let mut units: IndexMap<String, FunctionalUnitDefinition> = IndexMap::new();
    let mut groups: IndexMap<String, SignalGroupDefinition> = IndexMap::new();
    let mut mode_groups: IndexMap<String, ModeGroupDefinition> = IndexMap::new();
    let mut pad_sets: IndexMap<String, PadSetDefinition> = IndexMap::new();
    let mut route_rules: Vec<RouteRule> = Vec::new();
    for declaration in &document.declarations {
        match declaration {
            Declaration::DeviceProperty(property) => {
                if properties.contains_key(&property.name) {
                    return Err(error(
                        "CMP054",
                        format!("duplicate device property {}", repr(&property.name)),
                        &property.location,
                    ));
                }
                let Scalar::Str(text) = &property.value else {
                    return Err(error(
                        "CMP055",
                        "device metadata values must be strings or identifiers",
                        &property.location,
                    ));
                };
                properties.insert(property.name.clone(), text.clone());
                raw_properties.insert(property.name.clone(), property.value.clone());
            }
            Declaration::Pad(pad) => {
                let location = &pad.location;
                if pads.contains_key(&pad.name) {
                    return Err(error("CMP073", format!("duplicate device pad {}", repr(&pad.name)), location));
                }
                reject_unknown(
                    pad.attributes.keys(),
                    &[
                        "domains",
                        "directions",
                        "drive_modes",
                        "traits",
                        "power_domain",
                        "unpowered",
                        "voltage_min",
                        "voltage_max",
                        "absolute_min",
                        "absolute_max",
                        "when",
                    ],
                    location,
                )?;
                let profile = electrical_profile(&pad.attributes, location)?;
                let power_domain = optional_string(pad.attributes.get("power_domain"), "pad power_domain", location)?;
                let default = Scalar::Str(UnpoweredBehavior::Unknown.value().to_owned());
                let raw = string_value(
                    pad.attributes.get("unpowered").unwrap_or(&default),
                    "pad unpowered behavior",
                    location,
                )?;
                let Some(unpowered) = UnpoweredBehavior::parse(raw) else {
                    let allowed: Vec<&str> = UnpoweredBehavior::ALL.iter().map(|item| item.value()).collect();
                    return Err(error(
                        "CMP074",
                        format!("unknown unpowered behavior {}; expected: {}", repr(raw), allowed.join(", ")),
                        location,
                    ));
                };
                let when = condition(pad.attributes.get("when"), location)?;
                pads.insert(
                    pad.name.clone(),
                    DevicePadDefinition {
                        name: pad.name.clone(),
                        profile,
                        power_domain,
                        unpowered_behavior: unpowered,
                        when,
                    },
                );
            }
            Declaration::PowerDomain(domain) => {
                let location = &domain.location;
                if power_domains.contains_key(&domain.name) {
                    return Err(error("CMP075", format!("duplicate power domain {}", repr(&domain.name)), location));
                }
                reject_unknown(
                    domain.attributes.keys(),
                    &["supply_pads", "voltage_min", "voltage_max", "requires"],
                    location,
                )?;
                let supply_pads = csv_names(
                    required(&domain.attributes, "supply_pads", location)?,
                    "power-domain supply_pads",
                    location,
                )?;
                let voltage = voltage_range(&domain.attributes, location)?;
                let requires =
                    csv_names(domain.attributes.get("requires").unwrap_or(&empty()), "domain requirements", location)?;
                power_domains.insert(
                    domain.name.clone(),
                    PowerDomainDefinition { name: domain.name.clone(), supply_pads, voltage, requires },
                );
            }
            Declaration::Resource(resource) => {
                if resources.contains(&resource.name) {
                    return Err(error(
                        "CMP056",
                        format!("duplicate device resource {}", repr(&resource.name)),
                        &resource.location,
                    ));
                }
                resources.push(resource.name.clone());
            }
            Declaration::Peripheral(peripheral) => {
                if peripherals.contains_key(&peripheral.name) {
                    return Err(error(
                        "CMP057",
                        format!("duplicate peripheral {}", repr(&peripheral.name)),
                        &peripheral.location,
                    ));
                }
                let mut signals = IndexMap::new();
                for signal in &peripheral.signals {
                    if signals.contains_key(&signal.name) {
                        return Err(error(
                            "CMP058",
                            format!("duplicate peripheral signal {}", repr(&signal.name)),
                            &signal.location,
                        ));
                    }
                    reject_unknown(signal.attributes.keys(), &["required"], &signal.location)?;
                    let required = match signal.attributes.get("required") {
                        None => true,
                        Some(Scalar::Bool(flag)) => *flag,
                        Some(_) => {
                            return Err(error(
                                "CMP059",
                                "signal required property must be true or false",
                                &signal.location,
                            ));
                        }
                    };
                    let profile = profile_for_pin_type(&signal.pin_type, &signal.location)?;
                    signals.insert(
                        signal.name.clone(),
                        PeripheralSignalDefinition { name: signal.name.clone(), profile, required },
                    );
                }
                if signals.is_empty() {
                    return Err(error("CMP061", "peripheral must declare at least one signal", &peripheral.location));
                }
                peripherals.insert(
                    peripheral.name.clone(),
                    PeripheralDefinition { name: peripheral.name.clone(), kind: peripheral.kind.clone(), signals },
                );
            }
            Declaration::Mux(mux) => {
                let location = &mux.location;
                reject_unknown(mux.attributes.keys(), &["selector", "resource", "setting", "when"], location)?;
                let selector = required(&mux.attributes, "selector", location)?;
                let resource = mux.attributes.get("resource");
                let setting = mux.attributes.get("setting");
                let Scalar::Str(selector) = selector else {
                    return Err(error("CMP062", "mux selector must be a string or identifier", location));
                };
                let text = |value: Option<&Scalar>| match value {
                    Some(Scalar::Str(text)) => Ok(Some(text.clone())),
                    None => Ok(None),
                    Some(_) => Err(()),
                };
                let resource =
                    text(resource).map_err(|()| error("CMP063", "mux resource must be an identifier", location))?;
                let setting = text(setting)
                    .map_err(|()| error("CMP064", "mux setting must be a string or identifier", location))?;
                if resource.is_none() != setting.is_none() {
                    return Err(error("CMP065", "mux resource and setting must be specified together", location));
                }
                let when = condition(mux.attributes.get("when"), location)?;
                mux_options.push(MuxOption {
                    pad: mux.pad.clone(),
                    peripheral: mux.peripheral.clone(),
                    signal: mux.signal.clone(),
                    selector: selector.clone(),
                    resource,
                    setting,
                    when,
                });
            }
            Declaration::Unit(unit) => {
                let location = &unit.location;
                if units.contains_key(&unit.name) {
                    return Err(error("CMP088", format!("duplicate functional unit {}", repr(&unit.name)), location));
                }
                let mut raw = unit.attributes.clone();
                let shared = match raw.shift_remove("shared") {
                    None => false,
                    Some(Scalar::Bool(flag)) => flag,
                    Some(_) => return Err(error("CMP089", "unit shared property must be true or false", location)),
                };
                let mut terminals = IndexMap::new();
                for (name, value) in &raw {
                    let pad = string_value(value, "terminal pad", location)?.to_owned();
                    terminals.insert(name.clone(), TerminalBinding { pad, profile: None });
                }
                units.insert(
                    unit.name.clone(),
                    FunctionalUnitDefinition { name: unit.name.clone(), kind: unit.kind.clone(), terminals, shared },
                );
            }
            Declaration::SignalGroup(group) => {
                let location = &group.location;
                if groups.contains_key(&group.name) {
                    return Err(error("CMP090", format!("duplicate signal group {}", repr(&group.name)), location));
                }
                let mut raw = group.attributes.clone();
                let when = condition(raw.shift_remove("when").as_ref(), location)?;
                let mut members = IndexMap::new();
                for (name, value) in &raw {
                    members.insert(name.clone(), string_value(value, "signal-group member", location)?.to_owned());
                }
                groups.insert(
                    group.name.clone(),
                    SignalGroupDefinition {
                        name: group.name.clone(),
                        kind: group.kind.clone(),
                        members,
                        profile: None,
                        when,
                    },
                );
            }
            Declaration::ModeGroup(group) => {
                let location = &group.location;
                if mode_groups.contains_key(&group.name) {
                    return Err(error("CMP091", format!("duplicate mode group {}", repr(&group.name)), location));
                }
                reject_unknown(group.attributes.keys(), &["choices", "default"], location)?;
                let choices = csv_names(required(&group.attributes, "choices", location)?, "mode choices", location)?;
                let default = optional_string(group.attributes.get("default"), "mode default", location)?;
                if let Some(default) = &default
                    && !choices.contains(default)
                {
                    return Err(error(
                        "CMP092",
                        format!("mode default {} is not a declared choice", repr(default)),
                        location,
                    ));
                }
                mode_groups
                    .insert(group.name.clone(), ModeGroupDefinition { name: group.name.clone(), choices, default });
            }
            Declaration::PadSet(pad_set) => {
                let location = &pad_set.location;
                if pad_sets.contains_key(&pad_set.name) {
                    return Err(error("CMP093", format!("duplicate pad set {}", repr(&pad_set.name)), location));
                }
                reject_unknown(pad_set.attributes.keys(), &["pads"], location)?;
                let pads = csv_names(required(&pad_set.attributes, "pads", location)?, "pad set", location)?;
                pad_sets.insert(pad_set.name.clone(), PadSetDefinition { name: pad_set.name.clone(), pads });
            }
            Declaration::RouteRule(rule) => {
                let location = &rule.location;
                reject_unknown(rule.attributes.keys(), &["pad_set", "selector", "parameters", "when"], location)?;
                let pad_set =
                    string_value(required(&rule.attributes, "pad_set", location)?, "route pad_set", location)?
                        .to_owned();
                let kind =
                    string_value(required(&rule.attributes, "selector", location)?, "selector scheme", location)?
                        .to_owned();
                let parameters =
                    key_values(rule.attributes.get("parameters").unwrap_or(&empty()), "selector parameters", location)?;
                let when = condition(rule.attributes.get("when"), location)?;
                route_rules.push(RouteRule {
                    peripheral: rule.peripheral.clone(),
                    signal: rule.signal.clone(),
                    pad_set,
                    selector: SelectorScheme { kind, parameters },
                    when,
                });
            }
            _ => {
                return Err(error(
                    "CMP066",
                    "device files may only contain metadata, pads, power domains, peripherals, resources, and mux options",
                    &document.location,
                ));
            }
        }
    }
    let location = &document.location;
    for mux in &mux_options {
        if !pads.contains_key(&mux.pad) {
            return Err(error("CMP076", format!("mux references unknown device pad {}", repr(&mux.pad)), location));
        }
        let Some(peripheral) = peripherals.get(&mux.peripheral) else {
            return Err(error(
                "CMP067",
                format!("mux references unknown peripheral {}", repr(&mux.peripheral)),
                location,
            ));
        };
        if !peripheral.signals.contains_key(&mux.signal) {
            return Err(error(
                "CMP068",
                format!("mux references unknown signal {}.{}", mux.peripheral, mux.signal),
                location,
            ));
        }
        if let Some(resource) = &mux.resource
            && !resources.contains(resource)
        {
            return Err(error("CMP069", format!("mux references undeclared resource {}", repr(resource)), location));
        }
    }
    for pad in pads.values() {
        for reference in relative_limit_references(Some(&pad.profile)) {
            if !pads.contains_key(&reference) {
                return Err(error(
                    "CMP122",
                    format!("pad {} voltage limit references unknown device pad {}", repr(&pad.name), repr(&reference)),
                    location,
                ));
            }
        }
        if let Some(domain) = &pad.power_domain
            && !power_domains.contains_key(domain)
        {
            return Err(error(
                "CMP077",
                format!("pad {} references unknown power domain {}", repr(&pad.name), repr(domain)),
                location,
            ));
        }
    }
    for domain in power_domains.values() {
        let mut unknown: Vec<&String> = domain.supply_pads.iter().filter(|pad| !pads.contains_key(*pad)).collect();
        unknown.sort();
        if let Some(pad) = unknown.first() {
            return Err(error(
                "CMP078",
                format!("power domain {} references unknown supply pad {}", repr(&domain.name), repr(pad)),
                location,
            ));
        }
    }
    for unit in units.values() {
        for terminal in unit.terminals.values() {
            if !pads.contains_key(&terminal.pad) {
                return Err(error(
                    "CMP094",
                    format!("unit {} references unknown pad {}", repr(&unit.name), repr(&terminal.pad)),
                    location,
                ));
            }
        }
    }
    for group in groups.values() {
        let mut unknown: Vec<&String> = group.members.values().filter(|pad| !pads.contains_key(*pad)).collect();
        unknown.sort();
        unknown.dedup();
        if let Some(pad) = unknown.first() {
            return Err(error(
                "CMP095",
                format!("signal group {} references unknown pad {}", repr(&group.name), repr(pad)),
                location,
            ));
        }
        let members: BTreeSet<&str> = group.members.keys().map(String::as_str).collect();
        if group.kind == DIFFERENTIAL_PAIR && members != BTreeSet::from(["negative", "positive"]) {
            return Err(error(
                "CMP096",
                format!("differential group {} requires positive and negative members", repr(&group.name)),
                location,
            ));
        }
    }
    for pad_set in pad_sets.values() {
        let mut unknown: Vec<&String> = pad_set.pads.iter().filter(|pad| !pads.contains_key(*pad)).collect();
        unknown.sort();
        if let Some(pad) = unknown.first() {
            return Err(error(
                "CMP097",
                format!("pad set {} references unknown pad {}", repr(&pad_set.name), repr(pad)),
                location,
            ));
        }
    }
    for route in &route_rules {
        let known =
            peripherals.get(&route.peripheral).is_some_and(|peripheral| peripheral.signals.contains_key(&route.signal));
        if !known {
            return Err(error(
                "CMP098",
                format!("route references unknown signal {}.{}", route.peripheral, route.signal),
                location,
            ));
        }
        if !pad_sets.contains_key(&route.pad_set) {
            return Err(error(
                "CMP099",
                format!("route references unknown pad set {}", repr(&route.pad_set)),
                location,
            ));
        }
    }
    let keys: BTreeSet<(&str, &str, &str)> =
        mux_options.iter().map(|mux| (mux.pad.as_str(), mux.peripheral.as_str(), mux.signal.as_str())).collect();
    if keys.len() != mux_options.len() {
        return Err(error("CMP070", "device contains a duplicate mux option", location));
    }
    let source = source_reference(&raw_properties, location)?;
    Ok(DeviceDefinition {
        name: qualified_name.to_owned(),
        pads,
        peripherals,
        mux_options,
        power_domains,
        units,
        signal_groups: groups,
        mode_groups,
        pad_sets,
        route_rules,
        resources,
        metadata: properties.into_iter().filter(|(key, _)| !key.starts_with("source_")).collect(),
        source,
    })
}

fn is_parallel_group_name(text: &str) -> bool {
    let mut characters = text.chars();
    characters.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && characters.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// `_compile_part`.
pub fn compile_part(
    document: &Document,
    qualified_name: &str,
    resolve_device: &dyn Fn(&str) -> String,
    devices: &IndexMap<String, DeviceDefinition>,
) -> Result<PartDefinition> {
    let mut properties: IndexMap<String, Scalar> = IndexMap::new();
    let mut pins: IndexMap<String, PackagePinDefinition> = IndexMap::new();
    for declaration in &document.declarations {
        match declaration {
            Declaration::PartProperty(property) => {
                if properties.contains_key(&property.name) {
                    return Err(error(
                        "CMP042",
                        format!("duplicate part property {}", repr(&property.name)),
                        &property.location,
                    ));
                }
                properties.insert(property.name.clone(), property.value.clone());
            }
            Declaration::Pin(pin) => {
                let location = &pin.location;
                let attributes = &pin.attributes;
                if pins.contains_key(&pin.name) {
                    return Err(error("CMP043", format!("duplicate pin {}", repr(&pin.name)), location));
                }
                reject_unknown(
                    attributes.keys(),
                    &[
                        "number",
                        "domains",
                        "directions",
                        "drive_modes",
                        "traits",
                        "bond",
                        "voltage_min",
                        "voltage_max",
                        "absolute_min",
                        "absolute_max",
                        "connection",
                        "required_net_traits",
                        "parallel_power_group",
                        "when",
                    ],
                    location,
                )?;
                let Scalar::Str(number) = required(attributes, "number", location)? else {
                    return Err(error("CMP044", "pin number must be a string", location));
                };
                let bonds = csv_names(attributes.get("bond").unwrap_or(&empty()), "pin bond", location)?;
                let default = Scalar::Str(ConnectionPolicy::Normal.value().to_owned());
                let policy_text =
                    string_value(attributes.get("connection").unwrap_or(&default), "connection policy", location)?;
                let Some(policy) = ConnectionPolicy::parse(policy_text) else {
                    return Err(error("CMP100", "unknown package-pin connection policy", location));
                };
                let has_profile = [
                    "domains",
                    "directions",
                    "drive_modes",
                    "traits",
                    "voltage_min",
                    "voltage_max",
                    "absolute_min",
                    "absolute_max",
                ]
                .iter()
                .any(|name| attributes.contains_key(*name));
                let profile = if has_profile { Some(electrical_profile(attributes, location)?) } else { None };
                let parallel = attributes.get("parallel_power_group");
                if let Some(parallel) = parallel {
                    let valid = matches!(parallel, Scalar::Str(text) if is_parallel_group_name(text))
                        && profile.as_ref().is_some_and(|profile| {
                            profile.domains == BTreeSet::from([SignalDomain::Power])
                                && profile.directions == BTreeSet::from([Direction::Output])
                        });
                    if !valid {
                        return Err(error(
                            "CMP200",
                            "parallel_power_group requires a named explicit power-only output",
                            location,
                        ));
                    }
                }
                let bond_condition = condition(attributes.get("when"), location)?;
                let mut compiled = Vec::new();
                for bond in &bonds {
                    let (pad, raw) = bond.split_once('@').unwrap_or((bond, ""));
                    let when = if bond.contains('@') {
                        condition(Some(&Scalar::Str(raw.to_owned())), location)?
                    } else {
                        bond_condition.clone()
                    };
                    compiled.push(BondDefinition { pad: pad.to_owned(), when });
                }
                let traits = csv_names(
                    attributes.get("required_net_traits").unwrap_or(&empty()),
                    "required net traits",
                    location,
                )?;
                pins.insert(
                    pin.name.clone(),
                    PackagePinDefinition {
                        name: pin.name.clone(),
                        number: number.clone(),
                        profile,
                        bonds: compiled,
                        connection_policy: policy,
                        required_net_traits: traits.into_iter().collect(),
                        parallel_power_group: match parallel {
                            Some(Scalar::Str(text)) => Some(text.clone()),
                            _ => None,
                        },
                    },
                );
            }
            _ => return Err(error("CMP046", "part files may only contain properties and pins", &document.location)),
        }
    }
    let location = &document.location;
    reject_unknown(
        properties.keys(),
        &[
            "category",
            "traits",
            "manufacturer",
            "assembled",
            "footprint",
            "device",
            "source_document",
            "source_revision",
            "source_location",
            "source_url",
            "source_checksum",
            "internal_pad_groups",
            "dc_power_path",
        ],
        location,
    )?;
    let default_category = Scalar::Str("component.generic".to_owned());
    let category =
        string_value(properties.get("category").unwrap_or(&default_category), "part category", location)?.to_owned();
    let traits = csv_names(properties.get("traits").unwrap_or(&empty()), "part traits", location)?;
    let manufacturer = match properties.get("manufacturer") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP048", "part manufacturer must be a string", location)),
    };
    let assembled = match properties.get("assembled") {
        None => true,
        Some(Scalar::Bool(flag)) => *flag,
        Some(_) => return Err(error("CMP048", "part assembled must be a boolean", location)),
    };
    let footprint = match properties.get("footprint") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP049", "part footprint must be a string", location)),
    };
    let device = match properties.get("device") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP071", "part device must be an identifier", location)),
    };
    let resolved_device = device.as_deref().filter(|name| !name.is_empty()).map(resolve_device);
    let device_definition = devices.get(resolved_device.as_deref().unwrap_or(""));
    for pin in pins.values() {
        if resolved_device.is_some() && pin.bonds.is_empty() && pin.connection_policy != ConnectionPolicy::DoNotConnect
        {
            return Err(error("CMP079", format!("device-backed pin {} must declare bond", repr(&pin.name)), location));
        }
        if resolved_device.is_none()
            && pin.profile.is_none()
            && !matches!(pin.connection_policy, ConnectionPolicy::DoNotConnect | ConnectionPolicy::Optional)
        {
            return Err(error(
                "CMP080",
                format!("standalone pin {} must declare an electrical profile", repr(&pin.name)),
                location,
            ));
        }
        if let Some(device) = device_definition {
            let mut unknown: Vec<&String> =
                pin.bonds.iter().map(|bond| &bond.pad).filter(|pad| !device.pads.contains_key(*pad)).collect();
            unknown.sort();
            if let Some(pad) = unknown.first() {
                return Err(error(
                    "CMP081",
                    format!("pin {} bonds unknown device pad {}", repr(&pin.name), repr(pad)),
                    location,
                ));
            }
        }
        for reference in relative_limit_references(pin.profile.as_ref()) {
            let known = pins.contains_key(&reference)
                || device_definition.is_some_and(|device| device.pads.contains_key(&reference));
            if !known && (resolved_device.is_none() || device_definition.is_some()) {
                return Err(error(
                    "CMP122",
                    format!(
                        "pin {} voltage limit references unknown pin or device pad {}",
                        repr(&pin.name),
                        repr(&reference)
                    ),
                    location,
                ));
            }
        }
    }
    if pins.is_empty() {
        return Err(error("CMP050", "part must declare at least one pin", location));
    }
    let numbers: Vec<String> = pins.values().map(|pin| pin.number.clone()).collect();
    let unique: BTreeSet<&String> = numbers.iter().collect();
    if unique.len() != numbers.len() {
        return Err(error("CMP087", "part contains duplicate physical pin numbers", location));
    }
    let raw_groups = match properties.get("internal_pad_groups") {
        None => String::new(),
        Some(Scalar::Str(text)) => text.clone(),
        Some(_) => return Err(error("CMP088", "internal_pad_groups must be a quoted string", location)),
    };
    let groups = if raw_groups.is_empty() {
        Vec::new()
    } else {
        let mut groups = Vec::new();
        for value in raw_groups.split(';') {
            let numbers: Vec<String> =
                value.replace(',', " ").split(python_isspace).filter(|n| !n.is_empty()).map(str::to_owned).collect();
            groups.push(InternalPadGroup::new(numbers).map_err(|message| error("CMP088", message, location))?);
        }
        validate_internal_pad_groups(groups, &numbers).map_err(|message| error("CMP088", message, location))?
    };
    let dc_power_path = csv_names(properties.get("dc_power_path").unwrap_or(&empty()), "dc_power_path", location)?;
    let mut part = PartDefinition::new(qualified_name, pins);
    part.device = resolved_device;
    part.dc_power_path = dc_power_path;
    part.validate_dc_power_path().map_err(|message| error("CMP201", message, location))?;
    part.category = category;
    part.traits = traits.into_iter().collect();
    part.footprints = footprint.into_iter().collect();
    part.manufacturer = manufacturer;
    part.assembled = assembled;
    part.source = source_reference(&properties, location)?;
    part.internal_pad_groups = groups;
    Ok(part)
}

/// `_component`.
pub fn component(declaration: &ComponentDecl, part: &str) -> Result<ComponentInstance> {
    let location = &declaration.location;
    let attributes = &declaration.attributes;
    reject_unknown(attributes.keys(), &["value", "footprint", "modes", "role", "decouples"], location)?;
    let value = match attributes.get("value") {
        Some(raw @ Scalar::Quantity { .. }) => Some(ComponentValue::Quantity(quantity(raw, location)?)),
        Some(Scalar::Str(text)) => Some(ComponentValue::Text(text.clone())),
        None => None,
        Some(_) => return Err(error("CMP004", "component value must be a quantity or string", location)),
    };
    let footprint = match attributes.get("footprint") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP005", "component footprint must be a string", location)),
    };
    let modes = key_values(attributes.get("modes").unwrap_or(&empty()), "component modes", location)?;
    let role = attributes.get("role");
    let target = attributes.get("decouples");
    let mut properties = IndexMap::new();
    if role.is_some() || target.is_some() {
        let (Some(Scalar::Str(role)), Some(Scalar::Str(target))) = (role, target) else {
            return Err(error(
                "CMP005",
                "decoupling requires role = decoupling and decouples = COMPONENT.PIN",
                location,
            ));
        };
        if role != "decoupling" {
            return Err(error(
                "CMP005",
                "decoupling requires role = decoupling and decouples = COMPONENT.PIN",
                location,
            ));
        }
        Endpoint::parse(target).map_err(|message| error("CMP005", message, location))?;
        properties.insert("role".to_owned(), role.clone());
        properties.insert("decouples".to_owned(), target.clone());
    }
    Ok(ComponentInstance {
        reference: declaration.reference.clone(),
        part: part.to_owned(),
        value,
        footprint,
        properties,
        modes,
    })
}

/// `_no_connects`.
pub fn no_connects(declaration: &NoConnectDecl) -> Result<Vec<NoConnect>> {
    reject_unknown(declaration.attributes.keys(), &["reason"], &declaration.location)?;
    let reason = match declaration.attributes.get("reason") {
        None => None,
        Some(Scalar::Str(text)) if !text.chars().all(python_isspace) => Some(text.clone()),
        Some(_) => return Err(error("CMP124", "no_connect reason must be a non-empty string", &declaration.location)),
    };
    let mut result = Vec::new();
    for (index, text) in declaration.endpoints.iter().enumerate() {
        let location = declaration.endpoint_locations.get(index).unwrap_or(&declaration.location);
        let endpoint = Endpoint::parse(text).map_err(|message| error("CMP125", message, location))?;
        if endpoint.component == "port" {
            return Err(error(
                "CMP125",
                "no_connect cannot target a module boundary port; every port must be on an internal net",
                location,
            ));
        }
        result.push(NoConnect { endpoint, reason: reason.clone() });
    }
    Ok(result)
}

/// `_supply`.
pub fn supply(declaration: &NamedBlock) -> Result<Supply> {
    let location = &declaration.location;
    let attributes = &declaration.attributes;
    reject_unknown(attributes.keys(), &["voltage", "net", "source", "external"], location)?;
    let voltage = quantity(required(attributes, "voltage", location)?, location)?;
    if !is_voltage(&voltage) {
        return Err(error("CMP006", "supply voltage must use V or mV", location));
    }
    let net = match attributes.get("net") {
        None => declaration.name.clone(),
        Some(Scalar::Str(text)) => text.clone(),
        Some(_) => return Err(error("CMP007", "supply net must be a net name", location)),
    };
    let source = match attributes.get("source") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP008", "supply source must be an endpoint", location)),
    };
    let externally_driven = match attributes.get("external") {
        None => false,
        Some(Scalar::Bool(flag)) => *flag,
        Some(_) => return Err(error("CMP009", "supply external property must be true or false", location)),
    };
    let source =
        source.map(|text| Endpoint::parse(&text).map_err(|message| error("CMP008", message, location))).transpose()?;
    Ok(Supply { name: declaration.name.clone(), voltage, net, source, externally_driven })
}

/// `_interface`.
pub fn interface(declaration: &InterfaceDecl) -> Result<Interface> {
    let location = &declaration.location;
    let type_name =
        if declaration.kind.contains('.') { declaration.kind.clone() } else { format!("std.{}", declaration.kind) };
    let mut signals = IndexMap::new();
    for (name, value) in &declaration.attributes {
        if name == "pullup" {
            continue;
        }
        let Scalar::Str(net) = value else {
            return Err(error("CMP011", format!("interface signal {name} must be a net name"), location));
        };
        signals.insert(name.clone(), net.clone());
    }
    if type_name == "std.i2c" && !(signals.contains_key("sda") && signals.contains_key("scl")) {
        return Err(error("CMP011", "std.i2c requires sda and scl net mappings", location));
    }
    let pullup_supply = match declaration.attributes.get("pullup") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP012", "I2C pullup must be a supply name", location)),
    };
    let mut bindings = IndexMap::new();
    for binding in &declaration.bindings {
        bindings.insert(binding.component.clone(), binding.signals.clone());
    }
    Ok(Interface { name: declaration.name.clone(), type_name, signals, bindings, pullup_supply })
}

const BREAKOUT_RELAXES: [(&str, &str); 3] =
    [("breakout_width", "width"), ("breakout_gap", "pair_gap"), ("breakout_clearance", "clearance")];

fn parameter_length(value: Option<&ParameterValue>) -> Option<&Quantity> {
    match value {
        Some(ParameterValue::Quantity(quantity)) if is_length(quantity) => Some(quantity),
        _ => None,
    }
}

/// `_positive_length`.
fn positive_length<'a>(
    parameters: &'a IndexMap<String, ParameterValue>,
    name: &str,
    label: &str,
    location: &Location,
) -> Result<Option<&'a Quantity>> {
    let Some(value) = parameters.get(name) else { return Ok(None) };
    let code = if label == "routing" { "CMP110" } else { "CMP111" };
    let Some(length) = parameter_length(Some(value)) else {
        return Err(error(code, format!("{label} parameter {} must be a length", repr(name)), location));
    };
    if *length.base_value() <= Decimal::zero() {
        return Err(error(code, format!("{label} parameter {} must be positive", repr(name)), location));
    }
    Ok(Some(length))
}

fn number_value(value: &ParameterValue) -> Option<f64> {
    match value {
        ParameterValue::Int(number) => number.to_string().parse().ok(),
        ParameterValue::Float(number) => Some(*number),
        _ => None,
    }
}

fn blank(text: &str) -> bool {
    text.chars().all(python_isspace)
}

fn validate_routing(declaration: &ConstraintDecl, parameters: &IndexMap<String, ParameterValue>) -> Result<()> {
    let location = &declaration.location;
    let target = parameters.get("target_single_ended_ohms");
    if let Some(target) = target {
        let valid = matches!(target, ParameterValue::Int(number) if *number > 0.into());
        if !valid {
            return Err(error(
                "CMP110",
                "routing parameter 'target_single_ended_ohms' must be a positive integer",
                location,
            ));
        }
    }
    if let Some(tolerance) = parameters.get("impedance_tolerance_percent") {
        let valid = number_value(tolerance).is_some_and(|value| 0.0 < value && value < 100.0);
        if !valid {
            return Err(error(
                "CMP110",
                "routing parameter 'impedance_tolerance_percent' must be a number greater than 0 and below 100",
                location,
            ));
        }
        if !parameters.contains_key("target_impedance_ohms") && target.is_none() {
            return Err(error(
                "CMP110",
                "'impedance_tolerance_percent' requires 'target_impedance_ohms' or 'target_single_ended_ohms'",
                location,
            ));
        }
    }
    if let Some(group) = parameters.get("layer_group")
        && !matches!(group, ParameterValue::Text(text) if !blank(text))
    {
        return Err(error("CMP110", "routing parameter 'layer_group' must be a nonempty name", location));
    }
    let breakout_length = positive_length(parameters, "breakout_length", "routing", location)?;
    let relaxed: Vec<(&str, &str)> =
        BREAKOUT_RELAXES.iter().copied().filter(|(name, _)| parameters.contains_key(*name)).collect();
    for (name, base_name) in &relaxed {
        let value = positive_length(parameters, name, "routing", location)?.expect("present");
        let base = parameters.get(*base_name);
        if *name == "breakout_gap" && base.is_none() {
            return Err(error("CMP110", "'breakout_gap' requires a differential 'pair_gap'", location));
        }
        if let Some(base) = parameter_length(base)
            && value.base_value() > base.base_value()
        {
            return Err(error(
                "CMP110",
                format!("'{name}' may only relax '{base_name}', never tighten it ({name} exceeds {base_name})"),
                location,
            ));
        }
    }
    if !relaxed.is_empty() && breakout_length.is_none() {
        return Err(error("CMP110", "breakout properties require a positive 'breakout_length'", location));
    }
    if breakout_length.is_some() && relaxed.is_empty() {
        return Err(error(
            "CMP110",
            "'breakout_length' requires 'breakout_width', 'breakout_gap' or 'breakout_clearance'",
            location,
        ));
    }
    let style = parameters.get("tuning_style");
    let serpentine = matches!(style, Some(ParameterValue::Text(text)) if text == "serpentine");
    if let Some(style) = style
        && !matches!(style, ParameterValue::Text(text) if text == "bumps" || text == "serpentine")
    {
        return Err(error("CMP110", "routing parameter 'tuning_style' must be \"bumps\" or \"serpentine\"", location));
    }
    if positive_length(parameters, "tuning_spacing", "routing", location)?.is_some() && !serpentine {
        return Err(error("CMP110", "'tuning_spacing' requires tuning_style = \"serpentine\"", location));
    }
    if let Some(group) = parameters.get("tuning_group") {
        if !matches!(group, ParameterValue::Text(text) if !blank(text)) {
            return Err(error("CMP110", "routing parameter 'tuning_group' must be a nonempty name", location));
        }
        let general = match parameters.get("kind") {
            None => true,
            Some(ParameterValue::Text(text)) => text == "general",
            Some(_) => false,
        };
        if general {
            return Err(error("CMP110", "'tuning_group' requires a critical routing kind", location));
        }
    }
    Ok(())
}

fn validate_length_match(declaration: &ConstraintDecl, parameters: &IndexMap<String, ParameterValue>) -> Result<()> {
    let location = &declaration.location;
    if declaration.targets.len() < 2 {
        return Err(error("CMP111", "length_match requires at least two nets", location));
    }
    let mut duplicates: Vec<&String> = declaration
        .targets
        .iter()
        .filter(|target| declaration.targets.iter().filter(|other| other == target).count() > 1)
        .collect();
    duplicates.sort();
    if let Some(name) = duplicates.first() {
        return Err(error("CMP111", format!("length_match lists net {} more than once", repr(name)), location));
    }
    if let Some(pin) = declaration.targets.iter().find(|target| target.contains('.')) {
        return Err(error("CMP111", format!("length_match targets must be nets, not pins ({})", repr(pin)), location));
    }
    let mut unknown: Vec<&String> = parameters.keys().filter(|name| *name != "max_skew").collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return Err(error("CMP111", format!("unknown length_match parameter {}", repr(name)), location));
    }
    if !parameters.contains_key("max_skew") {
        return Err(error("CMP111", "length_match requires 'max_skew'", location));
    }
    positive_length(parameters, "max_skew", "length_match", location)?;
    Ok(())
}

fn validate_courtyard_gap(
    declaration: &ConstraintDecl,
    parameters: &IndexMap<String, ParameterValue>,
    mode: ConstraintMode,
) -> Result<()> {
    let location = &declaration.location;
    if !matches!(mode, ConstraintMode::Require | ConstraintMode::External) {
        return Err(error("CMP123", "courtyard_gap is a hard minimum; mode must be require or external", location));
    }
    if declaration.targets.is_empty() || declaration.targets.iter().any(|target| target.contains('.')) {
        return Err(error("CMP123", "courtyard_gap requires component targets, not pins", location));
    }
    let standalone = declaration.kind == ConstraintKind::CourtyardGap.value();
    let name = if standalone { "additional" } else { "courtyard_gap" };
    if standalone && (parameters.len() != 1 || !parameters.contains_key(name)) {
        return Err(error("CMP123", "courtyard_gap requires only the 'additional' parameter", location));
    }
    let Some(value) = parameter_length(parameters.get(name)) else {
        return Err(error(
            "CMP123",
            format!("courtyard_gap parameter {} must be a nonnegative length", repr(name)),
            location,
        ));
    };
    let millimetres = value.in_unit("mm").expect("lengths convert to mm");
    let nanometres = millimetres.mul(&Decimal::from_i64(1_000_000)).expect("within the context");
    if nanometres < Decimal::zero() || nanometres != nanometres.to_integral_value(Rounding::HalfEven) {
        return Err(error(
            "CMP123",
            format!("courtyard_gap parameter {} must be nonnegative exact integer nanometres", repr(name)),
            location,
        ));
    }
    Ok(())
}

fn validate_hole_clearance(declaration: &ConstraintDecl, parameters: &IndexMap<String, ParameterValue>) -> Result<()> {
    let location = &declaration.location;
    if declaration.targets.len() != 1 {
        return Err(error("CMP112", "hole_clearance requires exactly one component", location));
    }
    if declaration.targets[0].contains('.') {
        return Err(error(
            "CMP112",
            format!("hole_clearance targets a component, not a pin ({})", repr(&declaration.targets[0])),
            location,
        ));
    }
    let mut unknown: Vec<&String> =
        parameters.keys().filter(|name| *name != "clearance" && *name != "reason").collect();
    unknown.sort();
    if let Some(name) = unknown.first() {
        return Err(error("CMP112", format!("unknown hole_clearance parameter {}", repr(name)), location));
    }
    let Some(clearance) = parameters.get("clearance") else {
        return Err(error("CMP112", "hole_clearance requires 'clearance'", location));
    };
    if !parameter_length(Some(clearance)).is_some_and(|length| *length.base_value() > Decimal::zero()) {
        return Err(error("CMP112", "hole_clearance parameter 'clearance' must be a positive length", location));
    }
    let Some(reason) = parameters.get("reason") else {
        return Err(error("CMP112", "hole_clearance requires 'reason'", location));
    };
    if !matches!(reason, ParameterValue::Text(text) if !blank(text)) {
        return Err(error("CMP112", "hole_clearance parameter 'reason' must be a nonempty string", location));
    }
    Ok(())
}

/// `_constraint`.
pub fn constraint(declaration: &ConstraintDecl) -> Result<Constraint> {
    let location = &declaration.location;
    let Some(kind) = ConstraintKind::parse(&declaration.kind) else {
        return Err(error("CMP013", format!("unsupported constraint kind {}", repr(&declaration.kind)), location));
    };
    let mut parameters = IndexMap::new();
    for (name, value) in &declaration.parameters {
        if ["id", "mode", "consumers", "verifier"].contains(&name.as_str()) {
            continue;
        }
        let compiled = match value {
            Scalar::Quantity { .. } => ParameterValue::Quantity(quantity(value, location)?),
            Scalar::Str(text) => ParameterValue::Text(text.clone()),
            Scalar::Int(number) => ParameterValue::Int(number.clone()),
            Scalar::Float(number) => ParameterValue::Float(*number),
            Scalar::Bool(flag) => ParameterValue::Bool(*flag),
        };
        parameters.insert(name.clone(), compiled);
    }
    let constraint_id = match declaration.parameters.get("id") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP014", "constraint id must be a string", location)),
    };
    let raw_mode = match declaration.parameters.get("mode") {
        None => "require".to_owned(),
        Some(Scalar::Str(text)) => text.clone(),
        Some(_) => return Err(error("CMP014", "constraint mode must be a name", location)),
    };
    let Some(mode) = ConstraintMode::parse(&raw_mode) else {
        return Err(error("CMP014", format!("unsupported constraint mode {}", repr(&raw_mode)), location));
    };
    let consumers = match declaration.parameters.get("consumers") {
        None => Vec::new(),
        Some(Scalar::Str(text)) => text
            .split(',')
            .map(|item| item.trim_matches(python_isspace))
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(_) => return Err(error("CMP014", "constraint consumers must be a comma-separated string", location)),
    };
    let verifier = match declaration.parameters.get("verifier") {
        None => None,
        Some(Scalar::Str(text)) => Some(text.clone()),
        Some(_) => return Err(error("CMP014", "constraint verifier must be a string", location)),
    };
    match kind {
        ConstraintKind::Routing => validate_routing(declaration, &parameters)?,
        ConstraintKind::LengthMatch => validate_length_match(declaration, &parameters)?,
        ConstraintKind::HoleClearance => validate_hole_clearance(declaration, &parameters)?,
        ConstraintKind::CourtyardGap => validate_courtyard_gap(declaration, &parameters, mode)?,
        ConstraintKind::PlacementRegion if parameters.contains_key("courtyard_gap") => {
            validate_courtyard_gap(declaration, &parameters, mode)?
        }
        _ => {}
    }
    if constraint_id.as_deref() == Some("") {
        return Err(error("CMP014", "constraint id cannot be empty", location));
    }
    if matches!(mode, ConstraintMode::Require | ConstraintMode::External) && consumers.is_empty() != verifier.is_none()
    {
        return Err(error("CMP014", "hard constraint ownership requires both consumers and verifier", location));
    }
    Ok(Constraint {
        kind,
        targets: declaration.targets.clone(),
        parameters,
        constraint_id,
        mode,
        consumers,
        verifier,
        origins: vec![location.to_string()],
    })
}

/// `_power_state`.
pub fn power_state(declaration: &PowerStateDecl) -> Result<PowerState> {
    let mut rails = IndexMap::new();
    for (rail, raw) in &declaration.rails {
        let Some(state) = PowerRailState::parse(raw) else {
            let allowed: Vec<&str> = PowerRailState::ALL.iter().map(|item| item.value()).collect();
            return Err(error(
                "CMP083",
                format!("unknown power rail state {}; expected: {}", repr(raw), allowed.join(", ")),
                &declaration.location,
            ));
        };
        rails.insert(rail.clone(), state);
    }
    Ok(PowerState { name: declaration.name.clone(), rails })
}

/// `_derive_selector`.
fn derive_selector(rule: Option<&RouteRule>, pad: Option<&str>) -> Option<String> {
    let (rule, pad) = (rule?, pad?);
    if rule.selector.kind == "pad_name" {
        return Some(pad.to_owned());
    }
    if rule.selector.kind == "nrf_psel"
        && pad.contains('_')
        && pad[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        let (port, pin) = pad[1..].split_once('_').expect("contains _");
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        if digits(port) && digits(pin) {
            let port: u64 = port.parse().expect("digits");
            let pin: u64 = pin.parse().expect("digits");
            return Some((port * 32 + pin).to_string());
        }
    }
    Some(format!("{}:{pad}", rule.selector.kind))
}

/// `_configuration`.
pub fn configuration(
    declaration: &ConfigurationDecl,
    components: &[ComponentInstance],
    library: &IndexMap<String, PartDefinition>,
    devices: &IndexMap<String, DeviceDefinition>,
) -> Result<PeripheralSelection> {
    let component = components.iter().find(|item| item.reference == declaration.component);
    let part = component.and_then(|component| library.get(&component.part));
    let device = part.and_then(|part| part.device.as_ref()).and_then(|name| devices.get(name));
    let mut raw = declaration.signals.clone();
    let raw_usage = raw.shift_remove("usage").unwrap_or_else(|| SelectionUsage::Exclusive.value().to_owned());
    let Some(usage) = SelectionUsage::parse(&raw_usage) else {
        let allowed: Vec<&str> = SelectionUsage::ALL.iter().map(|item| item.value()).collect();
        return Err(error(
            "CMP082",
            format!("unknown configuration usage {}; expected: {}", repr(&raw_usage), allowed.join(", ")),
            &declaration.location,
        ));
    };
    let modes = component.map(|component| crate::modes::effective_modes(component, device)).unwrap_or_default();
    let mut signals = IndexMap::new();
    for (signal, pin) in &raw {
        let package_pin = part.and_then(|part| part.pins.get(pin));
        let bonded = match (package_pin, component) {
            (Some(package_pin), Some(component)) => crate::modes::active_bonded_pads(component, package_pin, device),
            _ => Vec::new(),
        };
        let options: Vec<&MuxOption> = device
            .map(|device| {
                device
                    .mux_options
                    .iter()
                    .filter(|option| {
                        bonded.contains(&option.pad)
                            && option.peripheral == declaration.peripheral
                            && option.signal == *signal
                    })
                    .collect()
            })
            .unwrap_or_default();
        let option = if options.len() == 1 { Some(options[0]) } else { None };
        let selected = if bonded.len() == 1 { Some(bonded[0].as_str()) } else { None };
        let route = match (device, selected) {
            (Some(device), Some(selected)) => device.route_rules.iter().find(|rule| {
                rule.peripheral == declaration.peripheral
                    && rule.signal == *signal
                    && device.pad_sets[&rule.pad_set].pads.iter().any(|pad| pad == selected)
                    && crate::modes::condition_active(rule.when.as_ref(), &modes)
            }),
            _ => None,
        };
        signals.insert(
            signal.clone(),
            PeripheralSignalSelection {
                pin: pin.clone(),
                pad: selected.map(str::to_owned),
                selector: match option {
                    Some(option) => Some(option.selector.clone()),
                    None => derive_selector(route, selected),
                },
                resource: option.and_then(|option| option.resource.clone()),
                setting: option.and_then(|option| option.setting.clone()),
            },
        );
    }
    Ok(PeripheralSelection {
        component: declaration.component.clone(),
        peripheral: declaration.peripheral.clone(),
        name: declaration.name.clone(),
        signals,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_limits_follow_the_lazy_regex() {
        assert_eq!(relative_limit("VTERM+0.1V"), Some(("VTERM".into(), Some(('+', "0.1".into(), "V".into())))));
        assert_eq!(relative_limit(" VDD-IO - 300mV "), Some(("VDD-IO".into(), Some(('-', "300".into(), "mV".into())))));
        assert_eq!(relative_limit("VDD"), Some(("VDD".into(), None)));
        assert_eq!(relative_limit("1V"), None);
        assert_eq!(fixed_limit(" -1.5 V"), Some(("-1.5".into(), "V".into())));
    }

    #[test]
    fn unit_list_matches_python_sorting() {
        assert_eq!(quantity_units()[..4], ["A", "F", "GHz", "H"]);
        assert_eq!(quantity_dimension("ohm"), Some(Dimension::Resistance));
    }
}

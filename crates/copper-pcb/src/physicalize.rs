//! Electrical-to-physical lowering for the PCB draft (`pcbir.physicalize`):
//! proxy or resolved footprints on a deterministic inspection grid, nets,
//! lowered physical constraints, board mechanics and artwork.

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::python_str_repr as repr;
use copper_lang::compiler::Design;
use copper_lang::footprints::{FootprintResolver, selected_footprint};
use copper_lang::mechanical::MechanicalDesign;
use copper_lang::model::{
    ComponentInstance, ComponentValue, ConstraintKind, ConstraintMode, DeviceDefinition, FlatElectricalView, Net,
    ParameterValue, PartDefinition,
};
use copper_lang::pyvalue;
use copper_physical::footprint::{
    FootprintPad, InternalPadGroup, PadKind, PadShape, PhysicalFootprint, Size, merge_internal_pad_groups,
};
use copper_physical::{
    BoardOutline, BoardSide, CopperKeepout, CopperLayer, DesignRules, PlacementKeepout, Point, RoundedConvexShape,
};
use copper_units::{Decimal, Dimension};

use crate::board::*;
use crate::transform::{placed_pad_shape, transformed_local_point};
use crate::zones::{length_nm, lower_zone_outline, polygon_boundary, validate_routing_reservations};
use crate::{Error, Result, fail};

/// `PrototypePhysicalOptions`.
#[derive(Clone, Debug)]
pub struct PhysicalOptions {
    pub board_width_mm: f64,
    pub board_height_mm: f64,
    pub copper_layers: i64,
    pub fabrication_profile: String,
    pub columns: i64,
    pub margin_mm: f64,
}

impl PhysicalOptions {
    pub fn new(width_mm: f64, height_mm: f64, copper_layers: i64, fabrication_profile: &str) -> Result<Self> {
        let options = PhysicalOptions {
            board_width_mm: width_mm,
            board_height_mm: height_mm,
            copper_layers,
            fabrication_profile: fabrication_profile.to_owned(),
            columns: 3,
            margin_mm: 12.0,
        };
        if options.board_width_mm <= 0.0 || options.board_height_mm <= 0.0 {
            return fail("prototype board dimensions must be positive");
        }
        if ![2, 4, 6].contains(&options.copper_layers) {
            return fail("prototype physicalizer supports two, four, or six copper layers");
        }
        if !["generic", "jlcpcb-four-layer", "jlcpcb-six-layer"].contains(&fabrication_profile) {
            return fail("unknown prototype fabrication profile");
        }
        if fabrication_profile == "jlcpcb-four-layer" && copper_layers != 4 {
            return fail("JLCPCB four-layer profile requires four copper layers");
        }
        if fabrication_profile == "jlcpcb-six-layer" && copper_layers != 6 {
            return fail("JLCPCB six-layer profile requires six copper layers");
        }
        if options.margin_mm * 2.0 >= options.board_width_mm.min(options.board_height_mm) {
            return fail("prototype board margin leaves no placement area");
        }
        Ok(options)
    }
}

/// Where footprint geometry comes from: generated proxies or verified files.
pub enum Footprints<'a> {
    Proxy,
    Resolved(&'a mut FootprintResolver),
}

// ---------------------------------------------------------------------------
// Constraint parameters

/// Python's `str(value)` of a constraint parameter.
pub fn parameter_str(value: &ParameterValue) -> String {
    match value {
        ParameterValue::Text(text) => text.clone(),
        ParameterValue::Int(number) => number.to_string(),
        ParameterValue::Float(number) => copper_json::python_float_repr(*number),
        ParameterValue::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        ParameterValue::Quantity(quantity) => quantity.to_string(),
    }
}

/// Python's `repr(value)` for the parameter values messages show.
pub(crate) fn parameter_repr(value: &ParameterValue) -> String {
    match value {
        ParameterValue::Text(text) => repr(text),
        other => parameter_str(other),
    }
}

/// Python `==` between parameter values (`1 == 1.0 == True`).
pub(crate) fn parameter_eq(first: &ParameterValue, second: &ParameterValue) -> bool {
    use ParameterValue::*;
    let number = |value: &ParameterValue| -> Option<f64> {
        match value {
            Int(n) => n.to_string().parse().ok(),
            Float(f) => Some(*f),
            Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    };
    match (first, second) {
        (Text(a), Text(b)) => a == b,
        (Quantity(a), Quantity(b)) => a == b,
        (Int(a), Int(b)) => a == b,
        _ => matches!((number(first), number(second)), (Some(a), Some(b)) if a == b),
    }
}

type Parameters = IndexMap<String, ParameterValue>;

fn constraint_length(parameters: &Parameters, name: &str) -> Result<i64> {
    parameters
        .get(name)
        .and_then(length_nm)
        .ok_or_else(|| Error::Value(format!("placement constraint parameter {} must be a length", repr(name))))
}

fn optional_length(parameters: &Parameters, name: &str) -> Result<Option<i64>> {
    if parameters.contains_key(name) { constraint_length(parameters, name).map(Some) } else { Ok(None) }
}

fn courtyard_gap_length(parameters: &Parameters, name: &str) -> Result<i64> {
    let invalid = || Error::Value(format!("courtyard_gap parameter {} must be a nonnegative length", repr(name)));
    let Some(ParameterValue::Quantity(quantity)) = parameters.get(name) else { return Err(invalid()) };
    if quantity.dimension() != Dimension::Length {
        return Err(invalid());
    }
    let nanometres = quantity
        .in_unit("mm")
        .map_err(|e| Error::Value(e.to_string()))?
        .mul(&Decimal::from_i64(1_000_000))
        .map_err(crate::decimal_error)?;
    if nanometres < Decimal::zero() || !nanometres.is_integral() {
        return Err(Error::Value(format!(
            "courtyard_gap parameter {} must be nonnegative exact integer nanometres",
            repr(name)
        )));
    }
    nanometres.to_i64().ok_or_else(|| Error::NotImplemented("courtyard gap beyond 2^63 nm".to_owned()))
}

fn constraint_int(parameters: &Parameters, name: &str, default: i64) -> Result<i64> {
    match parameters.get(name) {
        None => Ok(default),
        Some(ParameterValue::Int(number)) => {
            number.to_string().parse().map_err(|_| Error::NotImplemented(format!("integer parameter {name}")))
        }
        Some(ParameterValue::Float(number)) => {
            if number.is_finite() {
                Ok(number.trunc() as i64)
            } else {
                Err(Error::NotImplemented(format!("non-finite parameter {name}")))
            }
        }
        Some(_) => Err(Error::Value(format!("placement constraint parameter {} must be numeric", repr(name)))),
    }
}

fn optional_int(parameters: &Parameters, name: &str) -> Result<Option<i64>> {
    if parameters.contains_key(name) { constraint_int(parameters, name, 0).map(Some) } else { Ok(None) }
}

fn optional_string(parameters: &Parameters, name: &str) -> Result<Option<String>> {
    match parameters.get(name) {
        None => Ok(None),
        Some(ParameterValue::Text(text)) => Ok(Some(text.clone())),
        Some(_) => Err(Error::Value(format!("routing constraint parameter {} must be a string", repr(name)))),
    }
}

fn constraint_bool(parameters: &Parameters, name: &str, default: bool) -> Result<bool> {
    match parameters.get(name) {
        None => Ok(default),
        Some(ParameterValue::Bool(flag)) => Ok(*flag),
        Some(_) => Err(Error::Value(format!("routing constraint parameter {} must be boolean", repr(name)))),
    }
}

fn constraint_layers(value: Option<&ParameterValue>) -> Result<Vec<CopperLayer>> {
    match value {
        None => Ok(Vec::new()),
        Some(ParameterValue::Text(text)) => text
            .split(',')
            .map(crate::py_strip)
            .filter(|item| !item.is_empty())
            .map(|item| {
                CopperLayer::parse(item)
                    .ok_or_else(|| Error::Value("routing allowed_layers contains an unknown copper layer".to_owned()))
            })
            .collect(),
        Some(_) => fail("routing allowed_layers must be a comma-separated string"),
    }
}

fn constraint_side(value: Option<&ParameterValue>) -> Result<Option<BoardSide>> {
    match value {
        None => Ok(None),
        Some(ParameterValue::Text(text)) if text == "both" => Ok(None),
        Some(ParameterValue::Text(text)) => BoardSide::parse(text)
            .map(Some)
            .ok_or_else(|| Error::Value(format!("{} is not a valid BoardSide", repr(text)))),
        Some(_) => fail("placement constraint side must be front, back, or both"),
    }
}

fn constraint_orientations(value: Option<&ParameterValue>) -> Result<Vec<Decimal>> {
    let Some(ParameterValue::Text(text)) = value else {
        return fail("allowed orientations must be a comma-separated string");
    };
    let mut result = Vec::new();
    for item in text.split(',').map(crate::py_strip).filter(|item| !item.is_empty()) {
        result.push(
            Decimal::parse(item)
                .map_err(|_| Error::Value("allowed orientations contain a non-numeric value".to_owned()))?,
        );
    }
    if result.is_empty() {
        return fail("allowed orientations cannot be empty");
    }
    Ok(result)
}

fn optional_percent(parameters: &Parameters, name: &str) -> Result<Option<Decimal>> {
    match parameters.get(name) {
        None => Ok(None),
        Some(ParameterValue::Int(number)) => {
            Ok(Some(Decimal::parse(&number.to_string()).map_err(crate::decimal_error)?))
        }
        Some(ParameterValue::Float(number)) => crate::float_decimal(*number).map(Some),
        Some(_) => Err(Error::Value(format!("routing constraint parameter {} must be numeric", repr(name)))),
    }
}

fn enum_value<T>(
    parameters: &Parameters,
    name: &str,
    default: &str,
    enum_name: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<T> {
    let text = parameters.get(name).map_or_else(|| default.to_owned(), parameter_str);
    parse(&text).ok_or_else(|| Error::Value(format!("{} is not a valid {enum_name}", repr(&text))))
}

// ---------------------------------------------------------------------------
// Placement rules

/// The keyword changes `_merge_rule` receives; `Some(None)` passes an explicit `None`.
#[derive(Default)]
struct RuleChanges {
    region: Option<Option<String>>,
    allowed_orientations: Option<Vec<Decimal>>,
    fixed_position: Option<Option<Point>>,
    fixed_rotation_degrees: Option<Option<Decimal>>,
    side: Option<Option<BoardSide>>,
    priority: Option<i64>,
    edge_clearance_nm: Option<Option<i64>>,
    additional_courtyard_gap_nm: Option<Option<i64>>,
}

fn merge_rule(
    existing: Option<&ComponentPlacementRule>,
    reference: &str,
    mut changes: RuleChanges,
) -> Result<ComponentPlacementRule> {
    if let Some(existing) = existing {
        if let Some(value) = changes.additional_courtyard_gap_nm {
            match value {
                None => changes.additional_courtyard_gap_nm = None,
                Some(value) => {
                    if let Some(previous) = existing.additional_courtyard_gap_nm {
                        changes.additional_courtyard_gap_nm = Some(Some(previous.max(value)));
                    }
                }
            }
        }
        fn protect<T: PartialEq>(
            change: &mut Option<Option<T>>,
            current: &Option<T>,
            key: &str,
            reference: &str,
        ) -> Result<()> {
            if let (Some(value), Some(current)) = (change.as_ref(), current) {
                match value {
                    None => *change = None,
                    Some(value) if value != current => {
                        return Err(Error::Value(format!("conflicting {key} constraints for {}", repr(reference))));
                    }
                    Some(_) => {}
                }
            }
            Ok(())
        }
        protect(&mut changes.fixed_position, &existing.fixed_position, "fixed_position", reference)?;
        protect(
            &mut changes.fixed_rotation_degrees,
            &existing.fixed_rotation_degrees,
            "fixed_rotation_degrees",
            reference,
        )?;
        protect(&mut changes.side, &existing.side, "side", reference)?;
    }
    let mut rule = existing.cloned().unwrap_or_else(|| ComponentPlacementRule::new(reference));
    if let Some(value) = changes.region {
        rule.region = value;
    }
    if let Some(value) = changes.allowed_orientations {
        rule.allowed_orientations = value;
    }
    if let Some(value) = changes.fixed_position {
        rule.fixed_position = value;
    }
    if let Some(value) = changes.fixed_rotation_degrees {
        rule.fixed_rotation_degrees = value;
    }
    if let Some(value) = changes.side {
        rule.side = value;
    }
    if let Some(value) = changes.priority {
        rule.priority = value;
    }
    if let Some(value) = changes.edge_clearance_nm {
        rule.edge_clearance_nm = value;
    }
    if let Some(value) = changes.additional_courtyard_gap_nm {
        rule.additional_courtyard_gap_nm = value;
    }
    rule.normalized()
}

// ---------------------------------------------------------------------------
// Pins and footprints

/// `_natural`: digit runs compare as numbers, the rest case-folded.
pub(crate) fn natural_key(value: &str) -> Vec<(u8, String)> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut digits = false;
    for character in value.chars() {
        let is_digit = character.is_ascii_digit();
        if !current.is_empty() && is_digit != digits {
            pieces.push(std::mem::take(&mut current));
        }
        digits = is_digit;
        current.push(character);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
        .into_iter()
        .map(|piece| {
            if piece.chars().all(|c| c.is_ascii_digit()) {
                let trimmed = piece.trim_start_matches('0');
                // Equal-length digit strings order numerically; pad for that.
                (0, format!("{:0>40}", if trimmed.is_empty() { "0" } else { trimmed }))
            } else {
                (1, piece.to_lowercase())
            }
        })
        .collect()
}

/// `_physical_pin_number`: a package pin, or a unit terminal bonded to exactly one pin.
pub fn physical_pin_number(
    component: &ComponentInstance,
    part: &PartDefinition,
    devices: &IndexMap<String, DeviceDefinition>,
    endpoint: &str,
) -> Option<String> {
    if let Some(pin) = part.pins.get(endpoint) {
        return Some(pin.number.clone());
    }
    let (unit_name, terminal_name) = endpoint.split_once('.')?;
    let device = devices.get(part.device.as_deref().unwrap_or(""))?;
    let terminal = device.units.get(unit_name)?.terminals.get(terminal_name)?;
    let matches = copper_lang::modes::pins_bonded_to_pad(component, part, Some(device), &terminal.pad);
    if matches.len() == 1 { Some(matches[0].number.clone()) } else { None }
}

fn safe(value: &str) -> String {
    let replaced: String =
        value.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).collect();
    let stripped = replaced.trim_matches('_');
    if stripped.is_empty() { "Footprint".to_owned() } else { stripped.to_owned() }
}

fn hex_digest(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn proxy_pad(number: &str, position: Point) -> Result<FootprintPad> {
    Ok(FootprintPad {
        number: number.to_owned(),
        position,
        size: Size::new(1_000_000, 1_000_000)?,
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::zero(),
        drill: None,
        roundrect_ratio_ppm: 250_000,
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    }
    .validated()?)
}

/// `_proxy_footprint`: inspection-only lands in two columns.
fn proxy_footprint(part: &PartDefinition, selected: &str) -> Result<PhysicalFootprint> {
    let mut pins: Vec<&String> = part.pins.values().map(|pin| &pin.number).collect();
    pins.sort_by_key(|number| natural_key(number));
    let numbers: Vec<&str> = pins.iter().map(|n| n.as_str()).collect();
    let digest = hex_digest(format!("{}\0{selected}\0{}", part.name, numbers.join(",")).as_bytes());
    let name = format!("CopperScript/{}_{}", safe(selected), &digest[..10]);
    let mm = crate::nm_from_mm_float;
    let count = numbers.len();
    let (positions, body) = if count == 2 {
        (vec![Point::new(mm(-1.0)?, 0), Point::new(mm(1.0)?, 0)], Size::new(mm(1.6)?, mm(0.8)?)?)
    } else {
        let left_count = count.div_ceil(2);
        let right_count = count - left_count;
        let pitch = 1.27;
        let mut positions = Vec::new();
        for (x, column) in [(-2.5, left_count), (2.5, right_count)] {
            for index in 0..column {
                let y = (index as f64 - (column as f64 - 1.0) / 2.0) * pitch;
                positions.push(Point::new(mm(x)?, mm(y)?));
            }
        }
        let height = 3.0f64.max(left_count.max(right_count) as f64 * pitch);
        (positions, Size::new(mm(4.0)?, mm(height)?)?)
    };
    let pads =
        numbers.iter().zip(positions).map(|(number, position)| proxy_pad(number, position)).collect::<Result<_>>()?;
    Ok(PhysicalFootprint {
        name,
        pads,
        body_size: body,
        source_library_id: Some(selected.to_owned()),
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    }
    .validated()?)
}

/// `_validate_footprint_pins`: pin numbers equal the numbered pads exactly.
fn validate_footprint_pins(
    part: &PartDefinition,
    component: &ComponentInstance,
    footprint: &PhysicalFootprint,
) -> Result<()> {
    let part_numbers: IndexSet<&str> = part.pins.values().map(|pin| pin.number.as_str()).collect();
    let pad_numbers: IndexSet<&str> =
        footprint.pads.iter().filter(|pad| !pad.number.is_empty()).map(|pad| pad.number.as_str()).collect();
    let mut missing: Vec<&str> = part_numbers.iter().filter(|n| !pad_numbers.contains(*n)).copied().collect();
    let mut extra: Vec<&str> = pad_numbers.iter().filter(|n| !part_numbers.contains(*n)).copied().collect();
    missing.sort_by_key(|n| natural_key(n));
    extra.sort_by_key(|n| natural_key(n));
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    let mut details = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing pads {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        details.push(format!("unknown pads {}", extra.join(", ")));
    }
    let shown = footprint.source_library_id.as_deref().unwrap_or(&footprint.name);
    Err(Error::Value(format!(
        "component {} part {} is incompatible with footprint {}: {}",
        repr(&component.reference),
        repr(&part.name),
        repr(shown),
        details.join("; ")
    )))
}

fn groups_repr(groups: &[InternalPadGroup]) -> String {
    let items: Vec<String> = groups
        .iter()
        .map(|group| {
            let numbers: Vec<String> = group.numbers.iter().map(|n| repr(n)).collect();
            let tuple =
                if numbers.len() == 1 { format!("({},)", numbers[0]) } else { format!("({})", numbers.join(", ")) };
            format!("InternalPadGroup(numbers={tuple})")
        })
        .collect();
    if items.len() == 1 { format!("({},)", items[0]) } else { format!("({})", items.join(", ")) }
}

fn component_value(component: &ComponentInstance, part: &PartDefinition) -> String {
    match &component.value {
        None => part.name.rsplit('.').next().unwrap_or(&part.name).to_owned(),
        Some(ComponentValue::Quantity(quantity)) => quantity.to_string(),
        Some(ComponentValue::Text(text)) => text.clone(),
    }
}

// ---------------------------------------------------------------------------
// The lowering

pub(crate) struct Selected<'a> {
    pub component: &'a ComponentInstance,
    pub part: &'a PartDefinition,
    pub footprint: String,
}

fn fabrication_rules(options: &PhysicalOptions) -> DesignRules {
    if options.fabrication_profile.starts_with("jlcpcb-") {
        DesignRules {
            minimum_clearance_nm: 90_000,
            minimum_track_width_nm: 90_000,
            default_track_width_nm: 200_000,
            minimum_via_size_nm: Some(450_000),
            minimum_via_drill_nm: Some(200_000),
            ..DesignRules::default()
        }
    } else {
        DesignRules::default()
    }
}

fn apply_rule_overrides(mut rules: DesignRules, overrides: &IndexMap<String, i64>) -> Result<DesignRules> {
    for (name, value) in overrides {
        let value = *value;
        match name.as_str() {
            "minimum_clearance_nm" => rules.minimum_clearance_nm = value,
            "minimum_hole_clearance_nm" => rules.minimum_hole_clearance_nm = value,
            "minimum_track_width_nm" => rules.minimum_track_width_nm = value,
            "default_track_width_nm" => rules.default_track_width_nm = value,
            "default_via_size_nm" => rules.default_via_size_nm = value,
            "default_via_drill_nm" => rules.default_via_drill_nm = value,
            "minimum_slot_width_nm" => rules.minimum_slot_width_nm = value,
            "minimum_via_size_nm" => rules.minimum_via_size_nm = Some(value),
            "minimum_via_drill_nm" => rules.minimum_via_drill_nm = Some(value),
            other => {
                return Err(Error::Value(format!(
                    "DesignRules.__init__() got an unexpected keyword argument '{other}'"
                )));
            }
        }
    }
    rules.validate()?;
    Ok(rules)
}

/// `prototype_physicalize` / `resolved_physicalize`.
///
/// `locked` and `offline` are the design's own resolution flags, which source
/// physical macros use.
pub fn physicalize(
    design: &Design,
    mut source: Footprints<'_>,
    options: &PhysicalOptions,
    locked: bool,
    offline: bool,
) -> Result<PhysicalBoard> {
    let mechanical = design.mechanical.as_ref();
    let board = &design.electrical;
    let outline = match mechanical {
        Some(mechanical) => mechanical.outline.clone(),
        None => BoardOutline::rectangle(
            crate::nm_from_mm_float(options.board_width_mm)?,
            crate::nm_from_mm_float(options.board_height_mm)?,
            None,
        )?,
    };
    let flat = copper_lang::elaborate::elaborate(board)?;
    let mut metadata: IndexMap<String, String> = IndexMap::new();
    let proxy = matches!(source, Footprints::Proxy);
    if proxy {
        metadata.insert("generator".to_owned(), "copperscript-prototype-physicalizer".to_owned());
        metadata.insert("prototype_footprints".to_owned(), "true".to_owned());
    } else {
        metadata.insert("generator".to_owned(), "copperscript-resolved-footprint-physicalizer".to_owned());
        metadata.insert("resolved_footprints".to_owned(), "true".to_owned());
    }
    metadata.insert("prototype_placement".to_owned(), "true".to_owned());
    metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    let mut rules = fabrication_rules(options);
    if options.fabrication_profile.starts_with("jlcpcb-") {
        metadata.insert("fabrication_profile".to_owned(), options.fabrication_profile.clone());
    }
    if let Some(mechanical) = mechanical {
        rules = apply_rule_overrides(rules, &mechanical.rule_overrides)?;
    }
    // Python raises an uncaught KeyError here (a recorded difference).
    if let Some(component) = flat.components.iter().find(|c| !flat.library.contains_key(&c.part)) {
        return Err(Error::Value(format!(
            "component {} uses unknown part {}",
            repr(&component.reference),
            repr(&component.part)
        )));
    }
    let mut selected: Vec<Selected> = flat
        .components
        .iter()
        .filter_map(|component| {
            let part = &flat.library[&component.part];
            selected_footprint(component, part).map(|footprint| Selected { component, part, footprint })
        })
        .collect();
    selected.sort_by(|a, b| a.component.reference.cmp(&b.component.reference));
    // Electrical pin names beside physical pad numbers, for editors.
    let mut pad_names = copper_json::Object::new();
    for item in &selected {
        let mut by_number: IndexMap<&str, Vec<&str>> = IndexMap::new();
        for pin in item.part.pins.values() {
            by_number.entry(pin.number.as_str()).or_default().push(pin.name.as_str());
        }
        if by_number.is_empty() {
            continue;
        }
        let mut numbers: Vec<&&str> = by_number.keys().collect();
        numbers.sort();
        let mut names = copper_json::Object::new();
        for number in numbers {
            let mut unique: Vec<&str> = Vec::new();
            for name in &by_number[*number] {
                if !unique.contains(name) {
                    unique.push(name);
                }
            }
            names.insert((*number).to_owned(), copper_json::Value::Str(unique.join("/")));
        }
        pad_names.insert(item.component.reference.clone(), copper_json::Value::Object(names));
    }
    if !pad_names.is_empty() {
        let text = copper_json::Dumps::new().sort_keys(true).dumps(&copper_json::Value::Object(pad_names));
        metadata.insert("pad_names".to_owned(), text);
    }

    let xs: Vec<i64>;
    let ys: Vec<i64>;
    if let Some(circle) = outline.circular_boundary {
        xs = vec![circle.center.x_nm - circle.radius_nm, circle.center.x_nm + circle.radius_nm];
        ys = vec![circle.center.y_nm - circle.radius_nm, circle.center.y_nm + circle.radius_nm];
    } else {
        xs = outline.vertices.iter().map(|p| p.x_nm).collect();
        ys = outline.vertices.iter().map(|p| p.y_nm).collect();
    }
    let (min_x, max_x) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
    let (min_y, max_y) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
    let (left, top) = (min_x as f64 / 1e6, min_y as f64 / 1e6);
    let (width, height) = ((max_x - min_x) as f64 / 1e6, (max_y - min_y) as f64 / 1e6);
    let margin = options.margin_mm.min(width.min(height) / 4.0);
    let (usable_width, usable_height) = (width - 2.0 * margin, height - 2.0 * margin);
    let columns = options.columns;
    let rows = 1i64.max((selected.len() as i64 + columns - 1).div_euclid(columns));

    let mut footprints: IndexMap<String, PhysicalFootprint> = IndexMap::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut import_warnings: Vec<String> = Vec::new();
    let mut warned: IndexSet<String> = IndexSet::new();
    let mut cache: IndexMap<String, copper_physical::kicad_mod::FootprintImportResult> = IndexMap::new();
    for (index, item) in selected.iter().enumerate() {
        let mut footprint = match &mut source {
            Footprints::Proxy => proxy_footprint(item.part, &item.footprint)?,
            Footprints::Resolved(resolver) => {
                if !cache.contains_key(&item.footprint) {
                    let result = resolver.resolve(&item.footprint)?;
                    cache.insert(item.footprint.clone(), result);
                }
                let result = &cache[&item.footprint];
                validate_footprint_pins(item.part, item.component, &result.footprint)?;
                if warned.insert(item.footprint.clone()) {
                    import_warnings.extend(result.warnings.iter().map(|w| format!("{}: {w}", item.footprint)));
                    if !import_warnings.is_empty() {
                        metadata.insert("footprint_import_warnings".to_owned(), import_warnings.join("\n"));
                    }
                }
                result.footprint.clone()
            }
        };
        if !item.part.assembled {
            footprint.name = format!("{}__unassembled", footprint.name);
            footprint.internal_pad_groups = Vec::new();
            footprint.exclude_from_bom = true;
            footprint.exclude_from_pos_files = true;
            footprint = footprint.validated()?;
        } else if !item.part.internal_pad_groups.is_empty() {
            let mut combined: Vec<InternalPadGroup> = footprint.internal_pad_groups.clone();
            for group in &item.part.internal_pad_groups {
                combined.push(InternalPadGroup::new(group.numbers.clone())?);
            }
            let mut groups = merge_internal_pad_groups(&combined)?;
            if proxy {
                groups.retain(|group| group.numbers.len() > 1);
            }
            let suffix = &hex_digest(groups_repr(&groups).as_bytes())[..12];
            footprint.name = format!("{}__internal_{suffix}", footprint.name);
            footprint.internal_pad_groups = groups;
            footprint = footprint.validated()?;
        }
        if let Some(existing) = footprints.get(&footprint.name) {
            if *existing != footprint {
                return Err(Error::Value(format!(
                    "footprint ID {} resolved to conflicting geometry",
                    repr(&footprint.name)
                )));
            }
        } else {
            footprints.insert(footprint.name.clone(), footprint.clone());
        }
        let column = index as i64 % columns;
        let row = index as i64 / columns;
        let x = left + margin + usable_width * (column as f64 + 0.5) / columns as f64;
        let y = top + margin + usable_height * (row as f64 + 0.5) / rows as f64;
        let mut placement = Placement::new(
            &item.component.reference,
            &footprint.name,
            Point::new(crate::nm_from_mm_float(x)?, crate::nm_from_mm_float(y)?),
            &Decimal::zero(),
        )?;
        placement.value = component_value(item.component, item.part);
        placement.source_path = Some(item.component.reference.clone());
        placements.push(placement);
    }

    let component_index: IndexMap<&str, &Selected> =
        selected.iter().map(|item| (item.component.reference.as_str(), item)).collect();
    let mut nets: Vec<PhysicalNet> = Vec::new();
    let placement_footprint: IndexMap<&str, &str> =
        placements.iter().map(|p| (p.reference.as_str(), p.footprint.as_str())).collect();
    let mut sorted_nets: Vec<&Net> = flat.nets.iter().collect();
    sorted_nets.sort_by(|a, b| a.name.cmp(&b.name));
    for net in sorted_nets {
        let mut pads: Vec<PadReference> = Vec::new();
        for endpoint in &net.endpoints {
            let Some(item) = component_index.get(endpoint.component.as_str()) else { continue };
            if let Some(number) = physical_pin_number(item.component, item.part, &flat.devices, &endpoint.pin) {
                pads.push(PadReference::new(&item.component.reference, &number));
            }
        }
        for reference in pads.clone() {
            let footprint = &footprints[placement_footprint[reference.component.as_str()]];
            for group in &footprint.internal_pad_groups {
                if group.numbers.contains(&reference.pad) {
                    pads.extend(group.numbers.iter().map(|n| PadReference::new(&reference.component, n)));
                }
            }
        }
        if !pads.is_empty() {
            pads.sort();
            pads.dedup();
            nets.push(PhysicalNet { name: net.name.clone(), pads });
        }
    }

    let mut omitted: Vec<&str> = flat
        .components
        .iter()
        .filter(|c| selected_footprint(c, &flat.library[&c.part]).is_none())
        .map(|c| c.reference.as_str())
        .collect();
    omitted.sort_unstable();
    metadata.insert("omitted_components".to_owned(), omitted.join(","));
    let lowered = lower_physical_constraints(&flat, &component_index, &outline)?;
    if let Some(targets) = &lowered.omitted_targets {
        metadata.insert("omitted_constraint_targets".to_owned(), targets.clone());
    }
    let mut keepouts = lowered.keepouts;
    let mut placement_rules = lowered.rules;
    let mut attachments = Vec::new();
    if let Some(mechanical) = mechanical {
        keepouts.extend(mechanical.keepouts.iter().cloned());
        let names: Vec<&str> = keepouts.iter().map(|k| k.name.as_str()).collect();
        if crate::has_duplicates(&names) {
            return fail("conflicting placement keepout names");
        }
        placement_rules = bind_profile_connectors(mechanical, &flat, &footprints, &placements, placement_rules)?;
        (placement_rules, attachments) =
            bind_attachments(mechanical, &flat, &footprints, &placements, placement_rules)?;
        let owners: copper_json::Object = attachments
            .iter()
            .map(|a: &PhysicalAttachment| (a.reference.clone(), copper_json::Value::Str(a.id.clone())))
            .collect();
        let dumps = copper_json::Dumps::new().sort_keys(true);
        metadata.insert("mechanical_attachment_owners".to_owned(), dumps.dumps(&copper_json::Value::Object(owners)));
        if !mechanical.profiles.is_empty() {
            let provenance = copper_lang::mechanical::mechanical_provenance(mechanical);
            metadata.insert("mechanical_provenance".to_owned(), dumps.dumps(&provenance));
            let mut roles = copper_json::Object::new();
            for connector in &mechanical.connectors {
                roles.insert(pyvalue::text(&connector.reference), copper_json::Value::Str(connector.role.clone()));
            }
            metadata.insert("mechanical_connector_roles".to_owned(), dumps.dumps(&copper_json::Value::Object(roles)));
        }
    }
    let fixed: IndexMap<&str, &ComponentPlacementRule> =
        placement_rules.iter().map(|rule| (rule.reference.as_str(), rule)).collect();
    for placement in &mut placements {
        if let Some(rule) = fixed.get(placement.reference.as_str()) {
            if let Some(position) = rule.fixed_position {
                placement.position = position;
            }
            if let Some(rotation) = &rule.fixed_rotation_degrees {
                placement.rotation_degrees = crate::decimal_rem(rotation, 360)?;
            }
            if let Some(side) = rule.side {
                placement.side = side;
            }
        }
    }
    let copper_layers: Vec<CopperLayer> = match options.copper_layers {
        2 => vec![CopperLayer::Front, CopperLayer::Back],
        4 => vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back],
        _ => vec![
            CopperLayer::Front,
            CopperLayer::In1,
            CopperLayer::In2,
            CopperLayer::In3,
            CopperLayer::In4,
            CopperLayer::Back,
        ],
    };
    let stackup = match mechanical.and_then(|m| m.stackup.as_ref()) {
        Some(declared) => to_stackup(declared, copper_layers)?,
        None => Stackup::new(copper_layers),
    };
    let avoid = fixed_pad_shapes(&placements, &footprints, &fixed);
    let silkscreen = copper_lang::silkscreen::physical_artwork(&design.silkscreen, &outline, &avoid)?;
    let power_domains = crate::power::lower_power_domains(&flat, &nets);
    let power_feeds = crate::power::lower_power_feeds(&flat, &nets, &power_domains);
    let decoupling_links = lower_decoupling(&flat, &nets)?;
    let match_groups = lower_match_groups(&flat)?;
    let groups: Vec<(String, Vec<String>)> =
        match_groups.iter().map(|(id, nets, _)| (id.clone(), nets.clone())).collect();
    check_tuning_groups(&flat, &groups)?;
    let hole_clearances = lower_hole_clearances(&flat, &footprints, &placements, &rules, proxy)?;

    let component_heights = mechanical.map_or_else(Vec::new, |m| {
        m.component_heights
            .iter()
            .map(|h| ComponentHeight {
                id: h.id.clone(),
                reference: pyvalue::text(&h.reference),
                height_nm: h.height_nm,
            })
            .collect()
    });
    let mut physical = PhysicalBoard {
        name: board.name.clone(),
        outline,
        footprints,
        placements,
        nets,
        stackup,
        rules,
        tracks: Vec::new(),
        vias: Vec::new(),
        metadata,
        regions: lowered.regions,
        keepouts,
        placement_rules,
        relative_rules: lowered.relative,
        placement_groups: lowered.groups,
        power_domains,
        net_routing_rules: lowered.routing,
        zones: lowered.zones,
        polygons: Vec::new(),
        copper_keepouts: mechanical.map_or_else(Vec::new, |m| m.copper_keepouts.clone()),
        rigid_clusters: Vec::new(),
        hard_macros: Vec::new(),
        materialized_macros: Vec::new(),
        via_in_pad_rules: lowered.via_in_pad,
        mechanical_holes: mechanical.map_or_else(Vec::new, |m| m.holes.clone()),
        mechanical_slots: mechanical.map_or_else(Vec::new, |m| m.slots.clone()),
        datums: mechanical.map_or_else(Vec::new, |m| m.datums.clone()),
        boundary_edges: mechanical.map_or_else(Vec::new, |m| m.boundary_edges.clone()),
        attachments,
        component_heights,
        body_overhangs: mechanical.map_or_else(Vec::new, |m| m.body_overhangs.clone()),
        assembly_envelopes: mechanical.map_or_else(Vec::new, |m| m.assembly_envelopes.clone()),
        assembly_access: mechanical.map_or_else(Vec::new, |m| {
            m.assembly_access
                .iter()
                .map(|a| AssemblyAccess {
                    id: a.id.clone(),
                    reference: pyvalue::text(&a.reference),
                    outline: a.outline.clone(),
                    side: a.side.clone(),
                    purpose: a.purpose.clone(),
                })
                .collect()
        }),
        match_groups: match_groups
            .iter()
            .map(|(id, nets, max_skew_nm)| NetMatchGroup {
                id: id.clone(),
                nets: nets.clone(),
                max_skew_nm: *max_skew_nm,
            })
            .collect(),
        component_hole_clearances: hole_clearances,
        decoupling_links,
        silkscreen,
        power_feeds,
        mechanical_references: mechanical.map_or_else(Vec::new, |m| m.references.clone()),
    };
    let match_groups: Vec<(String, Vec<String>)> = match_groups.into_iter().map(|(id, nets, _)| (id, nets)).collect();
    finish_board(&mut physical, &match_groups, &lowered.region_names)?;
    if !flat.physical_macros.is_empty() {
        physical = crate::hard_macros::apply_source_macros(physical, &flat, locked, offline)?;
    }
    Ok(physical)
}

/// `MechanicalStackup.to_stackup`.
fn to_stackup(
    declared: &copper_lang::mechanical::stackup::MechanicalStackup,
    selected: Vec<CopperLayer>,
) -> Result<Stackup> {
    let layers: Vec<CopperLayer> = declared.layers.iter().filter_map(|layer| layer.copper_layer).collect();
    if layers != selected {
        let names = |layers: &[CopperLayer]| layers.iter().map(|l| l.value()).collect::<Vec<_>>().join(", ");
        return Err(Error::Value(format!(
            "{}: stackup declares {} copper layers ({}) but the selected stack has {} ({}); select --layers {}",
            declared.location,
            layers.len(),
            names(&layers),
            selected.len(),
            names(&selected),
            layers.len()
        )));
    }
    Ok(Stackup {
        copper_layers: layers,
        thickness_nm: declared.layers.iter().map(|layer| layer.thickness_nm).sum(),
        physical_layers: declared.layers.clone(),
    })
}

/// `_fixed_pad_shapes`: the copper of parts whose position is fixed.
fn fixed_pad_shapes(
    placements: &[Placement],
    footprints: &IndexMap<String, PhysicalFootprint>,
    fixed: &IndexMap<&str, &ComponentPlacementRule>,
) -> Vec<RoundedConvexShape> {
    let mut shapes = Vec::new();
    for pose in placements {
        if fixed.get(pose.reference.as_str()).is_none_or(|rule| rule.fixed_position.is_none()) {
            continue;
        }
        for pad in &footprints[&pose.footprint].pads {
            if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                continue;
            }
            shapes.push(placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose));
        }
    }
    shapes
}

/// The result of `_lower_physical_constraints`.
pub struct Lowered {
    pub region_names: Vec<String>,
    pub regions: Vec<PlacementRegion>,
    pub relative: Vec<RelativePlacementRule>,
    pub keepouts: Vec<PlacementKeepout>,
    pub rules: Vec<ComponentPlacementRule>,
    pub groups: Vec<PlacementGroup>,
    pub routing: Vec<NetRoutingRule>,
    pub zones: Vec<CopperZone>,
    pub via_in_pad: Vec<PadViaInPadRule>,
    /// `metadata["omitted_constraint_targets"]`, when a target was not selected.
    pub omitted_targets: Option<String>,
}

struct Target {
    reference: String,
    pad: Option<String>,
}

fn placement_targets(targets: &[Target]) -> Vec<PlacementTarget> {
    targets.iter().map(|t| PlacementTarget { reference: t.reference.clone(), pad: t.pad.clone() }).collect()
}

/// `_lower_physical_constraints` over the selected components of `flat`.
pub fn lower_constraints(
    flat: &FlatElectricalView,
    selected: &[(&ComponentInstance, &PartDefinition)],
    outline: &BoardOutline,
) -> Result<Lowered> {
    let items: Vec<Selected> =
        selected.iter().map(|(component, part)| Selected { component, part, footprint: String::new() }).collect();
    let components: IndexMap<&str, &Selected> = items.iter().map(|i| (i.component.reference.as_str(), i)).collect();
    lower_physical_constraints(flat, &components, outline)
}

fn lower_physical_constraints(
    flat: &FlatElectricalView,
    components: &IndexMap<&str, &Selected>,
    outline: &BoardOutline,
) -> Result<Lowered> {
    let skipped: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let mut groups: Vec<PlacementGroup> = Vec::new();
    let mut region_names = Vec::new();
    let mut regions = Vec::new();
    let mut relative = Vec::new();
    let mut keepouts = Vec::new();
    let mut rules: IndexMap<String, ComponentPlacementRule> = IndexMap::new();
    let mut routing = Vec::new();
    let mut zones = Vec::new();
    let mut via_in_pad = Vec::new();
    let targets = |values: &[String]| -> Result<Option<Vec<Target>>> {
        let mut lowered = Vec::new();
        for value in values {
            let (reference, pin) = match value.split_once('.') {
                Some((reference, pin)) => (reference, Some(pin)),
                None => (value.as_str(), None),
            };
            let Some(item) = components.get(reference) else {
                skipped.borrow_mut().push(value.clone());
                return Ok(None);
            };
            let pad = match pin {
                Some(pin) => {
                    Some(physical_pin_number(item.component, item.part, &flat.devices, pin).ok_or_else(|| {
                        Error::Value(format!("placement constraint target {} has no physical pad", repr(value)))
                    })?)
                }
                None => None,
            };
            lowered.push(Target { reference: reference.to_owned(), pad });
        }
        Ok(Some(lowered))
    };
    let net_names: IndexSet<&str> = flat.nets.iter().map(|net| net.name.as_str()).collect();
    for (index, constraint) in flat.constraints.iter().enumerate() {
        let parameters = &constraint.parameters;
        match constraint.kind {
            ConstraintKind::ViaInPad => {
                if constraint.targets.len() != 1 {
                    return fail("via_in_pad requires exactly one component.pin target");
                }
                let lowered = targets(&constraint.targets)?;
                let Some(Target { reference, pad: Some(pad) }) = lowered.and_then(|mut t| t.pop()) else {
                    return fail("via_in_pad target must resolve to a physical pad");
                };
                if parameters.keys().any(|key| !["process", "rows", "columns", "pitch"].contains(&key.as_str())) {
                    return fail("unknown via_in_pad parameter");
                }
                if parameters.contains_key("rows") != parameters.contains_key("columns") {
                    return fail("via_in_pad rows and columns must be given together");
                }
                if parameters.contains_key("pitch") && !parameters.contains_key("rows") {
                    return fail("via_in_pad pitch requires rows and columns");
                }
                let mut counts = [None, None];
                for (slot, name) in counts.iter_mut().zip(["rows", "columns"]) {
                    if let Some(value) = parameters.get(name) {
                        match value {
                            ParameterValue::Int(number) if number > &num_bigint::BigInt::from(0) => {
                                *slot =
                                    Some(number.to_string().parse::<i64>().map_err(|_| {
                                        Error::NotImplemented("via-in-pad array beyond 2^63".to_owned())
                                    })?);
                            }
                            _ => return fail("via_in_pad rows and columns must be positive integers"),
                        }
                    }
                }
                let pitch = optional_length(parameters, "pitch")?;
                let process = parameters.get("process").map_or_else(|| "filled-capped".to_owned(), parameter_str);
                if process != "filled-capped" {
                    return fail("via-in-pad supports only the filled-capped process");
                }
                if pitch.is_some_and(|pitch| pitch <= 0) {
                    return fail("via-in-pad array pitch must be positive and requires rows and columns");
                }
                via_in_pad.push(PadViaInPadRule {
                    pad: PadReference::new(&reference, &pad),
                    rows: counts[0],
                    columns: counts[1],
                    pitch_nm: pitch,
                });
                continue;
            }
            ConstraintKind::CopperZone => {
                if constraint.targets.len() != 1 {
                    return fail("copper_zone requires exactly one net target");
                }
                let net = &constraint.targets[0];
                if !net_names.contains(net.as_str()) {
                    return Err(Error::Value(format!("copper_zone references unknown net {}", repr(net))));
                }
                const KNOWN: [&str; 15] = [
                    "layers",
                    "inset",
                    "pad_connection",
                    "clearance",
                    "minimum_width",
                    "island_policy",
                    "region",
                    "polygon_mm",
                    "x",
                    "y",
                    "width",
                    "height",
                    "priority",
                    "reserve_routing",
                    "allow_same_net_hard_macro_overlap",
                ];
                let mut unknown: Vec<&str> =
                    parameters.keys().map(String::as_str).filter(|key| !KNOWN.contains(key)).collect();
                unknown.sort_unstable();
                if let Some(first) = unknown.first() {
                    return Err(Error::Value(format!("unknown copper_zone parameter {}", repr(first))));
                }
                let layers = constraint_layers(parameters.get("layers"))?;
                if layers.is_empty() {
                    return fail("copper_zone requires at least one copper layer");
                }
                let (outer, holes) = lower_zone_outline(parameters, outline, &flat.constraints)?;
                let priority = match parameters.get("priority") {
                    None => 0,
                    Some(ParameterValue::Int(number)) if number >= &num_bigint::BigInt::from(0) => {
                        number.to_string().parse().map_err(|_| Error::NotImplemented("zone priority".to_owned()))?
                    }
                    Some(_) => return fail("copper_zone priority must be a nonnegative integer"),
                };
                let island_policy =
                    enum_value(parameters, "island_policy", "remove_below_area", "IslandPolicy", IslandPolicy::parse)?;
                let reserve_routing = constraint_bool(parameters, "reserve_routing", false)?;
                let clearance_nm = optional_length(parameters, "clearance")?;
                let minimum_width_nm = match optional_length(parameters, "minimum_width")? {
                    Some(width) if width != 0 => width,
                    _ => 250_000,
                };
                let pad_connection =
                    enum_value(parameters, "pad_connection", "thermal", "ZoneConnection", crate::zone_connection)?;
                let allow_overlap = constraint_bool(parameters, "allow_same_net_hard_macro_overlap", false)?;
                let zone = CopperZone {
                    id: constraint
                        .constraint_id
                        .clone()
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| format!("zone:{net}:{index}")),
                    net: net.clone(),
                    layers,
                    outer,
                    holes,
                    priority,
                    clearance_nm,
                    minimum_width_nm,
                    pad_connection,
                    thermal_gap_nm: 300_000,
                    thermal_spoke_width_nm: 300_000,
                    island_policy,
                    minimum_island_area_nm2: (island_policy == IslandPolicy::RemoveBelowArea)
                        .then_some(10_000_000_000_000),
                    reserve_routing,
                    allow_same_net_hard_macro_overlap: allow_overlap,
                };
                zone.validate()?;
                zones.push(zone);
                continue;
            }
            ConstraintKind::Routing => {
                if constraint.targets.len() != 1 {
                    return fail("routing requires exactly one net target");
                }
                let net = &constraint.targets[0];
                if !net_names.contains(net.as_str()) {
                    return Err(Error::Value(format!("routing constraint references unknown net {}", repr(net))));
                }
                let rule = lower_routing_rule(net, parameters)?;
                rule.validate()?;
                routing.push(rule);
                continue;
            }
            ConstraintKind::LengthMatch | ConstraintKind::HoleClearance => continue,
            _ => {}
        }
        let courtyard = constraint.kind == ConstraintKind::CourtyardGap
            || (constraint.kind == ConstraintKind::PlacementRegion && parameters.contains_key("courtyard_gap"));
        if courtyard && !matches!(constraint.mode, ConstraintMode::Require | ConstraintMode::External) {
            return fail("courtyard_gap is a hard minimum; mode must be require or external");
        }
        let Some(lowered) = targets(&constraint.targets)? else {
            if courtyard {
                return fail("courtyard_gap target did not resolve to a physical component");
            }
            continue;
        };
        match constraint.kind {
            ConstraintKind::CourtyardGap => {
                if lowered.is_empty() || lowered.iter().any(|t| t.pad.is_some()) {
                    return fail("courtyard_gap requires component targets, not pins");
                }
                if parameters.len() != 1 || !parameters.contains_key("additional") {
                    return fail("courtyard_gap requires only the 'additional' parameter");
                }
                let gap = courtyard_gap_length(parameters, "additional")?;
                for target in &lowered {
                    let changes = RuleChanges { additional_courtyard_gap_nm: Some(Some(gap)), ..Default::default() };
                    let rule = merge_rule(rules.get(&target.reference), &target.reference, changes)?;
                    rules.insert(target.reference.clone(), rule);
                }
            }
            ConstraintKind::MaxDistance | ConstraintKind::MinDistance => {
                let distance = constraint_length(parameters, "distance")?;
                let weight = constraint_int(parameters, "weight", 10)?;
                if lowered.len() < 2 {
                    return fail("relative placement rule requires at least two targets");
                }
                if distance <= 0 {
                    return fail("distance placement rule requires a positive distance");
                }
                if weight <= 0 {
                    return fail("relative placement tolerance and weight are invalid");
                }
                let priority = constraint_int(parameters, "priority", 50)?;
                let kind = if constraint.kind == ConstraintKind::MaxDistance {
                    RelativePlacementKind::MaxDistance
                } else {
                    RelativePlacementKind::MinDistance
                };
                relative.push(RelativePlacementRule {
                    kind,
                    targets: placement_targets(&lowered),
                    distance_nm: Some(distance),
                    axis: None,
                    tolerance_nm: 0,
                    weight,
                });
                let references: Vec<String> = lowered.iter().map(|t| t.reference.clone()).collect();
                let anchor = references.last().cloned();
                groups.push(PlacementGroup::new(
                    &format!("proximity:{index}"),
                    &references,
                    anchor.as_deref(),
                    priority,
                    "constraint",
                )?);
            }
            ConstraintKind::PlacementRegion => {
                let gap = if parameters.contains_key("courtyard_gap") {
                    Some(courtyard_gap_length(parameters, "courtyard_gap")?)
                } else {
                    None
                };
                if gap.is_some() && (lowered.is_empty() || lowered.iter().any(|t| t.pad.is_some())) {
                    return fail("placement_region courtyard_gap requires component targets, not pins");
                }
                let name = parameters.get("name").map_or_else(|| format!("region:{index}"), parameter_str);
                let region = if let Some(polygon) = parameters.get("polygon_mm") {
                    if ["x", "y", "width", "height"].iter().any(|k| parameters.contains_key(*k)) {
                        return fail("placement_region boundary parameters are mutually exclusive");
                    }
                    polygon_boundary(polygon, "placement_region")?
                } else {
                    let width = constraint_length(parameters, "width")?;
                    let height = constraint_length(parameters, "height")?;
                    let origin = Point::new(constraint_length(parameters, "x")?, constraint_length(parameters, "y")?);
                    BoardOutline::rectangle(width, height, Some(origin))?
                };
                let side = constraint_side(parameters.get("side"))?;
                if name.is_empty() {
                    return fail("placement region name cannot be empty");
                }
                if !region.cutouts.is_empty() {
                    return fail("placement region outlines do not support cutouts");
                }
                region_names.push(name.clone());
                regions.push(PlacementRegion { name: name.clone(), outline: region, side });
                for target in &lowered {
                    let changes = RuleChanges {
                        region: Some(Some(name.clone())),
                        side: Some(side),
                        additional_courtyard_gap_nm: Some(gap),
                        ..Default::default()
                    };
                    let rule = merge_rule(rules.get(&target.reference), &target.reference, changes)?;
                    rules.insert(target.reference.clone(), rule);
                }
            }
            ConstraintKind::Keepout => {
                let name = parameters.get("name").map_or_else(|| format!("keepout:{index}"), parameter_str);
                let width = constraint_length(parameters, "width")?;
                let height = constraint_length(parameters, "height")?;
                let origin = Point::new(constraint_length(parameters, "x")?, constraint_length(parameters, "y")?);
                let shape = BoardOutline::rectangle(width, height, Some(origin))?;
                let side = constraint_side(parameters.get("side"))?;
                let maximum = optional_length(parameters, "maximum_height")?;
                keepouts.push(PlacementKeepout::new(&name, shape, side, maximum)?);
            }
            ConstraintKind::FixedPlacement => {
                if lowered.len() != 1 {
                    return fail("fixed_placement requires exactly one component");
                }
                let target = &lowered[0];
                if parameters.contains_key("x") != parameters.contains_key("y") {
                    return fail("fixed_placement position requires both x and y");
                }
                if !["x", "rotation", "side"].iter().any(|k| parameters.contains_key(*k)) {
                    return fail("fixed_placement requires position, rotation or side");
                }
                if target.pad.is_some() {
                    return fail("fixed_placement requires a component, not a pin");
                }
                let rotation = match parameters.get("rotation") {
                    None => None,
                    Some(ParameterValue::Int(number)) => {
                        Some(Decimal::parse(&number.to_string()).map_err(crate::decimal_error)?)
                    }
                    Some(ParameterValue::Float(number)) => Some(crate::float_decimal(*number)?),
                    Some(_) => return Err(Error::NotImplemented("a non-numeric fixed_placement rotation".to_owned())),
                };
                let existing = rules.get(&target.reference);
                let mut orientations = match (&rotation, existing) {
                    (Some(rotation), _) => vec![rotation.clone()],
                    (None, Some(existing)) => existing.allowed_orientations.clone(),
                    (None, None) => [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
                };
                for other in &flat.constraints {
                    if other.kind == ConstraintKind::AllowedOrientations
                        && targets(&other.targets)?.unwrap_or_default().iter().any(|t| t.reference == target.reference)
                    {
                        orientations = constraint_orientations(other.parameters.get("values"))?;
                    }
                }
                let position = if parameters.contains_key("x") {
                    Some(Point::new(constraint_length(parameters, "x")?, constraint_length(parameters, "y")?))
                } else {
                    None
                };
                let edge = optional_length(parameters, "edge_clearance")?;
                let side = constraint_side(parameters.get("side"))?;
                let priority = constraint_int(parameters, "priority", 1000)?;
                let changes = RuleChanges {
                    fixed_position: Some(position),
                    fixed_rotation_degrees: Some(rotation),
                    allowed_orientations: Some(orientations),
                    edge_clearance_nm: Some(edge),
                    side: Some(side),
                    priority: Some(priority),
                    ..Default::default()
                };
                let rule = merge_rule(rules.get(&target.reference), &target.reference, changes)?;
                rules.insert(target.reference.clone(), rule);
            }
            ConstraintKind::AllowedOrientations => {
                let orientations = constraint_orientations(parameters.get("values"))?;
                for target in &lowered {
                    let changes =
                        RuleChanges { allowed_orientations: Some(orientations.clone()), ..Default::default() };
                    let rule = merge_rule(rules.get(&target.reference), &target.reference, changes)?;
                    rules.insert(target.reference.clone(), rule);
                }
            }
            ConstraintKind::Align => {
                let axis = enum_value(parameters, "axis", "x", "AlignmentAxis", AlignmentAxis::parse)?;
                let tolerance =
                    if parameters.contains_key("tolerance") { constraint_length(parameters, "tolerance")? } else { 0 };
                let weight = constraint_int(parameters, "weight", 5)?;
                if lowered.len() < 2 {
                    return fail("relative placement rule requires at least two targets");
                }
                if tolerance < 0 || weight <= 0 {
                    return fail("relative placement tolerance and weight are invalid");
                }
                relative.push(RelativePlacementRule {
                    kind: RelativePlacementKind::Align,
                    targets: placement_targets(&lowered),
                    distance_nm: None,
                    axis: Some(axis),
                    tolerance_nm: tolerance,
                    weight,
                });
            }
            ConstraintKind::PlacementGroup => {
                let references: Vec<String> = lowered.iter().map(|t| t.reference.clone()).collect();
                let name = parameters.get("name").map_or_else(|| format!("group:{index}"), parameter_str);
                let Some(first) = references.first() else {
                    return Err(Error::NotImplemented("an empty placement_group".to_owned()));
                };
                let anchor = parameters.get("anchor").map_or_else(|| first.clone(), parameter_str);
                let priority = constraint_int(parameters, "priority", 0)?;
                groups.push(PlacementGroup::new(&name, &references, Some(&anchor), priority, "constraint")?);
            }
            _ => {}
        }
    }
    let pin_count = |reference: &str| components[reference].part.pins.len();
    // `max(members, key=...)`: the first member with the most pins.
    let anchor_of = |members: &[String]| {
        let mut best = &members[0];
        for member in members {
            if pin_count(member) > pin_count(best) {
                best = member;
            }
        }
        best.clone()
    };
    for module in &flat.module_instances {
        let prefix = format!("{}/", module.path);
        let mut members: Vec<String> =
            components.keys().filter(|r| r.starts_with(&prefix)).map(|r| (*r).to_owned()).collect();
        members.sort();
        if !members.is_empty() {
            let anchor = anchor_of(&members);
            groups.push(PlacementGroup::new(
                &format!("module:{}", module.path),
                &members,
                Some(&anchor),
                100,
                "hierarchy",
            )?);
        }
    }
    for interface in &flat.interfaces {
        let mut members: Vec<String> =
            interface.bindings.keys().filter(|r| components.contains_key(r.as_str())).cloned().collect();
        members.sort();
        if members.len() >= 2 {
            let anchor = anchor_of(&members);
            groups.push(PlacementGroup::new(
                &format!("interface:{}", interface.name),
                &members,
                Some(&anchor),
                25,
                "interface",
            )?);
        }
    }
    let mut ordered: Vec<ComponentPlacementRule> = rules.into_values().collect();
    ordered.sort_by(|a, b| a.reference.cmp(&b.reference));
    let mut omitted: Vec<String> = skipped.into_inner();
    omitted.sort();
    omitted.dedup();
    Ok(Lowered {
        region_names,
        regions,
        relative,
        keepouts,
        rules: ordered,
        groups: unique_groups(groups),
        routing,
        zones,
        via_in_pad,
        omitted_targets: (!omitted.is_empty()).then(|| omitted.join(",")),
    })
}

/// `_unique_groups`: a repeated name gets a `:N` suffix.
fn unique_groups(groups: Vec<PlacementGroup>) -> Vec<PlacementGroup> {
    let mut names: IndexMap<String, usize> = IndexMap::new();
    groups
        .into_iter()
        .map(|mut group| {
            let count = names.get(&group.name).copied().unwrap_or(0);
            names.insert(group.name.clone(), count + 1);
            if count != 0 {
                group.name = format!("{}:{count}", group.name);
            }
            group
        })
        .collect()
}

fn lower_routing_rule(net: &str, parameters: &Parameters) -> Result<NetRoutingRule> {
    let kind = enum_value(parameters, "kind", "general", "RouteKind", RouteKind::parse)?;
    let priority = constraint_int(parameters, "priority", 0)?;
    let width_nm = optional_length(parameters, "width")?;
    let clearance_nm = optional_length(parameters, "clearance")?;
    let allowed_layers = constraint_layers(parameters.get("allowed_layers"))?;
    let max_vias = optional_int(parameters, "max_vias")?;
    let max_length_nm = optional_length(parameters, "max_length")?;
    let differential_partner = optional_string(parameters, "partner")?;
    let pair_gap_nm = optional_length(parameters, "pair_gap")?;
    let max_skew_nm = optional_length(parameters, "max_skew")?;
    let topology = parameters.get("topology").map_or_else(|| "point_to_point".to_owned(), parameter_str);
    let target_impedance_ohms = optional_int(parameters, "target_impedance_ohms")?;
    let maximum_uncoupled_length_nm = optional_length(parameters, "maximum_uncoupled_length")?;
    let maximum_stub_length_nm = optional_length(parameters, "maximum_stub_length")?;
    let tuning_amplitude_limit_nm = optional_length(parameters, "tuning_amplitude_limit")?;
    let require_return_vias = constraint_bool(parameters, "require_return_vias", false)?;
    let reserve_corridor = constraint_bool(parameters, "reserve_corridor", false)?;
    if reserve_corridor && !parameters.contains_key("partner") {
        return Err(Error::Value(format!(
            "routing constraint for {}: reserve_corridor requires a differential partner",
            repr(net)
        )));
    }
    let return_via_net = optional_string(parameters, "return_via_net")?;
    let maximum_return_via_distance_nm = optional_length(parameters, "maximum_return_via_distance")?;
    let impedance_evidence_digest = optional_string(parameters, "impedance_evidence_digest")?;
    let return_via_policy =
        enum_value(parameters, "return_via_policy", "always", "ReturnViaPolicy", ReturnViaPolicy::parse)?;
    let shared_reference_layer = match parameters.get("shared_reference_layer") {
        None => None,
        Some(value) => {
            let text = parameter_str(value);
            Some(
                CopperLayer::parse(&text)
                    .ok_or_else(|| Error::Value(format!("{} is not a valid CopperLayer", repr(&text))))?,
            )
        }
    };
    let target_single_ended_ohms = optional_int(parameters, "target_single_ended_ohms")?;
    let impedance_tolerance_percent = optional_percent(parameters, "impedance_tolerance_percent")?;
    let layer_group = optional_string(parameters, "layer_group")?;
    let breakout_length_nm = optional_length(parameters, "breakout_length")?;
    let breakout_width_nm = optional_length(parameters, "breakout_width")?;
    let breakout_gap_nm = optional_length(parameters, "breakout_gap")?;
    let breakout_clearance_nm = optional_length(parameters, "breakout_clearance")?;
    let tuning_style = enum_value(parameters, "tuning_style", "bumps", "TuningStyle", TuningStyle::parse)?;
    let tuning_spacing_nm = optional_length(parameters, "tuning_spacing")?;
    let tuning_group = optional_string(parameters, "tuning_group")?;
    Ok(NetRoutingRule {
        net: net.to_owned(),
        kind,
        priority,
        width_nm,
        clearance_nm,
        allowed_layers,
        max_vias,
        max_length_nm,
        differential_partner,
        pair_gap_nm,
        max_skew_nm,
        topology,
        target_impedance_ohms,
        maximum_uncoupled_length_nm,
        maximum_stub_length_nm,
        tuning_amplitude_limit_nm,
        require_return_vias,
        reserve_corridor,
        return_via_net,
        maximum_return_via_distance_nm,
        impedance_evidence_digest,
        return_via_policy,
        shared_reference_layer,
        target_single_ended_ohms,
        impedance_tolerance_percent,
        layer_group,
        breakout_length_nm,
        breakout_width_nm,
        breakout_gap_nm,
        breakout_clearance_nm,
        tuning_style,
        tuning_spacing_nm,
        tuning_group,
    })
}

/// `_lower_decoupling`: each decoupler's explicit association.
fn lower_decoupling(flat: &FlatElectricalView, nets: &[PhysicalNet]) -> Result<Vec<DecouplingLink>> {
    let components: IndexMap<&str, &ComponentInstance> =
        flat.components.iter().map(|c| (c.reference.as_str(), c)).collect();
    let assigned: IndexMap<&PadReference, &str> =
        nets.iter().flat_map(|net| net.pads.iter().map(move |pad| (pad, net.name.as_str()))).collect();
    let ground: IndexSet<&str> =
        flat.supplies.iter().filter(|s| s.voltage.base_value().is_zero()).map(|s| s.net.as_str()).collect();
    let mut seen: IndexSet<&str> = IndexSet::new();
    let mut links = Vec::new();
    for component in &flat.components {
        if component.properties.get("role").map(String::as_str) != Some("decoupling") {
            continue;
        }
        let part = &flat.library[&component.part];
        if part.category != "passive.capacitor" {
            return fail("decoupling role requires a capacitor part");
        }
        let target_text = component
            .properties
            .get("decouples")
            .ok_or_else(|| Error::NotImplemented("decoupling without a target".to_owned()))?;
        let endpoint = copper_lang::model::Endpoint::parse(target_text)?;
        let Some(target_component) = components.get(endpoint.component.as_str()) else {
            return Err(Error::Value(format!("unknown decoupling target {}.{}", endpoint.component, endpoint.pin)));
        };
        let number =
            physical_pin_number(target_component, &flat.library[&target_component.part], &flat.devices, &endpoint.pin);
        let target = number.map(|n| PadReference::new(&endpoint.component, &n));
        let net = target.as_ref().and_then(|t| assigned.get(t).copied());
        let pads: Vec<&PadReference> =
            assigned.keys().filter(|p| p.component == component.reference).copied().collect();
        let feed: Vec<&&PadReference> = pads.iter().filter(|p| Some(assigned[**p]) == net).collect();
        let returns: Vec<&&PadReference> = pads.iter().filter(|p| ground.contains(assigned[**p])).collect();
        if net.is_none() || pads.len() != 2 || feed.len() != 1 || returns.len() != 1 || feed == returns {
            return Err(Error::Value(format!(
                "{}: decoupling requires two connected lands, target net and declared zero-volt return",
                component.reference
            )));
        }
        let (feed, back, target) = (*feed[0], *returns[0], target.expect("netted target"));
        if feed.component == target.component || net == Some(assigned[back]) {
            return fail("invalid decoupling association");
        }
        if !seen.insert(component.reference.as_str()) {
            return fail("a decoupler requires one explicit protected pin association");
        }
        links.push(DecouplingLink {
            capacitor: feed.clone(),
            target,
            return_pad: back.clone(),
            net: net.expect("netted target").to_owned(),
            return_net: assigned[back].to_owned(),
        });
    }
    Ok(links)
}

/// `_lower_match_groups`: the nets of each `length_match` constraint.
fn lower_match_groups(flat: &FlatElectricalView) -> Result<Vec<(String, Vec<String>, i64)>> {
    let nets: IndexSet<&str> = flat.nets.iter().map(|n| n.name.as_str()).collect();
    let mut groups: Vec<(String, Vec<String>, i64)> = Vec::new();
    let mut owner: IndexMap<String, String> = IndexMap::new();
    for (index, constraint) in flat.constraints.iter().enumerate() {
        if constraint.kind != ConstraintKind::LengthMatch {
            continue;
        }
        let origin = constraint.origins.first().cloned().unwrap_or_else(|| "length_match".to_owned());
        let result = (|| -> Result<(String, Vec<String>, i64)> {
            let id = constraint
                .constraint_id
                .clone()
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| format!("length_match:{index}"));
            if groups.iter().any(|(existing, _, _)| *existing == id) {
                return Err(Error::Value(format!("duplicate length_match id {}", repr(&id))));
            }
            for net in &constraint.targets {
                if !nets.contains(net.as_str()) {
                    return Err(Error::Value(format!("length_match references unknown net {}", repr(net))));
                }
                if let Some(previous) = owner.get(net) {
                    return Err(Error::Value(format!(
                        "net {} already belongs to length_match group {}",
                        repr(net),
                        repr(previous)
                    )));
                }
                owner.insert(net.clone(), id.clone());
            }
            let skew = constraint_length(&constraint.parameters, "max_skew")?;
            if constraint.targets.len() < 2 {
                return Err(Error::Value(format!("length-match group {} requires at least two nets", repr(&id))));
            }
            if crate::has_duplicates(&constraint.targets) || constraint.targets.iter().any(String::is_empty) {
                return Err(Error::Value(format!("length-match group {} nets must be unique and nonempty", repr(&id))));
            }
            if skew <= 0 {
                return Err(Error::Value(format!("length-match group {} maximum skew must be positive", repr(&id))));
            }
            Ok((id, constraint.targets.clone(), skew))
        })();
        match result {
            Ok(group) => groups.push(group),
            Err(Error::Value(message)) => return Err(Error::Value(format!("{origin}: {message}"))),
            Err(other) => return Err(other),
        }
    }
    Ok(groups)
}

/// `_check_tuning_groups`: a tuning group stays within one length_match group.
fn check_tuning_groups(flat: &FlatElectricalView, groups: &[(String, Vec<String>)]) -> Result<()> {
    let owner: IndexMap<&str, &str> =
        groups.iter().flat_map(|(id, nets)| nets.iter().map(move |net| (net.as_str(), id.as_str()))).collect();
    let mut declared: IndexMap<&str, (Option<String>, String)> = IndexMap::new();
    for constraint in &flat.constraints {
        if constraint.kind == ConstraintKind::Routing && constraint.targets.len() == 1 {
            let name = constraint.parameters.get("tuning_group").map(parameter_str);
            let origin = constraint.origins.first().cloned().unwrap_or_else(|| "routing".to_owned());
            declared.insert(constraint.targets[0].as_str(), (name, origin));
        }
    }
    let mut match_of: IndexMap<String, (String, String)> = IndexMap::new();
    for constraint in &flat.constraints {
        if constraint.kind != ConstraintKind::Routing || constraint.targets.len() != 1 {
            continue;
        }
        let net = constraint.targets[0].as_str();
        let (name, origin) = declared[net].clone();
        if let Some(ParameterValue::Text(partner)) = constraint.parameters.get("partner")
            && let Some((other, _)) = declared.get(partner.as_str())
            && *other != name
        {
            return Err(Error::Value(format!(
                "{origin}: CMP117: routing rules for {} and {} must agree on tuning_group",
                repr(net),
                repr(partner)
            )));
        }
        let Some(name) = name else { continue };
        let Some(group) = owner.get(net) else {
            return Err(Error::Value(format!(
                "{origin}: CMP117: tuning_group {} requires {} to belong to a length_match group",
                repr(&name),
                repr(net)
            )));
        };
        let first = match_of.entry(name.clone()).or_insert_with(|| ((*group).to_owned(), net.to_owned())).clone();
        if first.0 != *group {
            return Err(Error::Value(format!(
                "{origin}: CMP117: tuning_group {} joins {} of length_match group {} and {} of {}; a tuning group must stay within one length_match group",
                repr(&name),
                repr(net),
                repr(group),
                repr(&first.1),
                repr(&first.0)
            )));
        }
    }
    Ok(())
}

/// `_lower_hole_clearances`: component-scoped NPTH clearances (none for proxies).
fn lower_hole_clearances(
    flat: &FlatElectricalView,
    footprints: &IndexMap<String, PhysicalFootprint>,
    placements: &[Placement],
    rules: &DesignRules,
    proxy: bool,
) -> Result<Vec<ComponentHoleClearance>> {
    if proxy {
        return Ok(Vec::new());
    }
    let poses: IndexMap<&str, &Placement> = placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let components: IndexSet<&str> = flat.components.iter().map(|c| c.reference.as_str()).collect();
    let mut lowered: IndexMap<String, ComponentHoleClearance> = IndexMap::new();
    for constraint in &flat.constraints {
        if constraint.kind != ConstraintKind::HoleClearance {
            continue;
        }
        let origin = constraint.origins.first().cloned().unwrap_or_else(|| "hole_clearance".to_owned());
        let reference = &constraint.targets[0];
        if !components.contains(reference.as_str()) {
            return Err(Error::Value(format!(
                "{origin}: CMP113: hole_clearance target {} is not a component",
                repr(reference)
            )));
        }
        if lowered.contains_key(reference) {
            return Err(Error::Value(format!(
                "{origin}: CMP114: component {} already has a hole_clearance constraint",
                repr(reference)
            )));
        }
        let has_hole = poses.get(reference.as_str()).is_some_and(|pose| {
            footprints[&pose.footprint].pads.iter().any(|pad| pad.kind == PadKind::NonPlatedThroughHole)
        });
        if !has_hole {
            return Err(Error::Value(format!(
                "{origin}: CMP115: component {} has no footprint with non-plated holes for hole_clearance to apply to",
                repr(reference)
            )));
        }
        let clearance = constraint_length(&constraint.parameters, "clearance")?;
        if clearance > rules.minimum_hole_clearance_nm {
            return Err(Error::Value(format!(
                "{origin}: CMP116: hole_clearance for {} may only relax (not exceed) the board minimum hole clearance ({} nm)",
                repr(reference),
                rules.minimum_hole_clearance_nm
            )));
        }
        let reason = constraint.parameters.get("reason").map(parameter_str).unwrap_or_default();
        if clearance <= 0 {
            return Err(Error::Value(format!("hole clearance for {} must be positive", repr(reference))));
        }
        if crate::py_strip(&reason).is_empty() {
            return Err(Error::Value(format!("hole clearance for {} requires a reason", repr(reference))));
        }
        lowered.insert(
            reference.clone(),
            ComponentHoleClearance { reference: reference.clone(), clearance_nm: clearance, reason },
        );
    }
    let mut result: Vec<ComponentHoleClearance> = lowered.into_values().collect();
    result.sort_by(|a, b| a.reference.cmp(&b.reference));
    Ok(result)
}

// ---------------------------------------------------------------------------
// Mechanics

fn binding_reference(value: &copper_lang::syntax::Value) -> String {
    pyvalue::text(value)
}

/// `_bind_profile_connectors`: profile connector bindings become fixed poses.
fn bind_profile_connectors(
    mechanical: &MechanicalDesign,
    flat: &FlatElectricalView,
    footprints: &IndexMap<String, PhysicalFootprint>,
    placements: &[Placement],
    rules: Vec<ComponentPlacementRule>,
) -> Result<Vec<ComponentPlacementRule>> {
    let poses: IndexMap<&str, &Placement> = placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let mut by_reference: IndexMap<String, ComponentPlacementRule> =
        rules.into_iter().map(|rule| (rule.reference.clone(), rule)).collect();
    let components: IndexMap<&str, &ComponentInstance> =
        flat.components.iter().map(|c| (c.reference.as_str(), c)).collect();
    for binding in &mechanical.connectors {
        let reference = binding_reference(&binding.reference);
        let prefix = format!("{}: connector role {}: ", binding.location, repr(&binding.role));
        let Some(pose) = poses.get(reference.as_str()) else {
            return Err(Error::Value(prefix + "bound component needs a selected footprint; it cannot be omitted"));
        };
        let footprint = &footprints[&pose.footprint];
        let component = components[reference.as_str()];
        let chosen = selected_footprint(component, &flat.library[&component.part]);
        if let Some(required) = &binding.footprint
            && Some(required) != chosen.as_ref()
        {
            return Err(Error::Value(format!(
                "{prefix}requires footprint {}, got {}",
                repr(required),
                chosen.as_deref().map_or_else(|| "None".to_owned(), repr)
            )));
        }
        let pads: Vec<&FootprintPad> =
            footprint.pads.iter().filter(|p| p.number == binding.anchor_pad && p.kind.is_electrical()).collect();
        if pads.len() != 1 {
            return Err(Error::Value(prefix + "anchor_pad must identify exactly one electrical physical land"));
        }
        let zero = Placement {
            position: Point::new(0, 0),
            rotation_degrees: crate::decimal_rem(&binding.rotation, 360)?,
            side: binding.side,
            ..(*pose).clone()
        };
        let offset = transformed_local_point(&zero, pads[0].position);
        let origin = Point::new(binding.position.x_nm - offset.x_nm, binding.position.y_nm - offset.y_nm);
        let existing = by_reference.get(&reference);
        if let Some(existing) = existing {
            if existing.fixed_position.is_some_and(|old| old != origin) {
                return Err(Error::Value(prefix + "conflicts with existing fixed_position"));
            }
            if existing.fixed_rotation_degrees.as_ref().is_some_and(|old| *old != binding.rotation) {
                return Err(Error::Value(prefix + "conflicts with existing fixed_rotation_degrees"));
            }
            if existing.side.is_some_and(|old| old != binding.side) {
                return Err(Error::Value(prefix + "conflicts with existing side"));
            }
        }
        let explicit = flat.constraints.iter().any(|c| {
            c.kind == ConstraintKind::AllowedOrientations
                && c.targets.iter().any(|t| t.split('.').next() == Some(reference.as_str()))
        });
        let orientations = match existing {
            Some(existing) if explicit => existing.allowed_orientations.clone(),
            _ => vec![binding.rotation.clone()],
        };
        let priority = existing.map_or(0, |e| e.priority).max(1000);
        let changes = RuleChanges {
            fixed_position: Some(Some(origin)),
            fixed_rotation_degrees: Some(Some(binding.rotation.clone())),
            side: Some(Some(binding.side)),
            allowed_orientations: Some(orientations),
            priority: Some(priority),
            ..Default::default()
        };
        let rule = merge_rule(existing, &reference, changes)?;
        by_reference.insert(reference, rule);
    }
    let mut result: Vec<ComponentPlacementRule> = by_reference.into_values().collect();
    result.sort_by(|a, b| a.reference.cmp(&b.reference));
    Ok(result)
}

/// `mechanical_anchors.bind_attachments`: attachments become fixed poses.
fn bind_attachments(
    mechanical: &MechanicalDesign,
    flat: &FlatElectricalView,
    footprints: &IndexMap<String, PhysicalFootprint>,
    placements: &[Placement],
    rules: Vec<ComponentPlacementRule>,
) -> Result<(Vec<ComponentPlacementRule>, Vec<PhysicalAttachment>)> {
    let mut resolved = Vec::new();
    let explicit: IndexSet<&str> = flat
        .constraints
        .iter()
        .filter(|c| c.kind == ConstraintKind::AllowedOrientations)
        .flat_map(|c| c.targets.iter().map(String::as_str))
        .collect();
    let poses: IndexMap<&str, &Placement> = placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let mut by_reference: IndexMap<String, ComponentPlacementRule> =
        rules.into_iter().map(|rule| (rule.reference.clone(), rule)).collect();
    for binding in &mechanical.attachments {
        let Some(pose) = poses.get(binding.reference.as_str()) else {
            return Err(Error::Value(format!(
                "attachment {} requires a selected component footprint",
                repr(&binding.id)
            )));
        };
        let mut local = binding.anchor_point.unwrap_or(Point::new(0, 0));
        if binding.anchor == "pad" {
            let pads: Vec<&FootprintPad> = footprints[&pose.footprint]
                .pads
                .iter()
                .filter(|p| Some(&p.number) == binding.anchor_pad.as_ref() && p.kind.is_electrical())
                .collect();
            if pads.len() != 1 {
                return fail("attachment anchor_pad must identify exactly one electrical physical land");
            }
            local = pads[0].position;
        }
        let zero = Placement {
            position: Point::new(0, 0),
            rotation_degrees: crate::decimal_rem(&binding.rotation, 360)?,
            side: binding.side,
            ..(*pose).clone()
        };
        let offset = transformed_local_point(&zero, local);
        let origin = Point::new(binding.position.x_nm - offset.x_nm, binding.position.y_nm - offset.y_nm);
        let previous = by_reference.get(&binding.reference);
        let rotation = binding.rotation.clone();
        if let Some(previous) = previous {
            let conflict =
                |key: &str| Error::Value(format!("attachment {} conflicts with existing {key}", repr(&binding.id)));
            if previous.fixed_position.is_some_and(|old| old != origin) {
                return Err(conflict("fixed_position"));
            }
            if previous.fixed_rotation_degrees.as_ref().is_some_and(|old| *old != rotation) {
                return Err(conflict("fixed_rotation_degrees"));
            }
            if previous.side.is_some_and(|old| old != binding.side) {
                return Err(conflict("side"));
            }
            if explicit.contains(binding.reference.as_str()) && !previous.allowed_orientations.contains(&rotation) {
                return Err(Error::Value(format!(
                    "attachment {} conflicts with allowed orientations",
                    repr(&binding.id)
                )));
            }
        }
        let orientations = match previous {
            Some(previous) if explicit.contains(binding.reference.as_str()) => previous.allowed_orientations.clone(),
            _ => vec![rotation.clone()],
        };
        let changes = RuleChanges {
            fixed_position: Some(Some(origin)),
            fixed_rotation_degrees: Some(Some(rotation)),
            side: Some(Some(binding.side)),
            allowed_orientations: Some(orientations),
            priority: Some(previous.map_or(0, |p| p.priority).max(1000)),
            ..Default::default()
        };
        let rule = merge_rule(previous, &binding.reference, changes)?;
        by_reference.insert(binding.reference.clone(), rule);
        resolved.push(PhysicalAttachment {
            id: binding.id.clone(),
            reference: binding.reference.clone(),
            target: binding.target.clone(),
            position: binding.position,
            offset: binding.offset,
            anchor: binding.anchor.clone(),
            anchor_point: local,
            rotation: binding.rotation.clone(),
            side: binding.side,
        });
    }
    let mut result: Vec<ComponentPlacementRule> = by_reference.into_values().collect();
    result.sort_by(|a, b| a.reference.cmp(&b.reference));
    Ok((result, resolved))
}

// ---------------------------------------------------------------------------
// PhysicalBoard.__post_init__

/// `copper_artwork_keepouts`: fresh reservations around copper artwork.
pub fn copper_artwork_keepouts(board: &PhysicalBoard) -> Result<Vec<CopperKeepout>> {
    use copper_lang::silkscreen::{artwork_bounds, artwork_layer};
    if !board.silkscreen.iter().any(|item| artwork_layer(item).is_some()) {
        return Ok(Vec::new());
    }
    let clearance = std::iter::once(board.rules.minimum_clearance_nm)
        .chain(board.net_routing_rules.iter().map(|r| r.clearance_nm.unwrap_or(0)))
        .chain(board.zones.iter().map(|z| z.clearance_nm.unwrap_or(0)))
        .max()
        .unwrap_or(0);
    let mut result = Vec::new();
    for item in &board.silkscreen {
        let Some(layer) = artwork_layer(item) else { continue };
        let (left, low, right, high) = artwork_bounds(item)?;
        let corners = vec![
            Point::new(left - clearance, low - clearance),
            Point::new(right + clearance, low - clearance),
            Point::new(right + clearance, high + clearance),
            Point::new(left - clearance, high + clearance),
        ];
        result.push(CopperKeepout {
            id: format!("__copper_artwork:{}", item.id()),
            layers: vec![layer],
            vertices: copper_physical::polygon_ring(&corners)?,
            block_tracks: true,
            block_vias: true,
            block_pads: true,
            block_zones: true,
            block_footprints: true,
        });
    }
    Ok(result)
}

/// `replace(board, ...)`: rerun the `PhysicalBoard.__post_init__` checks on a changed board.
pub fn revalidate(board: &mut PhysicalBoard) -> Result<()> {
    let groups: Vec<(String, Vec<String>)> =
        board.match_groups.iter().map(|g| (g.id.clone(), g.nets.clone())).collect();
    let regions: Vec<String> = board.regions.iter().map(|r| r.name.clone()).collect();
    finish_board(board, &groups, &regions)
}

/// The `PhysicalBoard.__post_init__` checks a draft export can reach.
pub fn finish_board(
    board: &mut PhysicalBoard,
    match_groups: &[(String, Vec<String>)],
    regions: &[String],
) -> Result<()> {
    board.copper_keepouts.retain(|k| !k.id.starts_with("__copper_artwork:"));
    let derived = copper_artwork_keepouts(board)?;
    board.copper_keepouts.extend(derived);
    for (name, ids) in [
        ("datums", board.datums.iter().map(|d| d.id.as_str()).collect::<Vec<_>>()),
        ("boundary_edges", board.boundary_edges.iter().map(|e| e.id.as_str()).collect()),
        ("mechanical_slots", board.mechanical_slots.iter().map(|s| s.id.as_str()).collect()),
    ] {
        if crate::has_duplicates(&ids) {
            return Err(Error::Value(format!("duplicate {name} identity")));
        }
    }
    copper_physical::mechanical::validate_mechanical_holes(
        &board.outline,
        &board.mechanical_holes,
        &board.mechanical_slots,
        &board.rules,
    )?;
    validate_references(board, match_groups, regions)
}

fn validate_references(
    board: &PhysicalBoard,
    match_groups: &[(String, Vec<String>)],
    regions: &[String],
) -> Result<()> {
    let references: Vec<&str> = board.placements.iter().map(|p| p.reference.as_str()).collect();
    if crate::has_duplicates(&references) {
        return fail("physical placement references must be unique");
    }
    let net_names: Vec<&str> = board.nets.iter().map(|n| n.name.as_str()).collect();
    if crate::has_duplicates(&net_names) {
        return fail("physical net names must be unique");
    }
    let zone_ids: Vec<&str> = board.zones.iter().map(|z| z.id.as_str()).collect();
    if crate::has_duplicates(&zone_ids) {
        return fail("copper zone ids must be unique");
    }
    let polygon_ids: Vec<&str> = board.polygons.iter().map(|p| p.id.as_str()).collect();
    if crate::has_duplicates(&polygon_ids) {
        return fail("fixed copper polygon ids must be unique");
    }
    let keepout_ids: Vec<&str> = board.copper_keepouts.iter().map(|k| k.id.as_str()).collect();
    if crate::has_duplicates(&keepout_ids) {
        return fail("copper keepout ids must be unique");
    }
    let layers = &board.stackup.copper_layers;
    for zone in &board.zones {
        if !net_names.contains(&zone.net.as_str()) {
            return Err(Error::Value(format!(
                "copper zone {} references unknown net {}",
                repr(&zone.id),
                repr(&zone.net)
            )));
        }
        if !zone.layers.iter().all(|layer| layers.contains(layer)) {
            return Err(Error::Value(format!("copper zone {} references a layer outside the stackup", repr(&zone.id))));
        }
    }
    for polygon in &board.polygons {
        if !net_names.contains(&polygon.net.as_str()) || !layers.contains(&polygon.layer) {
            return Err(Error::Value(format!(
                "fixed copper polygon {} has an unknown net or layer",
                repr(&polygon.id)
            )));
        }
    }
    validate_routing_reservations(&board.zones)?;
    for keepout in &board.copper_keepouts {
        if !keepout.layers.iter().all(|layer| layers.contains(layer)) {
            return Err(Error::Value(format!(
                "copper keepout {} references a layer outside the stackup",
                repr(&keepout.id)
            )));
        }
    }
    let mut pads_by_component: IndexMap<&str, Vec<&FootprintPad>> = IndexMap::new();
    for placement in &board.placements {
        let Some(footprint) = board.footprints.get(&placement.footprint) else {
            return Err(Error::Value(format!(
                "placement {} uses unknown footprint {}",
                repr(&placement.reference),
                repr(&placement.footprint)
            )));
        };
        pads_by_component.insert(placement.reference.as_str(), footprint.pads.iter().collect());
    }
    let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            let Some(pads) = pads_by_component.get(pad.component.as_str()) else {
                return Err(Error::Value(format!(
                    "net {} references unknown placement {}",
                    repr(&net.name),
                    repr(&pad.component)
                )));
            };
            let Some(land) = pads.iter().rev().find(|p| p.number == pad.pad) else {
                return Err(Error::Value(format!(
                    "net {} references unknown pad {}.{}",
                    repr(&net.name),
                    pad.component,
                    pad.pad
                )));
            };
            if !land.kind.is_electrical() {
                return Err(Error::Value(format!(
                    "non-electrical pad {}.{} cannot belong to an electrical net",
                    pad.component, pad.pad
                )));
            }
            if let Some(previous) = assigned.get(pad) {
                return Err(Error::Value(format!(
                    "pad {}.{} belongs to both {} and {}",
                    pad.component,
                    pad.pad,
                    repr(previous),
                    repr(&net.name)
                )));
            }
            assigned.insert(pad, net.name.as_str());
        }
    }
    for placement in &board.placements {
        for group in &board.footprints[&placement.footprint].internal_pad_groups {
            let nets: Vec<Option<&str>> = group
                .numbers
                .iter()
                .map(|n| assigned.get(&PadReference::new(&placement.reference, n)).copied())
                .collect();
            let distinct: IndexSet<&str> = nets.iter().flatten().copied().collect();
            if distinct.len() > 1 {
                return fail("internally connected pads belong to different nets");
            }
            if nets.contains(&None) && !distinct.is_empty() {
                return fail("all numbered pads in an internal group must share a net");
            }
        }
    }
    let mut permitted: Vec<&PadReference> = Vec::new();
    for rule in &board.via_in_pad_rules {
        if permitted.contains(&&rule.pad) {
            return fail("multiple via-in-pad permissions for one pad");
        }
        permitted.push(&rule.pad);
        if assigned.get(&rule.pad).copied() != Some("GND") {
            return fail("via-in-pad permission requires a connected GND pad");
        }
        let placement = board.placements.iter().find(|p| p.reference == rule.pad.component).expect("assigned pad");
        let lands: Vec<&FootprintPad> =
            board.footprints[&placement.footprint].pads.iter().filter(|p| p.number == rule.pad.pad).collect();
        if lands.iter().any(|p| p.kind != PadKind::Smd) {
            return fail("via-in-pad permission requires SMD lands");
        }
        if board.metadata.get("fabrication_profile").map(String::as_str) != Some("jlcpcb-six-layer")
            || layers.len() != 6
        {
            return fail("filled-capped via-in-pad requires the JLCPCB six-layer profile");
        }
        if !board
            .zones
            .iter()
            .any(|z| z.net == "GND" && z.layers.iter().any(|l| !matches!(l, CopperLayer::Front | CopperLayer::Back)))
        {
            return fail("via-in-pad permission requires a declared inner GND zone");
        }
        if rule.rows.is_some() {
            return crate::not_implemented("via-in-pad via arrays");
        }
    }
    for track in &board.tracks {
        if !net_names.contains(&track.net.as_str()) {
            return Err(Error::Value(format!("track references unknown net {}", repr(&track.net))));
        }
        if !layers.contains(&track.layer) {
            return Err(Error::Value(format!("track uses unavailable layer {}", repr(track.layer.value()))));
        }
    }
    for via in &board.vias {
        if !net_names.contains(&via.net.as_str()) {
            return Err(Error::Value(format!("via references unknown net {}", repr(&via.net))));
        }
        if !layers.contains(&via.from_layer) || !layers.contains(&via.to_layer) {
            return fail("via uses a layer not present in the stackup");
        }
        // No source declares a via technology catalog.
        if let Some(technology) = &via.technology {
            return Err(Error::Value(format!("via references unknown technology {}", repr(technology))));
        }
    }
    if crate::has_duplicates(regions) {
        return fail("physical placement region names must be unique");
    }
    let keepout_names: Vec<&str> = board.keepouts.iter().map(|k| k.name.as_str()).collect();
    if crate::has_duplicates(&keepout_names) {
        return fail("physical placement keepout names must be unique");
    }
    let mut ruled: Vec<&str> = Vec::new();
    for rule in &board.placement_rules {
        if !references.contains(&rule.reference.as_str()) {
            return Err(Error::Value(format!("placement rule references unknown component {}", repr(&rule.reference))));
        }
        if ruled.contains(&rule.reference.as_str()) {
            return Err(Error::Value(format!("component {} has multiple placement rules", repr(&rule.reference))));
        }
        ruled.push(rule.reference.as_str());
        if let Some(region) = &rule.region
            && !regions.contains(region)
        {
            return Err(Error::Value(format!("placement rule references unknown region {}", repr(region))));
        }
    }
    crate::templates::validate_cluster_bindings(board)?;
    crate::hard_macros::validate_hard_macros(board)?;
    let mut routed: Vec<&str> = Vec::new();
    for rule in &board.net_routing_rules {
        if !net_names.contains(&rule.net.as_str()) {
            return Err(Error::Value(format!("routing rule references unknown net {}", repr(&rule.net))));
        }
        if routed.contains(&rule.net.as_str()) {
            return Err(Error::Value(format!("net {} has multiple routing rules", repr(&rule.net))));
        }
        routed.push(rule.net.as_str());
        let mut unavailable: Vec<&str> =
            rule.allowed_layers.iter().filter(|l| !layers.contains(l)).map(|l| l.value()).collect();
        unavailable.sort_unstable();
        if rule.shared_reference_layer.is_some_and(|l| !layers.contains(&l)) {
            return Err(Error::Value(format!(
                "routing rule for {} uses unavailable shared reference layer",
                repr(&rule.net)
            )));
        }
        if rule.shared_reference_layer.is_some_and(|l| rule.allowed_layers.contains(&l)) {
            return Err(Error::Value(format!(
                "routing rule for {} routes on its declared reference plane",
                repr(&rule.net)
            )));
        }
        if let Some(first) = unavailable.first() {
            return Err(Error::Value(format!(
                "routing rule for {} uses unavailable layer {}",
                repr(&rule.net),
                repr(first)
            )));
        }
        if let Some(partner) = &rule.differential_partner
            && !net_names.contains(&partner.as_str())
        {
            return Err(Error::Value(format!(
                "routing rule for {} references unknown partner {}",
                repr(&rule.net),
                repr(partner)
            )));
        }
        if let Some(net) = &rule.return_via_net
            && !net_names.contains(&net.as_str())
        {
            return Err(Error::Value(format!(
                "routing rule for {} references unknown return net {}",
                repr(&rule.net),
                repr(net)
            )));
        }
        let partner = board.net_routing_rules.iter().rev().find(|r| Some(&r.net) == rule.differential_partner.as_ref());
        if let Some(partner) = partner {
            if partner.tuning_style != rule.tuning_style {
                return Err(Error::Value(format!(
                    "routing rules for {} and {} must agree on tuning_style",
                    repr(&rule.net),
                    repr(&partner.net)
                )));
            }
            if partner.tuning_group != rule.tuning_group {
                return Err(Error::Value(format!(
                    "routing rules for {} and {} must agree on tuning_group",
                    repr(&rule.net),
                    repr(&partner.net)
                )));
            }
        }
        if let Some(width) = rule.breakout_width_nm {
            if width > rule.width_nm.unwrap_or(board.rules.default_track_width_nm) {
                return Err(Error::Value(format!(
                    "routing rule for {}: breakout width may only relax (not exceed) the effective routing width",
                    repr(&rule.net)
                )));
            }
            if width < board.rules.minimum_track_width_nm {
                return Err(Error::Value(format!(
                    "routing rule for {}: breakout width is below the board minimum track width",
                    repr(&rule.net)
                )));
            }
        }
        if let Some(clearance) = rule.breakout_clearance_nm {
            if clearance > rule.clearance_nm.unwrap_or(0).max(board.rules.minimum_clearance_nm) {
                return Err(Error::Value(format!(
                    "routing rule for {}: breakout clearance may only relax (not exceed) the effective routing clearance",
                    repr(&rule.net)
                )));
            }
            if clearance < board.rules.minimum_clearance_nm {
                return Err(Error::Value(format!(
                    "routing rule for {}: breakout clearance is below the board minimum clearance",
                    repr(&rule.net)
                )));
            }
        }
    }
    let ids: Vec<&str> = match_groups.iter().map(|(id, _)| id.as_str()).collect();
    if crate::has_duplicates(&ids) {
        return fail("length-match group ids must be unique");
    }
    let mut grouped: IndexMap<&str, &str> = IndexMap::new();
    for (id, nets) in match_groups {
        for net in nets {
            if !net_names.contains(&net.as_str()) {
                return Err(Error::Value(format!(
                    "length-match group {} references unknown net {}",
                    repr(id),
                    repr(net)
                )));
            }
            if let Some(previous) = grouped.get(net.as_str()) {
                return Err(Error::Value(format!(
                    "net {} belongs to length-match groups {} and {}",
                    repr(net),
                    repr(previous),
                    repr(id)
                )));
            }
            grouped.insert(net.as_str(), id.as_str());
        }
    }
    let mut tuning: IndexMap<&str, &str> = IndexMap::new();
    for rule in &board.net_routing_rules {
        let (Some(group), Some(owner)) = (&rule.tuning_group, grouped.get(rule.net.as_str())) else { continue };
        let first = *tuning.entry(group.as_str()).or_insert(owner);
        if first != *owner {
            return Err(Error::Value(format!(
                "tuning group {} spans length-match groups {} and {}",
                repr(group),
                repr(first),
                repr(owner)
            )));
        }
    }
    let mut scoped: Vec<&str> = Vec::new();
    for rule in &board.component_hole_clearances {
        if scoped.contains(&rule.reference.as_str()) {
            return Err(Error::Value(format!("component {} has more than one hole clearance", repr(&rule.reference))));
        }
        scoped.push(rule.reference.as_str());
        if rule.clearance_nm > board.rules.minimum_hole_clearance_nm {
            return Err(Error::Value(format!(
                "hole clearance for {} may only relax (not exceed) the board minimum hole clearance",
                repr(&rule.reference)
            )));
        }
    }
    Ok(())
}

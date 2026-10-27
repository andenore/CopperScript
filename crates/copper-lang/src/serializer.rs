//! Stable JSON of the compiled design (`pcbir.serializer`), the output of
//! `copper compile`: `json.dumps(board_to_dict(design), indent=2,
//! sort_keys=True)` plus a newline.

use std::path::Path;

use copper_json::{Dumps, Object, Value};
use copper_units::Quantity;
use indexmap::IndexMap;

use crate::compiler::Design;
use crate::model::*;
use crate::paths;

fn text(value: &str) -> Value {
    Value::from(value)
}

fn optional(value: &Option<String>) -> Value {
    value.as_deref().map(text).unwrap_or(Value::Null)
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn texts<'a>(values: impl IntoIterator<Item = &'a String>) -> Value {
    Value::Array(values.into_iter().map(|value| text(value)).collect())
}

fn sorted_texts<'a>(values: impl IntoIterator<Item = &'a str>) -> Value {
    let mut values: Vec<&str> = values.into_iter().collect();
    values.sort();
    Value::Array(values.into_iter().map(text).collect())
}

fn string_map(values: &IndexMap<String, String>) -> Value {
    Value::Object(values.iter().map(|(key, value)| (key.clone(), text(value))).collect())
}

/// `_value` for a quantity.
pub fn quantity_value(quantity: &Quantity) -> Value {
    object(vec![
        ("value", text(&quantity.value().decimal_text())),
        ("unit", text(quantity.display_unit())),
        ("base_value", text(&quantity.base_value().decimal_text())),
    ])
}

fn bound_value(bound: &Option<Bound>) -> Value {
    match bound {
        None => Value::Null,
        Some(Bound::Fixed(quantity)) => quantity_value(quantity),
        Some(Bound::Relative(relative)) => object(vec![
            ("reference", text(&relative.reference)),
            ("offset", quantity_value(&relative.offset)),
            ("limit", relative.limit.as_ref().map(quantity_value).unwrap_or(Value::Null)),
        ]),
    }
}

fn condition_value(condition: &Option<Condition>) -> Value {
    condition.as_ref().map(|condition| string_map(&condition.selections)).unwrap_or(Value::Null)
}

fn range_value(range: &Option<QuantityRange>) -> Value {
    let Some(range) = range else { return Value::Null };
    object(vec![
        ("minimum", bound_value(&range.minimum)),
        ("typical", bound_value(&range.typical)),
        ("maximum", bound_value(&range.maximum)),
        ("rating", text(&range.rating)),
        ("when", condition_value(&range.when)),
    ])
}

fn profile_value(profile: Option<&ElectricalProfile>) -> Value {
    let Some(profile) = profile else { return Value::Null };
    let mut result = vec![
        ("domains", sorted_texts(profile.domains.iter().map(|item| item.value()))),
        ("directions", sorted_texts(profile.directions.iter().map(|item| item.value()))),
        ("drive_modes", sorted_texts(profile.drive_modes.iter().map(|item| item.value()))),
        ("traits", sorted_texts(profile.traits.iter().map(String::as_str))),
        ("voltage", range_value(&profile.voltage)),
        ("current", range_value(&profile.current)),
    ];
    if profile.absolute_voltage.is_some() {
        result.push(("absolute_voltage", range_value(&profile.absolute_voltage)));
    }
    object(result)
}

fn source_value(source: &Option<SourceReference>) -> Value {
    let Some(source) = source else { return Value::Null };
    object(vec![
        ("document", optional(&source.document)),
        ("revision", optional(&source.revision)),
        ("location", optional(&source.location)),
        ("url", optional(&source.url)),
        ("checksum", optional(&source.checksum)),
    ])
}

fn part_value(part: &PartDefinition) -> Value {
    let pins = part
        .pins
        .values()
        .map(|pin| {
            let mut entries = vec![
                ("name", text(&pin.name)),
                ("number", text(&pin.number)),
                ("profile", profile_value(pin.profile.as_ref())),
                (
                    "bonds",
                    Value::Array(
                        pin.bonds
                            .iter()
                            .map(|bond| object(vec![("pad", text(&bond.pad)), ("when", condition_value(&bond.when))]))
                            .collect(),
                    ),
                ),
                ("connection_policy", text(pin.connection_policy.value())),
                ("required_net_traits", sorted_texts(pin.required_net_traits.iter().map(String::as_str))),
            ];
            if let Some(group) = &pin.parallel_power_group {
                entries.push(("parallel_power_group", text(group)));
            }
            object(entries)
        })
        .collect();
    object(vec![
        ("name", text(&part.name)),
        ("category", text(&part.category)),
        ("traits", sorted_texts(part.traits.iter().map(String::as_str))),
        ("manufacturer", optional(&part.manufacturer)),
        ("assembled", Value::Bool(part.assembled)),
        ("device", optional(&part.device)),
        ("source", source_value(&part.source)),
        ("footprints", texts(&part.footprints)),
        (
            "internal_pad_groups",
            Value::Array(part.internal_pad_groups.iter().map(|group| texts(&group.numbers)).collect()),
        ),
        ("metadata", string_map(&part.metadata)),
        ("pins", Value::Array(pins)),
    ])
}

fn device_value(device: &DeviceDefinition) -> Value {
    let pads = device
        .pads
        .values()
        .map(|pad| {
            object(vec![
                ("name", text(&pad.name)),
                ("profile", profile_value(Some(&pad.profile))),
                ("power_domain", optional(&pad.power_domain)),
                ("unpowered_behavior", text(pad.unpowered_behavior.value())),
                ("when", condition_value(&pad.when)),
            ])
        })
        .collect();
    let domains = device
        .power_domains
        .values()
        .map(|domain| {
            object(vec![
                ("name", text(&domain.name)),
                ("supply_pads", texts(&domain.supply_pads)),
                ("voltage", range_value(&domain.voltage)),
                ("requires", texts(&domain.requires)),
            ])
        })
        .collect();
    let peripherals = device
        .peripherals
        .values()
        .map(|peripheral| {
            let signals = peripheral
                .signals
                .values()
                .map(|signal| {
                    object(vec![
                        ("name", text(&signal.name)),
                        ("profile", profile_value(Some(&signal.profile))),
                        ("required", Value::Bool(signal.required)),
                    ])
                })
                .collect();
            object(vec![
                ("name", text(&peripheral.name)),
                ("kind", text(&peripheral.kind)),
                ("signals", Value::Array(signals)),
            ])
        })
        .collect();
    let muxes = device
        .mux_options
        .iter()
        .map(|option| {
            object(vec![
                ("pad", text(&option.pad)),
                ("peripheral", text(&option.peripheral)),
                ("signal", text(&option.signal)),
                ("selector", text(&option.selector)),
                ("resource", optional(&option.resource)),
                ("setting", optional(&option.setting)),
                ("when", condition_value(&option.when)),
            ])
        })
        .collect();
    let units = device
        .units
        .values()
        .map(|unit| {
            let terminals = unit
                .terminals
                .iter()
                .map(|(name, terminal)| {
                    (
                        name.clone(),
                        object(vec![
                            ("pad", text(&terminal.pad)),
                            ("profile", profile_value(terminal.profile.as_ref())),
                        ]),
                    )
                })
                .collect();
            object(vec![
                ("name", text(&unit.name)),
                ("kind", text(&unit.kind)),
                ("shared", Value::Bool(unit.shared)),
                ("terminals", Value::Object(terminals)),
            ])
        })
        .collect();
    let groups = device
        .signal_groups
        .values()
        .map(|group| {
            object(vec![
                ("name", text(&group.name)),
                ("kind", text(&group.kind)),
                ("members", string_map(&group.members)),
                ("profile", profile_value(group.profile.as_ref())),
                ("when", condition_value(&group.when)),
            ])
        })
        .collect();
    let modes = device
        .mode_groups
        .values()
        .map(|group| {
            object(vec![
                ("name", text(&group.name)),
                ("choices", texts(&group.choices)),
                ("default", optional(&group.default)),
            ])
        })
        .collect();
    let pad_sets = device
        .pad_sets
        .values()
        .map(|set| object(vec![("name", text(&set.name)), ("pads", texts(&set.pads))]))
        .collect();
    let routes = device
        .route_rules
        .iter()
        .map(|rule| {
            object(vec![
                ("peripheral", text(&rule.peripheral)),
                ("signal", text(&rule.signal)),
                ("pad_set", text(&rule.pad_set)),
                (
                    "selector",
                    object(vec![
                        ("kind", text(&rule.selector.kind)),
                        ("parameters", string_map(&rule.selector.parameters)),
                    ]),
                ),
                ("when", condition_value(&rule.when)),
            ])
        })
        .collect();
    object(vec![
        ("name", text(&device.name)),
        ("metadata", string_map(&device.metadata)),
        ("source", source_value(&device.source)),
        ("pads", Value::Array(pads)),
        ("power_domains", Value::Array(domains)),
        ("resources", texts(&device.resources)),
        ("peripherals", Value::Array(peripherals)),
        ("mux_options", Value::Array(muxes)),
        ("units", Value::Array(units)),
        ("signal_groups", Value::Array(groups)),
        ("mode_groups", Value::Array(modes)),
        ("pad_sets", Value::Array(pad_sets)),
        ("route_rules", Value::Array(routes)),
    ])
}

fn parameter_value(value: &ParameterValue) -> Value {
    match value {
        ParameterValue::Quantity(quantity) => quantity_value(quantity),
        ParameterValue::Text(value) => text(value),
        ParameterValue::Int(number) => Value::Int(number.clone()),
        ParameterValue::Float(number) => Value::Float(*number),
        ParameterValue::Bool(flag) => Value::Bool(*flag),
    }
}

/// `_unit_fields`: the body shared by boards and modules.
#[allow(clippy::too_many_arguments)]
fn unit_fields(
    library: &IndexMap<String, PartDefinition>,
    components: &[ComponentInstance],
    module_instances: &[ModuleInstance],
    nets: &[Net],
    supplies: &[Supply],
    interfaces: &[Interface],
    constraints: &[Constraint],
    devices: &IndexMap<String, DeviceDefinition>,
    selections: &[PeripheralSelection],
    power_states: &[PowerState],
) -> Vec<(&'static str, Value)> {
    let mut sorted_devices: Vec<&DeviceDefinition> = devices.values().collect();
    sorted_devices.sort_by(|a, b| a.name.cmp(&b.name));
    let mut sorted_parts: Vec<&PartDefinition> = library.values().collect();
    sorted_parts.sort_by(|a, b| a.name.cmp(&b.name));
    vec![
        ("devices", Value::Array(sorted_devices.into_iter().map(device_value).collect())),
        ("parts", Value::Array(sorted_parts.into_iter().map(part_value).collect())),
        (
            "components",
            Value::Array(
                components
                    .iter()
                    .map(|component| {
                        object(vec![
                            ("ref", text(&component.reference)),
                            ("part", text(&component.part)),
                            (
                                "value",
                                match &component.value {
                                    None => Value::Null,
                                    Some(ComponentValue::Quantity(quantity)) => quantity_value(quantity),
                                    Some(ComponentValue::Text(value)) => text(value),
                                },
                            ),
                            ("footprint", optional(&component.footprint)),
                            ("properties", string_map(&component.properties)),
                            ("modes", string_map(&component.modes)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "module_instances",
            Value::Array(
                module_instances
                    .iter()
                    .map(|instance| {
                        object(vec![("ref", text(&instance.reference)), ("module", text(&instance.module))])
                    })
                    .collect(),
            ),
        ),
        (
            "nets",
            Value::Array(
                nets.iter()
                    .map(|net| {
                        let endpoints = net.endpoints.iter().map(|endpoint| text(&endpoint.to_string())).collect();
                        object(vec![("name", text(&net.name)), ("endpoints", Value::Array(endpoints))])
                    })
                    .collect(),
            ),
        ),
        (
            "supplies",
            Value::Array(
                supplies
                    .iter()
                    .map(|supply| {
                        object(vec![
                            ("name", text(&supply.name)),
                            ("voltage", quantity_value(&supply.voltage)),
                            ("net", text(&supply.net)),
                            (
                                "source",
                                supply.source.as_ref().map(|source| text(&source.to_string())).unwrap_or(Value::Null),
                            ),
                            ("externally_driven", Value::Bool(supply.externally_driven)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "interfaces",
            Value::Array(
                interfaces
                    .iter()
                    .map(|interface| {
                        let bindings = interface
                            .bindings
                            .iter()
                            .map(|(component, pins)| (component.clone(), string_map(pins)))
                            .collect();
                        object(vec![
                            ("name", text(&interface.name)),
                            ("type", text(&interface.type_name)),
                            ("signals", string_map(&interface.signals)),
                            ("bindings", Value::Object(bindings)),
                            ("pullup_supply", optional(&interface.pullup_supply)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "constraints",
            Value::Array(
                constraints
                    .iter()
                    .map(|constraint| {
                        let parameters = constraint
                            .parameters
                            .iter()
                            .map(|(name, value)| (name.clone(), parameter_value(value)))
                            .collect();
                        object(vec![
                            ("kind", text(constraint.kind.value())),
                            ("targets", texts(&constraint.targets)),
                            ("parameters", Value::Object(parameters)),
                            ("id", optional(&constraint.constraint_id)),
                            ("mode", text(constraint.mode.value())),
                            ("consumers", texts(&constraint.consumers)),
                            ("verifier", optional(&constraint.verifier)),
                            ("origins", texts(&constraint.origins)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "peripheral_selections",
            Value::Array(
                selections
                    .iter()
                    .map(|selection| {
                        let signals = selection
                            .signals
                            .iter()
                            .map(|(name, signal)| {
                                (
                                    name.clone(),
                                    object(vec![
                                        ("pin", text(&signal.pin)),
                                        ("pad", optional(&signal.pad)),
                                        ("selector", optional(&signal.selector)),
                                        ("resource", optional(&signal.resource)),
                                        ("setting", optional(&signal.setting)),
                                    ]),
                                )
                            })
                            .collect();
                        object(vec![
                            ("name", text(&selection.name)),
                            ("component", text(&selection.component)),
                            ("peripheral", text(&selection.peripheral)),
                            ("usage", text(selection.usage.value())),
                            ("signals", Value::Object(signals)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "power_states",
            Value::Array(
                power_states
                    .iter()
                    .map(|state| {
                        let rails =
                            state.rails.iter().map(|(name, value)| (name.clone(), text(value.value()))).collect();
                        object(vec![("name", text(&state.name)), ("rails", Value::Object(rails))])
                    })
                    .collect(),
            ),
        ),
    ]
}

/// `macro_binding_document`.
pub fn macro_binding_document(binding: &MacroBinding) -> Value {
    let origin = |location: &crate::syntax::Location| {
        let managed =
            ["github.com/", "gitlab.com/", "https://"].iter().any(|prefix| binding.asset.asset.starts_with(prefix));
        let path = if !binding.asset.symbol.is_empty()
            && location.filename == binding.asset.location.filename
            && managed
        {
            binding.asset.asset.clone()
        } else {
            paths::as_posix(&paths::relpath(Path::new(&*location.filename), Path::new(&binding.asset.base_directory)))
        };
        format!("{path}:{}:{}", location.line, location.column)
    };
    let mut entries = vec![("name", text(&binding.name)), ("asset", text(&binding.asset.asset))];
    if !binding.asset.symbol.is_empty() {
        entries.push(("symbol", text(&binding.asset.symbol)));
    }
    entries.extend([
        ("base_directory", text(".")),
        ("scope", text(&binding.scope)),
        ("origin", text(&origin(&binding.location))),
        ("asset_origin", text(&origin(&binding.asset.location))),
        ("asset_validation", text("pending_physical_preparation")),
        (
            "mappings",
            Value::Array(
                binding
                    .mappings
                    .iter()
                    .map(|mapping| {
                        object(vec![
                            ("kind", text(&mapping.kind)),
                            ("key", text(&mapping.key)),
                            ("target", text(&mapping.target)),
                            ("origin", text(&origin(&mapping.location))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]);
    object(entries)
}

fn macro_fields(bindings: &[MacroBinding]) -> Vec<(&'static str, Value)> {
    if bindings.is_empty() {
        Vec::new()
    } else {
        vec![("physical_macros", Value::Array(bindings.iter().map(macro_binding_document).collect()))]
    }
}

fn no_connect_fields(items: &[NoConnect]) -> Vec<(&'static str, Value)> {
    if items.is_empty() {
        return Vec::new();
    }
    let entries = items
        .iter()
        .map(|item| object(vec![("endpoint", text(&item.endpoint.to_string())), ("reason", optional(&item.reason))]))
        .collect();
    vec![("no_connects", Value::Array(entries))]
}

fn module_value(definition: &ModuleDefinition) -> Value {
    let mut entries = vec![("kind", text("module")), ("name", text(&definition.name))];
    entries.extend(macro_fields(&definition.physical_macros));
    entries.extend(no_connect_fields(&definition.no_connects));
    entries.push((
        "ports",
        Value::Object(definition.ports.iter().map(|(name, kind)| (name.clone(), text(kind.value()))).collect()),
    ));
    entries.extend(unit_fields(
        &definition.library,
        &definition.components,
        &definition.module_instances,
        &definition.nets,
        &definition.supplies,
        &definition.interfaces,
        &definition.constraints,
        &definition.devices,
        &definition.peripheral_selections,
        &[],
    ));
    object(entries)
}

/// `board_to_dict(board)` for the electrical part.
pub fn board_value(board: &Board) -> Object {
    let mut entries =
        vec![("schema", text("copperscript-ir/v0.1")), ("kind", text("board")), ("name", text(&board.name))];
    entries.extend(macro_fields(&board.physical_macros));
    entries.extend(no_connect_fields(&board.no_connects));
    entries.extend(unit_fields(
        &board.library,
        &board.components,
        &board.module_instances,
        &board.nets,
        &board.supplies,
        &board.interfaces,
        &board.constraints,
        &board.devices,
        &board.peripheral_selections,
        &board.power_states,
    ));
    let mut modules: Vec<&ModuleDefinition> = board.module_definitions.values().collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    entries.push(("module_definitions", Value::Array(modules.into_iter().map(module_value).collect())));
    let mut dependencies: Vec<&Dependency> = board.dependencies.iter().collect();
    dependencies.sort_by(|a, b| a.import_path.cmp(&b.import_path));
    entries.push((
        "dependencies",
        Value::Array(
            dependencies
                .into_iter()
                .map(|dependency| {
                    object(vec![
                        ("import_path", text(&dependency.import_path)),
                        ("module_path", text(&dependency.module_path)),
                        ("version", text(&dependency.version)),
                        ("checksum", text(&dependency.checksum)),
                    ])
                })
                .collect(),
        ),
    ));
    entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

/// `board_to_dict(design)`.
pub fn design_value(design: &Design) -> Value {
    let mut result = board_value(&design.electrical);
    result.insert("silkscreen".to_owned(), crate::silkscreen::design_document(&design.silkscreen));
    if let Some(mechanical) = &design.mechanical {
        result.insert("mechanical".to_owned(), crate::mechanical::mechanical_document(mechanical));
    }
    Value::Object(result)
}

/// `board_to_json(design)`.
pub fn design_json(design: &Design) -> String {
    format!("{}\n", Dumps::new().indent(2).sort_keys(true).dumps(&design_value(design)))
}

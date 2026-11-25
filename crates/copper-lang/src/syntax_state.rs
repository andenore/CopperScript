//! The parsed syntax tree as Python's `state_codec` sees it: dataclasses
//! with their `pcbir` class and field names. The parser's golden test compares it;
//! `copper library` derives its source-declaration records from it.

use copper_state::StateValue;
use indexmap::IndexMap;

use crate::syntax::*;

fn class(module: &str, name: &str, fields: Vec<(&str, StateValue)>) -> StateValue {
    StateValue::Dataclass {
        name: format!("pcbir.{module}:{name}"),
        fields: fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
    }
}

fn syntax(name: &str, fields: Vec<(&str, StateValue)>) -> StateValue {
    class("syntax", name, fields)
}

fn text(value: &str) -> StateValue {
    StateValue::Str(value.to_owned())
}

fn texts(values: &[String]) -> StateValue {
    StateValue::Tuple(values.iter().map(|value| text(value)).collect())
}

fn int(value: usize) -> StateValue {
    StateValue::Int(value.into())
}

fn location(location: &Location) -> StateValue {
    syntax(
        "SourceLocation",
        vec![
            ("filename", text(&location.filename)),
            ("offset", int(location.offset)),
            ("line", int(location.line)),
            ("column", int(location.column)),
        ],
    )
}

fn locations(values: &[Location]) -> StateValue {
    StateValue::Tuple(values.iter().map(location).collect())
}

fn scalar(value: &Scalar) -> StateValue {
    match value {
        Scalar::Str(value) => text(value),
        Scalar::Bool(flag) => StateValue::Bool(*flag),
        Scalar::Int(number) => StateValue::Int(number.clone()),
        Scalar::Float(number) => StateValue::Float(*number),
        Scalar::Quantity { value, unit } => syntax("RawQuantity", vec![("value", text(value)), ("unit", text(unit))]),
    }
}

fn map<V>(values: &IndexMap<String, V>, item: impl Fn(&V) -> StateValue) -> StateValue {
    StateValue::Map { entries: values.iter().map(|(key, value)| (text(key), item(value))).collect(), proxy: false }
}

fn pad_reference(reference: &MacroPadReference) -> StateValue {
    syntax("MacroPadReference", vec![("member", text(&reference.member)), ("pad", text(&reference.pad))])
}

fn point(x: &Scalar, y: &Scalar) -> StateValue {
    StateValue::Tuple(vec![scalar(x), scalar(y)])
}

fn value(value: &Value) -> StateValue {
    match value {
        Value::Scalar(item) => scalar(item),
        Value::Point(x, y) => point(x, y),
        Value::Points(points) => StateValue::Tuple(points.iter().map(|(x, y)| point(x, y)).collect()),
        Value::Map(values) => map(values, self::value),
        Value::List(values) => StateValue::Tuple(values.iter().map(self::value).collect()),
        Value::PadRef(reference) => pad_reference(reference),
    }
}

fn named(name: &str, block: &NamedBlock) -> StateValue {
    syntax(
        name,
        vec![
            ("location", location(&block.location)),
            ("name", text(&block.name)),
            ("attributes", map(&block.attributes, scalar)),
        ],
    )
}

fn macro_item(item: &MacroItemDecl) -> StateValue {
    syntax(
        "MacroItemDecl",
        vec![
            ("location", location(&item.location)),
            ("kind", text(&item.kind)),
            ("name", text(&item.name)),
            ("parameters", map(&item.parameters, value)),
            ("children", StateValue::Tuple(item.children.iter().map(macro_item).collect())),
        ],
    )
}

fn mechanical_item(item: &MechanicalItem) -> StateValue {
    match item {
        MechanicalItem::Feature(item) => syntax(
            "MechanicalItemDecl",
            vec![
                ("location", location(&item.location)),
                ("kind", text(&item.kind)),
                ("name", text(&item.name)),
                ("shape", text(&item.shape)),
                ("parameters", map(&item.parameters, value)),
            ],
        ),
        MechanicalItem::ProfileUse(item) => syntax(
            "MechanicalProfileUseDecl",
            vec![
                ("location", location(&item.location)),
                ("profile", text(&item.profile)),
                ("instance", text(&item.instance)),
                ("bindings", map(&item.bindings, scalar)),
            ],
        ),
        MechanicalItem::Stackup(stackup) => class(
            "mechanical_stackup",
            "StackupDecl",
            vec![
                ("location", location(&stackup.location)),
                (
                    "layers",
                    StateValue::Tuple(
                        stackup
                            .layers
                            .iter()
                            .map(|layer| {
                                class(
                                    "mechanical_stackup",
                                    "StackupLayerDecl",
                                    vec![
                                        ("location", location(&layer.location)),
                                        ("kind", text(&layer.kind)),
                                        ("name", text(&layer.name)),
                                        ("parameters", map(&layer.parameters, scalar)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
    }
}

fn names(values: &IndexMap<String, String>) -> StateValue {
    map(values, |name| text(name))
}

/// The state encoding of one declaration.
pub fn declaration(declaration: &Declaration) -> StateValue {
    match declaration {
        Declaration::Component(item) => syntax(
            "ComponentDecl",
            vec![
                ("location", location(&item.location)),
                ("ref", text(&item.reference)),
                ("part", text(&item.part)),
                ("attributes", map(&item.attributes, scalar)),
            ],
        ),
        Declaration::Port(item) => syntax(
            "PortDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("pin_type", text(&item.pin_type)),
            ],
        ),
        Declaration::Pin(block) => named("PinDecl", block),
        Declaration::Pad(block) => named("PadDecl", block),
        Declaration::PowerDomain(block) => named("PowerDomainDecl", block),
        Declaration::ModeGroup(block) => named("ModeGroupDecl", block),
        Declaration::PadSet(block) => named("PadSetDecl", block),
        Declaration::Supply(block) => named("SupplyDecl", block),
        Declaration::PowerState(item) => syntax(
            "PowerStateDecl",
            vec![("location", location(&item.location)), ("name", text(&item.name)), ("rails", names(&item.rails))],
        ),
        Declaration::PartProperty(item) | Declaration::DeviceProperty(item) => syntax(
            if matches!(declaration, Declaration::PartProperty(_)) { "PartPropertyDecl" } else { "DevicePropertyDecl" },
            vec![("location", location(&item.location)), ("name", text(&item.name)), ("value", scalar(&item.value))],
        ),
        Declaration::Peripheral(item) => syntax(
            "PeripheralDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("kind", text(&item.kind)),
                (
                    "signals",
                    StateValue::Tuple(
                        item.signals
                            .iter()
                            .map(|signal| {
                                syntax(
                                    "PeripheralSignalDecl",
                                    vec![
                                        ("location", location(&signal.location)),
                                        ("name", text(&signal.name)),
                                        ("pin_type", text(&signal.pin_type)),
                                        ("attributes", map(&signal.attributes, scalar)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        Declaration::Mux(item) => syntax(
            "MuxDecl",
            vec![
                ("location", location(&item.location)),
                ("pad", text(&item.pad)),
                ("peripheral", text(&item.peripheral)),
                ("signal", text(&item.signal)),
                ("attributes", map(&item.attributes, scalar)),
            ],
        ),
        Declaration::Resource(item) => {
            syntax("ResourceDecl", vec![("location", location(&item.location)), ("name", text(&item.name))])
        }
        Declaration::Unit(item) | Declaration::SignalGroup(item) => {
            let (class_name, field) = if matches!(declaration, Declaration::Unit(_)) {
                ("UnitDecl", "terminals")
            } else {
                ("SignalGroupDecl", "attributes")
            };
            syntax(
                class_name,
                vec![
                    ("location", location(&item.location)),
                    ("name", text(&item.name)),
                    ("kind", text(&item.kind)),
                    (field, map(&item.attributes, scalar)),
                ],
            )
        }
        Declaration::RouteRule(item) => syntax(
            "RouteRuleDecl",
            vec![
                ("location", location(&item.location)),
                ("peripheral", text(&item.peripheral)),
                ("signal", text(&item.signal)),
                ("attributes", map(&item.attributes, scalar)),
            ],
        ),
        Declaration::Configuration(item) => syntax(
            "ConfigurationDecl",
            vec![
                ("location", location(&item.location)),
                ("component", text(&item.component)),
                ("peripheral", text(&item.peripheral)),
                ("name", text(&item.name)),
                ("signals", names(&item.signals)),
            ],
        ),
        Declaration::ModuleInstance(item) => syntax(
            "ModuleInstanceDecl",
            vec![
                ("location", location(&item.location)),
                ("ref", text(&item.reference)),
                ("module", text(&item.module)),
            ],
        ),
        Declaration::PhysicalMacro(item) => syntax(
            "PhysicalMacroDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("asset_alias", text(&item.asset_alias)),
                ("scope", text(&item.scope)),
                (
                    "mappings",
                    StateValue::Tuple(
                        item.mappings
                            .iter()
                            .map(|mapping| {
                                syntax(
                                    "MacroMappingDecl",
                                    vec![
                                        ("location", location(&mapping.location)),
                                        ("kind", text(&mapping.kind)),
                                        ("key", text(&mapping.key)),
                                        ("target", text(&mapping.target)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        Declaration::MacroProperty(item) => syntax(
            "MacroPropertyDecl",
            vec![("location", location(&item.location)), ("name", text(&item.name)), ("value", value(&item.value))],
        ),
        Declaration::MacroItem(item) => macro_item(item),
        Declaration::MacroPad(item) => syntax(
            "MacroPadDecl",
            vec![
                ("location", location(&item.location)),
                ("reference", pad_reference(&item.reference)),
                ("net", item.net.as_deref().map(text).unwrap_or(StateValue::Null)),
            ],
        ),
        Declaration::Net(item) => syntax(
            "NetDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("endpoints", texts(&item.endpoints)),
                ("endpoint_locations", locations(&item.endpoint_locations)),
            ],
        ),
        Declaration::NoConnect(item) => syntax(
            "NoConnectDecl",
            vec![
                ("location", location(&item.location)),
                ("endpoints", texts(&item.endpoints)),
                ("attributes", map(&item.attributes, scalar)),
                ("endpoint_locations", locations(&item.endpoint_locations)),
            ],
        ),
        Declaration::Interface(item) => syntax(
            "InterfaceDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("kind", text(&item.kind)),
                ("attributes", map(&item.attributes, scalar)),
                (
                    "bindings",
                    StateValue::Tuple(
                        item.bindings
                            .iter()
                            .map(|binding| {
                                syntax(
                                    "BindingDecl",
                                    vec![
                                        ("location", location(&binding.location)),
                                        ("component", text(&binding.component)),
                                        ("signals", names(&binding.signals)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        Declaration::Constraint(item) => syntax(
            "ConstraintDecl",
            vec![
                ("location", location(&item.location)),
                ("kind", text(&item.kind)),
                ("targets", texts(&item.targets)),
                ("parameters", map(&item.parameters, scalar)),
            ],
        ),
        Declaration::Mechanical(item) => syntax(
            "MechanicalDecl",
            vec![
                ("location", location(&item.location)),
                ("items", StateValue::Tuple(item.items.iter().map(mechanical_item).collect())),
            ],
        ),
        Declaration::Silkscreen(item) => syntax(
            "SilkscreenDecl",
            vec![
                ("location", location(&item.location)),
                (
                    "items",
                    StateValue::Tuple(
                        item.items
                            .iter()
                            .map(|entry| {
                                syntax(
                                    "SilkscreenItemDecl",
                                    vec![
                                        ("location", location(&entry.location)),
                                        ("kind", text(&entry.kind)),
                                        ("name", text(&entry.name)),
                                        ("parameters", map(&entry.parameters, value)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        Declaration::Build(item) => syntax(
            "BuildDecl",
            vec![
                ("location", location(&item.location)),
                (
                    "settings",
                    StateValue::Tuple(
                        item.settings
                            .iter()
                            .map(|entry| {
                                syntax(
                                    "BuildSettingDecl",
                                    vec![
                                        ("location", location(&entry.location)),
                                        ("group", text(&entry.group)),
                                        ("key", text(&entry.key)),
                                        ("value", scalar(&entry.value)),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        ),
        Declaration::MechanicalItem(item) => mechanical_item(item),
        Declaration::Param(item) => {
            let mut fields = vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("value", scalar(&item.value)),
            ];
            if let Some(derived) = &item.expression {
                fields.push(("expression", expression(derived)));
            }
            syntax("ParamDecl", fields)
        }
        Declaration::Sim(item) => syntax(
            "SimDecl",
            vec![
                ("location", location(&item.location)),
                ("kind", text(&item.kind)),
                ("unit", item.unit.as_deref().map(text).unwrap_or(StateValue::Null)),
                ("entries", sim_entries(&item.entries)),
            ],
        ),
        Declaration::Bench(statement) => bench_statement(statement),
    }
}

fn sim_entries(entries: &[SimEntry]) -> StateValue {
    StateValue::Tuple(
        entries
            .iter()
            .map(|entry| {
                let value = match &entry.value {
                    SimValue::Scalar(item) => scalar(item),
                    SimValue::List(items) => StateValue::Tuple(items.iter().map(scalar).collect()),
                    SimValue::Block { kind, entries } => {
                        syntax("SimBlock", vec![("kind", text(kind)), ("entries", sim_entries(entries))])
                    }
                };
                syntax(
                    "SimEntry",
                    vec![("location", location(&entry.location)), ("key", text(&entry.key)), ("value", value)],
                )
            })
            .collect(),
    )
}

fn expression(item: &Expr) -> StateValue {
    let (name, mut fields) = match &item.kind {
        ExprKind::Number(value) => ("NumberExpr", vec![("value", text(value))]),
        ExprKind::Quantity { value, unit } => ("QuantityExpr", vec![("value", text(value)), ("unit", text(unit))]),
        ExprKind::Name(name) => ("NameExpr", vec![("name", text(name))]),
        ExprKind::Probe { function, targets, target_locations } => (
            "ProbeExpr",
            vec![
                ("function", text(function)),
                ("targets", texts(targets)),
                ("target_locations", locations(target_locations)),
            ],
        ),
        ExprKind::Call { function, arguments } => (
            "CallExpr",
            vec![
                ("function", text(function)),
                ("arguments", StateValue::Tuple(arguments.iter().map(expression).collect())),
            ],
        ),
        ExprKind::Negate(operand) => ("NegateExpr", vec![("operand", expression(operand))]),
        ExprKind::Binary { operator, left, right } => (
            "BinaryExpr",
            vec![("operator", text(operator.symbol())), ("left", expression(left)), ("right", expression(right))],
        ),
        ExprKind::Compare { operator, left, right } => (
            "CompareExpr",
            vec![("operator", text(operator.symbol())), ("left", expression(left)), ("right", expression(right))],
        ),
    };
    fields.insert(0, ("location", location(&item.location)));
    syntax(name, fields)
}

fn bench_statement(statement: &BenchStatement) -> StateValue {
    match statement {
        BenchStatement::Target(item) => {
            let mut fields = vec![("location", location(&item.location)), ("board", text(&item.board))];
            if !item.fixtures.is_empty() {
                fields.push(("fixtures", texts(&item.fixtures)));
                fields.push(("fixture_locations", locations(&item.fixture_locations)));
            }
            syntax("BenchTargetDecl", fields)
        }
        BenchStatement::UseLayout(item) => syntax("UseLayoutDecl", vec![("location", location(&item.location))]),
        BenchStatement::InitialCondition(item) => {
            let target = match &item.target {
                InitialConditionTarget::Node(node) => syntax("NodeTarget", vec![("node", text(node))]),
                InitialConditionTarget::Component(reference) => {
                    syntax("ComponentTarget", vec![("component", text(reference))])
                }
            };
            syntax(
                "InitialConditionDecl",
                vec![
                    ("location", location(&item.location)),
                    ("target", target),
                    ("target_location", location(&item.target_location)),
                    ("value", scalar(&item.value)),
                ],
            )
        }
        BenchStatement::Replace(item) => syntax(
            "ReplaceDecl",
            vec![
                ("location", location(&item.location)),
                ("target", text(&item.target)),
                ("kind", text(&item.kind)),
                ("entries", sim_entries(&item.entries)),
            ],
        ),
        BenchStatement::Model(item) => syntax(
            "ModelDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("kind", text(&item.kind)),
                ("entries", sim_entries(&item.entries)),
            ],
        ),
        BenchStatement::Ignore(item) => syntax(
            "IgnoreDecl",
            vec![
                ("location", location(&item.location)),
                ("targets", texts(&item.targets)),
                ("target_locations", locations(&item.target_locations)),
            ],
        ),
        BenchStatement::Analysis(item) => {
            let mut fields = vec![("location", location(&item.location))];
            if let Some(name) = &item.name {
                fields.push(("name", text(name)));
            }
            fields.push(("kind", text(&item.kind)));
            fields.push(("entries", sim_entries(&item.entries)));
            syntax("AnalysisDecl", fields)
        }
        BenchStatement::Step(item) => {
            let mut fields = vec![
                ("location", location(&item.location)),
                ("param", text(&item.param)),
                ("values", StateValue::Tuple(item.values.iter().map(scalar).collect())),
            ];
            if !item.zip.is_empty() {
                let zip = item
                    .zip
                    .iter()
                    .map(|(param, values)| {
                        StateValue::Tuple(vec![text(param), StateValue::Tuple(values.iter().map(scalar).collect())])
                    })
                    .collect();
                fields.push(("zip", StateValue::Tuple(zip)));
            }
            if let Some(range) = &item.range {
                fields.push(("from", scalar(&range.from)));
                fields.push(("to", scalar(&range.to)));
                if let Some(by) = &range.by {
                    fields.push(("by", scalar(by)));
                }
                if let Some(points) = &range.points {
                    fields.push(("points", scalar(points)));
                }
            }
            syntax("StepDecl", fields)
        }
        BenchStatement::Reference(item) => syntax(
            "ReferenceDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("entries", sim_entries(&item.entries)),
            ],
        ),
        BenchStatement::Let(item) => syntax(
            "LetDecl",
            vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("expression", expression(&item.expression)),
            ],
        ),
        BenchStatement::Measure(item) => {
            let mut fields = vec![
                ("location", location(&item.location)),
                ("name", text(&item.name)),
                ("expression", expression(&item.expression)),
                ("limits", sim_entries(&item.limits)),
            ];
            if !item.conditions.is_empty() {
                fields.push(("conditions", StateValue::Tuple(item.conditions.iter().map(expression).collect())));
            }
            syntax("MeasureDecl", fields)
        }
        BenchStatement::Plot(item) => syntax(
            "PlotDecl",
            vec![
                ("location", location(&item.location)),
                ("expressions", StateValue::Tuple(item.expressions.iter().map(expression).collect())),
            ],
        ),
        BenchStatement::Variation(item) => {
            let mut fields = vec![("location", location(&item.location)), ("kind", text(&item.kind))];
            if let Some(target) = &item.target {
                fields.push(("target", text(target)));
            }
            if !item.others.is_empty() {
                fields.push(("others", texts(&item.others)));
            }
            if let Some(value) = &item.value {
                fields.push(("value", scalar(value)));
            }
            if let Some(value) = &item.match_value {
                fields.push(("match", scalar(value)));
            }
            fields.push(("entries", sim_entries(&item.entries)));
            syntax("VariationDecl", fields)
        }
    }
}

/// Python's `state_codec` value of a parsed document.
pub fn document(document: &Document) -> StateValue {
    syntax(
        "Document",
        vec![
            ("location", location(&document.location)),
            ("kind", text(document.kind.as_str())),
            ("name", text(&document.name)),
            ("libraries", texts(&document.libraries)),
            (
                "imports",
                StateValue::Tuple(
                    document
                        .imports
                        .iter()
                        .map(|import| {
                            syntax(
                                "ImportDecl",
                                vec![
                                    ("location", location(&import.location)),
                                    ("alias", text(&import.alias)),
                                    ("path", text(&import.path)),
                                ],
                            )
                        })
                        .collect(),
                ),
            ),
            ("declarations", StateValue::Tuple(document.declarations.iter().map(declaration).collect())),
        ],
    )
}

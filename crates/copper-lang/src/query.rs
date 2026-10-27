//! Compact, read-only questions about a compiled design (`pcbir.design_query`).
//!
//! Queries run on the flat electrical view, so hierarchical names use
//! `INSTANCE/REF`. They report declared intent and connectivity only.

use indexmap::{IndexMap, IndexSet};

use copper_json::{Object, Value, python_isspace, python_str_repr as repr};

use crate::erc::{Context, build_context, is_power_input, no_connect_endpoints, unmarked_pins};
use crate::model::{
    ComponentInstance, ComponentValue, ConnectionPolicy, ElectricalProfile, Endpoint, FlatElectricalView,
    PackagePinDefinition, PartDefinition,
};
use crate::suggest::close_match;

pub const QUERY_SCHEMA: &str = "copperscript.query/v1";
pub const QUERY_KINDS: [&str; 6] = ["components", "component", "pins", "nets", "net", "unconnected"];

type Result<T> = std::result::Result<T, String>;

/// Per-query derived indexes (`_facts`).
struct Facts {
    open_pins: IndexMap<Endpoint, Option<String>>,
    unmarked: IndexMap<String, Vec<String>>,
    decoupling: IndexMap<Endpoint, Vec<String>>,
    functions: IndexMap<Endpoint, Vec<String>>,
}

fn facts(context: &Context) -> Facts {
    let mut decoupling: IndexMap<Endpoint, Vec<String>> = IndexMap::new();
    for component in &context.board.components {
        let Some(target) = component.properties.get("decouples").filter(|target| !target.is_empty()) else {
            continue;
        };
        let Ok(endpoint) = Endpoint::parse(target) else { continue };
        let key = context.canonical_endpoint(&endpoint).unwrap_or(endpoint);
        decoupling.entry(key).or_default().push(component.reference.clone());
    }
    let mut functions: IndexMap<Endpoint, Vec<String>> = IndexMap::new();
    for selection in &context.board.peripheral_selections {
        for (signal, chosen) in &selection.signals {
            let endpoint = Endpoint::new(&selection.component, &chosen.pin);
            let key = context.canonical_endpoint(&endpoint).unwrap_or(endpoint);
            functions.entry(key).or_default().push(format!("{}.{signal}", selection.peripheral));
        }
    }
    Facts { open_pins: no_connect_endpoints(context), unmarked: unmarked_pins(context), decoupling, functions }
}

/// `run_query` on the flattened design.
pub fn run_query(board: FlatElectricalView, kind: &str, name: Option<&str>) -> Result<Value> {
    let context = build_context(board);
    let name = name.unwrap_or_default();
    if ["component", "pins", "net"].contains(&kind) && name.is_empty() {
        let what = if kind == "net" { "net name" } else { "component reference" };
        return Err(format!("`query {kind}` needs a {what}"));
    }
    let facts = facts(&context);
    let body: (&str, Value) = match kind {
        "components" => {
            let rows = refs(&context).iter().map(|r| component_row(&context, &facts, r)).collect::<Result<_>>()?;
            ("components", Value::Array(rows))
        }
        "component" => ("component", component_detail(&context, &facts, &component(&context, name)?)?),
        "pins" => {
            let reference = component(&context, name)?;
            let mut result = object([("schema", Value::from(QUERY_SCHEMA))]);
            result.insert("board".to_owned(), Value::from(context.board.name.as_str()));
            result.insert("query".to_owned(), Value::from(kind));
            result.insert("component".to_owned(), Value::from(reference.as_str()));
            result.insert("pins".to_owned(), Value::Array(pin_rows(&context, &facts, &reference)?));
            return Ok(Value::Object(result));
        }
        "nets" => ("nets", Value::Array(context.board.nets.iter().map(|net| net_row(&context, &net.name)).collect())),
        "net" => ("net", net_detail(&context, &facts, &net(&context, name)?)),
        "unconnected" => ("components", Value::Array(unconnected(&context, &facts)?)),
        _ => return Err(format!("unknown query {}; expected one of: {}", repr(kind), QUERY_KINDS.join(", "))),
    };
    let mut result = object([("schema", Value::from(QUERY_SCHEMA))]);
    result.insert("board".to_owned(), Value::from(context.board.name.as_str()));
    result.insert("query".to_owned(), Value::from(kind));
    result.insert(body.0.to_owned(), body.1);
    Ok(Value::Object(result))
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Object {
    entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

fn refs(context: &Context) -> Vec<String> {
    let unique: IndexSet<&str> = context.board.components.iter().map(|c| c.reference.as_str()).collect();
    unique.into_iter().map(str::to_owned).collect()
}

fn not_found<'a>(label: &str, name: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    let mut sorted: Vec<&str> = candidates.collect();
    sorted.sort_unstable();
    let hint = match close_match(name, &sorted) {
        Some(found) => format!("; did you mean {}?", repr(&found)),
        None => format!("; `copper query BOARD {label}s` lists them"),
    };
    format!("no {label} {}{hint}", repr(name))
}

fn component(context: &Context, reference: &str) -> Result<String> {
    if context.component(reference).is_some() {
        return Ok(reference.to_owned());
    }
    Err(not_found("component", reference, context.component_names()))
}

fn net(context: &Context, name: &str) -> Result<String> {
    let resolved = context.board.net_aliases.get(name).map_or(name, String::as_str);
    if context.net(resolved).is_some() {
        return Ok(resolved.to_owned());
    }
    Err(not_found("net", name, context.net_names()))
}

fn profile_text(profile: Option<&ElectricalProfile>) -> Value {
    let Some(profile) = profile else { return Value::Null };
    let mut domains: Vec<&str> = profile.domains.iter().map(|item| item.value()).collect();
    domains.sort_unstable();
    let mut directions: Vec<&str> = profile.directions.iter().map(|item| item.value()).collect();
    directions.sort_unstable();
    let parts: Vec<String> =
        [domains.join("/"), directions.join("/")].into_iter().filter(|part| !part.is_empty()).collect();
    if parts.is_empty() { Value::Null } else { Value::from(parts.join(" ").as_str()) }
}

fn value_text(value: Option<&ComponentValue>) -> Value {
    match value {
        None => Value::Null,
        Some(ComponentValue::Quantity(quantity)) => Value::from(quantity.to_string().as_str()),
        Some(ComponentValue::Text(text)) => Value::from(text.as_str()),
    }
}

fn open_status(
    context: &Context,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &PackagePinDefinition,
) -> &'static str {
    match pin.connection_policy {
        ConnectionPolicy::Optional => return "optional",
        ConnectionPolicy::Required => return "required_unconnected",
        _ => {}
    }
    let profile = context.pin_profile(component, part, pin);
    match &profile {
        None => "unmodeled",
        Some(_) if is_power_input(profile.as_ref()) => "power_input_unconnected",
        Some(_) => "open",
    }
}

fn pin_rows(context: &Context, facts: &Facts, reference: &str) -> Result<Vec<Value>> {
    let component = context.component(reference).expect("known component");
    let Some(part) = context.board.library.get(&component.part) else {
        return Err(format!("component {reference} uses unknown part {}; run `copper check`", repr(&component.part)));
    };
    let unmarked: Vec<&String> = facts.unmarked.get(reference).map(|pins| pins.iter().collect()).unwrap_or_default();
    let mut rows = Vec::new();
    for pin in part.pins.values() {
        let endpoint = Endpoint::new(reference, &pin.name);
        let nets = context.pin_to_nets.get(&endpoint).cloned().unwrap_or_default();
        let status = if !nets.is_empty() {
            "connected"
        } else if facts.open_pins.contains_key(&endpoint) {
            "no_connect"
        } else if pin.connection_policy == ConnectionPolicy::DoNotConnect {
            "part_do_not_connect"
        } else if unmarked.contains(&&pin.name) {
            "unmarked"
        } else {
            open_status(context, component, part, pin)
        };
        let mut row = object([
            ("pin", Value::from(pin.name.as_str())),
            ("number", Value::from(pin.number.as_str())),
            ("nets", texts(&nets)),
            ("status", Value::from(status)),
            ("type", profile_text(context.pin_profile(component, part, pin).as_ref())),
        ]);
        if pin.connection_policy != ConnectionPolicy::Normal {
            row.insert("policy".to_owned(), Value::from(pin.connection_policy.value()));
        }
        if let Some(Some(reason)) =
            facts.open_pins.get(&endpoint).filter(|reason| reason.as_deref().is_some_and(|r| !r.is_empty()))
        {
            row.insert("reason".to_owned(), Value::from(reason.as_str()));
        }
        if let Some(refs) = facts.decoupling.get(&endpoint).filter(|refs| !refs.is_empty()) {
            row.insert("decoupled_by".to_owned(), texts(refs));
        }
        if let Some(names) = facts.functions.get(&endpoint).filter(|names| !names.is_empty()) {
            row.insert("functions".to_owned(), texts(names));
        }
        rows.push(Value::Object(row));
    }
    Ok(rows)
}

fn statuses(rows: &[Value]) -> Vec<String> {
    rows.iter().map(|row| field(row, "status").to_owned()).collect()
}

fn field<'a>(row: &'a Value, key: &str) -> &'a str {
    match row {
        Value::Object(map) => match map.get(key) {
            Some(Value::Str(text)) => text,
            _ => "",
        },
        _ => "",
    }
}

fn component_row(context: &Context, facts: &Facts, reference: &str) -> Result<Value> {
    let component = context.component(reference).expect("known component");
    let mut row = object([
        ("ref", Value::from(reference)),
        ("part", Value::from(component.part.as_str())),
        ("value", value_text(component.value.as_ref())),
    ]);
    if context.board.library.contains_key(&component.part) {
        let statuses = statuses(&pin_rows(context, facts, reference)?);
        let count = |status: &str| Value::from(statuses.iter().filter(|item| *item == status).count() as i64);
        row.insert(
            "pins".to_owned(),
            Value::Object(object([
                ("total", Value::from(statuses.len() as i64)),
                ("connected", count("connected")),
                ("no_connect", count("no_connect")),
                ("unmarked", count("unmarked")),
            ])),
        );
    }
    Ok(Value::Object(row))
}

fn text_map(map: &IndexMap<String, String>) -> Value {
    Value::Object(map.iter().map(|(key, value)| (key.clone(), Value::from(value.as_str()))).collect())
}

fn component_detail(context: &Context, facts: &Facts, reference: &str) -> Result<Value> {
    let component = context.component(reference).expect("known component");
    let Value::Object(mut detail) = component_row(context, facts, reference)? else { unreachable!() };
    detail.insert("footprint".to_owned(), component.footprint.as_deref().map_or(Value::Null, Value::from));
    detail.insert("properties".to_owned(), text_map(&component.properties));
    detail.insert("modes".to_owned(), text_map(&component.modes));
    if context.board.library.contains_key(&component.part) {
        detail.insert("pin_list".to_owned(), Value::Array(pin_rows(context, facts, reference)?));
    }
    Ok(Value::Object(detail))
}

fn net_row(context: &Context, name: &str) -> Value {
    let net = context.net(name).expect("known net");
    let mut row = object([("net", Value::from(name)), ("endpoints", Value::from(net.endpoints.len() as i64))]);
    if let Some(supply) = context.board.supplies.iter().find(|item| item.net == name) {
        row.insert(
            "supply".to_owned(),
            Value::Object(object([
                ("name", Value::from(supply.name.as_str())),
                ("voltage", Value::from(supply.voltage.to_string().as_str())),
                (
                    "source",
                    supply.source.as_ref().map_or(Value::Null, |source| Value::from(source.to_string().as_str())),
                ),
                ("external", Value::Bool(supply.externally_driven)),
            ])),
        );
    }
    Value::Object(row)
}

fn net_detail(context: &Context, facts: &Facts, name: &str) -> Value {
    let net = context.net(name).expect("known net");
    let Value::Object(mut detail) = net_row(context, name) else { unreachable!() };
    let mut members = Vec::new();
    for endpoint in &net.endpoints {
        let mut member = object([("endpoint", Value::from(endpoint.to_string().as_str()))]);
        if let Some(component) = context.component(&endpoint.component) {
            member.insert("part".to_owned(), Value::from(component.part.as_str()));
        }
        match context.resolve_pin(endpoint) {
            Some((component, part, pin)) => {
                member.insert("type".to_owned(), profile_text(context.pin_profile(component, part, pin).as_ref()));
                let canonical = Endpoint::new(&endpoint.component, &pin.name);
                if let Some(refs) = facts.decoupling.get(&canonical).filter(|refs| !refs.is_empty()) {
                    member.insert("decoupled_by".to_owned(), texts(refs));
                }
            }
            None => {
                member.insert("unresolved".to_owned(), Value::Bool(true));
            }
        }
        members.push(Value::Object(member));
    }
    detail.insert("members".to_owned(), Value::Array(members));
    let mut interfaces: Vec<String> = context
        .board
        .interfaces
        .iter()
        .filter(|item| item.signals.values().any(|signal| signal == name))
        .map(|item| item.name.clone())
        .collect();
    interfaces.sort();
    if !interfaces.is_empty() {
        detail.insert("interfaces".to_owned(), texts(&interfaces));
    }
    let canonical: IndexSet<Endpoint> =
        net.endpoints.iter().map(|e| context.canonical_endpoint(e).unwrap_or_else(|| e.clone())).collect();
    let mut caps: Vec<String> = facts
        .decoupling
        .iter()
        .filter(|(target, _)| canonical.contains(*target))
        .flat_map(|(_, refs)| refs.iter().cloned())
        .collect();
    caps.sort();
    caps.dedup();
    if !caps.is_empty() {
        detail.insert("decoupling_capacitors".to_owned(), texts(&caps));
    }
    Value::Object(detail)
}

fn unconnected(context: &Context, facts: &Facts) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for reference in refs(context) {
        let component = context.component(&reference).expect("known component");
        if !context.board.library.contains_key(&component.part) {
            continue;
        }
        let mut groups: IndexMap<String, Vec<Value>> = IndexMap::new();
        for pin in pin_rows(context, facts, &reference)? {
            let status = field(&pin, "status").to_owned();
            if status == "connected" {
                continue;
            }
            let name = field(&pin, "pin");
            let reason = field(&pin, "reason");
            let entry = if reason.is_empty() {
                Value::from(name)
            } else {
                Value::Object(object([("pin", Value::from(name)), ("reason", Value::from(reason))]))
            };
            groups.entry(status).or_default().push(entry);
        }
        if !groups.is_empty() {
            let mut row = object([("ref", Value::from(reference.as_str()))]);
            row.extend(groups.into_iter().map(|(status, entries)| (status, Value::Array(entries))));
            rows.push(Value::Object(row));
        }
    }
    Ok(rows)
}

// --- text rendering ---------------------------------------------------------------

fn width(text: &str) -> usize {
    text.chars().count()
}

fn pad(text: &str, size: usize) -> String {
    format!("{text}{}", " ".repeat(size.saturating_sub(width(text))))
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..headers.len())
        .map(|i| rows.iter().map(|row| width(&row[i])).chain([width(headers[i])]).max().unwrap_or(0))
        .collect();
    let line = |cells: Vec<&str>| {
        let joined: Vec<String> = cells.iter().zip(&widths).map(|(cell, size)| pad(cell, *size)).collect();
        joined.join("  ").trim_end_matches(python_isspace).to_owned()
    };
    let mut lines = vec![line(headers.to_vec())];
    lines.extend(rows.iter().map(|row| line(row.iter().map(String::as_str).collect())));
    lines.join("\n")
}

fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key),
        _ => None,
    }
}

/// Python's `str(value)` for the JSON scalars a query holds.
fn show(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Str(text)) => text.clone(),
        Some(Value::Int(number)) => number.to_string(),
        Some(Value::Bool(flag)) => if *flag { "True" } else { "False" }.to_owned(),
        Some(other) => format!("{other:?}"),
    }
}

/// Python truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Str(text)) => !text.is_empty(),
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(items)) => !items.is_empty(),
        Some(Value::Int(number)) => *number != 0.into(),
        Some(Value::Float(number)) => *number != 0.0,
    }
}

fn joined(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(items)) => items.iter().map(|item| show(Some(item))).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

fn items(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn pin_table(pins: &[Value]) -> String {
    let rows: Vec<Vec<String>> = pins
        .iter()
        .map(|pin| {
            let mut notes = Vec::new();
            if truthy(get(pin, "reason")) {
                notes.push(format!("reason: {}", show(get(pin, "reason"))));
            }
            if truthy(get(pin, "decoupled_by")) {
                notes.push(format!("decoupled by {}", joined(get(pin, "decoupled_by"))));
            }
            if truthy(get(pin, "functions")) {
                notes.push(joined(get(pin, "functions")));
            }
            if truthy(get(pin, "policy")) {
                notes.push(format!("policy {}", show(get(pin, "policy"))));
            }
            let nets = joined(get(pin, "nets"));
            vec![
                show(get(pin, "pin")),
                show(get(pin, "number")),
                if nets.is_empty() { "-".to_owned() } else { nets },
                show(get(pin, "status")),
                if truthy(get(pin, "type")) { show(get(pin, "type")) } else { "-".to_owned() },
                notes.join("; "),
            ]
        })
        .collect();
    table(&["PIN", "NO", "NET", "STATUS", "TYPE", "NOTES"], &rows)
}

/// `render_query`: the text form of a query result.
pub fn render_query(result: &Value) -> String {
    let kind = show(get(result, "query"));
    match kind.as_str() {
        "pins" => format!("{} pins\n{}", show(get(result, "component")), pin_table(items(get(result, "pins")))),
        "component" => {
            let c = get(result, "component").expect("component");
            let value = if truthy(get(c, "value")) { format!(" = {}", show(get(c, "value"))) } else { String::new() };
            let mut head = vec![format!("{}: {}{value}", show(get(c, "ref")), show(get(c, "part")))];
            if truthy(get(c, "footprint")) {
                head.push(format!("footprint: {}", show(get(c, "footprint"))));
            }
            if let Some(Value::Object(properties)) = get(c, "properties") {
                let mut sorted: Vec<(&String, &Value)> = properties.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                head.extend(sorted.into_iter().map(|(key, value)| format!("{key}: {}", show(Some(value)))));
            }
            if get(c, "pin_list").is_some() {
                head.push(pin_table(items(get(c, "pin_list"))));
            }
            head.join("\n")
        }
        "components" => {
            let rows: Vec<Vec<String>> = items(get(result, "components"))
                .iter()
                .map(|c| {
                    let pins = get(c, "pins");
                    let pin_text = match pins {
                        Some(pins) if truthy(Some(pins)) => format!(
                            "{}/{} connected, {} nc, {} unmarked",
                            show(get(pins, "connected")),
                            show(get(pins, "total")),
                            show(get(pins, "no_connect")),
                            show(get(pins, "unmarked"))
                        ),
                        _ => "unknown part".to_owned(),
                    };
                    let value = if truthy(get(c, "value")) { show(get(c, "value")) } else { "-".to_owned() };
                    vec![show(get(c, "ref")), show(get(c, "part")), value, pin_text]
                })
                .collect();
            table(&["REF", "PART", "VALUE", "PINS"], &rows)
        }
        "nets" => {
            let rows: Vec<Vec<String>> = items(get(result, "nets"))
                .iter()
                .map(|n| {
                    let supply = match get(n, "supply") {
                        Some(supply) if truthy(Some(supply)) => {
                            let external = if truthy(get(supply, "external")) { " external" } else { "" };
                            format!("{}{external}", show(get(supply, "voltage")))
                        }
                        _ => "-".to_owned(),
                    };
                    vec![show(get(n, "net")), show(get(n, "endpoints")), supply]
                })
                .collect();
            table(&["NET", "ENDPOINTS", "SUPPLY"], &rows)
        }
        "net" => {
            let n = get(result, "net").expect("net");
            let mut head = vec![format!("net {} ({} endpoints)", show(get(n, "net")), show(get(n, "endpoints")))];
            if let Some(s) = get(n, "supply").filter(|s| truthy(Some(s))) {
                let source = if truthy(get(s, "source")) {
                    format!(", source {}", show(get(s, "source")))
                } else {
                    String::new()
                };
                let external = if truthy(get(s, "external")) { ", external" } else { "" };
                head.push(format!("supply {} = {}{source}{external}", show(get(s, "name")), show(get(s, "voltage"))));
            }
            if truthy(get(n, "interfaces")) {
                head.push(format!("interfaces: {}", joined(get(n, "interfaces"))));
            }
            let rows: Vec<Vec<String>> = items(get(n, "members"))
                .iter()
                .map(|m| {
                    let part = get(m, "part").map_or("?".to_owned(), |part| show(Some(part)));
                    let kind = if truthy(get(m, "type")) {
                        show(get(m, "type"))
                    } else if truthy(get(m, "unresolved")) {
                        "unresolved".to_owned()
                    } else {
                        "-".to_owned()
                    };
                    let notes = if truthy(get(m, "decoupled_by")) {
                        format!("decoupled by {}", joined(get(m, "decoupled_by")))
                    } else {
                        String::new()
                    };
                    vec![show(get(m, "endpoint")), part, kind, notes]
                })
                .collect();
            format!("{}\n{}", head.join("\n"), table(&["ENDPOINT", "PART", "TYPE", "NOTES"], &rows))
        }
        _ => {
            let components = items(get(result, "components"));
            if components.is_empty() {
                return "Every modeled pin is on a net.".to_owned();
            }
            let order = [
                "unmarked",
                "required_unconnected",
                "power_input_unconnected",
                "no_connect",
                "part_do_not_connect",
                "optional",
                "unmodeled",
                "open",
            ];
            components
                .iter()
                .map(|c| {
                    let parts: Vec<String> = order
                        .iter()
                        .filter_map(|status| {
                            let entries = get(c, status)?;
                            let names: Vec<String> = items(Some(entries))
                                .iter()
                                .map(|item| match item {
                                    Value::Str(name) => name.clone(),
                                    other => format!("{} ({})", show(get(other, "pin")), show(get(other, "reason"))),
                                })
                                .collect();
                            Some(format!("{status}: {}", names.join(", ")))
                        })
                        .collect();
                    format!("{}  {}", show(get(c, "ref")), parts.join(" | "))
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

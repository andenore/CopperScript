//! Offline, explicit procurement selections; never infer substitutes or stock
//! (`pcbir.assembly`).

use std::path::Path;

use copper_json::{Dumps, Object, UniqueError, Value, python_casefold, python_isspace};
use copper_lang::elaborate::elaborate;
use copper_lang::model::{Board, ComponentInstance, ComponentValue, FlatElectricalView, PartDefinition};
use copper_lang::serializer::{board_value, component_value, device_value, part_value, quantity_value};
use copper_pcb::kicad_pcb::kicad_reference;
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

use crate::csv::{self, Field};
use crate::files;

/// v0.4 binds each selection to its own component (`electrical_sha256` per
/// selection) instead of the whole compiled board, so a change elsewhere on
/// the board leaves unrelated selections and their reviews valid.
pub const SCHEMA: &str = "copperscript-assembly-lock/v0.4";
/// v0.3 added optional per-selection `alternates`.
const ALTERNATES_SCHEMA: &str = "copperscript-assembly-lock/v0.3";
/// Legacy locks bind the whole board with one digest; `assembly update`
/// rewrites them as v0.4. v0.1 locks predate `populate`.
const LEGACY_SCHEMAS: [&str; 3] =
    [ALTERNATES_SCHEMA, "copperscript-assembly-lock/v0.2", "copperscript-assembly-lock/v0.1"];
/// How a populated component reaches the board; only `supplier` components
/// are ordered and placed by the assembler.
const POPULATE_MODES: [&str; 3] = ["supplier", "consigned", "none"];
const SELECTION_FIELDS: [&str; 12] = [
    "reference",
    "part",
    "footprint",
    "manufacturer",
    "mpn",
    "supplier",
    "supplier_part",
    "source_url",
    "reviewed",
    "populate",
    "alternates",
    "electrical_sha256",
];
const OFFER_FIELDS: [&str; 3] = ["supplier", "supplier_part", "source_url"];

/// `AssemblyError`, or an `OSError` the CLI reports the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyError(pub String);

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, AssemblyError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(AssemblyError(message.into()))
}

/// Another place to buy the selected manufacturer part; never what is fitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub supplier: String,
    pub supplier_part: String,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub reference: String,
    pub part: String,
    pub footprint: Option<String>,
    pub manufacturer: Option<String>,
    pub mpn: Option<String>,
    pub supplier: Option<String>,
    pub supplier_part: Option<String>,
    pub source_url: Option<String>,
    pub reviewed: bool,
    pub populate: String,
    /// Same manufacturer part from other distributors; the assembler's BOM
    /// still uses only `supplier`/`supplier_part`.
    pub alternates: Vec<Offer>,
    /// The component this selection was made for (`component_digest`);
    /// None in a legacy lock, which binds the whole board instead.
    pub electrical_sha256: Option<String>,
}

impl Selection {
    /// The assembler sources and places this component.
    pub fn ordered(&self) -> bool {
        self.populate == "supplier"
    }

    /// Identity required for this component's populate mode.
    pub fn exact(&self) -> bool {
        let identity = self.manufacturer.is_some() && self.mpn.is_some() && self.footprint.is_some();
        match self.populate.as_str() {
            "none" => true,
            "consigned" => identity,
            _ => identity && self.supplier.is_some() && self.supplier_part.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub board: String,
    /// The whole-board digest of a legacy (v0.1-v0.3) lock; None for v0.4,
    /// whose selections carry their own.
    pub electrical_sha256: Option<String>,
    pub selections: Vec<Selection>,
}

/// `_text`: a nonempty, trimmed string without control characters.
fn check_text(value: &str, name: &str) -> Result<()> {
    let trimmed = value.trim_matches(python_isspace);
    if trimmed.is_empty() || trimmed.len() != value.len() || value.chars().any(|c| (c as u32) < 32) {
        return fail(format!("{name} must be a nonempty, trimmed string without control characters"));
    }
    Ok(())
}

fn text_field(value: &Value, name: &str) -> Result<String> {
    match value {
        Value::Str(text) => check_text(text, name).map(|()| text.clone()),
        _ => fail(format!("{name} must be a nonempty, trimmed string without control characters")),
    }
}

fn optional_text(value: Option<&Value>, name: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => text_field(value, name).map(Some),
    }
}

/// The `TypeError` of a dataclass constructor called without required fields.
fn missing_arguments(class: &str, required: &[&str], entry: &Object) -> Result<()> {
    let missing: Vec<String> =
        required.iter().filter(|name| !entry.contains_key(**name)).map(|name| format!("'{name}'")).collect();
    let listed = match missing.as_slice() {
        [] => return Ok(()),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    let noun = if missing.len() == 1 { "argument" } else { "arguments" };
    fail(format!(
        "invalid assembly lock: {class}.__init__() missing {} required positional {noun}: {listed}",
        missing.len()
    ))
}

fn offer(entry: &Object) -> Result<Offer> {
    missing_arguments("Offer", &["supplier", "supplier_part"], entry)?;
    Ok(Offer {
        supplier: text_field(&entry["supplier"], "supplier")?,
        supplier_part: text_field(&entry["supplier_part"], "supplier_part")?,
        source_url: optional_text(entry.get("source_url"), "source_url")?,
    })
}

fn digest_field(value: &Value) -> Result<String> {
    match value {
        Value::Str(text) if text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => {
            Ok(text.clone())
        }
        _ => fail("electrical_sha256 must be a lowercase SHA-256 digest"),
    }
}

/// `AssemblySelection(**entry)` with its `__post_init__` checks, in order.
fn selection(entry: &Object, alternates: Vec<Offer>) -> Result<Selection> {
    missing_arguments("AssemblySelection", &["reference", "part", "footprint"], entry)?;
    let reference = text_field(&entry["reference"], "reference")?;
    let part = text_field(&entry["part"], "part")?;
    let footprint = optional_text(entry.get("footprint"), "footprint")?;
    let manufacturer = optional_text(entry.get("manufacturer"), "manufacturer")?;
    let mpn = optional_text(entry.get("mpn"), "mpn")?;
    let supplier = optional_text(entry.get("supplier"), "supplier")?;
    let supplier_part = optional_text(entry.get("supplier_part"), "supplier_part")?;
    let source_url = optional_text(entry.get("source_url"), "source_url")?;
    let reviewed = match entry.get("reviewed") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return fail("reviewed must be a boolean"),
    };
    let populate = match entry.get("populate") {
        None => "supplier".to_owned(),
        Some(Value::Str(mode)) if POPULATE_MODES.contains(&mode.as_str()) => mode.clone(),
        // Python compares with `==`, so 1 and True are not modes either.
        Some(_) => return fail(format!("populate must be one of {}", POPULATE_MODES.join(", "))),
    };
    let electrical_sha256 = entry.get("electrical_sha256").map(digest_field).transpose()?;
    Ok(Selection {
        reference,
        part,
        footprint,
        manufacturer,
        mpn,
        supplier,
        supplier_part,
        source_url,
        reviewed,
        populate,
        alternates,
        electrical_sha256,
    })
}

/// `load_lock(path)`; `shown` is `str(path)` for OS errors.
pub fn load_lock(path: &Path, shown: &str) -> Result<Lock> {
    let bytes = std::fs::read(path).map_err(|failure| AssemblyError(files::open_error(&failure, shown)))?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => files::universal_newlines(&text),
        Err(failure) => return fail(files::decode_error(failure.as_bytes(), failure.utf8_error())),
    };
    let data = match copper_json::parse_unique(&text) {
        Ok(data) => data,
        Err(UniqueError::DuplicateKey(key)) => return fail(format!("duplicate JSON field: {key}")),
        Err(UniqueError::Syntax(failure)) => {
            let message = if text.starts_with('\u{feff}') {
                "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)".to_owned()
            } else {
                failure.python_message(&text)
            };
            return fail(format!("invalid assembly lock: {message}"));
        }
    };
    let Value::Object(data) = data else {
        return fail("expected schema, board, selections and (legacy) electrical_sha256 fields only");
    };
    let schema = data.get("schema").and_then(Value::as_str);
    let legacy = schema.is_some_and(|schema| LEGACY_SCHEMAS.contains(&schema));
    let top: &[&str] = if legacy {
        &["schema", "board", "electrical_sha256", "selections"]
    } else {
        &["schema", "board", "selections"]
    };
    if data.len() != top.len() || !top.iter().all(|key| data.contains_key(*key)) {
        return fail("expected schema, board, selections and (legacy) electrical_sha256 fields only");
    }
    let schema = schema.filter(|schema| legacy || *schema == SCHEMA);
    let (Some(schema), Some(entries)) = (schema, data["selections"].as_array()) else {
        return fail("unsupported assembly schema or invalid selections list");
    };
    let mut selections = Vec::new();
    for entry in entries {
        let Some(entry) =
            entry.as_object().filter(|entry| entry.keys().all(|k| SELECTION_FIELDS.contains(&k.as_str())))
        else {
            return fail("selection must be an object with known fields only");
        };
        let mut alternates = Vec::new();
        if let Some(offers) = entry.get("alternates") {
            let offers = offers.as_array().filter(|offers| {
                (schema == ALTERNATES_SCHEMA || schema == SCHEMA)
                    && offers.iter().all(|offer| {
                        offer.as_object().is_some_and(|offer| offer.keys().all(|k| OFFER_FIELDS.contains(&k.as_str())))
                    })
            });
            let Some(offers) = offers else {
                return fail(format!("alternates require {ALTERNATES_SCHEMA} or later and supplier offer objects"));
            };
            for item in offers {
                alternates.push(offer(item.as_object().expect("checked above"))?);
            }
        }
        if legacy == entry.contains_key("electrical_sha256") {
            return fail(format!("{SCHEMA} selections, and only they, require electrical_sha256"));
        }
        selections.push(selection(entry, alternates)?);
    }
    let board = text_field(&data["board"], "board")?;
    let electrical_sha256 = if legacy { Some(digest_field(&data["electrical_sha256"])?) } else { None };
    Ok(Lock { board, electrical_sha256, selections })
}

fn json_text(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, Value::from)
}

/// `lock_to_json(lock)`: always v0.4, so every selection needs its digest.
pub fn lock_to_json(lock: &Lock) -> String {
    let selections = lock
        .selections
        .iter()
        .map(|entry| {
            let mut fields: Object = Object::new();
            fields.insert("reference".into(), Value::from(entry.reference.as_str()));
            fields.insert("part".into(), Value::from(entry.part.as_str()));
            fields.insert("footprint".into(), json_text(&entry.footprint));
            fields.insert("manufacturer".into(), json_text(&entry.manufacturer));
            fields.insert("mpn".into(), json_text(&entry.mpn));
            fields.insert("supplier".into(), json_text(&entry.supplier));
            fields.insert("supplier_part".into(), json_text(&entry.supplier_part));
            fields.insert("source_url".into(), json_text(&entry.source_url));
            fields.insert("reviewed".into(), Value::Bool(entry.reviewed));
            fields.insert("populate".into(), Value::from(entry.populate.as_str()));
            fields.insert("electrical_sha256".into(), json_text(&entry.electrical_sha256));
            if !entry.alternates.is_empty() {
                let offers = entry
                    .alternates
                    .iter()
                    .map(|offer| {
                        let mut item = Object::new();
                        item.insert("supplier".into(), Value::from(offer.supplier.as_str()));
                        item.insert("supplier_part".into(), Value::from(offer.supplier_part.as_str()));
                        if let Some(url) = &offer.source_url {
                            item.insert("source_url".into(), Value::from(url.as_str()));
                        }
                        Value::Object(item)
                    })
                    .collect();
                fields.insert("alternates".into(), Value::Array(offers));
            }
            Value::Object(fields)
        })
        .collect();
    let mut document = Object::new();
    document.insert("schema".into(), Value::from(SCHEMA));
    document.insert("board".into(), Value::from(lock.board.as_str()));
    document.insert("selections".into(), Value::Array(selections));
    Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n"
}

fn sha256_hex(document: &Value) -> String {
    let canonical = Dumps::new().indent(2).sort_keys(true).dumps(document) + "\n";
    Sha256::digest(canonical.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `electrical_digest(board)`: design semantics and library provenance,
/// without the constraint origins that name checkout paths. Legacy locks
/// bind the whole board with it.
pub fn electrical_digest(board: &Board) -> String {
    let mut document = board_value(board);
    let strip = |unit: &mut Object| {
        if let Some(Value::Array(constraints)) = unit.get_mut("constraints") {
            for constraint in constraints {
                if let Value::Object(constraint) = constraint {
                    constraint.shift_remove("origins");
                }
            }
        }
    };
    strip(&mut document);
    if let Some(Value::Array(modules)) = document.get_mut("module_definitions") {
        for module in modules {
            if let Value::Object(module) = module {
                strip(module);
            }
        }
    }
    sha256_hex(&Value::Object(document))
}

/// The flat components by reference with their library parts (`_components`),
/// and the digest of each component's electrical context.
pub struct Components {
    pub by_reference: IndexMap<String, (ComponentInstance, PartDefinition)>,
    pub digests: IndexMap<String, String>,
}

pub fn components(board: &Board) -> Result<Components> {
    let flat = elaborate(board).map_err(AssemblyError)?;
    let mut by_reference = IndexMap::new();
    for component in &flat.components {
        if by_reference.contains_key(&component.reference) {
            return fail(format!("duplicate component reference: {}", component.reference));
        }
        let Some(part) = flat.library.get(&component.part) else {
            return fail(format!("unknown part: {}", component.part));
        };
        by_reference.insert(component.reference.clone(), (component.clone(), part.clone()));
    }
    let digests = component_digests(&flat, &by_reference);
    Ok(Components { by_reference, digests })
}

/// What one selection was reviewed against: the component (value, footprint,
/// properties), its library part and device, and for every pin the net it is
/// on, that net's supplies and voltages, and any no-connect. Changes elsewhere
/// on the board leave it unchanged.
fn component_digests(
    flat: &FlatElectricalView,
    by_reference: &IndexMap<String, (ComponentInstance, PartDefinition)>,
) -> IndexMap<String, String> {
    let mut nets: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &flat.nets {
        for endpoint in &net.endpoints {
            nets.insert((endpoint.component.as_str(), endpoint.pin.as_str()), net.name.as_str());
        }
    }
    let no_connects: Vec<(&str, &str)> =
        flat.no_connects.iter().map(|item| (item.endpoint.component.as_str(), item.endpoint.pin.as_str())).collect();
    let mut digests = IndexMap::new();
    for (reference, (component, part)) in by_reference {
        let mut pins: Vec<&str> = part.pins.keys().map(String::as_str).collect();
        pins.sort();
        let pins = pins
            .into_iter()
            .map(|pin| {
                let net = nets.get(&(reference.as_str(), pin)).copied();
                let mut supplies: Vec<Value> = flat
                    .supplies
                    .iter()
                    .filter(|supply| Some(supply.net.as_str()) == net)
                    .map(|supply| {
                        Value::Object(Object::from_iter([
                            ("name".to_owned(), Value::from(supply.name.as_str())),
                            ("voltage".to_owned(), quantity_value(&supply.voltage)),
                            ("externally_driven".to_owned(), Value::Bool(supply.externally_driven)),
                        ]))
                    })
                    .collect();
                supplies.sort_by_key(|supply| Dumps::compact().dumps(supply));
                Value::Object(Object::from_iter([
                    ("pin".to_owned(), Value::from(pin)),
                    ("net".to_owned(), net.map_or(Value::Null, Value::from)),
                    ("supplies".to_owned(), Value::Array(supplies)),
                    ("no_connect".to_owned(), Value::Bool(no_connects.contains(&(reference.as_str(), pin)))),
                ]))
            })
            .collect();
        let device = part.device.as_ref().and_then(|name| flat.devices.get(name)).map_or(Value::Null, device_value);
        let document = Value::Object(Object::from_iter([
            ("component".to_owned(), component_value(component)),
            ("footprint".to_owned(), json_text(&footprint(component, part))),
            ("part".to_owned(), part_value(part)),
            ("device".to_owned(), device),
            ("pins".to_owned(), Value::Array(pins)),
        ]));
        digests.insert(reference.clone(), sha256_hex(&document));
    }
    digests
}

fn footprint(component: &ComponentInstance, part: &PartDefinition) -> Option<String> {
    component.footprint.clone().or_else(|| part.footprints.first().cloned())
}

/// `snapshot(board)`: every assembled component, unreviewed.
pub fn snapshot(board: &Board) -> Result<Lock> {
    let components = components(board)?;
    let mut references: Vec<&String> = components.by_reference.keys().collect();
    references.sort();
    let mut selections = Vec::new();
    for reference in references {
        let (component, part) = &components.by_reference[reference];
        if !part.assembled {
            continue;
        }
        let mut entry = Object::new();
        entry.insert("reference".into(), Value::from(reference.as_str()));
        entry.insert("part".into(), Value::from(component.part.as_str()));
        entry.insert("footprint".into(), json_text(&footprint(component, part)));
        entry.insert("manufacturer".into(), json_text(&part.manufacturer));
        entry.insert("electrical_sha256".into(), Value::from(components.digests[reference].as_str()));
        selections.push(selection(&entry, Vec::new())?);
    }
    Ok(Lock { board: text_field(&Value::from(board.name.as_str()), "board")?, electrical_sha256: None, selections })
}

/// What `update` changed, by reference.
#[derive(Debug, Default)]
pub struct Update {
    /// Selections whose component is unchanged; their review is kept.
    pub unchanged: usize,
    /// (reference, what changed, whether a review was reset).
    pub changed: Vec<(String, String, bool)>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

fn shown(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("none")
}

/// `update(board, lock)`: rebind every selection to the current board. A
/// selection whose component is unchanged keeps its identity and review; a
/// changed one keeps its identity for reference but is no longer reviewed.
/// New components get unreviewed snapshot entries and selections for
/// components that are gone are dropped. Never marks anything reviewed.
pub fn update(board: &Board, lock: &Lock) -> Result<(Lock, Update)> {
    if lock.board != board.name {
        return fail(format!("lock belongs to {}, not {}", lock.board, board.name));
    }
    let mut previous: IndexMap<&str, &Selection> = IndexMap::new();
    for entry in &lock.selections {
        if previous.insert(entry.reference.as_str(), entry).is_some() {
            return fail(format!("{} has more than one selection; keep one before updating", entry.reference));
        }
    }
    // A legacy lock only records whether anything on the board changed.
    let legacy_changed = lock.electrical_sha256.as_ref().is_some_and(|digest| *digest != electrical_digest(board));
    let fresh = snapshot(board)?;
    let mut result = Update::default();
    let mut selections = Vec::new();
    for current in fresh.selections {
        let reference = current.reference.clone();
        let Some(entry) = previous.shift_remove(reference.as_str()) else {
            result.added.push(reference);
            selections.push(current);
            continue;
        };
        let mut changes = Vec::new();
        if entry.part != current.part {
            changes.push(format!("part {} -> {}", entry.part, current.part));
        }
        if entry.footprint != current.footprint {
            changes.push(format!("footprint {} -> {}", shown(&entry.footprint), shown(&current.footprint)));
        }
        if changes.is_empty() {
            if lock.electrical_sha256.is_some() && legacy_changed {
                changes.push("not verifiable: the legacy whole-board digest changed".to_owned());
            } else if lock.electrical_sha256.is_none() && entry.electrical_sha256 != current.electrical_sha256 {
                changes.push("value, part data or connections changed".to_owned());
            }
        }
        let mut kept = Selection {
            part: current.part,
            footprint: current.footprint,
            electrical_sha256: current.electrical_sha256,
            ..entry.clone()
        };
        if changes.is_empty() {
            result.unchanged += 1;
        } else {
            result.changed.push((reference, changes.join("; "), kept.reviewed));
            kept.reviewed = false;
        }
        selections.push(kept);
    }
    result.removed = previous.keys().map(|reference| (*reference).to_owned()).collect();
    result.removed.sort();
    Ok((Lock { board: lock.board.clone(), electrical_sha256: None, selections }, result))
}

fn unresolved_message(populate: &str) -> &'static str {
    match populate {
        "supplier" => "manufacturer, full MPN, supplier, supplier part and footprint are required",
        "consigned" => "a consigned component still requires manufacturer, full MPN and footprint",
        _ => "",
    }
}

/// `check_assembly(board, lock)`: the report document.
pub fn check_assembly(board: &Board, lock: &Lock) -> Result<Object> {
    let components = components(board)?;
    let mut required: Vec<&String> = components
        .by_reference
        .iter()
        .filter(|(_, (_, part))| part.assembled)
        .map(|(reference, _)| reference)
        .collect();
    required.sort();
    let mut issues: Vec<Value> = Vec::new();
    let mut issue = |code: &str, reference: Option<&str>, message: &str| {
        let mut item = Object::new();
        item.insert("code".into(), Value::from(code));
        item.insert("reference".into(), reference.map_or(Value::Null, Value::from));
        item.insert("message".into(), Value::from(message));
        issues.push(Value::Object(item));
    };
    if lock.board != board.name {
        issue("BOARD_MISMATCH", None, "lock belongs to a different board");
    }
    if lock.electrical_sha256.as_ref().is_some_and(|digest| *digest != electrical_digest(board)) {
        issue(
            "STALE_LOCK",
            None,
            "compiled electrical IR changed; run `copper assembly update` and review the reset selections",
        );
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut offers: IndexMap<(String, String), (Option<String>, Option<String>)> = IndexMap::new();
    let (mut exact, mut reviewed) = (Vec::new(), Vec::new());
    let mut populate: IndexMap<&str, Vec<&str>> = POPULATE_MODES.iter().map(|mode| (*mode, Vec::new())).collect();
    let mut entries: Vec<&Selection> = lock.selections.iter().collect();
    entries.sort_by(|a, b| a.reference.cmp(&b.reference));
    for entry in entries {
        let reference = entry.reference.as_str();
        if seen.contains(&reference) {
            issue("DUPLICATE_SELECTION", Some(reference), "component has more than one selection");
        } else {
            seen.push(reference);
        }
        let Some((component, part)) = components.by_reference.get(reference) else {
            issue("UNKNOWN_REFERENCE", Some(reference), "selection does not name a board component");
            continue;
        };
        if !part.assembled {
            issue("NON_ASSEMBLED_REFERENCE", Some(reference), "library part is explicitly non-assembled");
            continue;
        }
        if entry.part != component.part {
            issue("PART_MISMATCH", Some(reference), "selected library part differs from the board");
        }
        if entry.footprint != footprint(component, part) {
            issue("FOOTPRINT_MISMATCH", Some(reference), "selected footprint differs from the board");
        }
        if lock.electrical_sha256.is_none() && entry.electrical_sha256.as_ref() != components.digests.get(reference) {
            issue(
                "STALE_SELECTION",
                Some(reference),
                "part, footprint, value or connections changed; run `copper assembly update` and review it",
            );
        }
        if entry.populate != "none"
            && let Some(manufacturer) = part.manufacturer.as_deref().filter(|m| !m.is_empty())
            && entry.manufacturer.as_deref() != Some(manufacturer)
        {
            issue("MANUFACTURER_MISMATCH", Some(reference), "selection must use the library's manufacturer identity");
        }
        if entry.exact() {
            if !exact.contains(&reference) {
                exact.push(reference);
            }
        } else {
            issue("UNRESOLVED_SELECTION", Some(reference), unresolved_message(&entry.populate));
        }
        let modes = populate.get_mut(entry.populate.as_str()).expect("validated populate mode");
        if !modes.contains(&reference) {
            modes.push(reference);
        }
        if entry.reviewed {
            if !reviewed.contains(&reference) {
                reviewed.push(reference);
            }
        } else {
            issue("REVIEW_REQUIRED", Some(reference), "package, ratings, mapping and footprint have not been approved");
        }
        if !entry.alternates.is_empty() && !(entry.manufacturer.is_some() && entry.mpn.is_some()) {
            issue(
                "UNRESOLVED_SELECTION",
                Some(reference),
                "alternate offers require the manufacturer and full MPN they sell",
            );
        }
        let mut listed: Vec<(&str, &str)> = Vec::new();
        if let (Some(supplier), Some(code)) = (&entry.supplier, &entry.supplier_part) {
            listed.push((supplier, code));
        }
        listed.extend(entry.alternates.iter().map(|offer| (offer.supplier.as_str(), offer.supplier_part.as_str())));
        let mut folded: Vec<(String, &str)> = Vec::new();
        for (supplier, code) in &listed {
            let key = (python_casefold(supplier), *code);
            if !folded.contains(&key) {
                folded.push(key);
            }
        }
        if folded.len() != listed.len() {
            issue("DUPLICATE_OFFER", Some(reference), "one supplier code is listed more than once");
        }
        for (supplier, code) in listed {
            if supplier == "jlcpcb" && !is_jlcpcb_code(code) {
                issue(
                    "INVALID_SUPPLIER_PART",
                    Some(reference),
                    "JLCPCB codes must be C followed by a positive integer",
                );
            }
            let key = (supplier.to_owned(), code.to_owned());
            let identity = (entry.manufacturer.clone(), entry.mpn.clone());
            if offers.get(&key).is_some_and(|known| *known != identity) {
                issue(
                    "CONFLICTING_OFFER",
                    Some(reference),
                    "one supplier code is assigned to different manufacturer parts",
                );
            }
            offers.insert(key, identity);
        }
    }
    for reference in &required {
        if !seen.contains(&reference.as_str()) {
            issue("MISSING_SELECTION", Some(reference), "populated component has no assembly selection");
        }
    }
    let sorted = |names: &[&str]| -> Value {
        let mut names = names.to_vec();
        names.sort();
        Value::Array(names.into_iter().map(Value::from).collect())
    };
    let count = |n: usize| Value::from(n as i64);
    let mut report = Object::new();
    report.insert("schema".into(), Value::from("copperscript-assembly-report/v0.1"));
    report.insert("board".into(), Value::from(board.name.as_str()));
    report.insert("passed".into(), Value::Bool(issues.is_empty()));
    report.insert("component_count".into(), count(required.len()));
    report.insert("exact_selection_count".into(), count(exact.len()));
    report.insert("reviewed_count".into(), count(reviewed.len()));
    report.insert("supplier_assembled_count".into(), count(populate["supplier"].len()));
    report.insert("consigned_count".into(), count(populate["consigned"].len()));
    report.insert("not_fitted_count".into(), count(populate["none"].len()));
    report.insert("consigned".into(), sorted(&populate["consigned"]));
    report.insert("not_fitted".into(), sorted(&populate["none"]));
    report.insert("availability".into(), Value::from("not_checked"));
    report.insert("manufacturing_ready".into(), Value::Bool(false));
    report.insert("issues".into(), Value::Array(issues));
    Ok(report)
}

/// `C[1-9][0-9]*` (Python's `[0-9]` is ASCII only).
fn is_jlcpcb_code(code: &str) -> bool {
    let Some(digits) = code.strip_prefix('C') else { return false };
    digits.starts_with(|c: char| ('1'..='9').contains(&c)) && digits.bytes().all(|b| b.is_ascii_digit())
}

pub fn passed(report: &Object) -> bool {
    matches!(report.get("passed"), Some(Value::Bool(true)))
}

/// `_designators`: the board designator of each selection, refusing two
/// references that flatten to the same designator.
fn designators(entries: &[Selection]) -> Result<IndexMap<String, String>> {
    let mut result = IndexMap::new();
    let mut owner: IndexMap<String, String> = IndexMap::new();
    for entry in entries {
        let designator = kicad_reference(&entry.reference);
        if let Some(previous) = owner.get(&designator)
            && *previous != entry.reference
        {
            return fail(format!(
                "references {previous} and {} both become board designator {designator}",
                entry.reference
            ));
        }
        owner.insert(designator.clone(), entry.reference.clone());
        result.insert(entry.reference.clone(), designator);
    }
    Ok(result)
}

fn erc_errors(board: &Board) -> Result<bool> {
    let diagnostics = copper_lang::erc::check(board, false).map_err(AssemblyError)?;
    Ok(copper_lang::erc::has_errors(&diagnostics))
}

/// `write_jlcpcb_bom(board, lock, path)`; `shown` is `str(path)`.
pub fn write_jlcpcb_bom(board: &Board, lock: &Lock, path: &Path) -> Result<()> {
    if erc_errors(board)? {
        return fail("ERC failed; BOM export refused");
    }
    if !passed(&check_assembly(board, lock)?) {
        return fail("assembly selections failed; BOM export refused");
    }
    let ordered = supplier_assembled(lock, "JLCPCB BOM export")?;
    write_bom_rows(board, &ordered, &designators(&lock.selections)?, path)
}

fn supplier_assembled<'a>(lock: &'a Lock, purpose: &str) -> Result<Vec<&'a Selection>> {
    let ordered: Vec<&Selection> = lock.selections.iter().filter(|entry| entry.ordered()).collect();
    if ordered.iter().any(|entry| entry.supplier.as_deref() != Some("jlcpcb")) {
        return fail(format!(
            "{purpose} requires jlcpcb offers for every supplier-assembled component; \
             mark the others populate=\"consigned\" or populate=\"none\""
        ));
    }
    if ordered.is_empty() {
        return fail("no supplier-assembled components; nothing to export");
    }
    Ok(ordered)
}

fn value_text(component: &ComponentInstance) -> Option<String> {
    match &component.value {
        Some(ComponentValue::Quantity(quantity)) => Some(quantity.to_string()),
        Some(ComponentValue::Text(text)) => Some(text.clone()),
        None => None,
    }
}

fn write_csv(path: &Path, content: &str) -> Result<()> {
    files::mkdir_parents(path.parent().unwrap_or(Path::new("")), true, true).map_err(AssemblyError)?;
    std::fs::write(path, content.as_bytes())
        .map_err(|failure| AssemblyError(files::open_error(&failure, &copper_lang::paths::display(path))))
}

/// `_write_bom_rows`: one row per (value, footprint, code, manufacturer, MPN).
fn write_bom_rows(
    board: &Board,
    entries: &[&Selection],
    designators: &IndexMap<String, String>,
    output: &Path,
) -> Result<()> {
    let components = components(board)?;
    type Key = (Option<String>, Option<String>, Option<String>, Option<String>, Option<String>);
    let mut groups: IndexMap<Key, Vec<String>> = IndexMap::new();
    for entry in entries {
        let (component, _) = &components.by_reference[&entry.reference];
        let value = value_text(component).or_else(|| entry.mpn.clone());
        let key = (
            value,
            entry.footprint.clone(),
            entry.supplier_part.clone(),
            entry.manufacturer.clone(),
            entry.mpn.clone(),
        );
        groups.entry(key).or_default().push(designators[&entry.reference].clone());
    }
    let mut rows: Vec<(Key, Vec<String>)> = groups.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut content = String::new();
    csv::write_row(
        &mut content,
        &["Comment", "Designator", "Footprint", "LCSC Part #", "Manufacturer", "MPN"].map(Field::from),
    );
    for ((value, footprint, code, manufacturer, mpn), mut references) in rows {
        references.sort();
        let joined = references.join(",");
        csv::write_row(
            &mut content,
            &[
                Field::from(value.as_deref()),
                Field::Text(&joined),
                Field::from(footprint.as_deref()),
                Field::from(code.as_deref()),
                Field::from(manufacturer.as_deref()),
                Field::from(mpn.as_deref()),
            ],
        );
    }
    write_csv(output, &content)
}

/// `_short_footprint`: library-qualified or path-like ids, reduced to the name.
fn short_footprint(footprint: Option<&str>) -> String {
    let Some(footprint) = footprint.filter(|f| !f.is_empty()) else { return String::new() };
    let name = footprint.rsplit('/').next().unwrap_or("");
    let name = name.rsplit(':').next().unwrap_or("");
    name.strip_suffix(".kicad_mod").unwrap_or(name).to_owned()
}

fn or_empty(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

/// `write_hand_assembly_list`: every component the supplier will not place.
pub fn write_hand_assembly_list(board: &Board, lock: &Lock, path: &Path) -> Result<usize> {
    if !passed(&check_assembly(board, lock)?) {
        return fail("assembly selections failed; hand-assembly list refused");
    }
    let designators = designators(&lock.selections)?;
    let mut groups: IndexMap<Vec<String>, Vec<String>> = IndexMap::new();
    for entry in &lock.selections {
        if entry.ordered() {
            continue;
        }
        let action = if entry.populate == "consigned" { "hand-assemble" } else { "do not fit" };
        let key = vec![
            action.to_owned(),
            or_empty(&entry.manufacturer),
            or_empty(&entry.mpn),
            short_footprint(entry.footprint.as_deref()),
            or_empty(&entry.supplier),
            or_empty(&entry.supplier_part),
            or_empty(&entry.source_url),
        ];
        groups.entry(key).or_default().push(designators[&entry.reference].clone());
    }
    let mut rows: Vec<(Vec<String>, Vec<String>)> = groups.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut content = String::new();
    csv::write_row(
        &mut content,
        &["Action", "Designator", "Quantity", "Manufacturer", "MPN", "Footprint", "Supplier", "Supplier Part", "URL"]
            .map(Field::from),
    );
    let mut total = 0;
    for (key, mut references) in rows {
        references.sort();
        total += references.len();
        let joined = references.join(",");
        let mut fields = vec![Field::Text(&key[0]), Field::Text(&joined), Field::Int(references.len())];
        fields.extend(key[1..].iter().map(|field| Field::Text(field)));
        csv::write_row(&mut content, &fields);
    }
    write_csv(path, &content)?;
    Ok(total)
}

/// `write_ordering_list`: every selection with its sourcing, whoever fits it.
pub fn write_ordering_list(board: &Board, lock: &Lock, path: &Path) -> Result<usize> {
    if !passed(&check_assembly(board, lock)?) {
        return fail("assembly selections failed; ordering list refused");
    }
    let components = components(board)?;
    let designators = designators(&lock.selections)?;
    let route = assembly_route;
    // One column pair per alternate distributor, by first spelling in
    // supplier order (a stable sort keeps lock order among equals).
    let mut all: Vec<&Offer> = lock.selections.iter().flat_map(|entry| &entry.alternates).collect();
    all.sort_by(|a, b| a.supplier.cmp(&b.supplier));
    let mut distributors: IndexMap<String, String> = IndexMap::new();
    for offer in all {
        distributors.entry(python_casefold(&offer.supplier)).or_insert_with(|| offer.supplier.clone());
    }
    let mut names: Vec<&String> = distributors.keys().collect();
    names.sort();
    let mut groups: IndexMap<Vec<String>, Vec<String>> = IndexMap::new();
    for entry in &lock.selections {
        let (component, _) = &components.by_reference[&entry.reference];
        let value = value_text(component).or_else(|| entry.mpn.clone()).unwrap_or_default();
        let mut offers: IndexMap<String, Offer> =
            entry.alternates.iter().map(|offer| (python_casefold(&offer.supplier), offer.clone())).collect();
        if let (Some(supplier), Some(code)) = (&entry.supplier, &entry.supplier_part) {
            offers.entry(python_casefold(supplier)).or_insert_with(|| Offer {
                supplier: supplier.clone(),
                supplier_part: code.clone(),
                source_url: entry.source_url.clone(),
            });
        }
        let mut key = vec![
            route(&entry.populate).to_owned(),
            value,
            or_empty(&entry.manufacturer),
            or_empty(&entry.mpn),
            short_footprint(entry.footprint.as_deref()),
            or_empty(&entry.supplier),
            or_empty(&entry.supplier_part),
            or_empty(&entry.source_url),
        ];
        for name in &names {
            match offers.get(*name) {
                Some(offer) => {
                    key.push(offer.supplier_part.clone());
                    key.push(or_empty(&offer.source_url));
                }
                None => key.extend([String::new(), String::new()]),
            }
        }
        groups.entry(key).or_default().push(designators[&entry.reference].clone());
    }
    let mut rows: Vec<(Vec<String>, Vec<String>)> = groups
        .into_iter()
        .map(|(key, mut references)| {
            references.sort();
            (key, references)
        })
        .collect();
    rows.sort_by(|a, b| (&a.1, &a.0).cmp(&(&b.1, &b.0)));
    let mut header: Vec<String> =
        ["Designator", "Quantity", "Value", "Manufacturer", "MPN", "Footprint", "Assembly", "Supplier"]
            .map(str::to_owned)
            .to_vec();
    header.extend(["Supplier Part", "URL"].map(str::to_owned));
    for name in &names {
        for label in ["Part", "URL"] {
            header.push(format!("{} {label}", distributors[*name]));
        }
    }
    let mut content = String::new();
    csv::write_row(&mut content, &header.iter().map(|h| Field::Text(h)).collect::<Vec<_>>());
    let mut total = 0;
    for (key, references) in rows {
        total += references.len();
        let joined = references.join(",");
        let mut fields = vec![
            Field::Text(&joined),
            Field::Int(references.len()),
            Field::Text(&key[1]),
            Field::Text(&key[2]),
            Field::Text(&key[3]),
            Field::Text(&key[4]),
            Field::Text(&key[0]),
            Field::Text(&key[5]),
            Field::Text(&key[6]),
            Field::Text(&key[7]),
        ];
        fields.extend(key[8..].iter().map(|field| Field::Text(field)));
        csv::write_row(&mut content, &fields);
    }
    write_csv(path, &content)?;
    Ok(total)
}

/// Schema of the assembly sheet a bench assembly tracker reads.
pub const SHEET_SCHEMA: &str = "copperscript.assembly-sheet/v1";

fn assembly_route(populate: &str) -> &'static str {
    match populate {
        "supplier" => "supplier-assembled",
        "consigned" => "hand-assemble",
        _ => "do not fit",
    }
}

/// `assembly_sheet`: every selection with its board designator, value, part
/// identity and assembly route, for tracking a build by hand. `lock_sha256`
/// is the digest of the lock file's bytes. Placement (side, position) is not
/// part of the sheet: it comes from the board the tracker opens.
pub fn assembly_sheet(board: &Board, lock: &Lock, lock_sha256: &str) -> Result<Value> {
    if !passed(&check_assembly(board, lock)?) {
        return fail("assembly selections failed; assembly sheet refused");
    }
    let components = components(board)?;
    let designators = designators(&lock.selections)?;
    let mut entries: Vec<&Selection> = lock.selections.iter().collect();
    entries.sort_by(|a, b| (&designators[&a.reference], &a.reference).cmp(&(&designators[&b.reference], &b.reference)));
    let mut counts: IndexMap<&str, usize> =
        ["supplier-assembled", "hand-assemble", "do not fit"].into_iter().map(|route| (route, 0)).collect();
    let items: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            let (component, _) = &components.by_reference[&entry.reference];
            let route = assembly_route(&entry.populate);
            *counts.entry(route).or_default() += 1;
            let mut item = Object::new();
            item.insert("reference".into(), Value::from(entry.reference.as_str()));
            item.insert("designator".into(), Value::from(designators[&entry.reference].as_str()));
            item.insert("part".into(), Value::from(entry.part.as_str()));
            item.insert("value".into(), json_text(&value_text(component)));
            item.insert("manufacturer".into(), json_text(&entry.manufacturer));
            item.insert("mpn".into(), json_text(&entry.mpn));
            item.insert("footprint".into(), json_text(&entry.footprint));
            item.insert("populate".into(), Value::from(entry.populate.as_str()));
            item.insert("assembly".into(), Value::from(route));
            item.insert("supplier".into(), json_text(&entry.supplier));
            item.insert("supplier_part".into(), json_text(&entry.supplier_part));
            item.insert("source_url".into(), json_text(&entry.source_url));
            // The digest the selection was reviewed against: a tracker keeps
            // progress for a component only while this is unchanged.
            item.insert("electrical_sha256".into(), Value::from(components.digests[&entry.reference].as_str()));
            Value::Object(item)
        })
        .collect();
    let mut lock_identity = Object::new();
    lock_identity.insert("schema".into(), Value::from(SCHEMA));
    lock_identity.insert("sha256".into(), Value::from(lock_sha256));
    let mut summary = Object::new();
    summary.insert("component_count".into(), Value::from(items.len() as u64));
    summary.insert("supplier_assembled_count".into(), Value::from(counts["supplier-assembled"] as u64));
    summary.insert("hand_assemble_count".into(), Value::from(counts["hand-assemble"] as u64));
    summary.insert("not_fitted_count".into(), Value::from(counts["do not fit"] as u64));
    let mut document = Object::new();
    document.insert("schema".into(), Value::from(SHEET_SCHEMA));
    document.insert("board".into(), Value::from(board.name.as_str()));
    document.insert("lock".into(), Value::Object(lock_identity));
    document.insert("summary".into(), Value::Object(summary));
    document.insert("components".into(), Value::Array(items));
    document.insert(
        "limits".into(),
        Value::Array(
            [
                "Placement, side and pads come from the board export the tracker opens, matched by reference.",
                "Recording a component as placed or inspected is not assembly, electrical or manufacturing qualification.",
            ]
            .map(Value::from)
            .to_vec(),
        ),
    );
    Ok(Value::Object(document))
}

/// `write_assembly_sheet`: the sheet as indented JSON; returns the number of
/// components it lists.
pub fn write_assembly_sheet(board: &Board, lock: &Lock, lock_sha256: &str, path: &Path) -> Result<usize> {
    let document = assembly_sheet(board, lock, lock_sha256)?;
    let count = document.get("components").and_then(Value::as_array).map_or(0, <[Value]>::len);
    write_csv(path, &(Dumps::new().indent(2).dumps(&document) + "\n"))?;
    Ok(count)
}

/// `write_candidate_bom(board, lock, directory, native_positions=...)`;
/// `positions` is the position CSV with its `str()`.
pub fn write_candidate_bom(
    board: &Board,
    lock: &Lock,
    directory: &Path,
    positions: Option<(&Path, &str)>,
) -> Result<()> {
    if erc_errors(board)? {
        return fail("ERC failed; candidate BOM refused");
    }
    let mut diagnostics = check_assembly(board, lock)?;
    let inconsistent = diagnostics["issues"]
        .as_array()
        .expect("issues list")
        .iter()
        .any(|issue| issue.get("code").and_then(Value::as_str) != Some("REVIEW_REQUIRED"));
    if inconsistent {
        return fail("candidate selections are inconsistent or incomplete");
    }
    let designators = designators(&lock.selections)?;
    let fitted: Vec<&Selection> = lock.selections.iter().filter(|entry| entry.populate != "none").collect();
    let ordered: Vec<&Selection> = fitted
        .iter()
        .copied()
        .filter(|entry| entry.populate == "supplier" && entry.supplier.as_deref() == Some("jlcpcb"))
        .collect();
    let reference_map: IndexMap<String, String> =
        fitted.iter().map(|entry| (entry.reference.clone(), designators[&entry.reference].clone())).collect();
    let mut jlc_references: Vec<String> = ordered.iter().map(|entry| designators[&entry.reference].clone()).collect();
    jlc_references.sort();
    jlc_references.dedup();
    files::mkdir_parents(directory, true, false).map_err(AssemblyError)?;
    if !ordered.is_empty() {
        write_bom_rows(board, &ordered, &designators, &directory.join("bom-candidate-NOT-APPROVED.csv"))?;
    }
    let mut content = String::new();
    csv::write_row(
        &mut content,
        &["Designator", "Footprint", "Manufacturer", "MPN", "Supplier", "Supplier Part", "Source URL", "Reviewed"]
            .map(Field::from),
    );
    let mut sorted = fitted.clone();
    sorted.sort_by(|a, b| a.reference.cmp(&b.reference));
    for entry in sorted {
        csv::write_row(
            &mut content,
            &[
                Field::Text(&reference_map[&entry.reference]),
                Field::from(entry.footprint.as_deref()),
                Field::from(entry.manufacturer.as_deref()),
                Field::from(entry.mpn.as_deref()),
                Field::from(entry.supplier.as_deref()),
                Field::from(entry.supplier_part.as_deref()),
                Field::from(entry.source_url.as_deref()),
                Field::Bool(entry.reviewed),
            ],
        );
    }
    write_csv(&directory.join("all-suppliers-NOT-APPROVED.csv"), &content)?;
    let mut others: Vec<String> =
        reference_map.values().filter(|designator| !jlc_references.contains(designator)).cloned().collect();
    others.sort();
    others.dedup();
    let strings = |items: &[String]| Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect());
    diagnostics.insert("purpose".into(), Value::from("human_review_only"));
    diagnostics.insert("qualified_release".into(), Value::Bool(false));
    diagnostics.insert("ordering_approved".into(), Value::Bool(false));
    diagnostics.insert(
        "native_reference_map".into(),
        Value::Object(reference_map.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect()),
    );
    diagnostics.insert("jlcpcb_references".into(), strings(&jlc_references));
    diagnostics.insert("other_supplier_references".into(), strings(&others));
    if let Some((positions, positions_shown)) = positions {
        let read =
            || std::fs::read(positions).map_err(|failure| AssemblyError(files::open_error(&failure, positions_shown)));
        let before = files::sha256_hex(&read()?);
        let fitted_designators: Vec<String> = reference_map.values().cloned().collect();
        let rejected = |failure: crate::manufacturing::Error| match failure {
            crate::manufacturing::Error::Files(message) => AssemblyError(format!("candidate CPL rejected: {message}")),
            other => AssemblyError(other.to_string()),
        };
        // Validate every fitted part, including those assembled separately.
        crate::manufacturing::write_jlcpcb_cpl(
            positions,
            positions_shown,
            &directory.join("all-suppliers-positions-NOT-APPROVED.csv"),
            &fitted_designators,
        )
        .map_err(rejected)?;
        if !jlc_references.is_empty() {
            crate::manufacturing::write_jlcpcb_cpl(
                positions,
                positions_shown,
                &directory.join("cpl-candidate-NOT-APPROVED.csv"),
                &jlc_references,
            )
            .map_err(rejected)?;
        }
        if files::sha256_hex(&read()?) != before {
            return fail("native positions changed during candidate review export");
        }
        diagnostics.insert("native_positions_sha256".into(), Value::from(before.as_str()));
        diagnostics
            .insert("placement_review".into(), Value::from("not_approved; source/pose/pin-1/rotation review required"));
    }
    let review = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(diagnostics)) + "\n";
    write_csv(&directory.join("assembly-review.json"), &review)?;
    write_csv(
        &directory.join("README.txt"),
        "UNAPPROVED ASSEMBLY REVIEW CANDIDATE\n\
         Do not submit this bundle as an approved order.\n\
         Review exact packages/ratings/pin maps, supplier stock and final CPL.\n\
         The candidate BOM/CPL contains only explicitly selected jlcpcb offers.\n\
         All-suppliers files also include parts needing separate procurement/assembly.\n\
         An LCSC catalog identity does not establish JLCPCB assembly eligibility.\n\
         Selection flags and production BOM gates have not been changed.\n",
    )?;
    Ok(())
}

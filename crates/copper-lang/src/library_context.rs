//! Compact source-derived library discovery (`pcbir.library_context`).
//!
//! Indexing parses declarations; library data is never executed or
//! qualified. Inspection retrieves one definition with explicit pagination.

use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_json::{Dumps, Object, Value as Json, python_float_repr, python_isspace, python_str_repr as repr};
use copper_state::StateValue;
use copper_units::Decimal;

use crate::error::CopperScriptError;
use crate::packages::{PackageResolver, find_manifest, inventory_checksum, module_inventory, read_manifest};
use crate::paths;
use crate::syntax::{Document, Location};
use crate::syntax_state;

pub const INDEX_SCHEMA: &str = "copperscript.library-index/v1";
pub const PAGE_SCHEMA: &str = "copperscript.library-context/v1";
pub const KINDS: [&str; 5] = ["part", "device", "module", "board_profile", "macro"];
pub const SECTIONS: [&str; 9] = [
    "overview",
    "interfaces",
    "requirements",
    "composition",
    "properties",
    "notes",
    "dependencies",
    "declarations",
    "source",
];
const INTERFACES: [&str; 11] = [
    "PinDecl",
    "PadDecl",
    "PortDecl",
    "PeripheralDecl",
    "InterfaceDecl",
    "MacroPadDecl",
    "UnitDecl",
    "PowerDomainDecl",
    "SignalGroupDecl",
    "ModeGroupDecl",
    "PadSetDecl",
];
const REQUIREMENTS: [&str; 6] =
    ["ConstraintDecl", "RouteRuleDecl", "SupplyDecl", "PowerStateDecl", "PhysicalMacroDecl", "MacroItemDecl"];
const COMPOSITION: [&str; 9] = [
    "ComponentDecl",
    "ModuleInstanceDecl",
    "NetDecl",
    "NoConnectDecl",
    "ConfigurationDecl",
    "MuxDecl",
    "MechanicalProfileUseDecl",
    "MechanicalItemDecl",
    "MechanicalDecl",
];
const CORE_PROPERTIES: [&str; 14] = [
    "category",
    "manufacturer",
    "assembled",
    "mpn",
    "description",
    "device",
    "footprint",
    "footprints",
    "value",
    "source_document",
    "source_revision",
    "source_location",
    "source_url",
    "source_checksum",
];
const PROPERTY_DECLS: [&str; 3] = ["PartPropertyDecl", "DevicePropertyDecl", "MacroPropertyDecl"];

/// A library failure: Python's `ValueError`/`OSError` text or a `CopperScriptError`.
pub type Result<T> = std::result::Result<T, String>;

fn compile(error: CopperScriptError) -> String {
    error.to_string()
}

/// `_json`: sorted, compact, ASCII JSON.
pub fn to_json(value: &Json) -> String {
    Dumps::compact().sort_keys(true).dumps(value)
}

fn chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn class_name(state: &StateValue) -> &str {
    match state {
        StateValue::Dataclass { name, .. } => name.rsplit(':').next().unwrap_or(name),
        _ => "",
    }
}

fn field<'a>(state: &'a StateValue, name: &str) -> Option<&'a StateValue> {
    match state {
        StateValue::Dataclass { fields, .. } => fields.get(name),
        _ => None,
    }
}

fn state_text(state: Option<&StateValue>) -> String {
    match state {
        Some(StateValue::Str(text)) => text.clone(),
        _ => String::new(),
    }
}

/// `_value`: a declaration as plain data, without source positions.
fn value(state: &StateValue) -> Json {
    match state {
        StateValue::Dataclass { name, fields } => {
            let short = name.rsplit(':').next().unwrap_or(name);
            if short == "RawQuantity" {
                let number =
                    Decimal::parse(&state_text(fields.get("value"))).map(|d| d.decimal_text()).unwrap_or_default();
                return Json::from(format!("{number} {}", state_text(fields.get("unit"))).as_str());
            }
            if short == "SourceLocation" {
                let line = fields.get("line").map(value).unwrap_or(Json::Null);
                let column = fields.get("column").map(value).unwrap_or(Json::Null);
                return Json::Object(Object::from([("line".to_owned(), line), ("column".to_owned(), column)]));
            }
            Json::Object(
                fields
                    .iter()
                    .filter(|(key, _)| key.as_str() != "location" && key.as_str() != "endpoint_locations")
                    .map(|(key, item)| (key.clone(), value(item)))
                    .collect(),
            )
        }
        StateValue::Map { entries, .. } => {
            Json::Object(entries.iter().map(|(key, item)| (state_text(Some(key)), value(item))).collect())
        }
        StateValue::Tuple(items) | StateValue::List(items) => Json::Array(items.iter().map(value).collect()),
        StateValue::Str(text) => Json::from(text.as_str()),
        StateValue::Int(number) => Json::Int(number.clone()),
        StateValue::Float(number) => Json::Float(*number),
        StateValue::Bool(flag) => Json::Bool(*flag),
        _ => Json::Null,
    }
}

/// Python's `repr` of a plain JSON-like value.
fn py_repr(value: &Json) -> String {
    match value {
        Json::Null => "None".to_owned(),
        Json::Bool(true) => "True".to_owned(),
        Json::Bool(false) => "False".to_owned(),
        Json::Int(number) => number.to_string(),
        Json::Float(number) => python_float_repr(*number),
        Json::Str(text) => repr(text),
        Json::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Json::Object(items) => format!(
            "{{{}}}",
            items.iter().map(|(key, item)| format!("{}: {}", repr(key), py_repr(item))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Python's `str` of a plain JSON-like value.
fn py_str(value: &Json) -> String {
    match value {
        Json::Str(text) => text.clone(),
        other => py_repr(other),
    }
}

fn is_requirement(state: &StateValue) -> bool {
    let name = class_name(state);
    REQUIREMENTS.contains(&name)
        || (name == "ComponentDecl"
            && matches!(field(state, "attributes"), Some(StateValue::Map { entries, .. })
                if entries.iter().any(|(key, _)| matches!(key, StateValue::Str(k) if k == "role" || k == "decouples"))))
}

fn declaration_line(declaration: &StateValue) -> i64 {
    match field(declaration, "location").and_then(|location| field(location, "line")) {
        Some(StateValue::Int(line)) => line.try_into().unwrap_or(0),
        _ => 0,
    }
}

/// A parsed library source.
pub struct Source {
    pub document: Document,
    pub declarations: Vec<StateValue>,
    pub state: StateValue,
    pub text: String,
}

/// `Catalog`.
pub struct Catalog {
    pub providers: Vec<Json>,
    pub entries: Vec<Object>,
    pub diagnostics: Vec<Json>,
    pub complete: bool,
    pub sources: IndexMap<String, Source>,
    pub resolver: Option<PackageResolver>,
    pub roots: Vec<PathBuf>,
}

impl Catalog {
    pub fn index(&self) -> Json {
        Json::Object(Object::from([
            ("schema".to_owned(), Json::from(INDEX_SCHEMA)),
            ("providers".to_owned(), Json::Array(self.providers.clone())),
            ("entries".to_owned(), Json::Array(self.entries.iter().cloned().map(Json::Object).collect())),
            ("diagnostics".to_owned(), Json::Array(self.diagnostics.clone())),
            ("complete".to_owned(), Json::Bool(self.complete)),
        ]))
    }
}

fn entry_text<'a>(entry: &'a Object, key: &str) -> &'a str {
    match entry.get(key) {
        Some(Json::Str(text)) => text,
        _ => "",
    }
}

fn sort_entries(entries: &mut [Object]) {
    entries.sort_by(|a, b| {
        let key = |e: &Object| {
            (entry_text(e, "selector").to_owned(), entry_text(e, "kind").to_owned(), entry_text(e, "source").to_owned())
        };
        key(a).cmp(&key(b))
    });
}

/// `_notes`: lexer-skipped comments, with their lines.
fn notes(text: &str) -> Vec<Json> {
    let Ok(tokens) = crate::tokenize(text, "<notes>") else { return Vec::new() };
    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut previous = 0;
    for token in &tokens {
        let end = token.location.offset.min(chars.len());
        let mut index = previous.min(end);
        while index < end {
            let start = index;
            let marker = if chars[index] == '#' {
                Some(1)
            } else if chars[index] == '/' && index + 1 < end && chars[index + 1] == '/' {
                Some(2)
            } else {
                None
            };
            let Some(width) = marker else {
                index += 1;
                continue;
            };
            let mut stop = start + width;
            while stop < end && chars[stop] != '\r' && chars[stop] != '\n' {
                stop += 1;
            }
            let body: String = chars[start + width..stop].iter().collect();
            let line = chars[..start].iter().filter(|c| **c == '\n').count() + 1;
            result.push(Json::Object(Object::from([
                ("type".to_owned(), Json::from("SourceNote")),
                ("line".to_owned(), Json::from(line as i64)),
                ("text".to_owned(), Json::from(body.trim_matches(python_isspace))),
            ])));
            index = stop;
        }
        previous = token.end_offset;
    }
    result
}

/// `_provider`: the catalog of one module root.
fn provider(root: &Path, module: &str, version: &str, expected: Option<&str>) -> Result<Catalog> {
    let inventory = module_inventory(root)?;
    let checksum = inventory_checksum(&inventory);
    if expected.is_some_and(|expected| expected != checksum) {
        return Err(format!("library changed during discovery: {module}"));
    }
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let mut sources = IndexMap::new();
    for item in &inventory {
        if !item.path.ends_with(".copper") {
            continue;
        }
        let path = root.join(&item.path);
        let data = std::fs::read(&path).map_err(|e| paths::python_os_error(&e, Some(&paths::display(&path))))?;
        let digest: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
        if digest != item.sha256 {
            return Err(format!("library changed during discovery: {module}/{}", item.path));
        }
        let parsed = match std::str::from_utf8(&data) {
            Err(_) => Err(("ENCODING".to_owned(), format!("cannot decode {}/{} as UTF-8", module, item.path))),
            Ok(text) => crate::parse(text, &paths::display(&path)).map_err(|e| (e.code.clone(), e.to_string())),
        };
        let document = match parsed {
            Ok(document) => document,
            Err((code, detail)) => {
                diagnostics.push(Json::Object(Object::from([
                    ("source".to_owned(), Json::from(format!("{module}/{}", item.path).as_str())),
                    ("code".to_owned(), Json::from(code.as_str())),
                    ("detail".to_owned(), Json::from(chars(&detail, 300).as_str())),
                ])));
                continue;
            }
        };
        let kind = document.kind.as_str();
        if !KINDS.contains(&kind) {
            continue;
        }
        let directory = Path::new(&item.path).parent().map(paths::as_posix).unwrap_or_default();
        let package =
            if directory.is_empty() || directory == "." { module.to_owned() } else { format!("{module}/{directory}") };
        let selector = format!("{package}::{}", document.name);
        let declarations: Vec<StateValue> = document.declarations.iter().map(syntax_state::declaration).collect();
        let mut properties: IndexMap<String, Json> = IndexMap::new();
        for declaration in &declarations {
            if PROPERTY_DECLS.contains(&class_name(declaration)) {
                properties.insert(
                    state_text(field(declaration, "name")),
                    field(declaration, "value").map(value).unwrap_or(Json::Null),
                );
            }
        }
        let mut capabilities: Vec<String> = declarations
            .iter()
            .filter(|d| ["PeripheralDecl", "InterfaceDecl", "PortDecl"].contains(&class_name(d)))
            .map(|d| match field(d, "kind") {
                Some(kind) => state_text(Some(kind)),
                None => state_text(field(d, "pin_type")),
            })
            .filter(|capability| !capability.is_empty())
            .collect::<IndexSet<_>>()
            .into_iter()
            .collect();
        capabilities.sort();
        let mut metadata_keys: Vec<&str> =
            CORE_PROPERTIES.iter().copied().filter(|k| properties.contains_key(*k)).collect();
        metadata_keys.sort_unstable();
        let metadata: Object = metadata_keys
            .iter()
            .map(|k| ((*k).to_owned(), Json::from(chars(&py_str(&properties[*k]), 160).as_str())))
            .collect();
        let count =
            |predicate: &dyn Fn(&StateValue) -> bool| declarations.iter().filter(|d| predicate(d)).count() as i64;
        let entry = Object::from([
            ("selector".to_owned(), Json::from(selector.as_str())),
            ("kind".to_owned(), Json::from(kind)),
            ("symbol".to_owned(), Json::from(document.name.as_str())),
            ("package".to_owned(), Json::from(package.as_str())),
            ("provider".to_owned(), Json::from(module)),
            ("version".to_owned(), Json::from(version)),
            ("content_digest".to_owned(), Json::from(checksum.as_str())),
            ("source".to_owned(), Json::from(item.path.as_str())),
            ("source_sha256".to_owned(), Json::from(format!("sha256:{}", item.sha256).as_str())),
            ("line".to_owned(), Json::from(document.location.line as i64)),
            ("metadata".to_owned(), Json::Object(metadata)),
            ("metadata_is_preview".to_owned(), Json::Bool(true)),
            (
                "capabilities".to_owned(),
                Json::Array(capabilities.iter().take(8).map(|c| Json::from(c.as_str())).collect()),
            ),
            ("capabilities_omitted".to_owned(), Json::from(capabilities.len().saturating_sub(8) as i64)),
            (
                "counts".to_owned(),
                Json::Object(Object::from([
                    ("declarations".to_owned(), Json::from(declarations.len() as i64)),
                    ("imports".to_owned(), Json::from(document.imports.len() as i64)),
                    ("interfaces".to_owned(), Json::from(count(&|d| INTERFACES.contains(&class_name(d))))),
                    ("requirements".to_owned(), Json::from(count(&is_requirement))),
                ])),
            ),
        ]);
        let text = String::from_utf8(data).expect("decoded above");
        let state = syntax_state::document(&document);
        sources.insert(format!("{module}/{}", item.path), Source { document, declarations, state, text });
        entries.push(entry);
    }
    let mut duplicates: IndexMap<(String, String), usize> = IndexMap::new();
    for entry in &entries {
        *duplicates
            .entry((entry_text(entry, "selector").to_owned(), entry_text(entry, "kind").to_owned()))
            .or_default() += 1;
    }
    let mut repeated: Vec<&(String, String)> = duplicates.iter().filter(|(_, n)| **n > 1).map(|(k, _)| k).collect();
    repeated.sort();
    for (selector, kind) in repeated {
        diagnostics.push(Json::Object(Object::from([
            ("source".to_owned(), Json::from(selector.as_str())),
            ("code".to_owned(), Json::from("DUPLICATE_EXPORT")),
            (
                "detail".to_owned(),
                Json::from(format!("multiple {kind} definitions; repair this library before use").as_str()),
            ),
        ])));
    }
    if inventory_checksum(&module_inventory(root)?) != checksum {
        return Err(format!("library changed during discovery: {module}"));
    }
    sort_entries(&mut entries);
    let complete = diagnostics.is_empty();
    Ok(Catalog {
        providers: vec![Json::Object(Object::from([
            ("module".to_owned(), Json::from(module)),
            ("version".to_owned(), Json::from(version)),
            ("content_digest".to_owned(), Json::from(checksum.as_str())),
        ]))],
        entries,
        diagnostics,
        complete,
        sources,
        resolver: None,
        roots: vec![root.to_path_buf()],
    })
}

/// `index_library`: a portable index of one library root.
pub fn index_library(root: &Path) -> Result<Catalog> {
    let root = paths::resolve(root);
    let manifest = read_manifest(&root.join("copper.mod")).map_err(compile)?;
    provider(&root, &manifest.path, "workspace", None)
}

/// `project_catalog`: exactly the host project's declared dependencies.
pub fn project_catalog(project: &Path, locked: bool, offline: bool, providers: &[String]) -> Result<Catalog> {
    let Some(manifest_path) = find_manifest(&paths::resolve(project)) else {
        return Err("library discovery requires a project copper.mod".to_owned());
    };
    let manifest = read_manifest(&manifest_path).map_err(compile)?;
    let mut unknown: Vec<&String> = providers.iter().filter(|p| !manifest.requirements.contains_key(*p)).collect();
    unknown.sort();
    if let Some(first) = unknown.first() {
        return Err(format!("provider not declared in project: {first}"));
    }
    let mut modules: Vec<String> =
        if providers.is_empty() { manifest.requirements.keys().cloned().collect() } else { providers.to_vec() };
    modules.sort();
    modules.dedup();
    let mut resolver = PackageResolver::new(manifest, locked, offline).map_err(compile)?;
    let location = Location { filename: paths::display(&manifest_path).into(), offset: 0, line: 1, column: 1 };
    let mut catalog = Catalog {
        providers: Vec::new(),
        entries: Vec::new(),
        diagnostics: Vec::new(),
        complete: true,
        sources: IndexMap::new(),
        resolver: None,
        roots: Vec::new(),
    };
    for module in modules {
        let resolved = resolver.resolve(&module, &location).map_err(compile)?;
        let found = provider(&resolved.directory, &module, &resolved.version, Some(&resolved.checksum))?;
        catalog.providers.extend(found.providers);
        catalog.entries.extend(
            found
                .entries
                .into_iter()
                .filter(|e| resolver.matching_module(entry_text(e, "package")).as_deref() == Some(module.as_str())),
        );
        catalog.diagnostics.extend(found.diagnostics);
        catalog.sources.extend(found.sources);
        catalog.roots.extend(found.roots);
    }
    catalog.complete = catalog.diagnostics.is_empty();
    sort_entries(&mut catalog.entries);
    catalog.resolver = Some(resolver);
    Ok(catalog)
}

/// Paging options shared by search, inspect and diagnostics.
#[derive(Clone, Copy)]
pub struct PageOptions {
    pub offset: i64,
    pub limit: i64,
    pub max_chars: i64,
}

fn row_get<'a>(row: &'a Json, key: &str) -> Option<&'a Json> {
    match row {
        Json::Object(map) => map.get(key),
        _ => None,
    }
}

/// `_page`: a bounded page of rows.
fn page(header: Vec<(&str, Json)>, rows: Vec<Json>, options: PageOptions) -> Result<Json> {
    let PageOptions { offset, limit, max_chars } = options;
    if offset < 0 || !(1..=100).contains(&limit) {
        return Err("offset must be nonnegative; limit must be 1..100".to_owned());
    }
    if !(2048..=65536).contains(&max_chars) {
        return Err("max_chars must be 2048..65536".to_owned());
    }
    let (offset, limit, max_chars) = (offset as usize, limit as usize, max_chars as usize);
    if offset > rows.len() {
        return Err("offset exceeds result count".to_owned());
    }
    let mut result = Object::new();
    result.insert("schema".to_owned(), Json::from(PAGE_SCHEMA));
    for (key, item) in header {
        result.insert(key.to_owned(), item);
    }
    result.insert("offset".to_owned(), Json::from(offset as i64));
    result.insert("total".to_owned(), Json::from(rows.len() as i64));
    result.insert("items".to_owned(), Json::Array(Vec::new()));
    result.insert("next_offset".to_owned(), Json::Null);
    result.insert("omitted".to_owned(), Json::from((rows.len() - offset) as i64));
    result.insert("page_complete".to_owned(), Json::Bool(false));
    result.insert("evidence_level".to_owned(), Json::from("source_declarations_only"));
    if to_json(&Json::Object(result.clone())).len() > max_chars.saturating_sub(600) {
        return Err("identity/header exceeds output budget; increase max_chars".to_owned());
    }
    let mut items: Vec<Json> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(offset).take(limit) {
        let mut trial = result.clone();
        let mut trial_items = items.clone();
        trial_items.push(row.clone());
        trial.insert("items".to_owned(), Json::Array(trial_items));
        let mut row = row.clone();
        if to_json(&Json::Object(trial)).len() > max_chars.saturating_sub(300) {
            if !items.is_empty() {
                break;
            }
            row = Json::Object(Object::from([
                ("record_index".to_owned(), Json::from(i as i64)),
                ("needs_source".to_owned(), Json::Bool(true)),
                ("record_chars".to_owned(), Json::from(to_json(&row).len() as i64)),
                ("type".to_owned(), row_get(&row, "type").cloned().unwrap_or(Json::from("entry"))),
                ("line".to_owned(), row_get(&row, "line").cloned().unwrap_or(Json::Null)),
                (
                    "detail".to_owned(),
                    Json::from("record exceeds page budget; inspect its bound source or increase max_chars"),
                ),
            ]));
        }
        items.push(row);
    }
    let consumed = offset + items.len();
    let needs_source = items.iter().any(|row| matches!(row_get(row, "needs_source"), Some(Json::Bool(true))));
    result
        .insert("next_offset".to_owned(), if consumed < rows.len() { Json::from(consumed as i64) } else { Json::Null });
    result.insert("omitted".to_owned(), Json::from((rows.len() - consumed) as i64));
    result.insert("page_complete".to_owned(), Json::Bool(consumed == rows.len() && !needs_source));
    result.insert("items".to_owned(), Json::Array(items));
    Ok(Json::Object(result))
}

fn fold(text: &str) -> String {
    text.to_lowercase()
}

/// `search`: entries matching every term, best first.
pub fn search(catalog: &Catalog, query: &str, kind: Option<&str>, options: PageOptions) -> Result<Json> {
    if kind.is_some_and(|kind| !KINDS.contains(&kind)) {
        return Err("unsupported definition kind".to_owned());
    }
    if query.chars().count() > 512 {
        return Err("query must be at most 512 characters".to_owned());
    }
    let folded = fold(query);
    let terms: Vec<&str> = folded.split(python_isspace).filter(|term| !term.is_empty()).collect();
    let mut rows: Vec<(&Object, i64)> = Vec::new();
    for entry in &catalog.entries {
        let key = format!("{}/{}", entry_text(entry, "provider"), entry_text(entry, "source"));
        let source = &catalog.sources[&key];
        let haystack = fold(&format!(
            "{} {} {}",
            entry_text(entry, "selector"),
            entry_text(entry, "kind"),
            to_json(&value(&source.state))
        ));
        if kind.is_none_or(|kind| entry_text(entry, "kind") == kind) && terms.iter().all(|term| haystack.contains(term))
        {
            let symbol = fold(entry_text(entry, "symbol"));
            let metadata = fold(&to_json(entry.get("metadata").unwrap_or(&Json::Null)));
            let score: i64 = terms
                .iter()
                .map(|t| {
                    if *t == symbol {
                        30
                    } else if symbol.contains(t) {
                        10
                    } else if metadata.contains(t) {
                        5
                    } else {
                        1
                    }
                })
                .sum();
            rows.push((entry, score));
        }
    }
    rows.sort_by(|(a, sa), (b, sb)| {
        (-sa, entry_text(a, "selector"), entry_text(a, "kind"), entry_text(a, "source")).cmp(&(
            -sb,
            entry_text(b, "selector"),
            entry_text(b, "kind"),
            entry_text(b, "source"),
        ))
    });
    page(
        vec![
            ("action", Json::from("search")),
            ("query", Json::from(query)),
            ("discovery_complete", Json::Bool(catalog.complete)),
            ("diagnostic_count", Json::from(catalog.diagnostics.len() as i64)),
            (
                "next_step",
                Json::from("library inspect SELECTOR --kind KIND; library diagnostics for discovery failures"),
            ),
        ],
        rows.into_iter().map(|(entry, _)| Json::Object(entry.clone())).collect(),
        options,
    )
}

fn record(declaration: &StateValue, index: usize) -> Json {
    let mut row = Object::new();
    row.insert("record_index".to_owned(), Json::from(index as i64));
    let name = class_name(declaration);
    row.insert("type".to_owned(), Json::from(name.strip_suffix("Decl").unwrap_or(name)));
    row.insert("line".to_owned(), Json::from(declaration_line(declaration)));
    if let Json::Object(fields) = value(declaration) {
        row.extend(fields);
    }
    Json::Object(row)
}

fn is_macro_import(path: &str) -> bool {
    let base = path.split('?').next().unwrap_or("").split('#').next().unwrap_or("");
    base.to_lowercase().ends_with(".json")
}

/// `inspect`: one definition's section, paginated.
#[allow(clippy::too_many_arguments)]
pub fn inspect(
    catalog: &mut Catalog,
    selector: &str,
    kind: Option<&str>,
    section: &str,
    options: PageOptions,
    expected_digest: Option<&str>,
    members: &[String],
) -> Result<Json> {
    if !SECTIONS.contains(&section) {
        return Err("unsupported inspection section".to_owned());
    }
    if !members.is_empty() && (section == "source" || section == "notes") {
        return Err("member selection applies to declarations, not source/notes".to_owned());
    }
    if members.len() > 32 || members.iter().any(|m| m.is_empty() || m.chars().count() > 128) {
        return Err("members must be 1..128 character names; at most 32 selections".to_owned());
    }
    let matches: Vec<&Object> = catalog
        .entries
        .iter()
        .filter(|e| entry_text(e, "selector") == selector && kind.is_none_or(|kind| entry_text(e, "kind") == kind))
        .collect();
    if matches.is_empty() {
        return Err("definition not found; use library search with its provider/package".to_owned());
    }
    if matches.len() != 1 {
        return Err("ambiguous definition; supply --kind or repair duplicate exports".to_owned());
    }
    let entry = matches[0].clone();
    if expected_digest.is_some_and(|digest| digest != entry_text(&entry, "content_digest")) {
        return Err("library content changed since selection; search again before inspection".to_owned());
    }
    let key = format!("{}/{}", entry_text(&entry, "provider"), entry_text(&entry, "source"));
    let (declarations, document_libraries, imports, text) = {
        let source = &catalog.sources[&key];
        (
            source.declarations.clone(),
            source.document.libraries.clone(),
            source.document.imports.clone(),
            source.text.clone(),
        )
    };
    let mut rows: Vec<Json> = Vec::new();
    match section {
        "source" => {
            rows = python_splitlines(&text)
                .into_iter()
                .enumerate()
                .map(|(n, line)| {
                    Json::Object(Object::from([
                        ("type".to_owned(), Json::from("SourceLine")),
                        ("line".to_owned(), Json::from((n + 1) as i64)),
                        ("text".to_owned(), Json::from(line)),
                    ]))
                })
                .collect();
        }
        "dependencies" => {
            rows = document_libraries
                .iter()
                .map(|name| {
                    Json::Object(Object::from([
                        ("type".to_owned(), Json::from("BuiltinLibrary")),
                        ("name".to_owned(), Json::from(name.as_str())),
                    ]))
                })
                .collect();
            for declaration in &imports {
                let state = StateValue::Dataclass {
                    name: "pcbir.syntax:ImportDecl".to_owned(),
                    fields: IndexMap::from([
                        ("location".to_owned(), StateValue::Null),
                        ("alias".to_owned(), StateValue::Str(declaration.alias.clone())),
                        ("path".to_owned(), StateValue::Str(declaration.path.clone())),
                    ]),
                };
                let Json::Object(mut row) = record(&state, rows.len()) else { unreachable!() };
                row.insert("line".to_owned(), Json::from(declaration.location.line as i64));
                let resolver = catalog.resolver.as_mut();
                let outcome: std::result::Result<Vec<(&str, Json)>, CopperScriptError> = (|| {
                    let Some(resolver) = resolver else {
                        return Ok(vec![
                            ("status", Json::from("not_resolved")),
                            ("detail", Json::from("inspect through a consuming project's declared dependencies")),
                        ]);
                    };
                    if is_macro_import(&declaration.path) {
                        let identity = resolver.qualify_macro_asset(&declaration.path, &declaration.location)?;
                        if identity.starts_with("github.com/") || identity.starts_with("gitlab.com/") {
                            let asset = resolver.resolve_asset(&identity, &declaration.location)?;
                            return Ok(vec![
                                ("status", Json::from("resolved")),
                                ("asset", Json::from(asset.identity.as_str())),
                                ("provider", Json::from(asset.module_path.as_str())),
                                ("version", Json::from(asset.version.as_str())),
                                ("content_digest", Json::from(asset.checksum.as_str())),
                                ("asset_sha256", Json::from(asset.sha256.as_str())),
                            ]);
                        }
                        return Ok(vec![
                            ("status", Json::from("local_input")),
                            ("asset", Json::from(identity.as_str())),
                        ]);
                    }
                    let resolved = resolver.resolve(&declaration.path, &declaration.location)?;
                    Ok(vec![
                        ("status", Json::from("resolved")),
                        ("package", Json::from(resolved.import_path.as_str())),
                        ("provider", Json::from(resolved.module_path.as_str())),
                        ("version", Json::from(resolved.version.as_str())),
                        ("content_digest", Json::from(resolved.checksum.as_str())),
                    ])
                })();
                match outcome {
                    Ok(fields) => row.extend(fields.into_iter().map(|(k, v)| (k.to_owned(), v))),
                    Err(error) => {
                        row.insert("status".to_owned(), Json::from("blocked"));
                        row.insert("code".to_owned(), Json::from(error.code.as_str()));
                        row.insert("detail".to_owned(), Json::from(chars(&error.to_string(), 300).as_str()));
                    }
                }
                rows.push(Json::Object(row));
            }
            for declaration in &declarations {
                if class_name(declaration) == "PartPropertyDecl" && state_text(field(declaration, "name")) == "device" {
                    let name = field(declaration, "value").map(value).map(|v| py_str(&v)).unwrap_or_default();
                    rows.push(Json::Object(Object::from([
                        ("type".to_owned(), Json::from("DeviceContract")),
                        ("name".to_owned(), Json::from(name.as_str())),
                        (
                            "selector".to_owned(),
                            Json::from(format!("{}::{name}", entry_text(&entry, "package")).as_str()),
                        ),
                        (
                            "detail".to_owned(),
                            Json::from("inspect the selected device contract; inheritance is not expanded here"),
                        ),
                    ])));
                }
            }
        }
        "notes" => rows = notes(&text),
        _ => {
            if section == "overview" {
                rows.extend(notes(&text));
            }
            for (i, declaration) in declarations.iter().enumerate() {
                let t = class_name(declaration);
                let property = PROPERTY_DECLS.contains(&t);
                let selected = section == "declarations"
                    || (section == "interfaces" && INTERFACES.contains(&t))
                    || (section == "requirements" && is_requirement(declaration))
                    || (section == "composition" && COMPOSITION.contains(&t))
                    || (section == "properties" && property)
                    || (section == "overview"
                        && (INTERFACES.contains(&t)
                            || is_requirement(declaration)
                            || (property
                                && CORE_PROPERTIES.contains(&state_text(field(declaration, "name")).as_str()))));
                if selected {
                    rows.push(record(declaration, i));
                }
            }
            if section == "overview" {
                let priority = |row: &Json| -> u8 {
                    let row_type = row_get(row, "type").map(py_str).unwrap_or_default();
                    if row_type == "SourceNote" {
                        return 0;
                    }
                    let index = match row_get(row, "record_index") {
                        Some(Json::Int(index)) => usize::try_from(index).unwrap_or(0),
                        _ => 0,
                    };
                    if is_requirement(&declarations[index]) {
                        return 1;
                    }
                    if row_type == "PartProperty" || row_type == "DeviceProperty" { 2 } else { 3 }
                };
                rows.sort_by_key(priority);
            }
        }
    }
    let comments = notes(&text);
    let section_total = rows.len();
    let mut unmatched: IndexSet<&String> = members.iter().collect();
    if !members.is_empty() {
        let requested: IndexMap<String, &String> = members.iter().map(|m| (fold(m), m)).collect();
        let mut filtered = Vec::new();
        for row in rows {
            let mut names: IndexSet<String> = IndexSet::new();
            for key in ["name", "ref", "kind"] {
                if let Some(item) = row_get(&row, key) {
                    names.insert(fold(&py_str(item)));
                }
            }
            if let Some(Json::Array(targets)) = row_get(&row, "targets") {
                names.extend(targets.iter().map(|t| fold(&py_str(t))));
            }
            if let Some(Json::Object(attributes)) = row_get(&row, "attributes")
                && let Some(decouples) = attributes.get("decouples")
            {
                names.insert(fold(&py_str(decouples)));
            }
            let matching: Vec<&String> = names.iter().filter(|n| requested.contains_key(*n)).collect();
            if !matching.is_empty() {
                unmatched.retain(|m| !matching.contains(&&fold(m)));
                filtered.push(row);
            }
        }
        rows = filtered;
    }
    let identity: Object = entry
        .iter()
        .filter(|(k, _)| {
            !["metadata", "metadata_is_preview", "capabilities", "capabilities_omitted", "counts"].contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut unmatched: Vec<&str> = unmatched.into_iter().map(String::as_str).collect();
    unmatched.sort_unstable();
    let unselected = section_total - rows.len();
    page(
        vec![
            ("action", Json::from("inspect")),
            ("identity", Json::Object(identity)),
            ("section", Json::from(section)),
            ("available_sections", Json::Array(SECTIONS.iter().map(|s| Json::from(*s)).collect())),
            ("comment_count", Json::from(comments.len() as i64)),
            ("section_total", Json::from(section_total as i64)),
            ("unselected", Json::from(unselected as i64)),
            ("requested_members", Json::Array(members.iter().map(|m| Json::from(m.as_str())).collect())),
            ("unmatched_members", Json::Array(unmatched.into_iter().map(Json::from).collect())),
            ("view_scope", Json::from("selected source definition; inherited/imported definitions are not expanded")),
            (
                "next_step",
                Json::from(
                    "read requirements, properties and notes; declarations/source preserve all declared data; dependencies identifies imported contracts",
                ),
            ),
        ],
        rows,
        options,
    )
}

/// `diagnostics`: the discovery failures, paginated.
pub fn diagnostics(catalog: &Catalog, options: PageOptions) -> Result<Json> {
    page(
        vec![("action", Json::from("diagnostics")), ("discovery_complete", Json::Bool(catalog.complete))],
        catalog.diagnostics.clone(),
        options,
    )
}

/// `write_index`: a new index file outside the indexed roots.
pub fn write_index(catalog: &Catalog, output: &Path) -> Result<()> {
    let resolved = paths::resolve(output);
    if catalog.roots.iter().any(|root| paths::is_relative_to(&resolved, root)) {
        return Err("index output must stay outside indexed library roots".to_owned());
    }
    if output.exists() {
        return Err("index output must be a new file".to_owned());
    }
    let os = |e: std::io::Error| paths::python_os_error(&e, Some(&paths::display(output)));
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(os)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(output).map_err(os)?;
    std::io::Write::write_all(&mut file, format!("{}\n", to_json(&catalog.index())).as_bytes()).map_err(os)
}

/// Python's `str.splitlines()`.
fn python_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        if matches!(
            c,
            '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ) {
            lines.push(&text[start..index]);
            let mut next = index + c.len_utf8();
            if c == '\r' && iter.peek().is_some_and(|(_, n)| *n == '\n') {
                iter.next();
                next += 1;
            }
            start = next;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

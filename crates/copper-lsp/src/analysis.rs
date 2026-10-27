//! Source-derived symbols and compiler-backed editor diagnostics
//! (`pcbir.lsp.analysis`).
//!
//! Only declared imports are traversed. Package resolution remains
//! locked/offline; editor overlays affect analysis, never dependency locks or
//! build artifacts.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use copper_json::{Dumps, Object, Value, python_float_repr, python_isalnum, python_isalpha, python_str_repr};
use copper_lang::compiler::{compile_design_source_with, is_macro_import, package_sources};
use copper_lang::diagnostics::locate;
use copper_lang::elaborate::elaborate;
use copper_lang::erc::{Severity, build_context, check, unmarked_pins};
use copper_lang::lower::{quantity_dimension, quantity_of};
use copper_lang::macros::{MacroError, PreparationError, item_properties, lower_macro, prepare_macro_asset};
use copper_lang::model::{MacroAssetReference, PartDefinition};
use copper_lang::packages::{PackageResolver, SourceReader};
use copper_lang::syntax::{self as ast, Declaration, Document, DocumentKind, Location, MechanicalItem, Scalar};
use copper_lang::{CopperScriptError, TokenKind, library, macros, parse, paths};
use indexmap::IndexMap;

use crate::frontend::{Parsed, parse_recovering};
use crate::pyerr::{PyError, PyResult, get, item, iterate, truthy};
use crate::text::{PathKey, Text, as_uri, int, object, path_from_uri, strip};

pub const MAX_TEXT: usize = 8 * 1024 * 1024;
const MAX_IMPORT_DEPTH: usize = 64;
const KEYWORDS: [&str; 16] = [
    "component",
    "module",
    "net",
    "no_connect",
    "supply",
    "interface",
    "constraint",
    "configure",
    "power_state",
    "import",
    "use",
    "port",
    "mechanical",
    "silkscreen",
    "physical_macro",
    "macro",
];
const SILKSCREEN_ITEMS: [&str; 8] = ["branding", "text", "logo", "line", "rectangle", "circle", "arc", "polygon"];

/// `PROPERTIES[kind]`.
fn properties(kind: &str) -> &'static [&'static str] {
    match kind {
        "component" => &["value", "footprint", "role", "decouples"],
        "supply" => &["voltage", "net", "source", "external"],
        "no_connect" => &["reason"],
        "pin" => &[
            "number",
            "domains",
            "directions",
            "drive_modes",
            "voltage_min",
            "voltage_max",
            "bond",
            "connection",
            "traits",
        ],
        "branding" => &["enabled", "position", "size", "rotation", "side", "material", "exposed", "layout"],
        "text" => &["text", "position", "size", "thickness", "rotation", "side", "material", "exposed"],
        "logo" => &["position", "size", "rotation", "side", "material", "exposed"],
        "line" => &["start", "end", "thickness", "side", "material", "exposed"],
        "rectangle" => &["start", "end", "thickness", "filled", "side", "material", "exposed"],
        "circle" => &["center", "radius", "thickness", "filled", "side", "material", "exposed"],
        "arc" => &["start", "mid", "end", "thickness", "side", "material", "exposed"],
        "polygon" => &["vertices", "thickness", "filled", "side", "material", "exposed"],
        _ => &[],
    }
}

/// `KINDS`: LSP symbol kinds by syntax class.
fn symbol_kind(class: &str) -> u32 {
    match class {
        "ComponentDecl" => 19,
        "ModuleInstanceDecl" | "PhysicalMacroDecl" => 2,
        "NetDecl" | "SupplyDecl" | "PowerStateDecl" => 13,
        "PortDecl" | "PinDecl" | "PadDecl" => 8,
        "PeripheralDecl" | "ConfigurationDecl" => 6,
        "InterfaceDecl" => 11,
        "ConstraintDecl" => 14,
        _ => 7,
    }
}

/// `str(value)` of a quantity: the shared formatter, or the raw text for an unknown unit.
fn quantity_text(value: &str, unit: &str) -> String {
    match quantity_dimension(unit) {
        Some(dimension) => quantity_of(dimension, value, unit).to_string(),
        None => format!("{value} {unit}"),
    }
}

fn scalar_repr(value: &Scalar) -> String {
    copper_lang::pyvalue::scalar_repr(value)
}

/// `repr()` of a parsed value (tuples, dicts and dataclasses as Python prints them).
fn value_repr(value: &ast::Value) -> String {
    let tuple = |items: Vec<String>| {
        if items.len() == 1 { format!("({},)", items[0]) } else { format!("({})", items.join(", ")) }
    };
    match value {
        ast::Value::Scalar(scalar) => scalar_repr(scalar),
        ast::Value::Point(x, y) => tuple(vec![scalar_repr(x), scalar_repr(y)]),
        ast::Value::Points(points) => {
            tuple(points.iter().map(|(x, y)| tuple(vec![scalar_repr(x), scalar_repr(y)])).collect())
        }
        ast::Value::List(items) => tuple(items.iter().map(value_repr).collect()),
        ast::Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(key, value)| format!("{}: {}", python_str_repr(key), value_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ast::Value::PadRef(reference) => format!(
            "MacroPadReference(member={}, pad={})",
            python_str_repr(&reference.member),
            python_str_repr(&reference.pad)
        ),
    }
}

/// `scalar(value)` for an assignment value.
fn scalar(value: &Scalar) -> String {
    match value {
        Scalar::Quantity { value, unit } => quantity_text(value, unit),
        Scalar::Bool(flag) => if *flag { "true" } else { "false" }.to_owned(),
        Scalar::Str(text) => text.clone(),
        Scalar::Int(number) => number.to_string(),
        Scalar::Float(number) => python_float_repr(*number),
    }
}

/// `scalar(value)` for a geometry or macro value.
fn scalar_value(value: &ast::Value) -> String {
    let list = |items: Vec<String>| format!("[{}]", items.join(", "));
    match value {
        ast::Value::Scalar(item) => scalar(item),
        ast::Value::PadRef(reference) => format!("{}.\"{}\"", reference.member, reference.pad),
        ast::Value::Point(x, y) => list(vec![scalar(x), scalar(y)]),
        ast::Value::Points(points) => list(points.iter().map(|(x, y)| list(vec![scalar(x), scalar(y)])).collect()),
        ast::Value::List(items) => list(items.iter().map(scalar_value).collect()),
        ast::Value::Map(_) => value_repr(value),
    }
}

/// `str.splitlines()`.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        let breaks = matches!(
            c,
            '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..index]);
        start = index + c.len_utf8();
        if c == '\r' && iter.peek().is_some_and(|(_, next)| *next == '\n') {
            iter.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// What the editor knows about one syntax node.
struct Node<'a> {
    class: &'static str,
    /// `node.ref`, else `node.name`, else `node.kind`, else `""`.
    name: String,
    location: &'a Location,
    /// `(label, value)` for the description labels the node has.
    labels: Vec<(&'static str, String)>,
    fields: Vec<(String, String)>,
    endpoints: Option<&'a [String]>,
    value: Option<String>,
    mappings: Vec<String>,
}

fn attributes(fields: &ast::Attributes) -> Vec<(String, String)> {
    fields.iter().map(|(key, value)| (key.clone(), scalar(value))).collect()
}

fn parameters(fields: &ast::Parameters) -> Vec<(String, String)> {
    fields.iter().map(|(key, value)| (key.clone(), scalar_value(value))).collect()
}

fn node(declaration: &Declaration) -> Node<'_> {
    fn base<'a>(class: &'static str, name: &str, location: &'a Location) -> Node<'a> {
        Node {
            class,
            name: name.to_owned(),
            location,
            labels: Vec::new(),
            fields: Vec::new(),
            endpoints: None,
            value: None,
            mappings: Vec::new(),
        }
    }
    match declaration {
        Declaration::Component(item) => Node {
            labels: vec![("part", item.part.clone())],
            fields: attributes(&item.attributes),
            ..base("ComponentDecl", &item.reference, &item.location)
        },
        Declaration::Port(item) => {
            Node { labels: vec![("pin_type", item.pin_type.clone())], ..base("PortDecl", &item.name, &item.location) }
        }
        Declaration::Pin(block) => {
            Node { fields: attributes(&block.attributes), ..base("PinDecl", &block.name, &block.location) }
        }
        Declaration::Pad(block) => {
            Node { fields: attributes(&block.attributes), ..base("PadDecl", &block.name, &block.location) }
        }
        Declaration::PowerDomain(block) => {
            Node { fields: attributes(&block.attributes), ..base("PowerDomainDecl", &block.name, &block.location) }
        }
        Declaration::ModeGroup(block) => {
            Node { fields: attributes(&block.attributes), ..base("ModeGroupDecl", &block.name, &block.location) }
        }
        Declaration::PadSet(block) => {
            Node { fields: attributes(&block.attributes), ..base("PadSetDecl", &block.name, &block.location) }
        }
        Declaration::Supply(block) => {
            Node { fields: attributes(&block.attributes), ..base("SupplyDecl", &block.name, &block.location) }
        }
        Declaration::PowerState(item) => base("PowerStateDecl", &item.name, &item.location),
        Declaration::PartProperty(item) => {
            Node { value: Some(scalar(&item.value)), ..base("PartPropertyDecl", &item.name, &item.location) }
        }
        Declaration::DeviceProperty(item) => {
            Node { value: Some(scalar(&item.value)), ..base("DevicePropertyDecl", &item.name, &item.location) }
        }
        Declaration::Peripheral(item) => {
            Node { labels: vec![("kind", item.kind.clone())], ..base("PeripheralDecl", &item.name, &item.location) }
        }
        Declaration::Mux(item) => base("MuxDecl", "", &item.location),
        Declaration::Resource(item) => base("ResourceDecl", &item.name, &item.location),
        Declaration::Unit(item) => {
            Node { labels: vec![("kind", item.kind.clone())], ..base("UnitDecl", &item.name, &item.location) }
        }
        Declaration::SignalGroup(item) => Node {
            labels: vec![("kind", item.kind.clone())],
            fields: attributes(&item.attributes),
            ..base("SignalGroupDecl", &item.name, &item.location)
        },
        Declaration::RouteRule(item) => base("RouteRuleDecl", "", &item.location),
        Declaration::Configuration(item) => base("ConfigurationDecl", &item.name, &item.location),
        Declaration::ModuleInstance(item) => Node {
            labels: vec![("module", item.module.clone())],
            ..base("ModuleInstanceDecl", &item.reference, &item.location)
        },
        Declaration::PhysicalMacro(item) => Node {
            labels: vec![("asset_alias", item.asset_alias.clone()), ("scope", item.scope.clone())],
            mappings: item.mappings.iter().map(|m| format!("{} {} = {}", m.kind, m.key, m.target)).collect(),
            ..base("PhysicalMacroDecl", &item.name, &item.location)
        },
        Declaration::MacroProperty(item) => {
            Node { value: Some(scalar_value(&item.value)), ..base("MacroPropertyDecl", &item.name, &item.location) }
        }
        Declaration::MacroItem(item) => Node {
            labels: vec![("kind", item.kind.clone())],
            fields: parameters(&item.parameters),
            ..base("MacroItemDecl", &item.name, &item.location)
        },
        Declaration::MacroPad(item) => base("MacroPadDecl", "", &item.location),
        Declaration::Net(item) => {
            Node { endpoints: Some(&item.endpoints), ..base("NetDecl", &item.name, &item.location) }
        }
        Declaration::NoConnect(item) => base("NoConnectDecl", "", &item.location),
        Declaration::Interface(item) => Node {
            labels: vec![("kind", item.kind.clone())],
            fields: attributes(&item.attributes),
            ..base("InterfaceDecl", &item.name, &item.location)
        },
        Declaration::Constraint(item) => Node {
            labels: vec![("kind", item.kind.clone())],
            fields: attributes(&item.parameters),
            ..base("ConstraintDecl", &item.kind, &item.location)
        },
        Declaration::Mechanical(item) => base("MechanicalDecl", "", &item.location),
        Declaration::Silkscreen(item) => base("SilkscreenDecl", "", &item.location),
        Declaration::MechanicalItem(MechanicalItem::Feature(item)) => Node {
            labels: vec![("kind", item.kind.clone())],
            fields: parameters(&item.parameters),
            ..base("MechanicalItemDecl", &item.name, &item.location)
        },
        Declaration::MechanicalItem(MechanicalItem::ProfileUse(item)) => {
            base("MechanicalProfileUseDecl", "", &item.location)
        }
        Declaration::MechanicalItem(MechanicalItem::Stackup(item)) => base("StackupDecl", "", &item.location),
    }
}

/// Which syntax node a symbol stands for, inside its unit's document.
#[derive(Clone, Copy, Debug, PartialEq)]
enum NodeRef {
    Document,
    Import(usize),
    Declaration(usize),
}

#[derive(Clone, Debug)]
struct Symbol {
    name: String,
    unit: usize,
    node: NodeRef,
    start: usize,
    end: usize,
    selection_start: usize,
    selection_end: usize,
    kind: u32,
}

/// A symbol by unit and position in that unit's symbol list.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SymbolRef {
    unit: usize,
    index: usize,
}

struct Unit {
    path: PathBuf,
    namespace: String,
    text: Text,
    parsed: Parsed,
    symbols: Vec<Symbol>,
}

impl Unit {
    fn document(&self) -> Option<&Document> {
        self.parsed.document.as_ref()
    }

    fn class(&self, node: NodeRef) -> &'static str {
        match node {
            NodeRef::Document => "Document",
            NodeRef::Import(_) => "ImportDecl",
            NodeRef::Declaration(index) => self::node(&self.document().expect("indexed").declarations[index]).class,
        }
    }

    fn declaration(&self, node: NodeRef) -> Option<&Declaration> {
        match node {
            NodeRef::Declaration(index) => self.document().map(|document| &document.declarations[index]),
            _ => None,
        }
    }

    fn import(&self, node: NodeRef) -> Option<&ast::ImportDecl> {
        match node {
            NodeRef::Import(index) => self.document().map(|document| &document.imports[index]),
            _ => None,
        }
    }
}

fn symbol_char(token: &copper_lang::Token) -> bool {
    matches!(token.kind, TokenKind::Identifier | TokenKind::Number)
}

/// `index_unit`: the document symbol, then each named declaration.
fn index_unit(unit: &mut Unit, index: usize) {
    let Some(document) = unit.parsed.document.as_ref() else { return };
    let tokens = &unit.parsed.tokens;
    let mut symbols = vec![Symbol {
        name: document.name.clone(),
        unit: index,
        node: NodeRef::Document,
        start: tokens[0].location.offset,
        end: unit.text.len(),
        selection_start: tokens[1].location.offset,
        selection_end: tokens[1].end_offset,
        kind: 2,
    }];
    let mut nodes: Vec<(NodeRef, String, &Location, &'static str)> = document
        .declarations
        .iter()
        .enumerate()
        .map(|(i, declaration)| {
            let info = node(declaration);
            (NodeRef::Declaration(i), info.name, info.location, info.class)
        })
        .collect();
    for (i, import) in document.imports.iter().enumerate() {
        if is_macro_import(&import.path) || document.kind == DocumentKind::Macro {
            nodes.push((NodeRef::Import(i), import.alias.clone(), &import.location, "ImportDecl"));
        }
    }
    for (node, name, location, class) in nodes {
        if name.is_empty() {
            continue;
        }
        let Some(i) = tokens.iter().position(|t| t.location.offset == location.offset) else { continue };
        let mut selection = &tokens[i];
        if matches!(class, "ConstraintDecl" | "ConfigurationDecl" | "ImportDecl" | "MacroItemDecl") {
            selection = tokens[i..].iter().find(|t| t.text == name).unwrap_or(selection);
        }
        let start = if i > 0
            && (KEYWORDS.contains(&tokens[i - 1].text.as_str())
                || ["pin", "pad", "peripheral"].contains(&tokens[i - 1].text.as_str()))
        {
            i - 1
        } else {
            i
        };
        let mut j = i;
        let mut depth: i64 = 0;
        while j < tokens.len() - 1 {
            let token = &tokens[j];
            if token.kind == TokenKind::Symbol {
                if token.text == "{" {
                    depth += 1;
                } else if token.text == "}" {
                    depth -= 1;
                    if depth <= 0 {
                        break;
                    }
                } else if token.text == ";" && depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        symbols.push(Symbol {
            name,
            unit: index,
            node,
            start: tokens[start].location.offset,
            end: tokens[j].end_offset,
            selection_start: selection.location.offset,
            selection_end: selection.end_offset,
            kind: symbol_kind(class),
        });
    }
    unit.symbols = symbols;
}

/// Why one import declaration stopped: a reported diagnostic, or an
/// `OSError`/`ValueError` that only aborts that import.
enum ImportFailure {
    Compile(CopperScriptError),
    Other,
}

impl From<CopperScriptError> for ImportFailure {
    fn from(error: CopperScriptError) -> Self {
        ImportFailure::Compile(error)
    }
}

/// A source reader over a workspace snapshot (open buffers, then disk).
#[derive(Clone)]
pub struct Sources {
    texts: Arc<Mutex<IndexMap<PathKey, String>>>,
}

/// Read a source exactly (`open(encoding='utf-8', newline='')`).
fn read_exact(path: &Path) -> Result<String, PyError> {
    let raw = std::fs::read(path).map_err(|failure| {
        PyError::Os(copper_json::oserror::python_os_error(
            &failure,
            Some(&paths::display(path)),
            copper_json::oserror::Call::Open,
        ))
    })?;
    String::from_utf8(raw).map_err(|failure| {
        let position = failure.utf8_error().valid_up_to();
        let byte = failure.as_bytes()[position];
        let reason = if failure.utf8_error().error_len().is_none() {
            "unexpected end of data"
        } else if (0x80..0xc2).contains(&byte) || byte > 0xf4 {
            "invalid start byte"
        } else {
            "invalid continuation byte"
        };
        PyError::Value(format!("'utf-8' codec can't decode byte 0x{byte:02x} in position {position}: {reason}"))
    })
}

impl Sources {
    pub fn new(documents: impl IntoIterator<Item = (PathKey, String)>) -> Sources {
        Sources { texts: Arc::new(Mutex::new(documents.into_iter().collect())) }
    }

    fn cached(&self, key: &PathKey) -> Option<String> {
        self.texts.lock().expect("sources").get(key).cloned()
    }

    /// The analysed root: its open buffer or its file.
    pub fn root(&self, path: &Path) -> PyResult<String> {
        let key = PathKey::of(path);
        if let Some(text) = self.cached(&key) {
            return Ok(text);
        }
        let text = read_exact(path)?;
        if text.len() > MAX_TEXT {
            return Err(PyError::Value("document exceeds text bounds".to_owned()));
        }
        self.texts.lock().expect("sources").insert(key, text.clone());
        Ok(text)
    }

    /// `read(source_path)` of `Workspace.analysis`.
    pub fn read(&self, path: &Path) -> PyResult<String> {
        let path = paths::resolve(path);
        let key = PathKey::of(&path);
        if let Some(text) = self.cached(&key) {
            return Ok(text);
        }
        let text = read_exact(&path)?;
        if text.len() > MAX_TEXT {
            return Err(PyError::Value("imported source exceeds text bounds".to_owned()));
        }
        self.texts.lock().expect("sources").insert(key, text.clone());
        Ok(text)
    }

    fn reader(&self) -> SourceReader {
        let sources = self.clone();
        Arc::new(move |path: &Path| {
            sources.read(path).map_err(|failure| match failure {
                PyError::Os(_) => std::fs::read(paths::resolve(path)).err().unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, failure.message().to_owned())
                }),
                other => std::io::Error::new(std::io::ErrorKind::InvalidData, other.message().to_owned()),
            })
        })
    }
}

/// The editor's view of one root source and its imports.
pub struct Analysis {
    sources: Sources,
    /// The client's spellings of opened documents, for replies.
    pub uris: IndexMap<PathKey, String>,
    units: Vec<Unit>,
    unit_keys: IndexMap<(PathKey, String), usize>,
    exports: IndexMap<String, Vec<SymbolRef>>,
    import_errors: Vec<CopperScriptError>,
    macro_contracts: IndexMap<(PathKey, String), (PathBuf, Value)>,
    named_parts: IndexMap<String, PartDefinition>,
    resolver: Option<PackageResolver>,
}

const ROOT: usize = 0;

fn locked_resolver(path: &Path) -> Result<PackageResolver, CopperScriptError> {
    PackageResolver::for_source(path, true, true)
}

impl Analysis {
    pub fn new(path: &Path, sources: Sources, uris: IndexMap<PathKey, String>) -> PyResult<Analysis> {
        let mut analysis = Analysis {
            sources,
            uris,
            units: Vec::new(),
            unit_keys: IndexMap::new(),
            exports: IndexMap::new(),
            import_errors: Vec::new(),
            macro_contracts: IndexMap::new(),
            named_parts: IndexMap::new(),
            resolver: None,
        };
        analysis.load(path, "")?;
        let (libraries, has_imports) = match analysis.units[ROOT].document() {
            Some(document) => (document.libraries.clone(), !document.imports.is_empty()),
            None => (Vec::new(), false),
        };
        for name in &libraries {
            if let Some(parts) = library::library(name) {
                analysis.named_parts.extend(parts);
            }
        }
        if has_imports {
            match locked_resolver(path) {
                Ok(resolver) => {
                    let mut resolver = resolver;
                    resolver.source_reader = Some(analysis.sources.reader());
                    analysis.resolver = Some(resolver);
                    analysis.imports(ROOT, &[]);
                }
                Err(failure) => analysis.import_errors.push(failure),
            }
        }
        Ok(analysis)
    }

    fn root(&self) -> &Unit {
        &self.units[ROOT]
    }

    fn uri_for(&self, path: &Path) -> String {
        self.uris.get(&PathKey::of(path)).cloned().unwrap_or_else(|| as_uri(path))
    }

    fn symbol(&self, reference: SymbolRef) -> &Symbol {
        &self.units[reference.unit].symbols[reference.index]
    }

    fn load(&mut self, path: &Path, namespace: &str) -> PyResult<usize> {
        let resolved = paths::resolve(path);
        let key = (PathKey::of(&resolved), namespace.to_owned());
        if let Some(index) = self.unit_keys.get(&key) {
            return Ok(*index);
        }
        let text = self.sources.read(&resolved)?;
        let parsed = parse_recovering(&text, &paths::display(&resolved));
        let index = self.units.len();
        let mut unit = Unit {
            path: resolved,
            namespace: namespace.to_owned(),
            text: Text::new(&text),
            parsed,
            symbols: Vec::new(),
        };
        index_unit(&mut unit, index);
        self.units.push(unit);
        self.unit_keys.insert(key, index);
        Ok(index)
    }

    fn imports(&mut self, unit: usize, stack: &[String]) {
        let Some(document) = self.units[unit].document() else { return };
        if document.kind == DocumentKind::Macro {
            return; // Footprint imports are data assets, never source packages.
        }
        let declarations = document.imports.clone();
        for declaration in &declarations {
            if let Err(ImportFailure::Compile(failure)) = self.import(unit, declaration, stack) {
                self.import_errors.push(failure);
            }
        }
    }

    fn import(&mut self, unit: usize, declaration: &ast::ImportDecl, stack: &[String]) -> Result<(), ImportFailure> {
        let location = &declaration.location;
        let resolver = self.resolver.as_mut().expect("imports run with a resolver");
        if is_macro_import(&declaration.path) {
            let identity = resolver.qualify_macro_asset(&declaration.path, location)?;
            let reference = MacroAssetReference {
                asset: identity,
                base_directory: paths::display(&resolver.source_directory),
                location: location.clone(),
                locked: true,
                offline: true,
                symbol: String::new(),
            };
            let (path, _, data) =
                prepare_macro_asset(&reference, true, true, None).map_err(|failure| match failure {
                    MacroError::Compile(error) => error,
                    MacroError::Value(message) => CopperScriptError::new(
                        "MAC005",
                        format!("macro contract unavailable: {message}"),
                        location.clone(),
                    ),
                })?;
            let key = (PathKey::of(&self.units[unit].path), declaration.alias.clone());
            self.macro_contracts.insert(key, (path, data));
            return Ok(());
        }
        let resolved = resolver.resolve(&declaration.path, location)?;
        if stack.contains(&resolved.import_path) || stack.len() >= MAX_IMPORT_DEPTH {
            return Err(CopperScriptError::new(
                "CMP021",
                "cyclic or excessively deep package import",
                location.clone(),
            )
            .into());
        }
        let parent = &self.units[unit].namespace;
        let namespace =
            if parent.is_empty() { declaration.alias.clone() } else { format!("{parent}.{}", declaration.alias) };
        let mut nested = stack.to_vec();
        nested.push(resolved.import_path.clone());
        for path in package_sources(&resolved.directory) {
            let child = self.load(&path, &namespace).map_err(|_| ImportFailure::Other)?;
            let Some(document) = self.units[child].document() else { continue };
            let name = document.name.clone();
            let is_macro = document.kind == DocumentKind::Macro;
            self.exports.entry(format!("{namespace}.{name}")).or_default().push(SymbolRef { unit: child, index: 0 });
            if is_macro {
                // Index source roles for completion without requiring footprint
                // inventories for unselected variants.
                let resolver = self.resolver.as_ref().expect("imports run with a resolver");
                let document = self.units[child].document().expect("parsed");
                let (data, _) = lower_macro(document, resolver, false).map_err(|failure| match failure {
                    MacroError::Compile(error) => ImportFailure::Compile(error),
                    MacroError::Value(_) => ImportFailure::Other,
                })?;
                let key = (PathKey::of(&self.units[unit].path), format!("{}.{name}", declaration.alias));
                self.macro_contracts.insert(key, (path.clone(), data));
            }
            self.imports(child, &nested);
        }
        Ok(())
    }

    fn exported(&self, unit: usize, name: &str) -> Option<SymbolRef> {
        let namespace = &self.units[unit].namespace;
        let key = if namespace.is_empty() {
            name.to_owned()
        } else if name.is_empty() {
            namespace.clone()
        } else {
            format!("{namespace}.{name}")
        };
        let candidates = self.exports.get(&key)?;
        // Duplicate definitions never produce arbitrary navigation.
        let mut unique: IndexMap<(PathKey, usize), SymbolRef> = IndexMap::new();
        for candidate in candidates {
            let symbol = self.symbol(*candidate);
            unique.insert((PathKey::of(&self.units[symbol.unit].path), symbol.selection_start), *candidate);
        }
        if unique.len() == 1 { unique.values().next().copied() } else { None }
    }

    fn description(&self, reference: SymbolRef) -> String {
        let symbol = self.symbol(reference);
        let unit = &self.units[symbol.unit];
        let mut lines = Vec::new();
        match symbol.node {
            NodeRef::Document => {
                let document = unit.document().expect("indexed");
                return format!("{} {}", document.kind.as_str(), document.name);
            }
            NodeRef::Import(_) => {
                let import = unit.import(symbol.node).expect("indexed");
                lines.push(format!("Import: {}", symbol.name));
                lines.push(format!("path: {}", import.path));
            }
            NodeRef::Declaration(_) => {
                let info = node(unit.declaration(symbol.node).expect("indexed"));
                lines.push(format!("{}: {}", info.class.strip_suffix("Decl").unwrap_or(info.class), symbol.name));
                for (label, value) in &info.labels {
                    lines.push(format!("{label}: {value}"));
                }
                for (key, value) in &info.fields {
                    lines.push(format!("{key}: {value}"));
                }
                if let Some(endpoints) = info.endpoints {
                    lines.push(format!("endpoints: {}", endpoints.join(", ")));
                }
                if let Some(value) = info.value {
                    lines.push(value);
                }
                lines.extend(info.mappings);
            }
        }
        // Preceding source comments are library documentation, not instructions.
        let before = unit.text.slice(0, symbol.start);
        let mut notes = Vec::new();
        for line in splitlines(&before).into_iter().rev() {
            let line = strip(line);
            if let Some(rest) = line.strip_prefix("//") {
                notes.push(strip(rest).to_owned());
            } else if let Some(rest) = line.strip_prefix('#') {
                notes.push(strip(rest).to_owned());
            } else if line.is_empty() && notes.is_empty() {
                continue;
            } else {
                break;
            }
        }
        lines.extend(notes.into_iter().rev());
        lines.join("\n")
    }

    fn location(&self, reference: SymbolRef) -> Value {
        let symbol = self.symbol(reference);
        let unit = &self.units[symbol.unit];
        object([
            ("uri", Value::Str(self.uri_for(&unit.path))),
            ("range", unit.text.range(symbol.selection_start, symbol.selection_end)),
        ])
    }

    fn outline(&self, reference: SymbolRef) -> Value {
        let symbol = self.symbol(reference);
        let unit = &self.units[symbol.unit];
        object([
            ("name", Value::Str(symbol.name.clone())),
            ("kind", int(symbol.kind as usize)),
            ("range", unit.text.range(symbol.start, symbol.end)),
            ("selectionRange", unit.text.range(symbol.selection_start, symbol.selection_end)),
        ])
    }

    /// `[s.outline() for s in analysis.root.symbols[1:]]`.
    pub fn document_symbols(&self) -> Value {
        Value::Array(
            (1..self.root().symbols.len()).map(|index| self.outline(SymbolRef { unit: ROOT, index })).collect(),
        )
    }

    /// The module definition a module instance symbol names, through exports.
    fn instance_target(&self, unit: usize, symbol: Option<SymbolRef>) -> Option<SymbolRef> {
        let symbol = symbol?;
        let found = self.symbol(symbol);
        match self.units[found.unit].declaration(found.node) {
            Some(Declaration::ModuleInstance(instance)) => self.exported(unit, &instance.module),
            _ => None,
        }
    }

    fn members(&self, reference: SymbolRef) -> IndexMap<String, SymbolRef> {
        let symbol = self.symbol(reference);
        let unit = &self.units[symbol.unit];
        let target = match (symbol.node, unit.declaration(symbol.node)) {
            (_, Some(Declaration::Component(component))) => self.exported(symbol.unit, &component.part),
            (_, Some(Declaration::ModuleInstance(instance))) => self.exported(symbol.unit, &instance.module),
            (NodeRef::Document, _) => Some(reference),
            _ => None,
        };
        let Some(target) = target else { return IndexMap::new() };
        let target_unit = &self.units[self.symbol(target).unit];
        let kind = target_unit.document().expect("indexed").kind;
        let wanted: &[&str] = match kind {
            DocumentKind::Part => &["PinDecl"],
            DocumentKind::Module => &["PortDecl"],
            DocumentKind::Device => &["PadDecl", "PeripheralDecl"],
            _ => &[],
        };
        let mut members = IndexMap::new();
        for (index, symbol) in target_unit.symbols.iter().enumerate().skip(1) {
            if wanted.contains(&target_unit.class(symbol.node)) {
                members.insert(symbol.name.clone(), SymbolRef { unit: symbol.unit, index });
            }
        }
        members
    }

    fn resolve(&self, unit: usize, name: &str) -> Option<SymbolRef> {
        if let Some((owner, tail)) = name.split_once('/') {
            let symbol = self.resolve(unit, owner);
            let target = self.instance_target(unit, symbol)?;
            return self.resolve(self.symbol(target).unit, tail);
        }
        let current = &self.units[unit];
        if let Some(port) = name.strip_prefix("port.")
            && current.document().is_some_and(|document| document.kind == DocumentKind::Module)
        {
            return current
                .symbols
                .iter()
                .enumerate()
                .find(|(_, s)| current.class(s.node) == "PortDecl" && s.name == port)
                .map(|(index, _)| SymbolRef { unit, index });
        }
        let mut local: Vec<(usize, &Symbol)> =
            current.symbols.iter().enumerate().filter(|(_, s)| s.name == name).collect();
        if !local.is_empty() {
            // Supply and net names may intentionally coincide; prefer the net.
            local.sort_by_key(|(_, s)| current.class(s.node) != "NetDecl");
            return Some(SymbolRef { unit, index: local[0].0 });
        }
        if let Some(exported) = self.exported(unit, name) {
            return Some(exported);
        }
        if let Some((owner, member)) = name.rsplit_once('.') {
            let target = self.resolve(unit, owner)?;
            return self.members(target).get(member).copied();
        }
        None
    }

    fn word(&self, offset: usize) -> Option<(SymbolRef, Value)> {
        let root = self.root();
        let tokens = &root.parsed.tokens;
        let i = tokens.iter().position(|t| symbol_char(t) && t.location.offset <= offset && offset < t.end_offset)?;
        let mut start = i;
        while start >= 2 && [".", "/"].contains(&tokens[start - 1].text.as_str()) && symbol_char(&tokens[start - 2]) {
            start -= 2;
        }
        // Resolve the prefix ending at the hovered token, rather than a later member.
        let mut name: String = tokens[start..=i].iter().map(|t| t.text.as_str()).collect();
        for candidate in &root.symbols {
            let Some(Declaration::PhysicalMacro(declaration)) = root.declaration(candidate.node) else { continue };
            if declaration.scope.is_empty() {
                continue;
            }
            if candidate.start <= offset && offset <= candidate.end {
                let preceding: Vec<&str> = tokens
                    .iter()
                    .filter(|t| {
                        candidate.start <= t.location.offset && t.location.offset < tokens[start].location.offset
                    })
                    .map(|t| t.text.as_str())
                    .collect();
                let last = preceding.iter().rposition(|text| *text == ";").map(|j| j as i64).unwrap_or(-1);
                if preceding[(last + 1) as usize..].contains(&"=") {
                    name = format!("{}/{name}", declaration.scope);
                }
                break;
            }
        }
        let symbol = self.resolve(ROOT, &name)?;
        Some((symbol, root.text.range(tokens[i].location.offset, tokens[i].end_offset)))
    }

    pub fn hover(&self, position: &Value) -> PyResult<Value> {
        let offset = self.root().text.offset(position)?;
        Ok(match self.word(offset) {
            Some((symbol, range)) => object([
                (
                    "contents",
                    object([("kind", Value::from("plaintext")), ("value", Value::Str(self.description(symbol)))]),
                ),
                ("range", range),
            ]),
            None => Value::Null,
        })
    }

    pub fn definition(&self, position: &Value) -> PyResult<Value> {
        let offset = self.root().text.offset(position)?;
        let Some((symbol, _)) = self.word(offset) else { return Ok(Value::Null) };
        let found = self.symbol(symbol);
        if let Some(import) = self.units[found.unit].import(found.node) {
            let key = (PathKey::of(&self.units[found.unit].path), import.alias.clone());
            if let Some((path, _)) = self.macro_contracts.get(&key) {
                let zero = object([("line", int(0)), ("character", int(0))]);
                return Ok(object([
                    ("uri", Value::Str(self.uri_for(path))),
                    ("range", object([("start", zero.clone()), ("end", zero)])),
                ]));
            }
        }
        Ok(self.location(symbol))
    }

    fn macro_scope_unit(&self, declaration: &[String]) -> Option<usize> {
        let mut unit = Some(ROOT);
        if let Some(on) = declaration.iter().position(|text| text == "on") {
            let scope: String = declaration[on + 1..].concat();
            for instance in scope.split('/') {
                let current = unit.expect("checked below");
                let symbol = self.resolve(current, instance);
                unit = self.instance_target(current, symbol).map(|target| self.symbol(target).unit);
                if unit.is_none() {
                    break;
                }
            }
        }
        unit
    }

    pub fn completion(&self, position: &Value) -> PyResult<Value> {
        let root = self.root();
        let offset = root.text.offset(position)?;
        let mut tokens: Vec<&copper_lang::Token> =
            root.parsed.tokens.iter().filter(|t| t.kind != TokenKind::Eof && t.location.offset < offset).collect();
        // Do not offer identifiers in strings, comments or lexical-error gaps.
        let containing = tokens.iter().find(|t| t.location.offset < offset && offset < t.end_offset);
        if containing.is_some_and(|t| t.kind == TokenKind::String) {
            return Ok(Value::Array(Vec::new()));
        }
        if let Some(last) = tokens.last()
            && last.end_offset < offset
        {
            let gap = root.text.slice(last.end_offset, offset);
            if gap.contains('#') || gap.contains("//") {
                return Ok(Value::Array(Vec::new()));
            }
        }
        let mut prefix = String::new();
        if let Some(last) = tokens.last()
            && symbol_char(last)
            && last.end_offset >= offset
        {
            prefix = root.text.slice(last.location.offset, offset);
            tokens.pop();
        }
        let mut candidates: IndexMap<String, (u32, String)> = IndexMap::new();
        let mut stack: Vec<Vec<String>> = Vec::new();
        let mut header: Vec<String> = Vec::new();
        for token in &tokens {
            if token.kind == TokenKind::Symbol && token.text == "{" {
                stack.push(std::mem::take(&mut header));
            } else if token.kind == TokenKind::Symbol && token.text == "}" {
                stack.pop();
                header = Vec::new();
            } else if token.kind == TokenKind::Symbol && token.text == ";" {
                header = Vec::new();
            } else {
                header.push(token.text.clone());
            }
        }
        if tokens.last().is_some_and(|t| t.text == ".") {
            let mut i = tokens.len() as i64 - 2;
            let mut parts: Vec<&str> = Vec::new();
            while i >= 0 && symbol_char(tokens[i as usize]) {
                parts.insert(0, &tokens[i as usize].text);
                i -= 1;
                if i >= 0 && tokens[i as usize].text == "." {
                    i -= 1;
                } else {
                    break;
                }
            }
            let owner = parts.join(".");
            let unit = match stack.last() {
                Some(declaration) if declaration.first().is_some_and(|word| word == "physical_macro") => {
                    self.macro_scope_unit(declaration)
                }
                _ => Some(ROOT),
            };
            if owner == "port"
                && let Some(unit) = unit
                && self.units[unit].document().is_some_and(|document| document.kind == DocumentKind::Module)
            {
                let current = &self.units[unit];
                for (index, symbol) in current.symbols.iter().enumerate() {
                    if current.class(symbol.node) == "PortDecl" {
                        candidates.insert(symbol.name.clone(), (5, self.description(SymbolRef { unit, index })));
                    }
                }
            }
            let target = unit.and_then(|unit| self.resolve(unit, &owner));
            if let Some(target) = target {
                for (name, symbol) in self.members(target) {
                    candidates.insert(name, (5, self.description(symbol)));
                }
                let found = self.symbol(target);
                if let Some(Declaration::Component(component)) = self.units[found.unit].declaration(found.node)
                    && let Some(part) = self.named_parts.get(&component.part)
                {
                    for (name, pin) in &part.pins {
                        candidates.insert(name.clone(), (5, format!("pin {name}; pad {}", pin.number)));
                    }
                }
            } else {
                let lead = format!("{owner}.");
                for (name, symbols) in &self.exports {
                    if let Some(rest) = name.strip_prefix(&lead)
                        && !rest.contains('.')
                    {
                        candidates.insert(rest.to_owned(), (7, self.description(symbols[0])));
                    }
                }
            }
        } else {
            for index in 1..root.symbols.len() {
                candidates
                    .insert(root.symbols[index].name.clone(), (6, self.description(SymbolRef { unit: ROOT, index })));
            }
            for name in self.named_parts.keys() {
                candidates.insert(name.clone(), (7, format!("part {name}")));
            }
            for (name, symbols) in &self.exports {
                if name.matches('.').count() == 1 {
                    candidates.insert(name.clone(), (7, self.description(symbols[0])));
                }
            }
            for keyword in KEYWORDS {
                candidates.insert(keyword.to_owned(), (14, "CopperScript declaration".to_owned()));
            }
            if let Some(declaration) = stack.last() {
                let kind = declaration.first().map(String::as_str).unwrap_or("");
                for property in properties(kind) {
                    candidates.insert((*property).to_owned(), (10, format!("{kind} property")));
                }
                if kind == "silkscreen" {
                    for item in SILKSCREEN_ITEMS {
                        candidates.insert(item.to_owned(), (14, "Silkscreen declaration".to_owned()));
                    }
                }
                if kind == "physical_macro" {
                    let on = declaration.iter().position(|text| text == "on");
                    let alias_start =
                        declaration.iter().position(|text| text == ":").map(|i| i + 1).unwrap_or(declaration.len());
                    let alias_end = on.unwrap_or(declaration.len());
                    let alias: String = if alias_start < alias_end {
                        declaration[alias_start..alias_end].concat()
                    } else {
                        String::new()
                    };
                    let contract = self.macro_contracts.get(&(PathKey::of(&root.path), alias));
                    for mapping in ["member", "net"] {
                        candidates.insert(mapping.to_owned(), (14, "Physical macro mapping".to_owned()));
                    }
                    let mapping = header.first().map(String::as_str);
                    if let (Some((_, data)), Some(mapping @ ("member" | "net"))) = (contract, mapping)
                        && !header.iter().any(|text| text == "=")
                    {
                        let keys = contract_keys(data, mapping);
                        candidates = keys.into_iter().map(|key| (key, (5, format!("Asset {mapping} role")))).collect();
                    } else if header.iter().any(|text| text == "=")
                        && on.is_some()
                        && let Some(unit) = self.macro_scope_unit(declaration)
                    {
                        let wanted: &[&str] = if header[0] == "member" {
                            &["ComponentDecl", "ModuleInstanceDecl"]
                        } else {
                            &["NetDecl", "ModuleInstanceDecl"]
                        };
                        let current = &self.units[unit];
                        candidates = IndexMap::new();
                        for (index, symbol) in current.symbols.iter().enumerate() {
                            if wanted.contains(&current.class(symbol.node)) {
                                candidates
                                    .insert(symbol.name.clone(), (6, self.description(SymbolRef { unit, index })));
                            }
                        }
                        if header[0] == "net" {
                            for symbol in &current.symbols {
                                if current.class(symbol.node) == "PortDecl" {
                                    candidates.insert(
                                        format!("port.{}", symbol.name),
                                        (5, "Existing module boundary net".to_owned()),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        if root.document().is_some_and(|document| document.kind == DocumentKind::Macro) {
            let kind = stack.last().and_then(|declaration| declaration.first()).map(String::as_str).unwrap_or("");
            if kind == "macro" {
                candidates = macros::ROOT_PROPERTIES
                    .iter()
                    .chain(macros::ITEM_KINDS.iter())
                    .chain(["net", "pad", "isolated", "import"].iter())
                    .filter(|p| **p != "contact")
                    .map(|p| ((*p).to_owned(), (14, "Macro declaration".to_owned())))
                    .collect();
            } else if let Some(names) = item_properties(kind) {
                candidates = names.iter().map(|p| ((*p).to_owned(), (10, "Macro property".to_owned()))).collect();
                if kind == "plane_return" {
                    candidates.insert("contact".to_owned(), (14, "Dedicated pad-to-via contact".to_owned()));
                }
            }
            if header.iter().any(|text| text == "=") {
                candidates = IndexMap::new();
                for index in 1..root.symbols.len() {
                    candidates.insert(
                        root.symbols[index].name.clone(),
                        (6, self.description(SymbolRef { unit: ROOT, index })),
                    );
                }
            }
        }
        let mut names: Vec<(&String, &(u32, String))> = candidates.iter().collect();
        names.sort_by(|a, b| a.0.cmp(b.0));
        let prefix_length = prefix.chars().count();
        let range = root.text.range(offset - prefix_length, offset);
        Ok(Value::Array(
            names
                .into_iter()
                .filter(|(name, _)| name.starts_with(prefix.as_str()))
                .map(|(name, (kind, detail))| {
                    object([
                        ("label", Value::Str(name.clone())),
                        ("kind", int(*kind as usize)),
                        ("detail", Value::Str(detail.clone())),
                        ("textEdit", object([("range", range.clone()), ("newText", Value::Str(name.clone()))])),
                    ])
                })
                .collect(),
        ))
    }

    fn error_diagnostic(&self, error: &CopperScriptError) -> Value {
        let unit_index =
            self.units.iter().position(|unit| paths::display(&unit.path) == *error.location.filename).unwrap_or(ROOT);
        let unit = &self.units[unit_index];
        let offset = error.location.offset;
        let token = unit.parsed.tokens.iter().find(|t| t.location.offset == offset);
        let end = token.map(|t| t.end_offset).unwrap_or_else(|| (offset + 1).min(unit.text.len()));
        let mut diagnostic = Object::new();
        diagnostic.insert("range".to_owned(), unit.text.range(offset, end));
        diagnostic.insert("severity".to_owned(), int(1));
        diagnostic.insert("code".to_owned(), Value::Str(error.code.clone()));
        diagnostic.insert("source".to_owned(), Value::from("CopperScript"));
        let message = match error.hint.as_deref().filter(|hint| !hint.is_empty()) {
            Some(hint) => format!("{} ({hint})", error.message),
            None => error.message.clone(),
        };
        diagnostic.insert("message".to_owned(), Value::Str(message));
        if let Some(replacement) = error.replacement.as_deref().filter(|text| !text.is_empty())
            && token.is_some()
            && unit_index == ROOT
        {
            // Same rule as `copper check --json`: only an exact token span is machine-applicable.
            diagnostic.insert(
                "data".to_owned(),
                object([(
                    "fix",
                    object([
                        ("range", diagnostic["range"].clone()),
                        ("newText", Value::from(replacement)),
                        ("oldText", Value::Str(unit.text.slice(offset, end))),
                        ("title", Value::Str(format!("Replace with {}", python_str_repr(replacement)))),
                    ]),
                )]),
            );
        }
        if unit_index != ROOT {
            // Imported errors belong to this board's import, with the exact origin attached.
            let origin =
                object([("uri", Value::Str(self.uri_for(&unit.path))), ("range", diagnostic["range"].clone())]);
            let root = self.root();
            let declaration = root.document().and_then(|document| {
                document
                    .imports
                    .iter()
                    .find(|d| unit.namespace == d.alias || unit.namespace.starts_with(&format!("{}.", d.alias)))
            });
            let range = match declaration {
                Some(d) => root.text.range(d.location.offset, d.location.offset + d.alias.chars().count()),
                None => root.text.range(0, 0),
            };
            diagnostic.insert("range".to_owned(), range);
            diagnostic.insert(
                "relatedInformation".to_owned(),
                Value::Array(vec![object([("location", origin), ("message", Value::Str(error.message.clone()))])]),
            );
        }
        Value::Object(diagnostic)
    }

    /// Map semantic subjects to real source tokens, without parsing messages.
    fn subject_range(&self, subject: Option<&str>) -> Value {
        let subject = subject.unwrap_or("");
        let root = self.root();
        if let Some(symbol) = self.resolve(ROOT, subject)
            && self.symbol(symbol).unit == ROOT
        {
            return self.location(symbol).get("range").cloned().expect("a location");
        }
        let tokens = &root.parsed.tokens;
        for (i, token) in tokens.iter().enumerate() {
            if !symbol_char(token) {
                continue;
            }
            let mut name = token.text.clone();
            let mut end = i;
            while end + 2 < tokens.len()
                && [".", "/"].contains(&tokens[end + 1].text.as_str())
                && symbol_char(&tokens[end + 2])
            {
                name.push_str(&tokens[end + 1].text);
                name.push_str(&tokens[end + 2].text);
                end += 2;
            }
            if name == subject {
                return root.text.range(token.location.offset, tokens[end].end_offset);
            }
        }
        let owner = subject.split('/').next().unwrap_or("").split('.').next().unwrap_or("");
        match self.resolve(ROOT, owner) {
            Some(symbol) if self.symbol(symbol).unit == ROOT => {
                self.location(symbol).get("range").cloned().expect("a location")
            }
            _ => root.text.range(0, 0),
        }
    }

    /// `Analysis.diagnostics()`; an `Err` is an analysis that could not complete.
    pub fn diagnostics(&mut self) -> Result<Vec<Value>, String> {
        let mut errors: Vec<CopperScriptError> = self.root().parsed.errors.clone();
        for unit in &self.units[1..] {
            errors.extend(unit.parsed.errors.iter().cloned());
        }
        errors.extend(self.import_errors.iter().cloned());
        if !errors.is_empty() {
            return Ok(errors.iter().map(|error| self.error_diagnostic(error)).collect());
        }
        let root = self.root();
        let Some(document) = root.document() else { return Ok(Vec::new()) };
        if document.kind == DocumentKind::Macro {
            let lowered = locked_resolver(&root.path)
                .map_err(MacroError::Compile)
                .and_then(|resolver| lower_macro(document, &resolver, false));
            return Ok(match lowered {
                Ok(_) => Vec::new(),
                Err(MacroError::Compile(error)) => vec![self.error_diagnostic(&error)],
                Err(MacroError::Value(message)) => {
                    vec![self.error_diagnostic(&CopperScriptError::new("MAC010", message, document.location.clone()))]
                }
            });
        }
        if document.kind != DocumentKind::Board {
            return Ok(Vec::new()); // Standalone library files receive syntax diagnostics; boards drive ERC.
        }
        let source = root.text.source.clone();
        let filename = paths::display(&root.path);
        let design = match compile_design_source_with(&source, &filename, true, true, self.resolver.as_mut()) {
            Ok(design) => design,
            Err(error) => return Ok(vec![self.error_diagnostic(&error)]),
        };
        let board = &design.electrical;
        let flat = elaborate(board)?;
        match macros::prepare_macro_bindings_with(&flat, true, true, self.resolver.as_mut()) {
            Ok(_) => {}
            Err(PreparationError::Compile(error)) => return Ok(vec![self.error_diagnostic(&error)]),
            Err(PreparationError::Binding { location, message }) => {
                return Ok(vec![self.error_diagnostic(&CopperScriptError::new("MAC005", message, location))]);
            }
        }
        let findings = check(board, true)?;
        let unmarked = if findings.iter().any(|f| f.code == "UNMARKED_PIN") {
            unmarked_pins(&build_context(flat))
        } else {
            IndexMap::new()
        };
        let root = self.root();
        let mut result = Vec::new();
        // The CLI's locator: honours a finding's origin (net/no_connect) and repeats.
        for (finding, span) in locate(findings, &source, &filename) {
            let mut fix_span = None;
            let range = match &span {
                Some(span) if *span.start.filename == filename => {
                    let start = span.start.offset;
                    let end = root.text.starts[span.end_line - 1] + span.end_column - 1;
                    fix_span = Some((start, end));
                    root.text.range(start, end)
                }
                _ => self.subject_range(finding.subject.as_deref()),
            };
            let mut diagnostic = Object::new();
            diagnostic.insert("range".to_owned(), range.clone());
            diagnostic.insert("severity".to_owned(), int(if finding.severity == Severity::Error { 1 } else { 2 }));
            diagnostic.insert("code".to_owned(), Value::Str(finding.code.clone()));
            diagnostic.insert("source".to_owned(), Value::from("CopperScript"));
            let message = match finding.hint.as_deref().filter(|hint| !hint.is_empty()) {
                Some(hint) => format!("{} ({hint})", finding.message),
                None => finding.message.clone(),
            };
            diagnostic.insert("message".to_owned(), Value::Str(message));
            let mut data = Object::new();
            if let (Some((start, end)), Some(replacement)) = (fix_span, &finding.replacement) {
                let subject = finding.subject.as_deref().map(python_str_repr).unwrap_or_else(|| "None".to_owned());
                data.insert(
                    "fix".to_owned(),
                    object([
                        ("range", range.clone()),
                        ("newText", Value::Str(replacement.clone())),
                        ("oldText", Value::Str(root.text.slice(start, end))),
                        ("title", Value::Str(format!("Replace {subject} with {}", python_str_repr(replacement)))),
                    ]),
                );
            }
            if finding.code == "UNMARKED_PIN"
                && let Some(subject) = finding.subject.as_deref()
                && let Some(pins) = unmarked.get(subject)
                && !pins.is_empty()
                && !subject.contains('/')
                && pins.iter().all(|pin| pin_name(pin))
            {
                data.insert(
                    "no_connect".to_owned(),
                    Value::Array(pins.iter().map(|pin| Value::Str(format!("{subject}.{pin}"))).collect()),
                );
            }
            if !data.is_empty() {
                diagnostic.insert("data".to_owned(), Value::Object(data));
            }
            result.push(Value::Object(diagnostic));
        }
        Ok(result)
    }

    /// Quick fixes: apply a located replacement; declare no_connect for unmarked pins.
    pub fn code_actions(&self, params: &Value) -> PyResult<Value> {
        let uri = item(item(params, "textDocument")?, "uri")?.clone();
        let mut actions = Vec::new();
        let context =
            get(params, "context")?.filter(|value| truthy(value)).cloned().unwrap_or(Value::Object(Object::new()));
        let diagnostics =
            get(&context, "diagnostics")?.filter(|value| truthy(value)).cloned().unwrap_or(Value::Array(Vec::new()));
        for diagnostic in iterate(&diagnostics)? {
            let Value::Object(entries) = &diagnostic else { continue };
            let Some(data @ Value::Object(_)) = entries.get("data") else { continue };
            let fix = get(data, "fix")?;
            // A fix from an older publication is offered only while its range still holds the
            // text it was computed for (the document may have changed since).
            if let Some(fix @ Value::Object(fields)) = fix
                && matches!(fields.get("newText"), Some(Value::Str(_)))
                && same_text(self.range_text(fields.get("range")), fields.get("oldText"))
            {
                let title =
                    fields.get("title").filter(|value| truthy(value)).cloned().unwrap_or(Value::from("Apply fix"));
                let edit = object([("range", item(fix, "range")?.clone()), ("newText", fields["newText"].clone())]);
                actions.push(object([
                    ("title", title),
                    ("kind", Value::from("quickfix")),
                    ("isPreferred", Value::Bool(true)),
                    ("diagnostics", Value::Array(vec![diagnostic.clone()])),
                    ("edit", object([("changes", changes(&uri, edit)?)])),
                ]));
            }
            let pins = get(data, "no_connect")?;
            if let Some(Value::Array(pins)) = pins
                && !pins.is_empty()
                && pins.iter().all(|pin| pin.as_str().is_some_and(endpoint))
            {
                let pins: Vec<&str> = pins.iter().filter_map(Value::as_str).collect();
                let statement = format!("no_connect {} {{ reason = \"TODO: why unused\"; }}", pins.join(", "));
                if let Some(edit) = self.insert_before_board_end(&statement)
                    && self.parses_with(&edit)
                {
                    let names: Vec<&str> =
                        pins.iter().map(|pin| pin.split_once('.').map(|(_, rest)| rest).unwrap_or(pin)).collect();
                    actions.push(object([
                        ("title", Value::Str(format!("Declare no_connect for {}", names.join(", ")))),
                        ("kind", Value::from("quickfix")),
                        ("diagnostics", Value::Array(vec![diagnostic.clone()])),
                        ("edit", object([("changes", changes(&uri, edit)?)])),
                    ]));
                }
            }
        }
        Ok(Value::Array(actions))
    }

    /// `_range_text`: the text a range covers, or `None` for a malformed range.
    fn range_text(&self, range: Option<&Value>) -> Option<String> {
        let range = range?;
        let text = &self.root().text;
        let start = text.offset(item(range, "start").ok()?).ok()?;
        let end = text.offset(item(range, "end").ok()?).ok()?;
        Some(text.slice(start, end))
    }

    /// Offer an insertion only if the edited document still parses (pin names vary by library).
    fn parses_with(&self, edit: &Value) -> bool {
        let text = &self.root().text;
        let start = edit.get("range").and_then(|range| range.get("start")).expect("our own edit");
        let at = text.offset(start).expect("our own position");
        let new_text = edit.get("newText").and_then(Value::as_str).expect("our own edit");
        let source = format!("{}{new_text}{}", text.slice(0, at), text.slice(at, text.len()));
        parse(&source, &paths::display(&self.root().path)).is_ok()
    }

    /// A text edit adding `statement` as the last member of the root board.
    fn insert_before_board_end(&self, statement: &str) -> Option<Value> {
        let root = self.root();
        let tokens: Vec<&copper_lang::Token> = root.parsed.tokens.iter().filter(|t| t.kind != TokenKind::Eof).collect();
        let closing = tokens.iter().rev().find(|t| t.text == "}")?;
        let text = &root.text;
        let line_start = rfind_newline(text, closing.location.offset);
        // Indent like the board's first member (the token after the opening brace).
        let opening = tokens.iter().position(|t| t.text == "{");
        let mut indent = "    ".to_owned();
        if let Some(opening) = opening
            && opening + 1 < tokens.len()
        {
            let member = tokens[opening + 1].location.offset;
            let prefix = text.slice(rfind_newline(text, member), member);
            if !prefix.is_empty() && strip(&prefix).is_empty() {
                indent = prefix;
            }
        }
        let newline = if text.source.contains("\r\n") { "\r\n" } else { "\n" };
        let (at, inserted) = if strip(&text.slice(line_start, closing.location.offset)).is_empty() {
            (line_start, format!("{indent}{statement}{newline}"))
        } else {
            (closing.location.offset, format!("{newline}{indent}{statement}{newline}"))
        };
        let position = text.position(at);
        Some(object([
            ("range", object([("start", position.clone()), ("end", position)])),
            ("newText", Value::Str(inserted)),
        ]))
    }
}

/// `current == old` for a covered text (`None` when the range is malformed)
/// and a client value (`None` when absent or null).
fn same_text(current: Option<String>, old: Option<&Value>) -> bool {
    match (current, old.filter(|value| !matches!(value, Value::Null))) {
        (None, None) => true,
        (Some(text), Some(Value::Str(old))) => text == *old,
        _ => false,
    }
}

/// `source.rfind('\n', 0, end) + 1`.
fn rfind_newline(text: &Text, end: usize) -> usize {
    text.chars[..end.min(text.len())].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0)
}

/// `{uri: [edit]}`: a dict keyed by the client's URI value.
fn changes(uri: &Value, edit: Value) -> PyResult<Value> {
    let key = match uri {
        Value::Str(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => {
            return Err(PyError::Type(format!("unhashable type: '{}'", crate::pyerr::type_name(uri))));
        }
        other => Dumps::compact().dumps(other),
    };
    Ok(Value::Object(Object::from([(key, Value::Array(vec![edit]))])))
}

/// The data contract roles a physical macro mapping may name, quoted when not
/// a plain name.
fn contract_keys(data: &Value, mapping: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let text = |value: &Value| match value {
        Value::Str(text) => text.clone(),
        other => Dumps::compact().dumps(other),
    };
    if mapping == "member" {
        for row in data.get("members").and_then(Value::as_array).unwrap_or(&[]) {
            if let Some(reference) = row.get("reference") {
                keys.push(text(reference));
            }
        }
    } else {
        let mut roles: Vec<String> = data
            .get("pad_nets")
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|row| row.as_array().and_then(|row| row.get(2)).map(text))
            .collect();
        roles.sort();
        roles.dedup();
        keys = roles;
    }
    keys.into_iter()
        .map(|key| {
            let mut chars = key.chars();
            let plain = chars.next().is_some_and(|c| python_isalpha(c) || c == '_')
                && key.chars().all(|c| python_isalnum(c) || c == '_' || c == '-');
            if plain { key } else { copper_json::encode_str(&key, true) }
        })
        .collect()
}

/// `_PIN_NAME.fullmatch`.
fn pin_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `_ENDPOINT.fullmatch`.
fn endpoint(text: &str) -> bool {
    let Some((component, pin)) = text.split_once('.') else { return false };
    let mut chars = component.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && pin_name(pin)
}

/// `Path.suffix`.
fn suffix(path: &Path) -> String {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => name[dot..].to_owned(),
        _ => String::new(),
    }
}

/// One open document: its path as first opened, version and text.
#[derive(Clone, Debug)]
pub struct OpenDocument {
    pub path: PathBuf,
    pub version: num_bigint::BigInt,
    pub text: String,
}

/// Open documents and the client's URI spellings (`Workspace` and `_CLIENT_URIS`).
#[derive(Default)]
pub struct Workspace {
    pub documents: IndexMap<PathKey, OpenDocument>,
    pub generation: u64,
    pub uris: IndexMap<PathKey, String>,
}

impl Workspace {
    /// `Workspace.update`; `text` is `Value::Null` when absent, `changes` is
    /// `None` for an open and for `contentChanges: null`.
    pub fn update(&mut self, uri: &Value, version: &Value, text: &Value, changes: Option<&Value>) -> PyResult<bool> {
        let path = path_from_uri(uri)?;
        let key = PathKey::of(&path);
        self.uris.insert(key.clone(), uri.as_str().expect("a parsed URI").to_owned());
        let Value::Int(version) = version else {
            return Err(PyError::Value("a Copper document and integer version are required".to_owned()));
        };
        if suffix(&path).to_lowercase() != ".copper" {
            return Err(PyError::Value("a Copper document and integer version are required".to_owned()));
        }
        let previous = self.documents.get(&key);
        if previous.is_some_and(|previous| *version <= previous.version) {
            return Ok(false);
        }
        let text = match changes {
            Some(changes) => {
                let Some(previous) = previous else {
                    return Err(PyError::Value("document is not open".to_owned()));
                };
                Value::Str(Text::new(&previous.text).change(changes)?)
            }
            None => text.clone(),
        };
        let Value::Str(text) = text else {
            return Err(PyError::Value("document exceeds text bounds".to_owned()));
        };
        if text.len() > MAX_TEXT {
            return Err(PyError::Value("document exceeds text bounds".to_owned()));
        }
        match self.documents.get_mut(&key) {
            Some(document) => {
                document.version = version.clone();
                document.text = text;
            }
            None => {
                self.documents.insert(key, OpenDocument { path, version: version.clone(), text });
            }
        }
        self.generation += 1;
        Ok(true)
    }

    pub fn invalidate(&mut self) {
        self.generation += 1;
    }

    pub fn close(&mut self, uri: &Value) -> PyResult<()> {
        let path = path_from_uri(uri)?;
        let key = PathKey::of(&path);
        self.documents.shift_remove(&key);
        if self.uris.get(&key).map(|known| Value::Str(known.clone())).as_ref() == Some(uri) {
            self.uris.shift_remove(&key);
        }
        self.invalidate();
        Ok(())
    }

    pub fn uri_for(&self, path: &Path) -> String {
        self.uris.get(&PathKey::of(path)).cloned().unwrap_or_else(|| as_uri(path))
    }

    /// The open documents and URI spellings an analysis reads.
    pub fn sources(&self) -> (Sources, IndexMap<PathKey, String>) {
        let texts = self.documents.iter().map(|(key, document)| (key.clone(), document.text.clone()));
        (Sources::new(texts), self.uris.clone())
    }
}

/// `Workspace.analysis` of a resolved path over a snapshot of open documents.
pub fn analyse_path(path: &Path, sources: Sources, uris: IndexMap<PathKey, String>) -> PyResult<Analysis> {
    sources.root(path)?;
    Analysis::new(path, sources, uris)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_follows_python() {
        assert_eq!(splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
        assert_eq!(splitlines("a\n  "), vec!["a", "  "]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
    }

    #[test]
    fn endpoint_and_pin_patterns() {
        assert!(endpoint("R1.2"));
        assert!(!endpoint("1R.2"));
        assert!(pin_name("1A"));
        assert!(!pin_name("-A"));
    }
}

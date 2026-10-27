//! The recursive-descent parser (`pcbir.parser`).
//!
//! Branches, their order and every diagnostic (code, message, location) follow
//! the Python parser exactly, including where an error points: some checks run
//! after a block is read and so report the token that follows it.

use std::str::FromStr;

use copper_json::python_str_repr;
use indexmap::IndexMap;
use num_bigint::BigInt;

use crate::error::CopperScriptError;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::suggest::close_match;
use crate::syntax::*;

type Result<T> = std::result::Result<T, CopperScriptError>;

const BOARD_KEYWORDS: [&str; 15] = [
    "component",
    "port",
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
    "mechanical",
    "silkscreen",
    "physical_macro",
];

const SILKSCREEN_KINDS: [&str; 8] = ["branding", "text", "logo", "line", "rectangle", "circle", "arc", "polygon"];
const MECHANICAL_KINDS: [&str; 16] = [
    "outline",
    "cutout",
    "hole",
    "rules",
    "keepout",
    "copper_keepout",
    "datum",
    "edge",
    "attach",
    "overhang",
    "component_height",
    "enclosure",
    "assembly_access",
    "boundary",
    "slot",
    "reference",
];
const SHAPED_MECHANICAL_KINDS: [&str; 8] =
    ["outline", "cutout", "keepout", "copper_keepout", "enclosure", "assembly_access", "boundary", "reference"];

/// `pcbir.parser.parse(source, filename)`: lex everything, then parse.
pub fn parse(source: &str, filename: &str) -> Result<Document> {
    Parser { tokens: tokenize(source, filename)?, index: 0 }.document()
}

/// `Parser(tokens).parse()` on an existing token sequence ending in end of file
/// (the editor's recovering parse re-parses token slices).
pub fn parse_tokens(tokens: Vec<Token>) -> Result<Document> {
    Parser { tokens, index: 0 }.document()
}

fn repr(text: &str) -> String {
    python_str_repr(text)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn current(&self) -> &Token {
        &self.tokens[self.index]
    }

    /// The token `back` positions before the current one.
    fn previous(&self, back: usize) -> &Token {
        &self.tokens[self.index - back]
    }

    fn error_at(&self, code: &str, message: impl Into<String>, token: &Token) -> CopperScriptError {
        CopperScriptError::new(code, message, token.location.clone())
    }

    fn error(&self, code: &str, message: impl Into<String>) -> CopperScriptError {
        self.error_at(code, message, self.current())
    }

    fn describe(token: &Token) -> String {
        if token.kind == TokenKind::Eof { token.kind.description().to_owned() } else { repr(&token.text) }
    }

    fn advance(&mut self) -> Token {
        let token = self.current().clone();
        if token.kind != TokenKind::Eof {
            self.index += 1;
        }
        token
    }

    fn at_symbol(&self, symbol: &str) -> bool {
        let token = self.current();
        token.kind == TokenKind::Symbol && token.text == symbol
    }

    fn accept_symbol(&mut self, symbol: &str) -> bool {
        if self.at_symbol(symbol) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<Token> {
        if !self.at_symbol(symbol) {
            return Err(
                self.error("PAR009", format!("expected {}, found {}", repr(symbol), Self::describe(self.current())))
            );
        }
        Ok(self.advance())
    }

    fn expect(&mut self, kind: TokenKind, expected: &str) -> Result<Token> {
        if self.current().kind != kind {
            return Err(self.error("PAR010", format!("expected {expected}, found {}", Self::describe(self.current()))));
        }
        Ok(self.advance())
    }

    fn expect_word(&mut self, word: &str) -> Result<Token> {
        let token = self.expect(TokenKind::Identifier, &repr(word))?;
        if token.text != word {
            return Err(self.error_at(
                "PAR008",
                format!("expected {}, found {}", repr(word), repr(&token.text)),
                &token,
            ));
        }
        Ok(token)
    }

    fn name(&mut self, expected: &str) -> Result<String> {
        if !matches!(self.current().kind, TokenKind::Identifier | TokenKind::Number) {
            return Err(self.error("PAR007", format!("expected {expected}, found {}", Self::describe(self.current()))));
        }
        Ok(self.advance().text)
    }

    fn qualified_name(&mut self, expected: &str) -> Result<String> {
        let mut value = self.name(expected)?;
        while self.accept_symbol(".") {
            value = format!("{value}.{}", self.name(expected)?);
        }
        Ok(value)
    }

    fn document(mut self) -> Result<Document> {
        let root = self.expect(TokenKind::Identifier, "'board', 'module', 'part', 'device', or 'board_profile'")?;
        let Some(kind) = DocumentKind::from_keyword(&root.text) else {
            return Err(self.error_at(
                "PAR008",
                format!("expected board, module, part, device, board_profile, or macro, found {}", repr(&root.text)),
                &root,
            ));
        };
        let name = self.name(&format!("{} name", root.text))?;
        self.expect_symbol("{")?;
        let mut libraries = Vec::new();
        let mut imports = Vec::new();
        let mut declarations = Vec::new();
        while !self.at_symbol("}") {
            if self.current().kind == TokenKind::Eof {
                return Err(self.error("PAR001", "expected '}' before end of file"));
            }
            let keyword = self.name("declaration")?;
            let keyword_location = self.previous(1).location.clone();
            if keyword == "import" {
                let alias = self.name("import alias")?;
                let path = self.expect(TokenKind::String, "package import path")?.text;
                imports.push(ImportDecl { location: keyword_location, alias, path });
                self.expect_symbol(";")?;
            } else if kind == DocumentKind::Macro {
                declarations.push(self.macro_declaration(&keyword, keyword_location)?);
            } else if keyword == "silkscreen" {
                if kind != DocumentKind::Board {
                    return Err(self.error_at("PAR015", "silkscreen blocks are board-only", self.previous(1)));
                }
                declarations.push(Declaration::Silkscreen(self.silkscreen()?));
            } else if keyword == "physical_macro" {
                if !matches!(kind, DocumentKind::Board | DocumentKind::Module) {
                    return Err(self.error_at("PAR016", "physical macros require a board or module", self.previous(1)));
                }
                declarations.push(Declaration::PhysicalMacro(self.physical_macro()?));
            } else if kind == DocumentKind::BoardProfile {
                declarations.push(Declaration::MechanicalItem(self.mechanical_item(
                    &keyword,
                    keyword_location,
                    true,
                )?));
            } else if keyword == "use" {
                let import_kind = self.name("'library'")?;
                let import_name = self.expect(TokenKind::String, &format!("{import_kind} name"))?.text;
                if import_kind == "library" {
                    libraries.push(import_name);
                } else {
                    return Err(self.error_at(
                        "PAR011",
                        format!("expected 'library', found {}", repr(&import_kind)),
                        self.previous(2),
                    ));
                }
                self.expect_symbol(";")?;
            } else if keyword == "mechanical" {
                if kind != DocumentKind::Board {
                    return Err(self.error_at("PAR012", "mechanical blocks are board-only", self.previous(1)));
                }
                declarations.push(Declaration::Mechanical(self.mechanical()?));
            } else if kind == DocumentKind::Part && keyword == "pin" {
                declarations.push(Declaration::Pin(self.named_block("pin name")?));
            } else if kind == DocumentKind::Part {
                declarations.push(Declaration::PartProperty(self.property(keyword, keyword_location)?));
            } else if kind == DocumentKind::Device {
                declarations.push(self.device_declaration(keyword, keyword_location)?);
            } else {
                declarations.push(match keyword.as_str() {
                    "component" => Declaration::Component(self.component()?),
                    "port" => Declaration::Port(self.port()?),
                    "module" => Declaration::ModuleInstance(self.module_instance()?),
                    "net" => Declaration::Net(self.net()?),
                    "no_connect" => Declaration::NoConnect(self.no_connect()?),
                    "supply" => Declaration::Supply(self.named_block("supply name")?),
                    "interface" => Declaration::Interface(self.interface()?),
                    "constraint" => Declaration::Constraint(self.constraint()?),
                    "configure" => Declaration::Configuration(self.configuration()?),
                    "power_state" => Declaration::PowerState(self.power_state()?),
                    _ => {
                        let suggestion = close_match(&keyword, &BOARD_KEYWORDS);
                        let mut error = self.error_at(
                            "PAR002",
                            format!("unknown declaration {}", repr(&keyword)),
                            self.previous(1),
                        );
                        error.hint = suggestion.as_ref().map(|word| format!("did you mean {}?", repr(word)));
                        error.replacement = suggestion;
                        return Err(error);
                    }
                });
            }
        }
        self.expect_symbol("}")?;
        self.expect(TokenKind::Eof, "end of file")?;
        Ok(Document { location: root.location, kind, name, libraries, imports, declarations })
    }

    fn property(&mut self, name: String, location: Location) -> Result<PropertyDecl> {
        self.expect_symbol("=")?;
        let value = self.scalar()?;
        self.expect_symbol(";")?;
        Ok(PropertyDecl { location, name, value })
    }

    fn device_declaration(&mut self, keyword: String, location: Location) -> Result<Declaration> {
        Ok(match keyword.as_str() {
            "peripheral" => Declaration::Peripheral(self.peripheral()?),
            "pad" => Declaration::Pad(self.named_block("device pad name")?),
            "power_domain" => Declaration::PowerDomain(self.named_block("power domain name")?),
            "mux" => Declaration::Mux(self.mux()?),
            "resource" => {
                let name = self.name("resource name")?;
                self.expect_symbol(";")?;
                Declaration::Resource(ResourceDecl { location, name })
            }
            "unit" => Declaration::Unit(self.kinded_block("functional unit name", "functional unit kind")?),
            "group" => Declaration::SignalGroup(self.kinded_block("signal group name", "signal group kind")?),
            "mode_group" => Declaration::ModeGroup(self.named_block("mode group name")?),
            "pad_set" => Declaration::PadSet(self.named_block("pad set name")?),
            "route" => Declaration::RouteRule(self.route_rule()?),
            _ => Declaration::DeviceProperty(self.property(keyword, location)?),
        })
    }

    fn macro_declaration(&mut self, keyword: &str, location: Location) -> Result<Declaration> {
        if keyword == "pad" || keyword == "isolated" {
            let reference = self.macro_pad_reference()?;
            let mut net = None;
            if keyword == "pad" {
                self.expect_symbol("=")?;
                net = Some(self.macro_name()?);
            }
            self.expect_symbol(";")?;
            Ok(Declaration::MacroPad(MacroPadDecl { location, reference, net }))
        } else if keyword == "net" {
            let name = self.macro_name()?;
            let item = MacroItemDecl {
                location,
                kind: keyword.to_owned(),
                name,
                parameters: Parameters::new(),
                children: Vec::new(),
            };
            self.expect_symbol(";")?;
            Ok(Declaration::MacroItem(item))
        } else if self.accept_symbol("=") {
            let value = self.macro_value()?;
            self.expect_symbol(";")?;
            Ok(Declaration::MacroProperty(MacroPropertyDecl { location, name: keyword.to_owned(), value }))
        } else {
            Ok(Declaration::MacroItem(self.macro_item(keyword, location)?))
        }
    }

    fn mechanical(&mut self) -> Result<MechanicalDecl> {
        let location = self.previous(1).location.clone();
        self.expect_symbol("{")?;
        let mut items = Vec::new();
        while !self.at_symbol("}") {
            let start = self.current().location.clone();
            let kind = self.name("mechanical declaration")?;
            if kind == "stackup" {
                items.push(MechanicalItem::Stackup(self.stackup(start)?));
            } else {
                items.push(self.mechanical_item(&kind, start, false)?);
            }
        }
        self.expect_symbol("}")?;
        Ok(MechanicalDecl { location, items })
    }

    fn silkscreen(&mut self) -> Result<SilkscreenDecl> {
        let location = self.previous(1).location.clone();
        self.expect_symbol("{")?;
        let mut items = Vec::new();
        while !self.at_symbol("}") {
            let start = self.current().location.clone();
            let kind = self.name("silkscreen declaration")?;
            if !SILKSCREEN_KINDS.contains(&kind.as_str()) {
                return Err(self.error("PAR016", format!("unknown silkscreen declaration {}", repr(&kind))));
            }
            let name = if kind == "branding" { "branding".to_owned() } else { self.name("silkscreen feature id")? };
            let parameters = self.geometry_parameters()?;
            items.push(SilkscreenItemDecl { location: start, kind, name, parameters });
        }
        self.expect_symbol("}")?;
        Ok(SilkscreenDecl { location, items })
    }

    fn stackup(&mut self, location: Location) -> Result<StackupDecl> {
        self.expect_symbol("{")?;
        let mut layers = Vec::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let kind = self.name("stackup layer kind")?;
            if kind != "copper" && kind != "dielectric" {
                return Err(self.error_at(
                    "PAR014",
                    format!("unknown stackup layer kind {}; expected 'copper' or 'dielectric'", repr(&kind)),
                    &token,
                ));
            }
            let name = self.qualified_name(&format!("{kind} layer name"))?;
            let parameters = self.assignment_block()?;
            layers.push(StackupLayerDecl { location: token.location, kind, name, parameters });
        }
        self.expect_symbol("}")?;
        Ok(StackupDecl { location, layers })
    }

    fn mechanical_item(&mut self, kind: &str, location: Location, profile: bool) -> Result<MechanicalItem> {
        if kind == "use" {
            let profile_name = self.qualified_name("board profile")?;
            let mut instance = profile_name.rsplit('.').next().unwrap_or_default().to_owned();
            if self.current().text == "as" {
                self.advance();
                instance = self.name("profile instance")?;
            }
            let bindings = self.assignment_block()?;
            return Ok(MechanicalItem::ProfileUse(MechanicalProfileUseDecl {
                location,
                profile: profile_name,
                instance,
                bindings,
            }));
        }
        let allowed = MECHANICAL_KINDS.contains(&kind) || (profile && kind == "connector");
        if !allowed {
            return Err(self.error("PAR013", format!("unknown mechanical declaration {}", repr(kind))));
        }
        let name =
            if kind == "outline" || kind == "rules" { String::new() } else { self.name("mechanical feature id")? };
        let shape =
            if SHAPED_MECHANICAL_KINDS.contains(&kind) { self.name("mechanical shape")? } else { String::new() };
        let parameters = self.geometry_parameters()?;
        Ok(MechanicalItem::Feature(MechanicalItemDecl { location, kind: kind.to_owned(), name, shape, parameters }))
    }

    fn geometry_parameters(&mut self) -> Result<Parameters> {
        self.expect_symbol("{")?;
        let mut parameters = Parameters::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let key = self.name("mechanical property")?;
            if parameters.contains_key(&key) {
                return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &token));
            }
            self.expect_symbol("=")?;
            let value = if self.accept_symbol("[") {
                let mut points = vec![self.point()?];
                while self.accept_symbol(",") {
                    points.push(self.point()?);
                }
                self.expect_symbol("]")?;
                Value::Points(points)
            } else if self.at_symbol("(") {
                let (x, y) = self.point()?;
                Value::Point(x, y)
            } else {
                Value::Scalar(self.scalar()?)
            };
            parameters.insert(key, value);
            self.expect_symbol(";")?;
        }
        self.expect_symbol("}")?;
        Ok(parameters)
    }

    fn point(&mut self) -> Result<(Scalar, Scalar)> {
        self.expect_symbol("(")?;
        let x = self.scalar()?;
        self.expect_symbol(",")?;
        let y = self.scalar()?;
        self.expect_symbol(")")?;
        Ok((x, y))
    }

    fn component(&mut self) -> Result<ComponentDecl> {
        let location = self.current().location.clone();
        let reference = self.name("component reference")?;
        self.expect_symbol(":")?;
        let part = self.qualified_name("part name")?;
        let attributes = if self.accept_symbol(";") { Attributes::new() } else { self.assignment_block()? };
        Ok(ComponentDecl { location, reference, part, attributes })
    }

    fn port(&mut self) -> Result<PortDecl> {
        let location = self.current().location.clone();
        let name = self.name("port name")?;
        self.expect_symbol(":")?;
        let pin_type = self.name("port electrical type")?;
        self.expect_symbol(";")?;
        Ok(PortDecl { location, name, pin_type })
    }

    fn named_block(&mut self, expected: &str) -> Result<NamedBlock> {
        let location = self.current().location.clone();
        let name = self.name(expected)?;
        let attributes = self.assignment_block()?;
        Ok(NamedBlock { location, name, attributes })
    }

    fn kinded_block(&mut self, name_expected: &str, kind_expected: &str) -> Result<KindedBlock> {
        let location = self.current().location.clone();
        let name = self.name(name_expected)?;
        self.expect_symbol(":")?;
        let kind = self.qualified_name(kind_expected)?;
        let attributes = self.assignment_block()?;
        Ok(KindedBlock { location, name, kind, attributes })
    }

    /// An assignment block whose values must all be names (`str` in Python);
    /// the check runs after the block, so it reports the following token.
    fn name_block(&mut self, code: &str, message: &str) -> Result<IndexMap<String, String>> {
        let raw = self.assignment_block()?;
        let mut names = IndexMap::new();
        for (key, value) in raw {
            match value {
                Scalar::Str(text) => {
                    names.insert(key, text);
                }
                _ => return Err(self.error(code, message)),
            }
        }
        Ok(names)
    }

    fn power_state(&mut self) -> Result<PowerStateDecl> {
        let location = self.current().location.clone();
        let name = self.name("power state name")?;
        let rails = self.name_block("PAR013", "power state value must be on, off, or unknown")?;
        Ok(PowerStateDecl { location, name, rails })
    }

    fn module_instance(&mut self) -> Result<ModuleInstanceDecl> {
        let location = self.current().location.clone();
        let reference = self.name("module instance reference")?;
        self.expect_symbol(":")?;
        let module = self.qualified_name("module name")?;
        self.expect_symbol(";")?;
        Ok(ModuleInstanceDecl { location, reference, module })
    }

    fn peripheral(&mut self) -> Result<PeripheralDecl> {
        let location = self.current().location.clone();
        let name = self.name("peripheral name")?;
        self.expect_symbol(":")?;
        let kind = self.name("peripheral kind")?;
        self.expect_symbol("{")?;
        let mut signals = Vec::new();
        while !self.at_symbol("}") {
            self.expect_word("signal")?;
            let signal_location = self.current().location.clone();
            let signal_name = self.name("signal name")?;
            self.expect_symbol(":")?;
            let pin_type = self.name("signal electrical type")?;
            let attributes = if self.accept_symbol(";") { Attributes::new() } else { self.assignment_block()? };
            signals.push(PeripheralSignalDecl { location: signal_location, name: signal_name, pin_type, attributes });
        }
        self.expect_symbol("}")?;
        Ok(PeripheralDecl { location, name, kind, signals })
    }

    fn mux(&mut self) -> Result<MuxDecl> {
        let location = self.current().location.clone();
        let pad = self.name("device pad")?;
        self.expect_symbol(":")?;
        let peripheral = self.name("peripheral name")?;
        self.expect_symbol(".")?;
        let signal = self.name("peripheral signal")?;
        let attributes = self.assignment_block()?;
        Ok(MuxDecl { location, pad, peripheral, signal, attributes })
    }

    fn route_rule(&mut self) -> Result<RouteRuleDecl> {
        let location = self.current().location.clone();
        let peripheral = self.name("peripheral name")?;
        self.expect_symbol(".")?;
        let signal = self.name("peripheral signal")?;
        let attributes = self.assignment_block()?;
        Ok(RouteRuleDecl { location, peripheral, signal, attributes })
    }

    fn configuration(&mut self) -> Result<ConfigurationDecl> {
        let location = self.current().location.clone();
        let component = self.name("component reference")?;
        self.expect_symbol(".")?;
        let peripheral = self.name("peripheral name")?;
        self.expect_word("as")?;
        let name = self.name("configuration name")?;
        let signals = self.name_block("PAR012", "configured signal must name a physical pin")?;
        Ok(ConfigurationDecl { location, component, peripheral, name, signals })
    }

    fn net(&mut self) -> Result<NetDecl> {
        let location = self.current().location.clone();
        let name = self.name("net name")?;
        self.expect_symbol("{")?;
        let mut endpoints = Vec::new();
        let mut endpoint_locations = Vec::new();
        while !self.at_symbol("}") {
            endpoint_locations.push(self.current().location.clone());
            endpoints.push(self.endpoint()?);
            self.expect_symbol(";")?;
        }
        self.expect_symbol("}")?;
        Ok(NetDecl { location, name, endpoints, endpoint_locations })
    }

    fn no_connect(&mut self) -> Result<NoConnectDecl> {
        let location = self.previous(1).location.clone();
        let mut endpoints = Vec::new();
        let mut endpoint_locations = Vec::new();
        loop {
            endpoint_locations.push(self.current().location.clone());
            endpoints.push(self.endpoint()?);
            if !self.accept_symbol(",") {
                break;
            }
        }
        let attributes = if self.accept_symbol(";") { Attributes::new() } else { self.assignment_block()? };
        Ok(NoConnectDecl { location, endpoints, attributes, endpoint_locations })
    }

    fn interface(&mut self) -> Result<InterfaceDecl> {
        let location = self.current().location.clone();
        let name = self.name("interface name")?;
        self.expect_symbol(":")?;
        let kind = self.name("interface kind")?;
        self.expect_symbol("{")?;
        let mut attributes = Attributes::new();
        let mut bindings = Vec::new();
        while !self.at_symbol("}") {
            let entry_location = self.current().location.clone();
            let key = self.name("interface property or 'bind'")?;
            if key == "bind" {
                let component = self.name("component reference")?;
                let signals = self.name_block("PAR003", "interface pin binding must be a pin name")?;
                bindings.push(BindingDecl { location: entry_location, component, signals });
            } else {
                self.expect_symbol("=")?;
                let value = self.scalar()?;
                attributes.insert(key, value);
                self.expect_symbol(";")?;
            }
        }
        self.expect_symbol("}")?;
        Ok(InterfaceDecl { location, name, kind, attributes, bindings })
    }

    fn constraint(&mut self) -> Result<ConstraintDecl> {
        let location = self.current().location.clone();
        let kind = self.name("constraint kind")?;
        self.expect_symbol("(")?;
        let mut targets = Vec::new();
        if !self.at_symbol(")") {
            targets.push(self.reference()?);
            while self.accept_symbol(",") {
                targets.push(self.reference()?);
            }
        }
        self.expect_symbol(")")?;
        let parameters = self.assignment_block()?;
        Ok(ConstraintDecl { location, kind, targets, parameters })
    }

    fn physical_macro(&mut self) -> Result<PhysicalMacroDecl> {
        let location = self.current().location.clone();
        let name = self.name("physical macro name")?;
        self.expect_symbol(":")?;
        let asset_alias = self.qualified_name("macro asset import alias or exported definition")?;
        let mut scope = String::new();
        if self.current().text == "on" {
            self.advance();
            scope = self.reference()?;
        }
        self.expect_symbol("{")?;
        let mut mappings = Vec::new();
        let mut keys: Vec<(String, String)> = Vec::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let kind = self.name("'member' or 'net'")?;
            if kind != "member" && kind != "net" {
                return Err(self.error_at("PAR016", "macro bindings accept member and net mappings", &token));
            }
            let key =
                if self.current().kind == TokenKind::String { self.advance().text } else { self.name("asset role")? };
            let pair = (kind.clone(), key.clone());
            if key.is_empty() || keys.contains(&pair) {
                return Err(self.error_at(
                    "PAR016",
                    format!("empty or duplicate macro {kind} key {}", repr(&key)),
                    &token,
                ));
            }
            keys.push(pair);
            self.expect_symbol("=")?;
            let target = self.reference()?;
            self.expect_symbol(";")?;
            mappings.push(MacroMappingDecl { location: token.location, kind, key, target });
        }
        self.expect_symbol("}")?;
        Ok(PhysicalMacroDecl { location, name, asset_alias, scope, mappings })
    }

    fn macro_name(&mut self) -> Result<String> {
        if self.current().kind == TokenKind::String { Ok(self.advance().text) } else { self.name("macro name") }
    }

    fn macro_pad_reference(&mut self) -> Result<MacroPadReference> {
        let member = self.macro_name()?;
        self.expect_symbol(".")?;
        let pad = self.macro_name()?;
        Ok(MacroPadReference { member, pad })
    }

    fn macro_value(&mut self) -> Result<Value> {
        if self.accept_symbol("{") {
            let mut values = Parameters::new();
            while !self.at_symbol("}") {
                let token = self.current().clone();
                let key = self.macro_name()?;
                if values.contains_key(&key) {
                    return Err(self.error_at("PAR004", format!("duplicate evidence key {}", repr(&key)), &token));
                }
                self.expect_symbol("=")?;
                let value = self.macro_value()?;
                values.insert(key, value);
                self.expect_symbol(";")?;
            }
            self.expect_symbol("}")?;
            return Ok(Value::Map(values));
        }
        if self.at_symbol("[") || self.at_symbol("(") {
            let close = if self.advance().text == "[" { "]" } else { ")" };
            let mut values = Vec::new();
            if !self.at_symbol(close) {
                values.push(self.macro_value()?);
                while self.accept_symbol(",") {
                    values.push(self.macro_value()?);
                }
            }
            self.expect_symbol(close)?;
            return Ok(Value::List(values));
        }
        if matches!(self.current().kind, TokenKind::Identifier | TokenKind::String | TokenKind::Number)
            && self.tokens.get(self.index + 1).is_some_and(|next| next.text == ".")
        {
            return Ok(Value::PadRef(self.macro_pad_reference()?));
        }
        Ok(Value::Scalar(self.scalar()?))
    }

    fn macro_item(&mut self, kind: &str, location: Location) -> Result<MacroItemDecl> {
        let name = self.macro_name()?;
        self.expect_symbol("{")?;
        let mut parameters = Parameters::new();
        let mut children = Vec::new();
        while !self.at_symbol("}") {
            let token = self.current().clone();
            let key = self.name("macro property or contact")?;
            if key == "contact" && !self.at_symbol("=") {
                children.push(self.macro_item(&key, token.location)?);
            } else {
                if parameters.contains_key(&key) {
                    return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &token));
                }
                self.expect_symbol("=")?;
                let value = self.macro_value()?;
                parameters.insert(key, value);
                self.expect_symbol(";")?;
            }
        }
        self.expect_symbol("}")?;
        Ok(MacroItemDecl { location, kind: kind.to_owned(), name, parameters, children })
    }

    fn assignment_block(&mut self) -> Result<Attributes> {
        self.expect_symbol("{")?;
        let mut attributes = Attributes::new();
        while !self.at_symbol("}") {
            let key_token = self.current().clone();
            let key = self.name("property name")?;
            if attributes.contains_key(&key) {
                return Err(self.error_at("PAR004", format!("duplicate property {}", repr(&key)), &key_token));
            }
            self.expect_symbol("=")?;
            let value = self.scalar()?;
            attributes.insert(key, value);
            self.expect_symbol(";")?;
        }
        self.expect_symbol("}")?;
        Ok(attributes)
    }

    /// Python's `float(text) if "." in text else int(text)`.
    fn number(text: &str) -> Scalar {
        if text.contains('.') {
            Scalar::Float(text.parse().expect("a lexed number is a valid float"))
        } else {
            Scalar::Int(BigInt::from_str(text.trim_start_matches('+')).expect("a lexed number is a valid integer"))
        }
    }

    fn scalar(&mut self) -> Result<Scalar> {
        if self.at_symbol("-") || self.at_symbol("+") {
            let sign = self.advance().text;
            let number = format!("{sign}{}", self.expect(TokenKind::Number, "number after sign")?.text);
            if self.current().kind == TokenKind::Identifier {
                return Ok(Scalar::Quantity { value: number, unit: self.advance().text });
            }
            return Ok(Self::number(&number));
        }
        match self.current().kind {
            TokenKind::String => Ok(Scalar::Str(self.advance().text)),
            TokenKind::Number => {
                let number = self.advance().text;
                if self.current().kind == TokenKind::Identifier {
                    return Ok(Scalar::Quantity { value: number, unit: self.advance().text });
                }
                Ok(Self::number(&number))
            }
            TokenKind::Identifier => {
                let mut value = self.advance().text;
                match value.as_str() {
                    "true" => return Ok(Scalar::Bool(true)),
                    "false" => return Ok(Scalar::Bool(false)),
                    _ => {}
                }
                if self.accept_symbol(".") {
                    value = format!("{value}.{}", self.name("pin name")?);
                }
                Ok(Scalar::Str(value))
            }
            _ => Err(self.error("PAR006", "expected a string, identifier, boolean, number, or quantity")),
        }
    }

    fn endpoint(&mut self) -> Result<String> {
        let component = self.name("component reference")?;
        self.expect_symbol(".")?;
        Ok(format!("{component}.{}", self.qualified_name("pin or terminal name")?))
    }

    fn reference(&mut self) -> Result<String> {
        let mut value = self.name("target")?;
        while self.accept_symbol("/") {
            value = format!("{value}/{}", self.name("instance target")?);
        }
        if self.accept_symbol(".") {
            value = format!("{value}.{}", self.name("pin name")?);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_board() {
        let source =
            "board B {\n use library \"tiny\";\n component R1: RESISTOR { value = 10kohm; }\n net N { R1.1; }\n}";
        let document = parse(source, "b.copper").unwrap();
        assert_eq!(document.libraries, ["tiny"]);
        let Declaration::Component(component) = &document.declarations[0] else { panic!() };
        assert_eq!(component.attributes["value"], Scalar::Quantity { value: "10".into(), unit: "kohm".into() });
    }

    #[test]
    fn unknown_declarations_suggest_a_keyword() {
        let error = parse("board B { componnet R1: X; }", "b").unwrap_err();
        assert_eq!(error.to_string(), "b:1:11: PAR002: unknown declaration 'componnet' (did you mean 'component'?)");
        assert_eq!(error.replacement.as_deref(), Some("component"));
    }
}

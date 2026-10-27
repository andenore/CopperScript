//! The CopperScript syntax tree (`pcbir.syntax`).
//!
//! One type per Python node. Property maps keep source order, as Python
//! dicts do. Values carry the token text they came from where Python keeps
//! text (`RawQuantity`), and parsed numbers where Python converts them.

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;
use num_bigint::BigInt;

/// A source position: code-point offset, 1-based line and column.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    pub filename: Arc<str>,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}", self.filename, self.line, self.column)
    }
}

/// A property value in an assignment block (`Scalar` in Python).
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    /// A string literal, an identifier, or `name.pin`.
    Str(String),
    Bool(bool),
    Int(BigInt),
    Float(f64),
    /// A number with a unit suffix (`100nF`), kept as written.
    Quantity {
        value: String,
        unit: String,
    },
}

/// A geometry or macro parameter value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Scalar(Scalar),
    /// `(x, y)`.
    Point(Scalar, Scalar),
    /// `[(x, y), ...]`.
    Points(Vec<(Scalar, Scalar)>),
    /// A macro evidence block `{ key = value; ... }`.
    Map(IndexMap<String, Value>),
    /// A macro list `[...]` or tuple `(...)`.
    List(Vec<Value>),
    /// A macro pad reference `member.pad`.
    PadRef(MacroPadReference),
}

pub type Attributes = IndexMap<String, Scalar>;
pub type Parameters = IndexMap<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocumentKind {
    Board,
    Module,
    Part,
    Device,
    BoardProfile,
    Macro,
}

impl DocumentKind {
    pub fn from_keyword(keyword: &str) -> Option<DocumentKind> {
        Some(match keyword {
            "board" => DocumentKind::Board,
            "module" => DocumentKind::Module,
            "part" => DocumentKind::Part,
            "device" => DocumentKind::Device,
            "board_profile" => DocumentKind::BoardProfile,
            "macro" => DocumentKind::Macro,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Board => "board",
            DocumentKind::Module => "module",
            DocumentKind::Part => "part",
            DocumentKind::Device => "device",
            DocumentKind::BoardProfile => "board_profile",
            DocumentKind::Macro => "macro",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub location: Location,
    pub kind: DocumentKind,
    pub name: String,
    pub libraries: Vec<String>,
    pub imports: Vec<ImportDecl>,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportDecl {
    pub location: Location,
    pub alias: String,
    pub path: String,
}

/// Every declaration kind; the variant names match the Python classes
/// without their `Decl` suffix.
#[derive(Clone, Debug, PartialEq)]
pub enum Declaration {
    Component(ComponentDecl),
    Port(PortDecl),
    Pin(NamedBlock),
    Pad(NamedBlock),
    PowerDomain(NamedBlock),
    PowerState(PowerStateDecl),
    PartProperty(PropertyDecl),
    DeviceProperty(PropertyDecl),
    Peripheral(PeripheralDecl),
    Mux(MuxDecl),
    Resource(ResourceDecl),
    Unit(KindedBlock),
    SignalGroup(KindedBlock),
    ModeGroup(NamedBlock),
    PadSet(NamedBlock),
    RouteRule(RouteRuleDecl),
    Configuration(ConfigurationDecl),
    ModuleInstance(ModuleInstanceDecl),
    PhysicalMacro(PhysicalMacroDecl),
    MacroProperty(MacroPropertyDecl),
    MacroItem(MacroItemDecl),
    MacroPad(MacroPadDecl),
    Net(NetDecl),
    NoConnect(NoConnectDecl),
    Supply(NamedBlock),
    Interface(InterfaceDecl),
    Constraint(ConstraintDecl),
    Mechanical(MechanicalDecl),
    Silkscreen(SilkscreenDecl),
    /// A `board_profile` item at the top level.
    MechanicalItem(MechanicalItem),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentDecl {
    pub location: Location,
    pub reference: String,
    pub part: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    pub location: Location,
    pub name: String,
    pub pin_type: String,
}

/// `NAME { attributes }`: pins, pads, power domains, mode groups, pad sets, supplies.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedBlock {
    pub location: Location,
    pub name: String,
    pub attributes: Attributes,
}

/// `NAME: KIND { attributes }`: functional units (`terminals`) and signal groups.
#[derive(Clone, Debug, PartialEq)]
pub struct KindedBlock {
    pub location: Location,
    pub name: String,
    pub kind: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerStateDecl {
    pub location: Location,
    pub name: String,
    pub rails: IndexMap<String, String>,
}

/// `name = value;` in a part or device.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDecl {
    pub location: Location,
    pub name: String,
    pub value: Scalar,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralSignalDecl {
    pub location: Location,
    pub name: String,
    pub pin_type: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralDecl {
    pub location: Location,
    pub name: String,
    pub kind: String,
    pub signals: Vec<PeripheralSignalDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MuxDecl {
    pub location: Location,
    pub pad: String,
    pub peripheral: String,
    pub signal: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceDecl {
    pub location: Location,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteRuleDecl {
    pub location: Location,
    pub peripheral: String,
    pub signal: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigurationDecl {
    pub location: Location,
    pub component: String,
    pub peripheral: String,
    pub name: String,
    pub signals: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleInstanceDecl {
    pub location: Location,
    pub reference: String,
    pub module: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroMappingDecl {
    pub location: Location,
    pub kind: String,
    pub key: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalMacroDecl {
    pub location: Location,
    pub name: String,
    pub asset_alias: String,
    pub scope: String,
    pub mappings: Vec<MacroMappingDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroPadReference {
    pub member: String,
    pub pad: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroPropertyDecl {
    pub location: Location,
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroItemDecl {
    pub location: Location,
    pub kind: String,
    pub name: String,
    pub parameters: Parameters,
    pub children: Vec<MacroItemDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacroPadDecl {
    pub location: Location,
    pub reference: MacroPadReference,
    /// `None` for an `isolated` pad.
    pub net: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetDecl {
    pub location: Location,
    pub name: String,
    pub endpoints: Vec<String>,
    pub endpoint_locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoConnectDecl {
    pub location: Location,
    pub endpoints: Vec<String>,
    pub attributes: Attributes,
    pub endpoint_locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BindingDecl {
    pub location: Location,
    pub component: String,
    pub signals: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceDecl {
    pub location: Location,
    pub name: String,
    pub kind: String,
    pub attributes: Attributes,
    pub bindings: Vec<BindingDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintDecl {
    pub location: Location,
    pub kind: String,
    pub targets: Vec<String>,
    pub parameters: Attributes,
}

/// An item of a `mechanical` block or a `board_profile`.
#[derive(Clone, Debug, PartialEq)]
pub enum MechanicalItem {
    Feature(MechanicalItemDecl),
    ProfileUse(MechanicalProfileUseDecl),
    Stackup(StackupDecl),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalItemDecl {
    pub location: Location,
    pub kind: String,
    pub name: String,
    pub shape: String,
    pub parameters: Parameters,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalProfileUseDecl {
    pub location: Location,
    pub profile: String,
    pub instance: String,
    pub bindings: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StackupLayerDecl {
    pub location: Location,
    pub kind: String,
    pub name: String,
    pub parameters: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StackupDecl {
    pub location: Location,
    pub layers: Vec<StackupLayerDecl>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalDecl {
    pub location: Location,
    pub items: Vec<MechanicalItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SilkscreenItemDecl {
    pub location: Location,
    pub kind: String,
    pub name: String,
    pub parameters: Parameters,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SilkscreenDecl {
    pub location: Location,
    pub items: Vec<SilkscreenItemDecl>,
}

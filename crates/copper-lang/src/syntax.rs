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
    Panel,
    /// `bench NAME for TARGET { ... }`: a simulation bench of a board or a module.
    Bench,
    /// `fixture NAME for TARGET { ... }`: bench setup shared by the benches that
    /// name it (`bench NAME for TARGET using FIXTURE { ... }`).
    Fixture,
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
            "panel" => DocumentKind::Panel,
            "bench" => DocumentKind::Bench,
            "fixture" => DocumentKind::Fixture,
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
            DocumentKind::Panel => "panel",
            DocumentKind::Bench => "bench",
            DocumentKind::Fixture => "fixture",
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
    Build(BuildDecl),
    /// A `board_profile` item at the top level.
    MechanicalItem(MechanicalItem),
    /// `param NAME = quantity;` in a board or bench.
    Param(ParamDecl),
    /// `sim KIND [UNIT] { ... }` in a part.
    Sim(SimDecl),
    /// A bench statement; a bench document's first declaration is its `for BOARD` target.
    Bench(BenchStatement),
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

/// One `key = value;` of a board `build` block. `group` is the sub-block
/// (`placement`, `routing`, `planes`, `router`), empty at the top level.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildSettingDecl {
    /// The key token.
    pub location: Location,
    pub group: String,
    pub key: String,
    pub value: Scalar,
}

/// A board `build { ... }` block: settings in source order.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildDecl {
    pub location: Location,
    pub settings: Vec<BuildSettingDecl>,
}

/// `param NAME = quantity;`, or in a bench `param NAME = expression;` (a
/// param derived from earlier params).
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    pub location: Location,
    pub name: String,
    /// The literal value; an empty string when `expression` is set.
    pub value: Scalar,
    /// The expression of a derived param; `None` for a literal.
    pub expression: Option<Expr>,
}

/// A value in a simulation block.
#[derive(Clone, Debug, PartialEq)]
pub enum SimValue {
    Scalar(Scalar),
    /// `a, b, c` (`unmodelled = NC, EPAD;`).
    List(Vec<Scalar>),
    /// `KIND { key = value; ... }` (`waveform = sine { ... };`).
    Block {
        kind: String,
        entries: Vec<SimEntry>,
    },
}

/// `key = value;` in a simulation block; `location` is the key token.
#[derive(Clone, Debug, PartialEq)]
pub struct SimEntry {
    pub location: Location,
    pub key: String,
    pub value: SimValue,
}

/// A part's `sim KIND [UNIT] { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct SimDecl {
    pub location: Location,
    pub kind: String,
    pub unit: Option<String>,
    pub entries: Vec<SimEntry>,
}

/// The statements of a bench document.
#[derive(Clone, Debug, PartialEq)]
pub enum BenchStatement {
    /// The header's `for BOARD`.
    Target(BenchTargetDecl),
    Replace(ReplaceDecl),
    /// `model NAME: KIND { ... }`: a built-in model instance added by the bench.
    Model(ModelDecl),
    Ignore(IgnoreDecl),
    Analysis(AnalysisDecl),
    Step(StepDecl),
    Reference(ReferenceDecl),
    Let(LetDecl),
    Measure(MeasureDecl),
    Plot(PlotDecl),
    /// `montecarlo { ... }`, `worstcase;` or `tolerance TARGET = value;`.
    Variation(VariationDecl),
    /// `ic V(NODE) = value;` or `ic COMPONENT = value;`: an initial condition of a transient run.
    InitialCondition(InitialConditionDecl),
}

/// `for TARGET` in `bench NAME for TARGET { ... }`: a board, or a module the
/// board instantiates; `location` is the target's name.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchTargetDecl {
    pub location: Location,
    pub board: String,
    /// The fixtures of `using A, B` after the target, in order (a bench only).
    pub fixtures: Vec<String>,
    pub fixture_locations: Vec<Location>,
}

/// What an initial condition sets.
#[derive(Clone, Debug, PartialEq)]
pub enum InitialConditionTarget {
    /// `ic V(NODE) = ...;`: a node voltage against ground.
    Node(String),
    /// `ic C1 = ...;` or `ic L1 = ...;`: a capacitor's voltage or an inductor's current.
    Component(String),
}

/// `ic V(NODE) = value;` or `ic COMPONENT = value;`.
#[derive(Clone, Debug, PartialEq)]
pub struct InitialConditionDecl {
    pub location: Location,
    pub target: InitialConditionTarget,
    pub target_location: Location,
    pub value: Scalar,
}

/// `replace TARGET with KIND { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaceDecl {
    pub location: Location,
    pub target: String,
    pub kind: String,
    pub entries: Vec<SimEntry>,
}

/// `model NAME: KIND { roles; parameters }`: a simulation-only instance of a
/// built-in model (a stimulus such as a DDS stand-in on a module port).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDecl {
    pub location: Location,
    pub name: String,
    pub kind: String,
    pub entries: Vec<SimEntry>,
}

/// `ignore TARGET, TARGET;`.
#[derive(Clone, Debug, PartialEq)]
pub struct IgnoreDecl {
    pub location: Location,
    pub targets: Vec<String>,
    pub target_locations: Vec<Location>,
}

/// `analysis KIND;` or `analysis KIND { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisDecl {
    pub location: Location,
    pub kind: String,
    pub entries: Vec<SimEntry>,
}

/// `step PARAM in a, b, c;`, or zipped: `step A, B in (a1, b1), (a2, b2);`.
#[derive(Clone, Debug, PartialEq)]
pub struct StepDecl {
    pub location: Location,
    /// The first (or only) stepped param.
    pub param: String,
    /// Its values, one per step.
    pub values: Vec<Scalar>,
    /// The other params of a zipped step, each with one value per step.
    pub zip: Vec<(String, Vec<Scalar>)>,
}

/// `reference NAME { PARAM = value; ... }`: the bench re-run with these
/// params, read by `reference(x, NAME)` (a calibration standard).
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceDecl {
    pub location: Location,
    pub name: String,
    pub entries: Vec<SimEntry>,
}

/// `let NAME = expression;`.
#[derive(Clone, Debug, PartialEq)]
pub struct LetDecl {
    pub location: Location,
    pub name: String,
    pub expression: Expr,
}

/// `measure NAME = expression;` or `measure NAME = expression { min = ...; max = ...; }`.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureDecl {
    pub location: Location,
    pub name: String,
    pub expression: Expr,
    pub limits: Vec<SimEntry>,
    /// `where A and B;` in the limits block: the comparisons of params that
    /// select the steps the limits apply to (empty: every step).
    pub conditions: Vec<Expr>,
}

/// `plot expression, expression;`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotDecl {
    pub location: Location,
    pub expressions: Vec<Expr>,
}

/// A tolerance statement of a bench: `montecarlo { runs = N; ... }`,
/// `worstcase;`, or `tolerance TARGET = value;` (a param or `COMPONENT.PARAMETER`).
#[derive(Clone, Debug, PartialEq)]
pub struct VariationDecl {
    pub location: Location,
    /// `montecarlo`, `worstcase` or `tolerance`.
    pub kind: String,
    /// The target of a `tolerance` statement.
    pub target: Option<String>,
    /// The value of a `tolerance` statement.
    pub value: Option<Scalar>,
    /// The properties of `montecarlo { ... }` (and of `worstcase { ... }`).
    pub entries: Vec<SimEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// A comparison: `<`, `<=`, `>`, `>=`, `==` or `!=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOperator {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

impl CompareOperator {
    pub fn symbol(self) -> &'static str {
        match self {
            CompareOperator::Less => "<",
            CompareOperator::LessEqual => "<=",
            CompareOperator::Greater => ">",
            CompareOperator::GreaterEqual => ">=",
            CompareOperator::Equal => "==",
            CompareOperator::NotEqual => "!=",
        }
    }
}

impl BinaryOperator {
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOperator::Add => "+",
            BinaryOperator::Subtract => "-",
            BinaryOperator::Multiply => "*",
            BinaryOperator::Divide => "/",
        }
    }
}

/// An untyped bench expression. A binary node's location is its operator,
/// a call's its function name.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub location: Location,
    pub kind: ExprKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    /// A number without a unit, as written.
    Number(String),
    /// A number with a unit (`15uA`, `1%`), as written.
    Quantity {
        value: String,
        unit: String,
    },
    /// A param or `let` name.
    Name(String),
    /// `V(a)`, `V(a, b)`, `I(X)`, `I(X.PIN)`; targets as written.
    Probe {
        function: String,
        targets: Vec<String>,
        target_locations: Vec<Location>,
    },
    Call {
        function: String,
        arguments: Vec<Expr>,
    },
    Negate(Box<Expr>),
    Binary {
        operator: BinaryOperator,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// `a < b` and the other comparisons: only the argument of `upto` and
    /// `downto`, and a measure limit's `where`.
    Compare {
        operator: CompareOperator,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

//! The electrical intermediate representation (`pcbir.model`).
//!
//! Data only: the compiler builds it, ERC and the backends read it. Maps keep
//! insertion order like Python dicts (and compare without regard to order, as
//! Python's do); Python frozensets are `BTreeSet`s and are sorted by their
//! text wherever they are written out.

use std::collections::BTreeSet;
use std::fmt;

use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;
use num_bigint::BigInt;

use crate::syntax::{Location, MacroMappingDecl};

/// A Python `str` enum: variants with their `value` text.
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Python's `.value`.
            pub fn value(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }

            /// Python's `Enum(value)`, or `None` for an unknown value.
            pub fn parse(text: &str) -> Option<$name> {
                match text { $($text => Some($name::$variant),)+ _ => None }
            }

            /// Python's error for an unknown value: `"'x' is not a valid Name"`.
            pub fn invalid(text: &str) -> String {
                format!("{} is not a valid {}", copper_json::python_str_repr(text), stringify!($name))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.value())
            }
        }
    };
}

text_enum!(PinType {
    Passive = "passive",
    Input = "input",
    Output = "output",
    Bidirectional = "bidirectional",
    OpenDrain = "open_drain",
    PowerIn = "power_in",
    PowerOut = "power_out",
});

text_enum!(SignalDomain {
    Digital = "digital",
    Analog = "analog",
    Power = "power",
    Ground = "ground",
    Clock = "clock",
    Rf = "rf",
});

text_enum!(Direction { Input = "input", Output = "output", Bidirectional = "bidirectional", Passive = "passive" });

text_enum!(DriveMode { PushPull = "push_pull", OpenDrain = "open_drain", HighImpedance = "high_impedance" });

text_enum!(UnpoweredBehavior {
    Unknown = "unknown",
    HighImpedance = "high_impedance",
    Clamped = "clamped",
    Tolerant = "tolerant",
});

text_enum!(SelectionUsage { Exclusive = "exclusive", FirmwareManaged = "firmware_managed" });

text_enum!(PowerRailState { On = "on", Off = "off", Unknown = "unknown" });

text_enum!(ConnectionPolicy {
    Normal = "normal",
    Required = "required",
    DoNotConnect = "do_not_connect",
    Optional = "optional",
});

text_enum!(ConstraintKind {
    MaxDistance = "max_distance",
    MinDistance = "min_distance",
    PlacementRegion = "placement_region",
    CourtyardGap = "courtyard_gap",
    FixedPlacement = "fixed_placement",
    AllowedOrientations = "allowed_orientations",
    Align = "align",
    PlacementGroup = "placement_group",
    Keepout = "keepout",
    Note = "note",
    Routing = "routing",
    CopperZone = "copper_zone",
    ViaInPad = "via_in_pad",
    LengthMatch = "length_match",
    HoleClearance = "hole_clearance",
});

text_enum!(ConstraintMode { Require = "require", Target = "target", Prefer = "prefer", Assume = "assume", External = "external" });

pub const DIFFERENTIAL_PAIR: &str = "differential_pair";

/// A voltage limit that follows the supply on another pad of the same part,
/// optionally capped by a fixed limit (the tighter one applies).
#[derive(Clone, Debug, PartialEq)]
pub struct RelativeVoltage {
    pub reference: String,
    pub offset: Quantity,
    pub limit: Option<Quantity>,
}

/// `_plain_voltage`: `format(value.normalize(), 'f')` and the unit.
fn plain_voltage(voltage: &Quantity) -> String {
    let value = voltage.value().normalize().expect("a voltage normalizes within the context");
    format!("{} {}", value.to_fixed_string(), voltage.display_unit())
}

impl fmt::Display for RelativeVoltage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = self.reference.clone();
        let offset = self.offset.base_value();
        if !offset.is_zero() {
            let sign = if offset.is_negative() { "-" } else { "+" };
            let magnitude = Quantity::from_base(Dimension::Voltage, offset.abs(), self.offset.display_unit())
                .expect("a voltage offset keeps a voltage unit");
            text = format!("{text}{sign}{}", plain_voltage(&magnitude));
        }
        match &self.limit {
            None => formatter.write_str(&text),
            Some(limit) => write!(formatter, "{text}, {}", plain_voltage(limit)),
        }
    }
}

/// One bound of a quantity range.
#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    Fixed(Quantity),
    Relative(RelativeVoltage),
}

impl Bound {
    pub fn fixed(&self) -> Option<&Quantity> {
        match self {
            Bound::Fixed(quantity) => Some(quantity),
            Bound::Relative(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuantityRange {
    pub minimum: Option<Bound>,
    pub typical: Option<Bound>,
    pub maximum: Option<Bound>,
    pub rating: String,
    pub when: Option<Condition>,
}

impl QuantityRange {
    pub fn new(minimum: Option<Bound>, maximum: Option<Bound>, rating: &str) -> Self {
        QuantityRange { minimum, typical: None, maximum, rating: rating.to_owned(), when: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ElectricalProfile {
    pub domains: BTreeSet<SignalDomain>,
    pub directions: BTreeSet<Direction>,
    pub drive_modes: BTreeSet<DriveMode>,
    pub traits: BTreeSet<String>,
    pub voltage: Option<QuantityRange>,
    pub current: Option<QuantityRange>,
    /// Absolute-maximum ratings; `voltage` stays the operating range.
    pub absolute_voltage: Option<QuantityRange>,
}

impl ElectricalProfile {
    pub fn simple(domains: &[SignalDomain], directions: &[Direction], drive_modes: &[DriveMode]) -> Self {
        ElectricalProfile {
            domains: domains.iter().copied().collect(),
            directions: directions.iter().copied().collect(),
            drive_modes: drive_modes.iter().copied().collect(),
            traits: BTreeSet::new(),
            voltage: None,
            current: None,
            absolute_voltage: None,
        }
    }

    /// Whether this profile provides everything `required` asks for.
    pub fn satisfies(&self, required: &ElectricalProfile) -> bool {
        let mut provided = self.directions.clone();
        if provided.contains(&Direction::Bidirectional) {
            provided.insert(Direction::Input);
            provided.insert(Direction::Output);
        }
        required.domains.is_subset(&self.domains)
            && required.directions.is_subset(&provided)
            && required.drive_modes.is_subset(&self.drive_modes)
            && required.traits.is_subset(&self.traits)
    }
}

/// Mode selections that must all hold (`GROUP=CHOICE`).
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub selections: IndexMap<String, String>,
}

impl Condition {
    pub fn matches(&self, selected: &IndexMap<String, String>) -> bool {
        self.selections.iter().all(|(group, choice)| selected.get(group) == Some(choice))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceReference {
    pub document: Option<String>,
    pub revision: Option<String>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub checksum: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DevicePadDefinition {
    pub name: String,
    pub profile: ElectricalProfile,
    pub power_domain: Option<String>,
    pub unpowered_behavior: UnpoweredBehavior,
    pub when: Option<Condition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BondDefinition {
    pub pad: String,
    pub when: Option<Condition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PackagePinDefinition {
    pub name: String,
    pub number: String,
    pub profile: Option<ElectricalProfile>,
    pub bonds: Vec<BondDefinition>,
    pub connection_policy: ConnectionPolicy,
    pub required_net_traits: BTreeSet<String>,
    /// A manufacturer-permitted external power-output tie for this instance.
    pub parallel_power_group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerDomainDefinition {
    pub name: String,
    pub supply_pads: Vec<String>,
    pub voltage: Option<QuantityRange>,
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralSignalDefinition {
    pub name: String,
    pub profile: ElectricalProfile,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralDefinition {
    pub name: String,
    pub kind: String,
    pub signals: IndexMap<String, PeripheralSignalDefinition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MuxOption {
    pub pad: String,
    pub peripheral: String,
    pub signal: String,
    pub selector: String,
    pub resource: Option<String>,
    pub setting: Option<String>,
    pub when: Option<Condition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalBinding {
    pub pad: String,
    pub profile: Option<ElectricalProfile>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionalUnitDefinition {
    pub name: String,
    pub kind: String,
    pub terminals: IndexMap<String, TerminalBinding>,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignalGroupDefinition {
    pub name: String,
    pub kind: String,
    pub members: IndexMap<String, String>,
    pub profile: Option<ElectricalProfile>,
    pub when: Option<Condition>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModeGroupDefinition {
    pub name: String,
    pub choices: Vec<String>,
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PadSetDefinition {
    pub name: String,
    pub pads: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectorScheme {
    pub kind: String,
    pub parameters: IndexMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteRule {
    pub peripheral: String,
    pub signal: String,
    pub pad_set: String,
    pub selector: SelectorScheme,
    pub when: Option<Condition>,
}

/// Package-independent silicon capabilities and pin-mux choices.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceDefinition {
    pub name: String,
    pub pads: IndexMap<String, DevicePadDefinition>,
    pub peripherals: IndexMap<String, PeripheralDefinition>,
    pub mux_options: Vec<MuxOption>,
    pub power_domains: IndexMap<String, PowerDomainDefinition>,
    pub units: IndexMap<String, FunctionalUnitDefinition>,
    pub signal_groups: IndexMap<String, SignalGroupDefinition>,
    pub mode_groups: IndexMap<String, ModeGroupDefinition>,
    pub pad_sets: IndexMap<String, PadSetDefinition>,
    pub route_rules: Vec<RouteRule>,
    pub resources: Vec<String>,
    pub metadata: IndexMap<String, String>,
    pub source: Option<SourceReference>,
}

/// An always-conductive join of package lands (numbers kept sorted).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct InternalPadGroup {
    pub numbers: Vec<String>,
}

impl InternalPadGroup {
    /// `InternalPadGroup(numbers)` with its validation.
    pub fn new(numbers: Vec<String>) -> Result<Self, String> {
        let invalid = numbers.is_empty()
            || numbers
                .iter()
                .any(|n| n.is_empty() || n.chars().any(|c| copper_json::python_isspace(c) || c == ',' || c == ';'));
        if invalid {
            return Err("internal pad groups require nonempty pad numbers".to_owned());
        }
        let mut sorted = numbers.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != numbers.len() {
            return Err("internal pad group repeats a pad number".to_owned());
        }
        Ok(InternalPadGroup { numbers: sorted })
    }
}

/// `validate_internal_pad_groups`: known numbers, no overlap, sorted.
pub fn validate_internal_pad_groups(
    groups: Vec<InternalPadGroup>,
    known: &[String],
) -> Result<Vec<InternalPadGroup>, String> {
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    for group in &groups {
        if group.numbers.iter().any(|number| !known.contains(number)) {
            return Err("internal pad group references unknown pad number".to_owned());
        }
        if group.numbers.iter().any(|number| seen.contains(number)) {
            return Err("internal pad groups overlap".to_owned());
        }
        seen.extend(group.numbers.iter());
    }
    let mut groups = groups;
    groups.sort();
    Ok(groups)
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartDefinition {
    pub name: String,
    pub pins: IndexMap<String, PackagePinDefinition>,
    pub category: String,
    pub traits: BTreeSet<String>,
    pub footprints: Vec<String>,
    pub manufacturer: Option<String>,
    pub assembled: bool,
    pub device: Option<String>,
    pub source: Option<SourceReference>,
    pub metadata: IndexMap<String, String>,
    pub internal_pad_groups: Vec<InternalPadGroup>,
    pub dc_power_path: Vec<String>,
}

impl PartDefinition {
    pub fn new(name: &str, pins: IndexMap<String, PackagePinDefinition>) -> Self {
        PartDefinition {
            name: name.to_owned(),
            pins,
            category: "component.generic".to_owned(),
            traits: BTreeSet::new(),
            footprints: Vec::new(),
            manufacturer: None,
            assembled: true,
            device: None,
            source: None,
            metadata: IndexMap::new(),
            internal_pad_groups: Vec::new(),
            dc_power_path: Vec::new(),
        }
    }

    /// `PartDefinition.__post_init__`'s `dc_power_path` rule.
    pub fn validate_dc_power_path(&self) -> Result<(), String> {
        let path = &self.dc_power_path;
        if path.is_empty() {
            return Ok(());
        }
        if path.len() != 2 || path[0] == path[1] || path.iter().any(|name| !self.pins.contains_key(name)) {
            return Err("dc_power_path requires two distinct declared pin names".to_owned());
        }
        let passive_power = |name: &String| {
            let pin = &self.pins[name];
            pin.profile.as_ref().is_some_and(|profile| {
                profile.domains == BTreeSet::from([SignalDomain::Power])
                    && profile.directions == BTreeSet::from([Direction::Passive])
            }) && pin.connection_policy != ConnectionPolicy::DoNotConnect
        };
        if self.device.is_some() || !path.iter().all(passive_power) {
            return Err("dc_power_path requires standalone passive power-only pins".to_owned());
        }
        Ok(())
    }
}

/// A component's `value`: a quantity or free text.
#[derive(Clone, Debug, PartialEq)]
pub enum ComponentValue {
    Quantity(Quantity),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentInstance {
    pub reference: String,
    pub part: String,
    pub value: Option<ComponentValue>,
    pub footprint: Option<String>,
    pub properties: IndexMap<String, String>,
    pub modes: IndexMap<String, String>,
}

/// `COMPONENT.PIN`; ordered like the Python dataclass (component, then pin).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Endpoint {
    pub component: String,
    pub pin: String,
}

impl Endpoint {
    pub fn new(component: &str, pin: &str) -> Self {
        Endpoint { component: component.to_owned(), pin: pin.to_owned() }
    }

    /// `Endpoint.parse`: split at the first dot.
    pub fn parse(text: &str) -> Result<Endpoint, String> {
        match text.split_once('.') {
            Some((component, pin)) if !component.is_empty() && !pin.is_empty() => Ok(Endpoint::new(component, pin)),
            _ => Err(format!("endpoint must look like COMPONENT.PIN, got {}", copper_json::python_str_repr(text))),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.component, self.pin)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    pub name: String,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoConnect {
    pub endpoint: Endpoint,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Supply {
    pub name: String,
    pub voltage: Quantity,
    pub net: String,
    pub source: Option<Endpoint>,
    pub externally_driven: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub name: String,
    pub type_name: String,
    pub signals: IndexMap<String, String>,
    pub bindings: IndexMap<String, IndexMap<String, String>>,
    pub pullup_supply: Option<String>,
}

/// A constraint parameter: Python keeps quantities, text, ints, floats and
/// (because `bool` is an `int`) booleans.
#[derive(Clone, Debug, PartialEq)]
pub enum ParameterValue {
    Quantity(Quantity),
    Text(String),
    Int(BigInt),
    Float(f64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub targets: Vec<String>,
    pub parameters: IndexMap<String, ParameterValue>,
    pub constraint_id: Option<String>,
    pub mode: ConstraintMode,
    pub consumers: Vec<String>,
    pub verifier: Option<String>,
    pub origins: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleInstance {
    pub reference: String,
    pub module: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralSignalSelection {
    pub pin: String,
    pub pad: Option<String>,
    pub selector: Option<String>,
    pub resource: Option<String>,
    pub setting: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeripheralSelection {
    pub component: String,
    pub peripheral: String,
    pub name: String,
    pub signals: IndexMap<String, PeripheralSignalSelection>,
    pub usage: SelectionUsage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerState {
    pub name: String,
    pub rails: IndexMap<String, PowerRailState>,
}

/// Resolved source-package provenance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dependency {
    pub import_path: String,
    pub module_path: String,
    pub version: String,
    pub checksum: String,
}

/// A physical macro asset import (`pcbir.macro_bindings.MacroAssetReference`).
#[derive(Clone, Debug, PartialEq)]
pub struct MacroAssetReference {
    pub asset: String,
    pub base_directory: String,
    pub location: Location,
    pub locked: bool,
    pub offline: bool,
    pub symbol: String,
}

/// A source-selected physical realization of existing circuitry.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroBinding {
    pub name: String,
    pub asset: MacroAssetReference,
    pub scope: String,
    pub mappings: Vec<MacroMappingDecl>,
    pub location: Location,
}

/// A reusable hierarchical design with typed boundary ports.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleDefinition {
    pub name: String,
    pub ports: IndexMap<String, PinType>,
    pub library: IndexMap<String, PartDefinition>,
    pub components: Vec<ComponentInstance>,
    pub module_instances: Vec<ModuleInstance>,
    pub nets: Vec<Net>,
    pub supplies: Vec<Supply>,
    pub interfaces: Vec<Interface>,
    pub constraints: Vec<Constraint>,
    pub devices: IndexMap<String, DeviceDefinition>,
    pub peripheral_selections: Vec<PeripheralSelection>,
    pub physical_macros: Vec<MacroBinding>,
    pub no_connects: Vec<NoConnect>,
}

/// The authoritative hierarchical electrical design.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    pub name: String,
    pub library: IndexMap<String, PartDefinition>,
    pub components: Vec<ComponentInstance>,
    pub nets: Vec<Net>,
    pub supplies: Vec<Supply>,
    pub interfaces: Vec<Interface>,
    pub constraints: Vec<Constraint>,
    pub module_instances: Vec<ModuleInstance>,
    pub module_definitions: IndexMap<String, ModuleDefinition>,
    pub dependencies: Vec<Dependency>,
    pub devices: IndexMap<String, DeviceDefinition>,
    pub peripheral_selections: Vec<PeripheralSelection>,
    pub power_states: Vec<PowerState>,
    pub physical_macros: Vec<MacroBinding>,
    pub no_connects: Vec<NoConnect>,
}

/// Module provenance in a flat view.
#[derive(Clone, Debug, PartialEq)]
pub struct ElaboratedModuleInstance {
    pub path: String,
    pub module: String,
    pub ports: IndexMap<String, PinType>,
    pub connections: IndexMap<String, String>,
}

/// The derived flat view ERC and backends use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlatElectricalView {
    pub name: String,
    pub library: IndexMap<String, PartDefinition>,
    pub components: Vec<ComponentInstance>,
    pub nets: Vec<Net>,
    pub supplies: Vec<Supply>,
    pub interfaces: Vec<Interface>,
    pub constraints: Vec<Constraint>,
    pub module_instances: Vec<ElaboratedModuleInstance>,
    pub devices: IndexMap<String, DeviceDefinition>,
    pub peripheral_selections: Vec<PeripheralSelection>,
    pub power_states: Vec<PowerState>,
    pub physical_macros: Vec<MacroBinding>,
    pub net_aliases: IndexMap<String, String>,
    pub no_connects: Vec<NoConnect>,
}

/// A decimal written as Python's `str(Decimal)` would, for messages.
pub fn decimal_str(value: &Decimal) -> String {
    value.to_sci_string()
}

//! The physical board IR (`pcbir.physical`): placements, nets, rules,
//! zones, rigid clusters and hard macros, with their dataclass validation.

use indexmap::IndexMap;

use copper_lang::mechanical::stackup::StackupLayer;
use copper_lang::silkscreen::Artwork;
use copper_physical::footprint::PhysicalFootprint;
use copper_physical::{
    BoardDatum, BoardEdge, BoardOutline, BoardSide, CopperKeepout, CopperLayer, DesignRules, MechanicalHole,
    MechanicalSlot, PlacementKeepout, Point,
};
use copper_units::Decimal;

use crate::{Result, decimal_rem, fail};

/// `DEFAULT_ADDITIONAL_COURTYARD_GAP_NM`.
pub const DEFAULT_ADDITIONAL_COURTYARD_GAP_NM: i64 = 500_000;

/// `PadReference`, ordered by component then pad.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PadReference {
    pub component: String,
    pub pad: String,
}

impl PadReference {
    pub fn new(component: &str, pad: &str) -> Self {
        PadReference { component: component.to_owned(), pad: pad.to_owned() }
    }
}

impl std::fmt::Display for PadReference {
    /// The dataclass `repr`, which messages interpolate with `{}`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "PadReference(component={}, pad={})",
            copper_json::python_str_repr(&self.component),
            copper_json::python_str_repr(&self.pad)
        )
    }
}

/// `Placement`: one component pose.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub reference: String,
    pub footprint: String,
    pub position: Point,
    pub rotation_degrees: Decimal,
    pub side: BoardSide,
    pub value: String,
    pub source_path: Option<String>,
}

impl Placement {
    /// `Placement.__post_init__`: the rotation reduced modulo 360 (dividend's sign).
    pub fn new(reference: &str, footprint: &str, position: Point, rotation: &Decimal) -> Result<Self> {
        if reference.is_empty() {
            return fail("placement reference cannot be empty");
        }
        Ok(Placement {
            reference: reference.to_owned(),
            footprint: footprint.to_owned(),
            position,
            rotation_degrees: decimal_rem(rotation, 360)?,
            side: BoardSide::Front,
            value: String::new(),
            source_path: None,
        })
    }
}

/// `PhysicalNet`.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalNet {
    pub name: String,
    pub pads: Vec<PadReference>,
}

/// `Stackup`: copper order, thickness and the declared physical layers.
#[derive(Clone, Debug, PartialEq)]
pub struct Stackup {
    pub copper_layers: Vec<CopperLayer>,
    pub thickness_nm: i64,
    pub physical_layers: Vec<StackupLayer>,
}

impl Stackup {
    pub fn new(copper_layers: Vec<CopperLayer>) -> Self {
        Stackup { copper_layers, thickness_nm: 1_600_000, physical_layers: Vec::new() }
    }
}

text_enum!(IslandPolicy { RemoveAll = "remove_all", KeepAll = "keep_all", RemoveBelowArea = "remove_below_area" });

/// `ZoneConnection` for zones (the footprint pad enum spells the same values).
pub use copper_physical::footprint::ZoneConnection;

/// `CopperZone` with the default thermal relief (0.3 mm gap and spokes, four spokes).
#[derive(Clone, Debug, PartialEq)]
pub struct CopperZone {
    pub id: String,
    pub net: String,
    pub layers: Vec<CopperLayer>,
    pub outer: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
    pub priority: i64,
    pub clearance_nm: Option<i64>,
    pub minimum_width_nm: i64,
    pub pad_connection: ZoneConnection,
    pub thermal_gap_nm: i64,
    pub thermal_spoke_width_nm: i64,
    pub island_policy: IslandPolicy,
    pub minimum_island_area_nm2: Option<i64>,
    pub reserve_routing: bool,
    pub allow_same_net_hard_macro_overlap: bool,
}

impl CopperZone {
    /// `CopperZone.__post_init__`.
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() || self.net.is_empty() || self.layers.is_empty() {
            return fail("a copper zone requires an id, net, and layer");
        }
        if crate::has_duplicates(&self.layers) {
            return fail("copper zone layers must be unique");
        }
        if self.priority < 0 || self.minimum_width_nm <= 0 {
            return fail("copper zone priority and minimum width are invalid");
        }
        if self.clearance_nm.is_some_and(|clearance| clearance <= 0) {
            return fail("copper zone clearance must be positive");
        }
        if self.island_policy == IslandPolicy::RemoveBelowArea && !self.minimum_island_area_nm2.is_some_and(|a| a > 0) {
            return fail("area-based island removal requires a positive area");
        }
        Ok(())
    }
}

/// `ComponentPlacementRule`, with normalized orientations.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentPlacementRule {
    pub reference: String,
    pub region: Option<String>,
    pub allowed_orientations: Vec<Decimal>,
    pub fixed_position: Option<Point>,
    pub fixed_rotation_degrees: Option<Decimal>,
    pub side: Option<BoardSide>,
    pub priority: i64,
    pub edge_clearance_nm: Option<i64>,
    pub additional_courtyard_gap_nm: Option<i64>,
}

impl ComponentPlacementRule {
    pub fn new(reference: &str) -> Self {
        ComponentPlacementRule {
            reference: reference.to_owned(),
            region: None,
            allowed_orientations: [0, 90, 180, 270].iter().map(|v| Decimal::from_i64(*v)).collect(),
            fixed_position: None,
            fixed_rotation_degrees: None,
            side: None,
            priority: 0,
            edge_clearance_nm: None,
            additional_courtyard_gap_nm: None,
        }
    }

    /// `ComponentPlacementRule.__post_init__`.
    pub fn normalized(mut self) -> Result<Self> {
        if self.reference.is_empty() {
            return fail("component placement rule requires a reference");
        }
        if self.edge_clearance_nm.is_some_and(|value| value <= 0) {
            return fail("component edge clearance must be positive");
        }
        if self.additional_courtyard_gap_nm.is_some_and(|value| value < 0) {
            return fail("additional courtyard gap must be nonnegative integer nanometres");
        }
        let mut orientations: Vec<Decimal> = Vec::new();
        for value in &self.allowed_orientations {
            let value = decimal_rem(value, 360)?;
            if !orientations.contains(&value) {
                orientations.push(value);
            }
        }
        orientations.sort();
        if orientations.is_empty() {
            return fail("component placement rule requires a legal orientation");
        }
        self.allowed_orientations = orientations;
        if let Some(rotation) = &self.fixed_rotation_degrees {
            let rotation = decimal_rem(rotation, 360)?;
            if !self.allowed_orientations.contains(&rotation) {
                return fail("fixed rotation must be one of the allowed orientations");
            }
            self.fixed_rotation_degrees = Some(rotation);
        }
        Ok(self)
    }
}

text_enum!(RouteKind {
    General = "general", Critical = "critical", Differential = "differential", Clock = "clock",
    CanBus = "can_bus", RfFeed = "rf_feed", Power = "power",
});
text_enum!(ReturnViaPolicy { Always = "always", ReferenceChange = "reference_change" });
text_enum!(TuningStyle { Bumps = "bumps", Serpentine = "serpentine" });

/// `NetRoutingRule`: the fields the export and its validation read.
#[derive(Clone, Debug, PartialEq)]
pub struct NetRoutingRule {
    pub net: String,
    pub kind: RouteKind,
    pub priority: i64,
    pub width_nm: Option<i64>,
    pub clearance_nm: Option<i64>,
    pub allowed_layers: Vec<CopperLayer>,
    pub max_vias: Option<i64>,
    pub max_length_nm: Option<i64>,
    pub differential_partner: Option<String>,
    pub pair_gap_nm: Option<i64>,
    pub max_skew_nm: Option<i64>,
    pub topology: String,
    pub target_impedance_ohms: Option<i64>,
    pub maximum_uncoupled_length_nm: Option<i64>,
    pub maximum_stub_length_nm: Option<i64>,
    pub tuning_amplitude_limit_nm: Option<i64>,
    pub require_return_vias: bool,
    pub reserve_corridor: bool,
    pub return_via_net: Option<String>,
    pub maximum_return_via_distance_nm: Option<i64>,
    pub impedance_evidence_digest: Option<String>,
    pub return_via_policy: ReturnViaPolicy,
    pub shared_reference_layer: Option<CopperLayer>,
    pub target_single_ended_ohms: Option<i64>,
    pub impedance_tolerance_percent: Option<Decimal>,
    pub layer_group: Option<String>,
    pub breakout_length_nm: Option<i64>,
    pub breakout_width_nm: Option<i64>,
    pub breakout_gap_nm: Option<i64>,
    pub breakout_clearance_nm: Option<i64>,
    pub tuning_style: TuningStyle,
    pub tuning_spacing_nm: Option<i64>,
    pub tuning_group: Option<String>,
}

impl NetRoutingRule {
    /// `NetRoutingRule.__post_init__` and `_validate_signal_integrity_intent`.
    pub fn validate(&self) -> Result<()> {
        if self.return_via_policy == ReturnViaPolicy::ReferenceChange {
            if !self.require_return_vias || self.shared_reference_layer.is_none() {
                return fail("reference_change requires return vias and an explicit shared reference layer");
            }
        } else if self.shared_reference_layer.is_some() {
            return fail("shared reference layer requires reference_change return-via policy");
        }
        if self.net.is_empty() {
            return fail("routing rule requires a net");
        }
        if self.priority < 0 {
            return fail("routing priority cannot be negative");
        }
        for (name, value) in [
            ("width", self.width_nm),
            ("clearance", self.clearance_nm),
            ("maximum length", self.max_length_nm),
            ("pair gap", self.pair_gap_nm),
            ("maximum skew", self.max_skew_nm),
            ("maximum uncoupled length", self.maximum_uncoupled_length_nm),
            ("maximum stub length", self.maximum_stub_length_nm),
            ("tuning amplitude limit", self.tuning_amplitude_limit_nm),
            ("tuning spacing", self.tuning_spacing_nm),
            ("maximum return via distance", self.maximum_return_via_distance_nm),
        ] {
            if value.is_some_and(|value| value <= 0) {
                return fail(&format!("routing {name} must be positive"));
            }
        }
        if self.max_vias.is_some_and(|value| value < 0) {
            return fail("routing maximum via count cannot be negative");
        }
        if self.tuning_spacing_nm.is_some() && self.tuning_style != TuningStyle::Serpentine {
            return fail("routing tuning spacing requires tuning_style \"serpentine\"");
        }
        let differential = matches!(self.kind, RouteKind::Differential | RouteKind::CanBus);
        if differential {
            if self.differential_partner.is_none() || self.pair_gap_nm.is_none() {
                return fail("differential routing requires a partner net and pair gap");
            }
            if self.differential_partner.as_deref() == Some(self.net.as_str()) {
                return fail("a differential net cannot partner with itself");
            }
        }
        if self.target_impedance_ohms.is_some_and(|value| value <= 0) {
            return fail("target impedance must be positive");
        }
        if self.require_return_vias
            && (self.return_via_net.as_deref().unwrap_or("").is_empty()
                || self.maximum_return_via_distance_nm.is_none())
        {
            return fail("return-via routing requires a net and maximum distance");
        }
        if self.impedance_evidence_digest.as_ref().is_some_and(|digest| digest.chars().count() != 64) {
            return fail("impedance evidence digest must be SHA-256");
        }
        if self.topology.is_empty() {
            return fail("routing topology cannot be empty");
        }
        if let Some(target) = self.target_single_ended_ohms {
            if target <= 0 {
                return fail("target single-ended impedance must be positive");
            }
            if !differential && self.target_impedance_ohms.is_some_and(|value| value != target) {
                return fail("a single-ended rule's target_impedance_ohms and target_single_ended_ohms must agree");
            }
        }
        if let Some(tolerance) = &self.impedance_tolerance_percent {
            if !(*tolerance > Decimal::zero() && *tolerance < Decimal::from_i64(100)) {
                return fail("impedance tolerance must be greater than 0 and below 100 percent");
            }
            if self.target_impedance_ohms.is_none() && self.target_single_ended_ohms.is_none() {
                return fail("impedance tolerance requires target_impedance_ohms or target_single_ended_ohms");
            }
        }
        if self.layer_group.as_ref().is_some_and(|group| crate::py_strip(group).is_empty()) {
            return fail("routing layer group must be a nonempty name");
        }
        if let Some(group) = &self.tuning_group {
            if crate::py_strip(group).is_empty() {
                return fail("routing tuning group must be a nonempty name");
            }
            if self.kind == RouteKind::General {
                return fail("routing tuning group requires a critical routing kind");
            }
        }
        for (name, value) in [
            ("breakout length", self.breakout_length_nm),
            ("breakout width", self.breakout_width_nm),
            ("breakout gap", self.breakout_gap_nm),
            ("breakout clearance", self.breakout_clearance_nm),
        ] {
            if value.is_some_and(|value| value <= 0) {
                return fail(&format!("routing {name} must be positive"));
            }
        }
        let relaxations =
            self.breakout_width_nm.is_some() || self.breakout_gap_nm.is_some() || self.breakout_clearance_nm.is_some();
        if relaxations && self.breakout_length_nm.is_none() {
            return fail("breakout width, gap or clearance requires a positive breakout_length");
        }
        if self.breakout_length_nm.is_some() && !relaxations {
            return fail("breakout_length requires breakout_width, breakout_gap or breakout_clearance");
        }
        if let (Some(breakout), Some(width)) = (self.breakout_width_nm, self.width_nm)
            && breakout > width
        {
            return fail("breakout width may only relax (not exceed) the routing width");
        }
        if let Some(gap) = self.breakout_gap_nm {
            let Some(pair) = self.pair_gap_nm else { return fail("breakout gap requires a differential pair gap") };
            if gap > pair {
                return fail("breakout gap may only relax (not exceed) the pair gap");
            }
        }
        if let (Some(breakout), Some(clearance)) = (self.breakout_clearance_nm, self.clearance_nm)
            && breakout > clearance
        {
            return fail("breakout clearance may only relax (not exceed) the routing clearance");
        }
        Ok(())
    }
}

/// `PadViaInPadRule`.
#[derive(Clone, Debug, PartialEq)]
pub struct PadViaInPadRule {
    pub pad: PadReference,
    pub rows: Option<i64>,
    pub columns: Option<i64>,
    pub pitch_nm: Option<i64>,
}

/// `ComponentHoleClearance`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentHoleClearance {
    pub reference: String,
    pub clearance_nm: i64,
    pub reason: String,
}

/// `ComponentHeight` as the backend reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentHeight {
    pub reference: String,
    pub height_nm: i64,
}

/// `RigidPlacementMember`.
#[derive(Clone, Debug, PartialEq)]
pub struct RigidPlacementMember {
    pub reference: String,
    pub footprint: String,
    pub footprint_digest: String,
    pub position: Point,
    pub rotation_degrees: Decimal,
}

impl RigidPlacementMember {
    pub fn new(
        reference: &str,
        footprint: &str,
        footprint_digest: &str,
        position: Point,
        rotation: &Decimal,
    ) -> Result<Self> {
        if reference.is_empty() || footprint.is_empty() {
            return fail("rigid member requires a reference and footprint");
        }
        if footprint_digest.len() != 64 || !footprint_digest.chars().all(|c| "0123456789abcdef".contains(c)) {
            return fail("rigid member requires a SHA-256 footprint digest");
        }
        Ok(RigidPlacementMember {
            reference: reference.to_owned(),
            footprint: footprint.to_owned(),
            footprint_digest: footprint_digest.to_owned(),
            position,
            rotation_degrees: positive_rem(rotation)?,
        })
    }
}

/// `(value % 360 + 360) % 360`.
pub fn positive_rem(value: &Decimal) -> Result<Decimal> {
    let full = Decimal::from_i64(360);
    let reduced = decimal_rem(value, 360)?.add(&full).map_err(crate::decimal_error)?;
    decimal_rem(&reduced, 360)
}

/// `MacroEdgeOpening`.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroEdgeOpening {
    pub keepout: String,
    pub edge_indices: Vec<i64>,
}

/// `RigidPlacementCluster`.
#[derive(Clone, Debug, PartialEq)]
pub struct RigidPlacementCluster {
    pub name: String,
    pub anchor_reference: String,
    pub anchor_pad: Option<String>,
    pub members: Vec<RigidPlacementMember>,
    pub source: String,
    pub allowed_rotations: Vec<Decimal>,
    pub keepouts: Vec<CopperKeepout>,
    pub internal_clearance_nm: Option<i64>,
    pub edge_openings: Vec<MacroEdgeOpening>,
}

impl RigidPlacementCluster {
    /// `RigidPlacementCluster.__post_init__`.
    pub fn normalized(mut self) -> Result<Self> {
        let references: Vec<&str> = self.members.iter().map(|m| m.reference.as_str()).collect();
        if self.name.is_empty() || self.source.is_empty() || references.is_empty() {
            return fail("rigid cluster requires a name, evidence source and at least one member");
        }
        if crate::has_duplicates(&references) || !references.contains(&self.anchor_reference.as_str()) {
            return fail("rigid cluster requires unique members including its anchor");
        }
        if self.allowed_rotations.is_empty() {
            return fail("rigid cluster requires finite allowed rotations");
        }
        let mut rotations: Vec<Decimal> = Vec::new();
        for rotation in &self.allowed_rotations {
            let value = positive_rem(rotation)?;
            if !rotations.contains(&value) {
                rotations.push(value);
            }
        }
        rotations.sort();
        self.allowed_rotations = rotations;
        let ids: Vec<&str> = self.keepouts.iter().map(|k| k.id.as_str()).collect();
        if crate::has_duplicates(&ids) {
            return fail("rigid cluster keepout ids must be unique");
        }
        let openings: Vec<&str> = self.edge_openings.iter().map(|o| o.keepout.as_str()).collect();
        if crate::has_duplicates(&openings) {
            return fail("duplicate macro edge opening keepout");
        }
        for opening in &self.edge_openings {
            let region = self.keepouts.iter().find(|k| k.id == opening.keepout);
            if region.is_none_or(|region| {
                !region.block_zones || opening.edge_indices.iter().any(|i| *i as usize >= region.vertices.len())
            }) {
                return fail("macro edge opening requires a zone-blocking simple keepout and valid edges");
            }
        }
        if self.internal_clearance_nm.is_some_and(|value| value < 0) {
            return fail("rigid cluster internal courtyard clearance cannot be negative");
        }
        Ok(self)
    }
}

/// `TrackSegment`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrackSegment {
    pub net: String,
    pub start: Point,
    pub end: Point,
    pub width_nm: i64,
    pub layer: CopperLayer,
}

/// `Via`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Via {
    pub net: String,
    pub position: Point,
    pub size_nm: i64,
    pub drill_nm: i64,
    pub from_layer: CopperLayer,
    pub to_layer: CopperLayer,
    pub technology: Option<String>,
    pub finish: String,
}

/// `CopperPolygon`.
#[derive(Clone, Debug, PartialEq)]
pub struct CopperPolygon {
    pub id: String,
    pub net: String,
    pub layer: CopperLayer,
    pub vertices: Vec<Point>,
}

/// `MacroPort`.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroPort {
    pub name: String,
    pub net: String,
    pub position: Point,
    pub layer: CopperLayer,
    pub pads: Vec<PadReference>,
}

/// `MacroPlaneReturn`.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroPlaneReturn {
    pub net: String,
    pub layers: Vec<CopperLayer>,
    pub pads: Vec<PadReference>,
    pub dedicated_contacts: Vec<(PadReference, Point)>,
}

/// `MacroTrackWidthContract`: an evidence-bearing minimum for one local track segment.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroTrackWidthContract {
    pub track_index: usize,
    pub minimum_width_nm: i64,
    pub purpose: String,
    pub evidence: String,
}

/// `PhysicalHardMacro`: immutable owner copper bound to a rigid cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalHardMacro {
    pub cluster: String,
    pub asset_sha256: String,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
    pub ports: Vec<MacroPort>,
    pub protected_regions: Vec<CopperKeepout>,
    pub required_layers: Vec<CopperLayer>,
    pub pad_bindings: Vec<(PadReference, String)>,
    pub isolated_pads: Vec<PadReference>,
    pub zones: Vec<CopperZone>,
    pub plane_returns: Vec<MacroPlaneReturn>,
    pub polygons: Vec<CopperPolygon>,
    pub width_contracts: Vec<MacroTrackWidthContract>,
}

impl PhysicalHardMacro {
    /// `PhysicalHardMacro.__post_init__`.
    pub fn validate(&self) -> Result<()> {
        if self.cluster.is_empty()
            || self.asset_sha256.len() != 64
            || !self.asset_sha256.chars().all(|c| "0123456789abcdef".contains(c))
        {
            return fail("hard macro requires a cluster and SHA-256 asset identity");
        }
        if self.tracks.is_empty() && self.vias.is_empty() && self.polygons.is_empty() {
            return fail("hard macro requires explicit copper");
        }
        let unique = |ids: Vec<&str>| !crate::has_duplicates(&ids);
        if !unique(self.ports.iter().map(|p| p.name.as_str()).collect()) {
            return fail("hard macro port names must be unique");
        }
        if !unique(self.protected_regions.iter().map(|r| r.id.as_str()).collect()) {
            return fail("hard macro protected region IDs must be unique");
        }
        if !unique(self.zones.iter().map(|z| z.id.as_str()).collect()) {
            return fail("hard macro zone IDs must be unique");
        }
        if !unique(self.polygons.iter().map(|p| p.id.as_str()).collect()) {
            return fail("hard macro polygon IDs must be unique");
        }
        if self.protected_regions.iter().any(|r| !r.block_tracks || !r.block_vias) {
            return fail("macro access reservations require solid track/via-blocking polygons");
        }
        let indexes: Vec<usize> = self.width_contracts.iter().map(|c| c.track_index).collect();
        if crate::has_duplicates(&indexes) {
            return fail("duplicate macro width contract track index");
        }
        for contract in &self.width_contracts {
            let Some(track) = self.tracks.get(contract.track_index) else {
                return fail("macro width contract track index is out of range");
            };
            if track.width_nm < contract.minimum_width_nm {
                return fail("macro track is narrower than its width contract");
            }
            if self.tracks.iter().filter(|t| *t == track).count() != 1 {
                return fail("ambiguous duplicate macro width contract geometry");
            }
        }
        Ok(())
    }
}

/// `PlacementRegion`: a named area some components must stay inside.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementRegion {
    pub name: String,
    pub outline: BoardOutline,
    pub side: Option<BoardSide>,
}

text_enum!(RelativePlacementKind { MaxDistance = "max_distance", MinDistance = "min_distance", Align = "align" });
text_enum!(AlignmentAxis { X = "x", Y = "y" });

/// `PlacementTarget`: a component, or one of its pads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlacementTarget {
    pub reference: String,
    pub pad: Option<String>,
}

/// `RelativePlacementRule`.
#[derive(Clone, Debug, PartialEq)]
pub struct RelativePlacementRule {
    pub kind: RelativePlacementKind,
    pub targets: Vec<PlacementTarget>,
    pub distance_nm: Option<i64>,
    pub axis: Option<AlignmentAxis>,
    pub tolerance_nm: i64,
    pub weight: i64,
}

/// `NetMatchGroup`: nets whose routed lengths agree within `max_skew_nm`.
#[derive(Clone, Debug, PartialEq)]
pub struct NetMatchGroup {
    pub id: String,
    pub nets: Vec<String>,
    pub max_skew_nm: i64,
}

/// `AssemblyAccess` with its component reference resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyAccess {
    pub id: String,
    pub reference: String,
    pub outline: BoardOutline,
    pub side: String,
    pub purpose: String,
}

/// `PlacementGroup`: components the planner keeps together.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacementGroup {
    pub name: String,
    pub references: Vec<String>,
    pub anchor: Option<String>,
    pub priority: i64,
    pub source: String,
}

impl PlacementGroup {
    /// `PlacementGroup.__post_init__`: members deduplicated in order.
    pub fn new(name: &str, references: &[String], anchor: Option<&str>, priority: i64, source: &str) -> Result<Self> {
        let mut unique: Vec<String> = Vec::new();
        for reference in references {
            if !unique.contains(reference) {
                unique.push(reference.clone());
            }
        }
        if name.is_empty() || unique.is_empty() {
            return fail("placement group requires a name and members");
        }
        if anchor.is_some_and(|anchor| !unique.iter().any(|r| r == anchor)) {
            return fail("placement group anchor must be a member");
        }
        Ok(PlacementGroup {
            name: name.to_owned(),
            references: unique,
            anchor: anchor.map(str::to_owned),
            priority,
            source: source.to_owned(),
        })
    }
}

/// `PhysicalPowerDomain`: derived rail membership; never new connectivity.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalPowerDomain {
    pub net: String,
    pub members: Vec<PadReference>,
    pub sources: Vec<PadReference>,
    pub source_ports: Vec<String>,
}

/// `PowerFeedBoundary`: a directed, source-backed passive DC boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerFeedBoundary {
    pub net: String,
    pub terminal: PadReference,
    pub upstream_net: String,
    pub upstream_terminal: PadReference,
    pub provenance: String,
}

/// `DecouplingLink`: a semantic bypass association.
#[derive(Clone, Debug, PartialEq)]
pub struct DecouplingLink {
    pub capacitor: PadReference,
    pub target: PadReference,
    pub return_pad: PadReference,
    pub net: String,
    pub return_net: String,
}

/// `PhysicalAttachment`: a component pose bound to a mechanical target.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalAttachment {
    pub id: String,
    pub reference: String,
    pub target: Option<String>,
    pub position: Point,
    pub offset: Point,
    pub anchor: String,
    pub anchor_point: Point,
    pub rotation: Decimal,
    pub side: BoardSide,
}

/// `PhysicalBoard`: the fields this export reads or validates.
#[derive(Clone, Debug)]
pub struct PhysicalBoard {
    pub name: String,
    pub outline: BoardOutline,
    pub footprints: IndexMap<String, PhysicalFootprint>,
    pub placements: Vec<Placement>,
    pub nets: Vec<PhysicalNet>,
    pub stackup: Stackup,
    pub rules: DesignRules,
    pub tracks: Vec<TrackSegment>,
    pub vias: Vec<Via>,
    pub metadata: IndexMap<String, String>,
    pub regions: Vec<PlacementRegion>,
    pub keepouts: Vec<PlacementKeepout>,
    pub placement_rules: Vec<ComponentPlacementRule>,
    pub relative_rules: Vec<RelativePlacementRule>,
    pub placement_groups: Vec<PlacementGroup>,
    pub power_domains: Vec<PhysicalPowerDomain>,
    pub net_routing_rules: Vec<NetRoutingRule>,
    pub zones: Vec<CopperZone>,
    pub polygons: Vec<CopperPolygon>,
    pub copper_keepouts: Vec<CopperKeepout>,
    pub rigid_clusters: Vec<RigidPlacementCluster>,
    pub hard_macros: Vec<PhysicalHardMacro>,
    pub materialized_macros: Vec<String>,
    pub via_in_pad_rules: Vec<PadViaInPadRule>,
    pub mechanical_holes: Vec<MechanicalHole>,
    pub mechanical_slots: Vec<MechanicalSlot>,
    pub datums: Vec<BoardDatum>,
    pub boundary_edges: Vec<BoardEdge>,
    pub attachments: Vec<PhysicalAttachment>,
    pub component_heights: Vec<ComponentHeight>,
    pub body_overhangs: Vec<copper_lang::mechanical::assembly::BodyOverhang>,
    pub assembly_envelopes: Vec<copper_lang::mechanical::assembly::AssemblyEnvelope>,
    pub assembly_access: Vec<AssemblyAccess>,
    pub match_groups: Vec<NetMatchGroup>,
    pub component_hole_clearances: Vec<ComponentHoleClearance>,
    pub decoupling_links: Vec<DecouplingLink>,
    pub silkscreen: Vec<Artwork>,
    pub power_feeds: Vec<PowerFeedBoundary>,
    pub mechanical_references: Vec<copper_lang::mechanical::MechanicalReference>,
}

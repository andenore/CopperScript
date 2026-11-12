//! `copper emi-check`: screen the physical layout of switching cells for
//! noise sources (`docs/emi-noise-analysis.md` section 8).
//!
//! The derived model (`copper_lang::emi::EmiModel`) says who switches and which
//! nets are aggressors or victims; this module measures the placed board:
//!
//! * `EMI-HOT-LOOP`: the pulsed-current loop through the hot capacitor, the
//!   controller's hot-supply pin, the switch pin, the catch diode (or the hot
//!   return pin) and back, as a polygon through pad centres (enclosed area,
//!   path length, partial-inductance estimate over the nearest ground plane).
//! * `EMI-SWITCH-NODE`: copper area of the switch net, capacitance to the
//!   nearest ground plane, distance to the board edge and to cable connectors.
//! * `EMI-PROXIMITY`: switch node and magnetic against sensitive nets and
//!   cable connectors, in units of the inductor size.
//! * `emi_accept` handling and `EMI-ACCEPT-UNUSED`.
//!
//! Every number is screening evidence with the stated uncertainty
//! ([`UNCERTAINTY_DB`]); nothing is a pass or a fail, and a check that cannot
//! run on the given input says so in the coverage list. The geometry is `f64`
//! metres converted from the board's integer nanometres at the boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use copper_emi::estimate::{self, limits};
use copper_emi::inductance::{self, Segment};
use copper_json::{Dumps, Object, Value};
use copper_lang::emi::{EmiModel, HotCapacitor, Provenance, SwitchingCell};
use copper_lang::model::{
    ComponentInstance, ConstraintKind, Endpoint, FlatElectricalView, ParameterValue, Shielding, SwitchingFrequency,
    Topology,
};
use copper_pcb::board::*;
use copper_pcb::physicalize::physical_pin_number;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::{BoardSide, CopperLayer};
use copper_units::Quantity;

use crate::emi_trace::{StepKind, TraceArc, TraceCopper, TraceGraph, TracePad, TracePolygon, TraceTrack, TraceVia};
use crate::signal_integrity::copper_layer_depths_nm;
use crate::{Coverage, CoverageStatus, Finding, Severity};

mod conducted;
pub mod defaults;
mod field;

/// The `schema` of [`EmiCheckReport::json`].
pub const SCHEMA: &str = "copperscript-emi-check/v0.1";

/// Stated uncertainty of every absolute level, dB. The closed-form radiation
/// estimators are good to 10 to 20 dB; the middle of that range is used for
/// every margin.
pub const UNCERTAINTY_DB: f64 = 15.0;

/// Relative permittivity assumed when the stack-up declares none (FR-4).
const DEFAULT_ER: f64 = 4.5;
/// Outer copper thickness assumed when the stack-up declares none, metres (1 oz).
const DEFAULT_COPPER: f64 = 35e-6;
/// Duty cycle assumed when the cell's input and output voltages are not both declared.
const DEFAULT_DUTY: f64 = 0.5;
/// Lowest and highest frequency of the radiated sweep, Hz (the covered range of the limit tables).
const BAND: (f64, f64) = (30e6, 1e9);
/// Measurement distance when no `emission` standard names one, metres.
const DEFAULT_DISTANCE: f64 = 3.0;
/// The loop is treated as electrically small below `lambda / SMALL_LOOP_FRACTION`.
const SMALL_LOOP_FRACTION: f64 = 10.0;
/// Nominal trace width bounds for the straight loop segments, metres.
const WIDTH_BOUNDS: (f64, f64) = (0.2e-3, 1.0e-3);
/// Screening distance of the switch-node pads, in units of the inductor size.
const SWITCH_NODE_FACTOR: f64 = 1.5;

fn mm(metres: f64) -> f64 {
    metres * 1e3
}

fn nm_to_m(nm: i64) -> f64 {
    nm as f64 * 1e-9
}

fn rounded(value: f64, digits: u32) -> f64 {
    if value.is_finite() { copper_physical::pyfloat::round_digits(value, digits) } else { value }
}

fn quantity(value: &Quantity) -> f64 {
    value.base_value().to_sci_string().parse().unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// The primary numeric estimate of a finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub quantity: String,
    pub value: f64,
    pub unit: String,
    pub method: String,
    pub uncertainty_db: f64,
    /// The frequency the level was evaluated at, Hz.
    pub frequency_hz: Option<f64>,
    /// `[low, high]` level in the estimate's unit when an input is a rule of thumb: `value` is the
    /// nominal, `high` the unfavourable end (the one a margin is computed from).
    pub band_db: Option<[f64; 2]>,
    /// `rule_of_thumb` when the estimate rests on a default band.
    pub basis: Option<String>,
}

/// A [`Finding`] with the EMI additions of the spec: cell, provenance,
/// estimate, margin and acceptance.
#[derive(Clone, Debug)]
pub struct EmiFinding {
    pub finding: Finding,
    pub cell: Option<String>,
    pub provenance: Option<Provenance>,
    /// `placement_estimate`, `pads_only`, `inferred`, `provisional_placement`.
    pub tags: Vec<String>,
    pub estimate: Option<Estimate>,
    /// Limit minus level at the evaluated frequency (positive: below the line);
    /// only with an `emission` constraint whose standard has a table.
    pub margin_db: Option<f64>,
    /// The `emi_accept` reason when a constraint acknowledges the finding.
    pub accepted: Option<String>,
    /// Further measured numbers, by name.
    pub measures: Object,
    /// Components and `component.pin` endpoints the finding concerns, for `emi_accept` matching.
    pub subjects: Vec<String>,
    /// Geometric rank measure (larger is worse), used after the dB estimate.
    pub rank: f64,
}

/// One switching cell as analysed.
#[derive(Clone, Debug)]
pub struct CellSummary {
    pub controller: String,
    /// The block heading of the human output.
    pub heading: String,
    pub value: Value,
}

/// Standard, band and caveats of the estimates.
#[derive(Clone, Debug)]
pub struct Limits {
    pub uncertainty_db: f64,
    pub standard: Option<String>,
    pub limit_source: Option<String>,
    pub distance_m: f64,
    pub band_hz: (f64, f64),
    pub notes: Vec<String>,
}

/// The result of [`emi_check`].
#[derive(Clone, Debug)]
pub struct EmiCheckReport {
    pub board: String,
    pub cells: Vec<CellSummary>,
    pub findings: Vec<EmiFinding>,
    pub coverage: Vec<Coverage>,
    pub limits: Limits,
}

// ---------------------------------------------------------------------------
// Plane geometry
// ---------------------------------------------------------------------------

type P2 = [f64; 2];

fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Absolute area of the polygon through `points` (shoelace), m^2.
pub fn polygon_area(points: &[P2]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let twice: f64 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    twice.abs() / 2.0
}

/// Perimeter of the closed polygon through `points`, m.
pub fn closed_length(points: &[P2]) -> f64 {
    let n = points.len();
    if n < 2 {
        return 0.0;
    }
    (0..n).map(|i| dist(points[i], points[(i + 1) % n])).sum()
}

fn point_segment(p: P2, a: P2, b: P2) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length_squared).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

fn cross(o: P2, a: P2, b: P2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn segments_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

fn segment_distance(a: P2, b: P2, c: P2, d: P2) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    point_segment(a, c, d).min(point_segment(b, c, d)).min(point_segment(c, a, b)).min(point_segment(d, a, b))
}

/// A convex pad or track outline: a point, segment or polygon (the spine)
/// swept by `radius`, in metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub spine: Vec<P2>,
    pub radius: f64,
}

impl Shape {
    fn from_nm(shape: &copper_physical::RoundedConvexShape) -> Self {
        Shape {
            spine: shape.spine.iter().map(|p| [nm_to_m(p.x_nm), nm_to_m(p.y_nm)]).collect(),
            radius: nm_to_m(shape.radius_nm),
        }
    }

    /// A track segment of `width`.
    pub fn track(a: P2, b: P2, width: f64) -> Self {
        Shape { spine: vec![a, b], radius: width / 2.0 }
    }

    fn edges(&self) -> Vec<(P2, P2)> {
        match self.spine.len() {
            0 => Vec::new(),
            1 => vec![(self.spine[0], self.spine[0])],
            2 => vec![(self.spine[0], self.spine[1])],
            n => (0..n).map(|i| (self.spine[i], self.spine[(i + 1) % n])).collect(),
        }
    }

    /// Centre of the spine.
    pub fn center(&self) -> P2 {
        let n = self.spine.len().max(1) as f64;
        [self.spine.iter().map(|p| p[0]).sum::<f64>() / n, self.spine.iter().map(|p| p[1]).sum::<f64>() / n]
    }

    fn spine_perimeter(&self) -> f64 {
        match self.spine.len() {
            0 | 1 => 0.0,
            2 => 2.0 * dist(self.spine[0], self.spine[1]),
            _ => closed_length(&self.spine),
        }
    }

    /// Copper area, m^2: the spine polygon swept by the radius.
    pub fn area(&self) -> f64 {
        polygon_area(&self.spine) + self.spine_perimeter() * self.radius + PI * self.radius * self.radius
    }

    /// Outline length, m.
    pub fn perimeter(&self) -> f64 {
        self.spine_perimeter() + 2.0 * PI * self.radius
    }

    /// The shorter side, m (the nominal trace width of a pad).
    pub fn small_dimension(&self) -> f64 {
        let side = match self.spine.len() {
            0..=2 => 0.0,
            _ => self.edges().iter().map(|(a, b)| dist(*a, *b)).fold(f64::INFINITY, f64::min),
        };
        side + 2.0 * self.radius
    }
}

/// Clearance between two shapes, m (0 when they touch or overlap).
pub fn shape_gap(a: &Shape, b: &Shape) -> f64 {
    let mut best = f64::INFINITY;
    for (p, q) in a.edges() {
        for (r, s) in b.edges() {
            best = best.min(segment_distance(p, q, r, s));
        }
    }
    (best - a.radius - b.radius).max(0.0)
}

/// Clearance from `shape` to the closed ring `ring`, m.
pub fn ring_gap(shape: &Shape, ring: &[P2]) -> f64 {
    let n = ring.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (c, d) = (ring[i], ring[(i + 1) % n]);
        for (p, q) in shape.edges() {
            best = best.min(segment_distance(p, q, c, d));
        }
    }
    (best - shape.radius).max(0.0)
}

/// The cheapest closed path through one candidate of every group, in group
/// order, by dynamic programming over the groups with the first candidate
/// fixed in turn. `None` for fewer than three groups or an empty group.
pub fn shortest_cycle(groups: &[Vec<P2>]) -> Option<Vec<usize>> {
    let n = groups.len();
    if n < 3 || groups.iter().any(Vec::is_empty) {
        return None;
    }
    let mut best: Option<(f64, Vec<usize>)> = None;
    for start in 0..groups[0].len() {
        let mut cost: Vec<Vec<(f64, usize)>> = vec![
            groups[0].iter().enumerate().map(|(i, _)| (if i == start { 0.0 } else { f64::INFINITY }, 0)).collect(),
        ];
        for i in 1..n {
            let row = groups[i]
                .iter()
                .map(|&to| {
                    groups[i - 1]
                        .iter()
                        .enumerate()
                        .map(|(k, &from)| (cost[i - 1][k].0 + dist(from, to), k))
                        .fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a })
                })
                .collect();
            cost.push(row);
        }
        let closing = groups[n - 1]
            .iter()
            .enumerate()
            .map(|(j, &p)| (cost[n - 1][j].0 + dist(p, groups[0][start]), j))
            .fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a });
        if !closing.0.is_finite() {
            continue;
        }
        let mut path = vec![0; n];
        path[n - 1] = closing.1;
        for i in (1..n).rev() {
            path[i - 1] = cost[i][path[i]].1;
        }
        if best.as_ref().is_none_or(|(c, _)| closing.0 < *c) {
            best = Some((closing.0, path));
        }
    }
    best.map(|(_, path)| path)
}

// ---------------------------------------------------------------------------
// Board access
// ---------------------------------------------------------------------------

/// A placed pad: its footprint pad number, outline and centre in metres.
#[derive(Clone, Debug)]
pub struct PadGeom {
    pub component: String,
    pub number: String,
    pub shape: Shape,
    /// The placed outline in integer nanometres.
    pub raw: copper_physical::RoundedConvexShape,
    /// Copper layers the pad exists on.
    pub layers: Vec<CopperLayer>,
    /// Hole diameter of a through-hole pad, nm.
    pub drill_nm: Option<i64>,
}

/// How firmly the placement of some components is fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlacementState {
    /// Every component has a fixed position (`fixed_placement`) or follows a fixed anchor.
    Fixed,
    /// The components sit in one rigid macro cluster: relative geometry is fixed, the cluster's own position is not.
    ClusterRelative,
    /// At least one position is the prototype placement.
    Provisional,
}

impl PlacementState {
    fn value(self) -> &'static str {
        match self {
            PlacementState::Fixed => "fixed",
            PlacementState::ClusterRelative => "cluster_relative",
            PlacementState::Provisional => "provisional",
        }
    }
}

struct Layout<'a> {
    board: &'a PhysicalBoard,
    view: &'a FlatElectricalView,
    /// Placements with rigid clusters moved to the poses their anchor prescribes.
    poses: indexmap::IndexMap<String, Placement>,
    components: BTreeMap<&'a str, &'a ComponentInstance>,
    nets: BTreeMap<&'a str, &'a PhysicalNet>,
    fixed: BTreeSet<&'a str>,
    cluster_of: BTreeMap<&'a str, &'a str>,
    anchors: BTreeMap<&'a str, &'a str>,
    /// The net of each `(component, pad number)`.
    pad_nets: BTreeMap<(&'a str, &'a str), &'a str>,
}

impl<'a> Layout<'a> {
    fn new(board: &'a PhysicalBoard, view: &'a FlatElectricalView) -> Self {
        let fixed: BTreeSet<&str> = board
            .placement_rules
            .iter()
            .filter(|rule| rule.fixed_position.is_some())
            .map(|rule| rule.reference.as_str())
            .collect();
        let mut poses: indexmap::IndexMap<String, Placement> =
            board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
        let (mut cluster_of, mut anchors) = (BTreeMap::new(), BTreeMap::new());
        for cluster in &board.rigid_clusters {
            anchors.insert(cluster.name.as_str(), cluster.anchor_reference.as_str());
            for member in &cluster.members {
                cluster_of.insert(member.reference.as_str(), cluster.name.as_str());
            }
            // Members follow the anchor; when the prescribed poses cannot be computed the
            // raw placements stay and the cluster counts as provisional.
            if let Some(anchor) = poses.get(&cluster.anchor_reference)
                && let Ok(moved) = copper_pcb::hard_macros::cluster_placements(cluster, anchor, &poses)
            {
                poses.extend(moved);
            }
        }
        Layout {
            board,
            view,
            poses,
            components: view.components.iter().map(|c| (c.reference.as_str(), c)).collect(),
            nets: board.nets.iter().map(|n| (n.name.as_str(), n)).collect(),
            fixed,
            cluster_of,
            anchors,
            pad_nets: board
                .nets
                .iter()
                .flat_map(|net| {
                    net.pads.iter().map(move |pad| ((pad.component.as_str(), pad.pad.as_str()), net.name.as_str()))
                })
                .collect(),
        }
    }

    fn net_of_pad(&self, component: &str, number: &str) -> Option<String> {
        self.pad_nets.get(&(component, number)).map(|net| (*net).to_owned())
    }

    fn absolutely_fixed(&self, component: &str) -> bool {
        self.fixed.contains(component)
            || self
                .cluster_of
                .get(component)
                .and_then(|cluster| self.anchors.get(cluster))
                .is_some_and(|anchor| self.fixed.contains(anchor))
    }

    fn placement_state(&self, components: &[&str]) -> PlacementState {
        if components.iter().all(|c| self.absolutely_fixed(c)) {
            return PlacementState::Fixed;
        }
        let clusters: BTreeSet<Option<&&str>> = components.iter().map(|c| self.cluster_of.get(*c)).collect();
        if clusters.len() == 1 && clusters.iter().all(Option::is_some) {
            return PlacementState::ClusterRelative;
        }
        PlacementState::Provisional
    }

    fn pads_numbered(&self, component: &str, number: &str) -> Vec<PadGeom> {
        let Some(placement) = self.poses.get(component) else { return Vec::new() };
        let Some(footprint) = self.board.footprints.get(&placement.footprint) else { return Vec::new() };
        footprint
            .pads
            .iter()
            .filter(|pad| pad.number == number)
            .map(|pad| {
                let at = transformed_local_point(placement, pad.position);
                let raw = placed_pad_shape(at, pad, placement);
                let layers = if pad.kind == copper_physical::footprint::PadKind::Smd {
                    vec![copper_layer(placement.side)]
                } else {
                    self.board.stackup.copper_layers.clone()
                };
                PadGeom {
                    component: component.to_owned(),
                    number: number.to_owned(),
                    shape: Shape::from_nm(&raw),
                    raw,
                    layers,
                    drill_nm: pad.drill.map(|d| d.width_nm.min(d.height_nm)),
                }
            })
            .collect()
    }

    fn pads_of_endpoint(&self, endpoint: &Endpoint) -> Vec<PadGeom> {
        let Some(component) = self.components.get(endpoint.component.as_str()) else { return Vec::new() };
        let Some(part) = self.view.library.get(&component.part) else { return Vec::new() };
        let Some(number) = physical_pin_number(component, part, &self.view.devices, &endpoint.pin) else {
            return Vec::new();
        };
        self.pads_numbered(&endpoint.component, &number)
    }

    fn pads_of_component(&self, component: &str) -> Vec<PadGeom> {
        let Some(placement) = self.poses.get(component) else { return Vec::new() };
        let Some(footprint) = self.board.footprints.get(&placement.footprint) else { return Vec::new() };
        let numbers: BTreeSet<&str> =
            footprint.pads.iter().filter(|pad| pad.kind.is_electrical()).map(|pad| pad.number.as_str()).collect();
        numbers.into_iter().flat_map(|number| self.pads_numbered(component, number)).collect()
    }

    fn pads_of_net(&self, net: &str) -> Vec<PadGeom> {
        let Some(net) = self.nets.get(net) else { return Vec::new() };
        net.pads.iter().flat_map(|pad| self.pads_numbered(&pad.component, &pad.pad)).collect()
    }

    fn side(&self, component: &str) -> Option<BoardSide> {
        self.poses.get(component).map(|p| p.side)
    }

    /// The larger of the component's body and pad-extent dimensions, m.
    fn size_of(&self, component: &str) -> Option<f64> {
        let placement = self.poses.get(component)?;
        let footprint = self.board.footprints.get(&placement.footprint)?;
        let body = nm_to_m(footprint.body_size.width_nm.max(footprint.body_size.height_nm));
        let pads = self.pads_of_component(component);
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for pad in &pads {
            for p in &pad.shape.spine {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k] - pad.shape.radius);
                    hi[k] = hi[k].max(p[k] + pad.shape.radius);
                }
            }
        }
        let extent = if pads.is_empty() { 0.0 } else { (hi[0] - lo[0]).max(hi[1] - lo[1]) };
        Some(body.max(extent)).filter(|size| *size > 0.0)
    }

    fn ring(&self) -> Vec<P2> {
        self.board.outline.vertices.iter().map(|p| [nm_to_m(p.x_nm), nm_to_m(p.y_nm)]).collect()
    }
}

fn copper_layer(side: BoardSide) -> CopperLayer {
    match side {
        BoardSide::Front => CopperLayer::Front,
        BoardSide::Back => CopperLayer::Back,
    }
}

/// Copper on the given nets present in the board: tracks, arcs, vias,
/// polygons and zones, including the copper of hard macros that are not yet
/// materialized (moved from their cluster frame to the pose the anchor gives).
#[derive(Default)]
struct NetCopper {
    tracks: Vec<TrackSegment>,
    arcs: Vec<TrackArc>,
    vias: Vec<Via>,
    polygons: Vec<CopperPolygon>,
    zones: Vec<CopperZone>,
}

impl NetCopper {
    fn collect(layout: &Layout, nets: &[&str]) -> Self {
        let board = layout.board;
        let on = |net: &str| nets.contains(&net);
        let mut copper = NetCopper::default();
        copper.tracks.extend(board.tracks.iter().filter(|t| on(&t.net)).cloned());
        copper.arcs.extend(board.arcs.iter().filter(|t| on(&t.net)).cloned());
        copper.vias.extend(board.vias.iter().filter(|t| on(&t.net)).cloned());
        copper.polygons.extend(board.polygons.iter().filter(|t| on(&t.net)).cloned());
        copper.zones.extend(board.zones.iter().filter(|t| on(&t.net)).cloned());
        for hard in &board.hard_macros {
            // A materialized macro's copper is already part of the board's own.
            let placed = board
                .rigid_clusters
                .iter()
                .find(|c| c.name == hard.cluster)
                .is_some_and(|cluster| layout.poses.contains_key(&cluster.anchor_reference));
            if board.materialized_macros.contains(&hard.cluster) || !placed {
                continue;
            }
            use copper_pcb::hard_macros::{resolved_macro_geometry, resolved_macro_polygons, resolved_macro_zones};
            if let Ok(geometry) = resolved_macro_geometry(board, hard, &layout.poses) {
                copper.tracks.extend(geometry.tracks.into_iter().filter(|t| on(&t.net)));
                copper.vias.extend(geometry.vias.into_iter().filter(|t| on(&t.net)));
            }
            if let Ok(polygons) = resolved_macro_polygons(board, hard, &layout.poses) {
                copper.polygons.extend(polygons.into_iter().filter(|t| on(&t.net)));
            }
            if let Ok(zones) = resolved_macro_zones(board, hard, &layout.poses) {
                copper.zones.extend(zones.into_iter().filter(|t| on(&t.net)));
            }
        }
        copper
    }

    fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.arcs.is_empty()
            && self.vias.is_empty()
            && self.polygons.is_empty()
            && self.zones.is_empty()
    }
}

fn ring_m(points: &[copper_physical::Point]) -> Vec<P2> {
    points.iter().map(|p| [nm_to_m(p.x_nm), nm_to_m(p.y_nm)]).collect()
}

/// The stack-up as the loop and island estimates need it.
struct Stack<'a> {
    board: &'a PhysicalBoard,
    depths: indexmap::IndexMap<CopperLayer, i64>,
}

impl<'a> Stack<'a> {
    fn new(board: &'a PhysicalBoard) -> Self {
        Stack { board, depths: copper_layer_depths_nm(&board.stackup) }
    }

    fn declared(&self) -> bool {
        !self.board.stackup.physical_layers.is_empty()
    }

    /// Centre-to-centre distance of two copper layers, m.
    fn height(&self, a: CopperLayer, b: CopperLayer) -> Option<f64> {
        Some(nm_to_m((self.depths.get(&a)? - self.depths.get(&b)?).abs()))
    }

    /// Depth of the layer's centre below the top surface, nm.
    fn depth(&self, layer: CopperLayer) -> Option<i64> {
        self.depths.get(&layer).copied()
    }

    /// Height of the layer's centre above the top copper layer's centre, m (negative below it; 0 without a stack-up).
    fn z(&self, layer: CopperLayer) -> f64 {
        match (self.depths.get(&CopperLayer::Front), self.depths.get(&layer)) {
            (Some(top), Some(depth)) => nm_to_m(top - depth),
            _ => 0.0,
        }
    }

    /// The copper layers in stack-up order with their depth in nm.
    fn layer_list(&self) -> Vec<(CopperLayer, i64)> {
        self.board.stackup.copper_layers.iter().map(|l| (*l, self.depths.get(l).copied().unwrap_or(0))).collect()
    }

    fn copper_thickness(&self, layer: CopperLayer) -> f64 {
        self.board
            .stackup
            .physical_layers
            .iter()
            .find(|item| item.copper_layer == Some(layer))
            .map_or(DEFAULT_COPPER, |item| nm_to_m(item.thickness_nm))
    }

    /// Thickness-weighted relative permittivity of the dielectrics between two
    /// copper layers; the FR-4 default when none is declared.
    fn er_between(&self, a: CopperLayer, b: CopperLayer) -> f64 {
        let layers = &self.board.stackup.physical_layers;
        let position = |layer: CopperLayer| layers.iter().position(|item| item.copper_layer == Some(layer));
        let (Some(i), Some(j)) = (position(a), position(b)) else { return DEFAULT_ER };
        let (lo, hi) = (i.min(j), i.max(j));
        let (mut weighted, mut total) = (0.0, 0.0);
        for item in &layers[lo + 1..hi] {
            if let (None, Some(er)) = (&item.copper_layer, &item.relative_permittivity) {
                let (er, thickness) =
                    (er.to_sci_string().parse::<f64>().unwrap_or(DEFAULT_ER), item.thickness_nm as f64);
                weighted += er * thickness;
                total += thickness;
            }
        }
        if total > 0.0 { weighted / total } else { DEFAULT_ER }
    }
}

/// The ground reference of a copper layer: the nearest other layer with a
/// ground `copper_zone`.
#[derive(Clone, Debug, PartialEq)]
struct Plane {
    layer: CopperLayer,
    height: f64,
    er: f64,
}

fn ground_nets(view: &FlatElectricalView, cell: &SwitchingCell) -> BTreeSet<String> {
    let mut nets: BTreeSet<String> = view
        .supplies
        .iter()
        .filter(|supply| supply.voltage.base_value().is_zero())
        .map(|supply| supply.net.clone())
        .collect();
    nets.extend(cell.hot_return_net.iter().cloned());
    nets
}

fn nearest_plane(board: &PhysicalBoard, stack: &Stack, grounds: &BTreeSet<String>, from: CopperLayer) -> Option<Plane> {
    let zones = board.zones.iter().chain(board.hard_macros.iter().flat_map(|m| m.zones.iter()));
    zones
        .filter(|zone| grounds.contains(&zone.net))
        .flat_map(|zone| zone.layers.iter().copied())
        .filter(|layer| *layer != from)
        .filter_map(|layer| {
            let height = stack.height(from, layer)?;
            (height > 0.0).then(|| Plane { layer, height, er: stack.er_between(from, layer) })
        })
        .min_by(|a, b| a.height.total_cmp(&b.height))
}

// ---------------------------------------------------------------------------
// Emission standard and sweeps
// ---------------------------------------------------------------------------

struct Standard {
    name: Option<String>,
    line: Option<limits::LimitLine>,
    distance: f64,
    cable_length: f64,
    targets: Vec<String>,
    /// Why there is no limit line, when a standard is declared.
    unsupported: Option<String>,
}

fn standard_of(view: &FlatElectricalView) -> Standard {
    let mut standard = Standard {
        name: None,
        line: None,
        distance: DEFAULT_DISTANCE,
        cable_length: 1.0,
        targets: Vec::new(),
        unsupported: None,
    };
    let Some(constraint) = view.constraints.iter().find(|c| c.kind == ConstraintKind::Emission) else {
        return standard;
    };
    standard.targets = constraint.targets.clone();
    if let Some(ParameterValue::Quantity(length)) = constraint.parameters.get("cable_length") {
        standard.cable_length = quantity(length);
    }
    let Some(ParameterValue::Text(name)) = constraint.parameters.get("standard") else { return standard };
    standard.name = Some(name.clone());
    let line = match name.as_str() {
        "cispr32_class_a" => Some(limits::cispr32_radiated(limits::Class::A, limits::Distance::M10)),
        "cispr32_class_b" => Some(limits::cispr32_radiated(limits::Class::B, limits::Distance::M10)),
        "fcc15_class_a" => Some(limits::fcc15_radiated(limits::Class::A)),
        "fcc15_class_b" => Some(limits::fcc15_radiated(limits::Class::B)),
        _ => None,
    };
    match line {
        Some(line) => {
            standard.distance = line.distance.unwrap_or(DEFAULT_DISTANCE);
            standard.line = Some(line);
        }
        None => {
            standard.unsupported = Some(format!(
                "no radiated limit table for {name}: levels are at {DEFAULT_DISTANCE} m and carry no margin"
            ));
        }
    }
    standard
}

/// The worst point of an envelope sweep.
#[derive(Clone, Copy, Debug)]
struct Worst {
    frequency: f64,
    level_dbuvpm: f64,
    margin_db: Option<f64>,
}

/// Evaluate `field(f)` (V/m at the standard's distance) on a logarithmic grid
/// over `[low, high]` and return the worst frequency: the least margin when a
/// limit line is given, else the highest level (the lowest such frequency).
fn sweep(low: f64, high: f64, line: Option<&limits::LimitLine>, field: impl Fn(f64) -> f64) -> Option<Worst> {
    if high.partial_cmp(&low) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let steps = ((high / low).log10() * 24.0).ceil().max(1.0) as usize;
    let mut worst: Option<Worst> = None;
    for k in 0..=steps {
        let f = low * (high / low).powf(k as f64 / steps as f64);
        let level = estimate::vpm_to_dbuvpm(field(f));
        if !level.is_finite() {
            continue;
        }
        let margin_db = line.and_then(|line| line.margin(f, level));
        let candidate = Worst { frequency: f, level_dbuvpm: level, margin_db };
        let better = match (&worst, margin_db) {
            (None, _) => true,
            (Some(current), Some(m)) => current.margin_db.is_none_or(|c| m < c - 1e-9),
            (Some(current), None) => level > current.level_dbuvpm + 0.01,
        };
        if better {
            worst = Some(candidate);
        }
    }
    worst
}

// ---------------------------------------------------------------------------
// Cell inputs
// ---------------------------------------------------------------------------

/// Where the edge time of a cell comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeBasis {
    /// The switch pin's `rise_time`/`fall_time`.
    Declared,
    /// The switch pin's `slew_rate` and the hot-supply swing: exact, no band.
    SlewRate,
    /// The default slew band of [`defaults`]: nominal, fast and slow.
    RuleOfThumb,
}

/// Which end of an edge band a computation uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Corner {
    Nominal,
    Fast,
    Slow,
}

/// The 10-90 % edge of the switch node.
#[derive(Clone, Debug)]
struct Edge {
    basis: EdgeBasis,
    /// The edge a single-number estimate uses, s (the faster of rise and fall when declared).
    nominal: f64,
    fast: f64,
    slow: f64,
    /// Rise and fall times of the nominal edge, s.
    rise: f64,
    fall: f64,
    /// The slew rate the edge came from or is classified by, V/ns.
    slew: Option<f64>,
}

impl Edge {
    fn banded(&self) -> bool {
        self.basis == EdgeBasis::RuleOfThumb
    }

    /// The edge time of a single-number estimate at this end of the band (the same for all ends unless banded).
    fn time(&self, corner: Corner) -> f64 {
        match (self.banded(), corner) {
            (true, Corner::Fast) => self.fast,
            (true, Corner::Slow) => self.slow,
            _ => self.nominal,
        }
    }

    /// Rise and fall (10-90 %) at this end of the band.
    fn rise_fall(&self, corner: Corner) -> (f64, f64) {
        if self.banded() {
            let t = self.time(corner);
            (t, t)
        } else {
            (self.rise, self.fall)
        }
    }

    /// The tag of a result that uses this edge.
    fn tag(&self) -> Option<&'static str> {
        match self.basis {
            EdgeBasis::Declared => None,
            EdgeBasis::SlewRate => Some(defaults::TAG_EDGE_FROM_SLEW),
            EdgeBasis::RuleOfThumb => Some(defaults::TAG_EDGE),
        }
    }

    /// One sentence for `input:` lines and coverage.
    fn describe(&self, swing: Option<f64>) -> String {
        let ns = |t: f64| format!("{} ns", rounded(t * 1e9, 4));
        match self.basis {
            EdgeBasis::Declared => {
                format!("{} (10-90 %, switch pin rise_time/fall_time)", ns(self.nominal))
            }
            EdgeBasis::SlewRate => format!(
                "{} (10-90 %) from the declared slew_rate {} V/ns on a {} V swing: t = 0.8 V / slew",
                ns(self.nominal),
                rounded(self.slew.unwrap_or(0.0), 4),
                rounded(swing.unwrap_or(0.0), 4)
            ),
            EdgeBasis::RuleOfThumb => format!(
                "{} nominal (fast {}, slow {}; 10-90 %) from a default slew of {} V/ns ({} V/ns fast, {} V/ns slow) on a {} V swing",
                ns(self.nominal),
                ns(self.fast),
                ns(self.slow),
                defaults::SLEW_NOMINAL,
                defaults::SLEW_FAST,
                defaults::SLEW_SLOW,
                rounded(swing.unwrap_or(0.0), 4)
            ),
        }
    }

    /// The rule behind a default edge, worded for an `input:` line; `None` for a declared edge.
    fn rule_note(&self) -> Option<String> {
        self.banded().then(|| defaults::rule_text(&defaults::edge_rule(), "slew_rate or rise_time"))
    }

    /// The `input:` line of this edge.
    fn input_line(&self, swing: Option<f64>) -> String {
        match self.rule_note() {
            Some(rule) => format!("edge time {} {rule}", self.describe(swing)),
            None => format!("edge time {}", self.describe(swing)),
        }
    }
}

struct Drive {
    frequency: f64,
    frequency_note: &'static str,
    edge: Option<Edge>,
    /// Why there is no edge, when there is none.
    edge_missing: String,
    /// The hot-supply voltage: the switch-node swing, V.
    swing: Option<f64>,
    /// Information about the edge inputs (wide-bandgap class, slew rate and rise time disagreeing).
    edge_notes: Vec<String>,
    current: Option<f64>,
    duty: f64,
    duty_note: String,
}

fn supply_voltage(view: &FlatElectricalView, net: Option<&String>) -> Option<f64> {
    let net = net?;
    view.supplies.iter().find(|supply| &supply.net == net).map(|supply| quantity(&supply.voltage))
}

/// The slew rate of a quantity in V/ns.
fn slew_v_per_ns(value: &Quantity) -> f64 {
    quantity(value) * 1e-9
}

fn drive_of(view: &FlatElectricalView, cell: &SwitchingCell) -> Result<Drive, String> {
    let (frequency, frequency_note) = match cell.frequency.as_ref().map(|f| &f.spec) {
        Some(SwitchingFrequency::Fixed(value)) => (quantity(value), "fixed"),
        Some(SwitchingFrequency::Range { minimum, maximum }) => match (maximum, minimum) {
            (Some(value), _) => (quantity(value), "range upper bound"),
            (None, Some(value)) => (quantity(value), "range lower bound"),
            (None, None) => return Err("no switching frequency is declared".to_owned()),
        },
        None => return Err("no switching frequency is declared".to_owned()),
    };
    let positive = |q: &Option<Quantity>| q.as_ref().map(quantity).filter(|t| *t > 0.0);
    let swing = supply_voltage(view, cell.hot_supply_net.as_ref()).filter(|v| *v > 0.0);
    let slew = cell.slew_rate.as_ref().map(slew_v_per_ns).filter(|s| *s > 0.0);
    let mut edge_notes = Vec::new();
    if let Some(slew) = slew
        && slew >= defaults::WIDE_BANDGAP_SLEW
    {
        edge_notes.push(format!(
            "the declared slew_rate {} V/ns is wide-bandgap class (at or above {} V/ns; information only)",
            rounded(slew, 4),
            defaults::WIDE_BANDGAP_SLEW
        ));
    }
    let declared = match (positive(&cell.rise_time), positive(&cell.fall_time)) {
        (Some(r), Some(f)) => Some((r, f)),
        (Some(t), None) | (None, Some(t)) => Some((t, t)),
        (None, None) => None,
    };
    let mut edge_missing = String::new();
    let edge = if let Some((rise, fall)) = declared {
        if let (Some(slew), Some(swing)) = (slew, swing) {
            let implied = defaults::edge_10_90(swing, slew);
            let declared_edge = rise.min(fall);
            let factor = (declared_edge / implied).max(implied / declared_edge);
            if factor > 3.0 {
                edge_notes.push(format!(
                    "the declared rise_time/fall_time {} ns and the declared slew_rate {} V/ns imply {} ns on the {} V swing \
                     (a factor {:.1} apart); rise_time/fall_time is used",
                    rounded(declared_edge * 1e9, 4),
                    rounded(slew, 4),
                    rounded(implied * 1e9, 4),
                    rounded(swing, 4),
                    factor
                ));
            }
        }
        let nominal = rise.min(fall);
        Some(Edge { basis: EdgeBasis::Declared, nominal, fast: nominal, slow: nominal, rise, fall, slew })
    } else if let (Some(slew), Some(swing)) = (slew, swing) {
        let t = defaults::edge_10_90(swing, slew);
        Some(Edge { basis: EdgeBasis::SlewRate, nominal: t, fast: t, slow: t, rise: t, fall: t, slew: Some(slew) })
    } else if let Some(swing) = swing {
        let (nominal, fast, slow) = defaults::default_edge_band(swing);
        Some(Edge {
            basis: EdgeBasis::RuleOfThumb,
            nominal,
            fast,
            slow,
            rise: nominal,
            fall: nominal,
            slew: Some(defaults::SLEW_NOMINAL),
        })
    } else {
        edge_missing = if slew.is_some() {
            "the switch pin declares a slew_rate but the hot supply net declares no supply voltage for the swing"
                .to_owned()
        } else {
            "the switch pin declares no rise_time, fall_time or slew_rate and the hot supply net declares no supply voltage for the default slew"
                .to_owned()
        };
        None
    };
    let current = cell.current_peak.as_ref().map(quantity).filter(|i| *i > 0.0);
    let vout = supply_voltage(view, cell.magnetic.as_ref().and_then(|m| m.output_net.as_ref()));
    let (duty, duty_note) = match (swing, vout) {
        (Some(vin), Some(vout)) if vin > 0.0 && vout > 0.0 && vout < vin => {
            ((vout / vin).clamp(0.02, 0.98), format!("duty {:.2} from {vout} V / {vin} V", vout / vin))
        }
        _ => (DEFAULT_DUTY, format!("duty {DEFAULT_DUTY} assumed (input and output voltage not both declared)")),
    };
    Ok(Drive { frequency, frequency_note, edge, edge_missing, swing, edge_notes, current, duty, duty_note })
}

/// The result of sweeping an estimate over the edge band.
struct Banded {
    /// The worst point at the nominal edge.
    nominal: Worst,
    /// `[low, high]` of the worst levels over the nominal, fast and slow edge; `None` without a band.
    band: Option<[f64; 2]>,
    /// The margin the finding reports: from the fast bound when banded (the unfavourable end).
    margin_db: Option<f64>,
}

/// [`sweep`] of `field(frequency, edge_time)` at the nominal edge and, when the edge is a
/// rule-of-thumb band, at the fast and slow edge as well.
fn sweep_edge(
    edge: &Edge,
    low: f64,
    high: f64,
    line: Option<&limits::LimitLine>,
    field: impl Fn(f64, f64) -> f64,
) -> Option<Banded> {
    let nominal = sweep(low, high, line, |f| field(f, edge.time(Corner::Nominal)))?;
    if !edge.banded() {
        return Some(Banded { nominal, band: None, margin_db: nominal.margin_db });
    }
    let fast = sweep(low, high, line, |f| field(f, edge.time(Corner::Fast)))?;
    let slow = sweep(low, high, line, |f| field(f, edge.time(Corner::Slow)))?;
    let levels = [nominal.level_dbuvpm, fast.level_dbuvpm, slow.level_dbuvpm];
    let band = [
        levels.iter().copied().fold(f64::INFINITY, f64::min),
        levels.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ];
    Some(Banded { nominal, band: Some(band), margin_db: fast.margin_db })
}

// ---------------------------------------------------------------------------
// Accumulator
// ---------------------------------------------------------------------------

struct Context<'a> {
    layout: Layout<'a>,
    stack: Stack<'a>,
    standard: Standard,
    model: &'a EmiModel,
    connectors: BTreeSet<String>,
    findings: Vec<EmiFinding>,
    coverage: Vec<Coverage>,
    /// Loops analysed, loops with at least one traced leg, copper legs traced and unrouted, copper seen on a loop net.
    trace_loops: usize,
    trace_loops_traced: usize,
    trace_legs_traced: usize,
    trace_legs_unrouted: usize,
    trace_copper_seen: bool,
    /// The standard hot loop chosen for each cell (by controller), for `EMI-CONDUCTED`.
    loops: BTreeMap<String, LoopSummary>,
}

/// What `EMI-CONDUCTED` needs from the hot-loop analysis of a cell.
#[derive(Clone, Debug)]
struct LoopSummary {
    capacitor: String,
    inductance: Option<f64>,
    /// `traced copper path`, `placement estimate (pad-centre polygon)` ...
    basis: String,
    /// The loop's trace and placement tags.
    tags: Vec<String>,
    layer: String,
}

impl Context<'_> {
    fn cover(&mut self, check: String, status: CoverageStatus, detail: impl Into<String>) {
        self.coverage.push(Coverage { check, status, required: false, detail: detail.into() });
    }
}

fn base_finding(
    code: &str,
    cell: &SwitchingCell,
    message: String,
    objects: Vec<String>,
    nets: Vec<String>,
    layers: Vec<String>,
) -> EmiFinding {
    let finding = Finding::new(code, Severity::Warning, message).objects(objects).nets(nets).layers(layers);
    let inferred = cell.provenance == Provenance::Inferred;
    EmiFinding {
        finding,
        cell: Some(cell.controller.clone()),
        provenance: Some(cell.provenance),
        tags: if inferred { vec!["inferred".to_owned()] } else { Vec::new() },
        estimate: None,
        margin_db: None,
        accepted: None,
        measures: Object::new(),
        subjects: vec![cell.controller.clone()],
        rank: 0.0,
    }
}

fn put(measures: &mut Object, key: &str, value: Value) {
    measures.insert(key.to_owned(), value);
}

fn num(value: f64, digits: u32) -> Value {
    Value::from(rounded(value, digits))
}

fn endpoint_names(endpoints: &[Endpoint]) -> Vec<String> {
    endpoints.iter().map(ToString::to_string).collect()
}

fn dedup(items: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

// ---------------------------------------------------------------------------
// EMI-HOT-LOOP
// ---------------------------------------------------------------------------

/// How much of the loop's copper legs the traced path covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum TraceState {
    /// No routed copper on the loop nets: the pad-centre polygon is the loop.
    #[default]
    NoCopper,
    /// Copper exists on the loop nets but joins no pad pair of the loop.
    NotConnected,
    /// Some copper legs are traced, the others stay straight (`unrouted_leg`).
    Partial,
    /// Every copper leg is traced.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegKind {
    /// Both pads on one net and a copper path joins them.
    Traced,
    /// Both pads on one net, no copper path: kept as the straight pad-to-pad segment.
    Unrouted,
    /// The pads are on different nets: the current crosses a component (switch, diode, capacitor body).
    Component,
}

impl LegKind {
    fn value(self) -> &'static str {
        match self {
            LegKind::Traced => "traced",
            LegKind::Unrouted => "unrouted",
            LegKind::Component => "component",
        }
    }
}

/// One side of the loop polygon, between two consecutive pads of the sequence.
#[derive(Clone, Debug)]
struct Leg {
    from: String,
    to: String,
    net: Option<String>,
    kind: LegKind,
    /// Centre-line length, m: along the copper (via barrels included) for a traced leg, else straight.
    length: f64,
    vias: usize,
    layer_changes: usize,
    narrowest: Option<f64>,
    layers: Vec<String>,
    plane_shortcut: bool,
}

/// A straight bar of the loop on a copper layer, metres.
#[derive(Clone, Debug)]
struct LoopSegment {
    a: P2,
    b: P2,
    width: f64,
    layer: CopperLayer,
}

#[derive(Clone, Debug, Default)]
struct LoopCandidate {
    capacitor: String,
    /// The capacitor declares `decouples` of a hot-supply pin.
    declared: bool,
    labels: Vec<String>,
    /// The loop polygon: the traced path where it exists, else the pad centres.
    points: Vec<P2>,
    /// Nominal trace width at each pad, for the pad-centre version.
    widths: Vec<f64>,
    /// The pad-centre polygon.
    pad_points: Vec<P2>,
    area: f64,
    length: f64,
    inductance: Option<f64>,
    plane: Option<Plane>,
    inductance_note: String,
    state: TraceState,
    legs: Vec<Leg>,
    /// The bars of the traced version (empty when nothing is traced).
    segments: Vec<LoopSegment>,
    /// Via barrels of the traced path: (height, diameter), metres.
    vias: Vec<(f64, f64)>,
    /// The loop as a closed 3D polyline, metres: z up from the top copper layer (see [`Stack::z`]).
    path_3d: Vec<[f64; 3]>,
    /// z of the reference plane layer, metres, when the loop has one.
    plane_z: Option<f64>,
}

impl LoopCandidate {
    fn traced_legs(&self) -> usize {
        self.legs.iter().filter(|leg| leg.kind == LegKind::Traced).count()
    }

    fn unrouted_legs(&self) -> usize {
        self.legs.iter().filter(|leg| leg.kind == LegKind::Unrouted).count()
    }
}

/// Which loop of a cell is analysed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopKind {
    /// The topology's single hot loop (buck, boost).
    Standard,
    /// Four-switch buck-boost, buck leg: capacitor on the hot supply -> hot supply pad -> hot return pad.
    Input,
    /// Four-switch buck-boost, boost leg: capacitor on the output net -> output pin pad -> hot return pad.
    Output,
}

struct LoopSpec {
    kind: LoopKind,
    /// `input loop` or `output loop` for a cell with two loops.
    name: Option<&'static str>,
    /// Coverage and check suffix of a cell with two loops.
    key: Option<&'static str>,
    capacitors: Vec<HotCapacitor>,
    nets: Vec<String>,
    output_pins: Vec<Endpoint>,
}

/// The ordered pad groups of the cell's hot loop for one capacitor, or the
/// reason the topology is not supported.
fn loop_groups<'a>(
    cell: &'a SwitchingCell,
    capacitor: &'a HotCapacitor,
    spec: &'a LoopSpec,
) -> Result<Vec<(String, Vec<&'a Endpoint>)>, String> {
    let supply = |role: &str, pins: &'a [Endpoint]| (role.to_owned(), pins.iter().collect::<Vec<_>>());
    let cap_supply = (format!("{} supply", capacitor.component), vec![&capacitor.supply_pin]);
    let cap_return = (format!("{} return", capacitor.component), vec![&capacitor.return_pin]);
    let hot_supply = supply("hot supply", &cell.hot_supply_pins);
    let hot_return = supply("hot return", &cell.hot_return_pins);
    match spec.kind {
        LoopKind::Input => return Ok(vec![cap_supply, hot_supply, hot_return, cap_return]),
        LoopKind::Output => {
            return Ok(vec![cap_supply, supply("output pin", &spec.output_pins), hot_return, cap_return]);
        }
        LoopKind::Standard => {}
    }
    let switch = supply("switch", &cell.switch_pins);
    match (cell.topology, cell.catch_diode.as_ref()) {
        (Some(Topology::Buck) | None, Some(diode)) => Ok(vec![
            cap_supply,
            hot_supply,
            switch,
            (format!("{} switch side", diode.component), vec![&diode.switch_pin]),
            (format!("{} rail side", diode.component), vec![&diode.rail_pin]),
            cap_return,
        ]),
        (Some(Topology::Buck) | None, None) => Ok(vec![cap_supply, hot_supply, hot_return, cap_return]),
        (Some(Topology::Boost), Some(diode)) => Ok(vec![
            cap_supply,
            (format!("{} rail side", diode.component), vec![&diode.rail_pin]),
            (format!("{} switch side", diode.component), vec![&diode.switch_pin]),
            switch,
            hot_return,
            cap_return,
        ]),
        (Some(Topology::Boost), None) => Ok(vec![cap_supply, hot_supply, hot_return, cap_return]),
        (Some(other), _) => Err(format!("no hot-loop sequence is defined for the {} topology", other.value())),
    }
}

/// The bars of a pad-centre candidate (straight between the pads, at the plane height).
fn segments_of(candidate: &LoopCandidate, thickness: f64) -> Vec<Segment> {
    let n = candidate.points.len();
    let z = candidate.plane.as_ref().map_or(0.0, |plane| plane.height);
    (0..n)
        .filter_map(|i| {
            let j = (i + 1) % n;
            let (a, b) = (candidate.points[i], candidate.points[j]);
            if dist(a, b) < 1e-6 {
                return None;
            }
            let width = candidate.widths[i].min(candidate.widths[j]).clamp(WIDTH_BOUNDS.0, WIDTH_BOUNDS.1);
            Some(Segment { start: [a[0], a[1], z], end: [b[0], b[1], z], width, thickness })
        })
        .collect()
}

fn to_m(point: copper_physical::Point) -> P2 {
    [nm_to_m(point.x_nm), nm_to_m(point.y_nm)]
}

/// The routed copper of one net in the input form of the path graph.
fn trace_copper(board: &PhysicalBoard, copper: &NetCopper, net: &str) -> TraceCopper {
    let order = &board.stackup.copper_layers;
    let span = |from: CopperLayer, to: CopperLayer| -> Vec<CopperLayer> {
        match (order.iter().position(|l| *l == from), order.iter().position(|l| *l == to)) {
            (Some(a), Some(b)) => order[a.min(b)..=a.max(b)].to_vec(),
            _ => vec![from, to],
        }
    };
    let mut input = TraceCopper::default();
    for track in copper.tracks.iter().filter(|t| t.net == net) {
        input.tracks.push(TraceTrack {
            start: track.start,
            end: track.end,
            width_nm: track.width_nm,
            layer: track.layer,
        });
    }
    for arc in copper.arcs.iter().filter(|a| a.net == net) {
        input.arcs.push(TraceArc {
            start: arc.start,
            end: arc.end,
            chords: arc.chord_points(),
            length_nm: arc.length_nm(),
            width_nm: arc.width_nm,
            layer: arc.layer,
        });
    }
    for via in copper.vias.iter().filter(|v| v.net == net) {
        input.vias.push(TraceVia {
            position: via.position,
            size_nm: via.size_nm,
            drill_nm: via.drill_nm,
            layers: span(via.from_layer, via.to_layer),
        });
    }
    for polygon in copper.polygons.iter().filter(|p| p.net == net) {
        input.polygons.push(TracePolygon {
            layers: vec![polygon.layer],
            outer: polygon.vertices.clone(),
            holes: Vec::new(),
        });
    }
    for zone in copper.zones.iter().filter(|z| z.net == net) {
        input.polygons.push(TracePolygon {
            layers: zone.layers.clone(),
            outer: zone.outer.clone(),
            holes: zone.holes.clone(),
        });
    }
    input
}

/// The path graph of one net with the pads it was built over.
struct NetGraph {
    graph: TraceGraph,
    pads: Vec<PadGeom>,
}

/// The traced loop: polygon, legs, bars and via barrels.
struct TracedLoop {
    points: Vec<P2>,
    legs: Vec<Leg>,
    segments: Vec<LoopSegment>,
    vias: Vec<(f64, f64)>,
}

/// Barrel diameter assumed when a path crosses a through-hole pad without a hole size, m.
const DEFAULT_BARREL: f64 = 0.3e-3;
/// Bounds of the width given to the straight line of a plane shortcut, m.
const SHORTCUT_WIDTH: (f64, f64) = (1.0e-3, 5.0e-3);
/// Narrowest width given to a traced bar, m (a guard against zero-width input).
const MIN_TRACED_WIDTH: f64 = 20e-6;

/// Trace every leg of the loop through the copper of its net. `chosen` are
/// the loop pads in sequence order.
fn trace_loop(cx: &Context, copper: &NetCopper, chosen: &[&PadGeom], labels: &[String]) -> TracedLoop {
    let layers = cx.stack.layer_list();
    let mut graphs: BTreeMap<String, NetGraph> = BTreeMap::new();
    let mut result = TracedLoop { points: Vec::new(), legs: Vec::new(), segments: Vec::new(), vias: Vec::new() };
    let n = chosen.len();
    for i in 0..n {
        let (a, b) = (chosen[i], chosen[(i + 1) % n]);
        let (net_a, net_b) =
            (cx.layout.net_of_pad(&a.component, &a.number), cx.layout.net_of_pad(&b.component, &b.number));
        let net = match (&net_a, &net_b) {
            (Some(x), Some(y)) if x == y => Some(x.clone()),
            _ => None,
        };
        let mut leg = Leg {
            from: labels[i].clone(),
            to: labels[(i + 1) % n].clone(),
            net: net.clone(),
            kind: LegKind::Component,
            length: dist(a.shape.center(), b.shape.center()),
            vias: 0,
            layer_changes: 0,
            narrowest: None,
            layers: Vec::new(),
            plane_shortcut: false,
        };
        let mut path = None;
        if let Some(net) = &net {
            let entry = graphs.entry(net.clone()).or_insert_with(|| {
                let pads = cx.layout.pads_of_net(net);
                let trace_pads: Vec<TracePad> =
                    pads.iter().map(|p| TracePad::new(p.raw.clone(), p.layers.clone(), p.drill_nm)).collect();
                let input = trace_copper(cx.layout.board, copper, net);
                NetGraph { graph: TraceGraph::build(&trace_pads, &input, &layers), pads }
            });
            let find = |pad: &PadGeom| {
                entry
                    .pads
                    .iter()
                    .position(|p| p.component == pad.component && p.number == pad.number && p.raw == pad.raw)
            };
            path = match (find(a), find(b)) {
                (Some(from), Some(to)) if from != to => entry.graph.shortest_path(from, to),
                _ => None,
            };
            leg.kind = if path.is_some() { LegKind::Traced } else { LegKind::Unrouted };
        }
        let mut leg_points: Vec<P2> = vec![a.shape.center(), b.shape.center()];
        match &path {
            Some(path) => {
                leg.length = path.length_nm() * 1e-9;
                leg.vias = path.via_count();
                leg.layer_changes = path.layer_changes();
                leg.narrowest = path.narrowest_width_nm().map(nm_to_m);
                leg.layers = path.layers().iter().map(|l| l.value().to_owned()).collect();
                leg.plane_shortcut = path.plane_shortcut();
                leg_points = path.points().into_iter().map(to_m).collect();
                for step in &path.steps {
                    match step.kind {
                        StepKind::Via => {
                            let diameter = step.drill_nm.map(nm_to_m).filter(|d| *d > 0.0).unwrap_or(DEFAULT_BARREL);
                            result.vias.push((step.length_nm * 1e-9, diameter));
                        }
                        kind => {
                            let step_length = step.length_nm * 1e-9;
                            for pair in step.polyline.windows(2) {
                                let width = match (kind, step.width_nm) {
                                    (StepKind::PadStub, Some(w)) => nm_to_m(w).clamp(WIDTH_BOUNDS.0, WIDTH_BOUNDS.1),
                                    (StepKind::PlaneShortcut, _) => {
                                        step_length.clamp(SHORTCUT_WIDTH.0, SHORTCUT_WIDTH.1)
                                    }
                                    (_, Some(w)) => nm_to_m(w).max(MIN_TRACED_WIDTH),
                                    (_, None) => WIDTH_BOUNDS.0,
                                };
                                result.segments.push(LoopSegment {
                                    a: to_m(pair[0]),
                                    b: to_m(pair[1]),
                                    width,
                                    layer: step.layer,
                                });
                            }
                        }
                    }
                }
            }
            None => {
                let (pa, pb) = (a.shape.center(), b.shape.center());
                let width =
                    a.shape.small_dimension().min(b.shape.small_dimension()).clamp(WIDTH_BOUNDS.0, WIDTH_BOUNDS.1);
                let layer = a.layers.first().copied().unwrap_or(CopperLayer::Front);
                result.segments.push(LoopSegment { a: pa, b: pb, width, layer });
            }
        }
        for point in leg_points {
            if result.points.last().is_none_or(|last| dist(*last, point) > 1e-9) {
                result.points.push(point);
            }
        }
        result.legs.push(leg);
    }
    // The last leg closes on the first pad.
    if result.points.len() > 1
        && let (Some(first), Some(last)) = (result.points.first(), result.points.last())
        && dist(*first, *last) <= 1e-9
    {
        result.points.pop();
    }
    result
}

/// Height of a copper layer over the reference plane, when the layer is on the
/// same side of the plane as the loop's pads, m.
fn height_over_plane(stack: &Stack, plane: &Plane, pad_layer: CopperLayer, layer: CopperLayer) -> Option<f64> {
    let (depth, plane_depth, pad_depth) = (stack.depth(layer)?, stack.depth(plane.layer)?, stack.depth(pad_layer)?);
    let side = |d: i64| (d - plane_depth).signum();
    (layer != plane.layer && side(depth) == side(pad_depth) && side(depth) != 0)
        .then(|| nm_to_m((depth - plane_depth).abs()))
}

/// The traced bars in path order as a 3D polyline: each bar's ends at its layer's z, consecutive
/// duplicates dropped (a via is the vertical step between two bars on different layers).
fn traced_path_3d(stack: &Stack, segments: &[LoopSegment]) -> Vec<[f64; 3]> {
    let mut path: Vec<[f64; 3]> = Vec::new();
    for bar in segments {
        let z = stack.z(bar.layer);
        for end in [bar.a, bar.b] {
            let vertex = [end[0], end[1], z];
            if path.last().is_none_or(|last| (0..3).map(|k| (last[k] - vertex[k]).powi(2)).sum::<f64>() > 1e-18) {
                path.push(vertex);
            }
        }
    }
    if path.len() > 1 && path.first() == path.last() {
        path.pop();
    }
    path
}

fn build_candidate(
    cx: &Context,
    cell: &SwitchingCell,
    spec: &LoopSpec,
    capacitor: &HotCapacitor,
    grounds: &BTreeSet<String>,
    copper: &NetCopper,
) -> Result<LoopCandidate, String> {
    let sequence = loop_groups(cell, capacitor, spec)?;
    let mut labels = Vec::new();
    let mut pads: Vec<Vec<PadGeom>> = Vec::new();
    for (label, endpoints) in &sequence {
        let found: Vec<PadGeom> = endpoints.iter().flat_map(|e| cx.layout.pads_of_endpoint(e)).collect();
        if found.is_empty() {
            let names = endpoints.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
            return Err(format!("{label} ({names}) has no placed pad"));
        }
        labels.push(label.clone());
        pads.push(found);
    }
    let centres: Vec<Vec<P2>> = pads.iter().map(|g| g.iter().map(|p| p.shape.center()).collect()).collect();
    let path = shortest_cycle(&centres).ok_or_else(|| "the loop has fewer than three pad groups".to_owned())?;
    let chosen: Vec<&PadGeom> = path.iter().enumerate().map(|(i, &k)| &pads[i][k]).collect();
    let points: Vec<P2> = chosen.iter().map(|p| p.shape.center()).collect();
    let widths: Vec<f64> = chosen.iter().map(|p| p.shape.small_dimension()).collect();
    let labels: Vec<String> =
        chosen.iter().zip(&labels).map(|(pad, label)| format!("{label} {}.{}", pad.component, pad.number)).collect();
    let (area, length) = (polygon_area(&points), closed_length(&points));
    let path_3d: Vec<[f64; 3]> = chosen
        .iter()
        .map(|pad| {
            let c = pad.shape.center();
            [c[0], c[1], pad.layers.first().map_or(0.0, |layer| cx.stack.z(*layer))]
        })
        .collect();

    let sides: BTreeSet<BoardSide> = chosen.iter().filter_map(|p| cx.layout.side(&p.component)).collect();
    let mut candidate = LoopCandidate {
        capacitor: capacitor.component.clone(),
        declared: capacitor.declared,
        labels: labels.clone(),
        points: points.clone(),
        widths,
        pad_points: points,
        area,
        length,
        path_3d,
        ..LoopCandidate::default()
    };

    // Copper path of every leg whose two pads share a net.
    let mut traced = None;
    // A zone outline alone (no track, arc, via or polygon) is declared intent, not routed copper:
    // such a loop stays a placement estimate and carries no unrouted-leg claim.
    let routed =
        !(copper.tracks.is_empty() && copper.arcs.is_empty() && copper.vias.is_empty() && copper.polygons.is_empty());
    if routed {
        let result = trace_loop(cx, copper, &chosen, &labels);
        candidate.legs = result.legs.clone();
        candidate.state = match (candidate.traced_legs(), candidate.unrouted_legs()) {
            (0, _) => TraceState::NotConnected,
            (_, 0) => TraceState::Full,
            _ => TraceState::Partial,
        };
        if candidate.traced_legs() > 0 {
            candidate.points = result.points.clone();
            candidate.area = polygon_area(&candidate.points);
            candidate.length = closed_length(&candidate.points);
            candidate.segments = result.segments.clone();
            candidate.vias = result.vias.clone();
            candidate.path_3d = traced_path_3d(&cx.stack, &result.segments);
            traced = Some(result);
        }
    }

    if sides.len() != 1 {
        candidate.inductance_note =
            "loop pads are on both board sides; the planar inductance model does not apply".to_owned();
        return Ok(candidate);
    }
    let layer = copper_layer(*sides.iter().next().expect("one side"));
    candidate.plane = nearest_plane(cx.layout.board, &cx.stack, grounds, layer);
    candidate.plane_z = candidate.plane.as_ref().map(|plane| cx.stack.z(plane.layer));

    if traced.is_some() {
        // Bars with their real widths, each at its layer's height over the plane; via barrels in series.
        let mut bars = Vec::new();
        let mut off_side = false;
        for bar in &candidate.segments {
            if dist(bar.a, bar.b) < 1e-9 {
                continue;
            }
            let z = match &candidate.plane {
                Some(plane) => match height_over_plane(&cx.stack, plane, layer, bar.layer) {
                    Some(h) => h,
                    None => {
                        off_side = true;
                        0.0
                    }
                },
                None => cx.stack.depth(bar.layer).map_or(0.0, nm_to_m),
            };
            bars.push(Segment {
                start: [bar.a[0], bar.a[1], z],
                end: [bar.b[0], bar.b[1], z],
                width: bar.width,
                thickness: cx.stack.copper_thickness(bar.layer),
            });
        }
        let use_plane = candidate.plane.is_some() && !off_side;
        if off_side {
            // Segments on the plane layer or beyond it cannot be imaged: fall back to free space at true heights.
            for (bar, source) in bars.iter_mut().zip(candidate.segments.iter().filter(|b| dist(b.a, b.b) >= 1e-9)) {
                let z = cx.stack.depth(source.layer).map_or(0.0, nm_to_m);
                bar.start[2] = z;
                bar.end[2] = z;
            }
        }
        let planar = if use_plane {
            inductance::loop_inductance_over_plane(&bars, 0.0)
        } else {
            inductance::loop_inductance(&bars)
        };
        let (mut via_total, mut skipped) = (0.0, 0usize);
        for (height, diameter) in &candidate.vias {
            if *diameter > 0.0 && *height > diameter / 4.0 {
                via_total += inductance::via_inductance(*height, *diameter);
            } else {
                skipped += 1;
            }
        }
        let reference = match (&candidate.plane, use_plane) {
            (Some(plane), true) => format!(
                "each bar at its layer's height over the {} ground plane ({:.3} mm from the pad layer)",
                plane.layer.value(),
                mm(plane.height)
            ),
            (Some(plane), false) => format!(
                "free space at the true layer heights: the path uses the {} plane layer or the far side of it",
                plane.layer.value()
            ),
            (None, _) => "free space at the true layer heights (no ground copper_zone on another layer)".to_owned(),
        };
        candidate.inductance_note = format!(
            "partial inductance of the traced copper: {} bars with their real widths, {reference}; {} via barrel(s) added in series \
             (Johnson-Graham estimate{})",
            bars.len(),
            candidate.vias.len() - skipped,
            if skipped > 0 { format!(", {skipped} too short to model and omitted") } else { String::new() }
        );
        if !use_plane {
            // The inductance above is free space; the near-field map (`loop_reference_plane_z_mm`)
            // must not image the path in a plane the method note says it does not use.
            candidate.plane = None;
            candidate.plane_z = None;
        }
        candidate.inductance = Some(planar + via_total).filter(|l| l.is_finite() && *l > 0.0);
        return Ok(candidate);
    }

    let thickness = cx.stack.copper_thickness(layer);
    let segments = segments_of(&candidate, thickness);
    let value = match &candidate.plane {
        Some(plane) => {
            candidate.inductance_note = format!(
                "partial inductance of straight bars between pad centres at {:.3} mm over the {} ground plane",
                mm(plane.height),
                plane.layer.value()
            );
            inductance::loop_inductance_over_plane(&segments, 0.0)
        }
        None => {
            candidate.inductance_note =
                "free-space partial inductance of straight bars between pad centres (no ground copper_zone on another layer)"
                    .to_owned();
            inductance::loop_inductance(&segments)
        }
    };
    candidate.inductance = Some(value).filter(|l| l.is_finite() && *l > 0.0);
    Ok(candidate)
}

/// The output stage of a four-switch buck-boost: the controller's `VOUT*`
/// pins, their net and the capacitors from that net to the hot return.
struct OutputStage {
    pins: Vec<Endpoint>,
    net: String,
    capacitors: Vec<HotCapacitor>,
}

fn is_capacitor_part(view: &FlatElectricalView, component: &ComponentInstance) -> bool {
    let Some(part) = view.library.get(&component.part) else { return false };
    part.category.starts_with("passive.capacitor")
        || (part.category == "component.generic"
            && part.pins.len() == 2
            && matches!(&component.value, Some(copper_lang::model::ComponentValue::Quantity(v)) if v.dimension() == copper_units::Dimension::Capacitance))
}

/// Derived from the flat view: the phase-0 model records the inductor's far
/// net but not the controller's output pins.
fn output_stage(view: &FlatElectricalView, cell: &SwitchingCell) -> Result<OutputStage, String> {
    let Some(part) = view.library.get(&cell.part) else {
        return Err(format!("part {} is not in the library", cell.part));
    };
    let nets_of: BTreeMap<Endpoint, Vec<&str>> = {
        let mut map: BTreeMap<Endpoint, Vec<&str>> = BTreeMap::new();
        for net in &view.nets {
            for endpoint in &net.endpoints {
                map.entry(endpoint.clone()).or_default().push(net.name.as_str());
            }
        }
        map
    };
    let excluded: BTreeSet<&str> = cell
        .hot_supply_net
        .iter()
        .chain(cell.hot_return_net.iter())
        .chain(cell.switch_nets.iter())
        .map(String::as_str)
        .collect();
    let named: Vec<Endpoint> = part
        .pins
        .keys()
        .filter(|pin| pin.to_ascii_uppercase().starts_with("VOUT"))
        .map(|pin| Endpoint::new(&cell.controller, pin))
        .collect();
    if named.is_empty() {
        return Err(
            "the controller has no pin whose name starts with VOUT: the output loop cannot be located".to_owned()
        );
    }
    let net = named
        .iter()
        .find_map(|pin| nets_of.get(pin).and_then(|nets| nets.iter().find(|n| !excluded.contains(**n)).copied()))
        .ok_or_else(|| "the VOUT pins are not connected to an output net".to_owned())?
        .to_owned();
    let pins: Vec<Endpoint> =
        named.into_iter().filter(|pin| nets_of.get(pin).is_some_and(|nets| nets.iter().any(|n| **n == net))).collect();
    let Some(ret) = cell.hot_return_net.clone() else {
        return Err("the cell has no hot return net".to_owned());
    };
    let mut capacitors: Vec<HotCapacitor> = Vec::new();
    let members = view.nets.iter().find(|n| n.name == net).map(|n| n.endpoints.as_slice()).unwrap_or_default();
    for endpoint in members {
        let Some(component) = view.components.iter().find(|c| c.reference == endpoint.component) else { continue };
        if endpoint.component == cell.controller
            || !is_capacitor_part(view, component)
            || capacitors.iter().any(|c| c.component == endpoint.component)
        {
            continue;
        }
        let Some(capacitor_part) = view.library.get(&component.part) else { continue };
        let Some(other) = capacitor_part.pins.keys().find(|pin| **pin != endpoint.pin) else { continue };
        let other = Endpoint::new(&endpoint.component, other);
        if !nets_of.get(&other).is_some_and(|nets| nets.iter().any(|n| **n == ret)) {
            continue;
        }
        let declared = component
            .properties
            .get("decouples")
            .and_then(|target| Endpoint::parse(target).ok())
            .is_some_and(|target| pins.contains(&target));
        capacitors.push(HotCapacitor {
            component: endpoint.component.clone(),
            supply_pin: endpoint.clone(),
            return_pin: other,
            declared,
        });
    }
    capacitors.sort_by_key(|capacitor| !capacitor.declared);
    Ok(OutputStage { pins, net, capacitors })
}

fn hot_loop(cx: &mut Context, cell: &SwitchingCell) {
    let check = format!("EMI-HOT-LOOP:{}", cell.controller);
    if cell.hot_supply_pins.is_empty() || cell.hot_return_pins.is_empty() {
        cx.cover(check, CoverageStatus::NotApplicable, "the cell has no hot supply or no hot return pin");
        return;
    }
    if cell.topology == Some(Topology::BuckBoost) {
        // Four-switch buck-boost: the buck leg's input loop and the boost leg's output loop.
        let mut nets: Vec<String> = cell.hot_supply_net.iter().chain(cell.hot_return_net.iter()).cloned().collect();
        dedup(&mut nets);
        let input = LoopSpec {
            kind: LoopKind::Input,
            name: Some("input loop"),
            key: Some("input"),
            capacitors: cell.hot_capacitors.clone(),
            nets,
            output_pins: Vec::new(),
        };
        analyze_loop(cx, cell, &input);
        match output_stage(cx.layout.view, cell) {
            Ok(stage) => {
                let mut nets = vec![stage.net.clone()];
                nets.extend(cell.hot_return_net.iter().cloned());
                dedup(&mut nets);
                let output = LoopSpec {
                    kind: LoopKind::Output,
                    name: Some("output loop"),
                    key: Some("output"),
                    capacitors: stage.capacitors,
                    nets,
                    output_pins: stage.pins,
                };
                analyze_loop(cx, cell, &output);
            }
            Err(reason) => {
                cx.cover(format!("{check}:output"), CoverageStatus::NotApplicable, format!("output loop: {reason}"));
            }
        }
        return;
    }
    let mut nets: Vec<String> = Vec::new();
    nets.extend(cell.hot_supply_net.iter().cloned());
    nets.extend(cell.hot_return_net.iter().cloned());
    nets.extend(cell.switch_nets.iter().cloned());
    dedup(&mut nets);
    let standard = LoopSpec {
        kind: LoopKind::Standard,
        name: None,
        key: None,
        capacitors: cell.hot_capacitors.clone(),
        nets,
        output_pins: Vec::new(),
    };
    analyze_loop(cx, cell, &standard);
}

fn analyze_loop(cx: &mut Context, cell: &SwitchingCell, spec: &LoopSpec) {
    let check = match spec.key {
        Some(key) => format!("EMI-HOT-LOOP:{}:{key}", cell.controller),
        None => format!("EMI-HOT-LOOP:{}", cell.controller),
    };
    let what = spec.name.map_or(String::new(), |name| format!("{name}: "));
    if spec.capacitors.is_empty() {
        let reason = match spec.kind {
            LoopKind::Output => "no capacitor joins the output net and the hot return net; the loop cannot be formed",
            _ => {
                "no capacitor joins the hot supply and hot return nets (see EMI_HOT_LOOP_NO_CAPACITOR); the loop cannot be formed"
            }
        };
        cx.cover(check, CoverageStatus::NotApplicable, format!("{what}{reason}"));
        return;
    }
    let grounds = ground_nets(cx.layout.view, cell);
    let net_refs: Vec<&str> = spec.nets.iter().map(String::as_str).collect();
    let copper = NetCopper::collect(&cx.layout, &net_refs);
    let routed =
        !(copper.tracks.is_empty() && copper.arcs.is_empty() && copper.vias.is_empty() && copper.polygons.is_empty());

    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for capacitor in &spec.capacitors {
        match build_candidate(cx, cell, spec, capacitor, &grounds, &copper) {
            Ok(candidate) => candidates.push(candidate),
            Err(reason) => failures.push(format!("{}: {reason}", capacitor.component)),
        }
    }
    // A capacitor that declares `decouples` of a hot-supply pin is preferred; the least inductance
    // among undeclared ones is the fallback when no declared capacitor can be placed.
    let key = |c: &LoopCandidate| c.inductance.unwrap_or(c.area * 1e3);
    let declared_available = candidates.iter().any(|c| c.declared);
    let Some(best) =
        candidates.iter().filter(|c| c.declared || !declared_available).min_by(|a, b| key(a).total_cmp(&key(b)))
    else {
        cx.cover(
            check,
            CoverageStatus::NotApplicable,
            format!("{what}no loop could be built: {}", failures.join("; ")),
        );
        return;
    };
    let selection = if declared_available {
        "declared: the capacitor declares decouples of a hot-supply pin; least inductance among the declared ones"
    } else {
        "fallback: no capacitor declares decouples of a hot-supply pin; least inductance among the undeclared ones"
    };

    let loop_nets = spec.nets.clone();
    let mut components: Vec<String> = vec![cell.controller.clone(), best.capacitor.clone()];
    if let Some(diode) = &cell.catch_diode
        && spec.kind == LoopKind::Standard
    {
        components.push(diode.component.clone());
    }
    let refs: Vec<&str> = components.iter().map(String::as_str).collect();
    let state = cx.layout.placement_state(&refs);
    let provisional = state == PlacementState::Provisional;
    let mut tags = Vec::new();
    match best.state {
        TraceState::Full => tags.push("traced_copper".to_owned()),
        TraceState::Partial => tags.push("partially_traced".to_owned()),
        TraceState::NoCopper | TraceState::NotConnected => tags.push("placement_estimate".to_owned()),
    }
    if best.unrouted_legs() > 0 {
        tags.push("unrouted_leg".to_owned());
    }
    if best.legs.iter().any(|leg| leg.plane_shortcut) {
        tags.push("plane_shortcut".to_owned());
    }
    if state == PlacementState::ClusterRelative {
        tags.push("macro_relative_placement".to_owned());
    }
    if provisional {
        tags.push("provisional_placement".to_owned());
    }

    let loop_side = cx.layout.side(&cell.controller).map_or(CopperLayer::Front, copper_layer);
    if spec.kind == LoopKind::Standard {
        let basis = match best.state {
            TraceState::Full => "traced copper path",
            TraceState::Partial => "partially traced copper path, unrouted legs straight",
            TraceState::NoCopper | TraceState::NotConnected => "placement estimate from pad centres",
        };
        cx.loops.insert(
            cell.controller.clone(),
            LoopSummary {
                capacitor: best.capacitor.clone(),
                inductance: best.inductance,
                basis: basis.to_owned(),
                tags: tags.clone(),
                layer: loop_side.value().to_owned(),
            },
        );
    }
    let mut message = format!(
        "{}: {} {} area {:.2} mm2, path {:.1} mm",
        cell.controller,
        spec.name.unwrap_or("hot loop"),
        best.capacitor,
        best.area * 1e6,
        mm(best.length)
    );
    match best.inductance {
        Some(l) => message.push_str(&format!(", about {:.1} nH", l * 1e9)),
        None => message.push_str(", inductance not computed"),
    }
    let traced_vias: usize = best.legs.iter().map(|l| l.vias).sum();
    match best.state {
        TraceState::Full => message.push_str(&format!(
            " (traced copper: {} leg(s), {} via(s), {} layer change(s){}",
            best.traced_legs(),
            traced_vias,
            best.legs.iter().map(|l| l.layer_changes).sum::<usize>(),
            best.legs
                .iter()
                .filter_map(|l| l.narrowest)
                .reduce(f64::min)
                .map_or(String::new(), |w| format!(", narrowest {:.2} mm", mm(w)))
        )),
        TraceState::Partial => message.push_str(&format!(
            " (partially traced: {} of {} copper legs traced, {} unrouted and kept straight",
            best.traced_legs(),
            best.traced_legs() + best.unrouted_legs(),
            best.unrouted_legs()
        )),
        TraceState::NotConnected => message.push_str(
            " (pad-centre polygon, placement estimate; routed copper exists on the loop nets but joins no pad pair of the loop",
        ),
        TraceState::NoCopper => message.push_str(" (pad-centre polygon, placement estimate"),
    }
    if provisional {
        message.push_str("; positions are provisional, not fixed by a placement constraint");
    }
    message.push(')');

    let mut finding = base_finding(
        "EMI-HOT-LOOP",
        cell,
        message,
        components.clone(),
        loop_nets.clone(),
        vec![loop_side.value().to_owned()],
    );
    finding.tags.extend(tags.clone());
    finding.rank = best.area * 1e6;
    finding.subjects.extend(components.iter().cloned());
    finding.subjects.extend(endpoint_names(&cell.hot_supply_pins));
    finding.subjects.extend(endpoint_names(&cell.hot_return_pins));
    finding.subjects.extend(endpoint_names(&cell.switch_pins));
    finding.subjects.extend(endpoint_names(&spec.output_pins));
    dedup(&mut finding.subjects);

    if let Some(name) = spec.name {
        put(&mut finding.measures, "loop", Value::from(name));
    }
    put(&mut finding.measures, "loop_capacitor", Value::from(best.capacitor.as_str()));
    put(&mut finding.measures, "capacitor_selection", Value::from(selection));
    put(&mut finding.measures, "loop_area_mm2", num(best.area * 1e6, 4));
    put(&mut finding.measures, "path_length_mm", num(mm(best.length), 4));
    put(
        &mut finding.measures,
        "pad_sequence",
        Value::Array(best.labels.iter().map(|l| Value::from(l.as_str())).collect()),
    );
    put(
        &mut finding.measures,
        "loop_polygon_mm",
        Value::Array(best.points.iter().map(|p| Value::Array(vec![num(mm(p[0]), 4), num(mm(p[1]), 4)])).collect()),
    );
    // The loop as a 3D polyline for the near-field map (`copper emi-check --plot-field`): millimetres,
    // z = 0 at the centre of the top copper layer and negative below it, the reference plane's z when
    // the loop has one.
    put(
        &mut finding.measures,
        "loop_path_3d_mm",
        Value::Array(
            best.path_3d
                .iter()
                .map(|p| Value::Array(vec![num(mm(p[0]) + 0.0, 4), num(mm(p[1]) + 0.0, 4), num(mm(p[2]) + 0.0, 4)]))
                .collect(),
        ),
    );
    put(&mut finding.measures, "loop_reference_plane_z_mm", best.plane_z.map_or(Value::Null, |z| num(mm(z) + 0.0, 4)));
    put(&mut finding.measures, "field_victims", field::victims(cx, cell));
    put(
        &mut finding.measures,
        "candidates",
        Value::Array(
            candidates
                .iter()
                .map(|c| {
                    let mut item = Object::new();
                    put(&mut item, "capacitor", Value::from(c.capacitor.as_str()));
                    put(&mut item, "declared", Value::Bool(c.declared));
                    put(&mut item, "loop_area_mm2", num(c.area * 1e6, 4));
                    put(&mut item, "inductance_nh", c.inductance.map_or(Value::Null, |l| num(l * 1e9, 3)));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    if let Some(l) = best.inductance {
        put(&mut finding.measures, "loop_inductance_nh", num(l * 1e9, 3));
    }
    put(&mut finding.measures, "inductance_method", Value::from(best.inductance_note.as_str()));
    put(
        &mut finding.measures,
        "reference_plane",
        best.plane.as_ref().map_or(Value::Null, |p| Value::from(p.layer.value())),
    );
    let via_count = copper.vias.len();
    put(&mut finding.measures, "vias_on_loop_nets", Value::from(via_count as i64));
    put(&mut finding.measures, "routed_copper_on_loop_nets", Value::Bool(routed));
    put(&mut finding.measures, "placement", Value::from(state.value()));
    let trace_text = match best.state {
        TraceState::NoCopper => "no_copper",
        TraceState::NotConnected => "not_connected",
        TraceState::Partial => "partially_traced",
        TraceState::Full => "traced_copper",
    };
    put(&mut finding.measures, "trace_status", Value::from(trace_text));
    if !best.legs.is_empty() {
        let traced_length: f64 = best.legs.iter().filter(|l| l.kind == LegKind::Traced).map(|l| l.length).sum();
        put(&mut finding.measures, "traced_path_length_mm", num(mm(traced_length), 4));
        put(&mut finding.measures, "trace_via_count", Value::from(traced_vias as i64));
        put(
            &mut finding.measures,
            "trace_layer_changes",
            Value::from(best.legs.iter().map(|l| l.layer_changes).sum::<usize>() as i64),
        );
        put(
            &mut finding.measures,
            "trace_narrowest_width_mm",
            best.legs.iter().filter_map(|l| l.narrowest).reduce(f64::min).map_or(Value::Null, |w| num(mm(w), 4)),
        );
        put(&mut finding.measures, "trace_unrouted_legs", Value::from(best.unrouted_legs() as i64));
        put(
            &mut finding.measures,
            "pad_polygon_mm",
            Value::Array(
                best.pad_points.iter().map(|p| Value::Array(vec![num(mm(p[0]), 4), num(mm(p[1]), 4)])).collect(),
            ),
        );
        put(
            &mut finding.measures,
            "legs",
            Value::Array(
                best.legs
                    .iter()
                    .map(|leg| {
                        let mut item = Object::new();
                        put(&mut item, "from", Value::from(leg.from.as_str()));
                        put(&mut item, "to", Value::from(leg.to.as_str()));
                        put(&mut item, "net", leg.net.as_ref().map_or(Value::Null, |n| Value::from(n.as_str())));
                        put(&mut item, "kind", Value::from(leg.kind.value()));
                        put(&mut item, "length_mm", num(mm(leg.length), 4));
                        put(&mut item, "vias", Value::from(leg.vias as i64));
                        put(&mut item, "layer_changes", Value::from(leg.layer_changes as i64));
                        put(&mut item, "narrowest_width_mm", leg.narrowest.map_or(Value::Null, |w| num(mm(w), 4)));
                        put(
                            &mut item,
                            "layers",
                            Value::Array(leg.layers.iter().map(|l| Value::from(l.as_str())).collect()),
                        );
                        put(&mut item, "plane_shortcut", Value::Bool(leg.plane_shortcut));
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
    }

    let mut detail = format!(
        "{what}{} pads, {} candidate capacitor(s) of {} tried ({}); loop polygon area and length from {}{}",
        best.pad_points.len(),
        candidates.len(),
        spec.capacitors.len(),
        if declared_available { "declared capacitor preferred" } else { "no declared capacitor: least inductance" },
        match best.state {
            TraceState::Full | TraceState::Partial => "the traced copper path",
            _ => "pad centres",
        },
        if failures.is_empty() { String::new() } else { format!("; skipped {}", failures.join("; ")) }
    );
    if provisional {
        detail.push_str("; positions are the prototype placement, not a layout decision");
    }
    cx.cover(check.clone(), CoverageStatus::Executed, detail);
    if routed {
        let counts = format!(
            "{} vias, {} tracks, {} polygons, {} zones",
            via_count,
            copper.tracks.len() + copper.arcs.len(),
            copper.polygons.len(),
            copper.zones.len()
        );
        let trace_check = format!("{check}:copper");
        match best.state {
            TraceState::Full | TraceState::Partial => cx.cover(
                trace_check,
                CoverageStatus::Executed,
                format!(
                    "{what}routed or hard-macro copper on the loop nets ({counts}); {} of {} copper leg(s) traced as the shortest copper path, \
                     {} unrouted leg(s) keep the straight pad-to-pad segment; {} leg(s) cross a component and stay straight; \
                     traced length {:.2} mm, {} via(s), {} layer change(s); a copper leg through a polygon or zone is a straight line (plane_shortcut)",
                    best.traced_legs(),
                    best.traced_legs() + best.unrouted_legs(),
                    best.unrouted_legs(),
                    best.legs.iter().filter(|l| l.kind == LegKind::Component).count(),
                    mm(best.legs.iter().filter(|l| l.kind == LegKind::Traced).map(|l| l.length).sum::<f64>()),
                    traced_vias,
                    best.legs.iter().map(|l| l.layer_changes).sum::<usize>()
                ),
            ),
            _ => cx.cover(
                trace_check,
                CoverageStatus::Unsupported,
                format!(
                    "{what}routed or hard-macro copper exists on the loop nets ({counts}) but no copper path joins a pad pair of the loop; \
                     the polygon through pad centres is used"
                ),
            ),
        }
    }
    cx.trace_loops += 1;
    cx.trace_copper_seen |= routed;
    cx.trace_legs_traced += best.traced_legs();
    cx.trace_legs_unrouted += best.unrouted_legs();
    if best.traced_legs() > 0 {
        cx.trace_loops_traced += 1;
    }

    // Radiated differential-mode estimate, ringing, ring energy.
    let drive = drive_of(cx.layout.view, cell);
    radiated_estimate(cx, cell, best, &check, &drive, &mut finding);
    ringing_estimate(cx, cell, best, &check, &drive, &mut finding);
    finding.cell = Some(cell.controller.clone());
    cx.findings.push(finding);
}

/// Append an `input:` line to the finding's `measures.inputs_text`.
fn push_input(finding: &mut EmiFinding, line: String) {
    match finding.measures.get_mut("inputs_text") {
        Some(Value::Array(lines)) => lines.push(Value::from(line)),
        _ => put(&mut finding.measures, "inputs_text", Value::Array(vec![Value::from(line)])),
    }
}

/// Add a tag once.
fn add_tag(finding: &mut EmiFinding, tag: &str) {
    if !finding.tags.iter().any(|t| t == tag) {
        finding.tags.push(tag.to_owned());
    }
}

/// The `input:` lines, tag and coverage text that describe the edge a finding used.
fn record_edge(finding: &mut EmiFinding, drive: &Drive, edge: &Edge) {
    if let Some(tag) = edge.tag() {
        add_tag(finding, tag);
    }
    push_input(finding, edge.input_line(drive.swing));
    for note in &drive.edge_notes {
        push_input(finding, note.clone());
    }
}

/// Text of a level band: `[low .. high] dBuV/m`.
fn band_text(band: [f64; 2], unit: &str) -> String {
    format!("[{:.1} .. {:.1}] {unit}", band[0], band[1])
}

fn radiated_estimate(
    cx: &mut Context,
    cell: &SwitchingCell,
    best: &LoopCandidate,
    base: &str,
    drive: &Result<Drive, String>,
    finding: &mut EmiFinding,
) {
    let _ = cell;
    let check = format!("{base}:radiated");
    let drive = match drive {
        Ok(drive) => drive,
        Err(reason) => {
            cx.cover(check, CoverageStatus::NotApplicable, format!("differential-mode field skipped: {reason}"));
            return;
        }
    };
    let (Some(edge), Some(current)) = (drive.edge.as_ref(), drive.current) else {
        let mut missing: Vec<&str> = Vec::new();
        if drive.edge.is_none() {
            missing.push(&drive.edge_missing);
        }
        if drive.current.is_none() {
            missing.push("the switch pin declares no current_peak");
        }
        cx.cover(
            check,
            CoverageStatus::NotApplicable,
            format!("differential-mode field skipped: {}", missing.join("; ")),
        );
        return;
    };
    let period = 1.0 / drive.frequency;
    let high = BAND.1.min(copper_emi::C0 / (SMALL_LOOP_FRACTION * best.length));
    let field = |f: f64, t: f64| {
        let amplitude = estimate::trapezoid_envelope(current, drive.duty, period, t, f);
        estimate::differential_mode_field(amplitude, f, best.area, cx.standard.distance)
    };
    let Some(banded) = sweep_edge(edge, BAND.0, high, cx.standard.line.as_ref(), field) else {
        cx.cover(
            check,
            CoverageStatus::NotApplicable,
            format!(
                "differential-mode field skipped: the {:.0} mm loop path is not electrically small anywhere in 30 MHz to 1 GHz",
                mm(best.length)
            ),
        );
        return;
    };
    let worst = banded.nominal;
    let method = format!(
        "small-loop formula E = 1.316e-14 I f^2 A / r on the trapezoid envelope of the switch current \
         (amplitude current_peak, {} Hz {}, edge {}, {}), worst point of 30 MHz to {:.0} MHz at {} m{}",
        rounded(drive.frequency, 0),
        drive.frequency_note,
        edge.describe(drive.swing),
        drive.duty_note,
        high * 1e-6,
        cx.standard.distance,
        if banded.band.is_some() {
            "; the value is the nominal edge, the band covers the fast and slow bound and the margin is the fast bound's"
        } else {
            ""
        }
    );
    finding.estimate = Some(Estimate {
        quantity: "dm_radiated_field".to_owned(),
        value: rounded(worst.level_dbuvpm, 2),
        unit: "dBuV/m".to_owned(),
        method,
        uncertainty_db: UNCERTAINTY_DB,
        frequency_hz: Some(rounded(worst.frequency, 0)),
        band_db: banded.band.map(|b| [rounded(b[0], 2), rounded(b[1], 2)]),
        basis: banded.band.map(|_| defaults::BASIS.to_owned()),
    });
    finding.margin_db = banded.margin_db.map(|m| rounded(m, 2));
    finding.finding.message.push_str(&format!(
        "; differential-mode field about {:.1} dBuV/m at {:.0} MHz, {} m",
        worst.level_dbuvpm,
        worst.frequency * 1e-6,
        cx.standard.distance
    ));
    if let Some(band) = banded.band {
        finding.finding.message.push_str(&format!(
            " (band {} for the slow .. fast edge of a rule-of-thumb slew rate, nominal shown)",
            band_text(band, "dBuV/m")
        ));
    }
    if let Some(margin) = banded.margin_db {
        finding.finding.message.push_str(&format!(
            " ({:+.1} dB to the {} line{}, uncertainty {UNCERTAINTY_DB} dB on top of the band)",
            margin,
            cx.standard.name.as_deref().unwrap_or(""),
            if banded.band.is_some() { " at the fast bound" } else { "" }
        ));
    }
    record_edge(finding, drive, edge);
    push_input(
        finding,
        format!(
            "current amplitude {} A = current_peak, the current limit (worst case); typical operation would be {} A, -8.0 dB, not applied to this estimate {}",
            rounded(current, 4),
            rounded(current * defaults::OPERATING_CURRENT_FRACTION, 4),
            defaults::rule_text_unreplaceable(&defaults::current_rule())
        ),
    );
    cx.cover(
        check,
        CoverageStatus::Executed,
        format!(
            "{}; edge time {}{}; free-space value, the +6 dB ground-plane reflection is not applied{}",
            drive.duty_note,
            edge.describe(drive.swing),
            edge.rule_note().map(|r| format!(" {r}")).unwrap_or_default(),
            drive.edge_notes.iter().map(|n| format!("; {n}")).collect::<String>()
        ),
    );
}

fn ringing_estimate(
    cx: &mut Context,
    cell: &SwitchingCell,
    best: &LoopCandidate,
    base: &str,
    drive: &Result<Drive, String>,
    finding: &mut EmiFinding,
) {
    let check = format!("{base}:ringing");
    let view = cx.layout.view;
    let capacitance_of = |component: &str| {
        let instance = view.components.iter().find(|c| c.reference == component)?;
        view.library.get(&instance.part)?.emi.as_ref()?.output_capacitance.as_ref().map(quantity)
    };
    let current = drive.as_ref().ok().and_then(|d| d.current).or_else(|| cell.current_peak.as_ref().map(quantity));
    let reverse_voltage = supply_voltage(view, cell.hot_supply_net.as_ref());
    // (device, capacitance band, rule text when the band is a default)
    let mut parts: Vec<(String, defaults::CapacitanceBand, Option<String>)> = Vec::new();
    let mut unavailable: Vec<String> = Vec::new();
    match capacitance_of(&cell.controller) {
        Some(c) => {
            parts.push((cell.controller.clone(), defaults::CapacitanceBand { low: c, nominal: c, high: c }, None))
        }
        None => parts.push((cell.controller.clone(), defaults::fet_capacitance(), Some(defaults::fet_rule()))),
    }
    if let Some(diode) = &cell.catch_diode {
        match (capacitance_of(&diode.component), current) {
            (Some(c), _) => {
                parts.push((diode.component.clone(), defaults::CapacitanceBand { low: c, nominal: c, high: c }, None));
            }
            (None, Some(i)) => {
                let derated = reverse_voltage.is_some_and(|v| v >= defaults::DIODE_HIGH_VOLTAGE);
                parts.push((
                    diode.component.clone(),
                    defaults::diode_capacitance(i, reverse_voltage),
                    Some(defaults::diode_rule(derated)),
                ));
            }
            (None, None) => unavailable.push(format!(
                "{} declares no output_capacitance and the default needs current_peak as the diode rating proxy",
                diode.component
            )),
        }
    }
    let mut notes = Vec::new();
    if let (Some(l), Some(i)) = (best.inductance, current) {
        let energy = estimate::ring_energy(l, i);
        put(&mut finding.measures, "ring_energy_nj", num(energy * 1e9, 3));
        finding.finding.message.push_str(&format!("; stored energy L I^2/2 = {:.1} nJ", energy * 1e9));
        notes.push(format!("ring energy {:.1} nJ", energy * 1e9));
    } else {
        notes.push("ring energy needs the loop inductance and current_peak".to_owned());
    }
    if !unavailable.is_empty() {
        cx.cover(
            check,
            CoverageStatus::NotApplicable,
            format!("ringing frequency skipped: {}; {}", unavailable.join("; "), notes.join("; ")),
        );
        return;
    }
    let Some(l) = best.inductance else {
        cx.cover(check, CoverageStatus::NotApplicable, "ringing frequency skipped: no loop inductance was computed");
        return;
    };
    let total = parts
        .iter()
        .map(|(_, band, _)| *band)
        .reduce(defaults::CapacitanceBand::plus)
        .expect("the controller is always a part");
    let defaulted = parts.iter().any(|(_, _, rule)| rule.is_some());
    let ring = estimate::ring_frequency(l, total.nominal);
    put(&mut finding.measures, "switch_node_capacitance_pf", num(total.nominal * 1e12, 3));
    put(&mut finding.measures, "ring_frequency_mhz", num(ring * 1e-6, 2));
    finding.finding.message.push_str(&format!(
        "; ringing about {:.0} MHz with {:.0} pF",
        ring * 1e-6,
        total.nominal * 1e12
    ));
    if defaulted {
        // A larger capacitance rings lower and overshoots less: the high capacitance is the low end of both.
        let ring_band = [estimate::ring_frequency(l, total.high), estimate::ring_frequency(l, total.low)];
        put(
            &mut finding.measures,
            "switch_node_capacitance_band_pf",
            Value::Array(vec![num(total.low * 1e12, 3), num(total.high * 1e12, 3)]),
        );
        put(
            &mut finding.measures,
            "ring_frequency_band_mhz",
            Value::Array(vec![num(ring_band[0] * 1e-6, 2), num(ring_band[1] * 1e-6, 2)]),
        );
        put(&mut finding.measures, "ring_basis", Value::from(defaults::BASIS));
        finding.finding.message.push_str(&format!(
            " (rule-of-thumb capacitance band [{:.0} .. {:.0}] pF gives [{:.0} .. {:.0}] MHz)",
            total.low * 1e12,
            total.high * 1e12,
            ring_band[0] * 1e-6,
            ring_band[1] * 1e-6
        ));
        add_tag(finding, defaults::TAG_CAPACITANCE);
        for (name, band, rule) in &parts {
            if let Some(rule) = rule {
                push_input(
                    finding,
                    format!(
                        "switch-node capacitance of {name} {:.0} pF nominal [{:.0} .. {:.0}] pF {}",
                        band.nominal * 1e12,
                        band.low * 1e12,
                        band.high * 1e12,
                        defaults::rule_text(rule, "output_capacitance on the part")
                    ),
                );
            }
        }
    }
    if let Some(i) = current {
        let overshoot = estimate::ring_overshoot(l, total.nominal, i);
        put(&mut finding.measures, "ring_overshoot_v", num(overshoot, 2));
        finding.finding.message.push_str(&format!(", overshoot up to {overshoot:.1} V"));
        if defaulted {
            let band = [estimate::ring_overshoot(l, total.high, i), estimate::ring_overshoot(l, total.low, i)];
            put(&mut finding.measures, "ring_overshoot_band_v", Value::Array(vec![num(band[0], 2), num(band[1], 2)]));
            finding.finding.message.push_str(&format!(" [{:.1} .. {:.1}] V", band[0], band[1]));
        }
    }
    let names = parts.iter().map(|(name, _, _)| name.as_str()).collect::<Vec<_>>().join(" + ");
    let basis = if defaulted {
        format!(
            "; capacitance of {} is a rule of thumb ({}), the ring frequency and overshoot are bands",
            parts
                .iter()
                .filter(|(_, _, rule)| rule.is_some())
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(" + "),
            parts.iter().filter_map(|(_, _, rule)| rule.as_deref()).collect::<Vec<_>>().join("; ")
        )
    } else {
        String::new()
    };
    cx.cover(
        check,
        CoverageStatus::Executed,
        format!("L C resonance with the switch-node capacitance of {names}; {}{basis}", notes.join("; ")),
    );
}

// ---------------------------------------------------------------------------
// EMI-SWITCH-NODE
// ---------------------------------------------------------------------------

struct Island {
    area: f64,
    perimeter: f64,
    layer: CopperLayer,
}

fn zone_island(zone: &CopperZone, layer: CopperLayer) -> Island {
    let outer = ring_m(&zone.outer);
    let holes: f64 = zone.holes.iter().map(|h| polygon_area(&ring_m(h))).sum();
    Island { area: (polygon_area(&outer) - holes).max(0.0), perimeter: closed_length(&outer), layer }
}

fn switch_node(cx: &mut Context, cell: &SwitchingCell) {
    let grounds = ground_nets(cx.layout.view, cell);
    let drive = drive_of(cx.layout.view, cell);
    for net in &cell.switch_nets {
        let check = format!("EMI-SWITCH-NODE:{net}");
        let pads = cx.layout.pads_of_net(net);
        if pads.is_empty() {
            cx.cover(check, CoverageStatus::NotApplicable, format!("no placed pad is on the switch net {net}"));
            continue;
        }
        let copper = NetCopper::collect(&cx.layout, &[net.as_str()]);
        let pads_only = copper.is_empty();
        let mut islands: Vec<Island> = Vec::new();
        let mut shapes: Vec<Shape> = pads.iter().map(|p| p.shape.clone()).collect();
        for pad in &pads {
            let layer = cx.layout.side(&pad.component).map_or(CopperLayer::Front, copper_layer);
            islands.push(Island { area: pad.shape.area(), perimeter: pad.shape.perimeter(), layer });
        }
        let pad_area: f64 = islands.iter().map(|i| i.area).sum();
        for track in &copper.tracks {
            let (a, b) = (
                [nm_to_m(track.start.x_nm), nm_to_m(track.start.y_nm)],
                [nm_to_m(track.end.x_nm), nm_to_m(track.end.y_nm)],
            );
            let shape = Shape::track(a, b, nm_to_m(track.width_nm));
            islands.push(Island { area: shape.area(), perimeter: shape.perimeter(), layer: track.layer });
            shapes.push(shape);
        }
        for arc in &copper.arcs {
            // The chord stands in for the arc length (screening only).
            let (a, b) =
                ([nm_to_m(arc.start.x_nm), nm_to_m(arc.start.y_nm)], [nm_to_m(arc.end.x_nm), nm_to_m(arc.end.y_nm)]);
            let shape = Shape::track(a, b, nm_to_m(arc.width_nm));
            islands.push(Island { area: shape.area(), perimeter: shape.perimeter(), layer: arc.layer });
            shapes.push(shape);
        }
        for polygon in &copper.polygons {
            let ring = ring_m(&polygon.vertices);
            islands.push(Island { area: polygon_area(&ring), perimeter: closed_length(&ring), layer: polygon.layer });
        }
        for zone in &copper.zones {
            for layer in &zone.layers {
                islands.push(zone_island(zone, *layer));
            }
        }
        let area: f64 = islands.iter().map(|i| i.area).sum();

        // Capacitance to the nearest ground plane of each island's layer.
        let (mut capacitance, mut unreferenced, mut plane_note) = (0.0, 0usize, BTreeSet::new());
        for island in &islands {
            match nearest_plane(cx.layout.board, &cx.stack, &grounds, island.layer) {
                Some(plane) if island.area > 0.0 && island.perimeter > 0.0 => {
                    capacitance += estimate::island_capacitance(island.area, island.perimeter, plane.height, plane.er);
                    plane_note.insert(format!("{} at {:.3} mm", plane.layer.value(), mm(plane.height)));
                }
                Some(_) => {}
                None => unreferenced += 1,
            }
        }
        let have_c = capacitance > 0.0 && unreferenced == 0;

        // Distances.
        let ring = cx.layout.ring();
        let edge = shapes.iter().map(|s| ring_gap(s, &ring)).fold(f64::INFINITY, f64::min);
        let mut cables: Vec<(String, f64)> = Vec::new();
        for connector in &cx.connectors {
            let gap = cx
                .layout
                .pads_of_component(connector)
                .iter()
                .flat_map(|cp| shapes.iter().map(|s| shape_gap(s, &cp.shape)))
                .fold(f64::INFINITY, f64::min);
            if gap.is_finite() {
                cables.push((connector.clone(), gap));
            }
        }
        cables.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut message = format!(
            "{}: switch net {net} has {:.2} mm2 of copper ({}), {:.1} mm from the board edge",
            cell.controller,
            area * 1e6,
            if pads_only { "pads only, no routed copper" } else { "pads, tracks and pours" },
            mm(edge)
        );
        if have_c {
            message.push_str(&format!(", about {:.2} pF to the ground plane", capacitance * 1e12));
        }
        if let Some((name, gap)) = cables.first() {
            message.push_str(&format!(", {:.1} mm from cable connector {name}", mm(*gap)));
        }
        let mut finding = base_finding(
            "EMI-SWITCH-NODE",
            cell,
            message,
            vec![cell.controller.clone()],
            vec![net.clone()],
            islands.iter().map(|i| i.layer.value().to_owned()).collect::<BTreeSet<_>>().into_iter().collect(),
        );
        if pads_only {
            finding.tags.push("pads_only".to_owned());
        }
        let mut involved: Vec<&str> = vec![cell.controller.as_str()];
        involved.extend(cx.connectors.iter().map(String::as_str));
        if cx.layout.placement_state(&involved) != PlacementState::Fixed {
            finding.tags.push("provisional_placement".to_owned());
            finding
                .finding
                .message
                .push_str(" [distances to the board edge and connectors use a position no constraint fixes]");
        }
        finding.rank = area * 1e6;
        finding.subjects.extend(endpoint_names(&cell.switch_pins));
        finding.subjects.extend(pads.iter().map(|p| p.component.clone()));
        dedup(&mut finding.subjects);
        put(&mut finding.measures, "copper_area_mm2", num(area * 1e6, 4));
        put(&mut finding.measures, "pad_area_mm2", num(pad_area * 1e6, 4));
        put(&mut finding.measures, "pads", Value::from(pads.len() as i64));
        put(&mut finding.measures, "routed_copper", Value::Bool(!pads_only));
        put(&mut finding.measures, "distance_to_board_edge_mm", num(mm(edge), 3));
        put(
            &mut finding.measures,
            "distance_to_cable_connectors_mm",
            Value::Array(
                cables
                    .iter()
                    .map(|(name, gap)| {
                        let mut item = Object::new();
                        put(&mut item, "connector", Value::from(name.as_str()));
                        put(&mut item, "distance_mm", num(mm(*gap), 3));
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
        if have_c {
            put(&mut finding.measures, "capacitance_to_plane_pf", num(capacitance * 1e12, 4));
            put(
                &mut finding.measures,
                "reference",
                Value::from(plane_note.iter().cloned().collect::<Vec<_>>().join(", ")),
            );
        }
        let covered = if pads_only {
            "pads only: no routed copper is on this net, so the area is a lower bound"
        } else {
            "pads and routed copper of the board (overlaps counted twice)"
        };
        cx.cover(check.clone(), CoverageStatus::Executed, format!("{covered}; distances are pad and track clearances"));

        // Voltage-driven common mode.
        let cm_check = format!("EMI-SWITCH-NODE:{net}:common_mode");
        match (&drive, supply_voltage(cx.layout.view, cell.hot_supply_net.as_ref())) {
            (_, _) if !have_c => cx.cover(
                cm_check,
                CoverageStatus::NotApplicable,
                "voltage-driven common mode skipped: no ground copper_zone on another layer to take the capacitance to",
            ),
            (Err(reason), _) => cx.cover(
                cm_check,
                CoverageStatus::NotApplicable,
                format!("voltage-driven common mode skipped: {reason}"),
            ),
            (Ok(drive), swing) => match (drive.edge.as_ref(), swing) {
                (Some(edge), Some(swing)) if swing > 0.0 => {
                    let period = 1.0 / drive.frequency;
                    let scale = 3.0 / cx.standard.distance;
                    let field = |f: f64, t: f64| {
                        let amplitude = estimate::trapezoid_envelope(swing, drive.duty, period, t, f);
                        scale
                            * estimate::voltage_driven_cm_field(
                                f,
                                capacitance,
                                amplitude,
                                estimate::VOLTAGE_CM_COEFFICIENT_3M,
                            )
                    };
                    if let Some(banded) = sweep_edge(edge, BAND.0, BAND.1, cx.standard.line.as_ref(), field) {
                        let worst = banded.nominal;
                        finding.estimate = Some(Estimate {
                            quantity: "voltage_driven_cm_field".to_owned(),
                            value: rounded(worst.level_dbuvpm, 2),
                            unit: "dBuV/m".to_owned(),
                            method: format!(
                                "E = 115 f C V at 3 m scaled to {} m, V the trapezoid envelope of a {swing} V swing \
                                 ({} Hz {}, edge {}, {}), C the island capacitance to the ground plane; the constant is \
                                 not independently verified, use for ranking{}",
                                cx.standard.distance,
                                rounded(drive.frequency, 0),
                                drive.frequency_note,
                                edge.describe(drive.swing),
                                drive.duty_note,
                                if banded.band.is_some() {
                                    "; the value is the nominal edge, the band covers the fast and slow bound and the margin is the fast bound's"
                                } else {
                                    ""
                                }
                            ),
                            uncertainty_db: UNCERTAINTY_DB,
                            frequency_hz: Some(rounded(worst.frequency, 0)),
                            band_db: banded.band.map(|b| [rounded(b[0], 2), rounded(b[1], 2)]),
                            basis: banded.band.map(|_| defaults::BASIS.to_owned()),
                        });
                        finding.margin_db = banded.margin_db.map(|m| rounded(m, 2));
                        finding.finding.message.push_str(&format!(
                            "; voltage-driven common-mode field about {:.1} dBuV/m at {:.0} MHz, {} m",
                            worst.level_dbuvpm,
                            worst.frequency * 1e-6,
                            cx.standard.distance
                        ));
                        if let Some(band) = banded.band {
                            finding.finding.message.push_str(&format!(
                                " (band {} for the slow .. fast edge of a rule-of-thumb slew rate, nominal shown)",
                                band_text(band, "dBuV/m")
                            ));
                        }
                        if let Some(margin) = banded.margin_db {
                            finding.finding.message.push_str(&format!(
                                " ({:+.1} dB to the {} line{}, uncertainty {UNCERTAINTY_DB} dB on top of the band)",
                                margin,
                                cx.standard.name.as_deref().unwrap_or(""),
                                if banded.band.is_some() { " at the fast bound" } else { "" }
                            ));
                        }
                        record_edge(&mut finding, drive, edge);
                        cx.cover(
                            cm_check,
                            CoverageStatus::Executed,
                            format!(
                                "island capacitance to the nearest ground plane; edge time {}{}; the constant 115 is unverified{}",
                                edge.describe(drive.swing),
                                edge.rule_note().map(|r| format!(" {r}")).unwrap_or_default(),
                                drive.edge_notes.iter().map(|n| format!("; {n}")).collect::<String>()
                            ),
                        );
                    }
                }
                (None, _) => cx.cover(
                    cm_check,
                    CoverageStatus::NotApplicable,
                    format!("voltage-driven common mode skipped: {}", drive.edge_missing),
                ),
                (_, _) => cx.cover(
                    cm_check,
                    CoverageStatus::NotApplicable,
                    "voltage-driven common mode skipped: the hot supply net declares no supply voltage for the swing",
                ),
            },
        }
        cx.findings.push(finding);
    }
}

// ---------------------------------------------------------------------------
// EMI-PROXIMITY
// ---------------------------------------------------------------------------

struct Source {
    name: String,
    kind: &'static str,
    pads: Vec<PadGeom>,
    factor: Option<f64>,
    shielding: String,
}

fn proximity(cx: &mut Context, cell: &SwitchingCell) {
    let check = format!("EMI-PROXIMITY:{}", cell.controller);
    let view = cx.layout.view;
    let magnetic = cell.magnetic.as_ref().map(|m| m.component.clone());
    let sized = magnetic.as_deref().unwrap_or(&cell.controller);
    let Some(size) = cx.layout.size_of(sized) else {
        cx.cover(check, CoverageStatus::NotApplicable, format!("{sized} is not placed or has no footprint size"));
        return;
    };
    let shielding = magnetic.as_ref().and_then(|name| {
        let instance = view.components.iter().find(|c| &c.reference == name)?;
        view.library.get(&instance.part)?.emi.as_ref()?.shielding
    });
    let (magnetic_factor, shielding_text) = match shielding {
        Some(Shielding::Shielded) => (None, "shielded"),
        Some(Shielding::SemiShielded) => (Some(1.5), "semi_shielded"),
        Some(Shielding::Unshielded) => (Some(3.0), "unshielded"),
        None => (Some(3.0), "unknown"),
    };
    let mut sources: Vec<Source> = Vec::new();
    if let Some(name) = &magnetic {
        sources.push(Source {
            name: name.clone(),
            kind: "magnetic",
            pads: cx.layout.pads_of_component(name),
            factor: magnetic_factor,
            shielding: shielding_text.to_owned(),
        });
    }
    let switch_pads: Vec<PadGeom> = cell.switch_nets.iter().flat_map(|net| cx.layout.pads_of_net(net)).collect();
    sources.push(Source {
        name: cell.switch_nets.join("/"),
        kind: "switch_node",
        pads: switch_pads,
        factor: Some(SWITCH_NODE_FACTOR),
        shielding: "n/a".to_owned(),
    });

    // The cell's own parts sit next to the switch node by construction.
    let mut own: BTreeSet<String> = BTreeSet::new();
    own.insert(cell.controller.clone());
    own.extend(cell.magnetic.iter().map(|m| m.component.clone()));
    own.extend(cell.catch_diode.iter().map(|d| d.component.clone()));
    own.extend(cell.bootstrap_capacitor.iter().map(|b| b.component.clone()));
    own.extend(cell.hot_capacitors.iter().map(|c| c.component.clone()));

    struct Victim {
        name: String,
        kind: &'static str,
        pads: Vec<PadGeom>,
        subjects: Vec<String>,
    }
    let mut victims: Vec<Victim> = Vec::new();
    let mut skipped_nets = 0usize;
    for (net, classes) in &cx.model.nets {
        if !classes.sensitive || cell.switch_nets.contains(net) {
            continue;
        }
        let pads: Vec<PadGeom> =
            cx.layout.pads_of_net(net).into_iter().filter(|p| !own.contains(&p.component)).collect();
        if pads.is_empty() {
            skipped_nets += 1;
            continue;
        }
        let mut subjects = endpoint_names(&classes.sensitive_pins);
        subjects.extend(pads.iter().map(|p| p.component.clone()));
        victims.push(Victim { name: net.clone(), kind: "sensitive_net", pads, subjects });
    }
    for connector in &cx.connectors {
        let pads = cx.layout.pads_of_component(connector);
        if !pads.is_empty() {
            victims.push(Victim {
                name: connector.clone(),
                kind: "cable_connector",
                pads,
                subjects: vec![connector.clone()],
            });
        }
    }

    let mut reported = 0usize;
    for source in &sources {
        let Some(factor) = source.factor else { continue };
        if source.pads.is_empty() {
            continue;
        }
        let threshold = factor * size;
        for victim in &victims {
            let mut closest: Option<(f64, &PadGeom, &PadGeom)> = None;
            for a in &source.pads {
                for b in victim.pads.iter().filter(|b| b.component != a.component) {
                    let gap = shape_gap(&a.shape, &b.shape);
                    if closest.is_none_or(|(best, _, _)| gap < best) {
                        closest = Some((gap, a, b));
                    }
                }
            }
            let Some((gap, near_source, near_victim)) = closest else { continue };
            if gap >= threshold {
                continue;
            }
            reported += 1;
            let what = if victim.kind == "cable_connector" {
                format!("cable connector {}", victim.name)
            } else {
                format!("sensitive net {}", victim.name)
            };
            let from = if source.kind == "magnetic" {
                format!("{} ({})", source.name, source.shielding)
            } else {
                format!("switch node {}", source.name)
            };
            let message = format!(
                "{}: {from} is {:.2} mm from {what} (pads {}.{} and {}.{}), inside the screening distance {factor}x{:.1} mm = {:.1} mm",
                cell.controller,
                mm(gap),
                near_source.component,
                near_source.number,
                near_victim.component,
                near_victim.number,
                mm(size),
                mm(threshold)
            );
            let mut nets = vec![];
            if source.kind == "switch_node" {
                nets.extend(cell.switch_nets.iter().cloned());
            }
            if victim.kind == "sensitive_net" {
                nets.push(victim.name.clone());
            }
            let mut finding = base_finding(
                "EMI-PROXIMITY",
                cell,
                message,
                vec![source.name.clone(), victim.name.clone()],
                nets,
                Vec::new(),
            );
            finding.finding.measured_nm = Some((gap * 1e9).round() as i64);
            finding.finding.required_nm = Some((threshold * 1e9).round() as i64);
            finding.rank = threshold / gap.max(1e-6);
            finding.subjects.extend(victim.subjects.iter().cloned());
            if let Some(name) = &magnetic
                && source.kind == "magnetic"
            {
                finding.subjects.push(name.clone());
            }
            if source.kind == "switch_node" {
                finding.subjects.extend(endpoint_names(&cell.switch_pins));
            }
            dedup(&mut finding.subjects);
            put(&mut finding.measures, "source", Value::from(source.name.as_str()));
            put(&mut finding.measures, "source_kind", Value::from(source.kind));
            put(&mut finding.measures, "source_shielding", Value::from(source.shielding.as_str()));
            put(&mut finding.measures, "victim", Value::from(victim.name.as_str()));
            put(&mut finding.measures, "victim_kind", Value::from(victim.kind));
            put(&mut finding.measures, "distance_mm", num(mm(gap), 3));
            put(&mut finding.measures, "threshold_mm", num(mm(threshold), 3));
            put(&mut finding.measures, "threshold_factor", num(factor, 2));
            put(&mut finding.measures, "reference_size_mm", num(mm(size), 3));
            put(&mut finding.measures, "reference_component", Value::from(sized));
            if source.kind == "magnetic" || source.kind == "switch_node" {
                let pair = [near_source.component.as_str(), near_victim.component.as_str()];
                let state = cx.layout.placement_state(&pair);
                if state == PlacementState::Provisional {
                    finding.tags.push("provisional_placement".to_owned());
                }
            }
            cx.findings.push(finding);
        }
    }
    let shielded = if magnetic_factor.is_none() { "; the shielded magnetic is not a source" } else { "" };
    cx.cover(
        check,
        CoverageStatus::Executed,
        format!(
            "{} source(s) against {} victim(s) ({skipped_nets} sensitive net(s) without placed pads outside the cell); \
             thresholds {}x the {:.1} mm size of {sized} for the magnetic ({shielding_text}), {SWITCH_NODE_FACTOR}x for the switch node{shielded}; {reported} reported",
            sources.len(),
            victims.len(),
            magnetic_factor.map_or("no".to_owned(), |f| f.to_string()),
            mm(size)
        ),
    );
}

// ---------------------------------------------------------------------------
// emi_accept
// ---------------------------------------------------------------------------

fn target_matches(target: &str, subjects: &[String]) -> bool {
    subjects.iter().any(|s| s == target || (!target.contains('.') && s.starts_with(&format!("{target}."))))
}

fn apply_accepts(view: &FlatElectricalView, findings: &mut Vec<EmiFinding>) {
    let mut unused: Vec<EmiFinding> = Vec::new();
    for constraint in view.constraints.iter().filter(|c| c.kind == ConstraintKind::EmiAccept) {
        let text = |name: &str| match constraint.parameters.get(name) {
            Some(ParameterValue::Text(value)) => Some(value.clone()),
            _ => None,
        };
        let (Some(code), Some(reason)) = (text("code"), text("reason")) else { continue };
        let Some(target) = constraint.targets.first() else { continue };
        let mut matched = false;
        for item in findings.iter_mut().filter(|f| f.finding.code == code && target_matches(target, &f.subjects)) {
            matched = true;
            if item.accepted.is_none() {
                item.accepted = Some(reason.clone());
                item.finding.severity = Severity::Info;
            }
        }
        if !matched {
            let message = format!(
                "emi_accept({target}) for {code} matches no finding: the finding is gone or the target does not name it; remove or update the acknowledgement"
            );
            unused.push(EmiFinding {
                finding: Finding::new("EMI-ACCEPT-UNUSED", Severity::Warning, message).objects([target.clone()]),
                cell: None,
                provenance: None,
                tags: Vec::new(),
                estimate: None,
                margin_db: None,
                accepted: None,
                measures: {
                    let mut measures = Object::new();
                    put(&mut measures, "target", Value::from(target.as_str()));
                    put(&mut measures, "code", Value::from(code.as_str()));
                    put(&mut measures, "reason", Value::from(reason.as_str()));
                    measures
                },
                subjects: vec![target.clone()],
                rank: 0.0,
            });
        }
    }
    findings.extend(unused);
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

fn cable_connectors(view: &FlatElectricalView, standard: &Standard) -> BTreeSet<String> {
    let mut connectors: BTreeSet<String> = view
        .components
        .iter()
        .filter(|c| {
            view.library.get(&c.part).and_then(|p| p.emi.as_ref()).is_some_and(|emi| emi.external_cable == Some(true))
        })
        .map(|c| c.reference.clone())
        .collect();
    connectors.extend(standard.targets.iter().filter(|t| view.components.iter().any(|c| &c.reference == *t)).cloned());
    connectors
}

fn cell_summary(cx: &Context, cell: &SwitchingCell, drive: &Result<Drive, String>) -> CellSummary {
    let topology = cell.topology.map_or("unknown topology", |t| t.value());
    let frequency = match drive {
        Ok(d) => format!("{} ({})", si_frequency(d.frequency), d.frequency_note),
        Err(_) => "frequency unknown".to_owned(),
    };
    let heading =
        format!("CELL {} ({}, {topology}, {frequency}, {})", cell.controller, cell.part, cell.provenance.value());
    let q = |value: &Option<Quantity>| value.as_ref().map_or(Value::Null, |v| Value::from(v.to_string()));
    let mut item = Object::new();
    put(&mut item, "controller", Value::from(cell.controller.as_str()));
    put(&mut item, "part", Value::from(cell.part.as_str()));
    put(&mut item, "provenance", Value::from(cell.provenance.value()));
    put(&mut item, "topology", cell.topology.map_or(Value::Null, |t| Value::from(t.value())));
    put(&mut item, "frequency_hz", drive.as_ref().map_or(Value::Null, |d| num(d.frequency, 0)));
    put(&mut item, "frequency_basis", drive.as_ref().map_or(Value::Null, |d| Value::from(d.frequency_note)));
    put(&mut item, "rise_time", q(&cell.rise_time));
    put(&mut item, "fall_time", q(&cell.fall_time));
    put(&mut item, "slew_rate", q(&cell.slew_rate));
    put(&mut item, "current_peak", q(&cell.current_peak));
    put(&mut item, "switch_nets", Value::Array(cell.switch_nets.iter().map(|n| Value::from(n.as_str())).collect()));
    put(&mut item, "hot_supply_net", cell.hot_supply_net.as_ref().map_or(Value::Null, |n| Value::from(n.as_str())));
    put(&mut item, "hot_return_net", cell.hot_return_net.as_ref().map_or(Value::Null, |n| Value::from(n.as_str())));
    put(&mut item, "magnetic", cell.magnetic.as_ref().map_or(Value::Null, |m| Value::from(m.component.as_str())));
    put(&mut item, "catch_diode", cell.catch_diode.as_ref().map_or(Value::Null, |d| Value::from(d.component.as_str())));
    put(
        &mut item,
        "hot_capacitors",
        Value::Array(cell.hot_capacitors.iter().map(|c| Value::from(c.component.as_str())).collect()),
    );
    let _ = cx;
    CellSummary { controller: cell.controller.clone(), heading, value: Value::Object(item) }
}

fn si_frequency(hertz: f64) -> String {
    if hertz >= 1e6 {
        format!("{} MHz", rounded(hertz / 1e6, 3))
    } else if hertz >= 1e3 {
        format!("{} kHz", rounded(hertz / 1e3, 3))
    } else {
        format!("{} Hz", rounded(hertz, 3))
    }
}

/// Screen the switching cells of `model` on the placed `board`. `view` is the
/// flat electrical view the model was derived from (it carries the pin to pad
/// numbers, supplies, the `emission` and `emi_accept` constraints and the part
/// facts); `model` is `copper_lang::emi::analyze(view)`.
pub fn emi_check(board: &PhysicalBoard, view: &FlatElectricalView, model: &EmiModel) -> EmiCheckReport {
    let standard = standard_of(view);
    let connectors = cable_connectors(view, &standard);
    let mut cx = Context {
        layout: Layout::new(board, view),
        stack: Stack::new(board),
        standard,
        model,
        connectors,
        findings: Vec::new(),
        coverage: Vec::new(),
        trace_loops: 0,
        trace_loops_traced: 0,
        trace_legs_traced: 0,
        trace_legs_unrouted: 0,
        trace_copper_seen: false,
        loops: BTreeMap::new(),
    };
    let mut cells = Vec::new();
    if model.cells.is_empty() {
        cx.cover(
            "EMI-CELLS".to_owned(),
            CoverageStatus::NotApplicable,
            "no switching cell: no component has a switch pin (declared emi_role or the structural fallback for unannotated regulators)",
        );
    }
    for cell in &model.cells {
        let drive = drive_of(view, cell);
        cells.push(cell_summary(&cx, cell, &drive));
        hot_loop(&mut cx, cell);
        conducted::conducted(&mut cx, cell);
        switch_node(&mut cx, cell);
        proximity(&mut cx, cell);
    }
    apply_accepts(view, &mut cx.findings);

    match (&cx.standard.name, &cx.standard.unsupported) {
        (None, _) => cx.cover(
            "EMI-MARGIN".to_owned(),
            CoverageStatus::NotApplicable,
            format!(
                "no emission constraint: findings are ranked against each other at {DEFAULT_DISTANCE} m and carry no margin"
            ),
        ),
        (Some(name), Some(reason)) => cx.cover("EMI-MARGIN".to_owned(), CoverageStatus::Unsupported, format!("{name}: {reason}")),
        (Some(name), None) => {
            let source = cx.standard.line.as_ref().map_or("", |line| line.source);
            cx.cover(
                "EMI-MARGIN".to_owned(),
                CoverageStatus::Executed,
                format!(
                    "margin against the {name} radiated line ({source}) at {} m, uncertainty {UNCERTAINTY_DB} dB, never a pass or fail; EMI-CONDUCTED margins (dBuV at the LISN, uncertainty 6 dB below 5 MHz and 10 dB above) use the CISPR 32 mains-port quasi-peak and average lines for cispr32_class_a and cispr32_class_b only",
                    cx.standard.distance
                ),
            );
        }
    }
    cx.cover(
        "EMI-RETURN-PATH".to_owned(),
        CoverageStatus::Unsupported,
        "needs filled copper (zone fills) and traced routed copper: reference-plane gaps, plane edges and layer changes without a return via are not examined",
    );
    cx.cover(
        "EMI-DECOUPLING-PATH".to_owned(),
        CoverageStatus::Unsupported,
        "needs routed copper: the copper path length from each decoupling capacitor pad to its target pin and return via is not traced",
    );
    if !model.cells.is_empty() {
        cx.cover(
            "EMI-FIELD:inductor".to_owned(),
            CoverageStatus::Unsupported,
            "the near-field map (`copper emi-check --plot-field`) covers the hot-loop current path only: the field of the power inductor itself, usually the largest low-frequency source for an unshielded inductor, is not included",
        );
    }
    if cx.trace_legs_traced > 0 {
        let detail = format!(
            "{} of {} copper leg(s) traced as shortest copper paths in {} of {} hot loop(s) ({} unrouted leg(s) kept straight); traced length, enclosed area, via count, layer changes, narrowest width and inductance per loop are in the EMI-HOT-LOOP findings; only the hot loops are traced (not the switch node, return paths or decoupling links)",
            cx.trace_legs_traced,
            cx.trace_legs_traced + cx.trace_legs_unrouted,
            cx.trace_loops_traced,
            cx.trace_loops,
            cx.trace_legs_unrouted
        );
        cx.cover("EMI-COPPER-TRACE".to_owned(), CoverageStatus::Executed, detail);
    } else {
        let reason = if cx.trace_copper_seen {
            "routed copper exists on the loop nets but no copper path joins a pad pair of a hot loop; loops use the pad-centre polygon"
        } else {
            "no routed copper (tracks, arcs, vias or polygons, macro copper included; zone outlines alone are not routed copper) on the nets of any hot loop: loops use the pad-centre polygon"
        };
        cx.cover("EMI-COPPER-TRACE".to_owned(), CoverageStatus::Unsupported, reason);
    }
    cx.cover(
        "EMI-CABLE-COMMON-MODE".to_owned(),
        CoverageStatus::Unsupported,
        format!("current-driven common mode on cables (cable_length {} m) is not estimated", cx.standard.cable_length),
    );

    let mut findings = std::mem::take(&mut cx.findings);
    findings.sort_by(|a, b| {
        // Radiated estimates (dBuV/m) by level, then conducted estimates (dBuV at a LISN, a different
        // quantity that is never compared with them) by margin when margins exist, else by level.
        let key = |f: &EmiFinding| {
            if f.finding.code == "EMI-ACCEPT-UNUSED" {
                (3, 0.0)
            } else {
                match f.estimate.as_ref() {
                    None => (2, 0.0),
                    Some(e) if e.unit == "dBuV" => (1, f.margin_db.unwrap_or(-e.value)),
                    Some(e) => (0, -e.value),
                }
            }
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(b.rank.total_cmp(&a.rank))
    });

    let mut notes = vec![
        "screening evidence: ranks likely noise sources, never establishes EMC qualification".to_owned(),
        format!("every absolute level carries about {UNCERTAINTY_DB} dB of uncertainty; margins are not pass or fail"),
        "hot-loop geometry is the traced copper path where routed copper joins the loop pads (traced_copper, partially_traced) and the pad-centre polygon elsewhere (placement_estimate)".to_owned(),
        "differential-mode loop field is free space; the radiated sweep is the upper envelope of the trapezoid, peak against quasi-peak limits".to_owned(),
        "limit tables are transcribed from the published numbers and must be verified against the standards".to_owned(),
    ];
    let on_rules = findings.iter().filter(|f| f.tags.iter().any(|t| defaults::is_rule_tag(t))).count();
    if on_rules > 0 {
        notes.push(format!(
            "{on_rules} of {} finding(s) rest on rule-of-thumb defaults (tags rule_of_thumb_edge, rule_of_thumb_capacitance, rule_of_thumb_dc_bias, rule_of_thumb_current):              their levels are bands with the nominal or the unfavourable end stated, the defaults are listed as `input:` lines and none is a datasheet value;              declare the attribute named in each line to replace a default, and the uncertainty of {UNCERTAINTY_DB} dB stays on top of every band",
            findings.len()
        ));
    }
    if !cx.stack.declared() {
        notes.push(format!("no stack-up is declared: copper layers are spread evenly over the board thickness, relative permittivity {DEFAULT_ER} assumed"));
    }
    let limits = Limits {
        uncertainty_db: UNCERTAINTY_DB,
        standard: cx.standard.name.clone(),
        limit_source: cx.standard.line.as_ref().map(|line| line.source.to_owned()),
        distance_m: cx.standard.distance,
        band_hz: BAND,
        notes,
    };
    EmiCheckReport { board: board.name.clone(), cells, findings, coverage: cx.coverage, limits }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn status_text(status: CoverageStatus) -> &'static str {
    match status {
        CoverageStatus::Executed => "executed",
        CoverageStatus::NotApplicable => "not_applicable",
        CoverageStatus::Unsupported => "unsupported",
        CoverageStatus::Failed => "failed",
    }
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

impl EmiFinding {
    /// The deterministic JSON form.
    pub fn to_value(&self) -> Value {
        let mut item = Object::new();
        put(&mut item, "code", Value::from(self.finding.code.as_str()));
        put(&mut item, "severity", Value::from(self.finding.severity.value()));
        put(&mut item, "message", Value::from(self.finding.message.as_str()));
        put(&mut item, "objects", strings(&self.finding.objects));
        put(&mut item, "nets", strings(&self.finding.nets));
        put(&mut item, "layers", strings(&self.finding.layers));
        put(&mut item, "required_nm", self.finding.required_nm.map_or(Value::Null, Value::from));
        put(&mut item, "measured_nm", self.finding.measured_nm.map_or(Value::Null, Value::from));
        put(&mut item, "cell", self.cell.as_ref().map_or(Value::Null, |c| Value::from(c.as_str())));
        put(&mut item, "provenance", self.provenance.map_or(Value::Null, |p| Value::from(p.value())));
        put(&mut item, "tags", strings(&self.tags));
        put(
            &mut item,
            "estimate",
            self.estimate.as_ref().map_or(Value::Null, |e| {
                let mut estimate = Object::new();
                put(&mut estimate, "quantity", Value::from(e.quantity.as_str()));
                put(&mut estimate, "value", Value::from(e.value));
                put(&mut estimate, "unit", Value::from(e.unit.as_str()));
                put(&mut estimate, "method", Value::from(e.method.as_str()));
                put(&mut estimate, "uncertainty_db", Value::from(e.uncertainty_db));
                put(&mut estimate, "frequency_hz", e.frequency_hz.map_or(Value::Null, Value::from));
                if let Some([low, high]) = e.band_db {
                    put(&mut estimate, "band_db", Value::Array(vec![Value::from(low), Value::from(high)]));
                }
                if let Some(basis) = &e.basis {
                    put(&mut estimate, "basis", Value::from(basis.as_str()));
                }
                Value::Object(estimate)
            }),
        );
        if let Some(margin) = self.margin_db {
            put(&mut item, "margin_db", Value::from(margin));
        }
        if let Some(reason) = &self.accepted {
            let mut accepted = Object::new();
            put(&mut accepted, "reason", Value::from(reason.as_str()));
            put(&mut item, "accepted", Value::Object(accepted));
        }
        put(&mut item, "measures", Value::Object(self.measures.clone()));
        Value::Object(item)
    }

    fn line(&self) -> String {
        let label = match self.finding.severity {
            Severity::Info => "ACCEPTED",
            _ => "FINDING",
        };
        let mut line = format!("{label} {}: {}", self.finding.code, self.finding.message);
        if !self.tags.is_empty() {
            line.push_str(&format!(" [{}]", self.tags.join(", ")));
        }
        if let Some(reason) = &self.accepted {
            line.push_str(&format!(" [accepted: {reason}]"));
        }
        line
    }
}

impl EmiCheckReport {
    /// Findings not acknowledged by `emi_accept`.
    pub fn open_findings(&self) -> usize {
        self.findings.iter().filter(|f| f.accepted.is_none()).count()
    }

    /// Open findings whose numbers rest on a rule-of-thumb default (a `rule_of_thumb_*` tag).
    pub fn rule_of_thumb_findings(&self) -> usize {
        self.findings.iter().filter(|f| f.accepted.is_none() && f.tags.iter().any(|t| defaults::is_rule_tag(t))).count()
    }

    pub fn accepted_findings(&self) -> usize {
        self.findings.iter().filter(|f| f.accepted.is_some()).count()
    }

    /// The document of `--json` (`copperscript-emi-check/v0.1`).
    pub fn to_value(&self) -> Value {
        let mut document = Object::new();
        put(&mut document, "schema", Value::from(SCHEMA));
        put(&mut document, "board", Value::from(self.board.as_str()));
        put(&mut document, "evidence_grade", Value::from("screening"));
        put(&mut document, "cells", Value::Array(self.cells.iter().map(|c| c.value.clone()).collect()));
        put(&mut document, "findings", Value::Array(self.findings.iter().map(EmiFinding::to_value).collect()));
        put(
            &mut document,
            "coverage",
            Value::Array(
                self.coverage
                    .iter()
                    .map(|entry| {
                        let mut item = Object::new();
                        put(&mut item, "check", Value::from(entry.check.as_str()));
                        put(&mut item, "status", Value::from(status_text(entry.status)));
                        put(&mut item, "detail", Value::from(entry.detail.as_str()));
                        Value::Object(item)
                    })
                    .collect(),
            ),
        );
        let mut limits = Object::new();
        put(&mut limits, "uncertainty_db", Value::from(self.limits.uncertainty_db));
        put(&mut limits, "standard", self.limits.standard.as_ref().map_or(Value::Null, |s| Value::from(s.as_str())));
        put(
            &mut limits,
            "limit_source",
            self.limits.limit_source.as_ref().map_or(Value::Null, |s| Value::from(s.as_str())),
        );
        put(&mut limits, "distance_m", Value::from(self.limits.distance_m));
        put(
            &mut limits,
            "band_hz",
            Value::Array(vec![Value::from(self.limits.band_hz.0), Value::from(self.limits.band_hz.1)]),
        );
        put(&mut limits, "notes", strings(&self.limits.notes));
        put(&mut document, "limits", Value::Object(limits));
        let mut summary = Object::new();
        put(&mut summary, "cells", Value::from(self.cells.len() as i64));
        put(&mut summary, "findings", Value::from(self.open_findings() as i64));
        put(&mut summary, "accepted", Value::from(self.accepted_findings() as i64));
        put(&mut summary, "rule_of_thumb", Value::from(self.rule_of_thumb_findings() as i64));
        put(
            &mut summary,
            "conducted",
            Value::from(
                self.findings.iter().filter(|f| f.finding.code == "EMI-CONDUCTED" && f.accepted.is_none()).count()
                    as i64,
            ),
        );
        put(&mut document, "summary", Value::Object(summary));
        Value::Object(document)
    }

    /// [`Self::to_value`] as indented text with sorted keys and a trailing newline.
    pub fn json(&self) -> String {
        Dumps::new().indent(2).sort_keys(true).dumps(&self.to_value()) + "\n"
    }

    /// The human-readable report, one line each.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "EMI screening for {}: screening evidence, not EMC qualification; absolute levels carry about {UNCERTAINTY_DB} dB of uncertainty",
            self.board
        )];
        match &self.limits.standard {
            Some(name) => lines.push(format!(
                "Standard {name}: levels at {} m, margin is limit minus level (positive below the line), never pass or fail",
                self.limits.distance_m
            )),
            None => lines.push(format!(
                "No emission constraint: findings are ranked against each other at {} m, no margins",
                self.limits.distance_m
            )),
        }
        let global: Vec<&EmiFinding> = self.findings.iter().filter(|f| f.cell.is_none()).collect();
        for cell in &self.cells {
            lines.push(String::new());
            lines.push(cell.heading.clone());
            let mine: Vec<&EmiFinding> =
                self.findings.iter().filter(|f| f.cell.as_deref() == Some(cell.controller.as_str())).collect();
            if mine.is_empty() {
                lines.push("  no findings".to_owned());
            }
            for (rank, finding) in mine.iter().enumerate() {
                lines.push(format!("  {}. {}", rank + 1, finding.line()));
                if let Some(estimate) = &finding.estimate {
                    lines.push(format!(
                        "     estimate: {} {} {} ({})",
                        estimate.quantity, estimate.value, estimate.unit, estimate.method
                    ));
                }
                if let Some(Value::Array(inputs)) = finding.measures.get("inputs_text") {
                    for input in inputs.iter().filter_map(Value::as_str) {
                        lines.push(format!("     input: {input}"));
                    }
                }
                // Added by `copper emi-check --fullwave` (the region full-wave model of the hot loop).
                if let Some(text) = finding.measures.get("fullwave_text").and_then(Value::as_str) {
                    lines.push(format!("     fullwave: {text}"));
                }
            }
        }
        if !global.is_empty() {
            lines.push(String::new());
        }
        for finding in global {
            lines.push(finding.line());
        }
        lines.push(String::new());
        lines.push("COVERAGE".to_owned());
        for entry in &self.coverage {
            lines.push(format!("  {} {}: {}", status_text(entry.status), entry.check, entry.detail));
        }
        lines.push(String::new());
        let conducted =
            self.findings.iter().filter(|f| f.finding.code == "EMI-CONDUCTED" && f.accepted.is_none()).count();
        let mut summary = format!(
            "EMI check: {} cell(s), {} finding(s), {} accepted (screening evidence; exit status does not reflect findings)",
            self.cells.len(),
            self.open_findings(),
            self.accepted_findings()
        );
        let on_rules = self.rule_of_thumb_findings();
        if on_rules > 0 {
            summary.push_str(&format!(
                "; {on_rules} finding(s) rest on rule-of-thumb defaults (their levels are bands, see the input: lines)"
            ));
        }
        if conducted > 0 {
            summary.push_str(&format!(
                "; {conducted} conducted estimate(s) are dBuV at a LISN, the other estimates are dBuV/m and not comparable with them"
            ));
        }
        lines.push(summary);
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(cx: f64, cy: f64, w: f64, h: f64) -> Shape {
        Shape {
            spine: vec![
                [cx - w / 2.0, cy - h / 2.0],
                [cx + w / 2.0, cy - h / 2.0],
                [cx + w / 2.0, cy + h / 2.0],
                [cx - w / 2.0, cy + h / 2.0],
            ],
            radius: 0.0,
        }
    }

    #[test]
    fn shoelace_area_and_length_of_a_hand_computed_loop() {
        // A 6 mm by 2 mm rectangle with a notch point on its long side:
        // area 12 mm2 (the added midpoint is collinear), perimeter 16 mm.
        let points = [[0.0, 0.0], [3e-3, 0.0], [6e-3, 0.0], [6e-3, 2e-3], [0.0, 2e-3]];
        assert!((polygon_area(&points) - 12e-6).abs() < 1e-12);
        assert!((closed_length(&points) - 16e-3).abs() < 1e-12);
        // A right triangle with legs 3 mm and 4 mm: area 6 mm2, perimeter 12 mm.
        let triangle = [[0.0, 0.0], [3e-3, 0.0], [3e-3, 4e-3]];
        assert!((polygon_area(&triangle) - 6e-6).abs() < 1e-12);
        assert!((closed_length(&triangle) - 12e-3).abs() < 1e-12);
        assert_eq!(polygon_area(&points[..2]), 0.0);
    }

    #[test]
    fn shape_gaps_account_for_radius_and_edges() {
        let a = rect(0.0, 0.0, 1e-3, 1e-3);
        let b = rect(3e-3, 0.0, 1e-3, 1e-3);
        assert!((shape_gap(&a, &b) - 2e-3).abs() < 1e-12);
        // Diagonal neighbours: corner to corner is sqrt(2) * 1 mm.
        let c = rect(2e-3, 2e-3, 1e-3, 1e-3);
        assert!((shape_gap(&a, &c) - 2f64.sqrt() * 1e-3).abs() < 1e-12);
        // A round pad of radius 0.5 mm centred 2 mm away leaves 1.5 mm to a point pad.
        let round = Shape { spine: vec![[2e-3, 0.0]], radius: 0.5e-3 };
        let point = Shape { spine: vec![[0.0, 0.0]], radius: 0.0 };
        assert!((shape_gap(&round, &point) - 1.5e-3).abs() < 1e-12);
        assert_eq!(shape_gap(&a, &a), 0.0);
        // Distance to the outline ring: 5 mm from the left edge of a 20 mm square.
        let ring = [[0.0, 0.0], [20e-3, 0.0], [20e-3, 20e-3], [0.0, 20e-3]];
        let inside = rect(5.5e-3, 10e-3, 1e-3, 1e-3);
        assert!((ring_gap(&inside, &ring) - 5e-3).abs() < 1e-12);
    }

    #[test]
    fn shape_area_matches_the_pad_it_describes() {
        let pad = rect(0.0, 0.0, 1.0e-3, 0.6e-3);
        assert!((pad.area() - 0.6e-6).abs() < 1e-12);
        assert!((pad.small_dimension() - 0.6e-3).abs() < 1e-12);
        let round = Shape { spine: vec![[0.0, 0.0]], radius: 0.5e-3 };
        assert!((round.area() - PI * 0.25e-6).abs() < 1e-12);
        let stadium = Shape::track([0.0, 0.0], [2e-3, 0.0], 1e-3);
        assert!((stadium.area() - (2e-3 * 1e-3 + PI * 0.25e-6)).abs() < 1e-12);
    }

    #[test]
    fn the_cheapest_cycle_picks_the_nearest_candidate_of_each_group() {
        let groups = vec![
            vec![[0.0, 0.0]],
            vec![[1e-3, 0.0], [30e-3, 0.0]],
            vec![[1e-3, 1e-3], [30e-3, 5e-3]],
            vec![[0.0, 1e-3]],
        ];
        assert_eq!(shortest_cycle(&groups), Some(vec![0, 0, 0, 0]));
        assert_eq!(shortest_cycle(&groups[..2]), None);
        assert_eq!(shortest_cycle(&[vec![], vec![[0.0, 0.0]], vec![[1.0, 0.0]]]), None);
    }

    #[test]
    fn a_larger_loop_has_more_inductance() {
        let loop_of = |w: f64| {
            let points = [[0.0, 0.0], [w, 0.0], [w, 3e-3], [0.0, 3e-3]];
            let candidate = LoopCandidate {
                capacitor: "C".to_owned(),
                points: points.to_vec(),
                widths: vec![0.5e-3; 4],
                area: polygon_area(&points),
                length: closed_length(&points),
                ..LoopCandidate::default()
            };
            inductance::loop_inductance(&segments_of(&candidate, 35e-6))
        };
        assert!(loop_of(10e-3) > loop_of(5e-3));
        assert!(loop_of(5e-3) > 0.0);
    }

    #[test]
    fn sweep_takes_the_least_margin_or_the_highest_level() {
        let line = limits::cispr32_radiated(limits::Class::B, limits::Distance::M10);
        let worst = sweep(30e6, 1e9, None, |f| 1e-6 * (f / 30e6)).expect("a point");
        assert!((worst.frequency - 1e9).abs() < 1.0);
        assert!(worst.margin_db.is_none());
        let flat = sweep(30e6, 1e9, None, |_| 1e-6).expect("a point");
        assert!((flat.frequency - 30e6).abs() < 1.0, "ties resolve to the lowest frequency");
        // A flat 20 dBuV/m level sits 10 dB below the 30 dBuV/m Class B line.
        let margin = sweep(30e6, 200e6, Some(&line), |_| 10e-6).expect("a point");
        assert!((margin.margin_db.expect("margin") - 10.0).abs() < 1e-9);
        assert!(sweep(1e9, 30e6, None, |_| 1.0).is_none());
    }

    #[test]
    fn accept_targets_match_components_and_pins() {
        let subjects = vec!["U1".to_owned(), "U1.SW".to_owned(), "L1".to_owned()];
        assert!(target_matches("U1", &subjects));
        assert!(target_matches("U1.SW", &subjects));
        assert!(target_matches("L1", &subjects));
        assert!(!target_matches("U1.VIN", &subjects));
        assert!(!target_matches("U", &subjects));
        assert!(!target_matches("L2", &subjects));
    }
}

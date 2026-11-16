//! `EMI-RETURN-PATH`: where the return current of a fast signal has no reference copper under its trace
//! (`docs/emi-signal-cable-plan.md` P2, `docs/emi-noise-analysis.md` section 15).
//!
//! The subjects are the nets with a signal source (`EmiModel::signals`) and the nets of a differential, clock or
//! RF-feed routing rule (the set `signal_integrity::needs_reference_plane` selects); a differential pair is one
//! subject. The inputs are the routed `PhysicalBoard` and the native zone fills of the reference nets, supplied by
//! the caller: `copper-drc` cannot fill zones itself (`copper_fullwave::fill` produces the fills and depends on this
//! crate), so [`ReturnPathInput`] carries them in. Per routed segment four things are measured and reported, none
//! of them as a pass or a fail:
//!
//! 1. **Reference coverage.** The segment widened by `k H` each side (`H` the height over the reference plane, `k`
//!    [`defaults::RETURN_WINDOW_H`]) is swept against the reference copper; the length with no reference copper
//!    under some part of that cross-section is the uncovered length.
//! 2. **Slot crossing.** The voids of the reference copper inside its bounding box that the widened segment meets are
//!    the gaps. A gap's slot length `L` is the extent of its connected void perpendicular to the crossing, and its
//!    half-wave resonance is `c / (2 L sqrt(eps_eff))`. A row of via antipads whose clearances merge is one void.
//! 3. **Reference change at a via.** The reference plane (layer and net) is compared before and after each via of a
//!    subject net; on a change the nearest return path is a stitching via (same net) or a decoupling capacitor
//!    (different nets).
//! 4. **Plane edge and board edge.** The distance from the trace to the reference copper's outer edge and to the
//!    board outline against `20 H`, and outer-layer copper along the board edge with no stitched ground via on the
//!    edge side.
//!
//! The finding is `EMI-RETURN-PATH` with the numbers in `measures` and, where a slot exists, a ranking score
//! (`20 log10(V(f_res) L / (1 V mm))`, tag `ranking_only`): not a field, no margin.

use std::f64::consts::SQRT_2;

use copper_cam::poly::{self, Contour, Shapes};
use copper_emi::signal as spectra;
use copper_lang::emi_signals::{SignalKind, SignalSource, polarity_of};

use super::signal::{self as signals, ResolvedSignal};
use super::*;
use crate::signal_integrity::{effective_permittivity, microstrip_effective_er, needs_reference_plane};
use crate::{return_span_covers, return_via_covered, single_ended_return_rule};

/// The coverage text of an unrequested check on a board that has no subject net; unchanged since before P2.
pub const NOT_REQUESTED_REASON: &str = "needs filled copper (zone fills) and traced routed copper: reference-plane gaps, plane edges and layer changes without a return via are not examined";

/// Resolution bandwidth of the radiated-emission receiver above 30 MHz (CISPR 16-1-1), Hz: the bandwidth in which
/// random data is evaluated.
pub(super) const RBW: f64 = 120e3;
/// Longest step of the samples along a segment for distances to edges, nm.
const SAMPLE_STEP_NM: f64 = 250_000.0;
/// Sagitta bound of a flattened arc, nm.
const ARC_TOLERANCE_NM: i64 = 5_000;
/// Tolerance of "a void reaches the window's edge", nm.
const TOUCH_NM: i64 = 1_000;
/// Uncovered lengths below this are rounding of the polygon operations, nm.
const NEGLIGIBLE_NM: f64 = 1_000.0;
/// A void holding one via of the subject is that via's own antipad when it is at most this many via diameters long.
const OWN_ANTIPAD_DIAMETERS: f64 = 3.0;

/// What the score and the finding leave out.
const NOT_MODELLED: [&str; 7] = [
    "whether a cut that reaches the plane edge is a notch open at one end, which resonates at a quarter wave (half the frequency the score uses; carried as quarter_wave_resonance_hz), or a cut through the plane",
    "slot radiation efficiency (how much of the power a slot of this length takes up and radiates depends on how it is fed, its width and the structure around it)",
    "plane-pair cavity modes (a pair of planes resonates as a cavity; a slot, a cut or a row of vias only perturbs it)",
    "enclosure, chassis and cable coupling (the field outside the board, and what the slot's voltage drives)",
    "the common-mode part of a differential pair: the score uses the whole signal, a balanced pair excites a slot only through the part that is converted (mode conversion is a later check)",
    "return current outside the coverage window, current crowding at a slot's ends and the inductance of the detour",
    "the plane of a stripline other than the nearer one, and a reference net that changes inside one plane layer",
];

/// What the score is.
const SCORE_DEFINITION: &str = "20 log10(V(f_res) L / (1 V * 1 mm)): f_res the half-wave resonance c / (2 L sqrt(eps_eff)) of the gap's slot, L the slot length perpendicular to the crossing in mm, V the continuous-bound amplitude (volts, single-sided peak) of the signal's spectrum at f_res for its swing and its shorter edge. The headline uses the continuous bound because it is the one of the three spectrum quantities that needs no assumption about the data pattern; the named pattern lines and random data in a resolution bandwidth are carried beside it, each with its own score. Not a field, no distance, no limit: it ranks the slots of one board against each other";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// The filled copper of one zone on one layer, integer nanometres.
#[derive(Clone, Debug)]
pub struct PlaneFill {
    pub net: String,
    pub layer: CopperLayer,
    pub shapes: Shapes,
}

/// The native zone fills of a board (`copper_fullwave::fill::ZoneFillSet::plane_fills`).
#[derive(Clone, Debug, Default)]
pub struct PlaneFills {
    pub fills: Vec<PlaneFill>,
}

/// What the caller supplies for the checks that need the routed board (the return-path and the mode-conversion check).
#[derive(Clone, Debug)]
pub enum ReturnPathInput<'a> {
    /// No routed board and fills were asked for (the default run): the check stays `unsupported`.
    NotRequested,
    /// Routed copper and the native fills of its zones.
    Fills(&'a PlaneFills),
    /// Routed copper was given but its fills could not be made; the reason.
    Unavailable(String),
}

impl ReturnPathInput<'_> {
    /// Whether the routed board was supplied (with or without usable fills): the checks that need routed copper
    /// (`EMI-RETURN-PATH`, `EMI-MODE-CONVERSION`) run only then.
    pub fn routed(&self) -> bool {
        !matches!(self, ReturnPathInput::NotRequested)
    }
}
// ---------------------------------------------------------------------------
// Subjects
// ---------------------------------------------------------------------------

pub(super) struct Subject<'a> {
    /// The first member (the P net of a pair).
    pub(super) name: String,
    /// P net first.
    pub(super) nets: Vec<String>,
    pub(super) source: Option<&'a SignalSource>,
}

pub(super) fn subjects<'a>(board: &PhysicalBoard, model: &'a EmiModel) -> Vec<Subject<'a>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut add = |members: Vec<String>| {
        let found: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter(|(_, group)| members.iter().any(|m| group.contains(m)))
            .map(|(index, _)| index)
            .collect();
        match found.split_first() {
            None => groups.push(members),
            Some((first, rest)) => {
                for index in rest.iter().rev() {
                    let merged = groups.remove(*index);
                    groups[*first].extend(merged);
                }
                groups[*first].extend(members);
            }
        }
    };
    for (key, source) in &model.signals {
        let mut members = vec![key.clone()];
        members.extend(source.partner.iter().cloned());
        add(members);
    }
    for rule in board.net_routing_rules.iter().filter(|rule| needs_reference_plane(rule.kind)) {
        let mut members = vec![rule.net.clone()];
        members.extend(rule.differential_partner.iter().cloned());
        add(members);
    }
    let mut result: Vec<Subject<'a>> = groups
        .into_iter()
        .map(|mut nets| {
            let mut seen = BTreeSet::new();
            nets.retain(|net| seen.insert(net.clone()));
            if nets.len() == 2 {
                let positive_first = polarity_of(&nets[0], &nets[1]);
                if positive_first == Some(false) || (positive_first.is_none() && nets[0] > nets[1]) {
                    nets.swap(0, 1);
                }
            } else {
                nets.sort();
            }
            let source = nets.iter().find_map(|net| model.signals.get(net));
            Subject { name: nets[0].clone(), nets, source }
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Whether the board has a net the return-path check examines (a net with a signal source, or one of a
/// differential, clock or RF-feed routing rule).
pub fn has_subjects(board: &PhysicalBoard, model: &EmiModel) -> bool {
    !subjects(board, model).is_empty()
}

/// A source with nothing declared, for a subject that only has a routing rule: every number comes from the defaults.
pub(super) fn bare_source(subject: &Subject) -> SignalSource {
    SignalSource {
        net: subject.name.clone(),
        kind: if subject.nets.len() == 2 { SignalKind::Differential } else { SignalKind::SingleEnded },
        partner: subject.nets.get(1).cloned(),
        pins: Vec::new(),
        fast_edge: false,
        data_rate: None,
        clock_frequency: None,
        rise_time: None,
        fall_time: None,
        swing: None,
        spread_spectrum: None,
        open_circuit_swing: None,
        output_impedance: None,
        members: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Reference planes
// ---------------------------------------------------------------------------

type Bounds = [i64; 4];

fn bounds_of(shapes: &Shapes) -> Option<Bounds> {
    let mut result: Option<Bounds> = None;
    for shape in shapes {
        let (x0, y0, x1, y1) = poly::bounds(shape);
        result = Some(match result {
            None => [x0, y0, x1, y1],
            Some(b) => [b[0].min(x0), b[1].min(y0), b[2].max(x1), b[3].max(y1)],
        });
    }
    result
}

fn overlap(a: Bounds, b: Bounds) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn rectangle(b: Bounds) -> Shapes {
    poly::polygon(&[
        (b[0] as f64, b[1] as f64),
        (b[2] as f64, b[1] as f64),
        (b[2] as f64, b[3] as f64),
        (b[0] as f64, b[3] as f64),
    ])
}

/// A connected void of a reference layer: a piece of its bounding box with no copper.
struct Void {
    shape: Vec<Contour>,
    bounds: Bounds,
    /// The void reaches the bounding box of the plane (a cut, or a notch from the plane's edge).
    reaches_edge: bool,
    /// Vias of the board that pass through the layer inside the void.
    vias: usize,
}

/// The reference copper of one layer.
struct PlaneLayer {
    layer: CopperLayer,
    /// Fills by reference net, nets in name order.
    by_net: Vec<(String, Shapes)>,
    union: Shapes,
    voids: Vec<Void>,
    ground: bool,
}

fn layer_position(board: &PhysicalBoard, layer: CopperLayer) -> usize {
    board.stackup.copper_layers.iter().position(|item| *item == layer).unwrap_or(usize::MAX)
}

fn build_planes(board: &PhysicalBoard, view: &FlatElectricalView, fills: &PlaneFills) -> Vec<PlaneLayer> {
    let references: BTreeSet<&str> = view.supplies.iter().map(|supply| supply.net.as_str()).collect();
    let grounds: BTreeSet<&str> =
        view.supplies.iter().filter(|supply| supply.voltage.base_value().is_zero()).map(|s| s.net.as_str()).collect();
    let mut layers: BTreeMap<usize, (CopperLayer, BTreeMap<String, Shapes>)> = BTreeMap::new();
    for fill in &fills.fills {
        if !references.contains(fill.net.as_str()) || fill.shapes.is_empty() {
            continue;
        }
        layers
            .entry(layer_position(board, fill.layer))
            .or_insert_with(|| (fill.layer, BTreeMap::new()))
            .1
            .entry(fill.net.clone())
            .or_default()
            .extend(fill.shapes.iter().cloned());
    }
    let mut planes = Vec::new();
    for (position, (layer, nets)) in layers {
        let all: Shapes = nets.values().flat_map(|shapes| shapes.iter().cloned()).collect();
        let union = poly::union(&all);
        let Some(window) = bounds_of(&union) else { continue };
        let voids = poly::difference(&rectangle(window), &union)
            .into_iter()
            .map(|shape| {
                let (x0, y0, x1, y1) = poly::bounds(&shape);
                let outer = shape.first().cloned().unwrap_or_default();
                let vias = board
                    .vias
                    .iter()
                    .filter(|via| {
                        let (from, to) = (layer_position(board, via.from_layer), layer_position(board, via.to_layer));
                        from.min(to) <= position
                            && position <= from.max(to)
                            && poly::contour_contains(&outer, (via.position.x_nm as f64, via.position.y_nm as f64))
                    })
                    .count();
                Void {
                    reaches_edge: x0 - window[0] <= TOUCH_NM
                        || y0 - window[1] <= TOUCH_NM
                        || window[2] - x1 <= TOUCH_NM
                        || window[3] - y1 <= TOUCH_NM,
                    bounds: [x0, y0, x1, y1],
                    shape,
                    vias,
                }
            })
            .collect();
        let ground = nets.keys().any(|net| grounds.contains(net.as_str()));
        planes.push(PlaneLayer { layer, by_net: nets.into_iter().collect(), union, voids, ground });
    }
    planes
}

/// The reference plane chosen for a signal layer.
#[derive(Clone, Copy)]
struct Choice {
    plane: usize,
    /// Centre-to-centre height over the plane, m (the convention of `Stack::height`).
    height: f64,
    /// The nearest plane on the other side of the signal layer, when there is one: not evaluated.
    other: Option<(CopperLayer, f64)>,
}

/// The nearest plane layer above or below `layer` (the smaller height; at equal height a plane of a ground net, then
/// the one higher in the stack-up), and the nearest on the other side.
fn choose(stack: &Stack, planes: &[PlaneLayer], layer: CopperLayer) -> Option<Choice> {
    let board = stack.board;
    let own = stack.depth(layer)?;
    let mut candidates: Vec<(usize, f64, bool)> = planes
        .iter()
        .enumerate()
        .filter(|(_, plane)| plane.layer != layer)
        .filter_map(|(index, plane)| {
            let height = stack.height(layer, plane.layer)?;
            (height > 0.0).then_some((index, height, stack.depth(plane.layer)? < own))
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.1.total_cmp(&b.1)
            .then(planes[b.0].ground.cmp(&planes[a.0].ground))
            .then(layer_position(board, planes[a.0].layer).cmp(&layer_position(board, planes[b.0].layer)))
    });
    let first = *candidates.first()?;
    let other = candidates
        .iter()
        .find(|candidate| candidate.2 != first.2)
        .map(|candidate| (planes[candidate.0].layer, candidate.1));
    Some(Choice { plane: first.0, height: first.1, other })
}

// ---------------------------------------------------------------------------
// Geometry (nanometres, f64)
// ---------------------------------------------------------------------------

/// One straight piece of routed copper.
#[derive(Clone, Copy)]
pub(super) struct Seg {
    member: usize,
    pub(super) layer: CopperLayer,
    a: [f64; 2],
    b: [f64; 2],
    pub(super) width: f64,
}

impl Seg {
    pub(super) fn length(&self) -> f64 {
        dist(self.a, self.b)
    }

    /// The two ends, nm.
    pub(super) fn ends(&self) -> ([f64; 2], [f64; 2]) {
        (self.a, self.b)
    }

    fn unit(&self) -> [f64; 2] {
        let length = self.length().max(1e-9);
        [(self.b[0] - self.a[0]) / length, (self.b[1] - self.a[1]) / length]
    }

    fn at(&self, t: f64) -> [f64; 2] {
        [self.a[0] + t * (self.b[0] - self.a[0]), self.a[1] + t * (self.b[1] - self.a[1])]
    }

    /// The segment's widened footprint: `half` each side of the centre line, flush at the ends.
    fn strip(&self, half: f64) -> Shapes {
        let d = self.unit();
        let n = [-d[1] * half, d[0] * half];
        poly::polygon(&[
            (self.a[0] + n[0], self.a[1] + n[1]),
            (self.b[0] + n[0], self.b[1] + n[1]),
            (self.b[0] - n[0], self.b[1] - n[1]),
            (self.a[0] - n[0], self.a[1] - n[1]),
        ])
    }

    /// Where along the centre line the shapes extend: one interval of stations (nm from `a`) per shape, clamped to the
    /// segment.
    fn stations(&self, shapes: &Shapes) -> Vec<(f64, f64)> {
        let (d, length) = (self.unit(), self.length());
        shapes
            .iter()
            .filter_map(|shape| {
                let outer = shape.first()?;
                let (lo, hi) = outer.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |acc, p| {
                    let t = ((p.x as f64 - self.a[0]) * d[0] + (p.y as f64 - self.a[1]) * d[1]).clamp(0.0, length);
                    (acc.0.min(t), acc.1.max(t))
                });
                (hi > lo).then_some((lo, hi))
            })
            .collect()
    }
}

/// The length covered by a set of intervals.
fn union_length(mut intervals: Vec<(f64, f64)>) -> f64 {
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut total, mut end) = (0.0, f64::NEG_INFINITY);
    for (lo, hi) in intervals {
        let start = lo.max(end);
        if hi > start {
            total += hi - start;
        }
        end = end.max(hi);
    }
    total
}

fn ring_distance(ring: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| point_segment(p, ring[i], ring[(i + 1) % n])).fold(f64::INFINITY, f64::min)
}

fn contour_points(contour: &Contour) -> Vec<[f64; 2]> {
    contour.iter().map(|q| [q.x as f64, q.y as f64]).collect()
}

/// Distance from `p` to the nearest edge of the outer contours of `shapes`, nm.
fn outer_edge_distance(shapes: &Shapes, p: [f64; 2]) -> f64 {
    shapes
        .iter()
        .filter_map(|shape| shape.first())
        .map(|outer| ring_distance(&contour_points(outer), p))
        .fold(f64::INFINITY, f64::min)
}

/// Distance from `p` to the copper of `shapes` (0 inside), nm.
fn shapes_distance(shapes: &Shapes, p: [f64; 2]) -> f64 {
    if poly::shapes_contain(shapes, (p[0], p[1])) {
        return 0.0;
    }
    shapes
        .iter()
        .flat_map(|shape| shape.iter())
        .map(|contour| ring_distance(&contour_points(contour), p))
        .fold(f64::INFINITY, f64::min)
}

fn nmm(nm: f64) -> f64 {
    nm * 1e-6
}

fn mm_point(p: [f64; 2]) -> Value {
    Value::Array(vec![num(nmm(p[0]), 4), num(nmm(p[1]), 4)])
}

fn names(layers: impl IntoIterator<Item = CopperLayer>) -> Vec<String> {
    let set: BTreeSet<CopperLayer> = layers.into_iter().collect();
    set.into_iter().map(|layer| layer.value().to_owned()).collect()
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut item = Object::new();
    for (key, value) in entries {
        put(&mut item, key, value);
    }
    Value::Object(item)
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A void of a reference plane crossed by a member's copper.
struct Gap {
    signal_layer: CopperLayer,
    plane_layer: CopperLayer,
    reaches_edge: bool,
    vias: usize,
    /// Per member that crosses the void: station length of its crossings summed over its segments, nm, and the
    /// number of segments.
    crossings: Vec<(usize, f64, usize)>,
    position: [f64; 2],
    /// Slot length perpendicular to the first crossing, nm.
    slot: f64,
    eps_eff: f64,
    structure: &'static str,
}

impl Gap {
    fn resonance(&self) -> f64 {
        copper_emi::C0 / (2.0 * self.slot * 1e-9 * self.eps_eff.sqrt())
    }
}

type Reference = Option<(CopperLayer, String)>;

struct Change {
    member: usize,
    via: usize,
    position: [f64; 2],
    layers: [CopperLayer; 2],
    before: Reference,
    after: Reference,
    kind: &'static str,
    element: Option<String>,
    /// Distance from the via to the return path, nm.
    distance: Option<f64>,
    threshold: f64,
    /// Whether the threshold is the net's declared `maximum_return_via_distance`.
    declared: bool,
    /// `Some(satisfied)` when the net declares the DRC rule (`require_return_vias`).
    drc: Option<bool>,
}

impl Change {
    fn within(&self) -> bool {
        self.distance.is_some_and(|d| d <= self.threshold)
    }
}

#[derive(Default)]
struct EdgeStats {
    /// Smallest distance, nm, and where.
    nearest: Option<(f64, [f64; 2])>,
    /// Length within the limit, nm.
    within: f64,
}

impl EdgeStats {
    fn note(&mut self, distance: f64, at: [f64; 2]) {
        if self.nearest.is_none_or(|(d, _)| distance < d) {
            self.nearest = Some((distance, at));
        }
    }
}

struct LayerStats {
    layer: CopperLayer,
    routed: f64,
    uncovered: f64,
    segments: usize,
    plane_edge: EdgeStats,
    board_edge: EdgeStats,
    /// Outer-layer copper within the limit of the board edge with no stitched ground via on the edge side, nm.
    unguarded: f64,
    limit: f64,
    outer: bool,
}

struct ReferenceUsed {
    signal_layer: CopperLayer,
    plane_layer: CopperLayer,
    nets: Vec<String>,
    height: f64,
    structure: &'static str,
    eps_eff: f64,
    other: Option<(CopperLayer, f64)>,
}

/// Everything measured for one member net.
struct MemberResult {
    net: String,
    routed: f64,
    uncovered: f64,
    layers: Vec<LayerStats>,
    /// Layers with copper of this net and no reference plane at all.
    no_reference: Vec<CopperLayer>,
}

struct Outcome {
    members: Vec<MemberResult>,
    references: Vec<ReferenceUsed>,
    gaps: Vec<Gap>,
    changes: Vec<Change>,
    /// Layer transitions of the subject's vias that were examined.
    transitions: usize,
}

impl Outcome {
    fn routed(&self) -> f64 {
        self.members.iter().map(|m| m.routed).sum()
    }

    fn uncovered(&self) -> f64 {
        self.members.iter().map(|m| m.uncovered).sum()
    }

    fn unreturned(&self) -> Vec<&Change> {
        self.changes.iter().filter(|c| !c.within()).collect()
    }

    fn edge_flagged(&self) -> bool {
        self.members.iter().flat_map(|m| &m.layers).any(|l| {
            l.plane_edge.nearest.is_some_and(|(d, _)| d < l.limit)
                || l.board_edge.nearest.is_some_and(|(d, _)| d < l.limit)
        })
    }

    fn has_findings(&self) -> bool {
        self.uncovered() > NEGLIGIBLE_NM
            || !self.gaps.is_empty()
            || !self.changes.is_empty() && !self.unreturned().is_empty()
            || self.edge_flagged()
            || self.members.iter().any(|m| !m.no_reference.is_empty())
    }
}

// ---------------------------------------------------------------------------
// The measurement
// ---------------------------------------------------------------------------

pub(super) fn flatten(board: &PhysicalBoard, net: &str, member: usize) -> Vec<Seg> {
    let mut result = Vec::new();
    for track in board.tracks.iter().filter(|t| t.net == net) {
        result.push(Seg {
            member,
            layer: track.layer,
            a: [track.start.x_nm as f64, track.start.y_nm as f64],
            b: [track.end.x_nm as f64, track.end.y_nm as f64],
            width: track.width_nm as f64,
        });
    }
    for arc in board.arcs.iter().filter(|a| a.net == net) {
        let Ok(geometry) = arc.geometry() else { continue };
        for pair in geometry.chords(arc.start, arc.end, ARC_TOLERANCE_NM).windows(2) {
            result.push(Seg {
                member,
                layer: arc.layer,
                a: [pair[0].x_nm as f64, pair[0].y_nm as f64],
                b: [pair[1].x_nm as f64, pair[1].y_nm as f64],
                width: arc.width_nm as f64,
            });
        }
    }
    result.retain(|seg| seg.length() > 0.0);
    result
}

/// The reference planes and the board data the measurement needs.
struct Env<'a> {
    board: &'a PhysicalBoard,
    view: &'a FlatElectricalView,
    stack: &'a Stack<'a>,
    layout: &'a Layout<'a>,
    planes: Vec<PlaneLayer>,
    /// Names of the reference nets (the declared supplies and grounds).
    references: BTreeSet<String>,
    ring: Vec<[f64; 2]>,
}

impl Env<'_> {
    fn choice(&self, layer: CopperLayer) -> Option<Choice> {
        choose(self.stack, &self.planes, layer)
    }

    /// Structure and effective permittivity of a trace on `layer` over a plane at `height` m.
    fn permittivity(&self, layer: CopperLayer, width_nm: i64, plane: CopperLayer, height: f64) -> (&'static str, f64) {
        if let Some(found) = effective_permittivity(&self.board.stackup, layer, width_nm) {
            return found;
        }
        // No declared stack-up: the assumed permittivity over the spacing the stack-up fell back to.
        let er = self.stack.er_between(layer, plane);
        let layers = &self.board.stackup.copper_layers;
        if layers.first() == Some(&layer) || layers.last() == Some(&layer) {
            let thickness = (self.stack.copper_thickness(layer) * 1e9).round() as i64;
            let effective = microstrip_effective_er(width_nm, thickness.max(1), (height * 1e9).round() as i64, er)
                .unwrap_or((er + 1.0) / 2.0);
            ("microstrip", effective)
        } else {
            ("stripline", er)
        }
    }

    /// The reference plane layer and net under the point `p` (nm) of a trace on `layer`.
    fn reference_at(&self, layer: CopperLayer, p: [f64; 2]) -> Reference {
        let choice = self.choice(layer)?;
        let plane = &self.planes[choice.plane];
        let net = plane
            .by_net
            .iter()
            .map(|(net, shapes)| (shapes_distance(shapes, p), net))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)))
            .map(|(_, net)| net.clone())?;
        Some((plane.layer, net))
    }
}

fn examine(env: &Env, subject: &Subject, resolved: &ResolvedSignal) -> Option<Outcome> {
    let board = env.board;
    let mut members: Vec<MemberResult> = Vec::new();
    let mut segments: Vec<Seg> = Vec::new();
    for (index, net) in subject.nets.iter().enumerate() {
        segments.extend(flatten(board, net, index));
        members.push(MemberResult {
            net: net.clone(),
            routed: 0.0,
            uncovered: 0.0,
            layers: Vec::new(),
            no_reference: Vec::new(),
        });
    }
    if segments.is_empty() && !subject.nets.iter().any(|net| board.vias.iter().any(|via| via.net == *net)) {
        return None;
    }
    let outer_layers = [board.stackup.copper_layers.first().copied(), board.stackup.copper_layers.last().copied()];
    // The antipad of the subject's own via is how a layer transition looks in a plane: it is not a gap but the
    // transition itself, which the reference-change check examines. A void is such an antipad when it holds exactly one
    // via, a via of the subject, and is no longer than `OWN_ANTIPAD_DIAMETERS` via diameters.
    let mut own: BTreeSet<(usize, usize)> = BTreeSet::new();
    for via in board.vias.iter().filter(|via| subject.nets.contains(&via.net)) {
        let (from, to) = (layer_position(board, via.from_layer), layer_position(board, via.to_layer));
        for (plane_index, plane) in env.planes.iter().enumerate() {
            let position = layer_position(board, plane.layer);
            if position < from.min(to) || position > from.max(to) {
                continue;
            }
            for (void_index, void) in plane.voids.iter().enumerate() {
                let extent = (void.bounds[2] - void.bounds[0]).max(void.bounds[3] - void.bounds[1]) as f64;
                let inside = void.shape.first().is_some_and(|outer| {
                    poly::contour_contains(outer, (via.position.x_nm as f64, via.position.y_nm as f64))
                });
                if inside && void.vias == 1 && extent <= OWN_ANTIPAD_DIAMETERS * via.size_nm as f64 {
                    own.insert((plane_index, void_index));
                }
            }
        }
    }
    // The reference copper with those antipads closed, per plane.
    let effective: Vec<Shapes> = env
        .planes
        .iter()
        .enumerate()
        .map(|(plane_index, plane)| {
            let mut all = plane.union.clone();
            all.extend(own.iter().filter(|(p, _)| *p == plane_index).map(|(_, v)| plane.voids[*v].shape.clone()));
            if all.len() == plane.union.len() { all } else { poly::union(&all) }
        })
        .collect();
    let mut gaps: BTreeMap<(usize, usize), Gap> = BTreeMap::new();
    let mut references: BTreeMap<usize, ReferenceUsed> = BTreeMap::new();
    for seg in &segments {
        let length = seg.length();
        let member = &mut members[seg.member];
        let index = match member.layers.iter().position(|l| l.layer == seg.layer) {
            Some(index) => index,
            None => {
                member.layers.push(LayerStats {
                    layer: seg.layer,
                    routed: 0.0,
                    uncovered: 0.0,
                    segments: 0,
                    plane_edge: EdgeStats::default(),
                    board_edge: EdgeStats::default(),
                    unguarded: 0.0,
                    limit: 0.0,
                    outer: outer_layers.contains(&Some(seg.layer)),
                });
                member.layers.len() - 1
            }
        };
        member.routed += length;
        member.layers[index].routed += length;
        member.layers[index].segments += 1;
        let Some(choice) = env.choice(seg.layer) else {
            member.uncovered += length;
            member.layers[index].uncovered += length;
            if !member.no_reference.contains(&seg.layer) {
                member.no_reference.push(seg.layer);
            }
            continue;
        };
        let plane = &env.planes[choice.plane];
        let height_nm = choice.height * 1e9;
        let stats = &mut member.layers[index];
        stats.limit = defaults::EDGE_DISTANCE_H.0 * height_nm;
        let (structure, eps_eff) = env.permittivity(seg.layer, seg.width as i64, plane.layer, choice.height);
        references.entry(layer_position(board, seg.layer)).or_insert_with(|| ReferenceUsed {
            signal_layer: seg.layer,
            plane_layer: plane.layer,
            nets: plane.by_net.iter().map(|(net, _)| net.clone()).collect(),
            height: choice.height,
            structure,
            eps_eff,
            other: choice.other,
        });
        // 1. Coverage: the widened footprint against the reference copper.
        let strip = seg.strip(seg.width / 2.0 + defaults::RETURN_WINDOW_H.0 * height_nm);
        let uncovered = union_length(seg.stations(&poly::difference(&strip, &effective[choice.plane])));
        let uncovered = if uncovered < NEGLIGIBLE_NM { 0.0 } else { uncovered };
        stats.uncovered += uncovered;
        member.uncovered += uncovered;
        // 2. Slot crossings.
        if let Some(strip_bounds) = bounds_of(&strip) {
            for (void_index, void) in plane.voids.iter().enumerate() {
                if !overlap(void.bounds, strip_bounds) || own.contains(&(choice.plane, void_index)) {
                    continue;
                }
                let cut = poly::intersection(&strip, &vec![void.shape.clone()]);
                let crossing = union_length(seg.stations(&cut));
                if crossing < NEGLIGIBLE_NM {
                    continue;
                }
                let at = bounds_of(&cut).map_or(seg.a, |b| [(b[0] + b[2]) as f64 / 2.0, (b[1] + b[3]) as f64 / 2.0]);
                let gap = gaps.entry((choice.plane, void_index)).or_insert_with(|| {
                    let d = seg.unit();
                    let normal = [-d[1], d[0]];
                    let (lo, hi) = void.shape.first().map_or((0.0, 0.0), |outer| {
                        outer.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |acc, p| {
                            let t = p.x as f64 * normal[0] + p.y as f64 * normal[1];
                            (acc.0.min(t), acc.1.max(t))
                        })
                    });
                    Gap {
                        signal_layer: seg.layer,
                        plane_layer: plane.layer,
                        reaches_edge: void.reaches_edge,
                        vias: void.vias,
                        crossings: Vec::new(),
                        position: at,
                        slot: hi - lo,
                        eps_eff,
                        structure,
                    }
                });
                match gap.crossings.iter_mut().find(|c| c.0 == seg.member) {
                    Some(entry) => {
                        entry.1 += crossing;
                        entry.2 += 1;
                    }
                    None => gap.crossings.push((seg.member, crossing, 1)),
                }
            }
        }
        // 4. Plane edge and board edge, sampled along the segment.
        let count = (length / SAMPLE_STEP_NM).ceil().max(1.0) as usize;
        let step = length / count as f64;
        let limit = stats.limit;
        let guards: Vec<[f64; 2]> = if stats.outer {
            board
                .vias
                .iter()
                .filter(|via| {
                    env.references.contains(&via.net) && return_span_covers(board, via, seg.layer, plane.layer)
                })
                .map(|via| [via.position.x_nm as f64, via.position.y_nm as f64])
                .collect()
        } else {
            Vec::new()
        };
        for i in 0..=count {
            let p = seg.at(i as f64 / count as f64);
            if !env.ring.is_empty() {
                stats.board_edge.note(ring_distance(&env.ring, p), p);
            }
            if poly::shapes_contain(&plane.union, (p[0], p[1])) {
                stats.plane_edge.note(outer_edge_distance(&plane.union, p), p);
            }
        }
        for i in 0..count {
            let p = seg.at((i as f64 + 0.5) / count as f64);
            if poly::shapes_contain(&plane.union, (p[0], p[1])) && outer_edge_distance(&plane.union, p) < limit {
                stats.plane_edge.within += step;
            }
            if env.ring.is_empty() {
                continue;
            }
            let to_edge = ring_distance(&env.ring, p);
            if to_edge < limit {
                stats.board_edge.within += step;
                if stats.outer {
                    let guarded = guards.iter().any(|g| dist(*g, p) <= limit && ring_distance(&env.ring, *g) < to_edge);
                    if !guarded {
                        stats.unguarded += step;
                    }
                }
            }
        }
    }
    for member in &mut members {
        member.layers.sort_by_key(|l| layer_position(board, l.layer));
        member.no_reference.sort();
    }
    let mut gaps: Vec<Gap> = gaps.into_values().collect();
    gaps.sort_by(|a, b| {
        layer_position(board, a.signal_layer)
            .cmp(&layer_position(board, b.signal_layer))
            .then(a.position[0].total_cmp(&b.position[0]))
            .then(a.position[1].total_cmp(&b.position[1]))
    });
    let (changes, transitions) = via_changes(env, subject, resolved, &segments);
    Some(Outcome { members, references: references.into_values().collect(), gaps, changes, transitions })
}

/// 3. The reference plane before and after each layer transition of the subject's vias.
fn via_changes(env: &Env, subject: &Subject, resolved: &ResolvedSignal, segments: &[Seg]) -> (Vec<Change>, usize) {
    let board = env.board;
    let mut changes = Vec::new();
    let mut transitions = 0;
    let knee_edge = resolved.rise_time.value.min(resolved.fall_time.value);
    let declared = board
        .net_routing_rules
        .iter()
        .rev()
        .filter(|rule| subject.nets.contains(&rule.net))
        .find_map(|rule| rule.maximum_return_via_distance_nm);
    for (member, net) in subject.nets.iter().enumerate() {
        for (index, via) in board.vias.iter().enumerate().filter(|(_, via)| via.net == *net) {
            let centre = [via.position.x_nm as f64, via.position.y_nm as f64];
            let (from, to) = (layer_position(board, via.from_layer), layer_position(board, via.to_layer));
            let reach = via.size_nm as f64 / 2.0;
            let mut layers: Vec<CopperLayer> = Vec::new();
            for seg in segments.iter().filter(|s| s.member == member) {
                let position = layer_position(board, seg.layer);
                if position < from.min(to) || position > from.max(to) || layers.contains(&seg.layer) {
                    continue;
                }
                if dist(seg.a, centre).min(dist(seg.b, centre)) <= reach {
                    layers.push(seg.layer);
                }
            }
            layers.sort_by_key(|layer| layer_position(board, *layer));
            for pair in layers.windows(2) {
                transitions += 1;
                let (before, after) = (env.reference_at(pair[0], centre), env.reference_at(pair[1], centre));
                if before == after {
                    continue;
                }
                let width = |layer: CopperLayer| {
                    segments
                        .iter()
                        .find(|s| s.member == member && s.layer == layer)
                        .map_or(board.rules.default_track_width_nm, |s| s.width as i64)
                };
                let eps_of = |layer: CopperLayer| {
                    env.choice(layer).map_or(1.0, |choice| {
                        env.permittivity(layer, width(layer), env.planes[choice.plane].layer, choice.height).1
                    })
                };
                let eps_eff = eps_of(pair[0]).max(eps_of(pair[1])).max(1.0);
                let (threshold, is_declared) = match declared {
                    Some(limit) => (limit as f64, true),
                    None => (
                        defaults::return_via_threshold(knee_edge, eps_eff, defaults::RETURN_VIA_LAMBDA_FRACTION.0)
                            * 1e9,
                        false,
                    ),
                };
                let (kind, element, distance) = match (&before, &after) {
                    (Some((first, a)), Some((second, b))) if a == b => {
                        let nearest = board
                            .vias
                            .iter()
                            .enumerate()
                            .filter(|(_, other)| other.net == *a && return_span_covers(board, other, *first, *second))
                            .map(|(other_index, other)| {
                                (dist(centre, [other.position.x_nm as f64, other.position.y_nm as f64]), other_index)
                            })
                            .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
                        match nearest {
                            Some((distance, other)) => ("stitching_via", Some(format!("via:{other}")), Some(distance)),
                            None => ("none", None, None),
                        }
                    }
                    (Some((_, a)), Some((_, b))) => match nearest_capacitor(env, a, b, centre) {
                        Some((reference, distance)) => ("decoupling_capacitor", Some(reference), Some(distance)),
                        None => ("none", None, None),
                    },
                    _ => ("none", None, None),
                };
                let drc = single_ended_return_rule(board, net).map(|rule| return_via_covered(board, via, rule));
                changes.push(Change {
                    member,
                    via: index,
                    position: centre,
                    layers: [pair[0], pair[1]],
                    before,
                    after,
                    kind,
                    element,
                    distance,
                    threshold,
                    declared: is_declared,
                    drc,
                });
            }
        }
    }
    (changes, transitions)
}

/// The capacitor with a pad on each of `a` and `b` nearest to `at` (nm): its reference and the distance to the
/// nearer of its pad centres.
fn nearest_capacitor(env: &Env, a: &str, b: &str, at: [f64; 2]) -> Option<(String, f64)> {
    let mut best: Option<(String, f64)> = None;
    for component in &env.view.components {
        if !is_capacitor_part(env.view, component) {
            continue;
        }
        let pads = env.layout.pads_of_component(&component.reference);
        let on = |net: &str| {
            pads.iter()
                .filter(|pad| env.layout.net_of_pad(&component.reference, &pad.number).as_deref() == Some(net))
                .map(|pad| pad.shape.center())
                .collect::<Vec<P2>>()
        };
        let (first, second) = (on(a), on(b));
        if first.is_empty() || second.is_empty() {
            continue;
        }
        let distance =
            first.iter().chain(second.iter()).map(|p| dist([p[0] * 1e9, p[1] * 1e9], at)).fold(f64::INFINITY, f64::min);
        if best
            .as_ref()
            .is_none_or(|(reference, d)| distance < *d || (distance == *d && component.reference < *reference))
        {
            best = Some((component.reference.clone(), distance));
        }
    }
    best
}

// ---------------------------------------------------------------------------
// The signal's level at a slot's resonance
// ---------------------------------------------------------------------------

/// The three spectrum quantities of `copper_emi::signal` at one frequency, each separately.
pub(super) struct Levels {
    pub(super) frequency: f64,
    /// Upper bound on any single line of any two-level waveform (always defined).
    pub(super) continuous: f64,
    /// For each named pattern the strongest line within an octave centred on the frequency (`None`: no line there).
    pub(super) patterns: Vec<(&'static str, Option<(f64, f64)>)>,
    /// Random NRZ data in the receiver's resolution bandwidth.
    pub(super) random: Option<f64>,
    /// The strongest clock line within the octave.
    pub(super) clock: Option<Option<(f64, f64)>>,
}

fn strongest(lines: Vec<spectra::Line>, frequency: f64) -> Option<(f64, f64)> {
    lines
        .into_iter()
        .filter(|line| line.frequency >= frequency / SQRT_2 && line.frequency <= frequency * SQRT_2)
        .map(|line| (line.frequency, line.amplitude))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

pub(super) fn levels(signal: &ResolvedSignal, swing: f64, rise: f64, fall: f64, frequency: f64) -> Levels {
    let continuous = spectra::continuous_bound(swing, rise.min(fall), frequency);
    let top = frequency * SQRT_2;
    let (patterns, random) = match signal.data_rate.filter(|rate| *rate > 0.0) {
        Some(rate) => (
            spectra::NamedPattern::ALL
                .iter()
                .map(|pattern| {
                    (
                        pattern.name(),
                        strongest(spectra::pattern_lines(*pattern, rate, swing, rise, fall, top), frequency),
                    )
                })
                .collect(),
            Some(spectra::random_data_in_rbw(swing, rate, (rise + fall) / 2.0, frequency, RBW)),
        ),
        None => (Vec::new(), None),
    };
    let clock = signal
        .clock_frequency
        .filter(|f| *f > 0.0)
        .map(|f| strongest(spectra::clock_lines(swing, f, 0.5, rise, fall, top), frequency));
    Levels { frequency, continuous, patterns, random, clock }
}

/// The score of an amplitude over a slot of `slot_mm`.
fn score(amplitude: f64, slot_mm: f64) -> Option<f64> {
    (amplitude > 0.0 && slot_mm > 0.0).then(|| 20.0 * (amplitude * slot_mm).log10())
}

fn opt_score(amplitude: Option<f64>, slot_mm: f64) -> Value {
    amplitude.and_then(|a| score(a, slot_mm)).map_or(Value::Null, |s| num(s, 2))
}

fn levels_value(levels: &Levels, slot_mm: f64) -> Value {
    let mut item = Object::new();
    put(&mut item, "frequency_hz", num(levels.frequency, 0));
    put(
        &mut item,
        "continuous_bound",
        object(vec![
            ("amplitude_v", num(levels.continuous, 6)),
            ("score_db", opt_score(Some(levels.continuous), slot_mm)),
            (
                "note",
                Value::from(
                    "upper bound on any line of any two-level waveform with this swing and edge; loose by construction",
                ),
            ),
        ]),
    );
    if !levels.patterns.is_empty() {
        let patterns = levels
            .patterns
            .iter()
            .map(|(name, line)| {
                object(vec![
                    ("pattern", Value::from(*name)),
                    ("line_hz", line.map_or(Value::Null, |l| num(l.0, 0))),
                    ("amplitude_v", line.map_or(Value::Null, |l| num(l.1, 6))),
                    ("score_db", opt_score(line.map(|l| l.1), slot_mm)),
                ])
            })
            .collect();
        put(
            &mut item,
            "pattern_lines",
            object(vec![
                (
                    "window",
                    Value::from(
                        "the strongest line within one octave centred on the resonance, per named pattern; no pattern bounds the others",
                    ),
                ),
                ("patterns", Value::Array(patterns)),
            ]),
        );
    }
    if let Some(random) = levels.random {
        put(
            &mut item,
            "random_data_in_rbw",
            object(vec![
                ("rbw_hz", num(RBW, 0)),
                ("amplitude_v", num(random, 9)),
                ("score_db", opt_score(Some(random), slot_mm)),
            ]),
        );
    }
    if let Some(clock) = &levels.clock {
        put(
            &mut item,
            "clock_lines",
            object(vec![
                ("line_hz", clock.map_or(Value::Null, |l| num(l.0, 0))),
                ("amplitude_v", clock.map_or(Value::Null, |l| num(l.1, 6))),
                ("score_db", opt_score(clock.map(|l| l.1), slot_mm)),
            ]),
        );
    }
    Value::Object(item)
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

fn reference_value(reference: &Reference) -> Value {
    match reference {
        Some((layer, net)) => object(vec![("layer", Value::from(layer.value())), ("net", Value::from(net.as_str()))]),
        None => Value::Null,
    }
}

/// The ranking score of the worst gap, its band and the levels behind it.
struct Ranking {
    worst: usize,
    score: f64,
    band: Option<[f64; 2]>,
    levels: Levels,
}

fn ranking(signal: &ResolvedSignal, gaps: &[Gap]) -> Option<Ranking> {
    let swing = signal.swing.as_ref()?;
    let (rise, fall) = (signal.rise_ramp(), signal.fall_ramp());
    let at = |gap: &Gap, swing: f64, rise: f64, fall: f64| {
        let frequency = gap.resonance();
        let amplitude = spectra::continuous_bound(swing, rise.min(fall), frequency);
        score(amplitude, gap.slot * 1e-6)
    };
    let (worst, best) = gaps
        .iter()
        .enumerate()
        .filter_map(|(index, gap)| at(gap, swing.value, rise, fall).map(|s| (index, s)))
        .fold(None, |best: Option<(usize, f64)>, item| match best {
            Some(current) if current.1 >= item.1 => Some(current),
            _ => Some(item),
        })?;
    let gap = &gaps[worst];
    let band =
        (swing.band.is_some() || signal.rise_time.band.is_some() || signal.fall_time.band.is_some()).then(|| {
            let ramp = |number: &signals::Number, slow: bool| {
                let seconds = number.band.map_or(number.value, |b| if slow { b.1 } else { b.0 });
                seconds / defaults::RAMP_10_90
            };
            let swing_at = |high: bool| swing.band.map_or(swing.value, |b| if high { b.1 } else { b.0 });
            let low = at(gap, swing_at(false), ramp(&signal.rise_time, true), ramp(&signal.fall_time, true));
            let high = at(gap, swing_at(true), ramp(&signal.rise_time, false), ramp(&signal.fall_time, false));
            [low.unwrap_or(best).min(best), high.unwrap_or(best).max(best)]
        });
    Some(Ranking { worst, score: best, band, levels: levels(signal, swing.value, rise, fall, gap.resonance()) })
}

fn kind_of(gap: &Gap) -> &'static str {
    if gap.reaches_edge { "cut_to_plane_edge" } else { "enclosed_void" }
}

/// The text of one gap for the finding's message.
fn gap_text(gap: &Gap) -> String {
    format!(
        "slot {:.2} mm at ({:.2}, {:.2}) mm on {} over {}, resonance {:.0} MHz",
        nmm(gap.slot),
        nmm(gap.position[0]),
        nmm(gap.position[1]),
        gap.signal_layer.value(),
        gap.plane_layer.value(),
        gap.resonance() / 1e6
    )
}

fn make_finding(subject: &Subject, outcome: &Outcome, resolved: &ResolvedSignal) -> EmiFinding {
    let layers = names(outcome.members.iter().flat_map(|m| m.layers.iter().map(|l| l.layer)));
    let unreturned = outcome.unreturned();
    let ranked = ranking(resolved, &outcome.gaps);
    let label = subject.nets.join("/");
    let mut parts: Vec<String> = Vec::new();
    let (routed, uncovered) = (outcome.routed(), outcome.uncovered());
    if outcome.members.iter().any(|m| !m.no_reference.is_empty()) {
        parts.push("copper on a layer with no reference plane".to_owned());
    }
    if uncovered > NEGLIGIBLE_NM {
        parts.push(format!(
            "{:.2} mm of {:.2} mm routed has no reference copper within {} H of its centre line",
            nmm(uncovered),
            nmm(routed),
            defaults::RETURN_WINDOW_H.0
        ));
    }
    if !outcome.gaps.is_empty() {
        let longest = outcome.gaps.iter().max_by(|a, b| a.slot.total_cmp(&b.slot)).expect("a gap");
        parts.push(format!("{} gap(s) crossed, longest {}", outcome.gaps.len(), gap_text(longest)));
    }
    if !outcome.changes.is_empty() {
        parts.push(format!(
            "{} reference change(s) at vias, {} without a return path within the threshold",
            outcome.changes.len(),
            unreturned.len()
        ));
    }
    let reported = outcome.changes.iter().filter(|c| !c.within() && c.drc == Some(false)).count();
    if reported > 0 {
        parts.push(format!(
            "{reported} of them also fail the declared rule DRC-RETURN-VIA (the DRC reports those as violations; here they are measurements with their distance)"
        ));
    }
    if let Some(text) = edge_summary(outcome) {
        parts.push(text);
    }
    let mut message = format!("{label}: return path: {}", parts.join("; "));
    if let Some(ranked) = &ranked {
        message.push_str(&format!(
            "; slot ranking score {:+.1} dB (ranking only: not a field, not comparable with an emission limit)",
            ranked.score
        ));
    }
    let objects = if subject.nets.len() == 2 { vec![label.clone()] } else { subject.nets.clone() };
    let mut finding = EmiFinding {
        finding: Finding::new("EMI-RETURN-PATH", Severity::Warning, message)
            .objects(objects)
            .nets(subject.nets.clone())
            .layers(layers),
        cell: None,
        provenance: None,
        tags: vec![defaults::TAG_RETURN_WINDOW.to_owned(), defaults::TAG_EDGE_DISTANCE.to_owned()],
        estimate: None,
        margin_db: None,
        accepted: None,
        measures: Object::new(),
        subjects: Vec::new(),
        rank: outcome.gaps.len() as f64 + unreturned.len() as f64 + nmm(uncovered) * 1e-3,
        conducted_input: None,
    };
    if let Some(source) = subject.source {
        finding.subjects.extend(endpoint_names(&source.pins));
        finding.subjects.extend(source.pins.iter().map(|pin| pin.component.clone()));
    }
    finding.subjects.extend(subject.nets.iter().cloned());
    dedup(&mut finding.subjects);
    let used_default_threshold = outcome.changes.iter().any(|c| !c.declared);
    if used_default_threshold {
        add_tag(&mut finding, defaults::TAG_RETURN_VIA);
    }
    let edge_default = resolved.rise_time.is_default() || resolved.fall_time.is_default();
    if let Some(ranked) = &ranked {
        add_tag(&mut finding, "ranking_only");
        let worst = &outcome.gaps[ranked.worst];
        let slot_mm = nmm(worst.slot);
        finding.estimate = Some(Estimate {
            quantity: "slot_ranking_score".to_owned(),
            value: rounded(ranked.score, 2),
            unit: "dB".to_owned(),
            method: format!(
                "ranking score of the worst gap: slot {slot_mm:.3} mm, half-wave resonance {:.1} MHz, continuous-bound amplitude {:.4} V of a {:.3} V swing with a {:.3} ns ramp",
                worst.resonance() / 1e6,
                ranked.levels.continuous,
                resolved.swing.as_ref().map_or(0.0, |s| s.value),
                resolved.rise_ramp().min(resolved.fall_ramp()) * 1e9
            ),
            kind: EstimateKind::RankingScore,
            definition: Some(SCORE_DEFINITION.to_owned()),
            not_modelled: NOT_MODELLED.iter().map(|m| (*m).to_owned()).collect(),
            frequency_hz: Some(rounded(worst.resonance(), 0)),
            band_db: ranked.band.map(|b| [rounded(b[0], 2), rounded(b[1], 2)]),
            basis: ranked.band.map(|_| defaults::BASIS.to_owned()),
        });
        if let Some(tag) = resolved.swing.as_ref().filter(|s| s.is_default()).and_then(|s| s.tag) {
            add_tag(&mut finding, tag);
        }
    }
    if (ranked.is_some() || used_default_threshold) && edge_default {
        add_tag(&mut finding, defaults::TAG_SIGNAL_EDGE);
    }
    // Measures.
    let measures = &mut finding.measures;
    put(measures, "subject", Value::from(subject.name.as_str()));
    put(measures, "members", Value::Array(outcome.members.iter().map(member_value).collect()));
    put(measures, "routed_length_mm", num(nmm(routed), 3));
    put(measures, "uncovered_length_mm", num(nmm(uncovered), 3));
    put(
        measures,
        "reference",
        Value::Array(
            outcome
                .references
                .iter()
                .map(|r| {
                    let mut entries = vec![
                        ("signal_layer", Value::from(r.signal_layer.value())),
                        ("reference_layer", Value::from(r.plane_layer.value())),
                        ("reference_nets", Value::Array(r.nets.iter().map(|n| Value::from(n.as_str())).collect())),
                        ("height_mm", num(r.height * 1e3, 4)),
                        ("structure", Value::from(r.structure)),
                        ("effective_permittivity", num(r.eps_eff, 4)),
                    ];
                    entries.push((
                        "other_plane",
                        r.other.map_or(Value::Null, |(layer, height)| {
                            object(vec![
                                ("layer", Value::from(layer.value())),
                                ("height_mm", num(height * 1e3, 4)),
                                ("evaluated", Value::Bool(false)),
                            ])
                        }),
                    ));
                    object(entries)
                })
                .collect(),
        ),
    );
    put(measures, "gaps", Value::Array(outcome.gaps.iter().map(|g| gap_value(subject, g)).collect()));
    put(
        measures,
        "reference_changes",
        Value::Array(outcome.changes.iter().map(|c| change_value(subject, c)).collect()),
    );
    put(measures, "via_transitions_examined", Value::from(outcome.transitions as i64));
    put(
        measures,
        "drc_return_via_reported",
        Value::Array(
            outcome
                .changes
                .iter()
                .filter(|c| c.drc == Some(false))
                .map(|c| Value::from(format!("via:{}", c.via)))
                .collect(),
        ),
    );
    put(
        measures,
        "thresholds",
        object(vec![
            ("coverage_window_h", num(defaults::RETURN_WINDOW_H.0, 2)),
            (
                "coverage_window_fraction_of_return_current",
                num(defaults::return_window_fraction(defaults::RETURN_WINDOW_H.0), 4),
            ),
            ("edge_distance_h", num(defaults::EDGE_DISTANCE_H.0, 2)),
            ("return_via_lambda_fraction", num(defaults::RETURN_VIA_LAMBDA_FRACTION.0, 2)),
        ]),
    );
    if let Some(ranked) = &ranked {
        let worst = &outcome.gaps[ranked.worst];
        put(
            measures,
            "ranking",
            object(vec![
                ("worst_gap_position_mm", mm_point(worst.position)),
                ("slot_mm", num(nmm(worst.slot), 4)),
                ("spectrum", levels_value(&ranked.levels, nmm(worst.slot))),
            ]),
        );
    } else {
        let reason = if outcome.gaps.is_empty() {
            "no gap is crossed"
        } else {
            "the signal has no swing (declare signal_swing on the driving pin: a single-ended signal gets no default without its supply voltage)"
        };
        put(measures, "ranking", object(vec![("score", Value::Null), ("reason", Value::from(reason))]));
    }
    let mut inputs = vec![
        format!("reference window: {}", defaults::return_window_rule()),
        format!("edge distance: {}", defaults::edge_distance_rule()),
    ];
    if used_default_threshold {
        inputs.push(format!("return via distance: {}", defaults::return_via_rule()));
    }
    if resolved.rise_time.is_default() {
        inputs.push(format!(
            "edge time {:.3} ns {}",
            resolved.rise_time.value * 1e9,
            defaults::rule_text(resolved.rise_time.rule.as_deref().unwrap_or(""), "rise_time on the driving pin")
        ));
    }
    if let Some(swing) = resolved.swing.as_ref().filter(|s| s.is_default()) {
        inputs.push(format!(
            "swing {:.3} V {}",
            swing.value,
            defaults::rule_text(swing.rule.as_deref().unwrap_or(""), "signal_swing on the driving pin")
        ));
    }
    if subject.source.is_none() {
        inputs.push(
            "no signal source: the net has a differential, clock or rf_feed routing rule only, so its edge comes from the default band"
                .to_owned(),
        );
    }
    put(measures, "inputs_text", Value::Array(inputs.into_iter().map(Value::from).collect()));
    finding
}

fn edge_value(stats: &EdgeStats, limit: f64) -> Value {
    object(vec![
        ("nearest_mm", stats.nearest.map_or(Value::Null, |(d, _)| num(nmm(d), 4))),
        ("at_mm", stats.nearest.map_or(Value::Null, |(_, p)| mm_point(p))),
        ("limit_mm", num(nmm(limit), 4)),
        ("within_limit", Value::Bool(stats.nearest.is_some_and(|(d, _)| d < limit))),
        ("length_within_limit_mm", num(nmm(stats.within), 3)),
    ])
}

fn member_value(member: &MemberResult) -> Value {
    let layers = member
        .layers
        .iter()
        .map(|l| {
            let mut entries = vec![
                ("layer", Value::from(l.layer.value())),
                ("segments", Value::from(l.segments as i64)),
                ("routed_length_mm", num(nmm(l.routed), 3)),
                ("uncovered_length_mm", num(nmm(l.uncovered), 3)),
                ("plane_edge", edge_value(&l.plane_edge, l.limit)),
                ("board_edge", edge_value(&l.board_edge, l.limit)),
            ];
            if l.outer {
                entries.push(("outer_layer_unguarded_edge_length_mm", num(nmm(l.unguarded), 3)));
            }
            object(entries)
        })
        .collect();
    object(vec![
        ("net", Value::from(member.net.as_str())),
        ("routed_length_mm", num(nmm(member.routed), 3)),
        ("uncovered_length_mm", num(nmm(member.uncovered), 3)),
        ("layers", Value::Array(layers)),
        (
            "layers_without_reference_plane",
            Value::Array(member.no_reference.iter().map(|l| Value::from(l.value())).collect()),
        ),
    ])
}

fn gap_value(subject: &Subject, gap: &Gap) -> Value {
    object(vec![
        ("nets", Value::Array(gap.crossings.iter().map(|c| Value::from(subject.nets[c.0].as_str())).collect())),
        ("signal_layer", Value::from(gap.signal_layer.value())),
        ("reference_layer", Value::from(gap.plane_layer.value())),
        ("position_mm", mm_point(gap.position)),
        ("kind", Value::from(kind_of(gap))),
        ("via_antipads_in_void", Value::from(gap.vias as i64)),
        (
            "crossings",
            Value::Array(
                gap.crossings
                    .iter()
                    .map(|c| {
                        object(vec![
                            ("net", Value::from(subject.nets[c.0].as_str())),
                            ("segments", Value::from(c.2 as i64)),
                            ("length_mm", num(nmm(c.1), 4)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("slot_length_mm", num(nmm(gap.slot), 4)),
        ("structure", Value::from(gap.structure)),
        ("effective_permittivity", num(gap.eps_eff, 4)),
        ("resonance_hz", num(gap.resonance(), 0)),
        // A notch open at the plane edge and closed at the other end resonates at a quarter wave; the void's shape
        // does not say whether the cut is a notch or runs through, so both are reported and the score uses the half wave.
        ("quarter_wave_resonance_hz", if gap.reaches_edge { num(gap.resonance() / 2.0, 0) } else { Value::Null }),
    ])
}

fn change_value(subject: &Subject, change: &Change) -> Value {
    let drc = match change.drc {
        None => Value::Null,
        Some(satisfied) => object(vec![
            ("rule", Value::from("DRC-RETURN-VIA")),
            ("satisfied", Value::Bool(satisfied)),
            ("reported_by_drc", Value::Bool(!satisfied)),
        ]),
    };
    object(vec![
        ("net", Value::from(subject.nets[change.member].as_str())),
        ("via", Value::from(format!("via:{}", change.via))),
        ("position_mm", mm_point(change.position)),
        ("layers", Value::Array(change.layers.iter().map(|l| Value::from(l.value())).collect())),
        ("reference_before", reference_value(&change.before)),
        ("reference_after", reference_value(&change.after)),
        ("return_path", Value::from(change.kind)),
        ("return_element", change.element.as_deref().map_or(Value::Null, Value::from)),
        ("return_distance_mm", change.distance.map_or(Value::Null, |d| num(nmm(d), 4))),
        ("threshold_mm", num(nmm(change.threshold), 4)),
        (
            "threshold_source",
            Value::from(if change.declared {
                "declared maximum_return_via_distance"
            } else {
                "lambda_over_20_at_knee_frequency"
            }),
        ),
        ("within_threshold", Value::Bool(change.within())),
        ("drc_rule", drc),
    ])
}

fn subject_detail(subject: &Subject, outcome: &Outcome, finding: bool) -> String {
    let references: Vec<String> = outcome
        .references
        .iter()
        .map(|r| {
            format!(
                "{} over {} ({}, H {:.3} mm)",
                r.signal_layer.value(),
                r.plane_layer.value(),
                r.nets.join("/"),
                r.height * 1e3
            )
        })
        .collect();
    let mut detail = format!(
        "{}: {:.2} mm routed, {:.2} mm without reference copper within {} H, {} gap(s), {} reference change(s) of {} via transition(s), {}{}",
        subject.nets.join("/"),
        nmm(outcome.routed()),
        nmm(outcome.uncovered()),
        defaults::RETURN_WINDOW_H.0,
        outcome.gaps.len(),
        outcome.changes.len(),
        outcome.transitions,
        if references.is_empty() { "no reference plane on any layer used".to_owned() } else { references.join(", ") },
        if finding {
            "; see the EMI-RETURN-PATH finding"
        } else {
            "; nothing to report at these thresholds (not a pass: slot radiation, plane-pair cavity modes and the enclosure are not modelled)"
        }
    );
    if outcome.members.iter().any(|m| !m.no_reference.is_empty()) {
        detail.push_str("; copper on a layer with no reference plane");
    }
    detail
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the check and add its findings to `cx`; the returned coverage entries (the overall `EMI-RETURN-PATH` entry
/// first, then one per subject) are added by the caller where the coverage list holds the entry.
fn env_of<'c>(cx: &'c Context<'_>, fills: &PlaneFills) -> Env<'c> {
    Env {
        board: cx.layout.board,
        view: cx.layout.view,
        stack: &cx.stack,
        layout: &cx.layout,
        planes: build_planes(cx.layout.board, cx.layout.view, fills),
        references: cx.layout.view.supplies.iter().map(|supply| supply.net.clone()).collect(),
        ring: cx.layout.ring().into_iter().map(|p| [p[0] * 1e9, p[1] * 1e9]).collect(),
    }
}

/// A void of the reference copper that a straight corridor crosses.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SlotBetween {
    pub(super) plane_layer: CopperLayer,
    /// Where the void meets the corridor, nm.
    pub(super) at: [f64; 2],
    /// The extent of the void perpendicular to the corridor, nm.
    pub(super) slot: f64,
    /// The void reaches the edge of the plane (a cut or a notch).
    pub(super) reaches_edge: bool,
}

/// The voids of the reference plane of `layer` that the straight corridor from `a` to `b` (nm), widened by `3 H` each
/// side (H the height over the plane, as the return-path check), crosses: the slots between two loops of the
/// ground-noise network of `EMI-CABLE-COMMON-MODE`. A void that is one via's own antipad is not a slot. `None` when
/// the layer has no reference plane in the fills.
pub(super) fn slots_between(
    cx: &Context<'_>,
    fills: &PlaneFills,
    layer: CopperLayer,
    a: [f64; 2],
    b: [f64; 2],
) -> Option<Vec<SlotBetween>> {
    let env = env_of(cx, fills);
    let choice = env.choice(layer)?;
    let plane = &env.planes[choice.plane];
    let corridor = Seg { member: 0, layer, a, b, width: 0.0 };
    if corridor.length() <= 0.0 {
        return Some(Vec::new());
    }
    let strip = corridor.strip(defaults::RETURN_WINDOW_H.0 * choice.height * 1e9);
    let strip_bounds = bounds_of(&strip)?;
    let board = env.board;
    let mut found = Vec::new();
    for void in &plane.voids {
        if !overlap(void.bounds, strip_bounds) {
            continue;
        }
        let cut = poly::intersection(&strip, &vec![void.shape.clone()]);
        if union_length(corridor.stations(&cut)) < NEGLIGIBLE_NM {
            continue;
        }
        let extent = (void.bounds[2] - void.bounds[0]).max(void.bounds[3] - void.bounds[1]) as f64;
        let own_antipad = void.vias == 1
            && board.vias.iter().any(|via| {
                void.shape.first().is_some_and(|outer| {
                    poly::contour_contains(outer, (via.position.x_nm as f64, via.position.y_nm as f64))
                }) && extent <= OWN_ANTIPAD_DIAMETERS * via.size_nm as f64
            });
        if own_antipad {
            continue;
        }
        let d = corridor.unit();
        let normal = [-d[1], d[0]];
        let (lo, hi) = void.shape.first().map_or((0.0, 0.0), |outer| {
            outer.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |acc, p| {
                let t = p.x as f64 * normal[0] + p.y as f64 * normal[1];
                (acc.0.min(t), acc.1.max(t))
            })
        });
        let at = bounds_of(&cut).map_or(a, |b| [(b[0] + b[2]) as f64 / 2.0, (b[1] + b[3]) as f64 / 2.0]);
        found.push(SlotBetween { plane_layer: plane.layer, at, slot: hi - lo, reaches_edge: void.reaches_edge });
    }
    Some(found)
}

/// How much of one leg's routed copper lacks its return reference, for the amplitude imbalance of
/// `EMI-MODE-CONVERSION`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct LegExposure {
    /// Routed length of the leg, nm.
    pub(super) routed: f64,
    /// Length with no reference copper within the coverage window, plus the longer of the lengths within the edge limit
    /// of the reference copper's edge and of the board edge (per layer), at most the routed length, nm.
    pub(super) exposed: f64,
}

impl LegExposure {
    /// The exposed fraction of the routed length (0 for an unrouted leg).
    pub(super) fn fraction(&self) -> f64 {
        if self.routed > 0.0 { (self.exposed / self.routed).clamp(0.0, 1.0) } else { 0.0 }
    }
}

/// The exposure of both legs of each `(P net, N net)` section to return-path breaks, measured as the return-path
/// check measures a pair (same fills, same coverage window and edge limit). `None` for a section with no routed
/// copper on either leg.
pub(super) fn leg_exposures(
    cx: &Context<'_>,
    fills: &PlaneFills,
    sections: &[(String, String)],
) -> Vec<Option<[LegExposure; 2]>> {
    let env = env_of(cx, fills);
    sections
        .iter()
        .map(|(p, n)| {
            let subject = Subject { name: p.clone(), nets: vec![p.clone(), n.clone()], source: None };
            let resolved = signals::resolve(&bare_source(&subject), None);
            let outcome = examine(&env, &subject, &resolved)?;
            let leg = |member: &MemberResult| {
                let edges: f64 = member.layers.iter().map(|l| l.plane_edge.within.max(l.board_edge.within)).sum();
                LegExposure { routed: member.routed, exposed: (member.uncovered + edges).min(member.routed) }
            };
            match outcome.members.as_slice() {
                [first, second] => Some([leg(first), leg(second)]),
                _ => None,
            }
        })
        .collect()
}

/// The reference plane layer and the height over it (m) of a trace on `layer`, as the return-path check chooses them
/// from the fills (`None`: no plane layer in the fills).
pub(super) fn reference_of(cx: &Context<'_>, fills: &PlaneFills, layer: CopperLayer) -> Option<(CopperLayer, f64)> {
    let planes = build_planes(cx.layout.board, cx.layout.view, fills);
    let choice = choose(&cx.stack, &planes, layer)?;
    Some((planes[choice.plane].layer, choice.height))
}

pub(super) fn analyze(cx: &mut Context, input: &ReturnPathInput) -> Vec<(String, CoverageStatus, String)> {
    const CHECK: &str = "EMI-RETURN-PATH";
    let model = cx.model;
    let list = subjects(cx.layout.board, model);
    let entry = |status, detail: String| vec![(CHECK.to_owned(), status, detail)];
    let fills = match input {
        ReturnPathInput::NotRequested if list.is_empty() => {
            return entry(CoverageStatus::Unsupported, NOT_REQUESTED_REASON.to_owned());
        }
        ReturnPathInput::NotRequested => {
            let names: Vec<String> = list.iter().map(|s| s.nets.join("/")).collect();
            return entry(
                CoverageStatus::Unsupported,
                format!(
                    "{} subject net(s) ({}) but no routed copper and zone fills were supplied: run `copper emi-check BOARD --run RUN` on a routed board (plane gaps, plane edges and layer changes without a return via are not examined on the placed prototype)",
                    list.len(),
                    names.join(", ")
                ),
            );
        }
        ReturnPathInput::Unavailable(reason) => {
            return entry(
                CoverageStatus::Unsupported,
                format!(
                    "the zone fills of the routed board could not be made ({reason}): reference-plane gaps, plane edges and layer changes without a return via are not examined"
                ),
            );
        }
        ReturnPathInput::Fills(fills) => fills,
    };
    if list.is_empty() {
        return entry(
            CoverageStatus::NotApplicable,
            "no net has a signal source (a driving pin declaring data_rate, clock_frequency, signal_swing or an edge below 2 ns) or a differential, clock or rf_feed routing rule".to_owned(),
        );
    }
    let mut results: Vec<(Option<EmiFinding>, (CoverageStatus, String))> = Vec::new();
    {
        let env = env_of(cx, fills);
        for subject in &list {
            let resolved = signals::resolve(&subject.source.map_or_else(|| bare_source(subject), Clone::clone), None);
            match examine(&env, subject, &resolved) {
                None => results.push((
                    None,
                    (
                        CoverageStatus::Unsupported,
                        format!("{}: no routed copper (tracks, arcs or vias) on the net: the return path of an unrouted net is not examined", subject.nets.join("/")),
                    ),
                )),
                Some(outcome) => {
                    let finding = outcome.has_findings().then(|| make_finding(subject, &outcome, &resolved));
                    let detail = subject_detail(subject, &outcome, finding.is_some());
                    results.push((finding, (CoverageStatus::Executed, detail)));
                }
            }
        }
    }
    let examined = results.iter().filter(|(_, (status, _))| *status == CoverageStatus::Executed).count();
    let overall = if examined == 0 {
        (
            CoverageStatus::Unsupported,
            "no subject net has routed copper: reference-plane gaps, plane edges and layer changes without a return via are not examined".to_owned(),
        )
    } else {
        let planes = fills.fills.iter().map(|f| f.layer).collect::<BTreeSet<_>>().len();
        (
            CoverageStatus::Executed,
            format!(
                "{examined} of {} subject net(s) examined on the routed board with its native zone fills ({planes} layer(s) with fills): reference coverage within {} H, slot crossings with their half-wave resonance, reference changes at vias with the nearest return path, plane-edge and board-edge distance against {} H; screening evidence from the filled copper, never a pass or a fail, and the plane-pair cavity, slot radiation efficiency and enclosure are not modelled",
                list.len(),
                defaults::RETURN_WINDOW_H.0,
                defaults::EDGE_DISTANCE_H.0
            ),
        )
    };
    let mut coverage = vec![(CHECK.to_owned(), overall.0, overall.1)];
    for (subject, (finding, (status, detail))) in list.iter().zip(results) {
        if let Some(finding) = finding {
            cx.findings.push(finding);
        }
        coverage.push((format!("{CHECK}:{}", subject.name), status, detail));
    }
    coverage
}

/// The edge distances of a finding that has one within the limit, as text.
fn edge_summary(outcome: &Outcome) -> Option<String> {
    let layers: Vec<&LayerStats> = outcome.members.iter().flat_map(|m| &m.layers).collect();
    let nearest = |pick: fn(&LayerStats) -> &EdgeStats| {
        layers
            .iter()
            .filter_map(|l| pick(l).nearest.filter(|(d, _)| *d < l.limit).map(|(d, _)| (d, l.limit)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    };
    let (plane, board) = (nearest(|l| &l.plane_edge), nearest(|l| &l.board_edge));
    if plane.is_none() && board.is_none() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some((d, limit)) = plane {
        parts.push(format!("{:.2} mm from the edge of the reference copper (limit {:.2} mm)", nmm(d), nmm(limit)));
    }
    if let Some((d, limit)) = board {
        parts.push(format!("{:.2} mm from the board edge (limit {:.2} mm)", nmm(d), nmm(limit)));
    }
    let unguarded: f64 = layers.iter().map(|l| l.unguarded).sum();
    if unguarded > 0.0 {
        parts.push(format!(
            "{:.2} mm of outer-layer copper along the board edge with no stitched ground via on the edge side",
            nmm(unguarded)
        ));
    }
    Some(format!("trace {}", parts.join(", ")))
}

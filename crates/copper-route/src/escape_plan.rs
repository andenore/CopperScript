//! Escape planning (`pcbir.escape_plan`): which side, layer and order package
//! pins leave by.
//!
//! Diagnostics (E0: channel load and two-end crossings), the channel model of
//! pad fields, gaps and tiles (E1), array escapes by min-cost max-flow (E2)
//! and the two-end plan of crossing bundles (E3, E5). Diagnostics never
//! change routing; every number is computed from integer pad geometry with
//! deterministic tie-breaks.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_integer::{Integer, Roots};
use num_traits::{Signed, ToPrimitive};

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, Placement};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::zones::{distribution_zone_nets, routing_layers};
use copper_pcb::{Error, Result};
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::geometry::{
    Bounds, RoundedConvexShape, bounds, orientation, point_on_segment, shape_distance_squared,
};
use copper_physical::pyfloat::float_mod;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::routing_costs::LayerBias;
use crate::routing_lattice::true_div;

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn text(value: &str) -> Value {
    Value::Str(value.to_owned())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

fn strings<'s>(values: impl IntoIterator<Item = &'s str>) -> Value {
    Value::Array(values.into_iter().map(text).collect())
}

/// `_pad_text`.
fn pad_text(pad: &PadReference) -> String {
    format!("{}.{}", pad.component, pad.pad)
}

/// Total order on floats that never meet NaN (Python compares them plainly).
fn float_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).expect("finite escape geometry")
}

/// `EscapePlanOptions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapePlanOptions {
    pub cluster_pair_minimum_nets: i64,
    pub maximum_pair_nets: i64,
    pub field_spacing_factor: i64,
    pub field_spacing_minimum_nm: i64,
    pub two_end_order: bool,
    pub corridor_margin_nm: i64,
    pub layer_bias_rate: i64,
    pub array_flow: bool,
    pub bus_layers: bool,
    pub via_lattice: bool,
    pub lattice_band_rings: i64,
}

impl Default for EscapePlanOptions {
    fn default() -> Self {
        EscapePlanOptions {
            cluster_pair_minimum_nets: 2,
            maximum_pair_nets: 256,
            field_spacing_factor: 3,
            field_spacing_minimum_nm: 3_000_000,
            two_end_order: false,
            corridor_margin_nm: 1_000_000,
            layer_bias_rate: 20,
            array_flow: false,
            bus_layers: false,
            via_lattice: false,
            lattice_band_rings: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterPin {
    pub pad: PadReference,
    pub net: Option<String>,
    pub position: Point,
    pub layers: Vec<CopperLayer>,
    pub outer: bool,
}

/// Signal pins inside a pad field's outer ring against the tracks its gaps hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelLoad {
    pub layer: CopperLayer,
    pub demand: i64,
    pub capacity: i64,
    pub gaps: i64,
}

impl ChannelLoad {
    pub fn overflow(&self) -> i64 {
        0.max(self.demand - self.capacity)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterEscape {
    pub component: String,
    pub pins: Vec<ClusterPin>,
    pub bounds: Bounds,
    pub inner_signal_pins: Vec<PadReference>,
    pub channels: Vec<ChannelLoad>,
    pub fields: Vec<PadField>,
    /// Escape track width and clearance of the component's signal nets (the widest).
    pub rules: (i64, i64),
}

impl ClusterEscape {
    /// Inner signal pins the ring gaps cannot pass: surface pins on their
    /// layer, through-hole pins on any.
    pub fn overflow(&self) -> i64 {
        let mut spare: IndexMap<CopperLayer, i64> = self.channels.iter().map(|c| (c.layer, c.capacity)).collect();
        let layers: IndexMap<&PadReference, &Vec<CopperLayer>> =
            self.pins.iter().map(|pin| (&pin.pad, &pin.layers)).collect();
        let mut short = 0;
        for (layer, capacity) in spare.iter_mut() {
            let surface = self.inner_signal_pins.iter().filter(|pad| layers[pad].as_slice() == [*layer]).count() as i64;
            short += 0.max(surface - *capacity);
            *capacity = 0.max(*capacity - surface);
        }
        let through = self.inner_signal_pins.iter().filter(|pad| layers[pad].len() > 1).count() as i64;
        short + 0.max(through - spare.values().sum::<i64>())
    }

    pub fn report(&self) -> Value {
        object(vec![
            ("component", text(&self.component)),
            ("pins", int(self.pins.len() as i64)),
            ("inner_signal_pins", Value::Array(self.inner_signal_pins.iter().map(|p| text(&pad_text(p))).collect())),
            (
                "channels",
                Value::Array(
                    self.channels
                        .iter()
                        .map(|item| {
                            object(vec![
                                ("layer", text(item.layer.value())),
                                ("demand", int(item.demand)),
                                ("capacity", int(item.capacity)),
                                ("gaps", int(item.gaps)),
                                ("overflow", int(item.overflow())),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("overflow", int(self.overflow())),
            ("fields", Value::Array(self.fields.iter().map(|f| f.report(self.rules.0, self.rules.1)).collect())),
        ])
    }
}

/// A place along a cluster boundary: (angle, squared reach, pad).
pub type BoundaryPlace = (f64, f64, PadReference);

fn place_cmp(a: &BoundaryPlace, b: &BoundaryPlace) -> Ordering {
    float_cmp(a.0, b.0).then(float_cmp(a.1, b.1)).then_with(|| a.2.cmp(&b.2))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClusterPairEscape {
    pub first: String,
    pub second: String,
    pub nets: Vec<String>,
    /// Net pairs whose boundary orders disagree: they cross on one layer.
    pub crossing_pairs: i64,
    pub crossing_nets: Vec<String>,
    /// Nets outside one largest non-crossing subset: another layer or a detour.
    pub layer_moves: Vec<String>,
    pub analysed: bool,
    /// The bundle along the first component's boundary, from its far side.
    pub order: Vec<String>,
    /// Each net of `order`'s place along the second component's boundary.
    pub second_keys: Vec<BoundaryPlace>,
}

impl ClusterPairEscape {
    pub fn report(&self) -> Value {
        object(vec![
            ("components", strings([self.first.as_str(), self.second.as_str()])),
            ("nets", strings(self.nets.iter().map(String::as_str))),
            ("crossing_pairs", int(self.crossing_pairs)),
            ("crossing_nets", strings(self.crossing_nets.iter().map(String::as_str))),
            ("layer_moves", strings(self.layer_moves.iter().map(String::as_str))),
            ("analysed", Value::Bool(self.analysed)),
        ])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EscapeDiagnostics {
    pub clusters: Vec<ClusterEscape>,
    pub pairs: Vec<ClusterPairEscape>,
}

impl EscapeDiagnostics {
    /// Clusters with a channel load or overflow, and every cluster pair.
    pub fn report(&self) -> Value {
        let mut overflowing: Vec<&str> =
            self.clusters.iter().filter(|c| c.overflow() != 0).map(|c| c.component.as_str()).collect();
        overflowing.sort_unstable();
        object(vec![
            ("schema", text("copperscript.escape-diagnostics/v0.1")),
            (
                "clusters",
                Value::Array(
                    self.clusters.iter().filter(|c| !c.inner_signal_pins.is_empty()).map(|c| c.report()).collect(),
                ),
            ),
            ("pairs", Value::Array(self.pairs.iter().map(|p| p.report()).collect())),
            ("overflow_clusters", strings(overflowing)),
            ("crossing_pairs", int(self.pairs.iter().map(|p| p.crossing_pairs).sum())),
        ])
    }

    /// Escape causes per net: `two_end_crossing` and `channel_overflow`.
    ///
    /// Every net of a bundle with a crossing is tagged, not only the crossing
    /// ones: whichever nets route first take the gaps the others need.
    pub fn net_causes(&self) -> BTreeMap<String, Vec<String>> {
        let mut causes: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut add = |net: &str, cause: &str| {
            let entry = causes.entry(net.to_owned()).or_default();
            if !entry.iter().any(|c| c == cause) {
                entry.push(cause.to_owned());
                entry.sort();
            }
        };
        for pair in &self.pairs {
            if pair.crossing_pairs != 0 {
                for net in &pair.nets {
                    add(net, "two_end_crossing");
                }
            }
        }
        let pin_nets: IndexMap<&PadReference, &Option<String>> =
            self.clusters.iter().flat_map(|c| c.pins.iter().map(|pin| (&pin.pad, &pin.net))).collect();
        for cluster in &self.clusters {
            if cluster.overflow() != 0 {
                for pad in &cluster.inner_signal_pins {
                    // Inner signal pins always carry a net.
                    if let Some(net) = pin_nets[pad] {
                        add(net, "channel_overflow");
                    }
                }
            }
        }
        causes
    }
}

/// Pads of nets with at least two pads, by pad (a later net wins, as in Python).
fn pad_net_map(board: &PhysicalBoard) -> IndexMap<PadReference, String> {
    let mut pad_nets = IndexMap::new();
    for net in board.nets.iter().filter(|net| net.pads.len() >= 2) {
        for pad in &net.pads {
            pad_nets.insert(pad.clone(), net.name.clone());
        }
    }
    pad_nets
}

/// `escape_diagnostics`.
pub fn escape_diagnostics(board: &PhysicalBoard, options: &EscapePlanOptions) -> Result<EscapeDiagnostics> {
    if options.cluster_pair_minimum_nets < 1 || options.maximum_pair_nets < 1 {
        return fail("escape plan bounds must be positive");
    }
    let zone_nets = distribution_zone_nets(board);
    let pad_nets = pad_net_map(board);
    let signal: IndexSet<String> = pad_nets.values().filter(|net| !zone_nets.contains(*net)).cloned().collect();
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    let mut clusters = Vec::new();
    for placement in placements {
        if let Some(cluster) = cluster(board, placement, &pad_nets, &signal, options)? {
            clusters.push(cluster);
        }
    }
    let by_component: IndexMap<&str, &ClusterEscape> = clusters.iter().map(|c| (c.component.as_str(), c)).collect();
    // A bundle is the signal nets whose pads all lie on the two components;
    // nets reaching further parts are trees, free to leave the pair elsewhere.
    let mut shared: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for net in &board.nets {
        let mut components: Vec<&str> = net.pads.iter().map(|pad| pad.component.as_str()).collect();
        components.sort_unstable();
        components.dedup();
        if signal.contains(&net.name)
            && components.len() == 2
            && components.iter().all(|c| by_component.contains_key(c))
        {
            let nets = shared.entry((components[0].to_owned(), components[1].to_owned())).or_default();
            if !nets.contains(&net.name) {
                nets.push(net.name.clone());
            }
        }
    }
    let mut pairs = Vec::new();
    for ((first, second), mut nets) in shared {
        if (nets.len() as i64) < options.cluster_pair_minimum_nets {
            continue;
        }
        nets.sort();
        pairs.push(pair(by_component[first.as_str()], by_component[second.as_str()], nets, options));
    }
    Ok(EscapeDiagnostics { clusters, pairs })
}

// --------------------------------------------------------------------------- clusters

/// `(pad, world position, copper layers, placed shape)` of a copper pad.
type PlacedPad<'b> = (&'b FootprintPad, Point, Vec<CopperLayer>, RoundedConvexShape);

fn cluster(
    board: &PhysicalBoard,
    placement: &Placement,
    pad_nets: &IndexMap<PadReference, String>,
    signal: &IndexSet<String>,
    options: &EscapePlanOptions,
) -> Result<Option<ClusterEscape>> {
    let placed = placed_pads(board, placement);
    if placed.is_empty() {
        return Ok(None);
    }
    let hull = hull(&placed.iter().map(|item| item.1).collect::<Vec<_>>());
    let reference =
        |pad: &FootprintPad| PadReference { component: placement.reference.clone(), pad: pad.number.clone() };
    let pins: Vec<ClusterPin> = placed
        .iter()
        .map(|(pad, position, layers, _)| {
            let pad = reference(pad);
            ClusterPin {
                net: pad_nets.get(&pad).cloned(),
                pad,
                position: *position,
                layers: layers.clone(),
                outer: on_boundary(*position, &hull),
            }
        })
        .collect();
    // A surface land wide enough for a via (an exposed pad) leaves through its own via, not the ring.
    let via_lands: Vec<PadReference> = placed
        .iter()
        .filter(|(pad, _, layers, _)| {
            layers.len() == 1 && pad.size.width_nm.min(pad.size.height_nm) >= board.rules.default_via_size_nm
        })
        .map(|(pad, ..)| reference(pad))
        .collect();
    let is_signal = |net: &Option<String>| net.as_ref().is_some_and(|net| signal.contains(net));
    let inner: Vec<PadReference> = pins
        .iter()
        .filter(|pin| !pin.outer && is_signal(&pin.net) && !via_lands.contains(&pin.pad))
        .map(|pin| pin.pad.clone())
        .collect();
    let channels = if inner.is_empty() { Vec::new() } else { channels(board, &placed, &pins, &inner, pad_nets) };
    let nets: IndexSet<&str> =
        pins.iter().filter(|pin| is_signal(&pin.net)).map(|pin| pin.net.as_deref().unwrap()).collect();
    let rules = if nets.is_empty() { channel_rules(board, "", None) } else { net_rules(board, nets) };
    let positions: Vec<Point> = pins.iter().map(|pin| pin.position).collect();
    let fields = pad_fields_of(board, placement, &placed, options)?;
    Ok(Some(ClusterEscape {
        component: placement.reference.clone(),
        bounds: bounds(&positions),
        pins,
        inner_signal_pins: inner,
        channels,
        fields,
        rules,
    }))
}

/// Ring gaps on each layer the inner pins reach, against the inner pins there.
fn channels(
    board: &PhysicalBoard,
    placed: &[PlacedPad<'_>],
    pins: &[ClusterPin],
    inner: &[PadReference],
    pad_nets: &IndexMap<PadReference, String>,
) -> Vec<ChannelLoad> {
    let (width, clearance) = net_rules(board, inner.iter().map(|pad| pad_nets[pad].as_str()));
    let centre = centre(&pins.iter().map(|pin| pin.position).collect::<Vec<_>>());
    let mut ring: Vec<usize> = (0..pins.len()).filter(|index| pins[*index].outer).collect();
    ring.sort_by(|a, b| {
        float_cmp(angle(pins[*a].position, centre, 0.0, 1.0), angle(pins[*b].position, centre, 0.0, 1.0))
            .then_with(|| pins[*a].pad.cmp(&pins[*b].pad))
    });
    let inner_layers: IndexMap<&PadReference, &Vec<CopperLayer>> =
        pins.iter().map(|pin| (&pin.pad, &pin.layers)).collect();
    let mut layers: Vec<CopperLayer> = Vec::new();
    for pad in inner {
        for layer in inner_layers[pad] {
            if !layers.contains(layer) {
                layers.push(*layer);
            }
        }
    }
    layers.sort_by_key(|layer| layer_rank(*layer));
    let mut result = Vec::new();
    for layer in layers {
        let demand = inner.iter().filter(|pad| inner_layers[pad].contains(&layer)).count() as i64;
        let on_layer: Vec<usize> = ring.iter().copied().filter(|index| pins[*index].layers.contains(&layer)).collect();
        let (mut capacity, mut gaps) = (0, 0);
        if on_layer.len() >= 2 {
            for (position, index) in on_layer.iter().enumerate() {
                let following = on_layer[(position + 1) % on_layer.len()];
                if following == *index || (on_layer.len() == 2 && position == 1) {
                    continue;
                }
                let gap = edge_gap(&placed[*index].3, &placed[following].3);
                capacity += 0.max((gap - clearance).div_euclid(width + clearance));
                gaps += 1;
            }
        }
        result.push(ChannelLoad { layer, demand, capacity, gaps });
    }
    result
}

/// The widest escape track and largest escape clearance among `nets` (see [`channel_rules`]).
fn net_rules<'n>(board: &PhysicalBoard, nets: impl IntoIterator<Item = &'n str>) -> (i64, i64) {
    let rules: IndexMap<&str, &NetRoutingRule> =
        board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let mut nets: Vec<&str> = nets.into_iter().collect();
    nets.sort_unstable();
    nets.dedup();
    let values: Vec<(i64, i64)> = nets.iter().map(|net| channel_rules(board, net, rules.get(net).copied())).collect();
    (values.iter().map(|v| v.0).max().expect("nets"), values.iter().map(|v| v.1).max().expect("nets"))
}

/// Track width and clearance of `net` between a pad field's pins.
///
/// A net with breakout properties necks down there, as the routers do inside
/// its terminal regions; any other net uses its own rule, else the board
/// default, never below the board minimums.
pub fn channel_rules(board: &PhysicalBoard, _net: &str, rule: Option<&NetRoutingRule>) -> (i64, i64) {
    let breakout = rule.is_some_and(|r| r.breakout_length_nm.is_some());
    let width = match rule.and_then(|r| r.breakout_width_nm).filter(|_| breakout) {
        Some(width) => width,
        None => board.rules.minimum_track_width_nm.max(match rule.and_then(|r| r.width_nm) {
            Some(width) if width != 0 => width,
            _ => board.rules.default_track_width_nm,
        }),
    };
    let clearance = match rule.and_then(|r| r.breakout_clearance_nm).filter(|_| breakout) {
        Some(clearance) => clearance,
        None => rule.map_or(0, |r| r.clearance_nm.unwrap_or(0)),
    };
    (width, board.rules.minimum_clearance_nm.max(clearance))
}

fn edge_gap(first: &RoundedConvexShape, second: &RoundedConvexShape) -> i64 {
    let squared = shape_distance_squared(first, second);
    // `int(Fraction)` truncates; the squared distance is never negative.
    let whole: BigInt = squared.numer().abs().div_floor(&squared.denom().abs());
    let distance = whole.sqrt().to_i64().expect("board-sized distance");
    0.max(distance - first.radius_nm - second.radius_nm)
}

// --------------------------------------------------------------------------- channel model (E1)

/// The copper gap between two neighbouring pins of a pad field, on the layers both lands occupy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub first: PadReference,
    pub second: PadReference,
    /// "row", "column" or "diagonal" in a lattice; "ring" along any other field's outline.
    pub kind: String,
    pub gap_nm: i64,
    pub layers: Vec<CopperLayer>,
}

impl Gap {
    /// Tracks of `width_nm` side by side at `clearance_nm`: ⌊(gap − s)/(w + s)⌋.
    pub fn capacity(&self, width_nm: i64, clearance_nm: i64) -> i64 {
        0.max((self.gap_nm - clearance_nm).div_euclid(width_nm + clearance_nm))
    }
}

pub const TILE_SIDES: [&str; 4] = ["W", "N", "E", "S"];
pub const TILE_CORNERS: [&str; 4] = ["NW", "NE", "SE", "SW"];
/// Each diagonal of a tile splits its terminals into two halves; a track
/// joining the halves passes between the diagonal's two pins.
const DIAGONAL_HALVES: [([&str; 3], [&str; 3]); 2] =
    [(["N", "E", "NE"], ["W", "S", "SW"]), (["W", "N", "NW"], ["E", "S", "SE"])];

/// The cell between four neighbouring lattice positions (footprint frame:
/// x to the east, y to the south). A missing pin leaves its corner `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub column: i64,
    pub row: i64,
    pub corners: [Option<PadReference>; 4],
    /// W, N, E, S.
    pub sides: [Option<Gap>; 4],
    /// NW–SE, NE–SW.
    pub diagonals: [Option<Gap>; 2],
    pub centre: Point,
    /// The board's default via fits at the centre, clear of every corner land.
    pub via_site: bool,
}

impl Tile {
    /// Whether tracks between the given terminals fit through the tile on `layer`.
    pub fn admits(
        &self,
        flows: &[((&str, &str), i64)],
        layer: CopperLayer,
        width_nm: i64,
        clearance_nm: i64,
    ) -> Result<bool> {
        let terminals: Vec<&str> = TILE_SIDES.iter().chain(TILE_CORNERS.iter()).copied().collect();
        let mut use_: IndexMap<&str, i64> = terminals.iter().map(|t| (*t, 0)).collect();
        for ((first, second), count) in flows {
            if *count < 0 || first == second || !use_.contains_key(first) || !use_.contains_key(second) {
                return Err(Error::Value(format!("invalid tile flow {first}->{second}: {count}")));
            }
            use_[first] += count;
            use_[second] += count;
        }
        for (index, corner) in TILE_CORNERS.iter().enumerate() {
            if use_[corner] > i64::from(self.corners[index].is_some()) {
                return Ok(false);
            }
        }
        for (index, side) in TILE_SIDES.iter().enumerate() {
            if let Some(gap) = &self.sides[index]
                && gap.layers.contains(&layer)
                && use_[side] > gap.capacity(width_nm, clearance_nm)
            {
                return Ok(false);
            }
        }
        for (gap, (one, other)) in self.diagonals.iter().zip(DIAGONAL_HALVES.iter()) {
            let Some(gap) = gap.as_ref().filter(|gap| gap.layers.contains(&layer)) else { continue };
            let crossing: i64 = flows
                .iter()
                .filter(|((first, second), _)| {
                    (one.contains(first) && other.contains(second)) || (other.contains(first) && one.contains(second))
                })
                .map(|(_, count)| count)
                .sum();
            if crossing > gap.capacity(width_nm, clearance_nm) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Pads of one component that escape together, with their channel graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadField {
    pub component: String,
    pub pads: Vec<PadReference>,
    /// "lattice", "row", "perimeter" or "irregular".
    pub kind: String,
    /// Lattice positions per row (0 unless a lattice).
    pub columns: i64,
    pub rows: i64,
    /// Lattice (x, y) pitch in the footprint frame.
    pub pitch_nm: Option<(i64, i64)>,
    pub gaps: Vec<Gap>,
    pub tiles: Vec<Tile>,
}

impl PadField {
    /// Counts and the narrowest gap of each kind, in tracks at the given rules.
    pub fn report(&self, width_nm: i64, clearance_nm: i64) -> Value {
        let mut kinds: Vec<&str> = self.gaps.iter().map(|g| g.kind.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut entries = vec![("pads", int(self.pads.len() as i64)), ("kind", text(&self.kind))];
        if let Some((x, y)) = self.pitch_nm {
            entries.push(("columns", int(self.columns)));
            entries.push(("rows", int(self.rows)));
            entries.push(("pitch_nm", Value::Array(vec![int(x), int(y)])));
        }
        let of_kind = |kind: &str| -> Vec<&Gap> { self.gaps.iter().filter(|g| g.kind == kind).collect() };
        entries.push((
            "gaps",
            Value::Object(kinds.iter().map(|k| ((*k).to_owned(), int(of_kind(k).len() as i64))).collect()),
        ));
        entries.push((
            "minimum_capacity",
            Value::Object(
                kinds
                    .iter()
                    .map(|k| {
                        let minimum = of_kind(k).iter().map(|g| g.capacity(width_nm, clearance_nm)).min().expect("gap");
                        ((*k).to_owned(), int(minimum))
                    })
                    .collect(),
            ),
        ));
        entries.push(("tiles", int(self.tiles.len() as i64)));
        entries.push(("via_sites", int(self.tiles.iter().filter(|t| t.via_site).count() as i64)));
        object(entries)
    }
}

/// `pad_fields`: the pad fields of a placed component, largest first.
pub fn pad_fields(board: &PhysicalBoard, component: &str, options: &EscapePlanOptions) -> Result<Vec<PadField>> {
    let Some(placement) = board.placements.iter().find(|item| item.reference == component) else {
        return Err(Error::Value(format!("unknown component {}", copper_json::python_str_repr(component))));
    };
    pad_fields_of(board, placement, &placed_pads(board, placement), options)
}

/// `_placed_pads`: every copper pad of a placement.
fn placed_pads<'b>(board: &'b PhysicalBoard, placement: &Placement) -> Vec<PlacedPad<'b>> {
    let surface = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    board.footprints[&placement.footprint]
        .pads
        .iter()
        .filter(|pad| !matches!(pad.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture))
        .map(|pad| {
            let position = transformed_local_point(placement, pad.position);
            let layers = if pad.kind == PadKind::Smd { vec![surface] } else { board.stackup.copper_layers.clone() };
            (pad, position, layers, placed_pad_shape(position, pad, placement))
        })
        .collect()
}

fn pad_fields_of(
    board: &PhysicalBoard,
    placement: &Placement,
    placed: &[PlacedPad<'_>],
    options: &EscapePlanOptions,
) -> Result<Vec<PadField>> {
    if options.field_spacing_factor < 1 || options.field_spacing_minimum_nm < 0 {
        return fail("pad field spacing must be positive");
    }
    let groups = spacing_groups(&placed.iter().map(|item| item.0.position).collect::<Vec<_>>(), options);
    Ok(groups
        .into_iter()
        .map(|group| {
            let members: Vec<PlacedPad<'_>> = group.into_iter().map(|index| placed[index].clone()).collect();
            pad_field(board, placement, &members)
        })
        .collect())
}

/// Single-linkage groups of footprint-local pad centres, largest first.
fn spacing_groups(points: &[Point], options: &EscapePlanOptions) -> Vec<Vec<usize>> {
    if points.len() <= 1 {
        // A part with no copper pads has no field.
        return if points.is_empty() { Vec::new() } else { vec![vec![0]] };
    }
    let squared = |a: Point, b: Point| -> i128 {
        let (dx, dy) = ((a.x_nm - b.x_nm) as i128, (a.y_nm - b.y_nm) as i128);
        dx * dx + dy * dy
    };
    let mut nearest: Vec<i128> = (0..points.len())
        .map(|i| (0..points.len()).filter(|j| *j != i).map(|j| squared(points[i], points[j])).min().unwrap())
        .collect();
    nearest.sort_unstable();
    let median_low = nearest[(nearest.len() - 1) / 2];
    let link = (options.field_spacing_factor as i128 * median_low.sqrt()).max(options.field_spacing_minimum_nm as i128);
    let mut parent: Vec<usize> = (0..points.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..points.len() {
        for j in i + 1..points.len() {
            if squared(points[i], points[j]) <= link * link {
                // Python evaluates `parent[root(i)] = root(j)` right side first.
                let b = root(&mut parent, j);
                let a = root(&mut parent, i);
                parent[a] = b;
            }
        }
    }
    let mut groups: IndexMap<usize, Vec<usize>> = IndexMap::new();
    for index in 0..points.len() {
        let key = root(&mut parent, index);
        groups.entry(key).or_default().push(index);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    groups.sort_by_key(|group| (Reverse(group.len()), group[0]));
    groups
}

fn pad_field(board: &PhysicalBoard, placement: &Placement, placed: &[PlacedPad<'_>]) -> PadField {
    let reference = &placement.reference;
    let pads: Vec<PadReference> =
        placed.iter().map(|item| PadReference { component: reference.clone(), pad: item.0.number.clone() }).collect();
    if let Some((xs, ys)) = lattice(&placed.iter().map(|item| item.0.position).collect::<Vec<_>>()) {
        return lattice_field(board, placement, placed, pads, xs, ys);
    }
    let positions: Vec<Point> = placed.iter().map(|item| item.1).collect();
    let hull = hull(&positions);
    let outer: Vec<usize> = (0..placed.len()).filter(|index| on_boundary(placed[*index].1, &hull)).collect();
    let kind = if hull.len() <= 2 {
        "row"
    } else if outer.len() == placed.len() {
        "perimeter"
    } else {
        "irregular"
    };
    let links: Vec<(usize, usize)> = if kind == "row" {
        // Neighbours along the line, which runs between the hull's two ends.
        let start = hull[0];
        let mut ring: Vec<usize> = (0..placed.len()).collect();
        ring.sort_by(|a, b| {
            let reach = |index: usize| {
                let (dx, dy) =
                    ((placed[index].1.x_nm - start.x_nm) as i128, (placed[index].1.y_nm - start.y_nm) as i128);
                dx * dx + dy * dy
            };
            reach(*a).cmp(&reach(*b)).then_with(|| pads[*a].cmp(&pads[*b]))
        });
        ring.windows(2).map(|w| (w[0], w[1])).collect()
    } else {
        let centre = centre(&positions);
        let mut ring = outer;
        ring.sort_by(|a, b| {
            float_cmp(angle(placed[*a].1, centre, 0.0, 1.0), angle(placed[*b].1, centre, 0.0, 1.0))
                .then_with(|| pads[*a].cmp(&pads[*b]))
        });
        (0..ring.len()).map(|position| (ring[position], ring[(position + 1) % ring.len()])).collect()
    };
    let gaps = links.into_iter().map(|(first, second)| gap(placed, &pads, first, second, "ring")).collect();
    PadField {
        component: reference.clone(),
        pads,
        kind: kind.to_owned(),
        columns: 0,
        rows: 0,
        pitch_nm: None,
        gaps,
        tiles: Vec::new(),
    }
}

/// Footprint coordinates within this of a uniform pitch still form a lattice.
const LATTICE_TOLERANCE_NM: i64 = 1_000;

/// Uniform footprint-frame columns and rows holding at least half their positions, else None.
fn lattice(points: &[Point]) -> Option<(Vec<i64>, Vec<i64>)> {
    let mut xs: Vec<i64> = points.iter().map(|p| p.x_nm).collect();
    let mut ys: Vec<i64> = points.iter().map(|p| p.y_nm).collect();
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let mut unique: Vec<(i64, i64)> = points.iter().map(|p| (p.x_nm, p.y_nm)).collect();
    unique.sort_unstable();
    unique.dedup();
    if xs.len() < 2 || ys.len() < 2 || unique.len() != points.len() {
        return None;
    }
    if 2 * points.len() < xs.len() * ys.len() {
        return None;
    }
    for values in [&xs, &ys] {
        let steps: Vec<i64> = values.windows(2).map(|w| w[1] - w[0]).collect();
        if steps.iter().max().unwrap() - steps.iter().min().unwrap() > LATTICE_TOLERANCE_NM {
            return None;
        }
    }
    Some((xs, ys))
}

type Spot = (i64, i64);

fn lattice_field(
    board: &PhysicalBoard,
    placement: &Placement,
    placed: &[PlacedPad<'_>],
    pads: Vec<PadReference>,
    xs: Vec<i64>,
    ys: Vec<i64>,
) -> PadField {
    let index_of = |values: &[i64], value: i64| values.iter().position(|v| *v == value).expect("lattice value") as i64;
    let mut at: BTreeMap<Spot, usize> = BTreeMap::new();
    for (index, item) in placed.iter().enumerate() {
        at.insert((index_of(&xs, item.0.position.x_nm), index_of(&ys, item.0.position.y_nm)), index);
    }
    let mut gaps: BTreeMap<(Spot, Spot), Gap> = BTreeMap::new();
    for (&(column, row), &index) in &at {
        for ((dx, dy), kind) in [((1, 0), "row"), ((0, 1), "column"), ((1, 1), "diagonal"), ((1, -1), "diagonal")] {
            let neighbour = (column + dx, row + dy);
            if let Some(&other) = at.get(&neighbour) {
                gaps.insert(((column, row), neighbour), gap(placed, &pads, index, other, kind));
            }
        }
    }
    let find = |a: Spot, b: Spot| gaps.get(&(a, b)).or_else(|| gaps.get(&(b, a))).cloned();
    let via_radius = board.rules.default_via_size_nm.div_euclid(2);
    let mut tiles = Vec::new();
    for column in 0..xs.len() as i64 - 1 {
        for row in 0..ys.len() as i64 - 1 {
            let (nw, ne, se, sw) = ((column, row), (column + 1, row), (column + 1, row + 1), (column, row + 1));
            let corners = [nw, ne, se, sw].map(|spot| at.get(&spot).map(|index| pads[*index].clone()));
            let (c, r) = (column as usize, row as usize);
            let centre = transformed_local_point(
                placement,
                Point::new((xs[c] + xs[c + 1]).div_euclid(2), (ys[r] + ys[r + 1]).div_euclid(2)),
            );
            let point = RoundedConvexShape::new(vec![centre], 0).expect("point shape");
            let via_site = [nw, ne, se, sw]
                .iter()
                .filter_map(|spot| at.get(spot))
                .all(|index| edge_gap(&point, &placed[*index].3) >= via_radius + board.rules.minimum_clearance_nm);
            tiles.push(Tile {
                column,
                row,
                corners,
                sides: [find(nw, sw), find(nw, ne), find(ne, se), find(sw, se)],
                diagonals: [find(nw, se), find(ne, sw)],
                centre,
                via_site,
            });
        }
    }
    PadField {
        component: placement.reference.clone(),
        pads,
        kind: "lattice".to_owned(),
        columns: xs.len() as i64,
        rows: ys.len() as i64,
        pitch_nm: Some((xs[1] - xs[0], ys[1] - ys[0])),
        gaps: gaps.into_values().collect(),
        tiles,
    }
}

fn gap(placed: &[PlacedPad<'_>], pads: &[PadReference], first: usize, second: usize, kind: &str) -> Gap {
    let layers = placed[first].2.iter().copied().filter(|layer| placed[second].2.contains(layer)).collect();
    Gap {
        first: pads[first].clone(),
        second: pads[second].clone(),
        kind: kind.to_owned(),
        gap_nm: edge_gap(&placed[first].3, &placed[second].3),
        layers,
    }
}

// --------------------------------------------------------------------------- pairs

fn pair(
    first: &ClusterEscape,
    second: &ClusterEscape,
    nets: Vec<String>,
    options: &EscapePlanOptions,
) -> ClusterPairEscape {
    if nets.len() as i64 > options.maximum_pair_nets {
        return ClusterPairEscape {
            first: first.component.clone(),
            second: second.component.clone(),
            nets,
            crossing_pairs: 0,
            crossing_nets: Vec::new(),
            layer_moves: Vec::new(),
            analysed: false,
            order: Vec::new(),
            second_keys: Vec::new(),
        };
    }
    let first_centre = centre(&first.pins.iter().map(|pin| pin.position).collect::<Vec<_>>());
    let second_centre = centre(&second.pins.iter().map(|pin| pin.position).collect::<Vec<_>>());
    // Each boundary is cut on its far side; the second is read in the opposite
    // sense, so a crossing-free bundle has the same order at both ends.
    let first_order = boundary_order(first, &nets, first_centre, second_centre, 1.0);
    let second_order = boundary_order(second, &nets, second_centre, first_centre, -1.0);
    let mut ranked: Vec<&String> = nets.iter().collect();
    ranked.sort_by(|a, b| {
        place_cmp(&first_order[*a], &first_order[*b])
            .then_with(|| place_cmp(&second_order[*a], &second_order[*b]))
            .then_with(|| a.cmp(b))
    });
    let mut crossing_nets: Vec<String> = Vec::new();
    let mut crossings = 0;
    for (index, net) in ranked.iter().enumerate() {
        for other in &ranked[index + 1..] {
            if place_cmp(&first_order[*net], &first_order[*other]) == Ordering::Less
                && place_cmp(&second_order[*net], &second_order[*other]) == Ordering::Greater
            {
                crossings += 1;
                for name in [*net, *other] {
                    if !crossing_nets.contains(name) {
                        crossing_nets.push(name.clone());
                    }
                }
            }
        }
    }
    crossing_nets.sort();
    let ranked: Vec<String> = ranked.into_iter().cloned().collect();
    let kept = longest_chain(&ranked, &second_order);
    let layer_moves = nets.iter().filter(|net| !kept.contains(*net)).cloned().collect();
    let second_keys = ranked.iter().map(|net| second_order[net].clone()).collect();
    ClusterPairEscape {
        first: first.component.clone(),
        second: second.component.clone(),
        nets,
        crossing_pairs: crossings,
        crossing_nets,
        layer_moves,
        analysed: true,
        order: ranked,
        second_keys,
    }
}

/// A single row is seen from this far behind its centre.
const BEHIND_NM: f64 = 1_000_000.0;

/// Each net's place along the cluster boundary, starting on the side away from `far`.
///
/// Seen from the centre, angle order is perimeter order. A single row is seen
/// from just behind its centre. A net with several pins in the cluster is
/// placed at the pin nearest `far`.
fn boundary_order(
    cluster: &ClusterEscape,
    nets: &[String],
    centre: (f64, f64),
    far: (f64, f64),
    sense: f64,
) -> IndexMap<String, BoundaryPlace> {
    let away = (centre.0 - far.0, centre.1 - far.1);
    let length = (away.0.powi(2) + away.1.powi(2)).powf(0.5);
    let length = if length == 0.0 { 1.0 } else { length };
    let away = (away.0 / length, away.1 / length);
    let mut view = centre;
    if hull(&cluster.pins.iter().map(|pin| pin.position).collect::<Vec<_>>()).len() <= 2 {
        view = (centre.0 + away.0 * BEHIND_NM, centre.1 + away.1 * BEHIND_NM);
    }
    let cut = away.1.atan2(away.0);
    let mut pins: Vec<&ClusterPin> = cluster.pins.iter().collect();
    pins.sort_by(|a, b| a.pad.cmp(&b.pad));
    let mut order: IndexMap<String, (f64, BoundaryPlace)> = IndexMap::new();
    for pin in pins {
        let Some(net) = pin.net.as_ref().filter(|net| nets.contains(net)) else { continue };
        let reach = (pin.position.x_nm as f64 - view.0).powi(2) + (pin.position.y_nm as f64 - view.1).powi(2);
        let place = (angle(pin.position, view, cut, sense), reach, pin.pad.clone());
        let distance = (pin.position.x_nm as f64 - far.0).powi(2) + (pin.position.y_nm as f64 - far.1).powi(2);
        match order.get_mut(net) {
            Some(best) if distance < best.0 => *best = (distance, place),
            Some(_) => {}
            None => {
                order.insert(net.clone(), (distance, place));
            }
        }
    }
    order.into_iter().map(|(net, (_, place))| (net, place)).collect()
}

/// Nets of one longest subsequence of `ranked` that also increases in `second_order`.
fn longest_chain(ranked: &[String], second_order: &IndexMap<String, BoundaryPlace>) -> Vec<String> {
    let mut tails: Vec<&BoundaryPlace> = Vec::new();
    let mut tail_index: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = Vec::new();
    for (index, net) in ranked.iter().enumerate() {
        let value = &second_order[net];
        let slot = tails.partition_point(|tail| place_cmp(tail, value) != Ordering::Greater);
        if slot == tails.len() {
            tails.push(value);
            tail_index.push(index);
        } else {
            tails[slot] = value;
            tail_index[slot] = index;
        }
        previous.push(if slot > 0 { Some(tail_index[slot - 1]) } else { None });
    }
    let mut kept = Vec::new();
    let mut index = tail_index.last().copied();
    while let Some(current) = index {
        kept.push(ranked[current].clone());
        index = previous[current];
    }
    kept
}

// --------------------------------------------------------------------------- two-end plan (E3)

/// Layer assignment and route order of one crossing bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPlan {
    pub first: String,
    pub second: String,
    pub planar_layer: CopperLayer,
    /// On the planar layer, in boundary order.
    pub kept: Vec<String>,
    /// To another layer through a via.
    pub moved: Vec<(String, CopperLayer)>,
    /// Outside the planar subset, but with no other legal layer.
    pub unmoved: Vec<String>,
    pub corridor: Bounds,
}

impl PairPlan {
    pub fn report(&self) -> Value {
        let c = &self.corridor;
        object(vec![
            ("components", strings([self.first.as_str(), self.second.as_str()])),
            ("planar_layer", text(self.planar_layer.value())),
            ("kept", strings(self.kept.iter().map(String::as_str))),
            (
                "moved",
                Value::Object(self.moved.iter().map(|(net, layer)| (net.clone(), text(layer.value()))).collect()),
            ),
            ("unmoved", strings(self.unmoved.iter().map(String::as_str))),
            ("corridor_nm", Value::Array(vec![int(c.min_x), int(c.min_y), int(c.max_x), int(c.max_y)])),
        ])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EscapePlan {
    pub pairs: Vec<PairPlan>,
    /// Nets to route first, in this order: each bundle's moved nets, then its kept nets outermost first.
    pub net_order: Vec<String>,
    pub layer_biases: Vec<LayerBias>,
    /// Surface pins of kept nets: they escape on their surface, without a dogbone via.
    pub surface_pads: IndexSet<PadReference>,
}

impl EscapePlan {
    pub fn report(&self) -> Value {
        let mut surface: Vec<String> = self.surface_pads.iter().map(pad_text).collect();
        surface.sort();
        object(vec![
            ("pairs", Value::Array(self.pairs.iter().map(PairPlan::report).collect())),
            ("net_order", strings(self.net_order.iter().map(String::as_str))),
            ("surface_pads", strings(surface.iter().map(String::as_str))),
        ])
    }
}

/// `plan_two_end_escapes`: planar layer assignment and route order for every
/// crossing bundle (Ozdal & Wong). See the Python docstring for the policy.
pub fn plan_two_end_escapes(
    board: &PhysicalBoard,
    options: &EscapePlanOptions,
    diagnostics: Option<&EscapeDiagnostics>,
) -> Result<EscapePlan> {
    if options.corridor_margin_nm < 0 || options.layer_bias_rate < 0 {
        return fail("escape plan corridor margin and layer bias must not be negative");
    }
    let owned;
    let diagnostics = match diagnostics {
        Some(diagnostics) => diagnostics,
        None => {
            owned = escape_diagnostics(board, options)?;
            &owned
        }
    };
    let clusters: IndexMap<&str, &ClusterEscape> =
        diagnostics.clusters.iter().map(|c| (c.component.as_str(), c)).collect();
    let rules: IndexMap<&str, &NetRoutingRule> =
        board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let stack = &board.stackup.copper_layers;
    let position = |layer: CopperLayer| stack.iter().position(|l| *l == layer).expect("stackup layer") as i64;
    let (mut pairs, mut order, mut biases) = (Vec::new(), Vec::<String>::new(), Vec::new());
    let mut surface_pads: IndexSet<PadReference> = IndexSet::new();
    let zone_nets = distribution_zone_nets(board);
    for crossing in &diagnostics.pairs {
        if !crossing.analysed || crossing.crossing_pairs == 0 {
            continue;
        }
        // Every signal net joining the two parts, rails and other trees included.
        let net_set = |component: &str| -> IndexSet<String> {
            clusters[component].pins.iter().filter_map(|pin| pin.net.clone()).collect()
        };
        let second_nets = net_set(&crossing.second);
        let mut nets: Vec<String> = net_set(&crossing.first)
            .into_iter()
            .filter(|net| second_nets.contains(net) && !zone_nets.contains(net))
            .collect();
        nets.sort();
        let pair = pair(clusters[crossing.first.as_str()], clusters[crossing.second.as_str()], nets, options);
        if !pair.analysed {
            continue;
        }
        let pins: Vec<&ClusterPin> = [&pair.first, &pair.second]
            .iter()
            .flat_map(|component| clusters[component.as_str()].pins.iter())
            .filter(|pin| pin.net.as_ref().is_some_and(|net| pair.nets.contains(net)))
            .collect();
        let allowed: IndexMap<&str, Vec<CopperLayer>> = pair
            .nets
            .iter()
            .map(|net| (net.as_str(), routing_layers(board, net, rules.get(net.as_str()).copied())))
            .collect();
        let mut surfaces: Vec<CopperLayer> = Vec::new();
        for pin in pins.iter().filter(|pin| pin.layers.len() == 1) {
            if !surfaces.contains(&pin.layers[0]) {
                surfaces.push(pin.layers[0]);
            }
        }
        let shared: Vec<CopperLayer> =
            stack.iter().copied().filter(|layer| allowed.values().all(|layers| layers.contains(layer))).collect();
        if surfaces.len() > 1 || shared.is_empty() || surfaces.first().is_some_and(|s| !shared.contains(s)) {
            continue; // bundle pins on both outer layers, or no common layer: nothing planar to keep
        }
        let planar = surfaces.first().copied().unwrap_or(shared[0]);
        let kept: Vec<String> = pair.order.iter().filter(|net| !pair.layer_moves.contains(net)).cloned().collect();
        let (mut moved, mut unmoved): (Vec<(String, CopperLayer)>, Vec<String>) = (Vec::new(), Vec::new());
        if options.bus_layers {
            (moved, unmoved) = layered_moves(&pair, &kept, planar, &allowed, &rules, &pins, stack);
        } else {
            for net in pair.order.iter().filter(|net| pair.layer_moves.contains(net)) {
                let others: Vec<CopperLayer> =
                    allowed[net.as_str()].iter().copied().filter(|layer| *layer != planar).collect();
                match others.iter().min_by_key(|layer| (position(**layer) - position(planar)).abs()) {
                    Some(layer) => moved.push((net.clone(), *layer)),
                    None => unmoved.push(net.clone()),
                }
            }
        }
        let corridor =
            bounds(&pins.iter().map(|pin| pin.position).collect::<Vec<_>>()).expanded(options.corridor_margin_nm);
        let mut moved_layers: Vec<CopperLayer> = Vec::new();
        for (_, layer) in &moved {
            if !moved_layers.contains(layer) {
                moved_layers.push(*layer);
            }
        }
        let mut sequence: Vec<String> = Vec::new();
        for layer in &moved_layers {
            let group: Vec<String> = moved.iter().filter(|(_, l)| l == layer).map(|(n, _)| n.clone()).collect();
            sequence.extend(outside_in(&group));
        }
        sequence.extend(outside_in(&kept));
        sequence.extend(unmoved.iter().cloned());
        for net in sequence {
            if !order.contains(&net) {
                order.push(net);
            }
        }
        let mut sorted_moves = moved.clone();
        sorted_moves.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.value().cmp(b.1.value())));
        for (net, layer) in &sorted_moves {
            for other in allowed[net.as_str()].iter().filter(|other| *other != layer) {
                biases.push(LayerBias {
                    net: net.clone(),
                    layer: *other,
                    area: corridor,
                    rate: options.layer_bias_rate,
                });
            }
        }
        // Only nets running purely between the two parts escape on the surface;
        // a rail or other tree keeps its dogbone towards its further pads.
        for pin in &pins {
            let net = pin.net.as_ref().expect("bundle pin net");
            if kept.contains(net) && crossing.nets.contains(net) && pin.layers.as_slice() == [planar] {
                surface_pads.insert(pin.pad.clone());
            }
        }
        pairs.push(PairPlan {
            first: pair.first.clone(),
            second: pair.second.clone(),
            planar_layer: planar,
            kept,
            moved,
            unmoved,
            corridor,
        });
    }
    Ok(EscapePlan { pairs, net_order: order, layer_biases: biases, surface_pads })
}

/// A bundle's nets from both outer edges towards the middle.
fn outside_in(nets: &[String]) -> Vec<String> {
    (0..nets.len())
        .map(|index| if index % 2 == 0 { nets[index / 2].clone() } else { nets[nets.len() - 1 - index / 2].clone() })
        .collect()
}

/// Moved nets over successive layers, each a largest non-crossing subset.
fn layered_moves(
    pair: &ClusterPairEscape,
    kept: &[String],
    planar: CopperLayer,
    allowed: &IndexMap<&str, Vec<CopperLayer>>,
    rules: &IndexMap<&str, &NetRoutingRule>,
    pins: &[&ClusterPin],
    stack: &[CopperLayer],
) -> (Vec<(String, CopperLayer)>, Vec<String>) {
    let second: IndexMap<String, BoundaryPlace> =
        pair.order.iter().cloned().zip(pair.second_keys.iter().cloned()).collect();
    let mut remaining: Vec<String> = pair.order.iter().filter(|net| !kept.contains(net)).cloned().collect();
    let surface_ends: IndexMap<String, Vec<CopperLayer>> = remaining
        .iter()
        .map(|net| {
            let ends = pins
                .iter()
                .filter(|pin| pin.net.as_ref() == Some(net) && pin.layers.len() == 1)
                .map(|pin| pin.layers[0])
                .collect();
            (net.clone(), ends)
        })
        .collect();
    let fits = |net: &str, layer: CopperLayer| -> bool {
        let changes = surface_ends[net].iter().filter(|end| **end != layer).count() as i64;
        allowed[net].contains(&layer)
            && rules.get(net).is_none_or(|rule| rule.max_vias.is_none_or(|max| changes <= max))
    };
    let position = |layer: CopperLayer| stack.iter().position(|l| *l == layer).expect("stackup layer") as i64;
    let mut layers: Vec<CopperLayer> = stack.iter().copied().filter(|layer| *layer != planar).collect();
    layers.sort_by_key(|layer| ((position(*layer) - position(planar)).abs(), position(*layer)));
    let mut moved = Vec::new();
    for layer in layers {
        let candidates: Vec<String> = remaining.iter().filter(|net| fits(net, layer)).cloned().collect();
        let chain = longest_chain(&candidates, &second);
        moved.extend(candidates.iter().filter(|net| chain.contains(net)).map(|net| (net.clone(), layer)));
        remaining.retain(|net| !chain.contains(net));
    }
    (moved, remaining)
}

// --------------------------------------------------------------------------- array escape (E2)

/// Which signal balls of a surface ball grid escape on its surface layer (min-cost max-flow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayEscape {
    pub component: String,
    pub layer: CopperLayer,
    pub surface: Vec<PadReference>,
    pub vias: Vec<PadReference>,
    /// Planned surface tracks through each tile ((column, row) in the footprint frame).
    pub tile_load: BTreeMap<(i64, i64), i64>,
}

impl ArrayEscape {
    pub fn report(&self) -> Value {
        object(vec![
            ("component", text(&self.component)),
            ("layer", text(self.layer.value())),
            ("surface", int(self.surface.len() as i64)),
            ("vias", int(self.vias.len() as i64)),
            ("loaded_tiles", int(self.tile_load.values().filter(|load| **load != 0).count() as i64)),
        ])
    }
}

/// A side or diagonal through a missing pin is unbounded; this many tracks stands in for it.
const OPEN_CAPACITY: i64 = 1_000;

/// `plan_array_escapes`: surface escapes of every surface-mount ball grid, by
/// integer min-cost max-flow (Yan & Wong).
pub fn plan_array_escapes(board: &PhysicalBoard, options: &EscapePlanOptions) -> Result<Vec<ArrayEscape>> {
    let zone_nets = distribution_zone_nets(board);
    let pad_nets = pad_net_map(board);
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    let mut result = Vec::new();
    for placement in placements {
        let placed = placed_pads(board, placement);
        for field in pad_fields_of(board, placement, &placed, options)? {
            if field.kind != "lattice" || field.tiles.is_empty() {
                continue;
            }
            let mut layers: Vec<&Vec<CopperLayer>> = Vec::new();
            for item in &placed {
                let pad = PadReference { component: placement.reference.clone(), pad: item.0.number.clone() };
                if field.pads.contains(&pad) && !layers.contains(&&item.2) {
                    layers.push(&item.2);
                }
            }
            if layers.len() != 1 || layers[0].len() != 1 {
                continue; // through-hole or mixed fields escape on every layer
            }
            let layer = layers[0][0];
            let signal: Vec<PadReference> = field
                .pads
                .iter()
                .filter(|pad| pad_nets.get(*pad).is_some_and(|net| !zone_nets.contains(net)))
                .cloned()
                .collect();
            if signal.is_empty() {
                continue;
            }
            let (width, clearance) = net_rules(board, signal.iter().map(|pad| pad_nets[pad].as_str()));
            result.push(array_flow(&placement.reference, &field, layer, &signal, width, clearance));
        }
    }
    Ok(result)
}

fn array_flow(
    component: &str,
    field: &PadField,
    layer: CopperLayer,
    signal: &[PadReference],
    width: i64,
    clearance: i64,
) -> ArrayEscape {
    let capacity = |gap: &Option<Gap>| match gap {
        Some(gap) if gap.layers.contains(&layer) => gap.capacity(width, clearance),
        _ => OPEN_CAPACITY,
    };
    let tiles: BTreeMap<(i64, i64), &Tile> = field.tiles.iter().map(|t| ((t.column, t.row), t)).collect();
    let (columns, rows) = (field.columns, field.rows);
    let mut balls: IndexMap<PadReference, (i64, i64)> = IndexMap::new();
    for (&(column, row), tile) in &tiles {
        for (corner, (dx, dy)) in tile.corners.iter().zip([(0, 0), (1, 0), (1, 1), (0, 1)]) {
            if let Some(corner) = corner {
                balls.insert(corner.clone(), (column + dx, row + dy));
            }
        }
    }
    let mut ordered_balls: Vec<&PadReference> = balls.keys().filter(|pad| signal.contains(pad)).collect();
    ordered_balls.sort();
    let ordered_tiles: Vec<(i64, i64)> = tiles.keys().copied().collect();
    let (source, sink) = (0usize, 1usize);
    let ball_node: IndexMap<&PadReference, usize> =
        ordered_balls.iter().enumerate().map(|(index, pad)| (*pad, 2 + index)).collect();
    let tile_in: BTreeMap<(i64, i64), usize> =
        ordered_tiles.iter().enumerate().map(|(index, key)| (*key, 2 + ordered_balls.len() + 2 * index)).collect();
    let mut flow = MinCostFlow::new(2 + ordered_balls.len() + 2 * ordered_tiles.len());
    for pad in &ordered_balls {
        let (column, row) = balls[*pad];
        let node = ball_node[pad];
        flow.add(source, node, 1, 0);
        if column == 0 || column == columns - 1 || row == 0 || row == rows - 1 {
            flow.add(node, sink, 1, 0);
        }
        for key in [(column - 1, row - 1), (column, row - 1), (column - 1, row), (column, row)] {
            if let Some(tile) = tile_in.get(&key) {
                flow.add(node, *tile, 1, 0);
            }
        }
    }
    let mut through: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for key in &ordered_tiles {
        let tile = tiles[key];
        let diagonal = tile.diagonals.iter().map(capacity).min().expect("two diagonals");
        through.insert(*key, flow.add(tile_in[key], tile_in[key] + 1, diagonal, 0));
        let (column, row) = *key;
        // W, N, E, S: the neighbouring tile across each side, or the outside of the field.
        for (side, neighbour) in
            tile.sides.iter().zip([(column - 1, row), (column, row - 1), (column + 1, row), (column, row + 1)])
        {
            let target = tile_in.get(&neighbour).copied().unwrap_or(sink);
            flow.add(tile_in[key] + 1, target, capacity(side), 1);
        }
    }
    flow.run(source, sink);
    let surface: Vec<PadReference> =
        ordered_balls.iter().filter(|pad| flow.used(source, ball_node[**pad])).map(|pad| (*pad).clone()).collect();
    let vias = ordered_balls.iter().filter(|pad| !surface.contains(pad)).map(|pad| (*pad).clone()).collect();
    let tile_load = ordered_tiles.iter().map(|key| (*key, flow.edge_flow(tile_in[key], through[key]))).collect();
    ArrayEscape { component: component.to_owned(), layer, surface, vias, tile_load }
}

/// An edge: (end, residual, cost, reverse index, capacity).
type FlowEdge = [i64; 5];

/// Integer min-cost max-flow by successive shortest paths (Dijkstra with
/// potentials). Deterministic: edges are kept in insertion order and ties go
/// to the lower node.
struct MinCostFlow {
    graph: Vec<Vec<FlowEdge>>,
}

impl MinCostFlow {
    fn new(nodes: usize) -> Self {
        MinCostFlow { graph: vec![Vec::new(); nodes] }
    }

    /// Add an edge; returns its index in `start`'s list.
    fn add(&mut self, start: usize, end: usize, capacity: i64, cost: i64) -> usize {
        let reverse = self.graph[end].len() as i64;
        self.graph[start].push([end as i64, capacity, cost, reverse, capacity]);
        let forward = self.graph[start].len() as i64 - 1;
        self.graph[end].push([start as i64, 0, -cost, forward, 0]);
        self.graph[start].len() - 1
    }

    fn edge_flow(&self, start: usize, index: usize) -> i64 {
        let edge = self.graph[start][index];
        edge[4] - edge[1]
    }

    fn used(&self, start: usize, end: usize) -> bool {
        self.graph[start].iter().any(|edge| edge[0] == end as i64 && edge[4] != 0 && edge[1] == 0)
    }

    fn run(&mut self, source: usize, sink: usize) -> (i64, i64) {
        let size = self.graph.len();
        let mut potential = vec![0i64; size];
        let (mut total_flow, mut total_cost) = (0, 0);
        loop {
            let mut distance: Vec<Option<i64>> = vec![None; size];
            let mut parent: Vec<Option<(usize, usize)>> = vec![None; size];
            distance[source] = Some(0);
            let mut heap = BinaryHeap::from([Reverse((0i64, source))]);
            while let Some(Reverse((cost, node))) = heap.pop() {
                if cost > distance[node].expect("reached") {
                    continue;
                }
                for (index, &[end, residual, edge_cost, _, _]) in self.graph[node].iter().enumerate() {
                    if residual <= 0 {
                        continue;
                    }
                    let end = end as usize;
                    let candidate = cost + edge_cost + potential[node] - potential[end];
                    if distance[end].is_none_or(|current| candidate < current) {
                        distance[end] = Some(candidate);
                        parent[end] = Some((node, index));
                        heap.push(Reverse((candidate, end)));
                    }
                }
            }
            if distance[sink].is_none() {
                return (total_flow, total_cost);
            }
            for node in 0..size {
                if let Some(d) = distance[node] {
                    potential[node] += d;
                }
            }
            let mut push: Option<i64> = None;
            let mut node = sink;
            while node != source {
                let (previous, index) = parent[node].expect("path");
                let residual = self.graph[previous][index][1];
                push = Some(push.map_or(residual, |p| p.min(residual)));
                node = previous;
            }
            let push = push.expect("augmenting path");
            node = sink;
            while node != source {
                let (previous, index) = parent[node].expect("path");
                let edge = self.graph[previous][index];
                self.graph[previous][index][1] -= push;
                self.graph[node][edge[3] as usize][1] += push;
                total_cost += push * edge[2];
                node = previous;
            }
            total_flow += push;
        }
    }
}

// --------------------------------------------------------------------------- geometry

/// Strict convex hull, counter-clockwise in board coordinates.
fn hull(points: &[Point]) -> Vec<Point> {
    let mut unique: Vec<Point> = points.to_vec();
    unique.sort_by_key(|p| (p.x_nm, p.y_nm));
    unique.dedup();
    if unique.len() <= 2 {
        return unique;
    }
    let half = |sequence: &mut dyn Iterator<Item = Point>| -> Vec<Point> {
        let mut chain: Vec<Point> = Vec::new();
        for point in sequence {
            while chain.len() >= 2 && orientation(chain[chain.len() - 2], chain[chain.len() - 1], point) <= 0 {
                chain.pop();
            }
            chain.push(point);
        }
        chain.pop();
        chain
    };
    let mut result = half(&mut unique.iter().copied());
    result.extend(half(&mut unique.iter().rev().copied()));
    result
}

fn on_boundary(point: Point, hull: &[Point]) -> bool {
    if hull.len() <= 2 {
        return true;
    }
    (0..hull.len()).any(|index| point_on_segment(point, hull[index], hull[(index + 1) % hull.len()]))
}

/// Mean pin position: inside the pins' convex hull.
fn centre(points: &[Point]) -> (f64, f64) {
    let count = points.len() as i128;
    (
        true_div(points.iter().map(|p| p.x_nm as i128).sum(), count),
        true_div(points.iter().map(|p| p.y_nm as i128).sum(), count),
    )
}

/// Python's `round(value, 9)`: the correctly rounded decimal, read back.
fn round9(value: f64) -> f64 {
    format!("{value:.9}").parse().expect("formatted float")
}

/// Angle of `point` around `centre` from `cut`, in [0, 2π), increasing in `sense`.
fn angle(point: Point, centre: (f64, f64), cut: f64, sense: f64) -> f64 {
    let tau = 2.0 * std::f64::consts::PI;
    let value = sense * ((point.y_nm as f64 - centre.1).atan2(point.x_nm as f64 - centre.0) - cut);
    // Rounded so pins in one column of a rotated part keep a stable order.
    float_mod(round9(float_mod(value, tau)), round9(tau))
}

fn layer_rank(layer: CopperLayer) -> i64 {
    match layer {
        CopperLayer::Front => 0,
        CopperLayer::Back => 99,
        other => {
            let value = other.value();
            value[2..value.len() - 3].parse().expect("inner layer number")
        }
    }
}

//! Deterministic negotiated-congestion global routing (`pcbir.routing`).
//!
//! Global routes are capacity reservations and guides, not exact copper. The
//! detailed routers are the only stages that may turn these objects into
//! physical `TrackSegment` and `Via` geometry.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

use indexmap::{IndexMap, IndexSet};
use num_rational::BigRational;
use sha2::{Digest, Sha256};

use copper_drc::breakout::BreakoutRegions;
use copper_drc::connectivity::{Candidate, Connectivity, explicit_copper_connectivity};
use copper_drc::digest::reprs;
use copper_drc::legality::{point_in_material, shape_in_board};
use copper_json::{Dumps, Value as Json};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, RouteKind, TrackSegment, Via};
use copper_pcb::hard_macros::{
    macro_port_layers, macro_reservations, macro_routing_pads, macro_routing_ports, macro_source,
};
use copper_pcb::keepouts::resolved_copper_keepouts;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point, transformed_pad_position};
use copper_pcb::zones::{ZoneRoutingReservations, deferred_zone_nets, routing_layers};
use copper_pcb::{Error, Result};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{point_in_polygon, point_segment_distance_squared};
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::global_keepouts::GlobalTrackKeepout;
use crate::global_reserved_access::{admit_fixed_prefix, reserved_pin_access_candidates};
use crate::global_terminals::{register_terminal_capacity, terminal_access_failures};
use crate::pin_access::{LocalAccessBounds, local_access_path};
use crate::pin_escape::Anchor;
use crate::progress::{ProgressCallback, emit, int, text};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_costs::{COST_UNIT, length_cost, preference_cost};
use crate::routing_layers::signal_layer_preferences;
use crate::routing_vias::physical_via_span;

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalRoutingStatus {
    Success,
    Overflow,
    Unreachable,
}

impl GlobalRoutingStatus {
    pub fn value(self) -> &'static str {
        match self {
            GlobalRoutingStatus::Success => "success",
            GlobalRoutingStatus::Overflow => "overflow",
            GlobalRoutingStatus::Unreachable => "unreachable",
        }
    }
}

/// `GridNode`, ordered by layer, then x, then y.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridNode {
    pub layer_index: i64,
    pub x_index: i64,
    pub y_index: i64,
}

impl GridNode {
    pub const fn new(layer_index: i64, x_index: i64, y_index: i64) -> Self {
        GridNode { layer_index, x_index, y_index }
    }
}

/// `PinAccess`: one checked physical entrance of a pad into the capacity graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinAccess {
    pub pad: PadReference,
    pub pad_position: Point,
    pub node: GridNode,
    pub access_position: Point,
    pub layer: CopperLayer,
    pub tracks: Vec<TrackSegment>,
    pub via: Option<Via>,
    pub region_only: bool,
    pub retained_via: Option<Via>,
}

impl PinAccess {
    pub fn new(
        pad: PadReference,
        pad_position: Point,
        node: GridNode,
        access_position: Point,
        layer: CopperLayer,
    ) -> Self {
        PinAccess {
            pad,
            pad_position,
            node,
            access_position,
            layer,
            tracks: Vec::new(),
            via: None,
            region_only: false,
            retained_via: None,
        }
    }

    pub fn estimated_cost_nm(&self) -> i64 {
        let tracks: i64 = self
            .tracks
            .iter()
            .map(|item| (item.start.x_nm - item.end.x_nm).abs() + (item.start.y_nm - item.end.y_nm).abs())
            .sum();
        tracks
            + if self.via.is_some() { crate::nm_from_mm(2) } else { 0 }
            + if self.region_only { crate::nm_from_mm(3) } else { 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRouteSegment {
    pub net: String,
    pub layer: CopperLayer,
    pub start: Point,
    pub end: Point,
    pub guide_half_width_nm: i64,
    pub resource_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalViaProposal {
    pub net: String,
    pub position: Point,
    pub from_layer: CopperLayer,
    pub to_layer: CopperLayer,
    pub resource_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalNetRoute {
    pub net: String,
    pub connected: bool,
    pub accesses: Vec<PinAccess>,
    pub segments: Vec<GlobalRouteSegment>,
    pub vias: Vec<GlobalViaProposal>,
    pub length_nm: i64,
    pub diagnostics: Vec<String>,
    pub deferred_to_zone: bool,
}

impl GlobalNetRoute {
    fn failed(net: &str, accesses: Vec<PinAccess>, diagnostic: String) -> Self {
        GlobalNetRoute {
            net: net.to_owned(),
            connected: false,
            accesses,
            segments: Vec::new(),
            vias: Vec::new(),
            length_nm: 0,
            diagnostics: vec![diagnostic],
            deferred_to_zone: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingHotspot {
    pub resource_id: String,
    pub layer: Option<CopperLayer>,
    pub start: Point,
    pub end: Point,
    pub capacity: i64,
    pub usage: i64,
    pub overflow: i64,
    pub contributors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRoutingMetrics {
    pub routed_net_count: i64,
    pub unrouted_net_count: i64,
    pub total_overflow: i64,
    pub maximum_overflow: i64,
    pub overfull_resource_count: i64,
    pub maximum_utilization_ppm: i64,
    pub total_length_nm: i64,
    pub proposed_via_count: i64,
    pub iterations: i64,
    pub region_only_access_count: i64,
    pub deferred_net_count: i64,
}

impl GlobalRoutingMetrics {
    pub fn quality_vector(&self) -> [i64; 7] {
        [
            self.unrouted_net_count,
            self.total_overflow,
            self.maximum_overflow,
            self.overfull_resource_count,
            self.region_only_access_count,
            self.proposed_via_count,
            self.total_length_nm,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRoutingResult {
    pub status: GlobalRoutingStatus,
    pub routes: Vec<GlobalNetRoute>,
    pub hotspots: Vec<RoutingHotspot>,
    pub metrics: GlobalRoutingMetrics,
    pub placement_fingerprint: String,
    pub routing_fingerprint: String,
    pub fixed_prefix_digest: Option<String>,
}

fn j_point(point: Point) -> Json {
    Json::Array(vec![int(point.x_nm), int(point.y_nm)])
}

fn j_object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

impl GlobalRoutingResult {
    /// `to_json`: the `copperscript-global-route/v0.1` document.
    pub fn to_json(&self) -> String {
        let metrics = &self.metrics;
        let mut document = vec![
            ("schema", text("copperscript-global-route/v0.1")),
            ("status", text(self.status.value())),
            ("placement_fingerprint", text(&self.placement_fingerprint)),
            ("routing_fingerprint", text(&self.routing_fingerprint)),
        ];
        if let Some(digest) = self.fixed_prefix_digest.as_deref().filter(|d| !d.is_empty()) {
            document.push(("fixed_prefix_digest", text(digest)));
        }
        document.push((
            "metrics",
            j_object(vec![
                ("routed_net_count", int(metrics.routed_net_count)),
                ("unrouted_net_count", int(metrics.unrouted_net_count)),
                ("total_overflow", int(metrics.total_overflow)),
                ("maximum_overflow", int(metrics.maximum_overflow)),
                ("overfull_resource_count", int(metrics.overfull_resource_count)),
                ("maximum_utilization_ppm", int(metrics.maximum_utilization_ppm)),
                ("total_length_nm", int(metrics.total_length_nm)),
                ("proposed_via_count", int(metrics.proposed_via_count)),
                ("iterations", int(metrics.iterations)),
                ("region_only_access_count", int(metrics.region_only_access_count)),
                ("deferred_net_count", int(metrics.deferred_net_count)),
            ]),
        ));
        let routes = self
            .routes
            .iter()
            .map(|route| {
                let accesses = route
                    .accesses
                    .iter()
                    .map(|access| {
                        let mut entry = vec![
                            ("pad", text(&format!("{}.{}", access.pad.component, access.pad.pad))),
                            ("layer", text(access.layer.value())),
                            ("pad_position_nm", j_point(access.pad_position)),
                            ("access_position_nm", j_point(access.access_position)),
                            (
                                "physical_tracks",
                                Json::Array(
                                    access
                                        .tracks
                                        .iter()
                                        .map(|track| {
                                            j_object(vec![
                                                ("layer", text(track.layer.value())),
                                                ("start_nm", j_point(track.start)),
                                                ("end_nm", j_point(track.end)),
                                                ("width_nm", int(track.width_nm)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                            ("physical_via_nm", access.via.as_ref().map_or(Json::Null, |via| j_point(via.position))),
                            ("region_only", Json::Bool(access.region_only)),
                        ];
                        if let Some(retained) = &access.retained_via {
                            entry.push(("retained_via_nm", j_point(retained.position)));
                        }
                        j_object(entry)
                    })
                    .collect();
                let segments = route
                    .segments
                    .iter()
                    .map(|segment| {
                        j_object(vec![
                            ("layer", text(segment.layer.value())),
                            ("start_nm", j_point(segment.start)),
                            ("end_nm", j_point(segment.end)),
                            ("guide_half_width_nm", int(segment.guide_half_width_nm)),
                            ("resource", text(&segment.resource_id)),
                        ])
                    })
                    .collect();
                let vias = route
                    .vias
                    .iter()
                    .map(|via| {
                        j_object(vec![
                            ("position_nm", j_point(via.position)),
                            ("from_layer", text(via.from_layer.value())),
                            ("to_layer", text(via.to_layer.value())),
                            ("resource", text(&via.resource_id)),
                        ])
                    })
                    .collect();
                j_object(vec![
                    ("net", text(&route.net)),
                    ("connected", Json::Bool(route.connected)),
                    ("length_nm", int(route.length_nm)),
                    ("diagnostics", Json::Array(route.diagnostics.iter().map(|d| text(d)).collect())),
                    ("deferred_to_zone", Json::Bool(route.deferred_to_zone)),
                    ("accesses", Json::Array(accesses)),
                    ("segments", Json::Array(segments)),
                    ("vias", Json::Array(vias)),
                ])
            })
            .collect();
        document.push(("routes", Json::Array(routes)));
        let hotspots = self
            .hotspots
            .iter()
            .map(|item| {
                j_object(vec![
                    ("resource", text(&item.resource_id)),
                    ("layer", item.layer.map_or(Json::Null, |layer| text(layer.value()))),
                    ("start_nm", j_point(item.start)),
                    ("end_nm", j_point(item.end)),
                    ("capacity", int(item.capacity)),
                    ("usage", int(item.usage)),
                    ("overflow", int(item.overflow)),
                    ("contributors", Json::Array(item.contributors.iter().map(|c| text(c)).collect())),
                ])
            })
            .collect();
        document.push(("hotspots", Json::Array(hotspots)));
        Dumps::new().indent(2).sort_keys(true).dumps(&j_object(document)) + "\n"
    }
}

/// `GlobalRouterOptions`, validated by [`GlobalRouterOptions::validated`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalRouterOptions {
    pub tile_size_nm: i64,
    pub maximum_iterations: i64,
    pub present_penalty: i64,
    pub present_penalty_growth: i64,
    pub history_penalty: i64,
    pub via_cost: i64,
    pub bend_cost: i64,
    pub layer_preference_cost: i64,
    pub direction_preference_cost: i64,
    pub guide_half_width_tiles: i64,
    pub layer_assignment_passes: i64,
    pub local_demand_cost: i64,
    pub local_demand_pitch_nm: i64,
    pub pin_access_candidates: i64,
    pub escape_radius_nm: i64,
    pub escape_step_nm: i64,
    pub critical_escape_step_nm: Option<i64>,
}

impl Default for GlobalRouterOptions {
    fn default() -> Self {
        GlobalRouterOptions {
            tile_size_nm: crate::nm_from_mm(5),
            maximum_iterations: 20,
            present_penalty: 40,
            present_penalty_growth: 2,
            history_penalty: 20,
            via_cost: 20,
            bend_cost: 2,
            layer_preference_cost: 2,
            direction_preference_cost: 1,
            guide_half_width_tiles: 1,
            layer_assignment_passes: 0,
            local_demand_cost: 0,
            local_demand_pitch_nm: 0,
            pin_access_candidates: 4,
            escape_radius_nm: crate::nm_from_mm(3),
            escape_step_nm: 500_000,
            critical_escape_step_nm: None,
        }
    }
}

impl GlobalRouterOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if self.tile_size_nm <= 0 || self.maximum_iterations <= 0 {
            return fail("global router dimensions and iteration count must be positive");
        }
        if self.pin_access_candidates.min(self.escape_radius_nm).min(self.escape_step_nm) <= 0 {
            return fail("pin-access options must be positive");
        }
        if self.critical_escape_step_nm.is_some_and(|step| step <= 0) {
            return fail("critical pin-access refinement must be a positive integer");
        }
        if [
            self.present_penalty,
            self.present_penalty_growth,
            self.history_penalty,
            self.via_cost,
            self.bend_cost,
            self.layer_preference_cost,
            self.direction_preference_cost,
            self.layer_assignment_passes,
            self.local_demand_cost,
            self.local_demand_pitch_nm,
        ]
        .into_iter()
        .min()
        .expect("costs")
            < 0
        {
            return fail("global router costs cannot be negative");
        }
        if self.guide_half_width_tiles < 0 {
            return fail("global route guide width cannot be negative");
        }
        Ok(self)
    }
}

/// `_Resource`: one capacity-bearing edge of the coarse graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub identifier: String,
    pub first: GridNode,
    pub second: GridNode,
    pub capacity: i64,
    pub layer: Option<CopperLayer>,
}

/// A planar or vertical move: `""` (start), `"h"`, `"n"` or `"v"`, in Python's string order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Start,
    H,
    N,
    V,
}

/// `_Graph`: an immutable coarse graph snapshot with its static step costs.
#[derive(Clone, Debug)]
pub struct Graph {
    pub layers: Vec<CopperLayer>,
    pub xs: Vec<i64>,
    pub ys: Vec<i64>,
    pub legal_nodes: IndexSet<GridNode>,
    pub resources: IndexMap<(GridNode, GridNode), Resource>,
    pub via_sites: IndexMap<String, Vec<Point>>,
    pub layer_ranks: IndexMap<CopperLayer, i64>,
    pub preferred_headings: IndexMap<CopperLayer, &'static str>,
    pub demand_lanes: i64,
    step_cache: RefCell<IndexMap<(GridNode, i64, i64, i64), Vec<(GridNode, Resource, Direction, i64)>>>,
}

impl Graph {
    pub fn point(&self, node: GridNode) -> Point {
        Point::new(self.xs[node.x_index as usize], self.ys[node.y_index as usize])
    }

    pub fn layer_position(&self, layer: CopperLayer) -> Option<i64> {
        self.layers.iter().position(|l| *l == layer).map(|index| index as i64)
    }

    /// `search_steps`: exact static costs/edges only; never congestion or path answers.
    pub fn search_steps(
        &self,
        node: GridNode,
        options: &GlobalRouterOptions,
    ) -> Vec<(GridNode, Resource, Direction, i64)> {
        let key = (node, options.via_cost, options.layer_preference_cost, options.direction_preference_cost);
        if let Some(cached) = self.step_cache.borrow().get(&key) {
            return cached.clone();
        }
        // Keep planar order followed by ascending via layers. Per-search layer
        // filtering occurs before every dynamic gate, just as in `neighbors`.
        let all: IndexSet<i64> = (0..self.layers.len() as i64).collect();
        let mut steps = Vec::new();
        for other in neighbors(self, node, &all) {
            let resource = self.resources[&edge_key(node, other)].clone();
            let direction = direction(node, other);
            let base = if direction == Direction::V {
                options.via_cost * COST_UNIT
            } else {
                length_cost(self.point(node), self.point(other), 10)
            };
            let cost = base + graph_preference_cost(self, node, other, options);
            steps.push((other, resource, direction, cost));
        }
        // One graph-sized allowance across all cost variants.
        if self.legal_nodes.contains(&node) && self.step_cache.borrow().len() < self.legal_nodes.len() {
            self.step_cache.borrow_mut().insert(key, steps.clone());
        }
        steps
    }

    /// `replace(graph, resources=..., via_sites=...)`: a fresh snapshot with fresh derived data.
    pub fn with(
        &self,
        resources: IndexMap<(GridNode, GridNode), Resource>,
        via_sites: IndexMap<String, Vec<Point>>,
    ) -> Graph {
        Graph {
            layers: self.layers.clone(),
            xs: self.xs.clone(),
            ys: self.ys.clone(),
            legal_nodes: self.legal_nodes.clone(),
            resources,
            via_sites,
            layer_ranks: self.layer_ranks.clone(),
            preferred_headings: self.preferred_headings.clone(),
            demand_lanes: self.demand_lanes,
            step_cache: RefCell::new(IndexMap::new()),
        }
    }
}

/// `_Attempt`: one iteration's routes with their measured usage.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub routes: Vec<GlobalNetRoute>,
    pub usage: IndexMap<String, i64>,
    pub contributors: IndexMap<String, Vec<String>>,
    pub metrics: GlobalRoutingMetrics,
}

/// Candidate accesses per `(net, pad)`.
pub type AccessOptions = IndexMap<(String, PadReference), Vec<PinAccess>>;

/// `route_global`: route all multi-terminal nets into deterministic multilayer guides.
///
/// An explicit `fixed_prefix` may append immutable copper to the clean
/// source; its exact source contracts, macro/array prefix and fresh physical
/// DRC must pass before reserved ports can enter the coarse graph.
pub fn route_global(
    board: &PhysicalBoard,
    options: &GlobalRouterOptions,
    mut on_progress: Option<ProgressCallback<'_>>,
    reserved_accesses: Option<&IndexMap<PadReference, Anchor>>,
    fixed_prefix: Option<&PhysicalBoard>,
) -> Result<GlobalRoutingResult> {
    emit(&mut on_progress, "global_route", "started", vec![("maximum_iterations", int(options.maximum_iterations))]);
    let source = copper_drc::macros::materialize_hard_macros(macro_source(board.clone())?)?;
    let mut prefix_digest = None;
    let held;
    let board: &PhysicalBoard = match fixed_prefix {
        Some(prefix) => {
            prefix_digest = Some(admit_fixed_prefix(&source, prefix)?);
            held = prefix.clone();
            &held
        }
        None => {
            if reserved_accesses.is_some_and(|reserved| !reserved.is_empty()) {
                return fail("reserved global accesses require an explicitly fixed copper prefix");
            }
            &source
        }
    };
    emit(&mut on_progress, "global_graph", "started", Vec::new());
    let mut graph = build_graph(board, options, false)?;
    emit(
        &mut on_progress,
        "global_graph",
        "finished",
        vec![
            ("node_count", int(graph.legal_nodes.len() as i64)),
            ("resource_count", int(graph.resources.len() as i64)),
        ],
    );
    emit(&mut on_progress, "global_pin_access", "started", Vec::new());
    let rules: IndexMap<String, NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.clone(), r.clone())).collect();
    let clearance = RoutingClearanceIndex::new(board)?;
    let zone_nets = deferred_zone_nets(board)?;
    let owner_graph: Option<Connectivity> = if !board.tracks.is_empty() || !board.vias.is_empty() {
        Some(explicit_copper_connectivity(board, Candidate::default())?)
    } else {
        None
    };
    let owner_connected: IndexSet<String> = board
        .nets
        .iter()
        .filter(|net| owner_graph.as_ref().is_some_and(|graph| graph.net_connected(net)))
        .map(|net| net.name.clone())
        .collect();
    if let Some(reserved) = reserved_accesses {
        let known: IndexSet<&PadReference> = board.nets.iter().flat_map(|n| n.pads.iter()).collect();
        if !reserved.keys().all(|pad| known.contains(pad)) {
            return fail("reserved global access references an unknown terminal");
        }
    }
    let pin_access = |graph: &Graph| -> Result<AccessOptions> {
        let mut result = AccessOptions::new();
        for net in &board.nets {
            if net.pads.len() < 2 || zone_nets.contains(&net.name) || owner_connected.contains(&net.name) {
                continue;
            }
            let rule = rules.get(&net.name);
            let layers = routing_layers(board, &net.name, rule);
            for pad in macro_routing_pads(board, net)? {
                let candidates = match reserved_accesses.and_then(|reserved| reserved.get(&pad)) {
                    Some(anchor) => reserved_pin_access_candidates(
                        board, graph, &pad, &net.name, &layers, rule, &clearance, options, anchor,
                    )?,
                    None => {
                        global_pin_access_candidates(board, graph, &pad, &net.name, &layers, rule, &clearance, options)?
                    }
                };
                result.insert((net.name.clone(), pad), candidates);
            }
        }
        Ok(result)
    };
    let wider = GlobalRouterOptions { pin_access_candidates: 4 * options.pin_access_candidates, ..options.clone() };
    // Pin access after terminal capacity registration, which can drop launches.
    let registered_access = |graph: &Graph| -> Result<(Graph, AccessOptions)> {
        let expand = |key: &(String, PadReference)| -> Result<Vec<PinAccess>> {
            let (name, pad) = key;
            let rule = rules.get(name);
            pin_access_candidates(board, graph, pad, name, &routing_layers(board, name, rule), rule, &clearance, &wider)
        };
        register_terminal_capacity(board, graph, pin_access(graph)?, Some(&expand))
    };
    let generated_accesses = pin_access(&graph)?;
    let (registered_graph, mut access_options) = registered_access(&graph)?;
    graph = registered_graph;
    if access_options.values().any(|choices| choices.is_empty()) {
        // A pad near a partial edge tile can be beyond reach of every node, or
        // reach only nodes whose launches cannot be registered. Only then does
        // the graph gain or move an edge row or column, so boards whose pins
        // all have access keep their grid (and their routes).
        let strip_graph = build_graph(board, options, true)?;
        if (&strip_graph.xs, &strip_graph.ys) != (&graph.xs, &graph.ys) {
            let (strip_graph, strip_access) = registered_access(&strip_graph)?;
            let missing = |access: &AccessOptions| access.values().filter(|choices| choices.is_empty()).count();
            if missing(&strip_access) < missing(&access_options) {
                graph = strip_graph;
                access_options = strip_access;
                emit(
                    &mut on_progress,
                    "global_graph",
                    "edge_strips",
                    vec![
                        ("node_count", int(graph.legal_nodes.len() as i64)),
                        ("resource_count", int(graph.resources.len() as i64)),
                    ],
                );
            }
        }
    }
    let access_failures = terminal_access_failures(&generated_accesses, &access_options);
    if on_progress.is_some() {
        emit(
            &mut on_progress,
            "global_pin_access",
            "finished",
            vec![
                ("terminal_count", int(access_options.len() as i64)),
                ("missing_local_access_count", int(access_failures.values().filter(|c| c.0 == 0).count() as i64)),
                ("capacity_rejected_terminal_count", int(access_failures.values().filter(|c| c.0 != 0).count() as i64)),
            ],
        );
    }
    let mut capacities: IndexMap<String, i64> =
        graph.resources.values().map(|r| (r.identifier.clone(), r.capacity)).collect();
    let mut history: IndexMap<String, i64> = capacities.keys().map(|id| (id.clone(), 0)).collect();
    let mut best: Option<Attempt> = None;
    let mut present = options.present_penalty;
    let mut completed_iterations = 0;
    let mut refined_via_resources: IndexSet<String> = IndexSet::new();
    let demands = NetDemands::new(board, &rules, options)?;
    for iteration in 1..=options.maximum_iterations {
        completed_iterations = iteration;
        emit(
            &mut on_progress,
            "global_iteration",
            "started",
            vec![("iteration", int(iteration)), ("maximum_iterations", int(options.maximum_iterations))],
        );
        let mut attempt = route_iteration(
            board,
            &graph,
            &rules,
            &access_options,
            &history,
            present,
            options,
            iteration,
            &demands,
            owner_graph.as_ref(),
            &zone_nets,
        )?;
        if let Some(refined) = refine_overflowed_via_sites(
            board,
            &graph,
            &attempt.usage,
            &clearance,
            options.tile_size_nm,
            &mut refined_via_resources,
        )? {
            graph = refined;
            capacities = graph.resources.values().map(|r| (r.identifier.clone(), r.capacity)).collect();
            attempt = measure_attempt(&graph, attempt.routes, iteration, &demands);
            if let Some(previous) = best.take() {
                best = Some(measure_attempt(&graph, previous.routes, previous.metrics.iterations, &demands));
            }
        }
        let improved =
            best.as_ref().is_none_or(|best| attempt.metrics.quality_vector() < best.metrics.quality_vector());
        if improved {
            best = Some(attempt.clone());
        }
        emit(
            &mut on_progress,
            "global_iteration",
            "finished",
            vec![
                ("iteration", int(iteration)),
                ("maximum_iterations", int(options.maximum_iterations)),
                ("unrouted_net_count", int(attempt.metrics.unrouted_net_count)),
                ("total_overflow", int(attempt.metrics.total_overflow)),
                ("improved", Json::Bool(improved)),
            ],
        );
        if attempt.metrics.unrouted_net_count == 0 && attempt.metrics.total_overflow == 0 {
            best = Some(attempt);
            break;
        }
        for (identifier, used) in &attempt.usage {
            *history.get_mut(identifier).expect("resource history") += (used - capacities[identifier]).max(0);
        }
        present *= options.present_penalty_growth.max(1);
    }
    let mut best = best.expect("at least one iteration");
    if options.layer_assignment_passes != 0 {
        let assigned = crate::layer_assignment::assign_ordinary_layers(
            board,
            &graph,
            &best.routes,
            &rules,
            &access_options,
            options,
        )?;
        if assigned != best.routes {
            best = measure_attempt(&graph, assigned, best.metrics.iterations, &demands);
        }
    }
    let metrics = GlobalRoutingMetrics { iterations: completed_iterations, ..best.metrics.clone() };
    let hotspots = hotspots(&graph, &best.usage, &best.contributors);
    let status = if metrics.unrouted_net_count != 0 {
        GlobalRoutingStatus::Unreachable
    } else if metrics.total_overflow != 0 {
        GlobalRoutingStatus::Overflow
    } else {
        GlobalRoutingStatus::Success
    };
    let placement_fingerprint = placement_fingerprint(board);
    let mut failures: Vec<(&(String, PadReference), &(i64, i64))> = access_failures.iter().collect();
    failures.sort_by(|a, b| a.0.cmp(b.0));
    let reported_routes: Vec<GlobalNetRoute> = best
        .routes
        .iter()
        .map(|route| {
            if route.connected {
                return route.clone();
            }
            let mut diagnostics = route.diagnostics.clone();
            for ((net, pad), (generated, registered)) in &failures {
                if *net != route.net {
                    continue;
                }
                let cause = if *generated == 0 {
                    "local escape search found no candidate"
                } else {
                    "shared transition capacity rejected every candidate"
                };
                diagnostics.push(format!(
                    "terminal access {}.{}: generated {generated} candidate(s), registered {registered}; {cause}",
                    pad.component, pad.pad
                ));
            }
            GlobalNetRoute { diagnostics, ..route.clone() }
        })
        .collect();
    let routing_fingerprint =
        routing_fingerprint(&placement_fingerprint, &reported_routes, &metrics, options, prefix_digest.as_deref());
    let result = GlobalRoutingResult {
        status,
        routes: reported_routes,
        hotspots,
        metrics,
        placement_fingerprint,
        routing_fingerprint,
        fixed_prefix_digest: prefix_digest,
    };
    emit(
        &mut on_progress,
        "global_route",
        "finished",
        vec![
            ("status", text(status.value())),
            ("iterations", int(completed_iterations)),
            ("unrouted_net_count", int(result.metrics.unrouted_net_count)),
            ("total_overflow", int(result.metrics.total_overflow)),
        ],
    );
    Ok(result)
}

/// `_global_pin_access_candidates`: one opt-in finer retry for a failed specialized terminal.
#[allow(clippy::too_many_arguments)]
fn global_pin_access_candidates(
    board: &PhysicalBoard,
    graph: &Graph,
    pad: &PadReference,
    net: &str,
    layers: &[CopperLayer],
    rule: Option<&NetRoutingRule>,
    clearance: &RoutingClearanceIndex<'_>,
    options: &GlobalRouterOptions,
) -> Result<Vec<PinAccess>> {
    let candidates = pin_access_candidates(board, graph, pad, net, layers, rule, clearance, options)?;
    if candidates.is_empty()
        && let Some(rule) = rule
        && rule.kind != RouteKind::General
        && let Some(step) = options.critical_escape_step_nm
        && step < options.escape_step_nm
    {
        let finer = GlobalRouterOptions { escape_step_nm: step, critical_escape_step_nm: None, ..options.clone() };
        return pin_access_candidates(board, graph, pad, net, layers, Some(rule), clearance, &finer);
    }
    Ok(candidates)
}

/// Per-net lane demand and the breakout neck-down demand function (`_net_demand`, `_planar_demand`).
pub struct NetDemands<'b> {
    board: &'b PhysicalBoard,
    options: &'b GlobalRouterOptions,
    rules: &'b IndexMap<String, NetRoutingRule>,
    regions: Option<BreakoutRegions>,
}

/// `_planar_demand`'s closure: the narrow demand inside a shared region, else the default.
pub struct PlanarDemand<'d> {
    net: String,
    narrow: i64,
    default: i64,
    regions: &'d BreakoutRegions,
    cache: RefCell<IndexMap<(Point, Point), i64>>,
}

impl PlanarDemand<'_> {
    pub fn demand(&self, start: Point, end: Point) -> i64 {
        if let Some(value) = self.cache.borrow().get(&(start, end)) {
            return *value;
        }
        let value = if self.regions.region(&self.net, &[start, end]).is_some() { self.narrow } else { self.default };
        self.cache.borrow_mut().insert((start, end), value);
        value
    }
}

impl<'b> NetDemands<'b> {
    fn new(
        board: &'b PhysicalBoard,
        rules: &'b IndexMap<String, NetRoutingRule>,
        options: &'b GlobalRouterOptions,
    ) -> Result<Self> {
        let needs_regions = rules.values().any(|rule| {
            rule.kind == RouteKind::General && rule.breakout_length_nm.is_some() && rule.breakout_width_nm.is_some()
        });
        Ok(NetDemands { board, options, rules, regions: needs_regions.then(|| BreakoutRegions::new(board)) })
    }

    /// `_net_demand`.
    pub fn net_demand(&self, rule: Option<&NetRoutingRule>) -> i64 {
        net_demand(self.board, rule, self.options)
    }

    /// `_planar_demand`: ordinary source-declared neck-down demand on a complete coarse edge.
    ///
    /// Capacity guidance only, never emitted copper or a width waiver. Both
    /// ends must lie in one exact shared breakout region.
    pub fn planar_demand(&self, rule: Option<&NetRoutingRule>, default: Option<i64>) -> Option<PlanarDemand<'_>> {
        let rule = rule?;
        if rule.kind != RouteKind::General {
            return None;
        }
        let (Some(_), Some(breakout_width)) = (rule.breakout_length_nm, rule.breakout_width_nm) else { return None };
        let default = default.unwrap_or_else(|| self.net_demand(Some(rule)));
        let clearance = rule.clearance_nm.filter(|c| *c != 0).unwrap_or(self.board.rules.minimum_clearance_nm);
        let pitch = self.board.rules.default_track_width_nm + self.board.rules.minimum_clearance_nm;
        let narrow = ((breakout_width + clearance + pitch - 1).div_euclid(pitch)).max(1);
        if narrow >= default {
            return None;
        }
        let _ = self.rules;
        Some(PlanarDemand {
            net: rule.net.clone(),
            narrow,
            default,
            regions: self.regions.as_ref().expect("breakout regions"),
            cache: RefCell::new(IndexMap::new()),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn route_iteration(
    board: &PhysicalBoard,
    graph: &Graph,
    rules: &IndexMap<String, NetRoutingRule>,
    access_options: &AccessOptions,
    history: &IndexMap<String, i64>,
    present: i64,
    options: &GlobalRouterOptions,
    iteration: i64,
    demands: &NetDemands<'_>,
    owner_graph: Option<&Connectivity>,
    zone_nets: &IndexSet<String>,
) -> Result<Attempt> {
    let mut usage: IndexMap<String, i64> = graph.resources.values().map(|r| (r.identifier.clone(), 0)).collect();
    let mut routes = Vec::new();
    let mut ordered_nets: Vec<&copper_pcb::board::PhysicalNet> =
        board.nets.iter().filter(|n| n.pads.len() >= 2).collect();
    ordered_nets.sort_by_key(|net| net_order(&net.name, rules.get(&net.name)));
    for net in ordered_nets {
        if owner_graph.is_some_and(|graph| graph.net_connected(net)) {
            routes.push(GlobalNetRoute {
                net: net.name.clone(),
                connected: true,
                accesses: Vec::new(),
                segments: Vec::new(),
                vias: Vec::new(),
                length_nm: 0,
                diagnostics: vec!["connected by immutable input copper".to_owned()],
                deferred_to_zone: false,
            });
            continue;
        }
        if zone_nets.contains(&net.name) {
            routes.push(GlobalNetRoute {
                net: net.name.clone(),
                connected: false,
                accesses: Vec::new(),
                segments: Vec::new(),
                vias: Vec::new(),
                length_nm: 0,
                diagnostics: vec!["deferred to declared copper zone and verified fill".to_owned()],
                deferred_to_zone: true,
            });
            continue;
        }
        let rule = rules.get(&net.name);
        let demand = demands.net_demand(rule);
        let route = route_net(
            board,
            graph,
            &net.name,
            &macro_routing_pads(board, net)?,
            rule,
            access_options,
            demand,
            &usage,
            history,
            present,
            options,
            demands,
        )?;
        let connected = route.connected;
        if connected {
            let local = demands.planar_demand(rule, None);
            for (identifier, amount) in route_resource_demands(&route, demand, local.as_ref()) {
                *usage.get_mut(&identifier).expect("routed resource") += amount;
            }
        }
        routes.push(route);
    }
    Ok(measure_attempt(graph, routes, iteration, demands))
}

/// `_attempt`: measure planar lanes and distinct physical via sites in routing order.
fn measure_attempt(graph: &Graph, routes: Vec<GlobalNetRoute>, iteration: i64, demands: &NetDemands<'_>) -> Attempt {
    let mut usage: IndexMap<String, i64> = graph.resources.values().map(|r| (r.identifier.clone(), 0)).collect();
    let mut contributors: IndexMap<String, Vec<String>> =
        graph.resources.values().map(|r| (r.identifier.clone(), Vec::new())).collect();
    for route in &routes {
        if !route.connected {
            continue;
        }
        let rule = demands.rules.get(&route.net);
        let demand = demands.net_demand(rule);
        let local = demands.planar_demand(rule, None);
        for (identifier, amount) in route_resource_demands(route, demand, local.as_ref()) {
            *usage.get_mut(&identifier).expect("routed resource") += amount;
            contributors.get_mut(&identifier).expect("routed resource").push(route.net.clone());
        }
    }
    let capacities: IndexMap<String, i64> =
        graph.resources.values().map(|r| (r.identifier.clone(), r.capacity)).collect();
    let overflows: Vec<i64> = capacities.iter().map(|(key, capacity)| (usage[key] - capacity).max(0)).collect();
    let maximum_utilization = capacities
        .iter()
        .map(|(key, capacity)| (usage[key] * 1_000_000).div_euclid((*capacity).max(1)))
        .max()
        .unwrap_or(0);
    let metrics = GlobalRoutingMetrics {
        routed_net_count: routes.iter().filter(|r| r.connected).count() as i64,
        unrouted_net_count: routes.iter().filter(|r| !r.connected && !r.deferred_to_zone).count() as i64,
        total_overflow: overflows.iter().sum(),
        maximum_overflow: overflows.iter().copied().max().unwrap_or(0),
        overfull_resource_count: overflows.iter().filter(|v| **v > 0).count() as i64,
        maximum_utilization_ppm: maximum_utilization,
        total_length_nm: routes.iter().map(|r| r.length_nm).sum(),
        proposed_via_count: routes.iter().map(|r| r.vias.len() as i64).sum(),
        iterations: iteration,
        region_only_access_count: routes.iter().flat_map(|r| r.accesses.iter()).filter(|a| a.region_only).count()
            as i64,
        deferred_net_count: routes.iter().filter(|r| r.deferred_to_zone).count() as i64,
    };
    for value in contributors.values_mut() {
        value.sort();
    }
    Attempt { routes, usage, contributors, metrics }
}

/// `_resource_congestion_cost`: the same negotiated penalty for path edges and terminal transitions.
fn resource_congestion_cost(
    resource: &Resource,
    demand: i64,
    usage: &IndexMap<String, i64>,
    history: &IndexMap<String, i64>,
    present: i64,
    options: &GlobalRouterOptions,
) -> i64 {
    let overflow = (usage[&resource.identifier] + demand - resource.capacity).max(0);
    COST_UNIT * (present * overflow + options.history_penalty * history[&resource.identifier])
}

/// `_terminal_via_cost`: charge a new registered terminal hole, once per physical site/net.
#[allow(clippy::too_many_arguments)]
fn terminal_via_cost(
    graph: &Graph,
    candidate: &PinAccess,
    accesses: &[PinAccess],
    route_resources: &IndexMap<String, Resource>,
    path: &[(GridNode, GridNode)],
    usage: &IndexMap<String, i64>,
    history: &IndexMap<String, i64>,
    present: i64,
    options: &GlobalRouterOptions,
) -> Option<i64> {
    let Some(via) = &candidate.via else { return Some(0) };
    let (a, b) = (graph.layer_position(via.from_layer)?, graph.layer_position(via.to_layer)?);
    let (low, high) = (a.min(b), a.max(b));
    let identifier = via_resource_id(graph, candidate.node, via.from_layer, via.to_layer);
    let first = GridNode::new(low, candidate.node.x_index, candidate.node.y_index);
    let second = GridNode::new(high, candidate.node.x_index, candidate.node.y_index);
    let resource = graph.resources.get(&edge_key(first, second))?;
    if resource.layer.is_some()
        || resource.identifier != identifier
        || !graph.via_sites.get(&identifier).is_some_and(|sites| sites.contains(&via.position))
    {
        return None;
    }
    let mut positions: IndexSet<Point> = IndexSet::new();
    for access in accesses {
        if let Some(other) = &access.via
            && via_resource_id(graph, access.node, other.from_layer, other.to_layer) == identifier
        {
            positions.insert(other.position);
        }
    }
    positions.extend(
        route_resources
            .values()
            .filter(|item| item.layer.is_none() && item.identifier == identifier)
            .map(|item| graph.point(item.first)),
    );
    for (a, b) in path {
        let item = &graph.resources[&edge_key(*a, *b)];
        if item.identifier == identifier && item.layer.is_none() {
            positions.insert(graph.point(item.first));
        }
    }
    if positions.contains(&via.position) {
        return Some(0);
    }
    Some(resource_congestion_cost(resource, positions.len() as i64 + 1, usage, history, present, options))
}

#[allow(clippy::too_many_arguments)]
fn route_net(
    board: &PhysicalBoard,
    graph: &Graph,
    net: &str,
    pads: &[PadReference],
    rule: Option<&NetRoutingRule>,
    access_options: &AccessOptions,
    demand: i64,
    usage: &IndexMap<String, i64>,
    history: &IndexMap<String, i64>,
    present: i64,
    options: &GlobalRouterOptions,
    demands: &NetDemands<'_>,
) -> Result<GlobalNetRoute> {
    let allowed = routing_layers(board, net, rule);
    let local_demand = demands.planar_demand(rule, Some(demand));
    let reservations = ZoneRoutingReservations::new(board);
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    // Capacity is shared across nets; exclude unavailable planar resources for
    // this net only. Via transitions still use the ordinary physical rules.
    let mut blocked_resources: IndexSet<String> = IndexSet::new();
    if !reservations.zones.is_empty() {
        for resource in graph.resources.values() {
            if let Some(layer) = resource.layer {
                let shape = RoundedConvexShape::new(
                    vec![graph.point(resource.first), graph.point(resource.second)],
                    width.div_euclid(2),
                )?;
                if !reservations.can_area(net, &shape, layer) {
                    blocked_resources.insert(resource.identifier.clone());
                }
            }
        }
    }
    let mut sorted_pads = pads.to_vec();
    sorted_pads.sort();
    let empty = Vec::new();
    let by_pad: IndexMap<&PadReference, &Vec<PinAccess>> = sorted_pads
        .iter()
        .map(|pad| (pad, access_options.get(&(net.to_owned(), pad.clone())).unwrap_or(&empty)))
        .collect();
    for (pad, choices) in &by_pad {
        if choices.is_empty() {
            return Ok(GlobalNetRoute::failed(
                net,
                Vec::new(),
                format!("no legal bounded local/global access for {}.{}", pad.component, pad.pad),
            ));
        }
    }
    let root_pad = *by_pad.keys().min_by_key(|pad| (by_pad[*pad].len(), *pad)).expect("pads");
    let root =
        by_pad[root_pad].iter().min_by_key(|item| (item.estimated_cost_nm(), item.node)).expect("choices").clone();
    let mut accesses = vec![root.clone()];
    let mut tree_nodes: IndexSet<GridNode> = IndexSet::from([root.node]);
    let mut remaining: Vec<&PadReference> = by_pad.keys().copied().filter(|pad| *pad != root_pad).collect();
    let mut route_resources: IndexMap<String, Resource> = IndexMap::new();
    let mut via_count: i64 = root.via.is_some() as i64;
    while !remaining.is_empty() {
        let target_pad = *remaining
            .iter()
            .min_by_key(|pad| {
                let nearest = by_pad[**pad]
                    .iter()
                    .flat_map(|access| tree_nodes.iter().map(move |tree| node_distance(access.node, *tree)))
                    .min()
                    .expect("tree nodes");
                (nearest, (**pad).clone())
            })
            .expect("remaining");
        let mut found: Vec<(i64, &PinAccess, Vec<(GridNode, GridNode)>)> = Vec::new();
        for candidate in by_pad[target_pad] {
            let allowance =
                rule.and_then(|r| r.max_vias).map(|max_vias| max_vias - via_count - candidate.via.is_some() as i64);
            let path = search(
                graph,
                &tree_nodes,
                candidate.node,
                &allowed,
                demand,
                usage,
                history,
                present,
                options,
                allowance,
                &blocked_resources,
                local_demand.as_ref(),
            );
            let Some(path) = path else { continue };
            let mut path_cost = 0;
            for (a, b) in &path {
                let resource = &graph.resources[&edge_key(*a, *b)];
                let base = if a.layer_index != b.layer_index {
                    options.via_cost * COST_UNIT
                } else {
                    length_cost(graph.point(*a), graph.point(*b), 10)
                };
                let resource_demand = if a.layer_index == b.layer_index {
                    match &local_demand {
                        Some(local) => local.demand(graph.point(*a), graph.point(*b)),
                        None => demand,
                    }
                } else {
                    1
                };
                path_cost += base
                    + graph_preference_cost(graph, *a, *b, options)
                    + resource_congestion_cost(resource, resource_demand, usage, history, present, options);
            }
            let Some(terminal_cost) = terminal_via_cost(
                graph,
                candidate,
                &accesses,
                &route_resources,
                &path,
                usage,
                history,
                present,
                options,
            ) else {
                continue;
            };
            found.push((path_cost + 10 * candidate.estimated_cost_nm() + terminal_cost, candidate, path));
        }
        if found.is_empty() {
            return Ok(GlobalNetRoute::failed(
                net,
                accesses,
                format!("no capacity-graph path reaches terminal {}.{}", target_pad.component, target_pad.pad),
            ));
        }
        let key = |item: &(i64, &PinAccess, Vec<(GridNode, GridNode)>)| {
            (item.0, item.1.estimated_cost_nm(), item.1.node, item.1.access_position.x_nm, item.1.access_position.y_nm)
        };
        let best = found.iter().enumerate().min_by_key(|(_, item)| key(item)).map(|(index, _)| index).expect("found");
        let (_, chosen, path) = found.swap_remove(best);
        accesses.push(chosen.clone());
        via_count += chosen.via.is_some() as i64;
        tree_nodes.insert(chosen.node);
        for (first, second) in &path {
            let resource = &graph.resources[&edge_key(*first, *second)];
            route_resources.entry(resource.identifier.clone()).or_insert_with(|| resource.clone());
            tree_nodes.insert(*first);
            tree_nodes.insert(*second);
            if first.layer_index != second.layer_index {
                via_count += 1;
            }
        }
        remaining.retain(|pad| *pad != target_pad);
    }
    let mut segments = Vec::new();
    let mut vias: Vec<GlobalViaProposal> = Vec::new();
    let mut total_length: i64 = accesses
        .iter()
        .flat_map(|access| access.tracks.iter())
        .map(|track| (track.start.x_nm - track.end.x_nm).abs() + (track.start.y_nm - track.end.y_nm).abs())
        .sum();
    let mut identifiers: Vec<&String> = route_resources.keys().collect();
    identifiers.sort();
    for identifier in identifiers {
        let resource = &route_resources[identifier];
        let first = graph.point(resource.first);
        let second = graph.point(resource.second);
        if resource.first.layer_index == resource.second.layer_index {
            segments.push(GlobalRouteSegment {
                net: net.to_owned(),
                layer: graph.layers[resource.first.layer_index as usize],
                start: first,
                end: second,
                guide_half_width_nm: options.tile_size_nm * options.guide_half_width_tiles,
                resource_id: identifier.clone(),
            });
            total_length += (first.x_nm - second.x_nm).abs() + (first.y_nm - second.y_nm).abs();
        } else {
            vias.push(GlobalViaProposal {
                net: net.to_owned(),
                position: first,
                from_layer: graph.layers[resource.first.layer_index.min(resource.second.layer_index) as usize],
                to_layer: graph.layers[resource.first.layer_index.max(resource.second.layer_index) as usize],
                resource_id: identifier.clone(),
            });
        }
    }
    for access in &accesses {
        let Some(span) = &access.via else { continue };
        let identifier = via_resource_id(graph, access.node, span.from_layer, span.to_layer);
        vias.push(GlobalViaProposal {
            net: net.to_owned(),
            position: span.position,
            from_layer: span.from_layer,
            to_layer: span.to_layer,
            resource_id: identifier,
        });
    }
    let mut unique: Vec<GlobalViaProposal> = Vec::new();
    for via in vias {
        if !unique.contains(&via) {
            unique.push(via);
        }
    }
    Ok(GlobalNetRoute {
        net: net.to_owned(),
        connected: true,
        accesses,
        segments,
        vias: unique,
        length_nm: total_length,
        diagnostics: Vec::new(),
        deferred_to_zone: false,
    })
}

type State = (GridNode, Direction, i64);

/// `_search`: A* over the capacity graph from the tree to one target node.
#[allow(clippy::too_many_arguments)]
fn search(
    graph: &Graph,
    starts: &IndexSet<GridNode>,
    target: GridNode,
    allowed_layers: &[CopperLayer],
    demand: i64,
    usage: &IndexMap<String, i64>,
    history: &IndexMap<String, i64>,
    present: i64,
    options: &GlobalRouterOptions,
    remaining_vias: Option<i64>,
    blocked_resources: &IndexSet<String>,
    planar_demand: Option<&PlanarDemand<'_>>,
) -> Option<Vec<(GridNode, GridNode)>> {
    let allowed_indexes: IndexSet<i64> =
        allowed_layers.iter().filter_map(|layer| graph.layer_position(*layer)).collect();
    // (estimate, cost, heuristic, node, direction, vias, serial): a total
    // order, so the pop sequence is heapq's.
    let mut queue: BinaryHeap<Reverse<(i64, i64, i64, GridNode, Direction, i64, u64)>> = BinaryHeap::new();
    let mut previous: IndexMap<State, Option<State>> = IndexMap::new();
    let mut best: IndexMap<State, i64> = IndexMap::new();
    let mut serial = 0;
    let track_vias = remaining_vias.is_some();
    if remaining_vias.is_some_and(|remaining| remaining < 0) {
        return None;
    }
    let mut sorted_starts: Vec<GridNode> = starts.iter().copied().collect();
    sorted_starts.sort();
    for start in sorted_starts {
        if !allowed_indexes.contains(&start.layer_index) {
            continue;
        }
        let state = (start, Direction::Start, 0);
        best.insert(state, 0);
        previous.insert(state, None);
        let estimate = heuristic(graph, start, target);
        queue.push(Reverse((estimate, 0, estimate, start, Direction::Start, 0, serial)));
        serial += 1;
    }
    let mut final_state: Option<State> = None;
    while let Some(Reverse((_, cost, _, node, direction, vias_used, _))) = queue.pop() {
        let state = (node, direction, vias_used);
        if best.get(&state) != Some(&cost) {
            continue;
        }
        if node == target {
            final_state = Some(state);
            break;
        }
        for (neighbor, resource, next_direction, static_cost) in graph.search_steps(node, options) {
            if !allowed_indexes.contains(&neighbor.layer_index) || blocked_resources.contains(&resource.identifier) {
                continue;
            }
            let next_vias = if track_vias { vias_used + (next_direction == Direction::V) as i64 } else { 0 };
            if remaining_vias.is_some_and(|remaining| next_vias > remaining) {
                continue;
            }
            let bend = if direction != Direction::Start
                && direction != next_direction
                && direction != Direction::V
                && next_direction != Direction::V
            {
                options.bend_cost
            } else {
                0
            };
            let resource_demand = if resource.layer.is_some() {
                match planar_demand {
                    Some(local) => local.demand(graph.point(node), graph.point(neighbor)),
                    None => demand,
                }
            } else {
                1
            };
            let step = static_cost
                + COST_UNIT * bend
                + resource_congestion_cost(&resource, resource_demand, usage, history, present, options);
            let candidate_cost = cost + step;
            let candidate = (neighbor, next_direction, next_vias);
            if candidate_cost >= best.get(&candidate).copied().unwrap_or(1 << 60) {
                continue;
            }
            best.insert(candidate, candidate_cost);
            previous.insert(candidate, Some(state));
            let estimate = heuristic(graph, neighbor, target);
            queue.push(Reverse((
                candidate_cost + estimate,
                candidate_cost,
                estimate,
                neighbor,
                next_direction,
                next_vias,
                serial,
            )));
            serial += 1;
        }
    }
    let mut current = final_state?;
    let mut edges = Vec::new();
    while let Some(parent) = previous[&current] {
        edges.push((parent.0, current.0));
        current = parent;
    }
    edges.reverse();
    Some(edges)
}

/// `_preference_cost`: keep candidate scoring and path search on the same soft layer costs.
pub fn graph_preference_cost(graph: &Graph, first: GridNode, second: GridNode, options: &GlobalRouterOptions) -> i64 {
    let direction = direction(first, second);
    if direction == Direction::V {
        return 0;
    }
    let layer = graph.layers[first.layer_index as usize];
    let heading = match direction {
        Direction::H => "h",
        _ => "n",
    };
    let wrong_way = match graph.preferred_headings.get(&layer) {
        Some(preferred) if *preferred != heading => options.direction_preference_cost,
        _ => 0,
    };
    preference_cost(
        graph.point(first),
        graph.point(second),
        graph.layer_ranks[&layer],
        options.layer_preference_cost,
        wrong_way,
        false,
    )
}

/// `_build_graph`: the coarse capacity graph over the board's tiles.
pub fn build_graph(board: &PhysicalBoard, options: &GlobalRouterOptions, edge_strips: bool) -> Result<Graph> {
    let vertices = &board.outline.vertices;
    let min_x = vertices.iter().map(|p| p.x_nm).min().expect("outline");
    let max_x = vertices.iter().map(|p| p.x_nm).max().expect("outline");
    let min_y = vertices.iter().map(|p| p.y_nm).min().expect("outline");
    let max_y = vertices.iter().map(|p| p.y_nm).max().expect("outline");
    let (xs, x_strip) = axis_layout(min_x, max_x, options.tile_size_nm, edge_strips);
    let (ys, y_strip) = axis_layout(min_y, max_y, options.tile_size_nm, edge_strips);
    // Courtyards are assembly geometry; they are not copper obstructions.
    let mut copper_keepouts = resolved_copper_keepouts(board)?;
    copper_keepouts.extend(macro_reservations(board)?);
    let mut track_keepouts = Vec::new();
    for item in copper_keepouts.iter().filter(|item| item.block_tracks) {
        track_keepouts.push(GlobalTrackKeepout::from_keepout(item)?);
    }
    let stack = &board.stackup.copper_layers;
    let mut pad_obstacles: Vec<(Vec<CopperLayer>, (i64, i64, i64, i64))> = Vec::new();
    for placement in &board.placements {
        let footprint = &board.footprints[&placement.footprint];
        for pad in &footprint.pads {
            if pad.kind == PadKind::Aperture {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            let bounds = placed_pad_shape(position, pad, placement).bounds;
            let layers = if pad.kind == PadKind::Smd {
                vec![if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                stack.clone()
            };
            pad_obstacles.push((layers, (bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y)));
        }
    }
    for track in &board.tracks {
        let radius = track.width_nm.div_euclid(2);
        pad_obstacles.push((
            vec![track.layer],
            (
                track.start.x_nm.min(track.end.x_nm) - radius,
                track.start.y_nm.min(track.end.y_nm) - radius,
                track.start.x_nm.max(track.end.x_nm) + radius,
                track.start.y_nm.max(track.end.y_nm) + radius,
            ),
        ));
    }
    for via in &board.vias {
        let (a, b) = (crate::layer_index(stack, via.from_layer), crate::layer_index(stack, via.to_layer));
        let radius = via.size_nm.div_euclid(2);
        pad_obstacles.push((
            stack[a.min(b)..=a.max(b)].to_vec(),
            (
                via.position.x_nm - radius,
                via.position.y_nm - radius,
                via.position.x_nm + radius,
                via.position.y_nm + radius,
            ),
        ));
    }
    let mut legal: IndexSet<GridNode> = IndexSet::new();
    for (layer_index, layer) in stack.iter().enumerate() {
        for (x_index, x) in xs.iter().enumerate() {
            for (y_index, y) in ys.iter().enumerate() {
                let point = Point::new(*x, *y);
                if !point_in_material(board, point)? {
                    continue;
                }
                if track_keepouts.iter().any(|keepout| keepout.blocks_node(point, *layer)) {
                    continue;
                }
                legal.insert(GridNode::new(layer_index as i64, x_index as i64, y_index as i64));
            }
        }
    }
    let pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm;
    let mut resources: IndexMap<(GridNode, GridNode), Resource> = IndexMap::new();
    let mut via_sites: IndexMap<String, Vec<Point>> = IndexMap::new();
    let clearance = RoutingClearanceIndex::new(board)?;
    let mut sorted_legal: Vec<GridNode> = legal.iter().copied().collect();
    sorted_legal.sort();
    for node in sorted_legal {
        for (dx, dy) in [(1, 0), (0, 1)] {
            let neighbor = GridNode::new(node.layer_index, node.x_index + dx, node.y_index + dy);
            if !legal.contains(&neighbor) {
                continue;
            }
            let first = Point::new(xs[node.x_index as usize], ys[node.y_index as usize]);
            let second = Point::new(xs[neighbor.x_index as usize], ys[neighbor.y_index as usize]);
            if !shape_in_board(board, &RoundedConvexShape::new(vec![first, second], 0)?, 0, 0)? {
                continue;
            }
            let layer = stack[node.layer_index as usize];
            // An edge along a partial edge tile crosses only that strip.
            let band = if dx != 0 && y_strip.is_some() && node.y_index == ys.len() as i64 - 1 {
                y_strip
            } else if dy != 0 && x_strip.is_some() && node.x_index == xs.len() as i64 - 1 {
                x_strip
            } else {
                None
            };
            let capacity = planar_capacity(
                first,
                second,
                options.tile_size_nm,
                pitch,
                board.rules.minimum_clearance_nm,
                layer,
                &pad_obstacles,
                &track_keepouts,
                band,
            );
            if capacity != 0 {
                add_resource(&mut resources, node, neighbor, capacity, Some(layer), None);
            }
        }
        for other_index in node.layer_index + 1..stack.len() as i64 {
            let neighbor = GridNode::new(other_index, node.x_index, node.y_index);
            if !legal.contains(&neighbor) {
                continue;
            }
            let Some(span) = physical_via_span(board, stack[node.layer_index as usize], stack[other_index as usize])
            else {
                continue;
            };
            let span_start = crate::layer_index(stack, span.0);
            let span_end = crate::layer_index(stack, span.1);
            let identifier = format!("via:{span_start}:{span_end}:{}:{}", node.x_index, node.y_index);
            if !via_sites.contains_key(&identifier) {
                let sites = legal_via_sites(
                    board,
                    &clearance,
                    Point::new(xs[node.x_index as usize], ys[node.y_index as usize]),
                    options.tile_size_nm,
                    span.0,
                    span.1,
                )?;
                via_sites.insert(identifier.clone(), sites);
            }
            let count = via_sites[&identifier].len() as i64;
            if count != 0 {
                add_resource(&mut resources, node, neighbor, count, None, Some(identifier));
            }
        }
    }
    if resources.is_empty() {
        return fail("global routing capacity graph is empty");
    }
    let (layer_ranks, preferred_headings) = signal_layer_preferences(board);
    let demand_lanes = if options.local_demand_pitch_nm != 0 {
        options.tile_size_nm.div_euclid(options.local_demand_pitch_nm).max(1)
    } else {
        0
    };
    Ok(Graph {
        layers: stack.clone(),
        xs,
        ys,
        legal_nodes: legal,
        resources,
        via_sites,
        layer_ranks,
        preferred_headings,
        demand_lanes,
        step_cache: RefCell::new(IndexMap::new()),
    })
}

fn add_resource(
    resources: &mut IndexMap<(GridNode, GridNode), Resource>,
    first: GridNode,
    second: GridNode,
    capacity: i64,
    layer: Option<CopperLayer>,
    identifier: Option<String>,
) {
    let key = edge_key(first, second);
    let prefix = if first.layer_index != second.layer_index { "via" } else { "edge" };
    let identifier = identifier.unwrap_or_else(|| {
        format!(
            "{prefix}:{}:{}:{}:{}:{}:{}",
            first.layer_index, first.x_index, first.y_index, second.layer_index, second.x_index, second.y_index
        )
    });
    resources.insert(key, Resource { identifier, first: key.0, second: key.1, capacity, layer });
}

/// `_planar_capacity`: usable crossing lanes, conservatively clipping copper obstacles.
///
/// `band` replaces the tile's span across the edge (a partial edge tile).
#[allow(clippy::too_many_arguments)]
pub fn planar_capacity(
    first: Point,
    second: Point,
    tile_size_nm: i64,
    pitch_nm: i64,
    clearance_nm: i64,
    layer: CopperLayer,
    pads: &[(Vec<CopperLayer>, (i64, i64, i64, i64))],
    polygon_keepouts: &[GlobalTrackKeepout],
    band: Option<(i64, i64)>,
) -> i64 {
    let horizontal = first.y_nm == second.y_nm;
    let crossing =
        if horizontal { (first.x_nm + second.x_nm).div_euclid(2) } else { (first.y_nm + second.y_nm).div_euclid(2) };
    let center = if horizontal { first.y_nm } else { first.x_nm };
    let (low, high) = band.unwrap_or((center - tile_size_nm.div_euclid(2), center + tile_size_nm.div_euclid(2)));
    let mut blocked: Vec<(i64, i64)> = Vec::new();
    for (layers, (x0, y0, x1, y1)) in pads {
        if !layers.contains(&layer) {
            continue;
        }
        let (near, far) = if horizontal { (*x0, *x1) } else { (*y0, *y1) };
        if !(near - clearance_nm <= crossing && crossing <= far + clearance_nm) {
            continue;
        }
        let (start, end) = if horizontal { (*y0, *y1) } else { (*x0, *x1) };
        blocked.push((low.max(start - clearance_nm), high.min(end + clearance_nm)));
    }
    for keepout in polygon_keepouts {
        for (start, end) in keepout.blocked_intervals(first, second, tile_size_nm.div_euclid(2), clearance_nm, layer) {
            blocked.push((low.max(start), high.min(end)));
        }
    }
    blocked.sort();
    let mut cursor = low;
    let mut capacity = 0;
    for (start, end) in blocked {
        if end <= cursor || start >= high {
            continue;
        }
        if start > cursor {
            capacity += (start - cursor).div_euclid(pitch_nm);
        }
        cursor = cursor.max(end);
    }
    if cursor < high {
        capacity += (high - cursor).div_euclid(pitch_nm);
    }
    capacity
}

fn via_site_pitch(board: &PhysicalBoard) -> i64 {
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    (size + board.rules.minimum_clearance_nm)
        .max(drill + board.rules.minimum_hole_clearance_nm)
        .max(size.div_euclid(2) + drill.div_euclid(2) + board.rules.minimum_hole_clearance_nm)
}

/// `_refine_overflowed_via_sites`: try eight shifted phases once per congested tile, retaining old sites.
///
/// A greedy union counts only mutually legal, distinct-net vias: a bounded
/// capacity witness, not proof that detailed routes can reach them.
fn refine_overflowed_via_sites(
    board: &PhysicalBoard,
    graph: &Graph,
    usage: &IndexMap<String, i64>,
    clearance: &RoutingClearanceIndex<'_>,
    tile_size_nm: i64,
    refined: &mut IndexSet<String>,
) -> Result<Option<Graph>> {
    let mut sites = graph.via_sites.clone();
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    let half_phase = via_site_pitch(board).div_euclid(2);
    let limit = tile_size_nm.div_euclid(2) - (size.div_euclid(2) + board.rules.minimum_clearance_nm);
    let mut changed = false;
    for resource in graph.resources.values() {
        let identifier = &resource.identifier;
        if resource.layer.is_some()
            || refined.contains(identifier)
            || usage.get(identifier).copied().unwrap_or(0) <= resource.capacity
        {
            continue;
        }
        refined.insert(identifier.clone());
        let center = graph.point(resource.first);
        let span = physical_via_span(
            board,
            graph.layers[resource.first.layer_index as usize],
            graph.layers[resource.second.layer_index as usize],
        )
        .expect("via span");
        let mut pool: IndexSet<Point> = IndexSet::new();
        for dx in [-half_phase, 0, half_phase] {
            for dy in [-half_phase, 0, half_phase] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let found = legal_via_sites(
                    board,
                    clearance,
                    Point::new(center.x_nm + dx, center.y_nm + dy),
                    tile_size_nm,
                    span.0,
                    span.1,
                )?;
                pool.extend(found.into_iter().filter(|point| {
                    (point.x_nm - center.x_nm).abs() <= limit && (point.y_nm - center.y_nm).abs() <= limit
                }));
            }
        }
        let mut selected: Vec<Point> = sites[identifier].clone();
        let mut peers = RoutingClearanceIndex::new(board)?;
        let site_via = |index: usize, point: Point| Via {
            net: format!("<global-via-site-{index}>"),
            position: point,
            size_nm: size,
            drill_nm: drill,
            from_layer: span.0,
            to_layer: span.1,
            technology: span.2.clone(),
            finish: "standard".to_owned(),
        };
        for (index, point) in selected.iter().enumerate() {
            peers.add_via(site_via(index, *point), true, None)?;
        }
        let mut candidates: Vec<Point> = pool.into_iter().filter(|point| !selected.contains(point)).collect();
        candidates.sort_by_key(|p| (p.x_nm, p.y_nm));
        for point in candidates {
            let via = site_via(selected.len(), point);
            if peers.can_via(&via.net, point, size, span.0, span.1, Some(drill), false, None)? {
                peers.add_via(via, true, None)?;
                selected.push(point);
            }
        }
        if selected.len() as i64 > resource.capacity {
            sites.insert(identifier.clone(), selected);
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    let resources = graph
        .resources
        .iter()
        .map(|(key, resource)| {
            let resource = if resource.layer.is_none() {
                Resource { capacity: sites[&resource.identifier].len() as i64, ..resource.clone() }
            } else {
                resource.clone()
            };
            (*key, resource)
        })
        .collect();
    Ok(Some(graph.with(resources, sites)))
}

/// `_legal_via_sites`: independent, legal drill sites inside one coarse routing cell.
fn legal_via_sites(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    center: Point,
    tile_size_nm: i64,
    from_layer: CopperLayer,
    to_layer: CopperLayer,
) -> Result<Vec<Point>> {
    let size = board.rules.default_via_size_nm;
    let pitch = via_site_pitch(board);
    let edge_margin = size.div_euclid(2) + board.rules.minimum_clearance_nm;
    let half = tile_size_nm.div_euclid(2);
    if half < edge_margin {
        return Ok(Vec::new());
    }
    let steps = (half - edge_margin).div_euclid(pitch);
    let mut points = Vec::new();
    for dx in -steps..=steps {
        for dy in -steps..=steps {
            points.push(Point::new(center.x_nm + dx * pitch, center.y_nm + dy * pitch));
        }
    }
    points
        .sort_by_key(|item| ((item.x_nm - center.x_nm).abs() + (item.y_nm - center.y_nm).abs(), item.x_nm, item.y_nm));
    let outline = &board.outline.vertices;
    let margin_squared = BigRational::from_integer((i128::from(edge_margin) * i128::from(edge_margin)).into());
    let mut result = Vec::new();
    for point in points {
        if !point_in_polygon(point, outline) {
            continue;
        }
        if nearest_edge_distance_squared(point, outline) < margin_squared {
            continue;
        }
        if clearance.can_via("<global-via-site>", point, size, from_layer, to_layer, None, false, None)? {
            result.push(point);
        }
    }
    Ok(result)
}

fn nearest_edge_distance_squared(point: Point, outline: &[Point]) -> BigRational {
    (0..outline.len())
        .map(|index| point_segment_distance_squared(point, outline[index], outline[(index + 1) % outline.len()]))
        .min()
        .expect("outline edges")
}

const DIRECTIONS: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

/// `_pin_access_candidates`: checked physical entrances of one pad into the graph.
#[allow(clippy::too_many_arguments)]
pub fn pin_access_candidates(
    board: &PhysicalBoard,
    graph: &Graph,
    pad: &PadReference,
    net: &str,
    allowed_layers: &[CopperLayer],
    rule: Option<&NetRoutingRule>,
    clearance: &RoutingClearanceIndex<'_>,
    options: &GlobalRouterOptions,
) -> Result<Vec<PinAccess>> {
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let bounds = LocalAccessBounds {
        step_nm: options.escape_step_nm.min(250_000),
        detour_nm: options.escape_radius_nm,
        maximum_states: 600,
    };
    let max_distance = (options.tile_size_nm * 2).max(options.escape_radius_nm);
    if let Some(port) = macro_routing_ports(board, net)?.get(pad) {
        let layers: Vec<CopperLayer> =
            macro_port_layers(board, port)?.into_iter().filter(|layer| allowed_layers.contains(layer)).collect();
        let mut nearby: Vec<GridNode> = graph
            .legal_nodes
            .iter()
            .copied()
            .filter(|n| {
                layers.contains(&graph.layers[n.layer_index as usize])
                    && node_distance_nm(graph.point(*n), port.position) <= max_distance
            })
            .collect();
        nearby.sort_by_key(|n| (node_distance_nm(graph.point(*n), port.position), *n));
        let mut found = Vec::new();
        for node in nearby {
            let layer = graph.layers[node.layer_index as usize];
            let path = local_access_path(
                board,
                clearance,
                net,
                port.position,
                graph.point(node),
                layer,
                width,
                bounds,
                None,
                None,
            )?;
            if let Some(path) = path {
                let mut access = PinAccess::new(pad.clone(), port.position, node, graph.point(node), layer);
                access.tracks = path;
                found.push(access);
            }
            if found.len() as i64 >= options.pin_access_candidates {
                break;
            }
        }
        return Ok(found);
    }
    let placement = board
        .placements
        .iter()
        .find(|item| item.reference == pad.component)
        .ok_or_else(|| Error::Value(copper_json::python_str_repr(&pad.component)))?;
    let footprint = &board.footprints[&placement.footprint];
    let physical_pad = footprint
        .pads
        .iter()
        .find(|item| item.number == pad.pad)
        .ok_or_else(|| Error::Value("component has no such pad".to_owned()))?;
    let position = transformed_pad_position(board, placement, &pad.pad)?;
    let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
    let smd = physical_pad.kind == PadKind::Smd;
    if smd && !allowed_layers.contains(&side) {
        return Ok(Vec::new());
    }
    let direct_layers: Vec<CopperLayer> = if smd { vec![side] } else { allowed_layers.to_vec() };
    let mut nearby: Vec<GridNode> = graph
        .legal_nodes
        .iter()
        .copied()
        .filter(|node| {
            allowed_layers.contains(&graph.layers[node.layer_index as usize])
                && node_distance_nm(graph.point(*node), position) <= max_distance
        })
        .collect();
    nearby.sort_by_key(|node| (node_distance_nm(graph.point(*node), position), *node));
    let mut found: Vec<PinAccess> = Vec::new();
    for node in &nearby {
        let layer = graph.layers[node.layer_index as usize];
        if !direct_layers.contains(&layer) {
            continue;
        }
        let endpoint = graph.point(*node);
        // An ordinary breakout net may neck down next to its lands (plan R1).
        if position != endpoint && !clearance.can_route(net, position, endpoint, width, layer)? {
            continue;
        }
        let mut access = PinAccess::new(pad.clone(), position, *node, endpoint, layer);
        if position != endpoint {
            access.tracks = clearance.route_pieces(net, position, endpoint, width, layer)?;
        }
        found.push(access);
        if found.len() as i64 >= options.pin_access_candidates {
            break;
        }
    }
    // A legal local stem is not a usable graph entrance when its node has no
    // resource on this net's allowed layers. Prefer existing graph exits
    // before deciding whether the original bounded local fallback is needed.
    let allowed_indexes: IndexSet<i64> =
        allowed_layers.iter().filter_map(|layer| graph.layer_position(*layer)).collect();
    let has_graph_exit = |node: GridNode| !neighbors(graph, node, &allowed_indexes).is_empty();
    let (found_with_exit, stranded): (Vec<PinAccess>, Vec<PinAccess>) =
        found.into_iter().partition(|access| has_graph_exit(access.node));
    found = found_with_exit;
    if found.is_empty() {
        let mut local_nodes = Vec::new();
        for item in &nearby {
            let layer = graph.layers[item.layer_index as usize];
            if !direct_layers.contains(&layer) || !has_graph_exit(*item) {
                continue;
            }
            let point = graph.point(*item);
            let local_width =
                clearance.breakout.track_width_nm(net, clearance.breakout.region(net, &[point]).as_deref(), width);
            if clearance.can_track(net, point, point, local_width, layer)? {
                local_nodes.push(*item);
            }
        }
        let limit = options.pin_access_candidates.max(4) as usize;
        for node in local_nodes.iter().take(limit) {
            let layer = graph.layers[node.layer_index as usize];
            let endpoint = graph.point(*node);
            let tracks =
                local_access_path(board, clearance, net, position, endpoint, layer, width, bounds, None, None)?;
            if let Some(tracks) = tracks {
                let mut access = PinAccess::new(pad.clone(), position, *node, endpoint, layer);
                access.tracks = tracks;
                found.push(access);
            }
            if found.len() as i64 >= options.pin_access_candidates {
                break;
            }
        }
    }
    if found.is_empty() {
        // Registration may later add a checked terminal-via alias. Preserve
        // the original choices when no better exact local bridge was found.
        found = stranded;
    }
    let differential = rule.is_some_and(|r| r.kind == RouteKind::Differential);
    let no_vias = rule.is_some_and(|r| r.max_vias == Some(0));
    if smd && allowed_layers.len() > 1 && !differential && !no_vias {
        let margin = board.rules.default_via_size_nm.div_euclid(2) + board.rules.minimum_clearance_nm;
        let mut radius = options.escape_step_nm;
        while radius <= options.escape_radius_nm {
            for (dx, dy) in DIRECTIONS {
                let anchor = Point::new(position.x_nm + radius * dx, position.y_nm + radius * dy);
                if !inside_outline_with_margin(anchor, &board.outline.vertices, margin) {
                    continue;
                }
                if !clearance.can_route(net, position, anchor, width, side)? {
                    continue;
                }
                for layer in allowed_layers {
                    if *layer == side {
                        continue;
                    }
                    let Some(span) = physical_via_span(board, side, *layer) else { continue };
                    if !clearance.can_via(
                        net,
                        anchor,
                        board.rules.default_via_size_nm,
                        span.0,
                        span.1,
                        Some(board.rules.default_via_drill_nm),
                        false,
                        None,
                    )? {
                        continue;
                    }
                    let half = options.tile_size_nm.div_euclid(2);
                    let nodes: Vec<GridNode> = nearby
                        .iter()
                        .copied()
                        .filter(|node| {
                            graph.layers[node.layer_index as usize] == *layer
                                && (graph.point(*node).x_nm - anchor.x_nm).abs() <= half
                                && (graph.point(*node).y_nm - anchor.y_nm).abs() <= half
                        })
                        .collect();
                    let Some(node) =
                        nodes.iter().copied().min_by_key(|item| (node_distance_nm(graph.point(*item), anchor), *item))
                    else {
                        continue;
                    };
                    // This terminal transition is actual checked copper at
                    // anchor, not a sampled coarse-grid crossing. It ends at
                    // an existing legal node through its own exact lead-in.
                    let endpoint = graph.point(node);
                    if anchor != endpoint && !clearance.can_route(net, anchor, endpoint, width, *layer)? {
                        continue;
                    }
                    let mut tracks = clearance.route_pieces(net, position, anchor, width, side)?;
                    if anchor != endpoint {
                        tracks.extend(clearance.route_pieces(net, anchor, endpoint, width, *layer)?);
                    }
                    let via = Via {
                        net: net.to_owned(),
                        position: anchor,
                        size_nm: board.rules.default_via_size_nm,
                        drill_nm: board.rules.default_via_drill_nm,
                        from_layer: span.0,
                        to_layer: span.1,
                        technology: span.2.clone(),
                        finish: "standard".to_owned(),
                    };
                    let mut access = PinAccess::new(pad.clone(), position, node, endpoint, *layer);
                    access.tracks = tracks;
                    access.via = Some(via);
                    found.push(access);
                }
            }
            if found.len() as i64 >= options.pin_access_candidates * 3 {
                break;
            }
            radius += options.escape_step_nm;
        }
    }
    if found.is_empty() && rule.is_none_or(|r| r.kind == RouteKind::General) {
        // A coarse node denotes a routing region, not a compulsory track end.
        // Preserve a checked physical exit even when no straight/dogleg stub
        // reaches the center; detailed routing must close the remaining gap.
        let margin = width.div_euclid(2) + board.rules.minimum_clearance_nm;
        let half = options.tile_size_nm.div_euclid(2);
        let mut radius = options.escape_step_nm;
        while radius <= options.escape_radius_nm {
            for (dx, dy) in DIRECTIONS {
                let anchor = Point::new(position.x_nm + dx * radius, position.y_nm + dy * radius);
                if !inside_outline_with_margin(anchor, &board.outline.vertices, margin)
                    || !clearance.can_route(net, position, anchor, width, side)?
                {
                    continue;
                }
                let candidates: Vec<GridNode> = nearby
                    .iter()
                    .copied()
                    .filter(|node| {
                        graph.layers[node.layer_index as usize] == side
                            && (graph.point(*node).x_nm - anchor.x_nm).abs() <= half
                            && (graph.point(*node).y_nm - anchor.y_nm).abs() <= half
                    })
                    .collect();
                let Some(node) =
                    candidates.iter().copied().min_by_key(|item| (node_distance_nm(graph.point(*item), anchor), *item))
                else {
                    continue;
                };
                let mut access = PinAccess::new(pad.clone(), position, node, anchor, side);
                access.tracks = clearance.route_pieces(net, position, anchor, width, side)?;
                access.region_only = true;
                found.push(access);
            }
            if found.len() as i64 >= options.pin_access_candidates {
                break;
            }
            radius += options.escape_step_nm;
        }
    }
    Ok(bounded_access_choices(found, options.pin_access_candidates))
}

/// `_bounded_access_choices`: retain distinct physical launches before more aliases of one via.
fn bounded_access_choices(found: Vec<PinAccess>, limit: i64) -> Vec<PinAccess> {
    let mut ranked = found;
    ranked.sort_by_key(|item| {
        (
            item.estimated_cost_nm(),
            item.node,
            item.via.is_some(),
            item.via.as_ref().map_or(0, |via| via.position.x_nm),
            item.via.as_ref().map_or(0, |via| via.position.y_nm),
        )
    });
    #[derive(PartialEq)]
    enum Cohort {
        Via(Via),
        Surface(GridNode),
    }
    let mut cohorts: Vec<(Cohort, Vec<PinAccess>)> = Vec::new();
    for access in ranked {
        let key = match &access.via {
            Some(via) => Cohort::Via(via.clone()),
            None => Cohort::Surface(access.node),
        };
        match cohorts.iter_mut().find(|(cohort, _)| *cohort == key) {
            Some((_, group)) => group.push(access),
            None => cohorts.push((key, vec![access])),
        }
    }
    let mut result = Vec::new();
    let mut layers: IndexSet<CopperLayer> = IndexSet::new();
    let depth = cohorts.iter().map(|(_, group)| group.len()).max().unwrap_or(0);
    for _ in 0..depth {
        for (_, group) in cohorts.iter_mut() {
            if group.is_empty() {
                continue;
            }
            // Cover an unrepresented legal layer from this existing launch.
            let index = (0..group.len()).min_by_key(|i| (layers.contains(&group[*i].layer), *i)).expect("group");
            let chosen = group.remove(index);
            layers.insert(chosen.layer);
            result.push(chosen);
            if result.len() as i64 >= limit {
                return result;
            }
        }
    }
    result
}

pub fn node_distance_nm(first: Point, second: Point) -> i64 {
    (first.x_nm - second.x_nm).abs() + (first.y_nm - second.y_nm).abs()
}

fn inside_outline_with_margin(point: Point, outline: &[Point], margin: i64) -> bool {
    point_in_polygon(point, outline)
        && nearest_edge_distance_squared(point, outline)
            >= BigRational::from_integer((i128::from(margin) * i128::from(margin)).into())
}

pub fn via_resource_id(graph: &Graph, node: GridNode, first: CopperLayer, second: CopperLayer) -> String {
    let (a, b) = (graph.layer_position(first).expect("layer"), graph.layer_position(second).expect("layer"));
    format!("via:{}:{}:{}:{}", a.min(b), a.max(b), node.x_index, node.y_index)
}

/// `_neighbors`: planar neighbours in `(-1,0),(0,-1),(0,1),(1,0)` order, then ascending layers.
pub fn neighbors(graph: &Graph, node: GridNode, allowed_indexes: &IndexSet<i64>) -> Vec<GridNode> {
    let mut result = Vec::new();
    for (dx, dy) in [(-1, 0), (0, -1), (0, 1), (1, 0)] {
        let other = GridNode::new(node.layer_index, node.x_index + dx, node.y_index + dy);
        if !allowed_indexes.contains(&other.layer_index) || !graph.legal_nodes.contains(&other) {
            continue;
        }
        if graph.resources.contains_key(&edge_key(node, other)) {
            result.push(other);
        }
    }
    let mut layers: Vec<i64> = allowed_indexes.iter().copied().collect();
    layers.sort_unstable();
    for layer_index in layers {
        if layer_index == node.layer_index {
            continue;
        }
        let other = GridNode::new(layer_index, node.x_index, node.y_index);
        if graph.legal_nodes.contains(&other) && graph.resources.contains_key(&edge_key(node, other)) {
            result.push(other);
        }
    }
    result
}

/// `_net_order`: hardness, descending priority, then name.
pub fn net_order(net: &str, rule: Option<&NetRoutingRule>) -> (i64, i64, String) {
    let kind = rule.map_or(RouteKind::General, |r| r.kind);
    let hardness = match kind {
        RouteKind::Differential => 0,
        RouteKind::RfFeed => 1,
        RouteKind::Clock => 2,
        RouteKind::CanBus => 3,
        RouteKind::Critical => 4,
        RouteKind::Power => 5,
        RouteKind::General => 6,
    };
    (hardness, -rule.map_or(0, |r| r.priority), net.to_owned())
}

/// `_route_resource_demands`: trace width consumes lanes; each distinct via position one site.
pub fn route_resource_demands(
    route: &GlobalNetRoute,
    planar_demand: i64,
    local_demand: Option<&PlanarDemand<'_>>,
) -> IndexMap<String, i64> {
    let mut result: IndexMap<String, i64> = IndexMap::new();
    for segment in &route.segments {
        let demand = match local_demand {
            Some(local) => local.demand(segment.start, segment.end),
            None => planar_demand,
        };
        result.insert(segment.resource_id.clone(), demand);
    }
    let mut sites: IndexSet<(String, Point)> = IndexSet::new();
    for via in &route.vias {
        sites.insert((via.resource_id.clone(), via.position));
    }
    for (identifier, _) in sites {
        *result.entry(identifier).or_insert(0) += 1;
    }
    result.sort_keys();
    result
}

/// `_net_demand`: lanes one track of this net consumes on a coarse edge.
pub fn net_demand(board: &PhysicalBoard, rule: Option<&NetRoutingRule>, _options: &GlobalRouterOptions) -> i64 {
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let clearance = rule.and_then(|r| r.clearance_nm).filter(|c| *c != 0).unwrap_or(board.rules.minimum_clearance_nm);
    let pitch = board.rules.default_track_width_nm + board.rules.minimum_clearance_nm;
    (((width + clearance) as f64 / pitch as f64).ceil() as i64).max(1)
}

fn hotspots(
    graph: &Graph,
    usage: &IndexMap<String, i64>,
    contributors: &IndexMap<String, Vec<String>>,
) -> Vec<RoutingHotspot> {
    let mut sorted: Vec<&Resource> = graph.resources.values().collect();
    sorted.sort_by(|a, b| a.identifier.cmp(&b.identifier));
    let mut result = Vec::new();
    let mut seen: IndexSet<&str> = IndexSet::new();
    for resource in sorted {
        if !seen.insert(&resource.identifier) {
            continue;
        }
        let used = usage[&resource.identifier];
        let overflow = (used - resource.capacity).max(0);
        if overflow == 0 {
            continue;
        }
        result.push(RoutingHotspot {
            resource_id: resource.identifier.clone(),
            layer: resource.layer,
            start: graph.point(resource.first),
            end: graph.point(resource.second),
            capacity: resource.capacity,
            usage: used,
            overflow,
            contributors: contributors[&resource.identifier].clone(),
        });
    }
    result
}

/// `_axis_layout`: tile centres along one board axis, and the band of a partial edge tile.
///
/// Centres start half a tile in from `start` and step by whole tiles. When the
/// last centre is more than three quarters of a tile from `end`, one more
/// centre sits in the middle of the uncovered strip; when the last tile
/// overhangs `end` and its centre lies within a quarter tile of it, that
/// centre moves to the middle of the tile's part inside the board.
pub fn axis_layout(start: i64, end: i64, step: i64, edge_strip: bool) -> (Vec<i64>, Option<(i64, i64)>) {
    let first = start + step.div_euclid(2).min((end - start).max(0).div_euclid(2));
    let mut values = Vec::new();
    let mut value = first;
    while value < end {
        values.push(value);
        value += step;
    }
    if values.is_empty() {
        return (vec![(start + end).div_euclid(2)], None);
    }
    let last = *values.last().expect("values");
    let covered = last + step.div_euclid(2);
    if edge_strip && end - last > (3 * step).div_euclid(4) && covered < end {
        values.push((covered + end).div_euclid(2));
        return (values, Some((covered, end)));
    }
    if edge_strip && covered > end && end - last < step.div_euclid(4) && values.len() > 1 {
        let low = last - step.div_euclid(2);
        values.pop();
        values.push((low + end).div_euclid(2));
        return (values, Some((low, end)));
    }
    (values, None)
}

pub fn edge_key(first: GridNode, second: GridNode) -> (GridNode, GridNode) {
    if first < second { (first, second) } else { (second, first) }
}

pub fn direction(first: GridNode, second: GridNode) -> Direction {
    if first.layer_index != second.layer_index {
        Direction::V
    } else if first.y_index == second.y_index {
        Direction::H
    } else {
        Direction::N
    }
}

fn node_distance(first: GridNode, second: GridNode) -> i64 {
    (first.x_index - second.x_index).abs()
        + (first.y_index - second.y_index).abs()
        + (first.layer_index - second.layer_index).abs()
}

fn heuristic(graph: &Graph, first: GridNode, second: GridNode) -> i64 {
    let (a, b) = (graph.point(first), graph.point(second));
    10 * ((a.x_nm - b.x_nm).abs() + (a.y_nm - b.y_nm).abs())
}

fn j_text(value: &str) -> Json {
    text(value)
}

fn j_strings<T>(items: &[T], key: impl Fn(&T) -> String, repr: impl Fn(&T) -> String) -> Json {
    let mut sorted: Vec<&T> = items.iter().collect();
    sorted.sort_by_key(|item| key(item));
    Json::Array(sorted.into_iter().map(|item| j_text(&repr(item))).collect())
}

/// `_placement_fingerprint`: a digest of placement and topology identity.
pub fn placement_fingerprint(board: &PhysicalBoard) -> String {
    let mut document: Vec<(&str, Json)> = vec![("board", j_text(&board.name))];
    if !board.decoupling_links.is_empty() {
        document.push((
            "decoupling_links",
            Json::Array(board.decoupling_links.iter().map(|link| j_text(&reprs::decoupling_link(link))).collect()),
        ));
    }
    if board.zones.iter().any(|zone| zone.reserve_routing) {
        let mut zones: Vec<_> = board.zones.iter().collect();
        zones.sort_by(|a, b| a.id.cmp(&b.id));
        document.push((
            "zone_routing_reservations",
            Json::Array(zones.into_iter().filter(|z| z.reserve_routing).map(|z| j_text(&reprs::zone(z))).collect()),
        ));
    }
    document.push(("rigid_clusters", j_strings(&board.rigid_clusters, |c| c.name.clone(), reprs::rigid_cluster)));
    document.push(("hard_macros", j_strings(&board.hard_macros, |m| m.cluster.clone(), reprs::hard_macro)));
    let mut via_in_pad: Vec<_> = board.via_in_pad_rules.iter().collect();
    via_in_pad.sort_by(|a, b| a.pad.cmp(&b.pad));
    document.push((
        "via_in_pad_rules",
        Json::Array(via_in_pad.into_iter().map(|r| j_text(&reprs::via_in_pad_rule(r))).collect()),
    ));
    // Materialization does not change placement or topology identity.
    document.push(("outline", Json::Array(board.outline.vertices.iter().map(|p| j_point(*p)).collect())));
    document.push(("circular_boundary", j_text(&reprs::circular_boundary(&board.outline))));
    document.push(("boundary_path", j_text(&reprs::boundary_path(&board.outline))));
    document.push((
        "cutouts",
        Json::Array(board.outline.cutouts.iter().map(|c| j_text(&reprs::board_cutout(c))).collect()),
    ));
    document.push(("mechanical_holes", j_strings(&board.mechanical_holes, |h| h.id.clone(), reprs::mechanical_hole)));
    document.push(("mechanical_slots", j_strings(&board.mechanical_slots, |s| s.id.clone(), reprs::mechanical_slot)));
    document.push((
        "placements",
        Json::Array(
            board
                .placements
                .iter()
                .map(|item| {
                    Json::Array(vec![
                        j_text(&item.reference),
                        j_text(&item.footprint),
                        int(item.position.x_nm),
                        int(item.position.y_nm),
                        j_text(&item.rotation_degrees.to_sci_string()),
                        j_text(item.side.value()),
                    ])
                })
                .collect(),
        ),
    ));
    document.push((
        "rules",
        Json::Array(
            board
                .net_routing_rules
                .iter()
                .map(|item| {
                    Json::Array(vec![
                        j_text(&item.net),
                        j_text(item.kind.value()),
                        int(item.priority),
                        item.width_nm.map_or(Json::Null, int),
                        item.clearance_nm.map_or(Json::Null, int),
                        Json::Array(item.allowed_layers.iter().map(|l| j_text(l.value())).collect()),
                        item.max_vias.map_or(Json::Null, int),
                    ])
                })
                .collect(),
        ),
    ));
    sha256_hex(&Dumps::new().sort_keys(true).dumps(&j_object(document)))
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// `_routing_fingerprint`: a digest of the routes, quality and options.
pub fn routing_fingerprint(
    placement_fingerprint: &str,
    routes: &[GlobalNetRoute],
    metrics: &GlobalRoutingMetrics,
    options: &GlobalRouterOptions,
    fixed_prefix_digest: Option<&str>,
) -> String {
    let mut document: Vec<(&str, Json)> = vec![("placement", j_text(placement_fingerprint))];
    if let Some(digest) = fixed_prefix_digest.filter(|d| !d.is_empty()) {
        document.push(("fixed_prefix_digest", j_text(digest)));
    }
    let routes = routes
        .iter()
        .map(|route| {
            let accesses = route
                .accesses
                .iter()
                .map(|access| {
                    let mut entry = vec![
                        j_text(&access.pad.component),
                        j_text(&access.pad.pad),
                        int(access.node.layer_index),
                        int(access.node.x_index),
                        int(access.node.y_index),
                        Json::Bool(access.region_only),
                        access.via.as_ref().map_or(Json::Null, |via| int(via.position.x_nm)),
                        access.via.as_ref().map_or(Json::Null, |via| int(via.position.y_nm)),
                    ];
                    if let Some(retained) = &access.retained_via {
                        entry.push(j_text(&reprs::via(retained)));
                    }
                    Json::Array(entry)
                })
                .collect();
            Json::Array(vec![
                j_text(&route.net),
                Json::Bool(route.connected),
                Json::Bool(route.deferred_to_zone),
                Json::Array(accesses),
                Json::Array(route.segments.iter().map(|s| j_text(&s.resource_id)).collect()),
                Json::Array(route.vias.iter().map(|v| j_text(&v.resource_id)).collect()),
            ])
        })
        .collect();
    document.push(("routes", Json::Array(routes)));
    document.push(("quality", Json::Array(metrics.quality_vector().iter().map(|v| int(*v)).collect())));
    document.push((
        "options",
        Json::Array(
            [
                options.tile_size_nm,
                options.maximum_iterations,
                options.present_penalty,
                options.present_penalty_growth,
                options.history_penalty,
                options.via_cost,
                options.bend_cost,
                options.layer_preference_cost,
                options.direction_preference_cost,
                options.guide_half_width_tiles,
                options.pin_access_candidates,
                options.escape_radius_nm,
                options.escape_step_nm,
            ]
            .into_iter()
            .map(int)
            .collect(),
        ),
    ));
    sha256_hex(&Dumps::new().sort_keys(true).dumps(&j_object(document)))
}

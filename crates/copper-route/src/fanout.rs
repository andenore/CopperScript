//! Package pad escapes before routing (`pcbir.fanout`).

use std::collections::BTreeSet;

use copper_pcb::board::PadReference;

use crate::escape_assignment::EscapeAssignmentOptions;
use crate::{Error, Result, nm_from_mm};

/// `FanoutOptions`.
#[derive(Clone, Debug, PartialEq)]
pub struct FanoutOptions {
    pub minimum_component_pads: i64,
    pub escape_small_dense_packages: bool,
    pub maximum_neighbor_distance_nm: i64,
    pub step_nm: i64,
    pub maximum_radius_nm: i64,
    pub constrained_pins_first: bool,
    pub two_leg_escapes: bool,
    pub maximum_two_leg_candidates: i64,
    pub joint_escapes: bool,
    pub assignment_options: EscapeAssignmentOptions,
    pub refinement_step_nm: i64,
    pub maximum_refined_two_leg_candidates: i64,
    pub maze_escapes: bool,
    pub maze_state_budget: i64,
    pub surface_fallback: bool,
    /// Runtime planning policy for early ordinary reservations. Prefer a
    /// checked surface channel before adding a transition; keep via
    /// alternatives for joint conflicts. Source layer/width/length rules
    /// remain authoritative.
    pub prefer_surface_escapes: bool,
    pub surface_preference_depth: Option<i64>,
    /// Pins an escape plan keeps on their own surface (escape planning E3):
    /// a checked surface escape first, as `prefer_surface_escapes`, at any depth.
    pub surface_pads: BTreeSet<PadReference>,
    /// Runtime policy: keep these source associations pending for the native
    /// power-preparation ledger. This alone never certifies a bypass connection.
    pub plane_return_nets: BTreeSet<String>,
    pub diagnostic_samples_per_pad: i64,
    /// Experimental extra site family. Zero retains established generation work.
    pub maximum_geometry_candidates: i64,
}

impl Default for FanoutOptions {
    fn default() -> Self {
        FanoutOptions {
            minimum_component_pads: 12,
            escape_small_dense_packages: true,
            maximum_neighbor_distance_nm: 1_500_000,
            step_nm: 500_000,
            maximum_radius_nm: nm_from_mm(3),
            constrained_pins_first: true,
            two_leg_escapes: true,
            maximum_two_leg_candidates: 256,
            joint_escapes: true,
            assignment_options: EscapeAssignmentOptions::default(),
            refinement_step_nm: 100_000,
            maximum_refined_two_leg_candidates: 1500,
            maze_escapes: false,
            maze_state_budget: 12_000,
            surface_fallback: true,
            prefer_surface_escapes: false,
            surface_preference_depth: None,
            surface_pads: BTreeSet::new(),
            plane_return_nets: BTreeSet::new(),
            diagnostic_samples_per_pad: 24,
            maximum_geometry_candidates: 0,
        }
    }
}

impl FanoutOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        let fail = |message: &str| Err(Error::Value(message.to_owned()));
        if self.prefer_surface_escapes && !self.surface_fallback {
            return fail("surface preference requires enabled surface fallback");
        }
        if self.surface_preference_depth.is_some_and(|depth| depth < 0) {
            return fail("surface preference depth must be a nonnegative integer or None");
        }
        if [
            self.minimum_component_pads,
            self.maximum_neighbor_distance_nm,
            self.step_nm,
            self.maximum_radius_nm,
            self.maximum_two_leg_candidates,
            self.refinement_step_nm,
            self.maximum_refined_two_leg_candidates,
            self.maze_state_budget,
        ]
        .into_iter()
        .min()
        .expect("bounds")
            <= 0
        {
            return fail("fanout options must be positive");
        }
        if self.refinement_step_nm > self.step_nm {
            return fail("fanout refinement step must not exceed coarse step");
        }
        if self.diagnostic_samples_per_pad < 0 {
            return fail("fanout diagnostic samples must be a nonnegative integer");
        }
        if self.maximum_geometry_candidates < 0 {
            return fail("fanout geometry candidates must be a nonnegative integer");
        }
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// Package escapes

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{DecouplingLink, PhysicalBoard, Placement, RouteKind, TrackSegment, Via};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::zones::{distribution_zone_nets, routing_layers};
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::geometry::{point_in_polygon, point_segment_distance_at_least, shapes_clear};
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};

use crate::access_sites::{circular_array_depth, circular_cell_sites, obstacle_boundary_sites};
use crate::boundary_access::{BoundaryAccessOptions, package_collar, surface_escapes};
use crate::bypass_links::verified_bypass_links;
use crate::decoupling::DecouplingRouteResult;
use crate::decoupling_paths::{DecouplingCrossLayerWitness, refresh_crosslayer_witnesses};
use crate::escape_assignment::{EscapeAssignmentReport, EscapeCandidate, improve_escape_assignment};
use crate::pin_escape::{RoutingAccess, checked_access_path, checked_access_paths};
use crate::return_paths::existing_returns_permit;
use crate::route_quality::escape_paths_cross;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_vias::physical_via_span;
use crate::surface_path::{ViaSearch, surface_path_to_via};

/// `FanoutResult` (pin analysis and access diagnostics are not modelled).
#[derive(Clone, Debug)]
pub struct FanoutResult {
    pub board: PhysicalBoard,
    pub accesses: IndexMap<PadReference, Point>,
    pub pending_pads: Vec<PadReference>,
    pub added_track_count: i64,
    pub added_via_count: i64,
    pub created_vias: Vec<Via>,
    pub created_tracks: Vec<TrackSegment>,
    pub assignment: Option<EscapeAssignmentReport>,
    pub surface_accesses: Option<IndexMap<PadReference, RoutingAccess>>,
    /// Exact generated semantic feeds, separate from unrelated same-net launches.
    pub decoupling_tracks: Vec<TrackSegment>,
    /// Exact generated semantic supply/return transitions.
    pub decoupling_vias: Vec<Via>,
    pub decoupling_witnesses: Vec<DecouplingCrossLayerWitness>,
    pub decoupling_pending_links: Vec<(DecouplingLink, String)>,
}

impl FanoutResult {
    fn unchanged(board: &PhysicalBoard, pending: Vec<PadReference>) -> Self {
        FanoutResult {
            board: board.clone(),
            accesses: IndexMap::new(),
            pending_pads: pending,
            added_track_count: 0,
            added_via_count: 0,
            created_vias: Vec::new(),
            created_tracks: Vec::new(),
            assignment: None,
            surface_accesses: None,
            decoupling_tracks: Vec::new(),
            decoupling_vias: Vec::new(),
            decoupling_witnesses: Vec::new(),
            decoupling_pending_links: Vec::new(),
        }
    }

    /// The bypass pass's own copper and ledger on an unchanged fanout.
    fn bypass(bypass: &DecouplingRouteResult, pending: Vec<PadReference>) -> Self {
        let mut result = FanoutResult::unchanged(&bypass.board, pending);
        result.accesses = bypass.accesses.clone();
        result.added_track_count = bypass.created_tracks.len() as i64;
        result.added_via_count = bypass.created_vias.len() as i64;
        result.created_tracks = bypass.created_tracks.clone();
        result.created_vias = bypass.created_vias.clone();
        result.decoupling_tracks = bypass.created_tracks.clone();
        result.decoupling_vias = bypass.created_vias.clone();
        result.decoupling_witnesses = bypass.admitted_path_witnesses.clone();
        result.decoupling_pending_links = bypass.pending.clone();
        result
    }
}

fn side_of(placement: &Placement) -> CopperLayer {
    if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }
}

/// `_OutlineSites`: exact inside-and-margin tests of via sites, cached.
pub(crate) struct OutlineSites {
    vertices: Vec<Point>,
    admitted: std::cell::RefCell<IndexMap<(i64, i64, i64), bool>>,
}

impl OutlineSites {
    pub(crate) fn new(vertices: &[Point]) -> Self {
        OutlineSites { vertices: vertices.to_vec(), admitted: std::cell::RefCell::new(IndexMap::new()) }
    }

    fn admits(&self, point: Point, margin: i64) -> bool {
        let key = (point.x_nm, point.y_nm, margin);
        if let Some(answer) = self.admitted.borrow().get(&key) {
            return *answer;
        }
        let answer = point_in_polygon(point, &self.vertices) && self.clear(point, margin);
        self.admitted.borrow_mut().insert(key, answer);
        answer
    }

    fn clear(&self, point: Point, margin: i64) -> bool {
        let (x, y) = (point.x_nm, point.y_nm);
        for (index, first) in self.vertices.iter().enumerate() {
            let second = self.vertices[(index + 1) % self.vertices.len()];
            if x < first.x_nm.min(second.x_nm) - margin
                || x > first.x_nm.max(second.x_nm) + margin
                || y < first.y_nm.min(second.y_nm) - margin
                || y > first.y_nm.max(second.y_nm) + margin
            {
                continue;
            }
            if !point_segment_distance_at_least(point, *first, second, margin, 1) {
                return false;
            }
        }
        true
    }
}

/// `_candidates`: outward normal rays before diagonal fallback.
fn ray_candidates(position: Point, center: Point, options: &FanoutOptions, diagonal_normal: bool) -> Vec<Point> {
    let directions = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
    let rank = |pair: (i64, i64)| {
        let outward = i128::from(pair.0) * i128::from(position.x_nm - center.x_nm)
            + i128::from(pair.1) * i128::from(position.y_nm - center.y_nm);
        let diagonal = pair.0 != 0 && pair.1 != 0;
        (outward <= 0, diagonal != diagonal_normal, -outward, pair)
    };
    let mut sorted = directions.to_vec();
    sorted.sort_by_key(|pair| rank(*pair));
    let mut groups: Vec<(bool, bool)> = sorted.iter().map(|pair| (rank(*pair).0, rank(*pair).1)).collect();
    groups.sort();
    groups.dedup();
    let mut points = Vec::new();
    for group in groups {
        for step in 1..=options.maximum_radius_nm.div_euclid(options.step_nm) {
            for (dx, dy) in sorted.iter().filter(|pair| (rank(**pair).0, rank(**pair).1) == group) {
                points.push(Point::new(
                    position.x_nm + dx * step * options.step_nm,
                    position.y_nm + dy * step * options.step_nm,
                ));
            }
        }
    }
    points
}

/// `_package_candidates`: diagonal dogbones before outward rays.
pub(crate) fn package_candidates(
    board: &PhysicalBoard,
    placement: &Placement,
    pad: &FootprintPad,
    options: &FanoutOptions,
) -> Vec<Point> {
    let sites =
        circular_cell_sites(board, placement, pad, options.maximum_neighbor_distance_nm, options.maximum_radius_nm);
    let start = transformed_local_point(placement, pad.position);
    let diagonal = placement
        .rotation_degrees
        .rem(&copper_units::Decimal::from_i64(90))
        .is_ok_and(|r| r == copper_units::Decimal::from_i64(45));
    let mut points = sites.clone();
    for point in ray_candidates(start, placement.position, options, diagonal) {
        if !sites.contains(&point) {
            points.push(point);
        }
    }
    points
}

/// `_two_leg_candidates`: bounded off-ray sites on a half-step lattice.
pub(crate) fn two_leg_candidates(position: Point, center: Point, options: &FanoutOptions) -> Vec<Point> {
    let step = 1.max(options.step_nm.div_euclid(2));
    let mut points = Vec::new();
    for ring in 1..=options.maximum_radius_nm.div_euclid(step) {
        let mut offsets: IndexSet<(i64, i64)> = IndexSet::new();
        for along in (-ring + 1)..ring {
            for (dx, dy) in [(ring, along), (-ring, along), (along, ring), (along, -ring)] {
                if dx != 0 && dy != 0 && dx.abs() != dy.abs() {
                    offsets.insert((dx, dy));
                }
            }
        }
        let mut offsets: Vec<(i64, i64)> = offsets.into_iter().collect();
        offsets.sort_by_key(|(dx, dy)| {
            (
                dx * dx + dy * dy,
                -(i128::from(*dx) * i128::from(position.x_nm - center.x_nm)
                    + i128::from(*dy) * i128::from(position.y_nm - center.y_nm)),
                (*dx, *dy),
            )
        });
        for (dx, dy) in offsets {
            if points.len() as i64 >= options.maximum_two_leg_candidates {
                return points;
            }
            points.push(Point::new(position.x_nm + dx * step, position.y_nm + dy * step));
        }
    }
    points
}

fn unique(choices: Vec<EscapeCandidate>) -> Vec<EscapeCandidate> {
    let mut result: Vec<EscapeCandidate> = Vec::new();
    for choice in choices {
        if !result.contains(&choice) {
            result.push(choice);
        }
    }
    result
}

/// `_legal_choices`: exact-checked dogbone candidates at the given via sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn legal_choices(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    position: Point,
    placement: &Placement,
    endpoints: &[Point],
    multiple_orders: bool,
    outline: &OutlineSites,
) -> Result<Vec<EscapeCandidate>> {
    let rule = board.net_routing_rules.iter().find(|r| r.net == net);
    let side = side_of(placement);
    let allowed = routing_layers(board, net, rule);
    if !allowed.contains(&side) || allowed.len() < 2 {
        return Ok(Vec::new());
    }
    let other = *allowed.iter().find(|layer| **layer != side).expect("two layers");
    let Some(span) = physical_via_span(board, side, other) else { return Ok(Vec::new()) };
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    let mut choices = Vec::new();
    for endpoint in endpoints {
        let margin = size.div_euclid(2) + board.rules.minimum_clearance_nm;
        if !outline.admits(*endpoint, margin) {
            continue;
        }
        let coincident: Vec<&Via> = board.vias.iter().filter(|v| v.position == *endpoint).collect();
        let existing = coincident.iter().find(|v| v.net == net && (v.from_layer, v.to_layer) == (span.0, span.1));
        if !coincident.is_empty() && existing.is_none() {
            continue;
        }
        let via = if existing.is_some() {
            None
        } else {
            Some(Via {
                net: net.to_owned(),
                position: *endpoint,
                size_nm: size,
                drill_nm: drill,
                from_layer: span.0,
                to_layer: span.1,
                technology: span.2.clone(),
                finish: "standard".to_owned(),
            })
        };
        let checked = via.as_ref().or(existing.copied()).expect("a via");
        if !existing_returns_permit(board, rule, checked) {
            continue;
        }
        if via.is_some() && !clearance.can_via(net, *endpoint, size, span.0, span.1, Some(drill), false, None)? {
            continue;
        }
        let paths = if multiple_orders {
            checked_access_paths(board, clearance, net, position, *endpoint, width, side, false, false, None, false)?
        } else {
            checked_access_path(board, clearance, net, position, *endpoint, width, side, false, false, None)?
                .into_iter()
                .collect()
        };
        for path in paths {
            if !path.is_empty() {
                choices.push((path, via.clone()));
            }
        }
    }
    Ok(unique(choices))
}

/// One pin of a crowded package: (neighbor distance, reference, position, placement, pad).
type Pin<'a> = (i64, PadReference, Point, &'a Placement, &'a FootprintPad);

/// `_maze_choices`: bounded multi-bend launches when straight/elbow domains fail.
fn maze_choices(
    board: &PhysicalBoard,
    clearance: &RoutingClearanceIndex<'_>,
    net: &str,
    pins: &[Pin<'_>],
    options: &FanoutOptions,
) -> Result<Vec<EscapeCandidate>> {
    let rule = board.net_routing_rules.iter().find(|r| r.net == net);
    let width = rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let (size, drill) = (board.rules.default_via_size_nm, board.rules.default_via_drill_nm);
    let mut choices = Vec::new();
    for (_, _, position, placement, pad) in pins {
        let side = side_of(placement);
        let allowed = routing_layers(board, net, rule);
        if !allowed.contains(&side) || allowed.len() < 2 {
            continue;
        }
        let other = *allowed.iter().find(|layer| **layer != side).expect("two layers");
        let Some(span) = physical_via_span(board, side, other) else { continue };
        let pad_shape = placed_pad_shape(*position, pad, placement);
        let via_at = |p: Point| Via {
            net: net.to_owned(),
            position: p,
            size_nm: size,
            drill_nm: drill,
            from_layer: span.0,
            to_layer: span.1,
            technology: span.2.clone(),
            finish: "standard".to_owned(),
        };
        let accept = |p: Point| -> bool {
            RoundedConvexShape::new(vec![p], size.div_euclid(2)).is_ok_and(|shape| shapes_clear(&shape, &pad_shape, 1))
                && existing_returns_permit(board, rule, &via_at(p))
        };
        let search = ViaSearch {
            step_nm: options.refinement_step_nm,
            radius_nm: options.maximum_radius_nm,
            via_size_nm: size,
            via_drill_nm: drill,
            via_layers: (span.0, span.1),
            accept_via: &accept,
            via_targets: &[],
            state_budget: options.maze_state_budget,
        };
        if let Some((path, endpoint)) = surface_path_to_via(board, clearance, net, *position, width, side, &search)? {
            choices.push((path, Some(via_at(endpoint))));
        }
    }
    Ok(unique(choices))
}

/// `_reserve`: commit one candidate when it still clears the reserved copper.
fn reserve(
    board: &PhysicalBoard,
    clearance: &mut RoutingClearanceIndex<'_>,
    tracks: &mut Vec<TrackSegment>,
    vias: &mut Vec<Via>,
    candidate: &EscapeCandidate,
) -> Result<bool> {
    let (path, via) = candidate;
    if let Some(via) = via {
        let rule = board.net_routing_rules.iter().find(|r| r.net == via.net);
        if !existing_returns_permit(board, rule, via) {
            return Ok(false);
        }
    }
    if escape_paths_cross(path, tracks) {
        return Ok(false);
    }
    let mut via = via.clone();
    if let Some(v) = &via
        && board.vias.iter().chain(vias.iter()).any(|o| {
            o.net == v.net && o.position == v.position && (o.from_layer, o.to_layer) == (v.from_layer, v.to_layer)
        })
    {
        via = None;
    }
    for t in path {
        if !clearance.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)? {
            return Ok(false);
        }
    }
    if let Some(v) = &via
        && !clearance.can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, Some(v.drill_nm), false, None)?
    {
        return Ok(false);
    }
    for track in path {
        if !board.tracks.contains(track) && !tracks.contains(track) {
            tracks.push(track.clone());
            clearance.add_track(track.clone(), true, None)?;
        }
    }
    if let Some(v) = via {
        vias.push(v.clone());
        clearance.add_via(v, true, None)?;
    }
    Ok(true)
}

/// `_surface_layer`: the pad's own layer when its net may not leave it.
fn surface_layer(
    board: &PhysicalBoard,
    net: &str,
    rule: Option<&copper_pcb::board::NetRoutingRule>,
    placement: &Placement,
) -> Option<CopperLayer> {
    let side = side_of(placement);
    let allowed = routing_layers(board, net, rule);
    (allowed.contains(&side) && (allowed.len() == 1 || rule.is_some_and(|r| r.max_vias == Some(0)))).then_some(side)
}

/// `route_fanout`: escape crowded ordinary-net SMD pads to legal through-vias.
/// Boards with source decoupling links (bypass routing first) report not implemented.
pub fn route_fanout(
    board: &PhysicalBoard,
    options: Option<&FanoutOptions>,
    only_nets: Option<&IndexSet<String>>,
) -> Result<FanoutResult> {
    if !board.hard_macros.is_empty() && board.materialized_macros.is_empty() {
        return Err(Error::Value("materialize hard macros before package escape".to_owned()));
    }
    crate::pad_via_arrays::require_via_in_pad_arrays(board, "package escape")?;
    let options = options.cloned().unwrap_or_default();
    if let Some(only) = only_nets {
        let names: IndexSet<&str> = board.nets.iter().map(|n| n.name.as_str()).collect();
        let mut unknown: Vec<&String> = only.iter().filter(|n| !names.contains(n.as_str())).collect();
        if !unknown.is_empty() {
            unknown.sort();
            let unknown: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
            return Err(Error::Value(format!("unknown fanout nets: {}", unknown.join(", "))));
        }
        if only.is_empty() {
            return Ok(FanoutResult::unchanged(board, Vec::new()));
        }
    }
    if board.stackup.copper_layers.len() < 2 {
        return Ok(FanoutResult::unchanged(board, Vec::new()));
    }
    let bypass = crate::decoupling::route_decouplers(board, only_nets, Some(&options), None, None)?;
    let board = &bypass.board;
    let bypass_nets: IndexSet<String> = board.decoupling_links.iter().map(|l| l.net.clone()).collect();
    let protected_bypasses = if bypass_nets.is_empty() {
        IndexSet::new()
    } else {
        verified_bypass_links(board, &bypass_nets, None, None, &[])?
    };
    let mut clearance = RoutingClearanceIndex::new(board)?;
    let mut net_by_pad: IndexMap<PadReference, String> = IndexMap::new();
    for net in &board.nets {
        if net.pads.len() >= 2 {
            for pad in &net.pads {
                net_by_pad.insert(pad.clone(), net.name.clone());
            }
        }
    }
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let regional: IndexSet<&str> = board.zones.iter().filter(|z| z.reserve_routing).map(|z| z.net.as_str()).collect();
    let zone_nets: IndexSet<String> =
        distribution_zone_nets(board).into_iter().filter(|n| !regional.contains(n.as_str())).collect();
    let owned = crate::pair_terminals::macro_owned_pads(board);
    let mut pads: Vec<Pin<'_>> = Vec::new();
    for placement in &board.placements {
        let footprint = &board.footprints[&placement.footprint];
        let electrical: IndexSet<&str> = footprint
            .pads
            .iter()
            .filter(|p| matches!(p.kind, PadKind::Smd | PadKind::ThroughHole))
            .map(|p| p.number.as_str())
            .collect();
        let count = electrical.len() as i64;
        if count < options.minimum_component_pads && !(options.escape_small_dense_packages && count >= 3) {
            continue;
        }
        let surface: Vec<&FootprintPad> = footprint.pads.iter().filter(|p| p.kind == PadKind::Smd).collect();
        for pad in &surface {
            let mut reference = PadReference { component: placement.reference.clone(), pad: pad.number.clone() };
            if let Some(group) = footprint.internal_pad_groups.iter().find(|g| g.numbers.contains(&pad.number)) {
                let members: Vec<PadReference> = group
                    .numbers
                    .iter()
                    .map(|n| PadReference { component: placement.reference.clone(), pad: n.clone() })
                    .collect();
                if members.iter().any(|m| owned.contains(m)) {
                    continue;
                }
                if let Some(member) = members.iter().find(|m| net_by_pad.contains_key(*m)) {
                    reference = member.clone();
                }
            }
            let Some(net) = net_by_pad.get(&reference) else { continue };
            if owned.contains(&reference)
                || bypass.connected_pads.contains(&reference)
                || zone_nets.contains(net)
                || only_nets.is_some_and(|only| !only.contains(net))
            {
                continue;
            }
            if rules.get(net.as_str()).is_some_and(|rule| rule.kind != RouteKind::General) {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            let neighbor = surface
                .iter()
                .filter(|other| other.number != pad.number)
                .map(|other| {
                    let point = transformed_local_point(placement, other.position);
                    (position.x_nm - point.x_nm).abs() + (position.y_nm - point.y_nm).abs()
                })
                .min()
                .unwrap_or(1_000_000_000_000_000_000);
            if neighbor > options.maximum_neighbor_distance_nm {
                continue;
            }
            pads.push((neighbor, reference, position, placement, pad));
        }
    }
    let mut pin_by_ref: IndexMap<PadReference, Vec<Pin<'_>>> = IndexMap::new();
    for item in &pads {
        pin_by_ref.entry(item.1.clone()).or_default().push(item.clone());
    }
    let outline = OutlineSites::new(&board.outline.vertices);
    let mut surface: IndexMap<PadReference, CopperLayer> = IndexMap::new();
    for (reference, items) in &pin_by_ref {
        let net = &net_by_pad[reference];
        if let Some(layer) = surface_layer(board, net, rules.get(net.as_str()).copied(), items[0].3) {
            surface.insert(reference.clone(), layer);
        }
    }
    let required_surface: IndexMap<PadReference, CopperLayer> = surface.clone();
    let settings = BoundaryAccessOptions::default();
    let mut collars: IndexMap<String, copper_physical::geometry::Bounds> = IndexMap::new();
    for items in pin_by_ref.values() {
        for item in items {
            if !collars.contains_key(&item.3.reference) {
                collars.insert(
                    item.3.reference.clone(),
                    package_collar(board, &item.3.reference, settings.collar_margin_nm)?,
                );
            }
        }
    }
    let surface_choices = |reference: &PadReference,
                           layer: CopperLayer,
                           index: &RoutingClearanceIndex<'_>,
                           refined: bool|
     -> Result<Vec<EscapeCandidate>> {
        let mut choices = Vec::new();
        for (_, _, position, placement, _) in &pin_by_ref[reference] {
            for path in surface_escapes(
                board,
                index,
                &net_by_pad[reference],
                *position,
                layer,
                collars[&placement.reference],
                &settings,
                refined,
            )? {
                choices.push((path, None));
            }
        }
        Ok(unique(choices))
    };
    let choices_for = |reference: &PadReference,
                       index: &RoutingClearanceIndex<'_>,
                       candidate_options: &FanoutOptions,
                       elbows: bool,
                       multiple_orders: bool|
     -> Result<Vec<EscapeCandidate>> {
        let mut choices = Vec::new();
        for (_, _, position, placement, pad) in &pin_by_ref[reference] {
            let sites = if elbows {
                two_leg_candidates(*position, placement.position, candidate_options)
            } else {
                package_candidates(board, placement, pad, candidate_options)
            };
            choices.extend(legal_choices(
                board,
                index,
                &net_by_pad[reference],
                *position,
                placement,
                &sites,
                multiple_orders,
                &outline,
            )?);
        }
        Ok(unique(choices))
    };
    let fine = FanoutOptions {
        step_nm: options.refinement_step_nm,
        maximum_two_leg_candidates: options.maximum_refined_two_leg_candidates,
        ..options.clone()
    };
    let mut refinement_cache: IndexMap<PadReference, Vec<EscapeCandidate>> = IndexMap::new();
    let mut refine = |reference: &PadReference| -> Result<Vec<EscapeCandidate>> {
        if let Some(cached) = refinement_cache.get(reference) {
            return Ok(cached.clone());
        }
        if options.refinement_step_nm == options.step_nm {
            return Ok(Vec::new());
        }
        let index = RoutingClearanceIndex::new(board)?;
        let mut all = choices_for(reference, &index, &fine, false, false)?;
        if options.two_leg_escapes {
            all.extend(choices_for(reference, &index, &fine, true, true)?);
        }
        let all = unique(all);
        refinement_cache.insert(reference.clone(), all.clone());
        Ok(all)
    };
    let mut geometry_cache: IndexMap<PadReference, Vec<EscapeCandidate>> = IndexMap::new();
    let mut geometry_choices = |reference: &PadReference| -> Result<Vec<EscapeCandidate>> {
        if let Some(cached) = geometry_cache.get(reference) {
            return Ok(cached.clone());
        }
        let net = &net_by_pad[reference];
        let index = RoutingClearanceIndex::new(board)?;
        let mut choices = Vec::new();
        for (_, _, start, pose, pad) in &pin_by_ref[reference] {
            let side = side_of(pose);
            let allowed = routing_layers(board, net, rules.get(net.as_str()).copied());
            let Some(other) = allowed.iter().find(|l| **l != side) else { continue };
            let Some(span) = physical_via_span(board, side, *other) else { continue };
            if !allowed.contains(&side) {
                continue;
            }
            let seeds =
                circular_cell_sites(board, pose, pad, options.maximum_neighbor_distance_nm, options.maximum_radius_nm);
            let sites = obstacle_boundary_sites(
                &index,
                net,
                *start,
                board.rules.default_via_size_nm,
                board.rules.default_via_drill_nm,
                (span.0, span.1),
                options.maximum_radius_nm,
                options.maximum_geometry_candidates,
                &seeds,
                16,
            )?;
            choices.extend(legal_choices(board, &index, net, *start, pose, &sites, true, &outline)?);
        }
        let choices = unique(choices);
        geometry_cache.insert(reference.clone(), choices.clone());
        Ok(choices)
    };
    let side_allowed = |reference: &PadReference| -> Option<CopperLayer> {
        let pose = pin_by_ref[reference][0].3;
        let side = side_of(pose);
        let net = &net_by_pad[reference];
        routing_layers(board, net, rules.get(net.as_str()).copied()).contains(&side).then_some(side)
    };
    let mut domains: IndexMap<PadReference, Vec<EscapeCandidate>> = IndexMap::new();
    for reference in pin_by_ref.keys() {
        if let Some(layer) = surface.get(reference).copied() {
            let mut choices = surface_choices(reference, layer, &clearance, false)?;
            if choices.is_empty() {
                choices = surface_choices(reference, layer, &clearance, true)?;
            }
            domains.insert(reference.clone(), choices);
            continue;
        }
        let mut prefer_surface = options.prefer_surface_escapes;
        if options.surface_pads.contains(reference) {
            prefer_surface = true;
        } else if prefer_surface && let Some(limit) = options.surface_preference_depth {
            let (pose, pad) = (pin_by_ref[reference][0].3, pin_by_ref[reference][0].4);
            let depth = circular_array_depth(board, pose, pad, options.maximum_neighbor_distance_nm);
            prefer_surface = depth.is_some_and(|depth| depth <= limit);
        }
        if prefer_surface && let Some(side) = side_allowed(reference) {
            surface.insert(reference.clone(), side);
            let mut choices = surface_choices(reference, side, &clearance, false)?;
            if choices.is_empty() {
                choices = surface_choices(reference, side, &clearance, true)?;
            }
            if !choices.is_empty() {
                domains.insert(reference.clone(), choices);
                continue;
            }
        }
        let mut choices = choices_for(reference, &clearance, &options, false, false)?;
        if options.two_leg_escapes && choices.is_empty() {
            choices = choices_for(reference, &clearance, &options, true, false)?;
        }
        if choices.is_empty() {
            choices = refine(reference)?;
        }
        if choices.is_empty() && options.maximum_geometry_candidates != 0 {
            choices = geometry_choices(reference)?;
        }
        if choices.is_empty() && options.maze_escapes {
            choices = maze_choices(board, &clearance, &net_by_pad[reference], &pin_by_ref[reference], &options)?;
        }
        if choices.is_empty()
            && options.surface_fallback
            && let Some(side) = side_allowed(reference)
        {
            surface.insert(reference.clone(), side);
            choices = surface_choices(reference, side, &clearance, false)?;
            if choices.is_empty() {
                choices = surface_choices(reference, side, &clearance, true)?;
            }
        }
        domains.insert(reference.clone(), choices);
    }
    let mut ordered: Vec<&Pin<'_>> =
        pin_by_ref.values().map(|items| items.iter().min_by_key(|item| item.0).expect("pins")).collect();
    ordered.sort_by(|a, b| {
        let key = |item: &&Pin<'_>| {
            (if options.constrained_pins_first { domains[&item.1].len() } else { 0 }, item.0, item.1.clone())
        };
        key(a).cmp(&key(b))
    });
    let ordered_refs: Vec<PadReference> = ordered.iter().map(|item| item.1.clone()).collect();
    let mut tracks: Vec<TrackSegment> = Vec::new();
    let mut vias: Vec<Via> = Vec::new();
    let mut selected: IndexMap<PadReference, usize> = IndexMap::new();
    for reference in &ordered_refs {
        for (index, candidate) in domains[reference].iter().enumerate() {
            if reserve(board, &mut clearance, &mut tracks, &mut vias, candidate)? {
                selected.insert(reference.clone(), index);
                break;
            }
        }
    }
    let greedy = selected.clone();
    let mut assignment = None;
    if options.joint_escapes {
        let mut expand = |reference: &PadReference| -> Result<Vec<EscapeCandidate>> {
            if let Some(layer) = required_surface.get(reference).copied() {
                let index = RoutingClearanceIndex::new(board)?;
                let mut all = surface_choices(reference, layer, &index, false)?;
                all.extend(surface_choices(reference, layer, &index, true)?);
                return Ok(unique(all));
            }
            let mut all = if options.two_leg_escapes {
                choices_for(reference, &RoutingClearanceIndex::new(board)?, &options, true, true)?
            } else {
                Vec::new()
            };
            all.extend(refine(reference)?);
            if options.maze_escapes {
                all.extend(maze_choices(
                    board,
                    &RoutingClearanceIndex::new(board)?,
                    &net_by_pad[reference],
                    &pin_by_ref[reference],
                    &options,
                )?);
            }
            if options.maximum_geometry_candidates != 0 {
                all.extend(geometry_choices(reference)?);
            }
            Ok(unique(all))
        };
        let surface_cell = std::cell::RefCell::new(&mut surface);
        let mut expand_surface = |reference: &PadReference| -> Result<Vec<EscapeCandidate>> {
            let Some(side) = side_allowed(reference) else { return Ok(Vec::new()) };
            surface_cell.borrow_mut().insert(reference.clone(), side);
            let index = RoutingClearanceIndex::new(board)?;
            let mut all = surface_choices(reference, side, &index, false)?;
            all.extend(surface_choices(reference, side, &index, true)?);
            Ok(unique(all))
        };
        let fallback: Option<&mut dyn FnMut(&PadReference) -> Result<Vec<EscapeCandidate>>> =
            if options.surface_fallback { Some(&mut expand_surface) } else { None };
        let (improved, report) = improve_escape_assignment(
            board,
            &ordered_refs,
            &mut domains,
            &selected,
            &mut expand,
            &options.assignment_options,
            fallback,
        )?;
        selected = improved;
        assignment = Some(report);
    }
    let materialize = |selection: &IndexMap<PadReference, usize>| -> Result<(PhysicalBoard, Vec<TrackSegment>, Vec<Via>, IndexMap<PadReference, Point>)> {
        let mut index = RoutingClearanceIndex::new(board)?;
        let (mut new_tracks, mut new_vias, mut accesses) = (Vec::new(), Vec::new(), IndexMap::new());
        for reference in &ordered_refs {
            if let Some(choice) = selection.get(reference) {
                let candidate = &domains[reference][*choice];
                if reserve(board, &mut index, &mut new_tracks, &mut new_vias, candidate)? {
                    accesses.insert(reference.clone(), candidate.0[candidate.0.len() - 1].end);
                }
            }
        }
        drop(index);
        let mut routed = board.clone();
        routed.tracks.extend(new_tracks.iter().cloned());
        routed.vias.extend(new_vias.iter().cloned());
        Ok((routed, new_tracks, new_vias, accesses))
    };
    let (mut routed, mut tracks, mut vias, mut accesses) = materialize(&selected)?;
    // Native DRC is the final transactional gate: a new manufacturing
    // violation rejects the whole fanout proposal.
    const FATAL: [&str; 9] = [
        "DRC-SHORT",
        "DRC-CLEARANCE",
        "DRC-BOARD-EDGE",
        "DRC-HOLE-CLEARANCE",
        "DRC-DRILL-SPACING",
        "DRC-COPPER-KEEPOUT",
        "DRC-VIA-SPAN",
        "DRC-TRACK-WIDTH",
        "DRC-VIA-PAD-OVERLAP",
    ];
    let policy = copper_drc::Policy::default();
    let count =
        |report: &crate::drc_report::DrcReport, code: &str| report.findings().iter().filter(|f| f.code == code).count();
    let before = crate::drc_report::run_physical_drc(board, policy)?;
    let mut after = crate::drc_report::run_physical_drc(&routed, policy)?;
    let failed_gate = |after: &crate::drc_report::DrcReport, candidate: &PhysicalBoard| -> Result<bool> {
        if FATAL.iter().any(|code| count(after, code) > count(&before, code)) {
            return Ok(true);
        }
        if !protected_bypasses.is_empty() {
            let current = verified_bypass_links(candidate, &bypass_nets, None, None, &[])?;
            if !protected_bypasses.iter().all(|link| current.contains(link)) {
                return Ok(true);
            }
        }
        let replacing = IndexSet::new();
        Ok(refresh_crosslayer_witnesses(candidate, &bypass.admitted_path_witnesses, None, &replacing)?.1.is_some())
    };
    let selected_set: IndexSet<&PadReference> = selected.keys().collect();
    let access_set: IndexSet<&PadReference> = accesses.keys().collect();
    if let Some(report) = assignment.as_mut()
        && (selected_set != access_set || failed_gate(&after, &routed)?)
    {
        selected = greedy;
        report.native_accepted = false;
        (routed, tracks, vias, accesses) = materialize(&selected)?;
        after = crate::drc_report::run_physical_drc(&routed, policy)?;
    }
    let pending: Vec<PadReference> = ordered_refs.iter().filter(|r| !accesses.contains_key(*r)).cloned().collect();
    if failed_gate(&after, &routed)? {
        let all: Vec<PadReference> = pads.iter().map(|item| item.1.clone()).collect();
        let mut result = FanoutResult::bypass(&bypass, all);
        result.assignment = assignment;
        return Ok(result);
    }
    let mut escapes: IndexMap<PadReference, RoutingAccess> = IndexMap::new();
    for reference in accesses.keys() {
        let (path, via) = &domains[reference][selected[reference]];
        let end = path[path.len() - 1].end;
        let selected_surface = surface.contains_key(reference)
            && via.is_none()
            && !board.vias.iter().any(|item| item.net == net_by_pad[reference] && item.position == end);
        if selected_surface {
            escapes
                .insert(reference.clone(), RoutingAccess::new(end, surface[reference], path[0].start, path.clone())?);
        }
    }
    let replacing = IndexSet::new();
    let (admitted, reason) = refresh_crosslayer_witnesses(&routed, &bypass.admitted_path_witnesses, None, &replacing)?;
    if let Some(reason) = reason {
        return Err(Error::Value(format!("fanout lost an original admitted bypass path: {reason}")));
    }
    let mut plain = bypass.accesses.clone();
    for (reference, anchor) in &accesses {
        if !escapes.contains_key(reference) {
            plain.insert(reference.clone(), *anchor);
        }
    }
    let result = FanoutResult {
        board: routed,
        accesses: plain,
        pending_pads: pending,
        added_track_count: (tracks.len() + bypass.created_tracks.len()) as i64,
        added_via_count: (vias.len() + bypass.created_vias.len()) as i64,
        created_vias: bypass.created_vias.iter().chain(&vias).cloned().collect(),
        created_tracks: bypass.created_tracks.iter().chain(&tracks).cloned().collect(),
        assignment,
        surface_accesses: if surface.is_empty() { None } else { Some(escapes) },
        decoupling_tracks: bypass.created_tracks.clone(),
        decoupling_vias: bypass.created_vias.clone(),
        decoupling_witnesses: admitted,
        decoupling_pending_links: bypass.pending.clone(),
    };
    crate::fanout_bypass_completion::complete_late_bypasses(result, &bypass.pending, &options, only_nets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support_routers::*;

    fn wire(a: (f64, f64), b: (f64, f64)) -> TrackSegment {
        track("P", a, b, 0.2, CopperLayer::Front)
    }

    /// `tests/test_routing_quality.py::test_greedy_and_joint_assignment_reject_the_same_net_x` (greedy half).
    #[test]
    fn greedy_reservation_rejects_the_same_net_x() {
        let board = board("quality", 10.0, 10.0, Vec::new(), Vec::new(), vec![net("P", &[])]);
        let mut index = RoutingClearanceIndex::new(&board).expect("index");
        let (mut tracks, mut vias) = (Vec::new(), Vec::new());
        let first = (vec![wire((2.0, 2.0), (4.0, 4.0))], None);
        let second = (vec![wire((4.0, 2.0), (2.0, 4.0))], None);
        assert!(reserve(&board, &mut index, &mut tracks, &mut vias, &first).expect("reserve"));
        assert!(!reserve(&board, &mut index, &mut tracks, &mut vias, &second).expect("reserve"));
    }

    /// `tests/test_routing_quality.py::test_straight_outward_precedes_diagonal_even_at_farther_radius`.
    #[test]
    fn straight_outward_precedes_diagonal_even_at_farther_radius() {
        let options = FanoutOptions { maximum_radius_nm: mm(1.0), ..FanoutOptions::default() };
        let points = ray_candidates(pt(6.0, 5.1), pt(5.0, 5.0), &options, false);
        let at = |p: Point| points.iter().position(|q| *q == p).expect("candidate");
        assert!(at(pt(7.0, 5.1)) < at(pt(6.5, 5.6)));
        let rotated = ray_candidates(pt(6.0, 6.0), pt(5.0, 5.0), &options, true);
        assert_eq!(rotated[0], pt(6.5, 6.5));
    }
}

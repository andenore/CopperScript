//! Declared copper planes: pad-to-plane stitching (`pcbir.plane`).

use std::collections::BTreeSet;

use copper_pcb::board::PadReference;
use copper_physical::CopperKeepout;

use crate::{Error, Result, nm_from_mm};

/// `PlaneStitchOptions`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneStitchOptions {
    pub step_nm: i64,
    pub maximum_radius_nm: i64,
    pub maximum_contact_radius_nm: i64,
    pub maximum_detour_nm: i64,
    pub maze_step_nm: i64,
    pub maze_state_budget: i64,
    pub candidate_bias: Option<String>,
    pub escape_width_nm: Option<i64>,
    pub only_pads: Option<BTreeSet<PadReference>>,
    /// None prefers every GND pad; an empty set restores reuse-first behavior.
    pub preferred_ground_pads: Option<BTreeSet<PadReference>>,
    pub ground_via_in_pad: bool,
    /// Opt-in opposite-side surface zones, e.g. a two-layer rear ground pour.
    /// Same-side-only zones are refill intent, not a reason to invent a via.
    pub include_surface_zones: bool,
    /// Repair mode: a pad inside a same-side pour still needs explicit copper
    /// (a via, or a surface path to an already connected same-net pad or via).
    /// Used after routing, when a later trace may have walled a pad off.
    pub require_explicit: bool,
    /// Keepouts that bind this stitch only, never the board: the launch
    /// corridors beyond macro ports, which a critical pair needs for its own
    /// transition after the stitch has prepared its return contacts.
    pub keepouts: Vec<CopperKeepout>,
}

impl Default for PlaneStitchOptions {
    fn default() -> Self {
        PlaneStitchOptions {
            step_nm: 500_000,
            maximum_radius_nm: nm_from_mm(3),
            maximum_contact_radius_nm: 0,
            maximum_detour_nm: 0,
            maze_step_nm: 100_000,
            maze_state_budget: 12_000,
            candidate_bias: None,
            escape_width_nm: None,
            only_pads: None,
            preferred_ground_pads: None,
            ground_via_in_pad: false,
            include_surface_zones: false,
            require_explicit: false,
            keepouts: Vec::new(),
        }
    }
}

impl PlaneStitchOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if !matches!(self.candidate_bias.as_deref(), None | Some("east" | "south" | "west" | "north")) {
            return Err(Error::Value("plane-stitch candidate bias must be a cardinal direction".to_owned()));
        }
        if self.step_nm <= 0
            || self.maximum_radius_nm < self.step_nm
            || self.maximum_contact_radius_nm < 0
            || self.maximum_detour_nm < 0
            || self.maze_step_nm <= 0
            || self.maze_state_budget <= 0
            || self.escape_width_nm.is_some_and(|width| width < 90_000)
        {
            return Err(Error::Value("plane-stitch search bounds or escape width are invalid".to_owned()));
        }
        Ok(self)
    }
}

/// `PlaneStitchResult` (`copper_uses` are not modelled yet).
#[derive(Clone, Debug)]
pub struct PlaneStitchResult {
    pub board: copper_pcb::board::PhysicalBoard,
    pub stitched_pads: Vec<PadReference>,
    pub pending_pads: Vec<PadReference>,
    pub added_track_count: i64,
    pub added_via_count: i64,
    /// Remote pads of explicitly regional routing reservations remain
    /// ordinary feeder obligations; not a connected-pad inventory.
    pub explicit_feeder_pads: Vec<PadReference>,
    pub copper_uses: Vec<crate::copper_ownership::CopperUse>,
}

impl PlaneStitchResult {
    /// `complete`: all local targets have prospective contacts.
    pub fn complete(&self) -> bool {
        self.pending_pads.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Stitching

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, Connectivity, explicit_copper_connectivity};
use copper_pcb::board::{CopperZone, PhysicalBoard, Placement, TrackSegment, Via};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::{FootprintPad, PadKind};
use copper_physical::geometry::{point_segment_distance_squared, shapes_clear};
use copper_physical::pyfloat::hypot;
use copper_physical::{BoardSide, CopperLayer, Point, RoundedConvexShape};
use num_rational::BigRational;

use crate::access_sites::{circular_cell_sites, obstacle_boundary_sites};
use crate::copper_ownership::CopperUse;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::{ViaSearch, surface_path_detour, surface_path_to_via, via_inside_board};

/// `_PendingContact`.
#[derive(Clone, Debug)]
struct PendingContact {
    net: String,
    reference: PadReference,
    position: Point,
    side: CopperLayer,
    width_nm: i64,
}

fn side_layer(placement: &Placement) -> CopperLayer {
    if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }
}

fn squared(a: Point, b: Point) -> i128 {
    let (dx, dy) = (i128::from(a.x_nm - b.x_nm), i128::from(a.y_nm - b.y_nm));
    dx * dx + dy * dy
}

fn rational(value: i128) -> BigRational {
    BigRational::from_integer(value.into())
}

/// `_point_in_outline`: even-odd crossing test with Python's float crossing.
fn point_in_outline(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    for (index, first) in polygon.iter().enumerate() {
        let second = polygon[(index + 1) % polygon.len()];
        if (first.y_nm > point.y_nm) != (second.y_nm > point.y_nm) {
            let numerator = i128::from(second.x_nm - first.x_nm) * i128::from(point.y_nm - first.y_nm);
            let crossing =
                crate::routing_lattice::true_div(numerator, i128::from(second.y_nm - first.y_nm)) + first.x_nm as f64;
            if (point.x_nm as f64) < crossing {
                inside = !inside;
            }
        }
    }
    inside
}

/// `_point_in_zone`.
fn point_in_zone(point: Point, zone: &CopperZone) -> bool {
    point_in_outline(point, &zone.outer) && !zone.holes.iter().any(|hole| point_in_outline(point, hole))
}

fn point_in_keepout(point: Point, keepout: &CopperKeepout) -> bool {
    point_in_outline(point, &keepout.vertices)
}

/// `_via_contacts_zone`.
fn via_contacts_zone(point: Point, zone: &CopperZone, span: &[CopperLayer], keepouts: &[CopperKeepout]) -> bool {
    point_in_zone(point, zone)
        && zone.layers.iter().any(|layer| {
            span.contains(layer)
                && !keepouts.iter().any(|k| k.block_zones && k.layers.contains(layer) && point_in_keepout(point, k))
        })
}

/// The edges of a zone's rings, outer then holes.
fn zone_edges(zone: &CopperZone) -> Vec<(Point, Point)> {
    let mut edges = Vec::new();
    for ring in std::iter::once(&zone.outer).chain(zone.holes.iter()) {
        for (index, a) in ring.iter().enumerate() {
            edges.push((*a, ring[(index + 1) % ring.len()]));
        }
    }
    edges
}

/// `_outside_local_regions`.
fn outside_local_regions(
    placement: &Placement,
    land: &FootprintPad,
    zones: &[&CopperZone],
    options: &PlaneStitchOptions,
) -> bool {
    let position = transformed_local_point(placement, land.position);
    let reach = i128::from(2 * options.maximum_radius_nm + land.size.width_nm.max(land.size.height_nm));
    let limit = rational(reach * reach);
    zones.iter().all(|zone| {
        !point_in_zone(position, zone)
            && zone_edges(zone).iter().all(|(a, b)| point_segment_distance_squared(position, *a, *b) > limit)
    })
}

/// `_candidate_points`: rings of grid sites around a land, nearest first.
fn candidate_points(position: Point, options: &PlaneStitchOptions) -> Vec<Point> {
    let steps = options.maximum_radius_nm.div_euclid(options.step_nm);
    let preferred = match options.candidate_bias.as_deref() {
        Some("east") => (1, 0),
        Some("south") => (0, 1),
        Some("west") => (-1, 0),
        Some("north") => (0, -1),
        _ => (0, 0),
    };
    let mut points = Vec::new();
    for radius in 1..=steps {
        let mut offsets: Vec<(i64, i64)> = Vec::new();
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                if dx.abs().max(dy.abs()) == radius {
                    offsets.push((dx, dy));
                }
            }
        }
        offsets.sort_by_key(|(dx, dy)| (dx * dx + dy * dy, -(dx * preferred.0 + dy * preferred.1), *dx, *dy));
        for (dx, dy) in offsets {
            points.push(Point::new(position.x_nm + dx * options.step_nm, position.y_nm + dy * options.step_nm));
        }
    }
    points
}

/// A non-negative finite float with a total order, for the length search heap.
#[derive(Clone, Copy, PartialEq)]
struct Length(f64);

impl Eq for Length {}

impl PartialOrd for Length {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Length {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// `_surface_route_length`: shortest endpoint-connected copper length for a proposed escape.
fn surface_route_length(
    net: &str,
    layer: CopperLayer,
    width: i64,
    start: Point,
    end: Point,
    tracks: &[&TrackSegment],
) -> f64 {
    if start == end {
        return 0.0;
    }
    let mut neighbors: IndexMap<Point, Vec<(Point, f64)>> = IndexMap::new();
    for track in tracks {
        if track.net != net || track.layer != layer || track.width_nm < width {
            continue;
        }
        let length = hypot((track.start.x_nm - track.end.x_nm) as f64, (track.start.y_nm - track.end.y_nm) as f64);
        neighbors.entry(track.start).or_default().push((track.end, length));
        neighbors.entry(track.end).or_default().push((track.start, length));
    }
    let mut queue = BinaryHeap::from([std::cmp::Reverse((Length(0.0), 0i64, start))]);
    let mut seen: IndexSet<Point> = IndexSet::new();
    let mut serial = 0;
    while let Some(std::cmp::Reverse((Length(distance), _, point))) = queue.pop() {
        if point == end {
            return distance;
        }
        if !seen.insert(point) {
            continue;
        }
        for (neighbor, length) in neighbors.get(&point).map(Vec::as_slice).unwrap_or(&[]) {
            if !seen.contains(neighbor) {
                serial += 1;
                queue.push(std::cmp::Reverse((Length(distance + length), serial, *neighbor)));
            }
        }
    }
    hypot((start.x_nm - end.x_nm) as f64, (start.y_nm - end.y_nm) as f64)
}

/// The ranked contact proposals of one land.
struct Ranked {
    choices: Vec<(i64, f64, Point, Vec<TrackSegment>, Option<Via>)>,
    best_cost: f64,
    best_region_rank: i64,
}

/// The fixed context of one land's contact search.
struct LandSearch<'a> {
    board: &'a PhysicalBoard,
    net: &'a str,
    zones: &'a [&'a CopperZone],
    pad: &'a FootprintPad,
    placement: &'a Placement,
    position: Point,
    side: CopperLayer,
    width: i64,
    via_size: i64,
    via_drill: i64,
    outer: (CopperLayer, CopperLayer),
    options: &'a PlaneStitchOptions,
    committed_tracks: &'a [TrackSegment],
    committed_vias: &'a [Via],
}

type Contact = (Vec<TrackSegment>, Option<Via>);

impl LandSearch<'_> {
    fn via(&self, position: Point) -> Via {
        Via {
            net: self.net.to_owned(),
            position,
            size_nm: self.via_size,
            drill_nm: self.via_drill,
            from_layer: self.outer.0,
            to_layer: self.outer.1,
            technology: None,
            finish: "standard".to_owned(),
        }
    }

    fn path(
        &self,
        clearance: &RoutingClearanceIndex<'_>,
        from: Point,
        to: Point,
        layer: CopperLayer,
        committed: &[TrackSegment],
    ) -> Result<Option<Vec<TrackSegment>>> {
        surface_path_detour(
            self.board,
            clearance,
            self.net,
            from,
            to,
            self.width,
            layer,
            committed,
            self.options.maximum_detour_nm,
            500_000,
        )
    }

    /// `_existing_via_zone_tail`: retain an existing escape and contact a nearby zone.
    fn existing_via_zone_tail(&self, clearance: &RoutingClearanceIndex<'_>) -> Result<Option<Contact>> {
        let radius_squared = i128::from(self.options.maximum_radius_nm) * i128::from(self.options.maximum_radius_nm);
        let rule = self.board.net_routing_rules.iter().find(|r| r.net == self.net);
        let mut layers: Vec<CopperLayer> = Vec::new();
        for zone in self.zones {
            for layer in &zone.layers {
                if *layer != self.side
                    && rule.is_none_or(|r| r.allowed_layers.is_empty() || r.allowed_layers.contains(layer))
                    && !layers.contains(layer)
                {
                    layers.push(*layer);
                }
            }
        }
        let mut reusable: Vec<&Via> = self
            .committed_vias
            .iter()
            .filter(|via| {
                via.net == self.net
                    && (via.from_layer, via.to_layer) == self.outer
                    && squared(via.position, self.position) <= radius_squared
                    && !self.zones.iter().any(|z| point_in_zone(via.position, z))
            })
            .collect();
        reusable.sort_by_key(|via| (squared(via.position, self.position), via.position.x_nm, via.position.y_nm));
        for via in reusable {
            let Some(access) = self.path(clearance, self.position, via.position, self.side, self.committed_tracks)?
            else {
                continue;
            };
            let committed: Vec<TrackSegment> = self.committed_tracks.iter().chain(&access).cloned().collect();
            for target in candidate_points(self.position, self.options) {
                if squared(target, self.position) > radius_squared {
                    continue;
                }
                for layer in &layers {
                    if !self.zones.iter().any(|z| z.layers.contains(layer) && point_in_zone(target, z)) {
                        continue;
                    }
                    if let Some(tail) = self.path(clearance, via.position, target, *layer, &committed)? {
                        let mut path = access.clone();
                        path.extend(tail);
                        return Ok(Some((path, None)));
                    }
                }
            }
        }
        Ok(None)
    }

    /// `_stitch_land_choices` with one alternative and no site limit; the
    /// second value says whether a ranked selection chose it.
    fn choose(
        &self,
        clearance: &RoutingClearanceIndex<'_>,
        preferred_via_uses: &IndexMap<Point, i64>,
    ) -> Result<(Option<Contact>, bool)> {
        let (net, position, side, options) = (self.net, self.position, self.side, self.options);
        let reference = PadReference { component: self.placement.reference.clone(), pad: self.pad.number.clone() };
        let preferred = if net == "GND" {
            options.preferred_ground_pads.as_ref().is_some_and(|pads| pads.contains(&reference))
        } else {
            self.zones.iter().any(|zone| point_in_zone(position, zone))
        };
        let radius_squared = i128::from(options.maximum_radius_nm) * i128::from(options.maximum_radius_nm);
        let near = rational(radius_squared);
        let local_regions: Vec<&&CopperZone> = self
            .zones
            .iter()
            .filter(|zone| {
                preferred
                    && zone.reserve_routing
                    && zone.layers.contains(&side)
                    && (point_in_zone(position, zone)
                        || zone_edges(zone)
                            .iter()
                            .any(|(a, b)| point_segment_distance_squared(position, *a, *b) <= near))
            })
            .collect();
        let mut ranked = Ranked { choices: Vec::new(), best_cost: f64::INFINITY, best_region_rank: 1 };
        let consider = |ranked: &mut Ranked, path: Vec<TrackSegment>, target: Point, via: Option<Via>| {
            let committed: Vec<&TrackSegment> = self.committed_tracks.iter().chain(path.iter()).collect();
            let routed = surface_route_length(net, side, self.width, position, target, &committed);
            let mut sum = 0.0;
            for track in &path {
                sum += hypot((track.start.x_nm - track.end.x_nm) as f64, (track.start.y_nm - track.end.y_nm) as f64);
            }
            let length = if sum > routed { sum } else { routed };
            let mut cost = length + if via.is_some() { 200_000.0 } else { 0.0 };
            cost += (preferred_via_uses.get(&target).copied().unwrap_or(0) * 400_000) as f64;
            let region_rank =
                i64::from(!local_regions.is_empty() && !local_regions.iter().any(|zone| point_in_zone(target, zone)));
            if (region_rank, cost).partial_cmp(&(ranked.best_region_rank, ranked.best_cost)) == Some(Ordering::Less) {
                (ranked.best_region_rank, ranked.best_cost) = (region_rank, cost);
            }
            ranked.choices.push((region_rank, cost, target, path, via));
        };
        // Existing fanout and routed vias need not lie on the half-mm grid.
        let mut reusable: Vec<&Via> = self
            .committed_vias
            .iter()
            .filter(|via| {
                via.net == net
                    && via.from_layer == self.outer.0
                    && via.to_layer == self.outer.1
                    && self.zones.iter().any(|zone| point_in_zone(via.position, zone))
                    && squared(via.position, position) <= radius_squared
            })
            .collect();
        reusable.sort_by_key(|via| (squared(via.position, position), via.position.x_nm, via.position.y_nm));
        for existing in reusable {
            let path = self.path(clearance, position, existing.position, side, self.committed_tracks)?;
            match path {
                Some(path) if path.is_empty() => return Ok((Some((path, None)), false)),
                Some(path) => {
                    if !preferred {
                        return Ok((Some((path, None)), false));
                    }
                    consider(&mut ranked, path, existing.position, None);
                }
                None => {}
            }
        }
        let reserved_region = self.zones.iter().any(|zone| zone.reserve_routing);
        if reserved_region && let Some(tail) = self.existing_via_zone_tail(clearance)? {
            return Ok((Some(tail), false));
        }
        let pad_shape = placed_pad_shape(position, self.pad, self.placement);
        let mut legal_via_targets: Vec<Point> = Vec::new();
        let interstices = circular_cell_sites(
            self.board,
            self.placement,
            self.pad,
            options.maximum_radius_nm * 2,
            options.maximum_radius_nm,
        );
        let frontiers = if interstices.is_empty() {
            Vec::new()
        } else {
            obstacle_boundary_sites(
                clearance,
                net,
                position,
                self.via_size,
                self.via_drill,
                self.outer,
                options.maximum_radius_nm,
                16,
                &interstices,
                16,
            )?
        };
        let mut seen: IndexSet<Point> = IndexSet::new();
        let sequence = interstices
            .iter()
            .chain(frontiers.iter())
            .map(|p| (*p, false))
            .chain(candidate_points(position, options).into_iter().map(|p| (p, true)));
        let via_radius = self.via_size.div_euclid(2);
        for (candidate, radial) in sequence {
            if !seen.insert(candidate) {
                continue;
            }
            let distance_x = (candidate.x_nm - position.x_nm).abs();
            let distance_y = (candidate.y_nm - position.y_nm).abs();
            if preferred
                && !ranked.choices.is_empty()
                && (local_regions.is_empty() || ranked.best_region_rank == 0)
                && (distance_x.max(distance_y) as f64) > ranked.best_cost.min(2_000_000.0)
            {
                if radial {
                    break;
                }
                continue;
            }
            if !point_in_outline(candidate, &self.board.outline.vertices) {
                continue;
            }
            if !self.zones.iter().any(|zone| point_in_zone(candidate, zone)) {
                continue;
            }
            let annulus = RoundedConvexShape::new(vec![candidate], via_radius).map_err(copper_pcb::Error::Value)?;
            if !shapes_clear(&annulus, &pad_shape, 1) {
                continue;
            }
            let coincident: Vec<&Via> = self.committed_vias.iter().filter(|item| item.position == candidate).collect();
            let existing = coincident
                .iter()
                .find(|item| item.net == net && item.from_layer == self.outer.0 && item.to_layer == self.outer.1);
            if !coincident.is_empty() && existing.is_none() {
                continue;
            }
            let via = if existing.is_some() { None } else { Some(self.via(candidate)) };
            if existing.is_none() {
                if !via_inside_board(self.board, candidate, self.via_size)? {
                    continue;
                }
                if !clearance.can_via(
                    net,
                    candidate,
                    self.via_size,
                    self.outer.0,
                    self.outer.1,
                    Some(self.via_drill),
                    false,
                    None,
                )? {
                    continue;
                }
                legal_via_targets.push(candidate);
            }
            if let Some(path) = self.path(clearance, position, candidate, side, self.committed_tracks)? {
                if !preferred {
                    return Ok((Some((path, via)), false));
                }
                consider(&mut ranked, path, candidate, via);
            }
        }
        if !ranked.choices.is_empty() {
            let outward = (
                (position.x_nm - self.placement.position.x_nm) as f64,
                (position.y_nm - self.placement.position.y_nm) as f64,
            );
            let outward_length = hypot(outward.0, outward.1);
            let alignment = |target: Point| -> f64 {
                let (dx, dy) = (target.x_nm - position.x_nm, target.y_nm - position.y_nm);
                let span = hypot(dx as f64, dy as f64) * outward_length;
                if span != 0.0 { (dx as f64 * outward.0 + dy as f64 * outward.1) / span } else { 0.0 }
            };
            let score = |target: Point| -> i64 {
                if options.candidate_bias.is_none() {
                    clearance.terminal_preferences.score(net, side, &[position, target])
                } else {
                    0
                }
            };
            let mut ordered: Vec<_> = ranked.choices.iter().collect();
            ordered.sort_by(|a, b| {
                (a.0, a.1)
                    .partial_cmp(&(b.0, b.1))
                    .expect("finite costs")
                    .then((a.4.is_some()).cmp(&b.4.is_some()))
                    .then((-alignment(a.2)).partial_cmp(&-alignment(b.2)).expect("finite alignment"))
                    .then(score(a.2).cmp(&score(b.2)))
                    .then((a.2.x_nm, a.2.y_nm).cmp(&(b.2.x_nm, b.2.y_nm)))
            });
            let (_, _, _, path, via) = ordered[0];
            return Ok((Some((path.clone(), via.clone())), true));
        }
        // A dense package may have no legal straight, elbow, or short lateral
        // escape: search its small local neighborhood jointly over path and via.
        let occupied: IndexSet<Point> = self.committed_vias.iter().map(|via| via.position).collect();
        let accept = |candidate: Point| -> bool {
            !occupied.contains(&candidate)
                && self.zones.iter().any(|zone| point_in_zone(candidate, zone))
                && RoundedConvexShape::new(vec![candidate], via_radius)
                    .is_ok_and(|shape| shapes_clear(&shape, &pad_shape, 1))
        };
        let search = ViaSearch {
            step_nm: options.maze_step_nm,
            radius_nm: options.maximum_radius_nm,
            via_size_nm: self.via_size,
            via_drill_nm: self.via_drill,
            via_layers: self.outer,
            accept_via: &accept,
            via_targets: &legal_via_targets,
            state_budget: options.maze_state_budget,
        };
        if let Some((tracks, candidate)) =
            surface_path_to_via(self.board, clearance, net, position, self.width, side, &search)?
        {
            return Ok((Some((tracks, Some(self.via(candidate)))), false));
        }
        if options.ground_via_in_pad && net == "GND" {
            let (size, drill) = (300_000, 200_000);
            if self.pad.size.width_nm.min(self.pad.size.height_nm) >= size
                && self.zones.iter().any(|zone| point_in_zone(position, zone))
                && via_inside_board(self.board, position, size)?
                && !self.committed_vias.iter().any(|via| via.position == position)
                && clearance.can_via(
                    net,
                    position,
                    size,
                    self.outer.0,
                    self.outer.1,
                    Some(drill),
                    true,
                    Some(&reference),
                )?
            {
                let mut via = self.via(position);
                (via.size_nm, via.drill_nm, via.finish) = (size, drill, "filled-capped".to_owned());
                return Ok((Some((Vec::new(), Some(via))), false));
            }
        }
        let tail = if reserved_region { None } else { self.existing_via_zone_tail(clearance)? };
        Ok((tail, false))
    }
}

/// `stitch_zone_pads`: escape SMD pads to legal through-vias inside their declared zone.
pub fn stitch_zone_pads(board: &PhysicalBoard, options: Option<&PlaneStitchOptions>) -> Result<PlaneStitchResult> {
    if !board.hard_macros.is_empty() && board.materialized_macros.is_empty() {
        return Err(Error::Value("materialize hard macros before plane stitching".to_owned()));
    }
    let board = crate::pad_via_arrays::materialize_via_in_pad_arrays(board)?;
    let array_pads: IndexSet<PadReference> =
        crate::pad_via_arrays::array_rules(&board).into_iter().map(|rule| rule.pad.clone()).collect();
    let mut options = options.cloned().unwrap_or_default();
    if options.preferred_ground_pads.is_none() {
        options.preferred_ground_pads =
            Some(board.nets.iter().filter(|net| net.name == "GND").flat_map(|net| net.pads.iter().cloned()).collect());
    }
    if options.ground_via_in_pad
        && board.metadata.get("fabrication_profile").map(String::as_str) != Some("jlcpcb-six-layer")
    {
        return Err(Error::Value("filled/capped ground via-in-pad requires the JLCPCB six-layer profile".to_owned()));
    }
    if options
        .escape_width_nm
        .is_some_and(|width| width < board.rules.minimum_track_width_nm.max(board.rules.minimum_clearance_nm))
    {
        return Err(Error::Value("plane escape width is below the board rule floor".to_owned()));
    }
    if board.zones.is_empty() {
        return Ok(PlaneStitchResult {
            board,
            stitched_pads: Vec::new(),
            pending_pads: Vec::new(),
            added_track_count: 0,
            added_via_count: 0,
            explicit_feeder_pads: Vec::new(),
            copper_uses: Vec::new(),
        });
    }
    let preferred_pads = options.preferred_ground_pads.clone().unwrap_or_default();
    let mut clearance = RoutingClearanceIndex::new(&board)?;
    let placements: IndexMap<&str, &Placement> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let net_pads: IndexMap<&str, &Vec<PadReference>> = board.nets.iter().map(|n| (n.name.as_str(), &n.pads)).collect();
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let mut added_tracks: Vec<TrackSegment> = Vec::new();
    let mut added_vias: Vec<Via> = Vec::new();
    let mut targets: Vec<PadReference> = Vec::new();
    let permitted: IndexSet<&PadReference> = board.via_in_pad_rules.iter().map(|rule| &rule.pad).collect();
    let mut pending_lands: Vec<PendingContact> = Vec::new();
    let mut anchors_by_net: IndexMap<String, Vec<(Point, CopperLayer)>> = IndexMap::new();
    let mut preferred_via_uses: IndexMap<Point, i64> = IndexMap::new();
    let mut copper_uses: Vec<CopperUse> = Vec::new();
    let mut anchor_uses: IndexMap<(String, Point, CopperLayer), String> = IndexMap::new();
    let via_size = board.rules.default_via_size_nm;
    let via_drill = board.rules.default_via_drill_nm;
    let layers = &board.stackup.copper_layers;
    let outer = (layers[0], layers[layers.len() - 1]);
    let record_contact = |copper_uses: &mut Vec<CopperUse>,
                          anchor_uses: &mut IndexMap<(String, Point, CopperLayer), String>,
                          added_vias: &[Via],
                          net: &str,
                          reference: &PadReference,
                          position: Point,
                          side: CopperLayer,
                          tracks: &[TrackSegment],
                          dependencies: Vec<String>| {
        let mut points: IndexSet<Point> = IndexSet::from([position]);
        for track in tracks {
            points.insert(track.start);
            points.insert(track.end);
        }
        let vias: Vec<Via> = board
            .vias
            .iter()
            .chain(added_vias)
            .filter(|v| v.net == net && points.contains(&v.position))
            .cloned()
            .collect();
        let identity = format!("contact:{net}:{}.{}", reference.component, reference.pad);
        copper_uses.push(CopperUse {
            id: identity.clone(),
            kind: "contact".to_owned(),
            terminals: vec![reference.clone()],
            tracks: tracks.to_vec(),
            vias,
            dependencies,
        });
        anchor_uses.insert((net.to_owned(), position, side), identity);
    };
    let owned_pads = crate::pair_terminals::macro_owned_pads(&board);
    let keepouts = copper_pcb::keepouts::resolved_copper_keepouts(&board)?;
    let owner_graph: Option<Connectivity> =
        if owned_pads.is_empty() { None } else { Some(explicit_copper_connectivity(&board, Candidate::default())?) };
    let ground: IndexSet<String> = IndexSet::from(["GND".to_owned()]);
    let preferred_graph: Option<Connectivity> = if preferred_pads.is_empty() {
        None
    } else if let Some(graph) = &owner_graph {
        Some(graph.clone())
    } else {
        Some(explicit_copper_connectivity(&board, Candidate { only_nets: Some(&ground), ..Candidate::default() })?)
    };
    let mut preferred_roots: IndexMap<String, IndexSet<PadReference>> = IndexMap::new();
    if let Some(graph) = &preferred_graph {
        for pad in &preferred_pads {
            for node in graph.pad_nodes.get(pad).map(Vec::as_slice).unwrap_or(&[]) {
                preferred_roots.entry(graph.roots[node].clone()).or_default().insert(pad.clone());
            }
        }
    }
    let mut owned_pending: IndexSet<PadReference> = IndexSet::new();
    let mut explicit_feeders: BTreeSet<PadReference> = BTreeSet::new();
    let mut zones_by_net: std::collections::BTreeMap<&str, Vec<&CopperZone>> = std::collections::BTreeMap::new();
    let mut zones: Vec<&CopperZone> = board.zones.iter().collect();
    zones.sort_by(|a, b| a.id.cmp(&b.id));
    for zone in zones {
        let outer_only = |layer: &CopperLayer| *layer == outer.0 || *layer == outer.1;
        if zone.layers.iter().any(|l| !outer_only(l))
            || (options.include_surface_zones && zone.layers.iter().any(outer_only))
        {
            zones_by_net.entry(zone.net.as_str()).or_default().push(zone);
        }
    }
    let mut processed_groups: IndexSet<(String, Vec<String>)> = IndexSet::new();
    for (net, zones) in &zones_by_net {
        let mut references: Vec<&PadReference> =
            net_pads.get(net).map(|pads| pads.iter().collect()).unwrap_or_default();
        references.sort_by(|a, b| (!preferred_pads.contains(*a), *a).cmp(&(!preferred_pads.contains(*b), *b)));
        for reference in references {
            if options.only_pads.as_ref().is_some_and(|only| !only.contains(reference)) {
                continue;
            }
            let placement = placements[reference.component.as_str()];
            let side = side_layer(placement);
            let reference_zones: Vec<&CopperZone> =
                zones.iter().copied().filter(|z| z.layers.iter().any(|layer| *layer != side)).collect();
            if reference_zones.is_empty() {
                continue; // A same-side pour needs refill, not an invented through-via.
            }
            let footprint = &board.footprints[&placement.footprint];
            let mut lands: Vec<&FootprintPad> = footprint
                .pads
                .iter()
                .filter(|item| item.number == reference.pad && item.kind == PadKind::Smd)
                .collect();
            if lands.is_empty() {
                continue;
            }
            if !owned_pads.contains(reference)
                && reference_zones.iter().all(|z| z.reserve_routing)
                && lands.iter().all(|land| outside_local_regions(placement, land, &reference_zones, &options))
            {
                explicit_feeders.insert(reference.clone());
                continue;
            }
            targets.push(reference.clone());
            let group_numbers: Vec<String> = footprint
                .internal_pad_groups
                .iter()
                .find(|g| g.numbers.contains(&reference.pad))
                .map(|g| g.numbers.clone())
                .unwrap_or_else(|| vec![reference.pad.clone()]);
            if group_numbers.iter().any(|number| {
                array_pads.contains(&PadReference { component: reference.component.clone(), pad: number.clone() })
            }) {
                anchors_by_net
                    .entry((*net).to_owned())
                    .or_default()
                    .push((transformed_local_point(placement, lands[0].position), side));
                continue;
            }
            if owned_pads.contains(reference) {
                let graph = owner_graph.as_ref().expect("owner graph");
                let roots: IndexSet<&String> = graph
                    .pad_nodes
                    .get(reference)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .map(|n| &graph.roots[n])
                    .collect();
                let mut contacts: IndexSet<&String> = IndexSet::new();
                for (i, via) in board.vias.iter().enumerate() {
                    let (a, b) = (
                        layers.iter().position(|l| *l == via.from_layer).expect("layer"),
                        layers.iter().position(|l| *l == via.to_layer).expect("layer"),
                    );
                    let span = &layers[a.min(b)..=a.max(b)];
                    if via.net == *net
                        && reference_zones.iter().any(|zone| via_contacts_zone(via.position, zone, span, &keepouts))
                    {
                        contacts.insert(&graph.roots[&format!("via:{i}")]);
                    }
                }
                if roots.is_empty() || !roots.iter().all(|root| contacts.contains(root)) {
                    owned_pending.insert(reference.clone());
                }
                continue;
            }
            if *net == "GND"
                && let Some(graph) = &preferred_graph
                && preferred_pads.contains(reference)
            {
                let nodes = graph.pad_nodes.get(reference).map(Vec::as_slice).unwrap_or(&[]);
                if nodes.len() == 1 {
                    let root = &graph.roots[&nodes[0]];
                    let position = transformed_local_point(placement, lands[0].position);
                    let radius = i128::from(options.maximum_radius_nm) * i128::from(options.maximum_radius_nm);
                    let alone =
                        preferred_roots.get(root).is_some_and(|pads| pads.len() == 1 && pads.contains(reference));
                    if alone
                        && board.vias.iter().enumerate().any(|(i, via)| {
                            via.net == *net
                                && via.from_layer == outer.0
                                && via.to_layer == outer.1
                                && graph.roots.get(&format!("via:{i}")) == Some(root)
                                && squared(via.position, position) <= radius
                                && reference_zones
                                    .iter()
                                    .any(|zone| via_contacts_zone(via.position, zone, layers, &keepouts))
                        })
                    {
                        anchors_by_net.entry((*net).to_owned()).or_default().push((position, side));
                        continue;
                    }
                }
            }
            let group = footprint.internal_pad_groups.iter().find(|g| g.numbers.contains(&reference.pad));
            if let Some(group) = group {
                let key = (placement.reference.clone(), group.numbers.clone());
                if !processed_groups.insert(key) {
                    continue;
                }
                lands = footprint
                    .pads
                    .iter()
                    .filter(|p| group.numbers.contains(&p.number) && p.kind == PadKind::Smd)
                    .collect();
            }
            let rule = rules.get(net).copied();
            let width = rule
                .and_then(|r| r.width_nm)
                .unwrap_or(0)
                .max(options.escape_width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm));
            let mut internal_contact = false;
            for pad in &lands {
                let position = transformed_local_point(placement, pad.position);
                let land_reference = PadReference { component: reference.component.clone(), pad: pad.number.clone() };
                let land_options;
                let options_for = if permitted.contains(&land_reference) {
                    land_options = PlaneStitchOptions { ground_via_in_pad: true, ..options.clone() };
                    &land_options
                } else {
                    &options
                };
                let committed_tracks: Vec<TrackSegment> = board.tracks.iter().chain(&added_tracks).cloned().collect();
                let committed_vias: Vec<Via> = board.vias.iter().chain(&added_vias).cloned().collect();
                let search = LandSearch {
                    board: &board,
                    net,
                    zones: &reference_zones,
                    pad,
                    placement,
                    position,
                    side,
                    width,
                    via_size,
                    via_drill,
                    outer,
                    options: options_for,
                    committed_tracks: &committed_tracks,
                    committed_vias: &committed_vias,
                };
                let (choice, ranked) = search.choose(&clearance, &preferred_via_uses)?;
                if let Some((path, via)) = &choice
                    && ranked
                    && (!path.is_empty() || via.is_some())
                {
                    let target = via.as_ref().map_or_else(|| path[path.len() - 1].end, |via| via.position);
                    *preferred_via_uses.entry(target).or_insert(0) += 1;
                }
                if choice.is_none()
                    && !options.require_explicit
                    && zones.iter().any(|zone| zone.layers.contains(&side) && point_in_zone(position, zone))
                    && !keepouts
                        .iter()
                        .any(|k| k.block_zones && k.layers.contains(&side) && point_in_keepout(position, k))
                {
                    internal_contact = true;
                    continue;
                }
                let Some((tracks, via)) = choice else {
                    if group.is_none() {
                        pending_lands.push(PendingContact {
                            net: (*net).to_owned(),
                            reference: reference.clone(),
                            position,
                            side,
                            width_nm: width,
                        });
                    }
                    continue;
                };
                for track in &tracks {
                    added_tracks.push(track.clone());
                    clearance.add_track(track.clone(), false, None)?;
                }
                if let Some(via) = via {
                    added_vias.push(via.clone());
                    clearance.add_via(via, false, None)?;
                }
                record_contact(
                    &mut copper_uses,
                    &mut anchor_uses,
                    &added_vias,
                    net,
                    reference,
                    position,
                    side,
                    &tracks,
                    Vec::new(),
                );
                anchors_by_net.entry((*net).to_owned()).or_default().push((position, side));
                internal_contact = true;
                if group.is_some() {
                    break; // One verified prospective contact serves this group.
                }
            }
            if group.is_some() && !internal_contact && !lands.is_empty() {
                pending_lands.push(PendingContact {
                    net: (*net).to_owned(),
                    reference: reference.clone(),
                    position: transformed_local_point(placement, lands[0].position),
                    side,
                    width_nm: width,
                });
            }
        }
    }
    if options.require_explicit
        && let Some(only) = &options.only_pads
    {
        // Repair mode: pads outside the selection and the net's through vias
        // inside its zones are anchors.
        let nets: IndexSet<String> = pending_lands.iter().map(|c| c.net.clone()).collect();
        for net in nets {
            for reference in net_pads.get(net.as_str()).copied().into_iter().flatten() {
                if only.contains(reference) || !placements.contains_key(reference.component.as_str()) {
                    continue;
                }
                let placement = placements[reference.component.as_str()];
                if let Some(land) = board.footprints[&placement.footprint]
                    .pads
                    .iter()
                    .find(|item| item.number == reference.pad && item.kind == PadKind::Smd)
                {
                    anchors_by_net
                        .entry(net.clone())
                        .or_default()
                        .push((transformed_local_point(placement, land.position), side_layer(placement)));
                }
            }
            for via in board.vias.iter().chain(&added_vias) {
                if via.net == net
                    && via.from_layer == outer.0
                    && via.to_layer == outer.1
                    && zones_by_net
                        .get(net.as_str())
                        .is_some_and(|zones| zones.iter().any(|zone| point_in_zone(via.position, zone)))
                {
                    for side in [outer.0, outer.1] {
                        anchors_by_net.entry(net.clone()).or_default().push((via.position, side));
                    }
                }
            }
        }
    }
    // A blocked land may still reach the plane through an already escaped
    // same-net land. Retry until no further local chain can be proven.
    let mut unresolved = pending_lands;
    while !unresolved.is_empty() && options.maximum_contact_radius_nm != 0 {
        let limit = i128::from(options.maximum_contact_radius_nm) * i128::from(options.maximum_contact_radius_nm);
        let mut remaining = Vec::new();
        let count = unresolved.len();
        for contact in unresolved {
            let mut anchors: Vec<Point> = anchors_by_net
                .get(&contact.net)
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|(anchor, side)| *side == contact.side && squared(*anchor, contact.position) <= limit)
                .map(|(anchor, _)| *anchor)
                .collect();
            anchors.sort_by_key(|anchor| (squared(*anchor, contact.position), anchor.x_nm, anchor.y_nm));
            let committed: Vec<TrackSegment> = board.tracks.iter().chain(&added_tracks).cloned().collect();
            let mut selected = None;
            for anchor in anchors {
                if let Some(path) = surface_path_detour(
                    &board,
                    &clearance,
                    &contact.net,
                    contact.position,
                    anchor,
                    contact.width_nm,
                    contact.side,
                    &committed,
                    options.maximum_detour_nm,
                    500_000,
                )? {
                    selected = Some((path, anchor));
                    break;
                }
            }
            let Some((path, anchor)) = selected else {
                remaining.push(contact);
                continue;
            };
            for track in &path {
                added_tracks.push(track.clone());
                clearance.add_track(track.clone(), false, None)?;
            }
            let dependency = anchor_uses.get(&(contact.net.clone(), anchor, contact.side)).cloned();
            record_contact(
                &mut copper_uses,
                &mut anchor_uses,
                &added_vias,
                &contact.net,
                &contact.reference,
                contact.position,
                contact.side,
                &path,
                dependency.into_iter().collect(),
            );
            anchors_by_net.entry(contact.net.clone()).or_default().push((contact.position, contact.side));
        }
        let done = remaining.len() == count;
        unresolved = remaining;
        if done {
            break;
        }
    }
    let mut pending_refs: IndexSet<PadReference> = unresolved.iter().map(|item| item.reference.clone()).collect();
    pending_refs.extend(owned_pending);
    // Report every alias of a failed internal group, not just its representative.
    for reference in pending_refs.clone() {
        let footprint = &board.footprints[&placements[reference.component.as_str()].footprint];
        for group in &footprint.internal_pad_groups {
            if group.numbers.contains(&reference.pad) {
                pending_refs.extend(
                    group
                        .numbers
                        .iter()
                        .map(|n| PadReference { component: reference.component.clone(), pad: n.clone() }),
                );
            }
        }
    }
    let stitched: Vec<PadReference> = targets.iter().filter(|r| !pending_refs.contains(*r)).cloned().collect();
    let pending: Vec<PadReference> = targets.iter().filter(|r| pending_refs.contains(*r)).cloned().collect();
    drop(clearance);
    let mut routed = board.clone();
    routed.metadata.insert(
        "plane_stitching".to_owned(),
        if pending.is_empty() { "pad-escapes-only" } else { "partial" }.to_owned(),
    );
    routed.metadata.insert("fabrication_ready".to_owned(), "false".to_owned());
    let filled = added_vias.iter().filter(|via| via.finish == "filled-capped").count() as i64;
    if filled != 0 {
        routed.metadata.insert("via_in_pad_process".to_owned(), "filled-capped".to_owned());
        let previous: i64 = routed.metadata.get("via_in_pad_count").and_then(|v| v.parse().ok()).unwrap_or(0);
        routed.metadata.insert("via_in_pad_count".to_owned(), (previous + filled).to_string());
    }
    let (track_count, via_count) = (added_tracks.len() as i64, added_vias.len() as i64);
    routed.tracks.extend(added_tracks);
    routed.vias.extend(added_vias);
    Ok(PlaneStitchResult {
        board: routed,
        stitched_pads: stitched,
        pending_pads: pending,
        added_track_count: track_count,
        added_via_count: via_count,
        explicit_feeder_pads: explicit_feeders.into_iter().collect(),
        copper_uses,
    })
}

/// `repair_cut_off_plane_pads`: give explicit copper to pads the plane
/// estimate finds walled off; returns the result and the pads still cut off.
pub fn repair_cut_off_plane_pads(
    stitch: PlaneStitchResult,
    options: &PlaneStitchOptions,
    rounds: usize,
) -> Result<(PlaneStitchResult, Vec<PadReference>)> {
    let estimate_of = |board: &PhysicalBoard| {
        crate::plane_estimate::estimate_plane_connectivity(board).map_err(|error| match error {
            crate::unified_error::Error::Value(message) => Error::Value(message),
            other => Error::NotImplemented(other.crash_line()),
        })
    };
    let cut_off = |estimate: &crate::plane_estimate::PlaneEstimate| -> Vec<PadReference> {
        let mut pads: Vec<PadReference> = estimate
            .open_pads
            .values()
            .flatten()
            .map(|pad| {
                let (component, number) = pad.rsplit_once('.').unwrap_or((pad.as_str(), ""));
                PadReference { component: component.to_owned(), pad: number.to_owned() }
            })
            .collect();
        pads.sort();
        pads
    };
    let mut current = stitch;
    let mut estimate = estimate_of(&current.board)?;
    for _ in 0..rounds {
        let pads = cut_off(&estimate);
        if pads.is_empty() {
            break;
        }
        let repair_options = PlaneStitchOptions {
            only_pads: Some(pads.into_iter().collect()),
            require_explicit: true,
            ..options.clone()
        };
        let repair = stitch_zone_pads(&current.board, Some(&repair_options))?;
        if repair.added_track_count == 0 && repair.added_via_count == 0 {
            break;
        }
        let after = estimate_of(&repair.board)?;
        if after.opens >= estimate.opens {
            break;
        }
        let mut uses = current.copper_uses.clone();
        uses.extend(repair.copper_uses.iter().cloned());
        current = PlaneStitchResult {
            board: repair.board,
            stitched_pads: current.stitched_pads,
            pending_pads: current.pending_pads,
            added_track_count: current.added_track_count + repair.added_track_count,
            added_via_count: current.added_via_count + repair.added_via_count,
            explicit_feeder_pads: Vec::new(),
            copper_uses: uses,
        };
        estimate = after;
    }
    let remaining = cut_off(&estimate);
    Ok((current, remaining))
}

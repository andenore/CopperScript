//! `_Router`: the net model, the shared negotiation state and the setup of
//! the unified router (obstacles, rasters, net specs and attach candidates).

use crate::routing_lattice::Lattice;
use std::collections::BTreeSet;

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, TrackSegment, Via};
use copper_pcb::hard_macros::{macro_port_layers, macro_routing_pads, macro_routing_ports, resolved_macro_geometry};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::zones::routing_layers;
use copper_physical::footprint::PadKind;
use copper_physical::geometry::point_segment_distance_squared;
use copper_physical::pyfloat::hypot;
use copper_physical::{CopperLayer, Point};

use super::geometry::spine_distance;
use super::options::UnifiedRouterOptions;
use super::rasters::{FOREIGN, FREE, Obstacle, Rasters};
use crate::package_patterns::{ball_grid, circular_array};
use crate::pin_access::{LocalAccessBounds, SegmentCache, local_access_path};
use crate::pin_escape::{RoutingAccess, checked_access_path, verified_routing_access};
use crate::plane_estimate::label;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_lattice::UniformLattice;
use crate::routing_layers::signal_layer_preferences;
use crate::unified_error::{Error, Result, fail, not_implemented};

pub type Lead = Vec<TrackSegment>;
/// An attach candidate: `(node, lead-in copper, cost in pitches)`.
pub type Attach = (i64, Lead, f64);

/// `_NetSpec`.
#[derive(Clone, Debug)]
pub struct NetSpec {
    pub name: String,
    pub nid: i32,
    pub width: i64,
    pub clearance: i64,
    pub via_size: i64,
    pub via_drill: i64,
    pub layers: Vec<usize>,
    pub components: Vec<Vec<Attach>>,
    pub span: i64,
    pub via_factor: f64,
    pub neck_width: i64,
    pub neck_clearance: i64,
    pub plane: IndexSet<i64>,
    pub region: IndexSet<i64>,
    pub neck: IndexSet<i64>,
    pub neck_mid: IndexSet<i64>,
    pub target_only: Vec<usize>,
    pub optional: usize,
    pub via_budget: Option<i64>,
    pub max_length: Option<i64>,
    pub islands: usize,
    pub joined: IndexMap<usize, IndexSet<i64>>,
    pub explicit: IndexMap<usize, IndexSet<usize>>,
    pub anchors: Vec<Vec<Point>>,
}

/// `_NetRoute`.
#[derive(Clone, Debug, PartialEq)]
pub struct NetRoute {
    pub paths: Vec<Vec<i64>>,
    pub leads: Vec<Lead>,
    pub nodes: IndexSet<i64>,
    pub via_xy: IndexSet<i64>,
    pub claims: IndexSet<i64>,
    pub via_claims: IndexSet<i64>,
    pub connected: bool,
    pub length: f64,
    pub reason: String,
    /// Nodes covered by this route's own copper beyond a default-width
    /// track, mapped to the path node or via site owning them.
    pub body: IndexMap<i64, i64>,
}

impl NetRoute {
    pub fn empty(connected: bool, reason: &str) -> NetRoute {
        NetRoute {
            paths: Vec::new(),
            leads: Vec::new(),
            nodes: IndexSet::new(),
            via_xy: IndexSet::new(),
            claims: IndexSet::new(),
            via_claims: IndexSet::new(),
            connected,
            length: 0.0,
            reason: reason.to_owned(),
            body: IndexMap::new(),
        }
    }
}

/// `_Occupancy`: shared negotiation state.
#[derive(Clone, Debug)]
pub struct Occupancy {
    pub occ: Vec<i32>,
    pub via_occ: Vec<i32>,
    pub history: Vec<f64>,
    pub via_history: Vec<f64>,
}

impl Occupancy {
    pub fn new(size: usize, plane: usize) -> Occupancy {
        Occupancy {
            occ: vec![0; size],
            via_occ: vec![0; plane],
            history: vec![0.0; size],
            via_history: vec![0.0; plane],
        }
    }

    pub fn copy_history(&mut self, other: &Occupancy) {
        self.history = other.history.clone();
        self.via_history = other.via_history.clone();
    }

    pub fn add(&mut self, route: &NetRoute, sign: i32) {
        for &node in &route.claims {
            self.occ[node as usize] += sign;
        }
        for &xy in &route.via_claims {
            self.via_occ[xy as usize] += sign;
        }
    }
}

/// A grid anchor: `(position, layer indexes, outward axis)`.
pub type Anchor = (Point, Vec<usize>, Option<(i64, i64)>);

fn copper_layer_index(layers: &[CopperLayer], layer: CopperLayer) -> Result<usize> {
    layers
        .iter()
        .position(|l| *l == layer)
        .ok_or_else(|| Error::value(format!("{} is not in list", copper_json::python_str_repr(layer.value()))))
}

/// `_nearest_first`: the cover of `position` +- `reach`, nearest first
/// (stable on ties: the cover's x-major order).
pub fn nearest_first(lattice: &UniformLattice, position: Point, reach: f64) -> Vec<(f64, i64, i64, i64)> {
    let mut near: Vec<(f64, i64, i64, i64)> = lattice
        .cover(
            position.x_nm as f64 - reach,
            position.y_nm as f64 - reach,
            position.x_nm as f64 + reach,
            position.y_nm as f64 + reach,
        )
        .into_iter()
        .map(|(column, x, y)| (hypot((x - position.x_nm) as f64, (y - position.y_nm) as f64), column, x, y))
        .collect();
    near.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite distance"));
    near
}

/// The full ball arrays of the board (`_ball_grids`'s grids with points).
pub fn ball_grids(board: &PhysicalBoard) -> Result<Vec<(usize, crate::package_patterns::BallGrid, Vec<Point>)>> {
    let mut result = Vec::new();
    for (index, pose) in board.placements.iter().enumerate() {
        let Ok(Some(grid)) = ball_grid(board, &pose.reference) else { continue };
        let points = board.footprints[&pose.footprint]
            .pads
            .iter()
            .filter(|p| grid.balls.contains(&p.number))
            .map(|p| transformed_local_point(pose, p.position))
            .collect();
        result.push((index, grid, points));
    }
    Ok(result)
}

/// `_aligned`: a lattice of `pitch` has nodes on every ball and channel centre line.
pub fn aligned(grid: &crate::package_patterns::BallGrid, points: &[Point], pitch: i64) -> bool {
    let near = |value: i64| {
        let rest = value.rem_euclid(pitch);
        rest.min(pitch - rest) <= grid.tolerance_nm
    };
    near(grid.pitch_nm / 2) && points.iter().all(|p| near(p.x_nm) && near(p.y_nm))
}

/// `choose_pitch`.
pub fn choose_pitch(board: &PhysicalBoard, default_nm: i64) -> Result<i64> {
    let mut pitch = default_nm;
    for pose in &board.placements {
        let Ok(Some(array)) = circular_array(board, &pose.reference) else { continue };
        let aligned = crate::fine_regions::array_alignment(&array, pose);
        for candidate in [pitch, pitch.div_euclid(2), pitch.div_euclid(4), 50_000] {
            if candidate > 0 && aligned(candidate) {
                pitch = candidate;
                break;
            }
        }
    }
    Ok(pitch)
}

/// `_Router`.
pub struct Router<'p, 'b> {
    pub board: &'p PhysicalBoard,
    pub options: UnifiedRouterOptions,
    pub explicit_pads: IndexSet<PadReference>,
    pub lattice: UniformLattice,
    pub pitch: i64,
    pub via_cost: f64,
    pub rules: IndexMap<String, NetRoutingRule>,
    pub zone_nets: IndexSet<String>,
    pub clearance: Option<RoutingClearanceIndex<'b>>,
    pub net_ids: IndexMap<String, i32>,
    pub rasters: Rasters,
    pub occupancy: Occupancy,
    pub layer_cost: Vec<f64>,
    pub heading: Vec<Option<&'static str>>,
    pub default_width: i64,
    pub seeded_history: Vec<(usize, Vec<i64>, f64)>,
    pub required_accesses: IndexMap<PadReference, RoutingAccess>,
    pub specs: Vec<NetSpec>,
    pub candidate_via_size: i64,
    pub candidate_via_drill: i64,
    pub candidate_clearance: i64,
    pub exit_corridors: IndexMap<String, IndexSet<i64>>,
    pub routes: IndexMap<String, NetRoute>,
    pub lead_nodes: IndexMap<Lead, Vec<i64>>,
    pub lead_history: IndexMap<TrackSegment, f64>,
    pub present: f64,
    pub conflict_age: IndexMap<String, i64>,
    pub deadline_expired: bool,
    pub ignore_soft_costs: bool,
    pub open_cache: IndexMap<(i64, i64), Vec<bool>>,
    pub move_cache: Option<Vec<f64>>,
    pub turn_cache: Option<Vec<f64>>,
    pub local_spent: i64,
    pub segment_cache: Option<SegmentCache>,
    pub escape_bias_cache: IndexMap<String, Option<(Vec<i64>, Vec<f64>)>>,
    pub scratch: super::astar::Scratch,
    pub flags: Vec<u8>,
    pub extra: Vec<f64>,
    /// `(where, layer, blockers, fixed, reason)` of each exact-check rejection.
    pub rejections: IndexMap<String, Vec<(Point, String, Vec<String>, bool, String)>>,
    pub exact_track_conflicts: IndexMap<String, IndexSet<TrackSegment>>,
    pub exact_lead_conflicts: IndexMap<String, IndexSet<TrackSegment>>,
    pub exact_via_conflicts: IndexMap<String, IndexSet<i64>>,
    pub fine_plan: Option<crate::fine_regions::FineRegionPlan>,
}

impl<'p, 'b> Router<'p, 'b> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        board: &'p PhysicalBoard,
        options: &UnifiedRouterOptions,
        nets: &[&PhysicalNet],
        explicit_pads: IndexSet<PadReference>,
        required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
        clearance: RoutingClearanceIndex<'b>,
    ) -> Result<Self> {
        if options.guide_cost != 0.0 {
            return not_implemented("unified router guide_cost (global-route corridors)");
        }
        let fine_plan = super::fine_region_plan(board, options)?;
        let pitch = match &fine_plan {
            None => match options.pitch_nm {
                Some(pitch) if pitch != 0 => pitch,
                _ => choose_pitch(board, super::DEFAULT_PITCH_NM)?,
            },
            Some(plan) => plan.base_pitch_nm,
        };
        let via_cost = options.via_cost_mm * 1_000_000.0 / pitch as f64;
        if let Some(plan) = &fine_plan
            && !plan.patches.is_empty()
        {
            if options.grid_window_nm.is_some() {
                let references: Vec<&str> =
                    plan.patches.iter().flat_map(|p| p.references.iter().map(String::as_str)).collect();
                return fail(format!(
                    "grid_window_nm is not supported with fine_regions patches ({}); set fine_regions=\"off\" for a windowed search",
                    references.join(", ")
                ));
            }
            return not_implemented("unified router nested lattice (fine-region patches)");
        }
        let lattice = UniformLattice::new(board, pitch, options.grid_window_nm).map_err(|error| match error {
            copper_pcb::Error::Value(message) => Error::Value(message),
            copper_pcb::Error::NotImplemented(what) => Error::NotImplemented(what),
        })?;
        if let Some((x0, y0, x1, y1)) = options.grid_window_nm {
            let poses = placement_map(board);
            for net in nets {
                for reference in &net.pads {
                    let pose = &poses[reference.component.as_str()];
                    for land in &board.footprints[&pose.footprint].pads {
                        if land.number != reference.pad
                            || matches!(land.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
                        {
                            continue;
                        }
                        let point = transformed_local_point(pose, land.position);
                        if !(x0 <= point.x_nm && point.x_nm <= x1 && y0 <= point.y_nm && point.y_nm <= y1) {
                            return fail(format!(
                                "terminal {}.{} is outside grid_window_nm",
                                reference.component, reference.pad
                            ));
                        }
                    }
                }
            }
        }
        let rules: IndexMap<String, NetRoutingRule> =
            board.net_routing_rules.iter().map(|r| (r.net.clone(), r.clone())).collect();
        let zone_nets: IndexSet<String> = board.zones.iter().map(|z| z.net.clone()).collect();
        let mut net_ids = IndexMap::new();
        for net in &board.nets {
            let next = net_ids.len() as i32;
            net_ids.entry(net.name.clone()).or_insert(next);
        }
        let size = lattice.size as usize;
        let plane = lattice.plane as usize;
        let nl = lattice.nl as usize;
        let mut router = Router {
            board,
            options: options.clone(),
            explicit_pads,
            pitch,
            via_cost,
            rules,
            zone_nets,
            clearance: Some(clearance),
            net_ids,
            rasters: Rasters::new(board, &[], 0, false, Vec::new(), Vec::new(), vec![0], Vec::new()),
            occupancy: Occupancy::new(size, plane),
            layer_cost: Vec::new(),
            heading: Vec::new(),
            default_width: board.rules.default_track_width_nm,
            seeded_history: Vec::new(),
            required_accesses: IndexMap::new(),
            specs: Vec::new(),
            candidate_via_size: 0,
            candidate_via_drill: 0,
            candidate_clearance: 0,
            exit_corridors: IndexMap::new(),
            routes: IndexMap::new(),
            lead_nodes: IndexMap::new(),
            lead_history: IndexMap::new(),
            present: options.present_factor,
            conflict_age: IndexMap::new(),
            deadline_expired: false,
            ignore_soft_costs: false,
            open_cache: IndexMap::new(),
            move_cache: None,
            turn_cache: None,
            local_spent: 0,
            segment_cache: Some(SegmentCache::new()),
            escape_bias_cache: IndexMap::new(),
            scratch: super::astar::Scratch::new(size),
            flags: vec![0; size],
            extra: vec![0.0; size],
            rejections: IndexMap::new(),
            exact_track_conflicts: IndexMap::new(),
            exact_lead_conflicts: IndexMap::new(),
            exact_via_conflicts: IndexMap::new(),
            fine_plan,
            lattice,
        };
        let (obstacles, holes) = router.obstacles()?;
        let margin = options.raster_margin_nm.unwrap_or(pitch.div_euclid(8));
        let exact = if options.ball_field_exact { router.ball_field_boxes()? } else { Vec::new() };
        router.rasters = Rasters::new(board, &router.lattice.blocks, nl, false, obstacles, holes, vec![margin], exact);
        if options.ball_field_via_sites {
            return not_implemented("unified router ball_field_via_sites");
        }
        let (_, headings) = signal_layer_preferences(board);
        let g = &router.lattice;
        router.layer_cost =
            (0..nl).map(|i| if i == 0 || i == nl - 1 { options.outer_layer_cost } else { 0.0 }).collect();
        router.heading = g.layers.iter().map(|layer| headings.get(layer).copied()).collect();
        router.seed_ball_field_cost()?;
        let mut net_by_pad: IndexMap<&PadReference, &str> = IndexMap::new();
        for net in &board.nets {
            for pad in &net.pads {
                net_by_pad.insert(pad, &net.name);
            }
        }
        let selected: IndexSet<&str> = nets.iter().map(|net| net.name.as_str()).collect();
        for (pad, access) in required_accesses.into_iter().flatten() {
            let Some(name) = net_by_pad.get(pad) else {
                return fail(format!("required routing access has no net: {pad}"));
            };
            if !selected.contains(name) {
                continue;
            }
            if verified_routing_access(board, pad, name, access, router.clearance.as_ref().expect("index"))?.is_none() {
                return fail(format!("required routing access is stale or invalid: {pad}"));
            }
            router.required_accesses.insert(pad.clone(), access.clone());
        }
        let mut specs = Vec::new();
        for net in nets {
            if let Some(spec) = router.spec(net)? {
                specs.push(spec);
            }
        }
        router.specs = specs;
        router.segment_cache = None;
        let rules = &board.rules;
        router.candidate_via_size = std::iter::once(rules.default_via_size_nm)
            .chain(router.specs.iter().map(|s| s.via_size))
            .max()
            .expect("one");
        router.candidate_via_drill = std::iter::once(rules.default_via_drill_nm)
            .chain(router.specs.iter().map(|s| s.via_drill))
            .max()
            .expect("one");
        router.candidate_clearance = std::iter::once(rules.minimum_clearance_nm)
            .chain(router.specs.iter().map(|s| s.clearance))
            .max()
            .expect("one");
        router.exit_corridors = router.seed_exit_corridors()?;
        Ok(router)
    }

    // -- setup ------------------------------------------------------------

    fn seed_ball_field_cost(&mut self) -> Result<()> {
        let cost = self.options.ball_field_cost;
        if cost == 0.0 {
            return Ok(());
        }
        let board = self.board;
        for pose in &board.placements {
            let Ok(Some(_)) = circular_array(board, &pose.reference) else { continue };
            let corners: Vec<Point> = board.footprints[&pose.footprint]
                .pads
                .iter()
                .map(|p| transformed_local_point(pose, p.position))
                .collect();
            let bounds = (
                corners.iter().map(|p| p.x_nm).min().expect("pads") as f64,
                corners.iter().map(|p| p.y_nm).min().expect("pads") as f64,
                corners.iter().map(|p| p.x_nm).max().expect("pads") as f64,
                corners.iter().map(|p| p.y_nm).max().expect("pads") as f64,
            );
            let side = if pose.side.value() == "front" { CopperLayer::Front } else { CopperLayer::Back };
            let layer = copper_layer_index(&self.lattice.layers, side)?;
            for block in &self.lattice.blocks {
                let columns = block.window_columns(block.window(bounds.0, bounds.1, bounds.2, bounds.3), None);
                if columns.is_empty() {
                    continue;
                }
                for &column in &columns {
                    self.occupancy.history[(column + layer as i64 * self.lattice.plane) as usize] += cost;
                }
                self.seeded_history.push((layer, columns, cost));
            }
        }
        Ok(())
    }

    fn seed_exit_corridors(&mut self) -> Result<IndexMap<String, IndexSet<i64>>> {
        if self.options.exit_reservation_cost == 0.0 {
            return Ok(IndexMap::new());
        }
        not_implemented("unified router exit_reservation_cost (ball-grid exit corridors)")
    }

    fn ball_field_boxes(&self) -> Result<Vec<(i64, i64, i64, i64)>> {
        let mut boxes = Vec::new();
        for (_, array, points) in ball_grids(self.board)? {
            let step = self.lattice.step_nm(self.lattice.nearest(0, points[0]));
            if !aligned(&array, &points, step) {
                continue;
            }
            let grow = self.options.ball_field_exact_reach_nm.filter(|v| *v != 0).unwrap_or(array.pitch_nm);
            boxes.push((
                points.iter().map(|p| p.x_nm).min().expect("balls") - grow,
                points.iter().map(|p| p.y_nm).min().expect("balls") - grow,
                points.iter().map(|p| p.x_nm).max().expect("balls") + grow,
                points.iter().map(|p| p.y_nm).max().expect("balls") + grow,
            ));
        }
        Ok(boxes)
    }

    pub fn net_id(&mut self, name: &str) -> i32 {
        let next = self.net_ids.len() as i32;
        *self.net_ids.entry(name.to_owned()).or_insert(next)
    }

    fn obstacles(&mut self) -> Result<(Vec<Obstacle>, Vec<super::rasters::Hole>)> {
        let layers = self.lattice.layers.clone();
        let index_of = |list: &[CopperLayer]| -> Vec<usize> {
            list.iter().filter_map(|l| layers.iter().position(|x| x == l)).collect()
        };
        let mut obstacles = Vec::new();
        let objects: Vec<_> = self.clearance.as_ref().expect("index").objects().into_iter().cloned().collect();
        for item in &objects {
            let netless = item.net.starts_with('<');
            let rule_floor =
                if netless { 0 } else { self.rules.get(&item.net).map_or(0, |r| r.clearance_nm.unwrap_or(0)) };
            let floor = item.clearance_nm.max(rule_floor);
            let owner = if netless { FOREIGN } else { self.net_id(&item.net) };
            obstacles.push(Obstacle {
                owner,
                layers: index_of(&item.layers),
                spine: item.shape.spine.clone(),
                radius: item.shape.radius_nm,
                clearance: floor,
                is_pad: item.is_pad,
                blocks_tracks: true,
                blocks_vias: true,
                polygon: false,
            });
        }
        for keepout in self.clearance.as_ref().expect("index").keepouts() {
            obstacles.push(Obstacle {
                owner: FOREIGN,
                layers: index_of(&keepout.layers),
                spine: keepout.shape.spine.clone(),
                radius: keepout.shape.radius_nm,
                clearance: keepout.clearance_nm,
                is_pad: false,
                blocks_tracks: keepout.block_tracks,
                blocks_vias: keepout.block_vias,
                polygon: false,
            });
        }
        for (region_layers, shape) in self.clearance.as_ref().expect("index").macro_regions() {
            obstacles.push(Obstacle {
                owner: FOREIGN,
                layers: index_of(region_layers),
                spine: shape.spine.clone(),
                radius: shape.radius_nm,
                clearance: 0,
                is_pad: false,
                blocks_tracks: true,
                blocks_vias: true,
                polygon: false,
            });
        }
        let reservations = &self.clearance.as_ref().expect("index").zone_reservations;
        let zones: Vec<(String, Vec<usize>, Vec<Point>, i64, i64)> = reservations
            .zones
            .iter()
            .map(|(zone, outer)| {
                let floor = reservations
                    .minimum_clearance_nm
                    .max(zone.clearance_nm.unwrap_or(0))
                    .max(reservations.clearances.get(&zone.net).copied().unwrap_or(0));
                (zone.net.clone(), index_of(&zone.layers), outer.spine.clone(), outer.radius_nm, floor)
            })
            .collect();
        for (net, zone_layers, spine, radius, floor) in zones {
            let owner = self.net_id(&net);
            obstacles.push(Obstacle {
                owner,
                layers: zone_layers,
                spine,
                radius,
                clearance: floor,
                is_pad: false,
                blocks_tracks: true,
                blocks_vias: false,
                polygon: true,
            });
        }
        let hole_list: Vec<_> = self.clearance.as_ref().expect("index").holes().into_iter().cloned().collect();
        let mut holes = Vec::new();
        for hole in &hole_list {
            let owner = if hole.net.starts_with('<') { FOREIGN } else { self.net_id(&hole.net) };
            holes.push((owner, hole.position.x_nm as f64, hole.position.y_nm as f64, hole.radius_nm as f64));
        }
        Ok((obstacles, holes))
    }

    fn pad_anchors(&self, pad: &PadReference) -> Result<Vec<Anchor>> {
        let board = self.board;
        let pose = board.placements.iter().find(|p| p.reference == pad.component).expect("placed component");
        let footprint = &board.footprints[&pose.footprint];
        let mut result = Vec::new();
        for land in &footprint.pads {
            if land.number != pad.pad || matches!(land.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
                continue;
            }
            let position = transformed_local_point(pose, land.position);
            let layers = if land.kind == PadKind::Smd {
                let side = if pose.side.value() == "front" { CopperLayer::Front } else { CopperLayer::Back };
                vec![copper_layer_index(&self.lattice.layers, side)?]
            } else {
                (0..self.lattice.nl as usize).collect()
            };
            let (dx, dy) = (position.x_nm - pose.position.x_nm, position.y_nm - pose.position.y_nm);
            let mut outward = None;
            if dx.abs().max(dy.abs()) > self.options.outward_axis_threshold_nm {
                outward = Some(if dx.abs() >= dy.abs() {
                    (if dx > 0 { 1 } else { -1 }, 0)
                } else {
                    (0, if dy > 0 { 1 } else { -1 })
                });
            }
            result.push((position, layers, outward));
        }
        Ok(result)
    }

    fn spec(&mut self, net: &PhysicalNet) -> Result<Option<NetSpec>> {
        let board = self.board;
        let rule = self.rules.get(&net.name).cloned();
        let width =
            rule.as_ref().and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
        let clearance = board.rules.minimum_clearance_nm.max(rule.as_ref().and_then(|r| r.clearance_nm).unwrap_or(0));
        let mut allowed: IndexSet<CopperLayer> = routing_layers(board, &net.name, rule.as_ref()).into_iter().collect();
        if rule.as_ref().is_some_and(|r| r.max_vias == Some(0)) {
            let mut pad_layers = IndexSet::new();
            for pad in &net.pads {
                for (_, indexes, _) in self.pad_anchors(pad)? {
                    pad_layers.extend(indexes.into_iter().map(|i| self.lattice.layers[i]));
                }
            }
            allowed.retain(|layer| pad_layers.contains(layer));
        }
        let mut target_only = Vec::new();
        if self.zone_nets.contains(&net.name) {
            let zone_layers: IndexSet<CopperLayer> =
                board.zones.iter().filter(|z| z.net == net.name).flat_map(|z| z.layers.iter().copied()).collect();
            target_only = self
                .lattice
                .layers
                .iter()
                .enumerate()
                .filter(|(_, l)| zone_layers.contains(*l) && !allowed.contains(*l))
                .map(|(i, _)| i)
                .collect();
            allowed.extend(zone_layers);
        }
        let layers: Vec<usize> =
            self.lattice.layers.iter().enumerate().filter(|(_, l)| allowed.contains(*l)).map(|(i, _)| i).collect();
        if layers.is_empty() {
            return Ok(None);
        }
        let nid = self.net_id(&net.name);
        let mut spec = NetSpec {
            name: net.name.clone(),
            nid,
            width,
            clearance,
            via_size: board.rules.default_via_size_nm,
            via_drill: board.rules.default_via_drill_nm,
            layers,
            components: Vec::new(),
            span: 0,
            via_factor: 1.0,
            neck_width: 0,
            neck_clearance: 0,
            plane: IndexSet::new(),
            region: IndexSet::new(),
            neck: IndexSet::new(),
            neck_mid: IndexSet::new(),
            target_only,
            optional: 0,
            via_budget: None,
            max_length: None,
            islands: 0,
            joined: IndexMap::new(),
            explicit: IndexMap::new(),
            anchors: Vec::new(),
        };
        if let Some(rule) = &rule {
            if let Some(max_vias) = rule.max_vias {
                spec.via_factor = self.options.via_budget_factor;
                let existing = board.vias.iter().filter(|via| via.net == net.name).count() as i64;
                spec.via_budget = Some(0.max(max_vias - existing));
            }
            if let Some(length) = rule.max_length_nm.filter(|l| *l != 0) {
                spec.max_length = Some(length);
            }
        }
        let breakout = rule.as_ref().and_then(|r| match (r.breakout_length_nm, r.breakout_width_nm) {
            (Some(length), Some(neck)) if length != 0 && neck != 0 && neck < width => Some((length, neck)),
            _ => None,
        });
        if let Some((length, neck)) = breakout {
            let rule = rule.as_ref().expect("rule");
            spec.neck_width = neck.max(board.rules.minimum_track_width_nm);
            spec.neck_clearance = board
                .rules
                .minimum_clearance_nm
                .max(rule.breakout_clearance_nm.filter(|v| *v != 0).or(rule.clearance_nm).unwrap_or(0));
            self.neck_nodes(net, &mut spec, length)?;
        } else if self.options.implicit_neck_nm != 0
            && rule.as_ref().is_none_or(|r| r.width_nm.is_none())
            && width > board.rules.minimum_track_width_nm
        {
            spec.neck_width = board.rules.minimum_track_width_nm;
            spec.neck_clearance = clearance;
            let length = self.options.implicit_neck_nm;
            self.neck_nodes(net, &mut spec, length)?;
        }
        spec.components = self.components(net, &mut spec)?;
        if self.zone_nets.contains(&net.name) {
            let (mut planes, islands, optional) = self.zone_regions(&net.name, &spec);
            if self.options.reuse_plane_vias {
                let taps = self.plane_via_taps(&net.name, &spec, &planes)?;
                planes.extend(taps);
            }
            spec.plane = planes;
            spec.islands = islands.len() + optional.len();
            spec.optional = optional.len();
            for island in islands.into_iter().chain(optional) {
                spec.components.push(island.into_iter().map(|node| (node, Vec::new(), 0.0)).collect());
            }
        }
        let points: Vec<Point> =
            spec.components.iter().flat_map(|c| c.iter().map(|(node, _, _)| self.lattice.point(*node))).collect();
        spec.span = if points.is_empty() {
            0
        } else {
            let span = points.iter().map(|p| p.x_nm).max().unwrap() - points.iter().map(|p| p.x_nm).min().unwrap()
                + points.iter().map(|p| p.y_nm).max().unwrap()
                - points.iter().map(|p| p.y_nm).min().unwrap();
            span.div_euclid(self.pitch)
        };
        Ok(Some(spec))
    }

    /// `_neck_nodes`: breakout-region nodes and those legal there only at the neck width.
    fn neck_nodes(&mut self, net: &PhysicalNet, spec: &mut NetSpec, length: i64) -> Result<()> {
        let board = self.board;
        let main = self.rasters.track(spec.width, spec.clearance).0.to_vec();
        let main_mid = self.rasters.track(spec.width, spec.clearance).1.to_vec();
        let neck = self.rasters.track(spec.neck_width, spec.neck_clearance).0.to_vec();
        let neck_mid = self.rasters.track(spec.neck_width, spec.neck_clearance).1.to_vec();
        let nid = spec.nid;
        let poses = placement_map(board);
        let free = |owner: i32| owner == FREE || owner == nid;
        for pad in &net.pads {
            let pose = &poses[pad.component.as_str()];
            for land in &board.footprints[&pose.footprint].pads {
                if land.number != pad.pad {
                    continue;
                }
                let shape = placed_pad_shape(transformed_local_point(pose, land.position), land, pose);
                let reach = shape.radius_nm + length;
                let min_x = shape.spine.iter().map(|p| p.x_nm).min().expect("spine");
                let min_y = shape.spine.iter().map(|p| p.y_nm).min().expect("spine");
                let max_x = shape.spine.iter().map(|p| p.x_nm).max().expect("spine");
                let max_y = shape.spine.iter().map(|p| p.y_nm).max().expect("spine");
                for view in &self.rasters.views {
                    let block = &view.block;
                    let window = block.window(
                        (min_x - reach) as f64,
                        (min_y - reach) as f64,
                        (max_x + reach) as f64,
                        (max_y + reach) as f64,
                    );
                    let (ix0, iy0, ix1, iy1) = window;
                    if ix0 > ix1 || iy0 > iy1 {
                        continue;
                    }
                    let limit = (reach - block.pitch.div_euclid(8)) as f64;
                    let mut inside = Vec::new();
                    for iy in iy0..=iy1 {
                        for ix in ix0..=ix1 {
                            let at = (iy * block.nx + ix) as usize;
                            inside.push(spine_distance(&shape.spine, view.px[at], view.py[at]) < limit);
                        }
                    }
                    let mask = crate::routing_lattice::Mask { width: (ix1 - ix0 + 1) as usize, values: &inside };
                    for column in block.window_columns(window, Some(&mask)) {
                        for &layer in &spec.layers {
                            let node = self.lattice.layer_node(column, layer as i64);
                            spec.region.insert(node);
                            let n = node as usize;
                            if !free(main[n]) && free(neck[n]) {
                                spec.neck.insert(node);
                            }
                            if !free(main_mid[n]) && free(neck_mid[n]) {
                                spec.neck_mid.insert(node);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `_components`: group already-joined pads; attach candidates per group.
    fn components(&mut self, net: &PhysicalNet, spec: &mut NetSpec) -> Result<Vec<Vec<Attach>>> {
        let board = self.board;
        let only: IndexSet<String> = IndexSet::from([net.name.clone()]);
        let graph = explicit_copper_connectivity(board, Candidate { only_nets: Some(&only), ..Candidate::default() })?;
        let pads = macro_routing_pads(board, net)?;
        let mut groups: IndexMap<String, Vec<PadReference>> = IndexMap::new();
        for pad in &pads {
            let root = match graph.pad_nodes.get(pad).and_then(|nodes| nodes.first()) {
                Some(node) => graph.roots[node].clone(),
                None => format!("pad:{}.{}", pad.component, pad.pad),
            };
            groups.entry(root).or_default().push(pad.clone());
        }
        let ports = macro_routing_ports(board, &net.name)?;
        // `macro_owned_pads` for zone nets.
        let owned: IndexSet<&PadReference> = if self.zone_nets.contains(&net.name) {
            board
                .hard_macros
                .iter()
                .filter(|m| board.materialized_macros.contains(&m.cluster))
                .flat_map(|m| m.pad_bindings.iter().map(|(pad, _)| pad))
                .collect()
        } else {
            IndexSet::new()
        };
        let mut macro_tracks: IndexSet<TrackSegment> = IndexSet::new();
        let mut macro_vias: IndexSet<Via> = IndexSet::new();
        let poses_by_ref = placement_map(board);
        for hard in &board.hard_macros {
            if board.materialized_macros.contains(&hard.cluster) {
                let geometry = resolved_macro_geometry(board, hard, &poses_by_ref)?;
                macro_tracks.extend(geometry.tracks);
                macro_vias.extend(geometry.vias);
            }
        }
        let mut components = Vec::new();
        let mut separate: Vec<Vec<Anchor>> = Vec::new();
        let mut ordered: Vec<(String, Vec<PadReference>)> = groups.into_iter().collect();
        ordered.sort_by_key(|(_, members)| members[0].to_string());
        let layers = self.lattice.layers.clone();
        for (root, members) in ordered {
            if !owned.is_empty()
                && members.iter().all(|pad| owned.contains(pad))
                && !members.iter().any(|pad| ports.contains_key(pad))
            {
                continue;
            }
            let mut anchors: Vec<Anchor> = Vec::new();
            for pad in &members {
                if let Some(port) = ports.get(pad) {
                    let indexes = macro_port_layers(board, port)?
                        .iter()
                        .map(|l| copper_layer_index(&layers, *l))
                        .collect::<Result<_>>()?;
                    anchors.push((port.position, indexes, None));
                    continue;
                }
                let mut pad_anchors = self.pad_anchors(pad)?;
                for piece in self.separate_land_pieces(pad)? {
                    separate.push(pad_anchors.iter().filter(|a| piece.contains(&a.0)).cloned().collect());
                    pad_anchors.retain(|a| !piece.contains(&a.0));
                }
                anchors.extend(pad_anchors);
            }
            let mut joined: IndexSet<i64> = IndexSet::new();
            let mut host_anchors: Vec<Anchor> = Vec::new();
            let mut host_joined: IndexSet<i64> = IndexSet::new();
            let mut host_tracks: Vec<(TrackSegment, usize)> = Vec::new();
            for (index, via) in board.vias.iter().enumerate() {
                if via.net != net.name || graph.roots.get(&format!("via:{index}")) != Some(&root) {
                    continue;
                }
                let a = copper_layer_index(&layers, via.from_layer)?;
                let b = copper_layer_index(&layers, via.to_layer)?;
                let span: Vec<usize> = (a.min(b)..=a.max(b)).collect();
                anchors.push((via.position, span.clone(), None));
                let nodes: Vec<i64> = match self.lattice.snap(0, via.position) {
                    None => Vec::new(),
                    Some(site) => {
                        span.iter().map(|&l| self.lattice.layer_node(self.lattice.column(site), l as i64)).collect()
                    }
                };
                joined.extend(nodes.iter().copied());
                if !macro_vias.contains(via) {
                    host_anchors.push((via.position, span, None));
                    host_joined.extend(nodes);
                }
            }
            for (index, track) in board.tracks.iter().enumerate() {
                if track.net == net.name && graph.roots.get(&format!("track:{index}")) == Some(&root) {
                    let layer = copper_layer_index(&layers, track.layer)?;
                    anchors.push((track.start, vec![layer], None));
                    anchors.push((track.end, vec![layer], None));
                    if !macro_tracks.contains(track) {
                        host_anchors.push((track.start, vec![layer], None));
                        host_anchors.push((track.end, vec![layer], None));
                        host_tracks.push((track.clone(), layer));
                    }
                }
            }
            let boundary: Vec<&copper_pcb::board::MacroPort> =
                members.iter().filter_map(|pad| ports.get(pad)).collect();
            if !boundary.is_empty() {
                anchors = boundary
                    .iter()
                    .map(|port| {
                        let indexes = macro_port_layers(board, port)?
                            .iter()
                            .map(|l| copper_layer_index(&layers, *l))
                            .collect::<Result<Vec<_>>>()?;
                        Ok((port.position, indexes, None))
                    })
                    .collect::<Result<_>>()?;
                let mut public: IndexSet<i64> = IndexSet::new();
                for (position, indexes, _) in &anchors {
                    for &l in indexes {
                        if let Some(node) = self.lattice.snap(l as i64, *position) {
                            public.insert(node);
                        }
                    }
                }
                joined.retain(|node| public.contains(node) || host_joined.contains(node));
                anchors.extend(host_anchors);
            }
            let required: Vec<&RoutingAccess> =
                members.iter().filter_map(|pad| self.required_accesses.get(pad)).collect();
            if !required.is_empty() {
                anchors = required
                    .iter()
                    .map(|access| Ok((access.position, vec![copper_layer_index(&layers, access.layer)?], None)))
                    .collect::<Result<_>>()?;
                host_tracks.clear();
            }
            if !joined.is_empty() {
                spec.joined.insert(components.len(), joined);
            }
            let mut component = self.attach(&anchors, spec)?;
            let taps = self.track_taps(&host_tracks, spec);
            if !taps.is_empty() {
                component.retain(|c| !taps.contains(&c.0));
                component.extend(taps.iter().map(|&node| (node, Vec::new(), 0.0)));
            }
            components.push(component);
            spec.anchors.push(anchors.iter().map(|a| a.0).collect());
            if !self.explicit_pads.is_empty() && members.iter().any(|pad| self.explicit_pads.contains(pad)) {
                let exclusion = self.explicit_exclusion(&members)?;
                spec.explicit.insert(components.len() - 1, exclusion);
            }
        }
        for anchors in separate {
            let component = self.attach(&anchors, spec)?;
            components.push(component);
            spec.anchors.push(anchors.iter().map(|a| a.0).collect());
        }
        Ok(components)
    }

    /// `_separate_land_pieces`: land positions of each piece not joined to the first land.
    fn separate_land_pieces(&self, pad: &PadReference) -> Result<Vec<IndexSet<Point>>> {
        let board = self.board;
        let pose = board.placements.iter().find(|p| p.reference == pad.component).expect("placed component");
        let footprint = &board.footprints[&pose.footprint];
        if footprint.internal_pad_groups.iter().any(|group| group.numbers.contains(&pad.pad)) {
            return Ok(Vec::new());
        }
        Ok(crate::detailed::land_pieces(pose, footprint, &pad.pad)?
            .into_iter()
            .skip(1)
            .map(|piece| piece.into_iter().map(|(_, position)| position).collect())
            .collect())
    }

    /// `_track_taps`: grid nodes inside existing same-net tracks.
    fn track_taps(&mut self, tracks: &[(TrackSegment, usize)], spec: &NetSpec) -> BTreeSet<i64> {
        let owner = self.rasters.track(spec.width, spec.clearance).0;
        let g = &self.lattice;
        let mut taps = BTreeSet::new();
        for (track, layer) in tracks {
            if !spec.layers.contains(layer) {
                continue;
            }
            let (start, end) = (track.start, track.end);
            let half = track.width_nm.div_euclid(2) as i128;
            let length = hypot((end.x_nm - start.x_nm) as f64, (end.y_nm - start.y_nm) as f64);
            let steps = 1.max((2.0 * length / g.pitch_nm as f64).ceil() as i64);
            for i in 0..=steps {
                let sample = Point::new(
                    start.x_nm + ((end.x_nm - start.x_nm) * i).div_euclid(steps),
                    start.y_nm + ((end.y_nm - start.y_nm) * i).div_euclid(steps),
                );
                let node = g.nearest(*layer as i64, sample);
                let o = owner[node as usize];
                if taps.contains(&node) || (o != FREE && o != spec.nid && !spec.neck.contains(&node)) {
                    continue;
                }
                let squared = point_segment_distance_squared(g.point(node), start, end);
                if squared <= copper_physical::geometry::rational(half * half) {
                    taps.insert(node);
                }
            }
        }
        taps
    }

    /// `_explicit_exclusion`: the surface layers of these pads.
    fn explicit_exclusion(&self, pads: &[PadReference]) -> Result<IndexSet<usize>> {
        let poses = placement_map(self.board);
        let mut result = IndexSet::new();
        for pad in pads {
            if let Some(pose) = poses.get(&pad.component) {
                let side = if pose.side.value() == "front" { CopperLayer::Front } else { CopperLayer::Back };
                result.insert(copper_layer_index(&self.lattice.layers, side)?);
            }
        }
        Ok(result)
    }

    /// `_zone_regions`: plane target nodes of a zone net, its separate pours and fragments.
    fn zone_regions(&mut self, name: &str, spec: &NetSpec) -> (IndexSet<i64>, Vec<Vec<i64>>, Vec<Vec<i64>>) {
        let (fill_width, fill_clearance) = if self.options.plane_fill_width {
            (self.board.rules.minimum_track_width_nm, self.board.rules.minimum_clearance_nm)
        } else {
            (spec.width, spec.clearance)
        };
        let owner = self.rasters.track(fill_width, fill_clearance).0.to_vec();
        let g = &self.lattice;
        let (nl, ny, nx) = (g.nl as usize, g.ny as usize, g.nx as usize);
        let plane = nx * ny;
        let view = &self.rasters.views[0];
        let mut per_layer: IndexMap<usize, Vec<bool>> = IndexMap::new();
        for zone in self.board.zones.iter().filter(|z| z.net == name) {
            let inside: Vec<bool> = (0..plane)
                .map(|at| {
                    let (x, y) = (view.px[at], view.py[at]);
                    super::geometry::polygon_inside(&zone.outer, x, y)
                        && zone.holes.iter().all(|hole| !super::geometry::polygon_inside(hole, x, y))
                })
                .collect();
            for layer in &zone.layers {
                let index = g.layers.iter().position(|l| l == layer).expect("zone layer");
                let mask: Vec<bool> = (0..plane)
                    .map(|at| {
                        let o = owner[index * plane + at];
                        inside[at] && (o == FREE || o == spec.nid)
                    })
                    .collect();
                match per_layer.get_mut(&index) {
                    Some(existing) => {
                        for (e, m) in existing.iter_mut().zip(mask) {
                            *e |= m;
                        }
                    }
                    None => {
                        per_layer.insert(index, mask);
                    }
                }
            }
        }
        let _ = nl;
        let minimum = 4.max(self.options.plane_piece_area_nm2.div_euclid(g.pitch * g.pitch));
        let mut pieces: IndexMap<usize, Vec<Vec<i64>>> = IndexMap::new();
        for (&index, mask) in &per_layer {
            let (labels, count) = label(mask, ny, nx);
            if count == 0 {
                continue;
            }
            let mut sizes = vec![0i64; count + 1];
            for &l in &labels {
                sizes[l as usize] += 1;
            }
            // np.argsort(-sizes[1:]): ties keep label order.
            let mut order: Vec<usize> = (1..=count).collect();
            order.sort_by_key(|&l| -sizes[l]);
            let mut found = Vec::new();
            for (k, &l) in order.iter().enumerate() {
                if sizes[l] < minimum && k != 0 {
                    break;
                }
                found.push(
                    labels
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| **v as usize == l)
                        .map(|(at, _)| g.layer_node(at as i64, index as i64))
                        .collect::<Vec<i64>>(),
                );
            }
            pieces.insert(index, found);
        }
        if pieces.is_empty() {
            return (IndexSet::new(), Vec::new(), Vec::new());
        }
        let largest = pieces.values().map(|p| p[0].len()).max().expect("pieces") as f64;
        let mut planes = IndexSet::new();
        let (mut islands, mut optional) = (Vec::new(), Vec::new());
        for found in pieces.values() {
            let first = &found[0];
            if first.len() as f64 >= largest * self.options.plane_piece_fraction {
                planes.extend(first.iter().copied());
            } else {
                islands.push(first.clone());
            }
            optional.extend(found[1..].iter().cloned());
        }
        (planes, islands, optional)
    }

    /// `_plane_via_taps`: nodes at existing plane vias that already land in the plane.
    fn plane_via_taps(&self, name: &str, spec: &NetSpec, planes: &IndexSet<i64>) -> Result<IndexSet<i64>> {
        let g = &self.lattice;
        let layers_all = &g.layers;
        let mut taps = IndexSet::new();
        for via in &self.board.vias {
            if via.net != name {
                continue;
            }
            let Some(site) = g.snap(0, via.position) else { continue };
            let column = g.column(site);
            let (a, b) =
                (copper_layer_index(layers_all, via.from_layer)?, copper_layer_index(layers_all, via.to_layer)?);
            let span: Vec<usize> = (a..=b).collect();
            let step = g.step_nm(site) as f64;
            let reach = 1
                .max(copper_physical::pyfloat::round_int((via.size_nm as f64 / 2.0 + spec.clearance as f64) / step))
                + 1;
            let around = [(reach, 0), (-reach, 0), (0, reach), (0, -reach)];
            let touch = 1.max((via.size_nm as f64 / 2.0 / step).ceil() as i64);
            let square: Vec<(i64, i64)> =
                (-touch..=touch).flat_map(|dx| (-touch..=touch).map(move |dy| (dx, dy))).collect();
            let surrounded = |index: usize| {
                let node = g.layer_node(column, index as i64);
                if self.options.plane_fill_width {
                    return g.offset_nodes(node, &square).iter().filter(|n| planes.contains(*n)).count() >= 2;
                }
                let ring = g.offset_nodes(node, &around);
                ring.len() == around.len() && ring.iter().all(|n| planes.contains(n))
            };
            if !span.iter().any(|&index| surrounded(index)) {
                continue;
            }
            for &index in &span {
                if spec.layers.contains(&index) {
                    taps.insert(g.layer_node(column, index as i64));
                }
            }
        }
        Ok(taps)
    }

    /// `_attach`: grid nodes reachable from anchors with exact, checked lead-ins.
    fn attach(&mut self, anchors: &[Anchor], spec: &NetSpec) -> Result<Vec<Attach>> {
        let owner = self.rasters.track(spec.width, spec.clearance).0.to_vec();
        let radius = self.options.attach_radius_nm;
        let mut seen: IndexMap<i64, Attach> = IndexMap::new();
        for (position, layers, outward) in anchors {
            let near = nearest_first(&self.lattice, *position, radius as f64);
            for &layer in layers {
                if !spec.layers.contains(&layer) {
                    continue;
                }
                let copper = self.lattice.layers[layer];
                let mut found = 0;
                for &(distance, column, x, y) in &near {
                    if distance > radius as f64 || found >= self.options.attach_candidates {
                        break;
                    }
                    let node = self.lattice.layer_node(column, layer as i64);
                    let o = owner[node as usize];
                    if o != FREE && o != spec.nid && !spec.neck.contains(&node) {
                        continue;
                    }
                    let target = Point::new(x, y);
                    let mut path = None;
                    if !spec.neck.contains(&node) {
                        path = checked_access_path(
                            self.board,
                            self.clearance.as_ref().expect("index"),
                            &spec.name,
                            *position,
                            target,
                            spec.width,
                            copper,
                            false,
                            true,
                            None,
                        )?;
                    }
                    if path.is_none() && spec.neck_width != 0 {
                        path = checked_access_path(
                            self.board,
                            self.clearance.as_ref().expect("index"),
                            &spec.name,
                            *position,
                            target,
                            spec.neck_width,
                            copper,
                            false,
                            true,
                            None,
                        )?;
                    }
                    let Some(path) = path else { continue };
                    let cost = distance / self.pitch as f64;
                    if seen.get(&node).is_none_or(|s| s.2 > cost) {
                        seen.insert(node, (node, path, cost));
                    }
                    found += 1;
                }
                if found < 2 && outward.is_some() {
                    self.attach_extended(*position, outward.expect("outward"), layer, spec, &owner, &mut seen)?;
                }
                if self.options.local_escape_radius_nm != 0 {
                    let on_layer: Vec<i64> =
                        seen.keys().copied().filter(|n| self.lattice.layer_of(*n) == layer as i64).collect();
                    if !self.reaches_open(&on_layer, spec, None) {
                        self.attach_local(*position, layer, spec, &mut seen, *outward)?;
                    }
                }
            }
        }
        Ok(seen.into_values().collect())
    }

    fn open_space(&mut self, spec: &NetSpec) -> &[bool] {
        let width = if spec.neck_width != 0 { spec.neck_width } else { spec.width };
        let clearance = if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance };
        let key = (width, clearance);
        if !self.open_cache.contains_key(&key) {
            let g = &self.lattice;
            let (nl, ny, nx) = (g.nl as usize, g.ny as usize, g.nx as usize);
            let plane = g.plane as usize;
            let minimum = 16.max(self.options.open_space_area_nm2.div_euclid(g.pitch * g.pitch));
            let owner = self.rasters.track(width, clearance).0;
            let mut result = vec![false; nl * plane];
            for layer in 0..nl {
                let mask: Vec<bool> = owner[layer * plane..(layer + 1) * plane].iter().map(|&o| o == FREE).collect();
                let (labels, count) = label(&mask, ny, nx);
                if count == 0 {
                    continue;
                }
                let mut sizes = vec![0i64; count + 1];
                for &l in &labels {
                    sizes[l as usize] += 1;
                }
                for (at, &l) in labels.iter().enumerate() {
                    result[layer * plane + at] = l != 0 && sizes[l as usize] >= minimum;
                }
            }
            self.open_cache.insert(key, result);
        }
        &self.open_cache[&key]
    }

    /// `_reaches_open`: a short flood from `nodes` reaches open space.
    fn reaches_open(&mut self, nodes: &[i64], spec: &NetSpec, limit: Option<usize>) -> bool {
        if nodes.is_empty() {
            return false;
        }
        let limit = limit.unwrap_or(self.options.open_flood_limit as usize);
        let open = self.open_space(spec).to_vec();
        let width = if spec.neck_width != 0 { spec.neck_width } else { spec.width };
        let clearance = if spec.neck_clearance != 0 { spec.neck_clearance } else { spec.clearance };
        let owner = self.rasters.track(width, clearance).0;
        let mut stack: Vec<i64> = nodes.to_vec();
        let mut visited: IndexSet<i64> = nodes.iter().copied().collect();
        while !stack.is_empty() && visited.len() < limit {
            let node = stack.pop().expect("non-empty");
            if open[node as usize] {
                return true;
            }
            for (other, _) in self.lattice.neighbours(node, false) {
                let o = owner[other as usize];
                if !visited.contains(&other) && (o == FREE || o == spec.nid) {
                    visited.insert(other);
                    stack.push(other);
                }
            }
        }
        visited.len() >= limit
    }

    /// `_attach_local`: an exact-geometry escape for a pin in a pocket.
    fn attach_local(
        &mut self,
        position: Point,
        layer: usize,
        spec: &NetSpec,
        seen: &mut IndexMap<i64, Attach>,
        outward: Option<(i64, i64)>,
    ) -> Result<()> {
        if self.local_spent >= self.options.local_escape_limit {
            return Ok(());
        }
        self.local_spent += 1;
        let open = self.open_space(spec).to_vec();
        let width = if spec.neck_width != 0 { spec.neck_width } else { spec.width };
        let reach = self.options.local_escape_radius_nm;
        let owner = self.rasters.track(spec.width, spec.clearance).0.to_vec();
        let mut near: Vec<(f64, f64, i64, i64, i64)> = self
            .lattice
            .cover(
                (position.x_nm - reach) as f64,
                (position.y_nm - reach) as f64,
                (position.x_nm + reach) as f64,
                (position.y_nm + reach) as f64,
            )
            .into_iter()
            .map(|(column, x, y)| {
                let (dx, dy) = (x - position.x_nm, y - position.y_nm);
                let distance = hypot(dx as f64, dy as f64);
                let behind = outward.is_some_and(|(ox, oy)| dx * ox + dy * oy < 0);
                (distance * if behind { 2.0 } else { 1.0 }, distance, column, x, y)
            })
            .collect();
        near.sort_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).expect("finite"));
        let (mut found, mut tries, mut misses) = (0, 0, 0);
        let copper = self.lattice.layers[layer];
        for (_, distance, column, x, y) in near {
            if distance > reach as f64 || found >= 3 || tries >= 12 || misses >= 4 {
                break;
            }
            let node = self.lattice.layer_node(column, layer as i64);
            if !open[node as usize] || owner[node as usize] != FREE {
                continue;
            }
            tries += 1;
            let bounds = LocalAccessBounds {
                step_nm: self.options.local_escape_step_nm,
                detour_nm: self.options.local_escape_detour_nm,
                maximum_states: self.options.local_escape_states,
            };
            let path = local_access_path(
                self.board,
                self.clearance.as_ref().expect("index"),
                &spec.name,
                position,
                Point::new(x, y),
                copper,
                width,
                bounds,
                None,
                self.segment_cache.as_mut(),
            )?;
            let Some(path) = path.filter(|p| !p.is_empty()) else {
                misses += 1;
                continue;
            };
            misses = 0;
            let cost = 2.0 * distance / self.pitch as f64;
            if seen.get(&node).is_none_or(|s| s.2 > cost) {
                seen.insert(node, (node, path, cost));
            }
            found += 1;
        }
        Ok(())
    }

    /// `_attach_extended`: straight out along the pad's outward axis, then a checked lead-in.
    fn attach_extended(
        &mut self,
        position: Point,
        outward: (i64, i64),
        layer: usize,
        spec: &NetSpec,
        owner: &[i32],
        seen: &mut IndexMap<i64, Attach>,
    ) -> Result<()> {
        let copper = self.lattice.layers[layer];
        let mut widths = vec![spec.width];
        if spec.neck_width != 0 {
            widths.push(spec.neck_width);
        }
        let reach = self.options.attach_extension_reach_nm;
        for &extension in &self.options.attach_extension_steps_nm.clone() {
            let corner = Point::new(position.x_nm + outward.0 * extension, position.y_nm + outward.1 * extension);
            for &width in &widths {
                if !self.clearance.as_ref().expect("index").can_track(&spec.name, position, corner, width, copper)? {
                    continue;
                }
                let stem = TrackSegment {
                    net: spec.name.clone(),
                    start: position,
                    end: corner,
                    width_nm: width,
                    layer: copper,
                };
                let mut found = 0;
                for (distance, column, x, y) in nearest_first(&self.lattice, corner, reach as f64) {
                    if found >= 3 {
                        break;
                    }
                    let node = self.lattice.layer_node(column, layer as i64);
                    let o = owner[node as usize];
                    if o != FREE && o != spec.nid {
                        continue;
                    }
                    let path = checked_access_path(
                        self.board,
                        self.clearance.as_ref().expect("index"),
                        &spec.name,
                        corner,
                        Point::new(x, y),
                        width,
                        copper,
                        false,
                        true,
                        None,
                    )?;
                    let Some(path) = path else { continue };
                    let cost = (extension as f64 + distance) / self.pitch as f64;
                    if seen.get(&node).is_none_or(|s| s.2 > cost) {
                        let mut lead = vec![stem.clone()];
                        lead.extend(path);
                        seen.insert(node, (node, lead, cost));
                    }
                    found += 1;
                }
                if found != 0 {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

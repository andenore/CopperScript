//! Compatible orchestration of placement feedback, routing, and signoff
//! (`pcbir.flow`).
//!
//! The stage order is Python's: placement and global routing, escape frames,
//! critical routing, held copper, decouplers, power islands, ball-grid
//! fanout and plane stitching, ordinary (unified) routing with land closure,
//! placement feedback and signoff. A stage whose port has not landed reports
//! [`Error::NotImplemented`] when the run reaches it, never a wrong answer.

use indexmap::{IndexMap, IndexSet};

use copper_drc::Policy;
use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::Value;
use copper_pcb::board::{PhysicalBoard, RouteKind};
use copper_pcb::board::{TrackSegment, Via};
use copper_physical::CopperLayer;
use copper_physical::Point;
use copper_physical::pyfloat::{hypot, round_int};
use copper_place::PlannerOptions;

use crate::bga_fanout::plan_bga_fanouts;
use crate::congestion_feedback::{FeedbackOutcome, FeedbackState};
use crate::critical::{
    CriticalNetResult, CriticalRoutingResult, CriticalRoutingStatus, fingerprint as critical_fingerprint,
};
use crate::detailed::{DetailedNetResult, DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus};
use crate::drc_report::{DrcReport, run_physical_drc};
use crate::escape_plan::{ArrayEscape, EscapePlanOptions, plan_array_escapes, plan_two_end_escapes};
use crate::fanout::FanoutOptions;
use crate::pad_stitch::DuplicatePadStitchResult;
use crate::plane::{PlaneStitchOptions, PlaneStitchResult};
use crate::progress::{ProgressCallback, emit, int, text};
use crate::route_closure::close_detailed_lands;
use crate::routeflow::{PlacementRoutingFeedbackOptions, PlacementRoutingResult, optimize_placement_for_routing};
use crate::routing::{GlobalRouterOptions, GlobalRoutingResult};
use crate::unified_router::UnifiedRouterOptions;
use crate::{Error, Result};

/// `PhysicalFlowStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalFlowStatus {
    Pass,
    Fail,
}

impl PhysicalFlowStatus {
    pub fn value(self) -> &'static str {
        match self {
            PhysicalFlowStatus::Pass => "pass",
            PhysicalFlowStatus::Fail => "fail",
        }
    }
}

/// Stage boundaries where an iteration may stop early (`stop_after`).
/// Stopping never passes a gate: the result is a FAIL with `stopped_after` set.
pub const STOP_STAGES: [&str; 1] = ["placement"];

/// The keyword arguments of `run_routing_pipeline`.
#[derive(Clone, Debug)]
pub struct RoutingPipelineOptions {
    pub placement_options: Option<PlannerOptions>,
    pub global_options: Option<GlobalRouterOptions>,
    pub feedback_options: Option<PlacementRoutingFeedbackOptions>,
    pub drc_policy: Policy,
    pub fanout_options: Option<FanoutOptions>,
    pub plane_stitch_options: Option<PlaneStitchOptions>,
    pub detailed_feedback_trials: i64,
    pub detailed_feedback_movement_nm: i64,
    pub critical_feedback_trials: i64,
    pub critical_pair_state_limit: i64,
    pub unified_options: Option<UnifiedRouterOptions>,
    pub stop_after: Option<String>,
    pub escape_plan_options: Option<EscapePlanOptions>,
    pub placement_feedback_trials: i64,
    /// A saved placement that replaces placement and global routing.
    pub initial_placement: Option<PlacementRoutingResult>,
}

/// A completed stage, as `on_stage(name, value)` observes it (for checkpoints).
pub enum Stage<'a> {
    /// `placement`: the placement and global route the run continues from.
    Placement(&'a PlacementRoutingResult),
    /// `critical-inputs` or `critical-retry-inputs`: what the critical
    /// router of the kept run starts from.
    CriticalInputs {
        name: &'static str,
        board: &'a PhysicalBoard,
        global_route: &'a GlobalRoutingResult,
        reserved_accesses: Option<&'a crate::bypass_reservations::BypassReservation>,
        plane_stitch_options: Option<&'a PlaneStitchOptions>,
        fixed_prefix: Option<&'a crate::fixed_prefix::AdmittedFixedPrefix>,
    },
}

pub type StageCallback<'a> = &'a mut dyn FnMut(Stage<'_>);

impl Default for RoutingPipelineOptions {
    fn default() -> Self {
        RoutingPipelineOptions {
            placement_options: None,
            global_options: None,
            feedback_options: None,
            drc_policy: Policy::default(),
            fanout_options: None,
            plane_stitch_options: None,
            detailed_feedback_trials: 0,
            detailed_feedback_movement_nm: 500_000,
            critical_feedback_trials: 0,
            critical_pair_state_limit: 6_000,
            unified_options: None,
            stop_after: None,
            escape_plan_options: None,
            placement_feedback_trials: 0,
            initial_placement: None,
        }
    }
}

/// `RoutingPipelineResult`.
#[derive(Clone, Debug)]
pub struct RoutingPipelineResult {
    pub status: PhysicalFlowStatus,
    pub placement_and_global: PlacementRoutingResult,
    pub critical: CriticalRoutingResult,
    pub detailed: DetailedRoutingResult,
    pub drc: DrcReport,
    pub plane_stitch: Option<PlaneStitchResult>,
    pub duplicate_pad_stitch: Option<DuplicatePadStitchResult>,
    pub stopped_after: Option<String>,
    /// The router's options, for later transactional dependency repairs.
    pub unified_options: UnifiedRouterOptions,
    /// The congestion-driven placement feedback loop (None when not enabled).
    pub placement_feedback: Option<FeedbackOutcome>,
    /// Freshly admitted local copper, retained through final consumer cleanup.
    pub retained_prefix: Option<crate::fixed_prefix::AdmittedFixedPrefix>,
    /// Applicability of the opt-in mandatory-inner-ball retention policy.
    pub retained_escape_policy: Option<RetainedEscapePolicy>,
}

/// Policy selection is not copper admission or a complete escape certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedEscapePolicy {
    requested: bool,
    applicability: &'static str,
    reason: &'static str,
    recognized_packages: usize,
    mandatory_inner_obligations: usize,
    censused_obligations: usize,
    unsupported_pattern_candidates: usize,
    unsupported_pattern_components: Vec<String>,
    allocated_inner_sites: usize,
    existing_inner_launches: usize,
    pending_inner: Vec<(copper_pcb::board::PadReference, String)>,
}

impl RetainedEscapePolicy {
    fn active(&self) -> bool {
        self.requested && self.applicability == "applicable"
    }

    pub fn requested_policy_satisfied(&self, prefix_admitted: bool) -> bool {
        !self.requested || self.applicability == "not_applicable" || self.local_coverage_complete(prefix_admitted)
    }

    fn local_coverage_complete(&self, prefix_admitted: bool) -> bool {
        self.active()
            && prefix_admitted
            && self.mandatory_inner_obligations > 0
            && self.pending_inner.is_empty()
            && self.unsupported_pattern_candidates == 0
    }

    pub fn report(&self, prefix_admitted: bool, prefix_preserved: bool) -> Value {
        let mut fields = self.fields();
        fields.push(("retained_prefix_admitted", Value::Bool(prefix_admitted)));
        fields.push(("retained_prefix_occurrences_preserved", Value::Bool(prefix_preserved)));
        fields.push((
            "mandatory_inner_local_coverage_complete",
            Value::Bool(self.local_coverage_complete(prefix_admitted && prefix_preserved)),
        ));
        fields.push((
            "requested_policy_satisfied",
            Value::Bool(self.requested_policy_satisfied(prefix_admitted && prefix_preserved)),
        ));
        record(fields)
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("policy", text("mandatory_inner_ball_grid")),
            ("requested", Value::Bool(self.requested)),
            ("effective", Value::Bool(self.active())),
            (
                "effective_policy",
                text(if self.active() { "mandatory_inner_ball_grid" } else { "legacy_mutable_fanout" }),
            ),
            ("applicability", text(self.applicability)),
            ("reason", text(self.reason)),
            ("recognized_packages", int(self.recognized_packages as i64)),
            ("mandatory_inner_obligations", int(self.mandatory_inner_obligations as i64)),
            ("censused_obligations", int(self.censused_obligations as i64)),
            ("unsupported_pattern_candidates", int(self.unsupported_pattern_candidates as i64)),
            (
                "unsupported_pattern_components",
                Value::Array(self.unsupported_pattern_components.iter().take(16).map(|name| text(name)).collect()),
            ),
            (
                "unsupported_pattern_components_omitted_count",
                int(self.unsupported_pattern_components.len().saturating_sub(16) as i64),
            ),
            ("allocated_inner_sites", int(self.allocated_inner_sites as i64)),
            ("existing_inner_launches", int(self.existing_inner_launches as i64)),
            ("pending_inner_obligations", int(self.pending_inner.len() as i64)),
            (
                "pending_inner",
                Value::Array(
                    self.pending_inner
                        .iter()
                        .map(|(pad, reason)| record(vec![("pad", text(&pad_name(pad))), ("reason", text(reason))]))
                        .collect(),
                ),
            ),
            ("coverage_scope", text(if self.requested { "recognized_ball_grids_only" } else { "not_assessed" })),
            ("qualification", text("local_site_census_and_occurrence_retention_only")),
            ("boundary_channel_verification", text("not_performed")),
            ("field_protection", text("not_implemented")),
        ]
    }
}

impl RoutingPipelineResult {
    pub fn board(&self) -> &PhysicalBoard {
        &self.detailed.board
    }

    pub fn requested_escape_policy_satisfied(&self, current: &PhysicalBoard) -> bool {
        let preserved =
            self.retained_prefix.as_ref().is_some_and(|prefix| prefix.missing_copper_occurrences(current) == (0, 0));
        self.retained_escape_policy.as_ref().is_none_or(|policy| policy.requested_policy_satisfied(preserved))
    }
}

/// `CriticalRoutingResult(status, board, nets, locked_tracks, locked_vias, global, routing)`.
fn critical_result(
    status: CriticalRoutingStatus,
    board: PhysicalBoard,
    nets: Vec<CriticalNetResult>,
    global_fingerprint: &str,
    routing_fingerprint: String,
) -> CriticalRoutingResult {
    CriticalRoutingResult {
        status,
        board,
        nets,
        locked_tracks: Vec::new(),
        locked_vias: Vec::new(),
        global_routing_fingerprint: global_fingerprint.to_owned(),
        routing_fingerprint,
        reserved_track_count: 0,
        reserved_via_count: 0,
        bundles: Vec::new(),
        match_tuning: Vec::new(),
        plane_reservation_track_count: 0,
        plane_reservation_via_count: 0,
    }
}

/// `_stopped_after_placement`: zero-pass critical/detailed results for a run
/// stopped after placement.
fn stopped_after_placement(
    placement: &PlacementRoutingResult,
    drc_policy: Policy,
) -> Result<(CriticalRoutingResult, DetailedRoutingResult, DrcReport)> {
    // A placement preview still owns its prescribed immutable copper and via
    // arrays: commit and validate the complete prefix as routing would.
    let mut board = copper_drc::macros::materialize_hard_macros(placement.board.clone())?;
    for (key, value) in
        [("stopped_after", "placement"), ("detailed_routing", "not_run"), ("fabrication_ready", "false")]
    {
        board.metadata.insert(key.to_owned(), value.to_owned());
    }
    let global_fp = &placement.global_route.routing_fingerprint;
    let critical = critical_result(
        CriticalRoutingStatus::Warning,
        board.clone(),
        Vec::new(),
        global_fp,
        critical_fingerprint(global_fp, &[], &[], &[]),
    );
    let reason = "not routed: run stopped after placement (--stop-after placement)";
    let nets: Vec<DetailedNetResult> = board
        .nets
        .iter()
        .filter(|net| net.pads.len() >= 2)
        .map(|net| DetailedNetResult {
            net: net.name.clone(),
            connected: false,
            track_count: 0,
            via_count: 0,
            length_nm: 0,
            guide_deviation_count: 0,
            diagnostics: vec![reason.to_owned()],
            orthogonal_mode_used: false,
            resumed_branch_count: 0,
            search_budget_exhausted: false,
            retry_heuristic_weight_percent: None,
            warnings: Vec::new(),
        })
        .collect();
    let metrics = DetailedRoutingMetrics {
        routed_net_count: 0,
        unrouted_net_count: nets.len() as i64,
        conflict_resource_count: 0,
        total_conflict_overflow: 0,
        track_count: 0,
        via_count: 0,
        total_length_nm: 0,
        passes: 0,
    };
    let routing_fingerprint = crate::detailed::fingerprint(global_fp, &board, &metrics);
    let detailed = DetailedRoutingResult {
        status: DetailedRoutingStatus::Partial,
        locked_track_count: board.tracks.len() as i64,
        locked_via_count: board.vias.len() as i64,
        board: board.clone(),
        nets,
        metrics,
        global_routing_fingerprint: global_fp.clone(),
        routing_fingerprint,
        congestion: None,
    };
    let drc = run_physical_drc(&board, drc_policy)?;
    Ok((critical, detailed, drc))
}

/// `_direct_critical`: only source-fixed (macro) copper precedes negotiation.
fn direct_critical(placement: &PlacementRoutingResult) -> Result<CriticalRoutingResult> {
    let source = copper_drc::macros::materialize_hard_macros(placement.board.clone())?;
    let fp = &placement.global_route.routing_fingerprint;
    Ok(critical_result(CriticalRoutingStatus::Warning, source, Vec::new(), fp, fp.clone()))
}

/// Preserve the actual per-ball site assignments and pending sites alongside
/// their copper. An allocated site is not a verified package-boundary escape.
struct Lattice {
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    keepouts: Vec<copper_physical::CopperKeepout>,
    packages: Vec<crate::via_lattice::PackageLattice>,
}

/// Source-level diagnostic candidates, never authority to allocate copper.
/// Aggregate square lands as well as equal-size groups so a detector's smaller
/// dominant-size subgrid cannot certify that the source has no inner contacts.
fn potential_inner_contacts(
    board: &PhysicalBoard,
    component: &str,
    connected: &IndexSet<copper_pcb::board::PadReference>,
) -> IndexSet<copper_pcb::board::PadReference> {
    use copper_physical::footprint::PadKind;
    let pose = board.placements.iter().find(|pose| pose.reference == component).expect("placed component");
    let lands: Vec<_> = board.footprints[&pose.footprint].pads.iter().filter(|pad| pad.kind == PadKind::Smd).collect();
    let mut groups: IndexMap<_, Vec<_>> = IndexMap::new();
    for pad in &lands {
        groups.entry((pad.size.width_nm, pad.size.height_nm)).or_default().push(*pad);
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.push(lands.iter().copied().filter(|pad| pad.size.width_nm == pad.size.height_nm).collect());
    let mut candidates = IndexSet::new();
    for lands in groups {
        if lands.len() < 9 {
            continue;
        }
        let mut xs: Vec<_> = lands.iter().map(|pad| pad.position.x_nm).collect::<IndexSet<_>>().into_iter().collect();
        let mut ys: Vec<_> = lands.iter().map(|pad| pad.position.y_nm).collect::<IndexSet<_>>().into_iter().collect();
        xs.sort_unstable();
        ys.sort_unstable();
        if xs.len() < 5 || ys.len() < 5 {
            continue;
        }
        for pad in lands {
            let reference = copper_pcb::board::PadReference::new(component, &pad.number);
            let x = xs.binary_search(&pad.position.x_nm).unwrap();
            let y = ys.binary_search(&pad.position.y_nm).unwrap();
            if x.min(xs.len() - 1 - x).min(y).min(ys.len() - 1 - y) >= 2 && connected.contains(&reference) {
                candidates.insert(reference);
            }
        }
    }
    candidates
}

/// Select the package-specific policy before creating any retained frame.
/// Allocation success is deliberately irrelevant: Pending and existing source
/// launches are still mandatory obligations, not reasons to fall back silently.
fn retained_escape_policy(
    board: &PhysicalBoard,
    requested: bool,
    lattice: Option<&Lattice>,
) -> Result<RetainedEscapePolicy> {
    use copper_physical::footprint::PadKind;
    let mut policy = RetainedEscapePolicy {
        requested,
        applicability: "disabled",
        reason: "not_requested",
        recognized_packages: 0,
        mandatory_inner_obligations: 0,
        censused_obligations: 0,
        unsupported_pattern_candidates: 0,
        unsupported_pattern_components: Vec::new(),
        allocated_inner_sites: 0,
        existing_inner_launches: 0,
        pending_inner: Vec::new(),
    };
    if !requested {
        return Ok(policy);
    }
    let connected: IndexSet<_> = board.nets.iter().flat_map(|net| net.pads.iter().cloned()).collect();
    let mut expected = IndexSet::new();
    let mut unsupported_inner = IndexMap::new();
    for pose in &board.placements {
        let potential = potential_inner_contacts(board, &pose.reference, &connected);
        let array = match crate::package_patterns::ball_grid(board, &pose.reference) {
            Ok(Some(array)) => array,
            Ok(None) | Err(crate::unified_error::Error::Value(_)) => {
                if !potential.is_empty() {
                    policy.unsupported_pattern_candidates += 1;
                    policy.unsupported_pattern_components.push(pose.reference.clone());
                }
                continue;
            }
            Err(error) => return copper_pcb::not_implemented(&error.crash_line()),
        };
        policy.recognized_packages += 1;
        let mut package_has_unsupported_inner = false;
        for land in board.footprints[&pose.footprint].pads.iter().filter(|pad| pad.kind == PadKind::Smd) {
            let reference = copper_pcb::board::PadReference::new(&pose.reference, &land.number);
            if !connected.contains(&reference) {
                continue;
            }
            let inner = array
                .index(land.position)
                .is_some_and(|(x, y)| x.min(y).min(array.xs_nm.len() - 1 - x).min(array.ys_nm.len() - 1 - y) >= 2)
                || (array.xs_nm.len() >= 5
                    && array.ys_nm.len() >= 5
                    && land.position.x_nm >= array.xs_nm[2]
                    && land.position.x_nm <= array.xs_nm[array.xs_nm.len() - 3]
                    && land.position.y_nm >= array.ys_nm[2]
                    && land.position.y_nm <= array.ys_nm[array.ys_nm.len() - 3]);
            if inner {
                if !array.balls.contains(&land.number) {
                    package_has_unsupported_inner = true;
                    unsupported_inner.insert(reference.clone(), "unsupported_inner_land_geometry".to_owned());
                }
                expected.insert(reference);
            }
        }
        for reference in potential {
            if !expected.contains(&reference) {
                expected.insert(reference.clone());
                unsupported_inner.insert(reference, "uncensused_inner_surface_field".to_owned());
                package_has_unsupported_inner = true;
            }
        }
        if package_has_unsupported_inner {
            policy.unsupported_pattern_candidates += 1;
            policy.unsupported_pattern_components.push(pose.reference.clone());
        }
    }
    policy.mandatory_inner_obligations = expected.len();
    let covered: IndexSet<_> = lattice
        .into_iter()
        .flat_map(|plan| &plan.packages)
        .flat_map(|package| &package.coverage)
        .filter(|ball| ball.mandatory_inner && expected.contains(&ball.pad))
        .map(|ball| ball.pad.clone())
        .collect();
    policy.censused_obligations = covered.len();
    for pad in &expected {
        let coverage = lattice
            .into_iter()
            .flat_map(|plan| &plan.packages)
            .flat_map(|package| &package.coverage)
            .find(|ball| ball.pad == *pad && ball.mandatory_inner);
        use crate::via_lattice::BallSiteStatus;
        match coverage.map(|ball| &ball.status) {
            Some(BallSiteStatus::Allocated) => policy.allocated_inner_sites += 1,
            Some(BallSiteStatus::ExistingSourceLaunch { .. }) => policy.existing_inner_launches += 1,
            Some(BallSiteStatus::Pending(reason)) => policy.pending_inner.push((pad.clone(), reason.clone())),
            Some(BallSiteStatus::SurfacePolicy) => {
                policy.pending_inner.push((pad.clone(), "mandatory_inner_surface_policy_unverified".to_owned()))
            }
            None => policy.pending_inner.push((
                pad.clone(),
                unsupported_inner
                    .get(pad)
                    .cloned()
                    .unwrap_or_else(|| "mandatory_inner_lattice_census_missing".to_owned()),
            )),
        }
    }
    (policy.applicability, policy.reason) = if expected.is_empty() {
        if policy.unsupported_pattern_candidates > 0 {
            ("unsupported_or_uncensused", "unrecognized_surface_land_field")
        } else {
            ("not_applicable", "no_recognized_connected_inner_ball_obligations")
        }
    } else if covered.is_empty() {
        ("unsupported_or_uncensused", "mandatory_inner_lattice_census_missing")
    } else if covered.len() != expected.len() {
        ("applicable", "mandatory_inner_lattice_census_partial")
    } else {
        ("applicable", "mandatory_inner_obligations_censused")
    };
    Ok(policy)
}

/// `_plan_lattice`: ball grids' via sites (escape plan E7), or None. Planned
/// before escapes, bypasses and critical routes can take the sites: escapes
/// and bypasses plan with the lattice copper in their frame, critical routing
/// sees it as keepouts, and it joins the board after critical routing.
fn plan_lattice(
    board: &PhysicalBoard,
    escape_plan_options: Option<&EscapePlanOptions>,
    surface_balls: Option<&IndexSet<copper_pcb::board::PadReference>>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<Option<Lattice>> {
    let Some(options) = escape_plan_options.filter(|options| options.via_lattice) else { return Ok(None) };
    let frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    let planned = crate::via_lattice::plan_via_lattices(&frame, surface_balls, 2, options.lattice_band_rings)?;
    emit(on_progress, "via_lattice", "finished", planned.report());
    if planned.vias.is_empty() && planned.lattices.is_empty() {
        return Ok(None);
    }
    let keepouts = copper_keepouts(board, &planned.tracks, &planned.vias, "lattice-reserve", Some(&frame));
    Ok(Some(Lattice { tracks: planned.tracks, vias: planned.vias, keepouts, packages: planned.lattices }))
}

/// `_plan_array_escapes`: which ball-grid balls escape on the surface.
fn plan_array_frames(
    board: &PhysicalBoard,
    escape_plan_options: Option<&EscapePlanOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(Option<IndexMap<String, ArrayEscape>>, Option<IndexSet<copper_pcb::board::PadReference>>)> {
    let Some(options) = escape_plan_options.filter(|options| options.array_flow) else { return Ok((None, None)) };
    let plans: IndexMap<String, ArrayEscape> =
        plan_array_escapes(board, options)?.into_iter().map(|item| (item.component.clone(), item)).collect();
    emit(
        on_progress,
        "array_escape_plan",
        "finished",
        vec![
            ("arrays", int(plans.len() as i64)),
            ("surface", int(plans.values().map(|item| item.surface.len() as i64).sum())),
            ("vias", int(plans.values().map(|item| item.vias.len() as i64).sum())),
        ],
    );
    let surface = plans.values().flat_map(|item| item.surface.iter().cloned()).collect();
    Ok((Some(plans), Some(surface)))
}

/// `_macro_port_halos`: keepouts beyond macro ports whose nets may change
/// layer, for the escape and bypass planners only.
pub(crate) fn macro_port_halos(board: &PhysicalBoard) -> Result<Vec<copper_physical::CopperKeepout>> {
    let frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    let poses: IndexMap<&str, _> = board.placements.iter().map(|pose| (pose.reference.as_str(), pose)).collect();
    let placements: IndexMap<String, copper_pcb::board::Placement> =
        frame.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let gap = board.rules.minimum_clearance_nm;
    let mut halos = Vec::new();
    for hard in &frame.hard_macros {
        if !frame.materialized_macros.contains(&hard.cluster) {
            continue;
        }
        for port in copper_pcb::hard_macros::resolved_macro_geometry(&frame, hard, &placements)?.ports {
            let Some(rule) = rules.get(port.net.as_str()) else { continue };
            if rule.max_vias == Some(0) || port.pads.is_empty() {
                continue;
            }
            let owner = port.pads.iter().min().expect("pads");
            let centre = poses[owner.component.as_str()].position;
            let (dx, dy) = (port.position.x_nm - centre.x_nm, port.position.y_nm - centre.y_nm);
            let (ox, oy) =
                if dx.abs() >= dy.abs() { (if dx > 0 { 1 } else { -1 }, 0) } else { (0, if dy > 0 { 1 } else { -1 }) };
            let half = board.rules.default_via_size_nm
                + gap
                + rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm)
                + rule.pair_gap_nm.unwrap_or(0);
            let (x0, y0) = (port.position.x_nm + ox * gap, port.position.y_nm + oy * gap);
            let sorted = |a: i64, b: i64| if a <= b { (a, b) } else { (b, a) };
            let (xs, ys) = if ox != 0 {
                (sorted(x0, x0 + ox * 2 * half), (y0 - half, y0 + half))
            } else {
                ((x0 - half, x0 + half), sorted(y0, y0 + oy * 2 * half))
            };
            let corners = vec![
                copper_physical::Point::new(xs.0, ys.0),
                copper_physical::Point::new(xs.1, ys.0),
                copper_physical::Point::new(xs.1, ys.1),
                copper_physical::Point::new(xs.0, ys.1),
            ];
            halos.push(copper_physical::CopperKeepout {
                id: format!("port-halo-{}", halos.len()),
                layers: board.stackup.copper_layers.clone(),
                vertices: corners,
                block_tracks: true,
                block_vias: true,
                block_pads: false,
                block_zones: false,
                block_footprints: false,
            });
        }
    }
    Ok(halos)
}

/// `_plane_pin_halos`: keepouts in front of fine-pitch plane pins, for
/// ordinary escape planning. A pin of a distribution zone net without a zone
/// on its own layer reaches its plane through a stitching via placed after
/// routing; the halo keeps one via site free from one clearance past the
/// land's outer end, along the land's long axis away from the package
/// centre, where the neighbouring pins' straight escapes still pass beside it.
pub(crate) fn plane_pin_halos(board: &PhysicalBoard) -> Vec<copper_physical::CopperKeepout> {
    use copper_physical::footprint::PadKind;
    use copper_physical::{BoardSide, CopperLayer, Point};
    let zone_nets = copper_pcb::zones::distribution_zone_nets(board);
    if zone_nets.is_empty() {
        return Vec::new();
    }
    let mut zone_layers: IndexMap<&str, IndexSet<CopperLayer>> = IndexMap::new();
    for zone in &board.zones {
        zone_layers.entry(zone.net.as_str()).or_default().extend(zone.layers.iter().copied());
    }
    let gap = board.rules.minimum_clearance_nm;
    let via = board.rules.default_via_size_nm;
    let half = via.div_euclid(2) + gap;
    let track = board.rules.default_track_width_nm;
    let mut nets: IndexMap<(&str, &str), &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            nets.insert((pad.component.as_str(), pad.pad.as_str()), net.name.as_str());
        }
    }
    let mut halos = Vec::new();
    for placement in &board.placements {
        let footprint = &board.footprints[&placement.footprint];
        let side = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        let mut centres: IndexMap<&str, Point> = IndexMap::new();
        for pad in &footprint.pads {
            centres
                .insert(pad.number.as_str(), copper_pcb::transform::transformed_local_point(placement, pad.position));
        }
        for pad in &footprint.pads {
            let Some(net) = nets.get(&(placement.reference.as_str(), pad.number.as_str())) else { continue };
            if !zone_nets.contains(*net)
                || zone_layers.get(net).is_some_and(|layers| layers.contains(&side))
                || pad.kind != PadKind::Smd
            {
                continue;
            }
            let centre = centres[pad.number.as_str()];
            let bounds = copper_pcb::transform::placed_pad_shape(centre, pad, placement).bounds;
            let (width, height) = (bounds.max_x - bounds.min_x, bounds.max_y - bounds.min_y);
            if 2 * width.max(height) < 3 * width.min(height) {
                continue;
            }
            let (dx, dy) = (centre.x_nm - placement.position.x_nm, centre.y_nm - placement.position.y_nm);
            let (ox, oy) =
                if width > height { (if dx > 0 { 1 } else { -1 }, 0) } else { (0, if dy > 0 { 1 } else { -1 }) };
            if (ox != 0 && dx == 0) || (oy != 0 && dy == 0) {
                continue;
            }
            // Nearest neighbouring land across the outward axis.
            let across = centres
                .iter()
                .filter(|(other, point)| {
                    **other != pad.number.as_str()
                        && (point.x_nm - centre.x_nm).abs() + (point.y_nm - centre.y_nm).abs() <= 650_000
                })
                .map(
                    |(_, point)| {
                        if ox != 0 { (point.y_nm - centre.y_nm).abs() } else { (point.x_nm - centre.x_nm).abs() }
                    },
                )
                .min();
            let Some(across) = across else { continue };
            if half + gap + (track + 1).div_euclid(2) > across {
                continue;
            }
            let step = if ox != 0 { ox } else { oy };
            let edge = if ox != 0 {
                if ox > 0 { bounds.max_x } else { bounds.min_x }
            } else if oy > 0 {
                bounds.max_y
            } else {
                bounds.min_y
            };
            let (near, far) = (edge + step * gap, edge + step * (2 * gap + via));
            let sorted = (near.min(far), near.max(far));
            let (xs, ys) = if ox != 0 {
                (sorted, (centre.y_nm - half, centre.y_nm + half))
            } else {
                ((centre.x_nm - half, centre.x_nm + half), sorted)
            };
            halos.push(copper_physical::CopperKeepout {
                id: format!("plane-pin-halo-{}", halos.len()),
                layers: vec![side],
                vertices: vec![
                    Point::new(xs.0, ys.0),
                    Point::new(xs.1, ys.0),
                    Point::new(xs.1, ys.1),
                    Point::new(xs.0, ys.1),
                ],
                block_tracks: true,
                block_vias: true,
                block_pads: false,
                block_zones: false,
                block_footprints: false,
            });
        }
    }
    halos
}

/// The installed progress observer, reborrowed for one call.
pub(crate) fn reborrow<'a>(on_progress: &'a mut Option<ProgressCallback<'_>>) -> Option<ProgressCallback<'a>> {
    match on_progress {
        Some(callback) => Some(&mut **callback),
        None => None,
    }
}

/// `critical_return_plane_options`: declared return contacts before a
/// critical search needs them, or None.
pub fn critical_return_plane_options(
    board: &PhysicalBoard,
    options: Option<&PlaneStitchOptions>,
    keepouts: &[copper_physical::CopperKeepout],
) -> Option<PlaneStitchOptions> {
    let options = options?;
    let zone_nets: IndexSet<&str> = board.zones.iter().map(|zone| zone.net.as_str()).collect();
    let returns: IndexSet<&str> = board
        .net_routing_rules
        .iter()
        .filter(|r| r.kind != RouteKind::General && r.require_return_vias)
        .filter_map(|r| r.return_via_net.as_deref())
        .filter(|net| zone_nets.contains(net))
        .collect();
    let mut pads: std::collections::BTreeSet<copper_pcb::board::PadReference> = board
        .nets
        .iter()
        .filter(|net| returns.contains(net.name.as_str()))
        .flat_map(|net| net.pads.iter().cloned())
        .collect();
    if let Some(only) = &options.only_pads {
        pads.retain(|pad| only.contains(pad));
    }
    if pads.is_empty() {
        return None;
    }
    let mut result = options.clone();
    result.only_pads = Some(pads);
    result.keepouts.extend(keepouts.iter().cloned());
    Some(result)
}

/// Admit local launches as real copper before any critical search or plane
/// contact. This does not certify onward package channels.
fn retained_critical_inputs(
    board: &PhysicalBoard,
    reserved: Option<&Reserved>,
    lattice: Option<&Lattice>,
    held: Option<&Held>,
) -> Result<(crate::fixed_prefix::AdmittedFixedPrefix, Option<crate::bypass_reservations::BypassReservation>)> {
    use crate::fixed_prefix::{AdmittedFixedPrefix, FixedContribution, FixedPrefixRole};
    let mut contributions = Vec::new();
    if let Some(plan) = lattice {
        contributions.push(FixedContribution {
            id: "via-lattice".to_owned(),
            role: FixedPrefixRole::ViaLattice,
            tracks: plan.tracks.clone(),
            vias: plan.vias.clone(),
        });
    }
    if let Some(plan) = reserved {
        contributions.push(FixedContribution {
            id: "ordinary-escapes".to_owned(),
            role: FixedPrefixRole::OrdinaryEscape,
            tracks: plan.0.clone(),
            vias: plan.1.clone(),
        });
    }
    // No generated proxy masks enter this source. The same freshly admitted
    // physical frame is used by both bypass binding and critical routing.
    let prefix = AdmittedFixedPrefix::admit(board, &contributions)?;
    let bypass = match held {
        Some(held) if !held.tracks.is_empty() || !held.vias.is_empty() => {
            // Witnesses were refreshed in the previous reservation-mask
            // context. Prove those exact original paths there first, with
            // all retained physical contributions present. A stale original
            // context is not authority to invent a replacement witness.
            let mut current = prefix.board().clone();
            current.tracks.extend(held.tracks.iter().cloned());
            current.vias.extend(held.vias.iter().cloned());
            let provenance = held.provenance.as_ref().ok_or_else(|| {
                Error::Value("retained escape frame lacks original Held producer contexts".to_owned())
            })?;
            if let Some(original) = &provenance.complete_prefix {
                let original = original.revalidate_for(board)?;
                if original.board() != prefix.board() || original.contributions() != prefix.contributions() {
                    return Err(Error::Value("retained Held complete physical prefix changed".to_owned()));
                }
            } else if provenance.prefix_tracks.as_slice() != lattice.map_or(&[][..], |plan| plan.tracks.as_slice())
                || provenance.prefix_vias.as_slice() != lattice.map_or(&[][..], |plan| plan.vias.as_slice())
            {
                return Err(Error::Value("retained Held lattice prefix changed".to_owned()));
            }
            provenance.validate(board, &held.tracks, &held.vias)?;
            // Removing the explicitly known temporary masks changes the
            // source context. Re-prove the exact original paths in both
            // contexts; choosing another path can orphan generated ownership.
            let clearance = crate::routing_clearance::RoutingClearanceIndex::new(&current)?;
            let mut witnesses = Vec::new();
            for batch in &provenance.batches {
                for original in &batch.witnesses {
                    witnesses.push(crate::decoupling_paths::revalidate_after_temporary_keepout_removal(
                        prefix.baseline(),
                        &batch.raw,
                        &current,
                        original,
                        &batch.temporary_keepouts,
                        &clearance,
                    )?);
                }
            }
            let (bound, reason) = crate::bypass_reservations::bypass_copper_reservations_in_frame(
                board,
                &prefix,
                &held.tracks,
                &held.vias,
                &witnesses,
            )?;
            Some(bound.ok_or_else(|| {
                Error::Value(format!(
                    "retained escape frame cannot bind held bypass: {}",
                    reason.as_deref().unwrap_or("no fresh witness")
                ))
            })?)
        }
        _ => None,
    };
    Ok((prefix, bypass))
}

/// Reject a failed Held proposal without downgrading its independently
/// admitted physical prefix. This is admission/rollback, never another search.
fn retained_critical_binding(
    board: &PhysicalBoard,
    reserved: Option<&Reserved>,
    lattice: Option<&Lattice>,
    held: &mut Option<Held>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<Option<(crate::fixed_prefix::AdmittedFixedPrefix, Option<crate::bypass_reservations::BypassReservation>)>> {
    match retained_critical_inputs(board, reserved, lattice, held.as_ref()) {
        Ok(inputs) => Ok(Some(inputs)),
        Err(Error::Value(reason)) => {
            let original = held
                .as_ref()
                .and_then(|held| held.provenance.as_ref())
                .and_then(|provenance| provenance.complete_prefix.as_ref())
                .cloned();
            let prefix = match original {
                Some(original) => match original.revalidate_for(board) {
                    Ok(prefix) => Some(prefix),
                    Err(Error::Value(stale)) => {
                        emit(
                            on_progress,
                            "retained_escape_copper",
                            "not_admitted",
                            vec![("scope", text("complete_local_prefix")), ("reason", text(&stale))],
                        );
                        None
                    }
                    Err(error) => return Err(error),
                },
                None => None,
            };
            emit(
                on_progress,
                "retained_escape_copper",
                "not_admitted",
                vec![
                    ("scope", text(if prefix.is_some() { "held_suffix" } else { "complete_local_prefix" })),
                    ("reason", text(&reason)),
                ],
            );
            if let Some(prefix) = prefix {
                let pending: Vec<_> = board
                    .decoupling_links
                    .iter()
                    .map(|link| (link.clone(), format!("Held proposal rejected: {reason}")))
                    .collect();
                emit(
                    on_progress,
                    "bypass_reservation",
                    "held_rejected_prefix_preserved",
                    vec![
                        ("reason", text(&reason)),
                        ("pending", int(pending.len() as i64)),
                        ("pending_links", pending_links(&pending)),
                    ],
                );
                // No nonempty Held survives for late join to resurrect it.
                *held = None;
                Ok(Some((prefix, None)))
            } else {
                Ok(None)
            }
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn route_critical_stage(
    placement: &PlacementRoutingResult,
    unified_options: &UnifiedRouterOptions,
    unified_fanout: Option<&FanoutOptions>,
    pair_state_limit: Option<i64>,
    plane_stitch_options: Option<&PlaneStitchOptions>,
    lattice: Option<&Lattice>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    on_stage: &mut Option<StageCallback<'_>>,
) -> Result<(
    CriticalRoutingResult,
    Option<Reserved>,
    Option<Held>,
    bool,
    bool,
    Option<crate::fixed_prefix::AdmittedFixedPrefix>,
)> {
    let board = &placement.board;
    let retention_policy = retained_escape_policy(board, unified_options.retain_escape_copper, lattice)?;
    let retain = retention_policy.active();
    if retention_policy.requested {
        emit(on_progress, "retained_escape_policy", "assessed", retention_policy.fields());
    }
    let prefix: (&[TrackSegment], &[Via]) = lattice.map_or((&[], &[]), |plan| (&plan.tracks, &plan.vias));
    if let Some(plan) = lattice {
        emit(
            on_progress,
            "package_escape_sites",
            "assessed",
            vec![
                ("packages", int(plan.packages.len() as i64)),
                ("allocated_sites", int(plan.packages.iter().map(|package| package.sites.len()).sum::<usize>() as i64)),
                ("pending_sites", int(plan.packages.iter().map(|package| package.unsited.len()).sum::<usize>() as i64)),
                ("scope", text("dogbone_sites_only")),
                ("boundary_channel_verification", text("not_performed")),
            ],
        );
    }
    let planar = crate::critical::promote_planar_nets(board).1;
    if !planar.is_empty() {
        let mut sorted = planar.clone();
        sorted.sort();
        emit(
            on_progress,
            "planar_nets",
            "promoted",
            vec![("nets", Value::Array(sorted.iter().map(|n| text(n)).collect()))],
        );
    }
    let halos = macro_port_halos(board)?;
    let mut reserved = match unified_fanout {
        Some(fanout) if unified_options.escapes_first => {
            let mut keepouts = halos.clone();
            keepouts.extend(plane_pin_halos(board));
            plan_ordinary_escapes(board, fanout, on_progress, prefix, &keepouts, None, &planar, "finished")?
        }
        _ => None,
    };
    let mut held: Option<Held> = None;
    if let Some(planned) = &reserved {
        observe_source_reserved_geometry(board, planned, prefix, None, "initial_ordinary_plan", on_progress);
    }
    if !board.decoupling_links.is_empty() && unified_options.pre_route_decouplers {
        // Bypasses route before critical nets (as package access did), so
        // a critical route cannot take a capacitor's short surface feed.
        let (planned, kept) = if retain {
            plan_retained_decouplers(board, reserved.take(), lattice, &halos, unified_fanout, on_progress)?
        } else {
            plan_decouplers(board, reserved.take(), on_progress, prefix, &halos, unified_fanout)?
        };
        reserved = kept;
        let mut planned = planned;
        if let Some(kept) = reserved.take() {
            if planned.released.is_empty() {
                reserved = Some(kept);
            } else {
                let (kept, updated, restored) = revalidate_released_escapes(board, kept, planned)?;
                planned = updated;
                emit(on_progress, "ordinary_escapes", "revalidated", restored.fields());
                reserved = Some(match unified_fanout {
                    Some(fanout) if !restored.unrestored_nets.is_empty() => {
                        let mut keepouts = halos.clone();
                        keepouts.extend(plane_pin_halos(board));
                        let nets: IndexSet<String> = restored.unrestored_nets.iter().cloned().collect();
                        resite_escapes(board, fanout, kept, &planned, prefix, &keepouts, &nets, on_progress)?
                    }
                    _ => kept,
                });
            }
        }
        // The witnesses were admitted in the planning context; the critical
        // router's context has only the final reservations.
        let planned = if retain {
            // The original proof includes the complete physical prefix and
            // producer halos. Final binding adapts that exact recorded context.
            planned
        } else {
            let (planned, reason) = refresh_bypass_witnesses(board, reserved.as_ref(), planned)?;
            if let Some(reason) = reason {
                emit(on_progress, "bypass_reservation", "witnesses_stale", vec![("reason", text(&reason))]);
            }
            planned
        };
        held = Some(planned);
    }
    if let Some(planned) = &reserved {
        observe_source_reserved_geometry(board, planned, prefix, held.as_ref(), "before_first_critical", on_progress);
    }
    let retained = if retain {
        retained_critical_binding(board, reserved.as_ref(), lattice, &mut held, on_progress)?
    } else {
        None
    };
    let critical_inputs = |keepouts: &[copper_physical::CopperKeepout],
                           previous: Option<(&PhysicalBoard, &crate::bypass_reservations::BypassReservation)>,
                           on_progress: &mut Option<ProgressCallback<'_>>|
     -> Result<(PhysicalBoard, Option<crate::bypass_reservations::BypassReservation>)> {
        let mut source = board.clone();
        source.copper_keepouts.extend(keepouts.iter().cloned());
        let (accesses, reason) = match &held {
            Some(held) => crate::bypass_reservations::bypass_copper_reservations(
                &source,
                &held.tracks,
                &held.vias,
                &held.witnesses,
            )?,
            None => (None, None),
        };
        let (accesses, reason) = match (accesses, previous) {
            (None, Some((previous, reserved))) => {
                crate::bypass_reservations::rebind_bypass_keepout_removal(previous, reserved, &source)?
            }
            (accesses, _) => (accesses, reason),
        };
        if let (Some(held), None) = (&held, &accesses) {
            // Unsupported/ambiguous ownership keeps its original conservative
            // reservation. Never drop it or infer semantic authority by net.
            source.copper_keepouts.extend(held.keepouts.iter().cloned());
        }
        emit(
            on_progress,
            "bypass_reservation",
            "bound",
            vec![
                ("mode", text(if accesses.is_some() { "copper" } else { "keepouts" })),
                ("reason", reason.as_deref().map_or(Value::Null, text)),
            ],
        );
        if let Some(plan) = lattice {
            // Lattice vias are reserved for their balls while critical nets
            // route; they join the board afterwards.
            source.copper_keepouts.extend(plan.keepouts.iter().cloned());
        }
        Ok((source, accesses))
    };
    let (source, bypass_accesses, fixed_prefix) = match retained {
        Some((prefix, bypass)) => {
            emit(
                on_progress,
                "retained_escape_copper",
                "admitted",
                vec![
                    ("scope", text("local_dogbones_and_sites")),
                    ("boundary_channel_verification", text("not_performed")),
                ],
            );
            (board.clone(), bypass, Some(prefix))
        }
        None => {
            let (source, bypass) = critical_inputs(reserved.as_ref().map_or(&[][..], |r| &r.2[..]), None, on_progress)?;
            (source, bypass, None)
        }
    };
    let mut held_as_copper = bypass_accesses.is_some();
    let return_options = critical_return_plane_options(&source, plane_stitch_options, &halos);
    if let Some(callback) = on_stage.as_mut() {
        callback(Stage::CriticalInputs {
            name: "critical-inputs",
            board: &source,
            global_route: &placement.global_route,
            reserved_accesses: bypass_accesses.as_ref(),
            plane_stitch_options: return_options.as_ref(),
            fixed_prefix: fixed_prefix.as_ref(),
        });
    }
    let options = crate::critical::CriticalOptions {
        pair_state_limit,
        plane_stitch_options: return_options.as_ref(),
        unified_options: Some(unified_options),
        reserved_accesses: bypass_accesses.as_ref(),
        fixed_prefix: fixed_prefix.as_ref(),
        ..Default::default()
    };
    // A keepout release re-proves held copper against these inputs.
    let retry_base = (reserved.is_some() && bypass_accesses.is_some()).then(|| source.clone());
    let mut critical =
        crate::critical::route_critical_nets(source, &placement.global_route, &options, reborrow(on_progress))?;
    if let Some(planned) = &reserved {
        observe_reserved_geometry(
            board,
            &critical.board,
            planned,
            prefix,
            lattice.map(|plan| plan.keepouts.as_slice()),
            held.as_ref().filter(|_| !held_as_copper),
            "after_first_critical",
            None,
            on_progress,
        );
    }
    if let Some(planned) = reserved.as_mut() {
        let failed: IndexSet<String> =
            critical.nets.iter().filter(|item| !item.connected).flat_map(|item| item.nets.iter().cloned()).collect();
        if !failed.is_empty() && fixed_prefix.is_none() {
            // Critical nets come first: free the reservations around the pads
            // of any critical net they blocked and route again; keep whichever
            // attempt connects more critical nets.
            let kept = release_reservations(board, &planned.2, &failed, crate::nm_from_mm(2), &[]);
            if kept.len() < planned.2.len() {
                let mut nets: Vec<&String> = failed.iter().collect();
                nets.sort();
                emit(
                    on_progress,
                    "ordinary_escapes",
                    "released",
                    vec![
                        ("nets", Value::Array(nets.iter().map(|n| text(n)).collect())),
                        ("keepouts", int((planned.2.len() - kept.len()) as i64)),
                    ],
                );
                let (retry_source, retry_accesses) =
                    critical_inputs(&kept, retry_base.as_ref().zip(bypass_accesses.as_ref()), on_progress)?;
                if let Some(callback) = on_stage.as_mut() {
                    callback(Stage::CriticalInputs {
                        name: "critical-retry-inputs",
                        board: &retry_source,
                        global_route: &placement.global_route,
                        reserved_accesses: retry_accesses.as_ref(),
                        plane_stitch_options: return_options.as_ref(),
                        fixed_prefix: None,
                    });
                }
                let retry_options =
                    crate::critical::CriticalOptions { reserved_accesses: retry_accesses.as_ref(), ..options };
                let retry = crate::critical::route_critical_nets(
                    retry_source,
                    &placement.global_route,
                    &retry_options,
                    reborrow(on_progress),
                )?;
                let checked = adopt_critical_retry(
                    board,
                    &mut critical,
                    planned,
                    &mut held_as_copper,
                    held.as_ref(),
                    prefix,
                    lattice.map(|plan| plan.keepouts.as_slice()),
                    retry,
                    retry_accesses.is_some(),
                    kept,
                )?;
                emit(
                    on_progress,
                    "ordinary_escapes",
                    "critical_retry_checked",
                    vec![
                        ("critical_failure_improved", Value::Bool(checked.improved)),
                        ("adopted", Value::Bool(checked.adopted)),
                        ("source_masks_present", checked.source_masks_present.map_or(Value::Null, Value::Bool)),
                        (
                            "baseline_invalid_escape_nets",
                            Value::Array(checked.baseline_invalid.iter().map(|n| text(n)).collect()),
                        ),
                        (
                            "invalid_candidate_escape_nets",
                            Value::Array(checked.candidate_invalid.iter().map(|n| text(n)).collect()),
                        ),
                        (
                            "baseline_invalid_dependencies",
                            Value::Array(checked.baseline_invalid_dependencies.iter().map(|n| text(n)).collect()),
                        ),
                        (
                            "invalid_candidate_dependencies",
                            Value::Array(checked.candidate_invalid_dependencies.iter().map(|n| text(n)).collect()),
                        ),
                    ],
                );
            }
        }
    }
    if let Some(prefix) = &fixed_prefix {
        require_retained_prefix(&critical.board, prefix)?;
    }
    let sites_as_copper = fixed_prefix.is_some();
    Ok((critical, reserved, held, held_as_copper, sites_as_copper, fixed_prefix))
}

/// Ownership is the admitted ordered prefix, including duplicate occurrences.
/// Coordinate containment cannot stand in for this check.
fn require_retained_prefix(board: &PhysicalBoard, prefix: &crate::fixed_prefix::AdmittedFixedPrefix) -> Result<()> {
    if !board.tracks.starts_with(&prefix.board().tracks) || !board.vias.starts_with(&prefix.board().vias) {
        return Err(Error::Value("critical routing changed an admitted local escape prefix".to_owned()));
    }
    Ok(())
}

struct CriticalRetryCheck {
    improved: bool,
    adopted: bool,
    source_masks_present: Option<bool>,
    baseline_invalid: Vec<String>,
    candidate_invalid: Vec<String>,
    baseline_invalid_dependencies: Vec<String>,
    candidate_invalid_dependencies: Vec<String>,
}

/// The only mutation point for critical retry adoption. Candidate ownership
/// admission has already run in route_critical_nets. This additionally preserves
/// the complete ordinary escape ledger, using source-authorized mask counts.
#[allow(clippy::too_many_arguments)]
fn adopt_critical_retry(
    source: &PhysicalBoard,
    incumbent: &mut CriticalRoutingResult,
    planned: &mut Reserved,
    held_as_copper: &mut bool,
    held: Option<&Held>,
    prefix: (&[TrackSegment], &[Via]),
    prefix_masks: Option<&[copper_physical::CopperKeepout]>,
    candidate: CriticalRoutingResult,
    candidate_held_as_copper: bool,
    kept: Vec<copper_physical::CopperKeepout>,
) -> Result<CriticalRetryCheck> {
    let failures = |result: &CriticalRoutingResult| result.nets.iter().filter(|r| !r.connected).count();
    let improved = failures(&candidate) < failures(incumbent);
    let mut checked = CriticalRetryCheck {
        improved,
        adopted: false,
        source_masks_present: None,
        baseline_invalid: Vec::new(),
        candidate_invalid: Vec::new(),
        baseline_invalid_dependencies: Vec::new(),
        candidate_invalid_dependencies: Vec::new(),
    };
    if !improved {
        return Ok(checked);
    }
    let before = invalid_reserved_escape_nets(
        source,
        &incumbent.board,
        planned,
        prefix,
        prefix_masks,
        held.filter(|_| !*held_as_copper),
    )?;
    let after = invalid_reserved_escape_nets(
        source,
        &candidate.board,
        planned,
        prefix,
        prefix_masks,
        held.filter(|_| !candidate_held_as_copper),
    )?;
    checked.source_masks_present = Some(before.source_masks_present && after.source_masks_present);
    checked.baseline_invalid = before.invalid_escape_nets;
    checked.candidate_invalid = after.invalid_escape_nets;
    checked.baseline_invalid_dependencies = before.invalid_dependencies;
    checked.candidate_invalid_dependencies = after.invalid_dependencies;
    checked.adopted = checked.source_masks_present == Some(true)
        && checked.candidate_invalid.is_empty()
        && checked.candidate_invalid_dependencies.is_empty();
    if checked.adopted {
        *incumbent = candidate;
        planned.2 = kept;
        *held_as_copper = candidate_held_as_copper;
    }
    Ok(checked)
}

/// Freshly validate complete promised escape groups against the real candidate
/// copper, including fixed lattice and conservatively held bypass dependencies.
/// Remove only the represented generated multiplicities above trusted source
/// occurrences. Missing source masks invalidate the context. Extra occurrences
/// are retained; identity or geometry equality alone never grants removal.
fn strip_generated_keepouts(
    source: &PhysicalBoard,
    candidate: &PhysicalBoard,
    generated: &[copper_physical::CopperKeepout],
) -> Result<(PhysicalBoard, bool)> {
    let trusted = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(source.clone())?)?;
    let mut frame = candidate.clone();
    let mut source_masks_present = true;
    let mut inspected = Vec::new();
    let mut removal = Vec::new();
    for k in generated {
        if inspected.contains(k) {
            continue;
        }
        inspected.push(k.clone());
        let authored = trusted.copper_keepouts.iter().filter(|item| *item == k).count();
        let present = frame.copper_keepouts.iter().filter(|item| *item == k).count();
        source_masks_present &= present >= authored;
        let owned = generated.iter().filter(|item| *item == k).count();
        let removable = owned.min(present.saturating_sub(authored));
        removal.extend(std::iter::repeat_n(k.clone(), removable));
    }
    // Also detect missing authored masks that do not equal any generated mask.
    for k in &trusted.copper_keepouts {
        let authored = trusted.copper_keepouts.iter().filter(|item| *item == k).count();
        source_masks_present &= frame.copper_keepouts.iter().filter(|item| *item == k).count() >= authored;
    }
    for k in removal {
        let occurrence = frame.copper_keepouts.iter().rposition(|item| *item == k).expect("counted occurrence");
        frame.copper_keepouts.remove(occurrence);
    }
    Ok((frame, source_masks_present))
}

/// The complete validation frame has promised foreign escapes as real copper.
/// This runs before release_escape_masks joins them, so none are bound yet.
/// Explicit lattice/held prefixes are supplied only at the critical retry;
/// completion has already bound both and supplies empty/None here.
struct ReservedCopperCheck {
    source_masks_present: bool,
    invalid_escape_nets: Vec<String>,
    invalid_dependencies: Vec<String>,
}

fn invalid_reserved_escape_nets(
    source: &PhysicalBoard,
    candidate: &PhysicalBoard,
    reserved: &Reserved,
    prefix: (&[TrackSegment], &[Via]),
    prefix_masks: Option<&[copper_physical::CopperKeepout]>,
    unbound_held: Option<&Held>,
) -> Result<ReservedCopperCheck> {
    let (frame, source_masks_present) =
        reserved_geometry_frame(source, candidate, reserved, prefix, prefix_masks, unbound_held)?;
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&frame)?;
    let invalid_escape_nets = invalid_contribution_nets(&frame, &index, &reserved.0, &reserved.1)?;
    let mut invalid_dependencies: Vec<String> = invalid_contribution_nets(&frame, &index, prefix.0, prefix.1)?
        .iter()
        .map(|net| format!("fixed_prefix:{net}"))
        .collect();
    if let Some(held) = unbound_held {
        invalid_dependencies.extend(
            invalid_contribution_nets(&frame, &index, &held.tracks, &held.vias)?
                .iter()
                .map(|net| format!("held_bypass:{net}")),
        );
    }
    // Every original prefix/held occurrence is present unchanged, not inferred
    // from a net name. This checks geometry and via returns; package pad/access
    // proof must remain a separate check against its original provenance.
    Ok(ReservedCopperCheck { source_masks_present, invalid_escape_nets, invalid_dependencies })
}

fn reserved_geometry_frame(
    source: &PhysicalBoard,
    candidate: &PhysicalBoard,
    reserved: &Reserved,
    prefix: (&[TrackSegment], &[Via]),
    prefix_masks: Option<&[copper_physical::CopperKeepout]>,
    unbound_held: Option<&Held>,
) -> Result<(PhysicalBoard, bool)> {
    let mut generated = reserved.2.clone();
    generated.extend_from_slice(prefix_masks.unwrap_or_default());
    if let Some(held) = unbound_held {
        generated.extend_from_slice(&held.keepouts);
    }
    let (mut frame, source_masks_present) = strip_generated_keepouts(source, candidate, &generated)?;
    frame.tracks.extend_from_slice(prefix.0);
    frame.vias.extend_from_slice(prefix.1);
    if let Some(held) = unbound_held {
        frame.tracks.extend_from_slice(&held.tracks);
        frame.vias.extend_from_slice(&held.vias);
    }
    // All promised foreign-net copper participates in the exact check, not
    // merely the net currently being inspected. This is a read-only frame.
    frame.tracks.extend_from_slice(&reserved.0);
    frame.vias.extend_from_slice(&reserved.1);
    Ok((frame, source_masks_present))
}

/// Operational telemetry only: a geometry check is not original pad/access
/// proof. In particular a legal resited stem cannot prove the old pad's access.
/// All temporary frames are disposable and failures never affect adoption.
fn observe_source_reserved_geometry(
    source: &PhysicalBoard,
    reserved: &Reserved,
    prefix: (&[TrackSegment], &[Via]),
    unbound_held: Option<&Held>,
    boundary: &'static str,
    on_progress: &mut Option<ProgressCallback<'_>>,
) {
    if on_progress.is_none() {
        return;
    }
    let initial =
        copper_pcb::hard_macros::macro_source(source.clone()).and_then(copper_drc::macros::materialize_hard_macros);
    match initial {
        Ok(initial) => observe_reserved_geometry(
            source,
            &initial,
            reserved,
            prefix,
            None,
            unbound_held,
            boundary,
            None,
            on_progress,
        ),
        Err(error) => emit_reserved_geometry_error(boundary, None, &error, on_progress),
    }
}

fn emit_reserved_geometry_error(
    boundary: &'static str,
    link: Option<&str>,
    error: &Error,
    on_progress: &mut Option<ProgressCallback<'_>>,
) {
    emit(
        on_progress,
        "ordinary_escapes",
        "geometry_checked",
        vec![
            ("boundary", text(boundary)),
            ("link", link.map_or(Value::Null, text)),
            ("status", text("unavailable")),
            ("scope", text("legal_geometry_and_via_returns")),
            ("package_access_proof", text("not_checked")),
            ("reason", text(&error.to_string())),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
fn observe_reserved_geometry(
    source: &PhysicalBoard,
    candidate: &PhysicalBoard,
    reserved: &Reserved,
    prefix: (&[TrackSegment], &[Via]),
    prefix_masks: Option<&[copper_physical::CopperKeepout]>,
    unbound_held: Option<&Held>,
    boundary: &'static str,
    link: Option<&str>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) {
    if on_progress.is_none() {
        return;
    }
    let result = (|| -> Result<Vec<(&'static str, Value)>> {
        let checked = invalid_reserved_escape_nets(source, candidate, reserved, prefix, prefix_masks, unbound_held)?;
        let (frame, _) = reserved_geometry_frame(source, candidate, reserved, prefix, prefix_masks, unbound_held)?;
        let first = first_invalid_reserved_piece(&frame, reserved)?;
        const NET_LIMIT: usize = 32;
        let bounded_nets = |nets: &[String]| Value::Array(nets.iter().take(NET_LIMIT).map(|n| text(n)).collect());
        Ok(vec![
            ("boundary", text(boundary)),
            ("link", link.map_or(Value::Null, text)),
            ("status", text("checked")),
            ("scope", text("legal_geometry_and_via_returns")),
            ("package_access_proof", text("not_checked")),
            ("source_masks_present", Value::Bool(checked.source_masks_present)),
            ("invalid_escape_nets", bounded_nets(&checked.invalid_escape_nets)),
            ("invalid_escape_net_count", int(checked.invalid_escape_nets.len() as i64)),
            ("invalid_dependency_nets", bounded_nets(&checked.invalid_dependencies)),
            ("invalid_dependency_net_count", int(checked.invalid_dependencies.len() as i64)),
            ("net_limit", int(NET_LIMIT as i64)),
            ("reserved_track_count", int(reserved.0.len() as i64)),
            ("reserved_via_count", int(reserved.1.len() as i64)),
            ("reserved_mask_count", int(reserved.2.len() as i64)),
            ("candidate_track_count", int(candidate.tracks.len() as i64)),
            ("candidate_via_count", int(candidate.vias.len() as i64)),
            ("candidate_mask_count", int(candidate.copper_keepouts.len() as i64)),
            // This is copper-only identity, not serialized admission authority.
            ("frame_copper_digest", text(&critical_fingerprint("escape-geometry", &frame.tracks, &frame.vias, &[]))),
            ("first_invalid_piece", first.unwrap_or(Value::Null)),
        ])
    })();
    match result {
        Ok(fields) => emit(on_progress, "ordinary_escapes", "geometry_checked", fields),
        Err(error) => emit_reserved_geometry_error(boundary, link, &error, on_progress),
    }
}

fn first_invalid_reserved_piece(frame: &PhysicalBoard, reserved: &Reserved) -> Result<Option<Value>> {
    let index = crate::routing_clearance::RoutingClearanceIndex::new(frame)?;
    let point = |point: Point| record(vec![("x_nm", int(point.x_nm)), ("y_nm", int(point.y_nm))]);
    let evidence = |blockers: Vec<crate::routing_clearance::RoutingBlocker>| {
        const LIMIT: usize = 4;
        let total = blockers.len();
        record(vec![
            ("count", int(total as i64)),
            ("omitted_count", int(total.saturating_sub(LIMIT) as i64)),
            (
                "records",
                Value::Array(
                    blockers
                        .into_iter()
                        .take(LIMIT)
                        .map(|blocker| {
                            record(vec![
                                ("identity", text(&blocker.identity)),
                                ("occurrence", text(&blocker.occurrence)),
                                ("reason", text(&blocker.reason)),
                                ("net", blocker.net.as_deref().map_or(Value::Null, text)),
                                ("immutable", Value::Bool(blocker.immutable)),
                                ("layers", Value::Array(blocker.layers.iter().map(|layer| text(layer)).collect())),
                                (
                                    "positions_nm",
                                    Value::Array(
                                        blocker
                                            .positions_nm
                                            .into_iter()
                                            .map(|(x_nm, y_nm)| point(Point { x_nm, y_nm }))
                                            .collect(),
                                    ),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    };
    for (occurrence, track) in reserved.0.iter().enumerate() {
        if !index.can_track(&track.net, track.start, track.end, track.width_nm, track.layer)? {
            return Ok(Some(record(vec![
                ("kind", text("track")),
                ("reserved_occurrence", int(occurrence as i64)),
                ("net", text(&track.net)),
                ("layer", text(track.layer.value())),
                ("start", point(track.start)),
                ("end", point(track.end)),
                ("blockers", evidence(index.track_blocker_records(track)?)),
            ])));
        }
    }
    for (occurrence, via) in reserved.1.iter().enumerate() {
        let rule = frame.net_routing_rules.iter().find(|rule| rule.net == via.net);
        let legal = index.can_via(
            &via.net,
            via.position,
            via.size_nm,
            via.from_layer,
            via.to_layer,
            Some(via.drill_nm),
            false,
            None,
        )?;
        let returns = crate::return_paths::existing_returns_permit(frame, rule, via);
        if !legal || !returns {
            return Ok(Some(record(vec![
                ("kind", text("via")),
                ("reserved_occurrence", int(occurrence as i64)),
                ("net", text(&via.net)),
                ("position", point(via.position)),
                ("via_returns_permit", Value::Bool(returns)),
                ("blockers", evidence(index.via_blocker_records(via)?)),
            ])));
        }
    }
    Ok(None)
}

fn invalid_contribution_nets(
    frame: &PhysicalBoard,
    index: &crate::routing_clearance::RoutingClearanceIndex<'_>,
    tracks: &[TrackSegment],
    vias: &[Via],
) -> Result<Vec<String>> {
    let mut nets: Vec<String> =
        tracks.iter().map(|t| t.net.clone()).chain(vias.iter().map(|v| v.net.clone())).collect();
    nets.sort();
    nets.dedup();
    let mut invalid = Vec::new();
    'groups: for net in nets {
        for track in tracks.iter().filter(|t| t.net == net) {
            if !index.can_track(&track.net, track.start, track.end, track.width_nm, track.layer)? {
                invalid.push(net.clone());
                continue 'groups;
            }
        }
        for via in vias.iter().filter(|v| v.net == net) {
            let rule = frame.net_routing_rules.iter().find(|r| r.net == net);
            if !index.can_via(
                &via.net,
                via.position,
                via.size_nm,
                via.from_layer,
                via.to_layer,
                Some(via.drill_nm),
                false,
                None,
            )? || !crate::return_paths::existing_returns_permit(frame, rule, via)
            {
                invalid.push(net.clone());
                continue 'groups;
            }
        }
    }
    Ok(invalid)
}

/// `_complete_critical_stage` for a run without reservations or held copper.
#[allow(clippy::too_many_arguments)]
fn complete_critical_stage(
    source: &PhysicalBoard,
    mut critical: CriticalRoutingResult,
    reserved: Option<&Reserved>,
    held: Option<&Held>,
    held_as_copper: bool,
    sites_as_copper: bool,
    unified_options: &UnifiedRouterOptions,
    plane_stitch_options: Option<&PlaneStitchOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    array_plans: Option<&IndexMap<String, ArrayEscape>>,
    lattice: Option<&Lattice>,
    fanout_options: Option<&FanoutOptions>,
    escape_plan_options: Option<&EscapePlanOptions>,
) -> Result<(CriticalRoutingResult, Option<PlaneStitchResult>)> {
    if let Some(plan) = lattice.filter(|plan| !sites_as_copper && (!plan.tracks.is_empty() || !plan.vias.is_empty())) {
        // The lattice copper joins the board first (bypasses were planned on it).
        critical.board = join_reserved_copper(source, &critical.board, &plan.tracks, &plan.vias, &plan.keepouts)?;
    }
    if let Some(held) = held.filter(|_| !held_as_copper) {
        // The critical router kept clear of the reserved bypass copper, so it
        // joins the board unchanged.
        critical.board = join_reserved_copper(source, &critical.board, &held.tracks, &held.vias, &held.keepouts)?;
    }
    if !critical.board.decoupling_links.is_empty() && !unified_options.direct && unified_options.pre_route_decouplers {
        complete_bypasses(source, &mut critical, reserved, on_progress, fanout_options)?;
    }
    if let Some(choice) = &unified_options.power_islands {
        let layer = if choice == "auto" {
            crate::power_islands::auto_power_layer(&critical.board)
        } else {
            match CopperLayer::parse(choice) {
                Some(layer) => Some(layer),
                None => {
                    return Err(Error::Value(format!(
                        "{} is not a valid CopperLayer",
                        copper_json::python_str_repr(choice)
                    )));
                }
            }
        };
        if let Some(layer) = layer {
            emit(on_progress, "power_islands", "started", vec![("layer", text(layer.value()))]);
            let partition = if unified_options.power_islands_ball_grid_only {
                crate::power_islands::plan_ball_grid_islands(&critical.board, layer)
            } else {
                crate::power_islands::plan_power_islands(&critical.board, layer)
            }
            .map_err(|error| Error::NotImplemented(error.crash_line()))?;
            critical.board.zones.extend(partition.zones.iter().cloned());
            emit(on_progress, "power_islands", "finished", partition.report());
        }
    }
    if unified_options.bga_fanout {
        emit(on_progress, "bga_fanout", "started", Vec::new());
        let bga = plan_bga_fanouts(&critical.board, 2, None, array_plans)?;
        emit(on_progress, "bga_fanout", "finished", bga.report());
        if let Some(path) = unified_options.save_inputs_path.as_deref().filter(|path| !path.is_empty())
            && (!bga.tracks.is_empty() || !bga.vias.is_empty())
        {
            // Beside the router inputs: the fanout copper, so a replay can
            // take it out and fan out again.
            let text = crate::state_codec::bga_sidecar_text(
                &bga.tracks,
                &bga.vias,
                array_plans.is_some(),
                escape_plan_options,
            );
            crate::unified_router::route::save_inputs(&format!("{path}.bga.json"), &text)
                .map_err(|error| Error::NotImplemented(error.crash_line()))?;
        }
        critical.board = bga.board;
    }
    let plane_stitch = match plane_stitch_options {
        Some(options) => Some(crate::plane::stitch_zone_pads(&critical.board, Some(options))?),
        None => None,
    };
    Ok((critical, plane_stitch))
}

/// `_route_ordinary_stage`: ordinary routing then land closure and DRC.
fn route_ordinary_stage(
    unified_options: &UnifiedRouterOptions,
    fanout_options: Option<&FanoutOptions>,
    pre_detailed: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    drc_policy: Policy,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(DetailedRoutingResult, DrcReport, DuplicatePadStitchResult)> {
    emit(on_progress, "ordinary_area", "started", Vec::new());
    // `_route_unified_compat`: failed signal nets get pad escapes
    // (`route_fanout`) in the seeded repair attempts.
    let mut seeder = |partial: &PhysicalBoard, nets: &IndexSet<String>| {
        crate::fanout::route_fanout(partial, fanout_options, Some(nets))
            .map(|fanout| fanout.board)
            .map_err(crate::unified_error::Error::from)
    };
    let escape_seeder: Option<&mut crate::unified_router::route::EscapeSeeder<'_>> =
        if fanout_options.is_some() && !unified_options.direct { Some(&mut seeder) } else { None };
    let detailed = crate::unified_router::route::route_unified(
        pre_detailed,
        global_route,
        unified_options,
        None,
        None,
        None,
        escape_seeder,
        reborrow(on_progress),
    )
    .map_err(|error| match error {
        crate::unified_error::Error::Value(message) => Error::Value(message),
        other => Error::NotImplemented(other.crash_line()),
    })?;
    let failed: Vec<Value> = detailed.nets.iter().filter(|net| !net.connected).map(|net| text(&net.net)).collect();
    emit(
        on_progress,
        "ordinary_area",
        "finished",
        vec![
            ("status", text(detailed.status.value())),
            ("failed_nets", Value::Array(failed)),
            ("passes", int(detailed.metrics.passes)),
        ],
    );
    emit(on_progress, "land_closure_native_drc", "started", Vec::new());
    let (detailed, drc, duplicate) = close_detailed_lands(&detailed, drc_policy)?;
    emit(
        on_progress,
        "land_closure_native_drc",
        "finished",
        vec![("decision", text(crate::drc_report::decision_value(drc.decision())))],
    );
    Ok((detailed, drc, duplicate))
}

/// `_unified_profile_review`: report final profile copper without calling
/// another routing solver; unsupported differential obligations fail closed.
fn unified_profile_review(
    detailed: &DetailedRoutingResult,
    global_route: &GlobalRoutingResult,
    drc: &DrcReport,
) -> Result<CriticalRoutingResult> {
    const BUDGET_CODES: [&str; 3] = ["DRC-MAX-LENGTH", "DRC-DIFF-SKEW", "DRC-LENGTH-MATCH"];
    let board = &detailed.board;
    let graph = explicit_copper_connectivity(board, Candidate::default())?;
    let nets: IndexMap<&str, _> = board.nets.iter().map(|net| (net.name.as_str(), net)).collect();
    let mut rules: Vec<_> = board.net_routing_rules.iter().map(|rule| (rule.net.as_str(), rule)).collect();
    // `{rule.net: rule}` keeps the last rule of a net; sorted by net name.
    let mut by_net: IndexMap<&str, _> = IndexMap::new();
    for (name, rule) in rules.drain(..) {
        by_net.insert(name, rule);
    }
    let mut sorted: Vec<_> = by_net.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut results = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for (name, rule) in sorted {
        if rule.kind == RouteKind::General || seen.contains(name) {
            continue;
        }
        let group: Vec<String> = match &rule.differential_partner {
            Some(partner) => {
                let mut group: Vec<String> =
                    IndexSet::<String>::from([name.to_owned(), partner.clone()]).into_iter().collect();
                group.sort();
                group
            }
            None => vec![name.to_owned()],
        };
        seen.extend(group.iter().cloned());
        let tracks: Vec<_> = board.tracks.iter().filter(|t| group.contains(&t.net)).collect();
        let vias = board.vias.iter().filter(|v| group.contains(&v.net)).count();
        let lengths: Vec<i64> = group
            .iter()
            .map(|net| {
                tracks
                    .iter()
                    .filter(|t| &t.net == net)
                    .map(|t| round_int(hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64)))
                    .sum()
            })
            .collect();
        let connected = group.iter().all(|net| nets.get(net.as_str()).is_some_and(|item| graph.net_connected(item)));
        let touches = |finding_nets: &[String]| finding_nets.iter().any(|net| group.contains(net));
        let mut diagnostics: Vec<String> = drc
            .findings()
            .iter()
            .filter(|f| f.severity == copper_drc::Severity::Error && touches(&f.nets))
            .map(|f| f.message.clone())
            .collect();
        let mut warnings: Vec<String> = Vec::new();
        for finding in drc.findings() {
            if BUDGET_CODES.contains(&finding.code.as_str())
                && touches(&finding.nets)
                && !warnings.contains(&finding.message)
            {
                warnings.push(finding.message.clone());
            }
        }
        if !connected {
            diagnostics.push("unified routing left profile terminals disconnected".to_owned());
        }
        if matches!(rule.kind, RouteKind::Differential | RouteKind::CanBus) {
            diagnostics
                .push("unified differential coupling and return-path verification is not implemented".to_owned());
        }
        let mut unique: Vec<String> = Vec::new();
        for item in diagnostics {
            if !unique.contains(&item) {
                unique.push(item);
            }
        }
        let skew = if lengths.is_empty() {
            0
        } else {
            lengths.iter().max().expect("lengths") - lengths.iter().min().expect("lengths")
        };
        let mut result = CriticalNetResult::new(
            group,
            connected && unique.is_empty(),
            tracks.len() as i64,
            vias as i64,
            lengths,
            skew,
            unique,
        );
        result.strategy = "unified_final_review".to_owned();
        result.warnings = warnings;
        results.push(result);
    }
    let status = if results.iter().any(|item| !item.connected) {
        CriticalRoutingStatus::Failed
    } else if results.iter().any(|item| !item.warnings.is_empty()) {
        CriticalRoutingStatus::Warning
    } else {
        CriticalRoutingStatus::Success
    };
    Ok(critical_result(
        status,
        board.clone(),
        results,
        &global_route.routing_fingerprint,
        detailed.routing_fingerprint.clone(),
    ))
}

/// `run_routing_pipeline`: steps 4-8 in dependency order with the unified
/// negotiated router.
pub fn run_routing_pipeline(
    board: PhysicalBoard,
    options: RoutingPipelineOptions,
    on_progress: Option<ProgressCallback<'_>>,
) -> Result<RoutingPipelineResult> {
    run_routing_pipeline_staged(board, options, on_progress, None)
}

/// [`run_routing_pipeline`] with `on_stage(name, value)`, which observes
/// completed stages (e.g. for checkpoints).
pub fn run_routing_pipeline_staged(
    board: PhysicalBoard,
    mut options: RoutingPipelineOptions,
    mut on_progress: Option<ProgressCallback<'_>>,
    mut on_stage: Option<StageCallback<'_>>,
) -> Result<RoutingPipelineResult> {
    crate::install_hooks();
    let mut unified_options = options.unified_options.clone().unwrap_or_default();
    let mut unified_fanout = options.fanout_options.clone();
    let board = copper_pcb::hard_macros::macro_source(board)?;
    let placement_options = match options.placement_options {
        Some(placement_options) => placement_options,
        None => PlannerOptions::default().validated()?,
    };
    let global_options = match options.global_options {
        Some(global_options) => global_options,
        None => GlobalRouterOptions::default().validated()?,
    };
    let feedback_options = match options.feedback_options {
        Some(feedback_options) => feedback_options,
        None => PlacementRoutingFeedbackOptions {
            initial_movement_nm: global_options.tile_size_nm,
            ..PlacementRoutingFeedbackOptions::default()
        }
        .validated()?,
    };
    if options.detailed_feedback_trials < 0
        || options.critical_feedback_trials < 0
        || options.placement_feedback_trials < 0
        || options.detailed_feedback_movement_nm <= 0
    {
        return Err(Error::Value("routing placement feedback bounds are invalid".to_owned()));
    }
    if options.critical_pair_state_limit < 0 {
        return Err(Error::Value("critical pair state limit must be non-negative".to_owned()));
    }
    if let Some(stop_after) = &options.stop_after
        && !STOP_STAGES.contains(&stop_after.as_str())
    {
        return Err(Error::Value(format!("stop_after must be one of {}", STOP_STAGES.join(", "))));
    }
    let placement = if let Some(placement) = options.initial_placement.take() {
        let names = |board: &PhysicalBoard| board.nets.iter().map(|net| net.name.clone()).collect::<IndexSet<_>>();
        let (saved, current) = (names(&placement.board), names(&board));
        if placement.board.name != board.name
            || saved.len() != current.len()
            || !saved.iter().all(|n| current.contains(n))
        {
            return Err(Error::Value("the supplied placement belongs to a different board".to_owned()));
        }
        emit(
            &mut on_progress,
            "placement_global",
            "reused",
            vec![
                ("candidate", text(&placement.placement_candidate)),
                ("global_status", text(placement.global_route.status.value())),
            ],
        );
        placement
    } else {
        emit(&mut on_progress, "placement_global", "started", Vec::new());
        let placement = {
            let progress: Option<ProgressCallback<'_>> = match &mut on_progress {
                Some(callback) => Some(&mut **callback),
                None => None,
            };
            optimize_placement_for_routing(&board, &placement_options, &global_options, &feedback_options, progress)?
        };
        emit(
            &mut on_progress,
            "placement_global",
            "finished",
            vec![
                ("candidate", text(&placement.placement_candidate)),
                ("global_status", text(placement.global_route.status.value())),
            ],
        );
        placement
    };
    if let Some(callback) = on_stage.as_mut() {
        callback(Stage::Placement(&placement));
    }
    if options.stop_after.as_deref() == Some("placement") {
        emit(&mut on_progress, "ordinary_area", "stopped", vec![("reason", text("stop_after_placement"))]);
        let (critical, detailed, drc) = stopped_after_placement(&placement, options.drc_policy)?;
        return Ok(RoutingPipelineResult {
            status: PhysicalFlowStatus::Fail,
            placement_and_global: placement,
            critical,
            detailed,
            drc,
            plane_stitch: None,
            duplicate_pad_stitch: None,
            stopped_after: Some("placement".to_owned()),
            unified_options,
            placement_feedback: None,
            retained_prefix: None,
            retained_escape_policy: None,
        });
    }
    let escape_plan_options = options.escape_plan_options.as_ref();
    let (array_plans, surface_balls) = plan_array_frames(&placement.board, escape_plan_options, &mut on_progress)?;
    if let (Some(surface), Some(fanout)) = (&surface_balls, unified_fanout.as_mut()) {
        fanout.surface_pads.extend(surface.iter().cloned());
    }
    let lattice = plan_lattice(&placement.board, escape_plan_options, surface_balls.as_ref(), &mut on_progress)?;
    if let Some(plan_options) = escape_plan_options.filter(|options| options.two_end_order) {
        let plan = plan_two_end_escapes(&placement.board, plan_options, None)?;
        emit(
            &mut on_progress,
            "escape_plan",
            "finished",
            vec![
                ("bundles", int(plan.pairs.len() as i64)),
                ("planned_nets", int(plan.net_order.len() as i64)),
                ("surface_pads", int(plan.surface_pads.len() as i64)),
            ],
        );
        unified_options.escape_net_order = plan.net_order.clone();
        unified_options.escape_layer_biases = plan.layer_biases.clone();
        if let Some(fanout) = unified_fanout.as_mut() {
            fanout.surface_pads.extend(plan.surface_pads.iter().cloned());
        }
    }
    let plane_stitch_options = if unified_options.direct { None } else { options.plane_stitch_options.clone() };
    let pair_state_limit = unified_options
        .critical_pair_state_limit
        .filter(|limit| *limit != 0)
        .or(Some(options.critical_pair_state_limit).filter(|limit| *limit != 0));
    let tail = TailInputs {
        unified_options: &unified_options,
        unified_fanout: unified_fanout.as_ref(),
        pair_state_limit,
        critical_plane_options: options.plane_stitch_options.as_ref(),
        plane_stitch_options: plane_stitch_options.as_ref(),
        drc_policy: options.drc_policy,
        escape_plan_options,
    };
    if unified_options.direct {
        emit(
            &mut on_progress,
            "unified_direct",
            "started",
            vec![("reason", text("all mutable nets negotiate together; only source-fixed copper is retained"))],
        );
    }
    let routed =
        route_tail(&placement, array_plans.as_ref(), lattice.as_ref(), &tail, &mut on_progress, &mut on_stage)?;
    let feedback_budget =
        options.placement_feedback_trials.max(options.detailed_feedback_trials + options.critical_feedback_trials);
    let mut placement = placement;
    let (mut critical, mut detailed, mut drc) = (routed.critical, routed.detailed, routed.drc);
    let (mut plane_stitch, mut duplicate_stitch) = (routed.plane_stitch, routed.duplicate);
    let mut retained_prefix = routed.retained_prefix;
    let mut retention_policy = routed.retained_escape_policy;
    let placement_feedback = if feedback_budget != 0 {
        let state = FeedbackState {
            board: placement.board.clone(),
            global_route: placement.global_route.clone(),
            critical,
            detailed,
            drc,
            deferred_nets: routed.deferred,
            payload: (plane_stitch, duplicate_stitch, None, retained_prefix, retention_policy),
        };
        // Each trial routes from its own placement, so it must not overwrite
        // the replay inputs of the run that is kept: trials save beside them,
        // and only the accepted trial's file replaces them.
        let inputs_path = unified_options.save_inputs_path.clone();
        let trial_inputs = std::cell::RefCell::new(Vec::<String>::new());
        let mut rerun = |trial_board: &PhysicalBoard,
                         trial_global: &GlobalRoutingResult,
                         progress: &mut Option<ProgressCallback<'_>>|
         -> Result<FeedbackState<TailPayload>> {
            let mut trial_options = unified_options.clone();
            let saved = inputs_path.as_ref().map(|path| {
                let mut list = trial_inputs.borrow_mut();
                let name = format!("{path}.trial-{}", list.len() + 1);
                list.push(name);
                list.last().expect("trial inputs").clone()
            });
            if saved.is_some() {
                trial_options.save_inputs_path = saved.clone();
            }
            let trial_tail = TailInputs { unified_options: &trial_options, ..tail };
            let mut trial = placement.clone();
            trial.board = trial_board.clone();
            trial.global_route = trial_global.clone();
            // The escape frame (array plans, via lattice) of the trial placement.
            let (trial_plans, trial_surface) = plan_array_frames(trial_board, escape_plan_options, progress)?;
            let trial_lattice = plan_lattice(trial_board, escape_plan_options, trial_surface.as_ref(), progress)?;
            let routed =
                route_tail(&trial, trial_plans.as_ref(), trial_lattice.as_ref(), &trial_tail, progress, &mut None)?;
            Ok(FeedbackState {
                board: trial_board.clone(),
                global_route: trial_global.clone(),
                critical: routed.critical,
                detailed: routed.detailed,
                drc: routed.drc,
                deferred_nets: routed.deferred,
                payload: (
                    routed.plane_stitch,
                    routed.duplicate,
                    saved,
                    routed.retained_prefix,
                    routed.retained_escape_policy,
                ),
            })
        };
        let score = |state: &FeedbackState<TailPayload>| feedback_score(&state.critical, &state.detailed, &state.drc);
        let looped = crate::congestion_feedback::run_placement_feedback(
            state,
            feedback_budget,
            options.detailed_feedback_movement_nm,
            &placement_options,
            &global_options,
            &mut rerun,
            &score,
            &mut on_progress,
        );
        let accepted = looped.and_then(|(best, outcome)| {
            if outcome.accepted_moves != 0
                && let (Some(path), Some(accepted)) = (&inputs_path, &best.payload.2)
            {
                std::fs::rename(accepted, path).map_err(|failure| {
                    Error::NotImplemented(format!("an OSError moving the accepted trial's inputs ({failure})"))
                })?;
                // The accepted trial's ball-grid fanout sidecar goes with it.
                let (source, target) = (format!("{accepted}.bga.json"), format!("{path}.bga.json"));
                if std::fs::rename(&source, &target).is_err() {
                    let _ = std::fs::remove_file(&target);
                }
            }
            Ok((best, outcome))
        });
        for leftover in trial_inputs.borrow().iter() {
            // Rejected trials' inputs are not this run's; never leave them behind.
            for stale in [leftover.clone(), format!("{leftover}.bga.json")] {
                let _ = std::fs::remove_file(stale);
            }
        }
        let (best, outcome) = accepted?;
        if outcome.accepted_moves != 0 {
            placement.board = best.board;
            placement.global_route = best.global_route;
            placement.placement_candidate = format!("{}-congestion", placement.placement_candidate);
        }
        (critical, detailed, drc) = (best.critical, best.detailed, best.drc);
        (plane_stitch, duplicate_stitch, _, retained_prefix, retention_policy) = best.payload;
        Some(outcome)
    } else {
        None
    };
    let critical = if unified_options.direct {
        unified_profile_review(&detailed, &placement.global_route, &drc)?
    } else {
        critical
    };
    let passed = placement.full_route_certified
        && critical.status != CriticalRoutingStatus::Failed
        && detailed.status == DetailedRoutingStatus::Success
        && drc.decision() == copper_drc::Decision::Pass
        && retention_policy.requested_policy_satisfied(
            retained_prefix.as_ref().is_some_and(|prefix| prefix.missing_copper_occurrences(&detailed.board) == (0, 0)),
        );
    Ok(RoutingPipelineResult {
        status: if passed { PhysicalFlowStatus::Pass } else { PhysicalFlowStatus::Fail },
        placement_and_global: placement,
        critical,
        detailed,
        drc,
        plane_stitch,
        duplicate_pad_stitch: Some(duplicate_stitch),
        stopped_after: None,
        unified_options,
        placement_feedback,
        retained_prefix,
        retained_escape_policy: Some(retention_policy),
    })
}

/// What every pass of the pipeline tail shares (`_route_critical_stage`
/// onwards), on the main placement and on each congestion trial.
struct TailInputs<'a> {
    unified_options: &'a UnifiedRouterOptions,
    unified_fanout: Option<&'a FanoutOptions>,
    pair_state_limit: Option<i64>,
    /// The critical stage's plane options (the caller's, also when direct).
    critical_plane_options: Option<&'a PlaneStitchOptions>,
    plane_stitch_options: Option<&'a PlaneStitchOptions>,
    drc_policy: Policy,
    escape_plan_options: Option<&'a EscapePlanOptions>,
}

/// The plane stitch, duplicate-pad stitch and saved-inputs path of one tail pass.
type TailPayload = (
    Option<PlaneStitchResult>,
    DuplicatePadStitchResult,
    Option<String>,
    Option<crate::fixed_prefix::AdmittedFixedPrefix>,
    RetainedEscapePolicy,
);

struct TailResult {
    critical: CriticalRoutingResult,
    plane_stitch: Option<PlaneStitchResult>,
    deferred: IndexSet<String>,
    detailed: DetailedRoutingResult,
    drc: DrcReport,
    duplicate: DuplicatePadStitchResult,
    retained_prefix: Option<crate::fixed_prefix::AdmittedFixedPrefix>,
    retained_escape_policy: RetainedEscapePolicy,
}

/// Reservations, critical routing, held copper, islands, dogbones, plane
/// stitch, ordinary routing and closure on `placement`.
fn route_tail(
    placement: &PlacementRoutingResult,
    array_plans: Option<&IndexMap<String, ArrayEscape>>,
    lattice: Option<&Lattice>,
    tail: &TailInputs<'_>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    on_stage: &mut Option<StageCallback<'_>>,
) -> Result<TailResult> {
    let mut retention_policy =
        retained_escape_policy(&placement.board, tail.unified_options.retain_escape_copper, lattice)?;
    if tail.unified_options.direct && retention_policy.active() {
        retention_policy.applicability = "unsupported_or_uncensused";
        retention_policy.reason = "direct_pipeline_not_supported";
    }
    let (critical, reserved, held, held_as_copper, sites_as_copper, retained_prefix) = if tail.unified_options.direct {
        (direct_critical(placement)?, None, None, false, false, None)
    } else {
        route_critical_stage(
            placement,
            tail.unified_options,
            tail.unified_fanout,
            tail.pair_state_limit,
            tail.critical_plane_options,
            lattice,
            on_progress,
            on_stage,
        )?
    };
    let (mut critical, mut plane_stitch) = complete_critical_stage(
        &placement.board,
        critical,
        reserved.as_ref(),
        held.as_ref(),
        held_as_copper,
        sites_as_copper,
        tail.unified_options,
        tail.plane_stitch_options,
        on_progress,
        array_plans,
        lattice,
        tail.unified_fanout,
        tail.escape_plan_options,
    )?;
    let pre_detailed = plane_stitch.as_ref().map_or(&critical.board, |stitch| &stitch.board).clone();
    let deferred = copper_pcb::zones::deferred_zone_nets(&pre_detailed)?;
    let pre_detailed = release_escape_masks(
        &placement.board,
        reserved.as_ref(),
        &mut critical,
        &mut plane_stitch,
        pre_detailed,
        tail.unified_options,
        on_progress,
    )?;
    let (detailed, drc, duplicate) = route_ordinary_stage(
        tail.unified_options,
        tail.unified_fanout,
        &pre_detailed,
        &placement.global_route,
        tail.drc_policy,
        on_progress,
    )?;
    Ok(TailResult {
        critical,
        plane_stitch,
        deferred,
        detailed,
        drc,
        duplicate,
        retained_prefix,
        retained_escape_policy: retention_policy,
    })
}

/// Meaning of each acceptance score component (`_feedback_score`); lower is better.
pub const SCORE_ORDER: [&str; 7] = [
    "hard_drc_errors",
    "open_signal_nets",
    "critical_failures",
    "unrouted_nets",
    "conflict_overflow",
    "total_length_nm",
    "vias",
];

/// `_feedback_score`: the acceptance tuple of the placement feedback loop.
fn feedback_score(
    critical: &CriticalRoutingResult,
    detailed: &DetailedRoutingResult,
    drc: &DrcReport,
) -> Result<Vec<i64>> {
    let hard = drc
        .findings()
        .iter()
        .filter(|f| {
            !["DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"].contains(&f.code.as_str())
                && f.severity == copper_drc::Severity::Error
        })
        .count() as i64;
    let zone_nets = copper_pcb::zones::deferred_zone_nets(&detailed.board)?;
    let opens: IndexSet<&String> = drc
        .findings()
        .iter()
        .filter(|f| f.code == "DRC-OPEN-NET")
        .flat_map(|f| f.nets.iter())
        .filter(|net| !zone_nets.contains(*net))
        .collect();
    let critical_failures =
        critical.nets.iter().filter(|item| !item.connected || !item.diagnostics.is_empty()).count() as i64;
    let m = &detailed.metrics;
    Ok(vec![
        hard,
        opens.len() as i64,
        critical_failures,
        m.unrouted_net_count,
        m.total_conflict_overflow,
        m.total_length_nm,
        m.via_count,
    ])
}

// ---------------------------------------------------------------------------
// Ordinary escape reservations

/// Planned escape copper and the keepouts that reserve it.
pub(crate) type Reserved = (Vec<TrackSegment>, Vec<Via>, Vec<copper_physical::CopperKeepout>);

/// `Counter(after) - Counter(before)`, `.elements()` order.
fn added_elements<T: Clone + Eq + std::hash::Hash>(after: &[T], before: &[T]) -> Vec<T> {
    let mut counts: IndexMap<&T, i64> = IndexMap::new();
    for item in after {
        *counts.entry(item).or_insert(0) += 1;
    }
    for item in before {
        if let Some(count) = counts.get_mut(item) {
            *count -= 1;
        }
    }
    let mut result = Vec::new();
    for (item, count) in counts {
        for _ in 0..count.max(0) {
            result.push(item.clone());
        }
    }
    result
}

/// `drc._point_segment_distance` (float).
fn point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    if dx == 0 && dy == 0 {
        return hypot((point.x_nm - start.x_nm) as f64, (point.y_nm - start.y_nm) as f64);
    }
    let numerator =
        i128::from(point.x_nm - start.x_nm) * i128::from(dx) + i128::from(point.y_nm - start.y_nm) * i128::from(dy);
    let denominator = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
    let fraction = crate::routing_lattice::true_div(numerator, denominator).clamp(0.0, 1.0);
    hypot(
        point.x_nm as f64 - (start.x_nm as f64 + fraction * dx as f64),
        point.y_nm as f64 - (start.y_nm as f64 + fraction * dy as f64),
    )
}

/// `drc._segment_distance` (float).
fn segment_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    let orientation = |p: Point, q: Point, r: Point| -> i32 {
        let value = i128::from(q.y_nm - p.y_nm) * i128::from(r.x_nm - q.x_nm)
            - i128::from(q.x_nm - p.x_nm) * i128::from(r.y_nm - q.y_nm);
        value.signum() as i32
    };
    let on_segment = |p: Point, q: Point, r: Point| {
        p.x_nm.min(r.x_nm) <= q.x_nm
            && q.x_nm <= p.x_nm.max(r.x_nm)
            && p.y_nm.min(r.y_nm) <= q.y_nm
            && q.y_nm <= p.y_nm.max(r.y_nm)
    };
    let (o1, o2, o3, o4) = (orientation(a, b, c), orientation(a, b, d), orientation(c, d, a), orientation(c, d, b));
    let intersect = (o1 != o2 && o3 != o4)
        || (o1 == 0 && on_segment(a, c, b))
        || (o2 == 0 && on_segment(a, d, b))
        || (o3 == 0 && on_segment(c, a, d))
        || (o4 == 0 && on_segment(c, b, d));
    if intersect {
        return 0.0;
    }
    [(a, c, d), (b, c, d), (c, a, b), (d, a, b)]
        .iter()
        .map(|(p, s, e)| point_segment_distance(*p, *s, *e))
        .fold(f64::INFINITY, |acc, v| if v < acc { v } else { acc })
}

/// A disk-containing octagon for a via reservation, using integer outward
/// rounding throughout. Axis sides are at R; diagonal sides x+y=D have
/// D=ceil(sqrt(2*R*R)), so every side stays outside the intended disk.
/// This represents annulus plus board minimum gap only, not net-specific
/// clearance, drill legality, return paths or complete package-access proof.
fn via_reservation_octagon(position: Point, size_nm: i64, gap_nm: i64) -> Option<Vec<Point>> {
    if size_nm <= 0 || gap_nm < 0 {
        return None;
    }
    let radius = size_nm.div_euclid(2).checked_add(size_nm.rem_euclid(2))?.checked_add(gap_nm)?;
    let r = u128::try_from(radius).ok()?;
    let twice_squared = r.checked_mul(r)?.checked_mul(2)?;
    let floor = twice_squared.isqrt();
    let diagonal = floor.checked_add(u128::from(floor.checked_mul(floor)? < twice_squared))?;
    let tangent = i64::try_from(diagonal.checked_sub(r)?).ok()?;
    [
        (-radius, -tangent),
        (-tangent, -radius),
        (tangent, -radius),
        (radius, -tangent),
        (radius, tangent),
        (tangent, radius),
        (-tangent, radius),
        (-radius, tangent),
    ]
    .into_iter()
    .map(|(x, y)| Some(Point::new(position.x_nm.checked_add(x)?, position.y_nm.checked_add(y)?)))
    .collect()
}

/// `_copper_keepouts`: keepouts covering planned copper plus clearance.
pub(crate) fn copper_keepouts(
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    prefix: &str,
    fixed: Option<&PhysicalBoard>,
) -> Vec<copper_physical::CopperKeepout> {
    let gap = board.rules.minimum_clearance_nm;
    let layers = &board.stackup.copper_layers;
    let position = |layer: CopperLayer| layers.iter().position(|l| *l == layer).expect("layer");
    let span = |via: &Via| layers[position(via.from_layer)..=position(via.to_layer)].to_vec();
    let mut solid: Vec<(Point, Point, f64, Vec<CopperLayer>)> = Vec::new();
    if let Some(fixed) = fixed {
        for t in &fixed.tracks {
            solid.push((t.start, t.end, t.width_nm as f64 / 2.0, vec![t.layer]));
        }
        for v in &fixed.vias {
            solid.push((v.position, v.position, v.size_nm as f64 / 2.0, span(v)));
        }
    }
    let touches = |a: Point, b: Point, radius: i64, layers: &[CopperLayer]| {
        solid.iter().any(|(c, d, half, other)| {
            layers.iter().any(|l| other.contains(l)) && segment_distance(a, b, *c, *d) < radius as f64 * 1.42 + half
        })
    };
    let capsule = |a: Point, b: Point, radius: i64| -> Vec<Point> {
        let (dx, dy) = (b.x_nm - a.x_nm, b.y_nm - a.y_nm);
        let length = hypot(dx as f64, dy as f64);
        let (ux, uy) = if length != 0.0 { (dx as f64 / length, dy as f64 / length) } else { (1.0, 0.0) };
        let (nx, ny) = (-uy, ux);
        let mut corners = Vec::new();
        for ((px, py), sx) in [((a.x_nm, a.y_nm), -1i64), ((b.x_nm, b.y_nm), 1)] {
            let normals: [i64; 2] = if sx < 0 { [-1, 1] } else { [1, -1] };
            for sn in normals {
                let x = px as f64 + (sx * radius) as f64 * ux + (sn * radius) as f64 * nx;
                let y = py as f64 + (sx * radius) as f64 * uy + (sn * radius) as f64 * ny;
                corners.push(Point::new(round_int(x), round_int(y)));
            }
        }
        corners
    };
    let keepout = |id: String, layers: Vec<CopperLayer>, vertices: Vec<Point>| copper_physical::CopperKeepout {
        id,
        layers,
        vertices,
        block_tracks: true,
        block_vias: true,
        block_pads: false,
        block_zones: false,
        block_footprints: false,
    };
    let mut keepouts = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let radius = track.width_nm.div_euclid(2) + gap;
        if touches(track.start, track.end, radius, &[track.layer]) {
            continue;
        }
        keepouts.push(keepout(
            format!("{prefix}-track-{index}"),
            vec![track.layer],
            capsule(track.start, track.end, radius),
        ));
    }
    for (index, via) in vias.iter().enumerate() {
        let radius = via.size_nm.div_euclid(2) + gap;
        let layers = span(via);
        if touches(via.position, via.position, radius, &layers) {
            continue;
        }
        // Keep the legacy square fallback for unrepresentable/unsupported
        // inputs; never silently omit a reservation or change the Vec API.
        let vertices = via_reservation_octagon(via.position, via.size_nm, gap)
            .unwrap_or_else(|| capsule(via.position, via.position, radius));
        keepouts.push(keepout(format!("{prefix}-via-{index}"), layers, vertices));
    }
    keepouts
}

#[cfg(test)]
mod via_mask_geometry_tests {
    use super::*;
    use crate::test_support_routers as routers;
    use copper_physical::RoundedConvexShape;
    use copper_physical::geometry::{circle_inside_shape, shapes_clear};

    #[test]
    fn via_octagon_contains_disk_with_integer_outward_rounding_including_odd_nanometers() {
        let center = Point::new(77_875_000, 21_625_000);
        for (size, gap) in [(450_000, 100_000), (450_001, 100_000), (450_000, 100_001), (3, 0), (5, 7)] {
            let vertices = via_reservation_octagon(center, size, gap).expect("representable mask");
            let radius = size / 2 + size % 2 + gap;
            let tangent = center.y_nm - vertices[0].y_nm;
            let diagonal = i128::from(radius) + i128::from(tangent);
            let squared = 2 * i128::from(radius) * i128::from(radius);
            assert!(diagonal * diagonal >= squared, "diagonal sides must contain the disk");
            assert!((diagonal - 1) * (diagonal - 1) < squared, "ceil sqrt must be minimal");
            let polygon = RoundedConvexShape::new(vertices.clone(), 0).expect("octagon");
            assert!(circle_inside_shape(center, radius, &polygon));
            // Check every non-degenerate side without floating-point angles.
            for index in 0..vertices.len() {
                let a = vertices[index];
                let b = vertices[(index + 1) % vertices.len()];
                let dx = i128::from(b.x_nm - a.x_nm);
                let dy = i128::from(b.y_nm - a.y_nm);
                let length_squared = dx * dx + dy * dy;
                let cross = dx * i128::from(center.y_nm - a.y_nm) - dy * i128::from(center.x_nm - a.x_nm);
                assert!(cross >= 0, "consistent convex orientation");
                assert!(cross * cross >= i128::from(radius) * i128::from(radius) * length_squared);
            }
        }
    }

    #[test]
    fn real_via_mask_removes_square_corner_false_positive_but_preserves_disk_collisions() {
        let mut board = routers::pad_board(&[], 2, Vec::new());
        board.rules.minimum_clearance_nm = 100_000;
        let via = routers::via("S", (0.0, 0.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back);
        let masks = copper_keepouts(&board, &[], std::slice::from_ref(&via), "escape-reserve", None);
        assert_eq!(masks.len(), 1);
        assert_eq!(masks[0].vertices.len(), 8, "exercise the actual via-mask call site");
        let octagon = RoundedConvexShape::new(masks[0].vertices.clone(), 0).expect("octagon");
        let radius = 325_000;
        let square = RoundedConvexShape::new(
            vec![
                Point::new(-radius, -radius),
                Point::new(radius, -radius),
                Point::new(radius, radius),
                Point::new(-radius, radius),
            ],
            0,
        )
        .expect("legacy square");
        let disk = RoundedConvexShape::new(vec![Point::new(0, 0)], radius).expect("intended clearance disk");
        for (x_sign, y_sign) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
            // Actual ADC_DCO A1-to-reservation relative offset is (-.375,+.375) mm.
            let start = Point::new(x_sign * 375_000, y_sign * 375_000);
            let end = Point::new(x_sign * 625_000, y_sign * 375_000);
            let track = RoundedConvexShape::new(vec![start, end], 75_000).expect(".15 mm track");
            assert!(!shapes_clear(&square, &track, 0), "old square corner blocks the track");
            assert!(shapes_clear(&disk, &track, 0), "the intended disk permits this exit");
            assert!(shapes_clear(&octagon, &track, 0), "octagon must permit the clear corner exit");
        }
        let colliding = RoundedConvexShape::new(vec![Point::new(-375_000, 0), Point::new(-625_000, 0)], 75_000)
            .expect("real disk collision");
        assert!(!shapes_clear(&disk, &colliding, 0));
        assert!(!shapes_clear(&octagon, &colliding, 0), "a genuine disk collision remains blocked");
    }

    #[test]
    fn octagon_rejects_unrepresentable_or_unsupported_inputs() {
        assert!(via_reservation_octagon(Point::new(i64::MAX, 0), 450_000, 100_000).is_none());
        assert!(via_reservation_octagon(Point::new(0, 0), 0, 100_000).is_none());
    }
}

/// `_route_ordinary_escapes`: escapes of `nets` on the macro frame.
fn route_ordinary_escapes(
    board: &PhysicalBoard,
    fanout_options: &FanoutOptions,
    nets: &IndexSet<String>,
    prefix: (&[TrackSegment], &[Via]),
    keepouts: &[copper_physical::CopperKeepout],
) -> Result<(Vec<TrackSegment>, Vec<Via>, Vec<copper_physical::CopperKeepout>, Vec<copper_pcb::board::PadReference>)> {
    let mut frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    frame.tracks.extend_from_slice(prefix.0);
    frame.vias.extend_from_slice(prefix.1);
    frame.copper_keepouts.extend(keepouts.iter().cloned());
    let escapes = crate::fanout::route_fanout(&frame, Some(fanout_options), Some(nets))?;
    let tracks = added_elements(&escapes.board.tracks, &frame.tracks);
    let vias = added_elements(&escapes.board.vias, &frame.vias);
    let reserved = copper_keepouts(board, &tracks, &vias, "escape-reserve", Some(&frame));
    Ok((tracks, vias, reserved, escapes.pending_pads))
}

/// `_plan_ordinary_escapes`: escapes for crowded ordinary pins, and the
/// keepouts that reserve them, or None. `only_nets` limits the nets planned;
/// `event` names the closing progress event.
#[allow(clippy::too_many_arguments)]
fn plan_ordinary_escapes(
    board: &PhysicalBoard,
    fanout_options: &FanoutOptions,
    on_progress: &mut Option<ProgressCallback<'_>>,
    prefix: (&[TrackSegment], &[Via]),
    keepouts: &[copper_physical::CopperKeepout],
    only_nets: Option<&IndexSet<String>>,
    exclude: &[String],
    event: &str,
) -> Result<Option<Reserved>> {
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let mut supply: IndexSet<&str> = board.zones.iter().map(|z| z.net.as_str()).collect();
    for link in &board.decoupling_links {
        supply.insert(&link.net);
        supply.insert(&link.return_net);
    }
    let ordinary: IndexSet<String> = board
        .nets
        .iter()
        .filter(|net| {
            net.pads.len() >= 2
                && !supply.contains(net.name.as_str())
                && rules.get(net.name.as_str()).is_none_or(|rule| rule.kind == RouteKind::General)
                && !exclude.contains(&net.name)
                && only_nets.is_none_or(|only| only.contains(&net.name))
        })
        .map(|net| net.name.clone())
        .collect();
    if ordinary.is_empty() {
        return Ok(None);
    }
    emit(on_progress, "ordinary_escapes", "started", vec![("nets", int(ordinary.len() as i64))]);
    match route_ordinary_escapes(board, fanout_options, &ordinary, prefix, keepouts) {
        Err(Error::Value(message)) => {
            emit(on_progress, "ordinary_escapes", event, vec![("skipped", text(&message))]);
            Ok(None)
        }
        Err(error) => Err(error),
        Ok((tracks, vias, reserved, pending)) => {
            let mut nets: Vec<String> =
                tracks.iter().map(|t| t.net.clone()).chain(vias.iter().map(|v| v.net.clone())).collect();
            nets.sort();
            nets.dedup();
            emit(
                on_progress,
                "ordinary_escapes",
                event,
                vec![
                    ("vias", int(vias.len() as i64)),
                    ("tracks", int(tracks.len() as i64)),
                    ("pending", int(pending.len() as i64)),
                    ("nets", Value::Array(nets.iter().map(|n| text(n)).collect())),
                ],
            );
            Ok(Some((tracks, vias, reserved)))
        }
    }
}

/// `_release_reservations`: escape reservations outside the pad boxes of
/// `nets` (+margin).
fn release_reservations(
    board: &PhysicalBoard,
    keepouts: &[copper_physical::CopperKeepout],
    nets: &IndexSet<String>,
    margin_nm: i64,
    pads: &[&copper_pcb::board::PadReference],
) -> Vec<copper_physical::CopperKeepout> {
    let poses: IndexMap<&str, _> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let place = |pad: &copper_pcb::board::PadReference| {
        let pose = poses[pad.component.as_str()];
        let land = board.footprints[&pose.footprint].pads.iter().find(|l| l.number == pad.pad).expect("land");
        copper_pcb::transform::transformed_local_point(pose, land.position)
    };
    let mut groups: Vec<Vec<Point>> =
        board.nets.iter().filter(|n| nets.contains(&n.name)).map(|net| net.pads.iter().map(place).collect()).collect();
    groups.extend(pads.iter().map(|pad| vec![place(pad)]));
    let mut boxes = Vec::new();
    for points in groups {
        if !points.is_empty() {
            boxes.push((
                points.iter().map(|p| p.x_nm).min().expect("points") - margin_nm,
                points.iter().map(|p| p.y_nm).min().expect("points") - margin_nm,
                points.iter().map(|p| p.x_nm).max().expect("points") + margin_nm,
                points.iter().map(|p| p.y_nm).max().expect("points") + margin_nm,
            ));
        }
    }
    let inside = |keepout: &copper_physical::CopperKeepout| {
        let count = keepout.vertices.len() as i128;
        let cx = crate::routing_lattice::true_div(keepout.vertices.iter().map(|v| i128::from(v.x_nm)).sum(), count);
        let cy = crate::routing_lattice::true_div(keepout.vertices.iter().map(|v| i128::from(v.y_nm)).sum(), count);
        boxes
            .iter()
            .any(|(x0, y0, x1, y1)| *x0 as f64 <= cx && cx <= *x1 as f64 && *y0 as f64 <= cy && cy <= *y1 as f64)
    };
    keepouts.iter().filter(|k| !inside(k)).cloned().collect()
}

/// `track_containment.track_is_contained`.
pub(crate) fn track_is_contained(candidate: &TrackSegment, existing: &[TrackSegment]) -> bool {
    existing.iter().any(|held| {
        held.net == candidate.net
            && held.layer == candidate.layer
            && held.width_nm == candidate.width_nm
            && copper_physical::geometry::point_on_segment(candidate.start, held.start, held.end)
            && copper_physical::geometry::point_on_segment(candidate.end, held.start, held.end)
    })
}

/// `_join_reserved_copper`: drop a stage's reservation keepouts and add the
/// copper they held, each piece checked exactly.
fn join_reserved_copper(
    source: &PhysicalBoard,
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    keepouts: &[copper_physical::CopperKeepout],
) -> Result<PhysicalBoard> {
    let (mut base, _) = strip_generated_keepouts(source, board, keepouts)?;
    let (kept_tracks, kept_vias) = {
        let mut index = crate::routing_clearance::RoutingClearanceIndex::new(&base)?;
        let (mut kept_tracks, mut kept_vias) = (Vec::new(), Vec::new());
        let mut present: IndexSet<(String, Point, CopperLayer, CopperLayer)> =
            base.vias.iter().map(|v| (v.net.clone(), v.position, v.from_layer, v.to_layer)).collect();
        for via in vias {
            let key = (via.net.clone(), via.position, via.from_layer, via.to_layer);
            if present.contains(&key) {
                continue;
            }
            if index.can_via(
                &via.net,
                via.position,
                via.size_nm,
                via.from_layer,
                via.to_layer,
                Some(via.drill_nm),
                false,
                None,
            )? {
                index.add_via(via.clone(), false, None)?;
                kept_vias.push(via.clone());
                present.insert(key);
            }
        }
        for track in tracks {
            if track_is_contained(track, &base.tracks) {
                continue;
            }
            if index.can_track(&track.net, track.start, track.end, track.width_nm, track.layer)? {
                index.add_track(track.clone(), false, None)?;
                kept_tracks.push(track.clone());
            }
        }
        (kept_tracks, kept_vias)
    };
    base.tracks.extend(kept_tracks);
    base.vias.extend(kept_vias);
    Ok(base)
}

/// `_release_escape_masks`: drop the temporary escape keepouts before
/// ordinary routing; with `planned_escapes_as_copper` the promised exits join
/// the ordinary stage's board as copper.
pub(crate) fn release_escape_masks(
    source: &PhysicalBoard,
    reserved: Option<&Reserved>,
    critical: &mut CriticalRoutingResult,
    plane_stitch: &mut Option<PlaneStitchResult>,
    pre_detailed: PhysicalBoard,
    unified_options: &UnifiedRouterOptions,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<PhysicalBoard> {
    let Some((tracks, vias, keepouts)) = reserved else { return Ok(pre_detailed) };
    critical.board = strip_generated_keepouts(source, &critical.board, keepouts)?.0;
    if let Some(stitch) = plane_stitch.as_mut() {
        stitch.board = strip_generated_keepouts(source, &stitch.board, keepouts)?.0;
    }
    let mut pre_detailed = strip_generated_keepouts(source, &pre_detailed, keepouts)?.0;
    if unified_options.planned_escapes_as_copper && (!tracks.is_empty() || !vias.is_empty()) {
        let joined = join_reserved_copper(source, &pre_detailed, tracks, vias, &[])?;
        let kept_tracks = &joined.tracks[pre_detailed.tracks.len()..];
        let kept_vias = &joined.vias[pre_detailed.vias.len()..];
        let present: IndexSet<(&str, Point, CopperLayer, CopperLayer)> =
            pre_detailed.vias.iter().map(|v| (v.net.as_str(), v.position, v.from_layer, v.to_layer)).collect();
        let reused_vias: Vec<&Via> =
            vias.iter().filter(|v| present.contains(&(v.net.as_str(), v.position, v.from_layer, v.to_layer))).collect();
        let reused_tracks: Vec<&TrackSegment> =
            tracks.iter().filter(|t| track_is_contained(t, &pre_detailed.tracks)).collect();
        let dropped_vias: Vec<&Via> =
            vias.iter().filter(|v| !kept_vias.contains(v) && !reused_vias.contains(v)).collect();
        let dropped_tracks: Vec<&TrackSegment> =
            tracks.iter().filter(|t| !kept_tracks.contains(t) && !reused_tracks.contains(t)).collect();
        let mut dropped_nets: Vec<String> =
            dropped_tracks.iter().map(|t| t.net.clone()).chain(dropped_vias.iter().map(|v| v.net.clone())).collect();
        dropped_nets.sort();
        dropped_nets.dedup();
        emit(
            on_progress,
            "ordinary_escapes",
            "joined",
            vec![
                ("tracks", int(kept_tracks.len() as i64)),
                ("vias", int(kept_vias.len() as i64)),
                ("reused_tracks", int(reused_tracks.len() as i64)),
                ("reused_vias", int(reused_vias.len() as i64)),
                ("dropped_tracks", int(dropped_tracks.len() as i64)),
                ("dropped_vias", int(dropped_vias.len() as i64)),
                ("dropped_nets", Value::Array(dropped_nets.iter().map(|n| text(n)).collect())),
            ],
        );
        pre_detailed = joined;
    }
    Ok(pre_detailed)
}

// ---------------------------------------------------------------------------
// Bypass reservations

/// `held`: bypass copper routed before critical nets, the keepouts that hold
/// it, the escape reservations it released, and its admitted witnesses.
#[derive(Clone, Debug)]
struct Held {
    tracks: Vec<TrackSegment>,
    vias: Vec<Via>,
    keepouts: Vec<copper_physical::CopperKeepout>,
    released: Vec<copper_physical::CopperKeepout>,
    witnesses: Vec<crate::decoupling_paths::DecouplingCrossLayerWitness>,
    /// Immutable producer contexts; replacing current witnesses never replaces these.
    provenance: Option<HeldProvenance>,
}

#[derive(Clone, Debug)]
struct HeldProducerBatch {
    raw: PhysicalBoard,
    temporary_keepouts: Vec<copper_physical::CopperKeepout>,
    tracks: std::ops::Range<usize>,
    vias: std::ops::Range<usize>,
    witnesses: Vec<crate::decoupling_paths::DecouplingCrossLayerWitness>,
}

#[derive(Clone, Debug)]
struct HeldProvenance {
    source: PhysicalBoard,
    /// This mode admits ordinary launches and lattice sites before the first
    /// bypass candidate. They remain prefix copper, never generated Held.
    complete_prefix: Option<crate::fixed_prefix::AdmittedFixedPrefix>,
    prefix_tracks: Vec<TrackSegment>,
    prefix_vias: Vec<Via>,
    batches: Vec<HeldProducerBatch>,
    rejected: Option<String>,
}

impl HeldProvenance {
    fn check_context(&self, raw: &PhysicalBoard, temporary: &[copper_physical::CopperKeepout]) -> Result<()> {
        let baseline =
            copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(self.source.clone())?)?;
        let mut intent = crate::fixed_prefix::strip_declared_temporary_keepouts(&baseline, raw, temporary)?;
        intent.tracks = baseline.tracks.clone();
        intent.vias = baseline.vias.clone();
        if intent != baseline {
            return Err(Error::Value("Held producer changed its exact clean source contract".to_owned()));
        }
        Ok(())
    }

    fn record(
        &mut self,
        input: &PhysicalBoard,
        result: &crate::decoupling::DecouplingRouteResult,
        temporary: &[copper_physical::CopperKeepout],
    ) -> Result<()> {
        let batch = HeldProducerBatch {
            raw: result.board.clone(),
            temporary_keepouts: temporary.to_vec(),
            tracks: input.tracks.len()..result.board.tracks.len(),
            vias: input.vias.len()..result.board.vias.len(),
            witnesses: result.admitted_path_witnesses.clone(),
        };
        // Capture before any caller rewrites masks. The complete result must
        // be this exact input followed by exactly the declared new copper.
        let checked = (|| -> Result<()> {
            let mut intent = result.board.clone();
            intent.tracks = input.tracks.clone();
            intent.vias = input.vias.clone();
            let tracks: Vec<_> = input.tracks.iter().chain(&result.created_tracks).cloned().collect();
            let vias: Vec<_> = input.vias.iter().chain(&result.created_vias).cloned().collect();
            if intent != *input || result.board.tracks != tracks || result.board.vias != vias {
                return Err(Error::Value(
                    "Held producer output differs from its input plus declared occurrences".to_owned(),
                ));
            }
            self.check_context(&batch.raw, &batch.temporary_keepouts)?;
            let (_, reason) = crate::decoupling_paths::refresh_crosslayer_witnesses(
                &batch.raw,
                &batch.witnesses,
                None,
                &IndexSet::new(),
            )?;
            if let Some(reason) = reason {
                return Err(Error::Value(format!("invalid original Held producer witness: {reason}")));
            }
            Ok(())
        })();
        self.batches.push(batch);
        if let Err(error) = &checked {
            self.rejected.get_or_insert_with(|| error.to_string());
        }
        checked
    }

    fn validate(
        &self,
        source: &PhysicalBoard,
        held_tracks: &[TrackSegment],
        held_vias: &[Via],
    ) -> Result<Vec<crate::decoupling_paths::DecouplingCrossLayerWitness>> {
        if source != &self.source {
            return Err(Error::Value("Held producer exact clean source changed".to_owned()));
        }
        if let Some(reason) = &self.rejected {
            return Err(Error::Value(format!("Held producer provenance was not admitted: {reason}")));
        }
        let (mut tracks, mut vias) = match &self.complete_prefix {
            Some(prefix) => {
                let fresh = prefix.revalidate_for(source)?;
                (fresh.board().tracks.clone(), fresh.board().vias.clone())
            }
            None => {
                let baseline = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(
                    source.clone(),
                )?)?;
                let mut tracks = baseline.tracks;
                tracks.extend(self.prefix_tracks.iter().cloned());
                let mut vias = baseline.vias;
                vias.extend(self.prefix_vias.iter().cloned());
                (tracks, vias)
            }
        };
        let (mut generated_tracks, mut generated_vias, mut originals) = (Vec::new(), Vec::new(), Vec::new());
        for batch in &self.batches {
            self.check_context(&batch.raw, &batch.temporary_keepouts)?;
            if batch.tracks.start != tracks.len()
                || batch.vias.start != vias.len()
                || batch.tracks.end != batch.raw.tracks.len()
                || batch.vias.end != batch.raw.vias.len()
                || batch.raw.tracks.get(..batch.tracks.start) != Some(tracks.as_slice())
                || batch.raw.vias.get(..batch.vias.start) != Some(vias.as_slice())
            {
                return Err(Error::Value("Held producer ordered source/generated prefix changed".to_owned()));
            }
            let (fresh, reason) = crate::decoupling_paths::refresh_crosslayer_witnesses(
                &batch.raw,
                &batch.witnesses,
                None,
                &IndexSet::new(),
            )?;
            if let Some(reason) = reason {
                return Err(Error::Value(format!("invalid original Held producer witness: {reason}")));
            }
            originals.extend(fresh);
            let more_tracks = &batch.raw.tracks[batch.tracks.clone()];
            let more_vias = &batch.raw.vias[batch.vias.clone()];
            generated_tracks.extend_from_slice(more_tracks);
            generated_vias.extend_from_slice(more_vias);
            tracks.extend_from_slice(more_tracks);
            vias.extend_from_slice(more_vias);
        }
        if generated_tracks != held_tracks || generated_vias != held_vias {
            return Err(Error::Value("Held ownership differs from exact producer-generated occurrences".to_owned()));
        }
        Ok(originals)
    }
}

/// `_link_name`.
fn link_name(link: &copper_pcb::board::DecouplingLink) -> String {
    format!("{}.{}->{}.{}", link.capacitor.component, link.capacitor.pad, link.target.component, link.target.pad)
}

fn pending_links(pending: &[(copper_pcb::board::DecouplingLink, String)]) -> Value {
    Value::Array(
        pending
            .iter()
            .map(|(link, reason)| {
                Value::Object(
                    [("link", text(&link_name(link))), ("reason", text(reason))]
                        .into_iter()
                        .map(|(k, v)| (k.to_owned(), v))
                        .collect(),
                )
            })
            .collect(),
    )
}

/// `_reservation_pads`: the pads of `net` that the reserved escape `tracks`
/// leave from.
fn reservation_pads(
    board: &PhysicalBoard,
    net: &str,
    tracks: &[TrackSegment],
) -> Result<Vec<copper_pcb::board::PadReference>> {
    let ends: IndexSet<Point> = tracks.iter().filter(|t| t.net == net).flat_map(|t| [t.start, t.end]).collect();
    let mut pads = Vec::new();
    for item in board.nets.iter().filter(|n| n.name == net) {
        for pad in &item.pads {
            let pose = board.placements.iter().find(|p| p.reference == pad.component).expect("a placed pad");
            if ends.contains(&copper_pcb::transform::transformed_pad_position(board, pose, &pad.pad)?) {
                pads.push(pad.clone());
            }
        }
    }
    Ok(pads)
}

/// `_legal_reservation_nets`: the `nets` whose complete escape reservation is
/// legal beside the bypass copper, without costing it a proxy keepout.
fn legal_reservation_nets(
    board: &PhysicalBoard,
    base: &PhysicalBoard,
    bypass: (&[TrackSegment], &[Via]),
    tracks: &[TrackSegment],
    vias: &[Via],
    nets: &IndexSet<String>,
) -> Result<IndexSet<String>> {
    let mut fixed = base.clone();
    fixed.tracks.extend_from_slice(bypass.0);
    fixed.vias.extend_from_slice(bypass.1);
    let index = crate::routing_clearance::RoutingClearanceIndex::new(&fixed)?;
    let mut legal = IndexSet::new();
    'nets: for net in nets {
        let own_tracks: Vec<TrackSegment> = tracks.iter().filter(|t| t.net == *net).cloned().collect();
        let own_vias: Vec<Via> = vias.iter().filter(|v| v.net == *net).cloned().collect();
        for t in &own_tracks {
            if !index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)? {
                continue 'nets;
            }
        }
        for v in &own_vias {
            let rule = board.net_routing_rules.iter().find(|r| r.net == v.net);
            if !index.can_via(&v.net, v.position, v.size_nm, v.from_layer, v.to_layer, Some(v.drill_nm), false, None)?
                || !crate::return_paths::existing_returns_permit(&fixed, rule, v)
            {
                continue 'nets;
            }
        }
        if copper_keepouts(board, &own_tracks, &own_vias, "check", Some(&fixed)).len()
            < copper_keepouts(board, &own_tracks, &own_vias, "check", Some(base)).len()
        {
            continue;
        }
        legal.insert(net.clone());
    }
    Ok(legal)
}

fn pad_name(pad: &copper_pcb::board::PadReference) -> String {
    format!("{}.{}", pad.component, pad.pad)
}

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// `_plan_decouplers`: bypass copper routed before critical nets on the macro
/// frame (respecting the escape reservations), held as keepouts meanwhile.
///
/// A link the reservations block is tried again, one link at a time, with the
/// reservations within 1.5 mm of its pads lifted. A reservation the new copper
/// leaves legal stays bound; every ball whose reservation the copper takes is
/// planned again beside it (`fanout_options`), or the link stays pending,
/// naming the ball. Without `fanout_options` the taken reservations are
/// released for the caller.
fn plan_decouplers(
    board: &PhysicalBoard,
    reserved: Option<Reserved>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    prefix: (&[TrackSegment], &[Via]),
    extra_keepouts: &[copper_physical::CopperKeepout],
    fanout_options: Option<&FanoutOptions>,
) -> Result<(Held, Option<Reserved>)> {
    plan_decouplers_with_provenance(board, reserved, on_progress, prefix, extra_keepouts, fanout_options, false)
}

/// One bounded bypass batch on the actual ordinary/lattice prefix. No proxy
/// self-masks or flow-level release/displacement retries enter this mode.
fn plan_retained_decouplers(
    board: &PhysicalBoard,
    reserved: Option<Reserved>,
    lattice: Option<&Lattice>,
    halos: &[copper_physical::CopperKeepout],
    fanout_options: Option<&FanoutOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(Held, Option<Reserved>)> {
    emit(
        on_progress,
        "bypass_reservation",
        "started",
        vec![("links", int(board.decoupling_links.len() as i64)), ("mode", text("complete_physical_prefix"))],
    );
    let (prefix, _) = match retained_critical_inputs(board, reserved.as_ref(), lattice, None) {
        Ok(inputs) => inputs,
        Err(Error::Value(reason)) => {
            // A failed optional prefix must not route bypasses through the
            // proposed launches or stop unrelated outside routing.
            let pending: Vec<_> = board
                .decoupling_links
                .iter()
                .map(|link| (link.clone(), format!("physical prefix not admitted: {reason}")))
                .collect();
            emit(
                on_progress,
                "bypass_reservation",
                "finished",
                vec![
                    ("mode", text("complete_physical_prefix_not_admitted")),
                    ("reason", text(&reason)),
                    ("tracks", int(0)),
                    ("vias", int(0)),
                    ("pending", int(pending.len() as i64)),
                    ("pending_links", pending_links(&pending)),
                ],
            );
            return Ok((
                Held {
                    tracks: Vec::new(),
                    vias: Vec::new(),
                    keepouts: Vec::new(),
                    released: Vec::new(),
                    witnesses: Vec::new(),
                    provenance: None,
                },
                reserved,
            ));
        }
        Err(error) => return Err(error),
    };
    let mut frame = prefix.board().clone();
    frame.copper_keepouts.extend(halos.iter().cloned());
    let result = crate::decoupling::route_decouplers(&frame, None, fanout_options, None, None)?;
    let mut provenance = HeldProvenance {
        source: board.clone(),
        complete_prefix: Some(prefix.clone()),
        prefix_tracks: Vec::new(),
        prefix_vias: Vec::new(),
        batches: Vec::new(),
        rejected: None,
    };
    let reason = provenance.record(&frame, &result, halos).err();
    emit(
        on_progress,
        "bypass_reservation",
        "producer_context_captured",
        vec![
            ("batch", int(1)),
            ("mode", text("complete_physical_prefix")),
            ("original_witnesses", int(result.admitted_path_witnesses.len() as i64)),
            ("reason", reason.as_ref().map_or(Value::Null, |reason| text(&reason.to_string()))),
        ],
    );
    let keepouts =
        copper_keepouts(board, &result.created_tracks, &result.created_vias, "bypass-reserve", Some(prefix.board()));
    emit(
        on_progress,
        "bypass_reservation",
        "finished",
        vec![
            ("mode", text("complete_physical_prefix")),
            ("tracks", int(result.created_tracks.len() as i64)),
            ("vias", int(result.created_vias.len() as i64)),
            ("pending", int(result.pending.len() as i64)),
            ("pending_links", pending_links(&result.pending)),
            ("released", int(0)),
            ("flow_displacement_retries", int(0)),
            ("boundary_channel_verification", text("not_performed")),
        ],
    );
    Ok((
        Held {
            tracks: result.created_tracks,
            vias: result.created_vias,
            keepouts,
            released: Vec::new(),
            witnesses: result.admitted_path_witnesses,
            provenance: Some(provenance),
        },
        reserved,
    ))
}

#[allow(clippy::too_many_arguments)]
fn plan_decouplers_with_provenance(
    board: &PhysicalBoard,
    reserved: Option<Reserved>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    prefix: (&[TrackSegment], &[Via]),
    extra_keepouts: &[copper_physical::CopperKeepout],
    fanout_options: Option<&FanoutOptions>,
    retain_provenance: bool,
) -> Result<(Held, Option<Reserved>)> {
    use copper_pcb::board::DecouplingLink;
    emit(on_progress, "bypass_reservation", "started", vec![("links", int(board.decoupling_links.len() as i64))]);
    let had_reserved = reserved.is_some();
    let (tracks0, vias0, keepouts) = reserved.unwrap_or_default();
    let mut base = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    base.tracks.extend_from_slice(prefix.0);
    base.vias.extend_from_slice(prefix.1);
    // `extra_keepouts` (macro port halos) bind every attempt and are never
    // released for a pending link, unlike the escape reservations.
    let mut frame = base.clone();
    frame.copper_keepouts.extend(extra_keepouts.iter().cloned());
    frame.copper_keepouts.extend(keepouts.iter().cloned());
    let mut producer_masks = extra_keepouts.to_vec();
    producer_masks.extend_from_slice(&keepouts);
    let mut provenance = retain_provenance.then(|| HeldProvenance {
        source: board.clone(),
        complete_prefix: None,
        prefix_tracks: prefix.0.to_vec(),
        prefix_vias: prefix.1.to_vec(),
        batches: Vec::new(),
        rejected: None,
    });
    let result = crate::decoupling::route_decouplers(&frame, None, fanout_options, None, None)?;
    if let Some(provenance) = provenance.as_mut() {
        let reason = provenance.record(&frame, &result, &producer_masks).err();
        emit(
            on_progress,
            "bypass_reservation",
            "producer_context_captured",
            vec![
                ("batch", int(provenance.batches.len() as i64)),
                ("original_witnesses", int(result.admitted_path_witnesses.len() as i64)),
                ("reason", reason.as_ref().map_or(Value::Null, |reason| text(&reason.to_string()))),
            ],
        );
    }
    let mut current = result.board;
    let mut pending: Vec<(DecouplingLink, String)> = result.pending;
    let mut witnesses = result.admitted_path_witnesses;
    let (mut res_tracks, mut res_vias, mut res_keepouts) = (tracks0, vias0, keepouts.clone());
    if had_reserved {
        observe_reserved_geometry(
            board,
            &current,
            &(res_tracks.clone(), res_vias.clone(), res_keepouts.clone()),
            (&[], &[]),
            None,
            None,
            "after_initial_decoupler_batch",
            None,
            on_progress,
        );
    }
    let (mut released, mut swapped, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
    let mut owners: IndexMap<String, String> = IndexMap::new();
    // Reservation keepouts of the current escape copper beside all bypass copper.
    let rebuild = |owners: &mut IndexMap<String, String>,
                   res_tracks: &[TrackSegment],
                   res_vias: &[Via],
                   fixed: (&[TrackSegment], &[Via])| {
        let mut frame = base.clone();
        frame.tracks.extend_from_slice(fixed.0);
        frame.vias.extend_from_slice(fixed.1);
        let fresh = copper_keepouts(board, res_tracks, res_vias, "escape-reserve", Some(&frame));
        owners.clear();
        owners.extend(res_tracks.iter().enumerate().map(|(i, t)| (format!("escape-reserve-track-{i}"), t.net.clone())));
        owners.extend(res_vias.iter().enumerate().map(|(i, v)| (format!("escape-reserve-via-{i}"), v.net.clone())));
        fresh
    };
    if !keepouts.is_empty() {
        rebuild(&mut owners, &res_tracks, &res_vias, (&[], &[]));
    }
    for (link, reason) in pending.clone() {
        if res_keepouts.is_empty() {
            break;
        }
        if reason == "native_plane_return_pending" {
            continue;
        }
        let pads = [&link.capacitor, &link.target, &link.return_pad];
        let kept = release_reservations(board, &res_keepouts, &IndexSet::new(), 1_500_000, &pads);
        let kept_ids: IndexSet<&String> = kept.iter().map(|k| &k.id).collect();
        let candidates: IndexSet<String> =
            res_keepouts.iter().map(|k| k.id.clone()).filter(|id| !kept_ids.contains(id)).collect();
        if candidates.is_empty() {
            continue;
        }
        let mut retry_frame = current.clone();
        retry_frame.copper_keepouts.retain(|k| !candidates.contains(&k.id));
        let retry_masks: Vec<_> = producer_masks.iter().filter(|k| !candidates.contains(&k.id)).cloned().collect();
        let only: IndexSet<DecouplingLink> = [link.clone()].into_iter().collect();
        let retry = crate::decoupling::route_decouplers(&retry_frame, None, fanout_options, Some(&only), None)?;
        if retry.pending.iter().any(|(item, _)| *item == link) {
            continue;
        }
        let bypass_tracks = added_elements(&retry.board.tracks, &base.tracks);
        let bypass_vias = added_elements(&retry.board.vias, &base.vias);
        // Exact ownership: only the groups the new copper makes illegal are taken.
        let candidate_nets: IndexSet<String> = candidates.iter().filter_map(|k| owners.get(k).cloned()).collect();
        let bypass = (bypass_tracks.as_slice(), bypass_vias.as_slice());
        let legal = legal_reservation_nets(board, &base, bypass, &res_tracks, &res_vias, &candidate_nets)?;
        let mut taken: Vec<String> = candidate_nets.iter().filter(|n| !legal.contains(*n)).cloned().collect();
        taken.sort();
        let mut taken_pads: Vec<String> = Vec::new();
        for net in &taken {
            taken_pads.extend(reservation_pads(board, net, &res_tracks)?.iter().map(pad_name));
        }
        taken_pads.sort();
        taken_pads.dedup();
        let taken_set: IndexSet<String> = taken.iter().cloned().collect();
        let is_taken = |id: &str| owners.get(id).is_some_and(|net| taken_set.contains(net));
        if let Some(options) = fanout_options.filter(|_| !taken.is_empty()) {
            // The swap: every ball the copper takes is planned again beside it.
            let mut remaining: Vec<copper_physical::CopperKeepout> =
                res_keepouts.iter().filter(|k| !is_taken(&k.id)).cloned().collect();
            remaining.extend(extra_keepouts.iter().cloned());
            // The macro source does not contain the original fixed prefix.
            // Keep it in the disposable swap frame before the new bypass
            // contribution, preserving ordered occurrences without deduplication.
            let mut swap_tracks = prefix.0.to_vec();
            swap_tracks.extend_from_slice(bypass.0);
            let mut swap_vias = prefix.1.to_vec();
            swap_vias.extend_from_slice(bypass.1);
            let (again_tracks, again_vias, lost) =
                match route_ordinary_escapes(board, options, &taken_set, (&swap_tracks, &swap_vias), &remaining) {
                    Ok((tracks, vias, _, pending)) => (tracks, vias, pending.iter().map(pad_name).collect()),
                    Err(Error::Value(message)) => (Vec::new(), Vec::new(), vec![message]),
                    Err(error) => return Err(error),
                };
            if !lost.is_empty() {
                let balls = Value::Array(taken_pads.iter().map(|p| text(p)).collect());
                let lost = Value::Array(lost.iter().map(|p: &String| text(p)).collect());
                rejected.push(record(vec![("link", text(&link_name(&link))), ("balls", balls), ("pending", lost)]));
                let note = taken_pads.join(", ");
                for (item, why) in pending.iter_mut() {
                    if *item == link {
                        *why = format!("{why}; swap rejected: {note} would lose their site");
                    }
                }
                continue;
            }
            res_tracks.retain(|t| !taken_set.contains(&t.net));
            res_tracks.extend(again_tracks);
            res_vias.retain(|v| !taken_set.contains(&v.net));
            res_vias.extend(again_vias);
            let balls = Value::Array(taken_pads.iter().map(|p| text(p)).collect());
            swapped.push(record(vec![("link", text(&link_name(&link))), ("balls", balls)]));
        } else if !taken.is_empty() {
            released.extend(res_keepouts.iter().filter(|k| is_taken(&k.id)).cloned());
            res_tracks.retain(|t| !taken_set.contains(&t.net));
            res_vias.retain(|v| !taken_set.contains(&v.net));
        }
        if let Some(provenance) = provenance.as_mut() {
            let reason = provenance.record(&retry_frame, &retry, &retry_masks).err();
            emit(
                on_progress,
                "bypass_reservation",
                "producer_context_captured",
                vec![
                    ("batch", int(provenance.batches.len() as i64)),
                    ("original_witnesses", int(retry.admitted_path_witnesses.len() as i64)),
                    ("reason", reason.as_ref().map_or(Value::Null, |reason| text(&reason.to_string()))),
                ],
            );
        }
        res_keepouts = rebuild(&mut owners, &res_tracks, &res_vias, bypass);
        producer_masks = extra_keepouts.to_vec();
        producer_masks.extend_from_slice(&res_keepouts);
        current = retry.board;
        current.copper_keepouts = base.copper_keepouts.clone();
        current.copper_keepouts.extend(extra_keepouts.iter().cloned());
        current.copper_keepouts.extend(res_keepouts.iter().cloned());
        pending.retain(|(item, _)| *item != link);
        pending.extend(retry.pending.into_iter().filter(|(item, _)| *item != link));
        witnesses.extend(retry.admitted_path_witnesses);
        if had_reserved {
            observe_reserved_geometry(
                board,
                &current,
                &(res_tracks.clone(), res_vias.clone(), res_keepouts.clone()),
                (&[], &[]),
                None,
                None,
                "after_adopted_decoupler_retry",
                Some(&link_name(&link)),
                on_progress,
            );
        }
    }
    let tracks = added_elements(&current.tracks, &base.tracks);
    let vias = added_elements(&current.vias, &base.vias);
    emit(
        on_progress,
        "bypass_reservation",
        "finished",
        vec![
            ("tracks", int(tracks.len() as i64)),
            ("vias", int(vias.len() as i64)),
            ("pending", int(pending.len() as i64)),
            ("released", int(released.len() as i64)),
            ("swapped", Value::Array(swapped)),
            ("rejected", Value::Array(rejected)),
            ("pending_links", pending_links(&pending)),
        ],
    );
    let held_keepouts = copper_keepouts(board, &tracks, &vias, "bypass-reserve", Some(&base));
    let reserved = had_reserved.then_some((res_tracks, res_vias, res_keepouts));
    Ok((Held { tracks, vias, keepouts: held_keepouts, released, witnesses, provenance }, reserved))
}

/// `_refresh_bypass_witnesses`: the admitted witnesses verified again in the
/// keepout context critical routing sees; a failure keeps `held` and names why.
fn refresh_bypass_witnesses(
    board: &PhysicalBoard,
    reserved: Option<&Reserved>,
    held: Held,
) -> Result<(Held, Option<String>)> {
    if held.witnesses.is_empty() {
        return Ok((held, None));
    }
    let frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    let mut context = frame;
    context.tracks.extend(held.tracks.iter().cloned());
    context.vias.extend(held.vias.iter().cloned());
    context.copper_keepouts = board.copper_keepouts.clone();
    if let Some(reserved) = reserved {
        context.copper_keepouts.extend(reserved.2.iter().cloned());
    }
    let fresh = crate::routing_clearance::RoutingClearanceIndex::new(&context)?;
    let mut witnesses = Vec::new();
    for witness in &held.witnesses {
        let checked = crate::decoupling_paths::verify_crosslayer_decoupling(
            &context,
            &witness.link,
            Some(&witness.return_target),
            10_000,
            Some(&fresh),
        )?;
        match checked.witness {
            Some(witness) => witnesses.push(witness),
            None => return Ok((held, Some(checked.reason))),
        }
    }
    Ok((Held { witnesses, ..held }, None))
}

/// The outcome of [`revalidate_released_escapes`], reported as the
/// `ordinary_escapes`/`revalidated` progress event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Revalidation {
    pub(crate) restored_keepouts: usize,
    pub(crate) restored_nets: Vec<String>,
    pub(crate) unrestored_nets: Vec<String>,
    pub(crate) outcome: &'static str,
    pub(crate) reason: Option<String>,
}

impl Revalidation {
    fn fields(&self) -> Vec<(&'static str, Value)> {
        let names = |nets: &[String]| Value::Array(nets.iter().map(|n| text(n)).collect());
        let mut fields = vec![
            ("restored_keepouts", int(self.restored_keepouts as i64)),
            ("restored_nets", names(&self.restored_nets)),
            ("unrestored_nets", names(&self.unrestored_nets)),
            ("outcome", text(self.outcome)),
        ];
        if let Some(reason) = &self.reason {
            fields.push(("reason", text(reason)));
        }
        fields
    }
}

/// A fresh verification of one admitted bypass witness in a keepout context
/// (`verify_crosslayer_decoupling`; replaceable in tests).
type WitnessCheck<'v> = &'v mut dyn FnMut(
    &PhysicalBoard,
    &crate::decoupling_paths::DecouplingCrossLayerWitness,
    &crate::routing_clearance::RoutingClearanceIndex<'_>,
) -> Result<crate::decoupling_paths::DecouplingPathResult>;

/// `_revalidate_released_escapes`: restore collateral reservations only as
/// complete, freshly legal net groups.
///
/// Bypass planning releases proximity boxes, not proven conflicting exits; a
/// surviving complete launch must still constrain the critical routing that
/// follows. No copper is changed. The restored keepout context gets a fresh
/// bypass ledger (every witness verified again in it) and ordinary strict
/// prefix admission; any failed check rolls the whole restoration back and
/// returns `held` with its original witnesses.
fn revalidate_released_escapes(
    board: &PhysicalBoard,
    reserved: Reserved,
    held: Held,
) -> Result<(Reserved, Held, Revalidation)> {
    let mut verify = |context: &PhysicalBoard,
                      witness: &crate::decoupling_paths::DecouplingCrossLayerWitness,
                      index: &crate::routing_clearance::RoutingClearanceIndex<'_>| {
        crate::decoupling_paths::verify_crosslayer_decoupling(
            context,
            &witness.link,
            Some(&witness.return_target),
            10_000,
            Some(index),
        )
    };
    revalidate_released_escapes_with(board, reserved, held, &mut verify)
}

fn revalidate_released_escapes_with(
    board: &PhysicalBoard,
    reserved: Reserved,
    held: Held,
    verify: WitnessCheck<'_>,
) -> Result<(Reserved, Held, Revalidation)> {
    let frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    let mut fixed = frame;
    fixed.tracks.extend(held.tracks.iter().cloned());
    fixed.vias.extend(held.vias.iter().cloned());
    let (tracks, vias, keepouts) = reserved;
    let owners: IndexMap<String, &str> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (format!("escape-reserve-track-{i}"), t.net.as_str()))
        .chain(vias.iter().enumerate().map(|(i, v)| (format!("escape-reserve-via-{i}"), v.net.as_str())))
        .collect();
    // A released keepout is matched whole: planning renumbers the surviving
    // reservations after a release, so an id alone may name another net's.
    let is_released = |k: &copper_physical::CopperKeepout| held.released.contains(k);
    let mut affected: Vec<String> = keepouts
        .iter()
        .filter(|k| is_released(k))
        .filter_map(|k| owners.get(&k.id).map(|net| (*net).to_owned()))
        .collect();
    affected.sort();
    affected.dedup();
    // Preserve the complete original group, not a legal via with a blocked stem.
    let mut legal: IndexSet<String> = IndexSet::new();
    {
        let index = crate::routing_clearance::RoutingClearanceIndex::new(&fixed)?;
        'nets: for net in &affected {
            for t in tracks.iter().filter(|t| t.net == *net) {
                if !index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)? {
                    continue 'nets;
                }
            }
            for v in vias.iter().filter(|v| v.net == *net) {
                let rule = board.net_routing_rules.iter().find(|r| r.net == v.net);
                if !index.can_via(
                    &v.net,
                    v.position,
                    v.size_nm,
                    v.from_layer,
                    v.to_layer,
                    Some(v.drill_nm),
                    false,
                    None,
                )? || !crate::return_paths::existing_returns_permit(&fixed, rule, v)
                {
                    continue 'nets;
                }
            }
            legal.insert(net.clone());
        }
    }
    // Proxy keepouts must themselves avoid the real immutable/bypass prefix.
    let safe: IndexSet<String> =
        copper_keepouts(board, &tracks, &vias, "escape-reserve", Some(&fixed)).into_iter().map(|k| k.id).collect();
    legal.retain(|net| {
        keepouts
            .iter()
            .filter(|k| is_released(k) && owners.get(&k.id).is_some_and(|owner| owner == net))
            .all(|k| safe.contains(&k.id))
    });
    let baseline: Vec<copper_physical::CopperKeepout> = keepouts.iter().filter(|k| !is_released(k)).cloned().collect();
    let kept: Vec<copper_physical::CopperKeepout> = keepouts
        .iter()
        .filter(|k| !is_released(k) || owners.get(&k.id).is_some_and(|net| legal.contains(*net)))
        .cloned()
        .collect();
    let count = kept.len() - baseline.len();
    let unrestored = Revalidation {
        restored_keepouts: 0,
        restored_nets: Vec::new(),
        unrestored_nets: affected.clone(),
        outcome: "no_complete_legal_group",
        reason: None,
    };
    if count == 0 {
        return Ok(((tracks, vias, baseline), held, unrestored));
    }
    let mut context = fixed;
    context.copper_keepouts = board.copper_keepouts.clone();
    context.copper_keepouts.extend(kept.iter().cloned());
    let mut witnesses = Vec::new();
    {
        let fresh = crate::routing_clearance::RoutingClearanceIndex::new(&context)?;
        for witness in &held.witnesses {
            let checked = verify(&context, witness, &fresh)?;
            match checked.witness {
                Some(witness) => witnesses.push(witness),
                None => {
                    let report = Revalidation {
                        outcome: "bypass_revalidation_failed",
                        reason: Some(checked.reason),
                        ..unrestored
                    };
                    return Ok(((tracks, vias, baseline), held, report));
                }
            }
        }
    }
    let mut source = board.clone();
    source.copper_keepouts = context.copper_keepouts.clone();
    let (_, reason) =
        crate::bypass_reservations::bypass_copper_reservations(&source, &held.tracks, &held.vias, &witnesses)?;
    if let Some(reason) = reason {
        let report = Revalidation { outcome: "prefix_admission_failed", reason: Some(reason), ..unrestored };
        return Ok(((tracks, vias, baseline), held, report));
    }
    let remaining: Vec<copper_physical::CopperKeepout> =
        held.released.iter().filter(|k| !kept.contains(k)).cloned().collect();
    let mut restored_nets: Vec<String> = legal.iter().cloned().collect();
    restored_nets.sort();
    let report = Revalidation {
        restored_keepouts: count,
        unrestored_nets: affected.iter().filter(|net| !legal.contains(*net)).cloned().collect(),
        restored_nets,
        outcome: "restored",
        reason: None,
    };
    let updated = Held { released: remaining, witnesses, ..held };
    Ok(((tracks, vias, kept), updated, report))
}

/// `_resite_escapes`: plan the exits of `nets` again around the bypass
/// copper that displaced them.
///
/// Revalidation restores only the groups still legal beside the bypass
/// copper; a ball whose dogbone site that copper took would otherwise be left
/// to the fanout stage with its cell centres taken. The new escapes plan with
/// the bypass copper as prefix and the remaining reservations as keepouts;
/// reservation ids stay index-stable.
#[allow(clippy::too_many_arguments)]
fn resite_escapes(
    board: &PhysicalBoard,
    fanout_options: &FanoutOptions,
    reserved: Reserved,
    held: &Held,
    prefix: (&[TrackSegment], &[Via]),
    halos: &[copper_physical::CopperKeepout],
    nets: &IndexSet<String>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<Reserved> {
    let mut prefix_tracks = prefix.0.to_vec();
    prefix_tracks.extend(held.tracks.iter().cloned());
    let mut prefix_vias = prefix.1.to_vec();
    prefix_vias.extend(held.vias.iter().cloned());
    let mut keepouts = reserved.2.clone();
    keepouts.extend(halos.iter().cloned());
    let again = plan_ordinary_escapes(
        board,
        fanout_options,
        on_progress,
        (&prefix_tracks, &prefix_vias),
        &keepouts,
        Some(nets),
        &[],
        "resited",
    )?;
    let Some((again_tracks, again_vias, _)) = again else {
        return Ok(reserved);
    };
    let (mut tracks, mut vias, _) = reserved;
    tracks.extend(again_tracks);
    vias.extend(again_vias);
    let mut fixed = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    fixed.tracks.extend(prefix_tracks);
    fixed.vias.extend(prefix_vias);
    let keepouts = copper_keepouts(board, &tracks, &vias, "escape-reserve", Some(&fixed))
        .into_iter()
        .filter(|k| !held.released.contains(k))
        .collect();
    Ok((tracks, vias, keepouts))
}

/// The late bypass pass of `_complete_critical_stage`: the links left pending
/// at planning get the stage's launch options beside the critical copper.
struct BypassCompletionStatus {
    pending: IndexSet<copper_pcb::board::DecouplingLink>,
    reasons: IndexMap<copper_pcb::board::DecouplingLink, String>,
    connected: IndexSet<copper_pcb::board::DecouplingLink>,
    unchecked: IndexMap<copper_pcb::board::DecouplingLink, String>,
}

/// Mirror the existing bypass router's ownership boundary. Skipped associations
/// are neither freshly connected nor eligible pending routes. Report the scope
/// instead of letting a later skipped retry look like an improvement.
fn unchecked_bypass_links(
    board: &PhysicalBoard,
    fanout_options: Option<&FanoutOptions>,
) -> IndexMap<copper_pcb::board::DecouplingLink, String> {
    let owned = crate::pair_terminals::macro_owned_pads(board);
    board
        .decoupling_links
        .iter()
        .filter_map(|link| {
            if fanout_options.is_some_and(|o| o.plane_return_nets.contains(&link.net)) {
                return None;
            }
            if owned.contains(&link.target) || owned.contains(&link.capacitor) {
                Some((link.clone(), "macro-owned association is outside generic bypass verification".to_owned()))
            } else if board
                .net_routing_rules
                .iter()
                .find(|r| r.net == link.net)
                .is_some_and(|r| r.kind != RouteKind::General)
            {
                Some((link.clone(), "critical topology association is outside generic bypass verification".to_owned()))
            } else {
                None
            }
        })
        .collect()
}

/// Rejection status belongs to the retained board. Use the existing nonrouting
/// feed/return witness contract; no candidate flags or new routing attempt.
fn retained_bypass_status(
    source: &PhysicalBoard,
    board: &PhysicalBoard,
    eligibility_source: &PhysicalBoard,
    reserved: Option<&Reserved>,
    fanout_options: Option<&FanoutOptions>,
) -> Result<BypassCompletionStatus> {
    let (frame, source_masks_present) =
        strip_generated_keepouts(source, board, reserved.map_or(&[][..], |r| r.2.as_slice()))?;
    let nets = eligibility_source.decoupling_links.iter().map(|l| l.net.clone()).collect();
    let verified = crate::bypass_links::verified_bypass_links(&frame, &nets, None, None, &[])?;
    let mut status = BypassCompletionStatus {
        pending: IndexSet::new(),
        reasons: IndexMap::new(),
        connected: IndexSet::new(),
        unchecked: unchecked_bypass_links(eligibility_source, fanout_options),
    };
    for link in &eligibility_source.decoupling_links {
        if status.unchecked.contains_key(link) {
            continue;
        }
        let plane_pending = fanout_options.is_some_and(|o| o.plane_return_nets.contains(&link.net));
        let budget = crate::decoupling_paths::budget_reason(&frame, &link.net);
        if source_masks_present && !plane_pending && budget.is_empty() && verified.contains(link) {
            status.connected.insert(link.clone());
        } else {
            status.pending.insert(link.clone());
            let reason = if !source_masks_present {
                "board is missing authored keepout occurrences"
            } else if plane_pending {
                "native_plane_return_pending"
            } else if !budget.is_empty() {
                budget
            } else {
                "board lacks a freshly verified bypass association"
            };
            status.reasons.insert(link.clone(), reason.to_owned());
        }
    }
    Ok(status)
}

/// Actual acceptance point for both existing late bypass attempts. Held bypass
/// and lattice copper are already physical, while promised ordinary escapes
/// are still unjoined. Fresh statuses share the incumbent's eligible link set.
/// Rejection leaves the entire incumbent object untouched.
#[allow(clippy::too_many_arguments)]
fn adopt_bypass_candidate(
    source: &PhysicalBoard,
    critical: &mut CriticalRoutingResult,
    reserved: Option<&Reserved>,
    fanout_options: Option<&FanoutOptions>,
    is_retry: bool,
    candidate: crate::decoupling::DecouplingRouteResult,
    restore_mask_context: bool,
) -> Result<(CriticalRetryCheck, BypassCompletionStatus)> {
    let before_status = retained_bypass_status(source, &critical.board, &critical.board, reserved, fanout_options)?;
    let mut after_status = retained_bypass_status(source, &candidate.board, &critical.board, reserved, fanout_options)?;
    let improved = after_status.pending.len() < before_status.pending.len();
    let mut checked = CriticalRetryCheck {
        improved,
        adopted: false,
        source_masks_present: None,
        baseline_invalid: Vec::new(),
        candidate_invalid: Vec::new(),
        baseline_invalid_dependencies: Vec::new(),
        candidate_invalid_dependencies: Vec::new(),
    };
    if !is_retry || improved {
        let empty = Reserved::default();
        let promised = reserved.unwrap_or(&empty);
        let before = invalid_reserved_escape_nets(source, &critical.board, promised, (&[], &[]), None, None)?;
        let after = invalid_reserved_escape_nets(source, &candidate.board, promised, (&[], &[]), None, None)?;
        checked.source_masks_present = Some(before.source_masks_present && after.source_masks_present);
        checked.baseline_invalid = before.invalid_escape_nets;
        checked.candidate_invalid = after.invalid_escape_nets;
        checked.baseline_invalid_dependencies = before.invalid_dependencies;
        checked.candidate_invalid_dependencies = after.invalid_dependencies;
        checked.adopted = checked.source_masks_present == Some(true)
            && checked.candidate_invalid.is_empty()
            && checked.candidate_invalid_dependencies.is_empty();
    }
    if !checked.adopted {
        return Ok((checked, before_status));
    }
    // Preserve useful candidate diagnostics only for independently verified
    // pending obligations. Partial only_links flags cannot erase old successes.
    for (link, reason) in &candidate.pending {
        if after_status.pending.contains(link) && !reason.is_empty() {
            after_status.reasons.insert(link.clone(), reason.clone());
        }
    }
    let masks = restore_mask_context.then(|| critical.board.copper_keepouts.clone());
    critical.board = candidate.board;
    if let Some(masks) = masks {
        critical.board.copper_keepouts = masks;
    }
    Ok((checked, after_status))
}
fn complete_bypasses(
    source: &PhysicalBoard,
    critical: &mut CriticalRoutingResult,
    reserved: Option<&Reserved>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    fanout_options: Option<&FanoutOptions>,
) -> Result<()> {
    let before: IndexSet<copper_pcb::board::DecouplingLink> = critical.board.decoupling_links.iter().cloned().collect();
    emit(on_progress, "bypass_completion", "started", vec![("links", int(before.len() as i64))]);
    let bypass = crate::decoupling::route_decouplers(&critical.board, None, fanout_options, None, None)?;
    let (first, mut status) = adopt_bypass_candidate(source, critical, reserved, fanout_options, false, bypass, false)?;
    emit_bypass_adoption(on_progress, "first_pass_checked", &first);
    if let Some(reserved) = reserved.filter(|_| !status.pending.is_empty()) {
        // A bypass the reservations blocked gets them released around its
        // own capacitor and target pad only, then one more attempt.
        let pads: Vec<&copper_pcb::board::PadReference> =
            status.pending.iter().flat_map(|l| [&l.capacitor, &l.target, &l.return_pad]).collect();
        let kept = release_reservations(&critical.board, &reserved.2, &IndexSet::new(), 500_000, &pads);
        if kept.len() < reserved.2.len() {
            let temporary: Vec<_> = reserved.2.iter().filter(|k| !kept.contains(k)).cloned().collect();
            let (released, _) = strip_generated_keepouts(source, &critical.board, &temporary)?;
            let retry =
                crate::decoupling::route_decouplers(&released, None, fanout_options, Some(&status.pending), None)?;
            let (checked, retained) =
                adopt_bypass_candidate(source, critical, Some(reserved), fanout_options, true, retry, true)?;
            status = retained;
            emit_bypass_adoption(on_progress, "retry_checked", &checked);
        }
    }
    let mut connected: Vec<String> = status.connected.iter().map(link_name).collect();
    connected.sort();
    let mut remaining: Vec<(copper_pcb::board::DecouplingLink, String)> =
        status.pending.iter().map(|link| (link.clone(), status.reasons[link].clone())).collect();
    remaining.sort_by_key(|(link, _)| link_name(link));
    emit(
        on_progress,
        "bypass_completion",
        "finished",
        vec![
            ("pending", int(status.pending.len() as i64)),
            ("connected", Value::Array(connected.iter().map(|n| text(n)).collect())),
            ("pending_links", pending_links(&remaining)),
            ("unchecked_links", pending_links(&status.unchecked.into_iter().collect::<Vec<_>>())),
        ],
    );
    Ok(())
}

fn emit_bypass_adoption(on_progress: &mut Option<ProgressCallback<'_>>, event: &str, checked: &CriticalRetryCheck) {
    emit(
        on_progress,
        "bypass_completion",
        event,
        vec![
            ("score_improved", Value::Bool(checked.improved)),
            ("adopted", Value::Bool(checked.adopted)),
            ("source_masks_present", checked.source_masks_present.map_or(Value::Null, Value::Bool)),
            ("baseline_invalid_escape_nets", Value::Array(checked.baseline_invalid.iter().map(|n| text(n)).collect())),
            (
                "invalid_candidate_escape_nets",
                Value::Array(checked.candidate_invalid.iter().map(|n| text(n)).collect()),
            ),
            (
                "baseline_invalid_dependencies",
                Value::Array(checked.baseline_invalid_dependencies.iter().map(|n| text(n)).collect()),
            ),
            (
                "invalid_candidate_dependencies",
                Value::Array(checked.candidate_invalid_dependencies.iter().map(|n| text(n)).collect()),
            ),
        ],
    );
}

/// `tests/test_released_escape_revalidation.py`: a proximity release must
/// not sacrifice a still-legal complete package exit.
#[cfg(test)]
#[path = "flow_prefix_tests.rs"]
mod prefix_tests;

#[cfg(test)]
mod released_escape_revalidation {
    use super::*;
    use crate::test_support_routers as routers;
    use copper_pcb::board::{PadReference, PhysicalNet};

    fn empty_held(released: Vec<copper_physical::CopperKeepout>) -> Held {
        Held {
            tracks: Vec::new(),
            vias: Vec::new(),
            keepouts: Vec::new(),
            released,
            witnesses: Vec::new(),
            provenance: None,
        }
    }

    #[derive(Default)]
    struct GeometryEvents(Vec<IndexMap<String, Value>>);

    impl crate::progress::Progress for GeometryEvents {
        fn observe(&mut self, phase: &str, event: &str, fields: Vec<(&'static str, Value)>) {
            if phase == "ordinary_escapes" && event == "geometry_checked" {
                self.0.push(fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect());
            }
        }
    }

    #[test]
    fn passive_geometry_events_locate_first_crossing_without_claiming_package_access() {
        let (source, reserved, _) = fixture(false);
        let source_before = format!("{source:?}");
        let reserved_before = format!("{reserved:?}");
        let mut crossed = source.clone();
        crossed.tracks.push(routers::track("C", (12.0, 11.0), (14.0, 11.0), 0.2, CopperLayer::Front));
        crossed.copper_keepouts.extend(reserved.2.iter().cloned());
        let crossed_before = format!("{crossed:?}");
        let mut events = GeometryEvents::default();
        observe_source_reserved_geometry(
            &source,
            &reserved,
            (&[], &[]),
            None,
            "initial_ordinary_plan",
            &mut Some(&mut events),
        );
        observe_reserved_geometry(
            &source,
            &crossed,
            &reserved,
            (&[], &[]),
            None,
            None,
            "after_initial_decoupler_batch",
            Some("C_CAP.1"),
            &mut Some(&mut events),
        );
        assert_eq!(events.0.len(), 2);
        assert_eq!(events.0[0]["invalid_escape_net_count"], int(0));
        assert_eq!(events.0[0]["first_invalid_piece"], Value::Null);
        let bad = &events.0[1];
        assert_eq!(bad["boundary"], text("after_initial_decoupler_batch"));
        assert_eq!(bad["link"], text("C_CAP.1"));
        assert_eq!(bad["invalid_escape_nets"], Value::Array(vec![text("S")]));
        assert_eq!(bad["package_access_proof"], text("not_checked"));
        let Value::Object(piece) = &bad["first_invalid_piece"] else { panic!("located piece") };
        assert_eq!(piece["kind"], text("track"));
        assert_eq!(piece["reserved_occurrence"], int(0));
        let Value::Object(blockers) = &piece["blockers"] else { panic!("blocker evidence") };
        let Value::Array(records) = &blockers["records"] else { panic!("blocker records") };
        assert!(records.iter().any(|item| matches!(item, Value::Object(record) if record["net"] == text("C"))));
        assert_eq!(format!("{source:?}"), source_before);
        assert_eq!(format!("{reserved:?}"), reserved_before);
        assert_eq!(format!("{crossed:?}"), crossed_before);
    }

    #[test]
    fn passive_geometry_keeps_authored_duplicate_masks_and_reports_missing_source_occurrences() {
        let (mut source, reserved, _) = fixture(false);
        source.copper_keepouts = vec![reserved.2[0].clone(), reserved.2[0].clone()];
        let mut candidate = source.clone();
        candidate.copper_keepouts.extend(reserved.2.iter().cloned());
        let original = candidate.clone();
        let mut events = GeometryEvents::default();
        observe_reserved_geometry(
            &source,
            &candidate,
            &reserved,
            (&[], &[]),
            None,
            None,
            "authored_masks_present",
            None,
            &mut Some(&mut events),
        );
        candidate.copper_keepouts.clear();
        observe_reserved_geometry(
            &source,
            &candidate,
            &reserved,
            (&[], &[]),
            None,
            None,
            "authored_masks_missing",
            None,
            &mut Some(&mut events),
        );
        assert_eq!(events.0[0]["source_masks_present"], Value::Bool(true));
        assert_eq!(events.0[0]["invalid_escape_nets"], Value::Array(vec![text("S")]));
        assert_eq!(events.0[1]["source_masks_present"], Value::Bool(false));
        assert_eq!(original.copper_keepouts.len(), source.copper_keepouts.len() + reserved.2.len());
        assert!(candidate.copper_keepouts.is_empty());
    }

    /// A front clock net `C` along y = 12 mm and the reserved exit of `S`
    /// (a stem from its pad at (13, 10) mm to a via at (13, 12) mm) that the
    /// clock would cross; the bypass planning released the whole exit.
    fn fixture(block_stem: bool) -> (PhysicalBoard, Reserved, Held) {
        let mut board = routers::pad_board(
            &[("S", &[(10.0, 7.0), (10.0, 17.0)]), ("C", &[(1.0, 9.0), (19.0, 9.0)])],
            2,
            Vec::new(),
        );
        board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule {
            kind: RouteKind::Clock,
            width_nm: Some(routers::mm(0.2)),
            allowed_layers: vec![CopperLayer::Front],
            max_vias: Some(0),
            ..routers::rule("C")
        }];
        let stem = routers::track("S", (13.0, 10.0), (13.0, 12.0), 0.2, CopperLayer::Front);
        let via = routers::via("S", (13.0, 12.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
        if block_stem {
            board.copper_keepouts = vec![routers::copper_keepout(
                "real-blocker",
                &[CopperLayer::Front],
                routers::rect(12.0, 10.8, 14.0, 11.2),
            )];
        }
        let keepouts = copper_keepouts(
            &board,
            std::slice::from_ref(&stem),
            std::slice::from_ref(&via),
            "escape-reserve",
            Some(&board),
        );
        assert_eq!(keepouts.len(), 2);
        (board, (vec![stem], vec![via], keepouts.clone()), empty_held(keepouts))
    }

    fn critical(board: PhysicalBoard) -> CriticalRoutingResult {
        let guides =
            crate::route_global(&board, &crate::GlobalRouterOptions::default(), None, None, None).expect("guides");
        crate::critical::route_critical_nets(board, &guides, &crate::critical::CriticalOptions::default(), None)
            .expect("critical")
    }

    fn channel_fixture() -> (PhysicalBoard, Reserved) {
        let (mut board, reserved, _) = fixture(false);
        for placement in board.placements.iter_mut().filter(|p| p.reference.starts_with("C_")) {
            placement.position.y_nm -= routers::mm(1.0);
        }
        // The source areas leave the promised stem/via legal. Its temporary
        // masks close the sole central F.Cu channel to a foreign feed.
        board.copper_keepouts = vec![
            routers::copper_keepout("upper-wall", &[CopperLayer::Front], routers::rect(12.0, 0.0, 14.0, 9.5)),
            routers::copper_keepout("lower-wall", &[CopperLayer::Front], routers::rect(12.0, 12.5, 14.0, 26.0)),
        ];
        (board, reserved)
    }

    fn surface_bypass_channel() -> (PhysicalBoard, Reserved, copper_pcb::board::DecouplingLink) {
        let (mut board, reserved) = channel_fixture();
        board.net_routing_rules[0].kind = RouteKind::General;
        let pose = board.placements.iter().position(|p| p.reference == "C_0").expect("capacitor");
        let mut footprint = board.footprints[&board.placements[pose].footprint].clone();
        footprint.name = "test/channel-cap".to_owned();
        let mut return_land = footprint.pads[0].clone();
        return_land.number = "2".to_owned();
        return_land.position = routers::pt(0.0, 2.0);
        footprint.pads.push(return_land);
        board.placements[pose].footprint = footprint.name.clone();
        board.footprints.insert(footprint.name.clone(), footprint);
        board.nets.push(PhysicalNet { name: "R".to_owned(), pads: vec![PadReference::new("C_0", "2")] });
        let link = copper_pcb::board::DecouplingLink {
            capacitor: PadReference::new("C_0", "1"),
            target: PadReference::new("C_1", "1"),
            return_pad: PadReference::new("C_0", "2"),
            net: "C".to_owned(),
            return_net: "R".to_owned(),
        };
        board.decoupling_links = vec![link.clone()];
        (board, reserved, link)
    }

    #[test]
    fn collateral_release_restores_complete_exit_and_deflects_clock() {
        let (board, reserved, held) = fixture(false);
        let (kept, updated, report) =
            revalidate_released_escapes(&board, reserved.clone(), held).expect("revalidation");
        assert_eq!(kept.2, reserved.2);
        assert!(updated.released.is_empty());
        assert_eq!(report.restored_nets, ["S"]);
        assert_eq!(report.restored_keepouts, reserved.2.len());
        assert_eq!(report.outcome, "restored");
        for protect in [false, true] {
            let mut source = board.clone();
            source.copper_keepouts = if protect { kept.2.clone() } else { Vec::new() };
            let routed = critical(source);
            assert!(routed.nets.iter().all(|n| n.connected), "protect={protect}");
            // Temporary masks are planning-only; query the actual routed clock copper.
            let mut actual = routed.board;
            actual.copper_keepouts.clear();
            let index = crate::routing_clearance::RoutingClearanceIndex::new(&actual).expect("index");
            let (stem, via) = (&reserved.0[0], &reserved.1[0]);
            let legal = index.can_track(&stem.net, stem.start, stem.end, stem.width_nm, stem.layer).expect("track")
                && index
                    .can_via(
                        &via.net,
                        via.position,
                        via.size_nm,
                        via.from_layer,
                        via.to_layer,
                        Some(via.drill_nm),
                        false,
                        None,
                    )
                    .expect("via");
            assert_eq!(legal, protect);
        }
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }

    #[test]
    fn critical_retry_checks_the_complete_exit_beside_real_candidate_copper() {
        let (mut board, _, _) = fixture(false);
        board.placements.iter_mut().find(|p| p.reference == "S_0").expect("source land").position =
            routers::pt(13.0, 8.0);
        // The default global guide uses 5 mm tiles. Leave a full coarse row
        // available without the reservations, rather than testing a 3 mm
        // channel that the surface bypass can use but the global guide cannot.
        // The complete real stem closes the wider channel when protected.
        board.copper_keepouts = vec![
            routers::copper_keepout("upper-wall", &[CopperLayer::Front], routers::rect(12.0, 0.0, 14.0, 7.5)),
            routers::copper_keepout("lower-wall", &[CopperLayer::Front], routers::rect(12.0, 16.5, 14.0, 26.0)),
        ];
        let stem = routers::track("S", (13.0, 8.0), (13.0, 16.0), 0.2, CopperLayer::Front);
        let via = routers::via("S", (13.0, 16.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
        let masks = copper_keepouts(
            &board,
            std::slice::from_ref(&stem),
            std::slice::from_ref(&via),
            "escape-reserve",
            Some(&board),
        );
        let reserved = (vec![stem], vec![via], masks);
        let mut protected = board.clone();
        protected.copper_keepouts.extend(reserved.2.iter().cloned());
        let mut incumbent = critical(protected);
        let guides = crate::route_global(&board, &crate::GlobalRouterOptions::default(), None, None, None)
            .expect("unmasked guides");
        assert!(
            guides.routes.iter().any(|r| r.net == "C" && r.connected),
            "unmasked C guide: {:?}",
            guides.routes.iter().filter(|r| r.net == "C").collect::<Vec<_>>()
        );
        let retry = crate::critical::route_critical_nets(
            board.clone(),
            &guides,
            &crate::critical::CriticalOptions::default(),
            None,
        )
        .expect("unmasked critical route");
        assert!(incumbent.nets.iter().any(|n| !n.connected), "protected incumbent: {:?}", incumbent.nets);
        assert!(retry.nets.iter().all(|n| n.connected), "unmasked retry: {:?}", retry.nets);
        let before = incumbent.board.tracks.clone();
        assert!(
            invalid_reserved_escape_nets(&board, &incumbent.board, &reserved, (&[], &[]), None, None)
                .expect("incumbent exits")
                .invalid_escape_nets
                .is_empty()
        );
        assert_eq!(
            invalid_reserved_escape_nets(&board, &retry.board, &reserved, (&[], &[]), None, None)
                .expect("retry exits")
                .invalid_escape_nets,
            ["S"]
        );
        assert_eq!(incumbent.board.tracks, before, "inspection cannot mutate the incumbent");

        // The via remains legal in the retry fixture: the actual loss is the
        // stem. Checking counts, the via alone, or critical connectivity misses it.
        let via = &reserved.1[0];
        let frame = retry.board.clone();
        let index = crate::routing_clearance::RoutingClearanceIndex::new(&frame).expect("retry index");
        assert!(
            index
                .can_via(
                    &via.net,
                    via.position,
                    via.size_nm,
                    via.from_layer,
                    via.to_layer,
                    Some(via.drill_nm),
                    false,
                    None
                )
                .expect("via remains legal")
        );
        let incumbent_before = format!("{incumbent:?}");
        let mut planned = reserved.clone();
        let mut held_as_copper = false;
        let checked = adopt_critical_retry(
            &board,
            &mut incumbent,
            &mut planned,
            &mut held_as_copper,
            None,
            (&[], &[]),
            None,
            retry,
            true,
            Vec::new(),
        )
        .expect("actual adoption branch");
        assert!(checked.improved && checked.source_masks_present == Some(true));
        assert!(!checked.adopted);
        assert!(checked.baseline_invalid.is_empty());
        assert_eq!(checked.candidate_invalid, ["S"]);
        assert_eq!(format!("{incumbent:?}"), incumbent_before, "every result field rolls back");
        assert_eq!(planned, reserved, "complete geometry and reservation masks roll back");
        assert!(!held_as_copper, "bypass ownership mode rolls back");
    }

    #[test]
    fn bypass_first_pass_and_retry_reject_a_feed_that_takes_an_ordinary_exit() {
        let (board, reserved, link) = surface_bypass_channel();
        let candidate = crate::decoupling::route_decouplers(&board, None, None, None, None)
            .expect("real unmasked bypass candidate");
        assert!(candidate.pending.is_empty());
        assert!(candidate.connected_pads.contains(&link.capacitor));
        assert!(!candidate.created_tracks.is_empty());
        let mut incumbent = critical(board.clone());
        let original = format!("{incumbent:?}");
        for retry in [false, true] {
            let previous_pending: IndexSet<_> = [link.clone()].into_iter().collect();
            let (checked, retained) =
                adopt_bypass_candidate(&board, &mut incumbent, Some(&reserved), None, retry, candidate.clone(), retry)
                    .expect("actual bypass acceptance branch");
            assert!(checked.improved && checked.source_masks_present == Some(true));
            assert!(!checked.adopted, "retry={retry}");
            assert!(checked.baseline_invalid.is_empty());
            assert_eq!(checked.candidate_invalid, ["S"]);
            assert_eq!(format!("{incumbent:?}"), original, "all poses/copper/metadata/ledger roll back");
            assert_eq!(retained.pending, previous_pending, "status comes from retained copper");
            assert!(retained.connected.is_empty(), "rejected feed must not be reported connected");
            assert!(retained.reasons[&link].contains("verified bypass"));
        }
        // A legal candidate with no conflicting promised exit still adopts.
        let (checked, accepted) =
            adopt_bypass_candidate(&board, &mut incumbent, None, None, false, candidate.clone(), false)
                .expect("legal first pass");
        assert!(checked.adopted);
        assert!(accepted.pending.is_empty());
        assert!(accepted.connected.contains(&link));
        assert_eq!(incumbent.board, candidate.board);
    }

    #[test]
    fn bypass_status_keeps_budget_failures_pending_and_skipped_links_unchecked() {
        let (mut source, _, link) = surface_bypass_channel();
        let mut routed = crate::decoupling::route_decouplers(&source, None, None, None, None).expect("feed");
        source.net_routing_rules[0].max_length_nm = Some(routers::mm(1.0));
        routed.board.net_routing_rules = source.net_routing_rules.clone();
        let status = retained_bypass_status(&source, &routed.board, &source, None, None).expect("budget status");
        assert!(status.pending.contains(&link));
        assert!(!status.connected.contains(&link));
        assert_eq!(status.reasons[&link], "whole_net_length_budget_exceeded");

        let (mut source, _, link) = surface_bypass_channel();
        source.net_routing_rules[0].kind = RouteKind::Clock;
        let mut incumbent = critical(source.clone());
        let candidate = crate::decoupling::route_decouplers(&incumbent.board, None, None, None, None)
            .expect("router skips critical association");
        assert!(candidate.pending.is_empty());
        let before = format!("{incumbent:?}");
        let (checked, status) =
            adopt_bypass_candidate(&source, &mut incumbent, None, None, true, candidate, true).expect("skipped retry");
        assert!(!checked.improved && !checked.adopted, "skipped candidate is not an improvement");
        assert!(status.pending.is_empty() && status.connected.is_empty());
        assert!(status.unchecked.contains_key(&link));
        assert_eq!(format!("{incumbent:?}"), before);
    }

    #[test]
    fn bypass_subset_retry_keeps_previously_completed_links_in_full_board_status() {
        let (mut source, _, link) = surface_bypass_channel();
        source.placements.push(routers::placement("Z0", "test/channel-cap", 4.0, 5.0));
        source.placements.push(routers::placement("Z1", "test/one-pad", 10.0, 5.0));
        source.nets.push(PhysicalNet {
            name: "Z".to_owned(),
            pads: vec![PadReference::new("Z0", "1"), PadReference::new("Z1", "1")],
        });
        source.nets.iter_mut().find(|n| n.name == "R").expect("return net").pads.push(PadReference::new("Z0", "2"));
        let other = copper_pcb::board::DecouplingLink {
            capacitor: PadReference::new("Z0", "1"),
            target: PadReference::new("Z1", "1"),
            return_pad: PadReference::new("Z0", "2"),
            net: "Z".to_owned(),
            return_net: "R".to_owned(),
        };
        source.decoupling_links.push(other.clone());
        let only_other: IndexSet<_> = [other.clone()].into_iter().collect();
        let initial = crate::decoupling::route_decouplers(&source, None, None, Some(&only_other), None)
            .expect("first completed link");
        assert!(initial.connected_pads.contains(&other.capacitor));
        let mut incumbent = critical(source.clone());
        incumbent.board = initial.board;
        let only_pending: IndexSet<_> = [link.clone()].into_iter().collect();
        let candidate = crate::decoupling::route_decouplers(&incumbent.board, None, None, Some(&only_pending), None)
            .expect("pending-only retry");
        assert!(!candidate.connected_pads.contains(&other.capacitor), "result flags are subset-scoped");
        let (checked, status) = adopt_bypass_candidate(&source, &mut incumbent, None, None, true, candidate, true)
            .expect("full-board adoption status");
        assert!(checked.improved && checked.adopted);
        assert!(status.pending.is_empty());
        assert!(status.connected.contains(&link) && status.connected.contains(&other));
    }

    #[test]
    fn unbound_prefix_and_held_contributions_are_checked_not_only_inserted_as_obstacles() {
        let (source, reserved, _) = fixture(false);
        let routed = critical(source.clone());
        let empty = Reserved::default();
        let prefix = invalid_reserved_escape_nets(
            &source,
            &routed.board,
            &empty,
            (&reserved.0, &reserved.1),
            Some(&reserved.2),
            None,
        )
        .expect("fixed prefix check");
        assert!(prefix.invalid_escape_nets.is_empty());
        assert_eq!(prefix.invalid_dependencies, ["fixed_prefix:S"]);
        let mut held = empty_held(Vec::new());
        held.tracks = reserved.0;
        held.vias = reserved.1;
        held.keepouts = reserved.2;
        let check = invalid_reserved_escape_nets(&source, &routed.board, &empty, (&[], &[]), None, Some(&held))
            .expect("held contribution check");
        assert!(check.invalid_escape_nets.is_empty());
        assert_eq!(check.invalid_dependencies, ["held_bypass:S"]);
    }

    #[test]
    fn authored_equal_mask_occurrences_survive_actual_join_and_final_escape_release() {
        let (mut source, reserved, _) = fixture(false);
        let authored = reserved.2[0].clone();
        source.copper_keepouts = vec![authored.clone(), authored.clone()];
        let mut masked = source.clone();
        masked.copper_keepouts.extend(reserved.2.iter().cloned());
        let joined =
            join_reserved_copper(&source, &masked, &reserved.0, &reserved.1, &reserved.2).expect("actual join");
        assert_eq!(joined.copper_keepouts, source.copper_keepouts);
        assert!(!joined.tracks.contains(&reserved.0[0]), "authored mask still blocks illegal copper");
        let mut result = critical(masked);
        let mut plane = Some(crate::plane::stitch_zone_pads(&result.board, None).expect("plane context"));
        let pre = result.board.clone();
        let final_board = release_escape_masks(
            &source,
            Some(&reserved),
            &mut result,
            &mut plane,
            pre,
            &UnifiedRouterOptions::default(),
            &mut None,
        )
        .expect("actual escape release");
        for board in [&result.board, &plane.as_ref().expect("plane").board, &final_board] {
            assert_eq!(board.copper_keepouts.iter().filter(|k| **k == authored).count(), 2);
        }
        assert!(!final_board.tracks.contains(&reserved.0[0]), "release cannot force copper through source intent");
    }

    #[test]
    fn retry_exit_validation_includes_fixed_prefix_and_preserves_authored_keepouts() {
        let (mut board, reserved, _) = fixture(false);
        let crossing = routers::track("C", (12.0, 11.0), (14.0, 11.0), 0.2, CopperLayer::Front);
        assert!(
            invalid_reserved_escape_nets(&board, &board, &reserved, (&[], &[]), None, None)
                .expect("without prefix")
                .invalid_escape_nets
                .is_empty()
        );
        assert_eq!(
            invalid_reserved_escape_nets(&board, &board, &reserved, (std::slice::from_ref(&crossing), &[]), None, None)
                .expect("with real prefix")
                .invalid_escape_nets,
            ["S"]
        );
        // A matching id is insufficient authority to remove an authored area.
        board.copper_keepouts = vec![routers::copper_keepout(
            &reserved.2[0].id,
            &[CopperLayer::Front],
            routers::rect(12.0, 10.8, 14.0, 11.2),
        )];
        assert_eq!(
            invalid_reserved_escape_nets(&board, &board, &reserved, (&[], &[]), None, None)
                .expect("authored keepout")
                .invalid_escape_nets,
            ["S"]
        );
        // Even exact equality does not authorize deleting source occurrences.
        board.copper_keepouts = vec![reserved.2[0].clone(), reserved.2[0].clone()];
        let mut candidate = board.clone();
        candidate.copper_keepouts.extend(reserved.2.iter().cloned());
        let checked = invalid_reserved_escape_nets(&board, &candidate, &reserved, (&[], &[]), None, None)
            .expect("authored exact duplicates");
        assert!(checked.source_masks_present);
        assert_eq!(checked.invalid_escape_nets, ["S"]);
        candidate.copper_keepouts.clear();
        let checked = invalid_reserved_escape_nets(&board, &candidate, &reserved, (&[], &[]), None, None)
            .expect("missing source occurrences");
        assert!(!checked.source_masks_present, "an already missing authored mask cannot grant retry authority");
    }

    #[test]
    fn legal_via_cannot_restore_an_exit_with_a_blocked_stem() {
        let (board, reserved, held) = fixture(true);
        let original = held.clone();
        let (kept, updated, report) = revalidate_released_escapes(&board, reserved, held).expect("revalidation");
        assert!(kept.2.is_empty() && report.restored_keepouts == 0);
        assert_eq!(report.unrestored_nets, ["S"]);
        assert_eq!(report.outcome, "no_complete_legal_group");
        assert_eq!(updated.released, original.released);
        assert!(updated.witnesses.is_empty() && board.tracks.is_empty() && board.vias.is_empty());
    }

    /// The cross-layer bypass of `test_decoupling_crosslayer_paths.fixture`
    /// routed, plus a released escape of a signal net `S` beside it.
    fn bypass_fixture() -> (PhysicalBoard, Reserved, Held) {
        let (mut board, _) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
        let footprint = routers::one_pad();
        board.placements.push(routers::placement("S0", &footprint.name, 3.0, 3.0));
        board.placements.push(routers::placement("S1", &footprint.name, 20.0, 3.0));
        board.footprints.insert(footprint.name.clone(), footprint);
        board.nets.push(PhysicalNet {
            name: "S".to_owned(),
            pads: vec![PadReference::new("S0", "1"), PadReference::new("S1", "1")],
        });
        let bypass = crate::decoupling::route_decouplers(&board, None, None, None, None).expect("bypass");
        assert!(!bypass.admitted_path_witnesses.is_empty());
        let stem = routers::track("S", (3.0, 3.0), (3.0, 5.0), 0.15, CopperLayer::Front);
        let via = routers::via("S", (3.0, 5.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back);
        let masks = copper_keepouts(
            &board,
            std::slice::from_ref(&stem),
            std::slice::from_ref(&via),
            "escape-reserve",
            Some(&board),
        );
        let held = Held {
            tracks: bypass.created_tracks,
            vias: bypass.created_vias,
            keepouts: Vec::new(),
            released: masks.clone(),
            witnesses: bypass.admitted_path_witnesses,
            provenance: None,
        };
        (board, (vec![stem], vec![via], masks), held)
    }

    #[test]
    fn restored_context_requires_a_new_fresh_bypass_ledger() {
        let (board, reserved, held) = bypass_fixture();
        let (kept, updated, report) =
            revalidate_released_escapes(&board, reserved, held.clone()).expect("revalidation");
        assert_eq!(report.outcome, "restored", "{report:?}");
        let mut candidate = board.clone();
        candidate.tracks = held.tracks.clone();
        candidate.vias = held.vias.clone();
        candidate.copper_keepouts = kept.2.clone();
        let validate = |witness| {
            crate::decoupling_paths::validate_crosslayer_witness(&candidate, witness, None).expect("validate")
        };
        assert!(!validate(&held.witnesses[0]).is_empty());
        assert_eq!(validate(&updated.witnesses[0]), "");
        assert_ne!(held.witnesses[0].context_digest, updated.witnesses[0].context_digest);
        assert_eq!((&updated.tracks, &updated.vias, &updated.keepouts), (&held.tracks, &held.vias, &held.keepouts));
        assert!(updated.released.is_empty());
    }

    #[test]
    fn failed_bypass_revalidation_rolls_back_masks_and_original_ledger() {
        let (board, reserved, held) = bypass_fixture();
        let mut failed = |candidate: &PhysicalBoard,
                          _: &crate::decoupling_paths::DecouplingCrossLayerWitness,
                          _: &crate::routing_clearance::RoutingClearanceIndex<'_>| {
            Ok(crate::decoupling_paths::DecouplingPathResult {
                board: candidate.clone(),
                created_tracks: Vec::new(),
                created_vias: Vec::new(),
                witness: None,
                reason: "injected verification failure".to_owned(),
                supply_path: None,
                return_path: None,
            })
        };
        let (kept, updated, report) =
            revalidate_released_escapes_with(&board, reserved, held.clone(), &mut failed).expect("revalidation");
        assert!(kept.2.is_empty());
        assert_eq!(updated.released, held.released);
        assert_eq!(updated.witnesses, held.witnesses);
        assert_eq!(report.outcome, "bypass_revalidation_failed");
        assert_eq!(report.reason.as_deref(), Some("injected verification failure"));
        assert_eq!(report.restored_keepouts, 0);
        assert!(board.tracks.is_empty() && board.vias.is_empty());
    }
}

/// Bypass planning that releases an escape reservation, then revalidation.
/// Planning drops the copper of the group it takes and renumbers the
/// surviving reservations, so the released group has nothing to restore and
/// a survivor whose new id equals a released one is not mistaken for it.
#[cfg(test)]
mod released_escape_planning {
    use super::*;
    use crate::test_support_routers as routers;
    use copper_pcb::board::{PadReference, PhysicalNet};

    #[test]
    fn decoupler_planning_releases_a_blocking_reservation_and_revalidation_keeps_the_survivor() {
        let (mut board, link) = crate::test_support_planes::support_planes::decoupling::crosslayer(false, true);
        let footprint = routers::one_pad();
        for (reference, x, y) in [("S0", 3.0, 3.0), ("S1", 20.0, 3.0), ("T0", 3.0, 17.0), ("T1", 20.0, 17.0)] {
            board.placements.push(routers::placement(reference, &footprint.name, x, y));
        }
        board.footprints.insert(footprint.name.clone(), footprint);
        for name in ["S", "T"] {
            let pads = vec![PadReference::new(&format!("{name}0"), "1"), PadReference::new(&format!("{name}1"), "1")];
            board.nets.push(PhysicalNet { name: name.to_owned(), pads });
        }
        // A ring of reserved S vias walls in the chip's supply pad at (9, 8)
        // mm; T's reserved via is far from the bypass.
        let ring = [(9.0, 8.7), (9.0, 7.3), (9.7, 8.0), (8.3, 8.0), (9.5, 8.5), (8.5, 8.5), (9.5, 7.5), (8.5, 7.5)];
        let mut vias: Vec<Via> =
            ring.iter().map(|at| routers::via("S", *at, 0.45, 0.2, CopperLayer::Front, CopperLayer::Back)).collect();
        vias.push(routers::via("T", (6.0, 17.0), 0.45, 0.2, CopperLayer::Front, CopperLayer::Back));
        let keepouts = copper_keepouts(&board, &[], &vias, "escape-reserve", Some(&board));
        assert_eq!(keepouts.len(), vias.len());
        let walled = {
            let mut walled = board.clone();
            walled.copper_keepouts.extend(keepouts.iter().cloned());
            crate::decoupling::route_decouplers(&walled, None, None, None, None).expect("bypass")
        };
        assert!(walled.pending.iter().any(|(item, _)| *item == link), "the reservations must block the bypass");
        let planned = plan_decouplers(&board, Some((Vec::new(), vias, keepouts)), &mut None, (&[], &[]), &[], None);
        let (held, reserved) = planned.expect("planning");
        let reserved = reserved.expect("reservations");
        assert!(!held.tracks.is_empty() && !held.witnesses.is_empty(), "the bypass routes once released");
        assert!(!held.released.is_empty(), "planning released the reservation the bypass took");
        // The taken S copper is gone; T's via is renumbered onto a released id.
        assert!(reserved.1.iter().all(|v| v.net == "T") && reserved.2.len() == 1);
        assert!(held.released.iter().any(|k| k.id == reserved.2[0].id));
        let original = held.clone();
        let (kept, updated, report) =
            revalidate_released_escapes(&board, reserved.clone(), held).expect("revalidation");
        assert_eq!(report.outcome, "no_complete_legal_group", "{report:?}");
        assert!(report.unrestored_nets.is_empty() && report.restored_keepouts == 0);
        assert_eq!(kept, reserved);
        // No copper changed and the original ledger is returned.
        assert_eq!(
            (updated.tracks, updated.vias, updated.witnesses),
            (original.tracks, original.vias, original.witnesses)
        );
        assert_eq!(updated.released, original.released);
    }
}

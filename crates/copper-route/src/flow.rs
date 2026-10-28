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
use copper_physical::CopperLayer;
use copper_physical::pyfloat::{hypot, round_int};
use copper_place::PlannerOptions;

use crate::bga_fanout::plan_bga_fanouts;
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
}

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
}

impl RoutingPipelineResult {
    pub fn board(&self) -> &PhysicalBoard {
        &self.detailed.board
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
fn macro_port_halos(board: &PhysicalBoard) -> Result<Vec<copper_physical::CopperKeepout>> {
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

/// `_route_critical_stage` without escape or bypass reservations: planar
/// promotion, then critical routing on the placed board.
fn route_critical_stage(
    placement: &PlacementRoutingResult,
    unified_options: &UnifiedRouterOptions,
    unified_fanout: Option<&FanoutOptions>,
    pair_state_limit: Option<i64>,
    plane_stitch_options: Option<&PlaneStitchOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<CriticalRoutingResult> {
    let board = &placement.board;
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
    if unified_fanout.is_some() && unified_options.escapes_first {
        return copper_pcb::not_implemented("ordinary escape reservations (flow._plan_ordinary_escapes)");
    }
    if !board.decoupling_links.is_empty() && unified_options.pre_route_decouplers {
        return copper_pcb::not_implemented("bypass reservations (flow._plan_decouplers)");
    }
    emit(on_progress, "bypass_reservation", "bound", vec![("mode", text("keepouts")), ("reason", Value::Null)]);
    let return_options = critical_return_plane_options(board, plane_stitch_options, &halos);
    if return_options.is_some() {
        return copper_pcb::not_implemented(
            "critical return-plane contacts (route_critical_nets plane_stitch_options)",
        );
    }
    let jobs = crate::critical::critical_jobs(&{
        let mut promoted = board.clone();
        if let (Some(rules), _) = crate::critical::promote_planar_nets(board) {
            promoted.net_routing_rules = rules;
        }
        promoted
    });
    let defaults = UnifiedRouterOptions::default();
    if !jobs.is_empty() {
        if on_progress.is_some() {
            return copper_pcb::not_implemented("critical_group progress events");
        }
        if unified_options.critical_pair_state_escalation != defaults.critical_pair_state_escalation
            || unified_options.maximum_iterations < 16
            || unified_options.commit_rounds < 2
            || unified_options.time_budget_s.is_some()
        {
            return copper_pcb::not_implemented("unified options passed to route_critical_nets");
        }
    }
    let options = crate::critical::CriticalOptions {
        only_nets: None,
        pair_state_limit,
        bundle_repair_limit: crate::critical_bundles::BUNDLE_REPAIR_LIMIT,
    };
    crate::critical::route_critical_nets(board.clone(), &placement.global_route, &options)
}

/// `_complete_critical_stage` for a run without reservations or held copper.
fn complete_critical_stage(
    mut critical: CriticalRoutingResult,
    unified_options: &UnifiedRouterOptions,
    plane_stitch_options: Option<&PlaneStitchOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
    array_plans: Option<&IndexMap<String, ArrayEscape>>,
) -> Result<(CriticalRoutingResult, Option<PlaneStitchResult>)> {
    if !critical.board.decoupling_links.is_empty() && !unified_options.direct && unified_options.pre_route_decouplers {
        return copper_pcb::not_implemented("bypass completion (decoupling.route_decouplers)");
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
            if unified_options.power_islands_ball_grid_only {
                return copper_pcb::not_implemented("power_islands.plan_ball_grid_islands");
            }
            let partition = crate::power_islands::plan_power_islands(&critical.board, layer)
                .map_err(|error| Error::NotImplemented(error.crash_line()))?;
            critical.board.zones.extend(partition.zones.iter().cloned());
            emit(on_progress, "power_islands", "finished", partition.report());
        }
    }
    if unified_options.bga_fanout {
        emit(on_progress, "bga_fanout", "started", Vec::new());
        let bga = plan_bga_fanouts(&critical.board, 2, None, array_plans)?;
        emit(on_progress, "bga_fanout", "finished", bga.report());
        if unified_options.save_inputs_path.is_some() && (!bga.tracks.is_empty() || !bga.vias.is_empty()) {
            return copper_pcb::not_implemented("the ball-grid fanout sidecar of --unified-save-inputs");
        }
        critical.board = bga.board;
    }
    if plane_stitch_options.is_some() {
        return copper_pcb::not_implemented("plane.stitch_zone_pads");
    }
    Ok((critical, None))
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
    if fanout_options.is_some() && !unified_options.direct {
        return copper_pcb::not_implemented("the unified router's escape seeder (fanout.route_fanout)");
    }
    let progress: Option<&mut dyn crate::progress::Progress> = match on_progress {
        Some(callback) => Some(&mut **callback),
        None => None,
    };
    let detailed = crate::unified_router::route::route_unified(
        pre_detailed,
        global_route,
        unified_options,
        None,
        None,
        None,
        progress,
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
    mut on_progress: Option<ProgressCallback<'_>>,
) -> Result<RoutingPipelineResult> {
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
        });
    }
    let escape_plan_options = options.escape_plan_options.as_ref();
    let (array_plans, surface_balls) = plan_array_frames(&placement.board, escape_plan_options, &mut on_progress)?;
    if let (Some(surface), Some(fanout)) = (&surface_balls, unified_fanout.as_mut()) {
        fanout.surface_pads.extend(surface.iter().cloned());
    }
    if escape_plan_options.is_some_and(|options| options.via_lattice) {
        return copper_pcb::not_implemented("the via lattice (via_lattice.plan_via_lattices)");
    }
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
    let critical = if unified_options.direct {
        emit(
            &mut on_progress,
            "unified_direct",
            "started",
            vec![("reason", text("all mutable nets negotiate together; only source-fixed copper is retained"))],
        );
        direct_critical(&placement)?
    } else {
        let pair_state_limit = unified_options
            .critical_pair_state_limit
            .filter(|limit| *limit != 0)
            .or(Some(options.critical_pair_state_limit).filter(|limit| *limit != 0));
        route_critical_stage(
            &placement,
            &unified_options,
            unified_fanout.as_ref(),
            pair_state_limit,
            options.plane_stitch_options.as_ref(),
            &mut on_progress,
        )?
    };
    let (critical, plane_stitch) = complete_critical_stage(
        critical,
        &unified_options,
        plane_stitch_options.as_ref(),
        &mut on_progress,
        array_plans.as_ref(),
    )?;
    let pre_detailed = plane_stitch.as_ref().map_or(&critical.board, |stitch| &stitch.board).clone();
    let (detailed, drc, duplicate_stitch) = route_ordinary_stage(
        &unified_options,
        unified_fanout.as_ref(),
        &pre_detailed,
        &placement.global_route,
        options.drc_policy,
        &mut on_progress,
    )?;
    let feedback_budget =
        options.placement_feedback_trials.max(options.detailed_feedback_trials + options.critical_feedback_trials);
    if feedback_budget != 0 {
        // The loop costs nothing when every net routes, but its entry still
        // differs; not rewritten yet.
        return copper_pcb::not_implemented(
            "congestion-driven placement feedback (congestion_feedback.run_placement_feedback)",
        );
    }
    let critical = if unified_options.direct {
        unified_profile_review(&detailed, &placement.global_route, &drc)?
    } else {
        critical
    };
    let passed = placement.full_route_certified
        && critical.status != CriticalRoutingStatus::Failed
        && detailed.status == DetailedRoutingStatus::Success
        && drc.decision() == copper_drc::Decision::Pass;
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
    })
}

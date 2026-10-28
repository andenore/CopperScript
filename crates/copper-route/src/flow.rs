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
    /// The congestion-driven placement feedback loop (None when not enabled).
    pub placement_feedback: Option<FeedbackOutcome>,
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

/// The installed progress observer, reborrowed for one call.
fn reborrow<'a>(on_progress: &'a mut Option<ProgressCallback<'_>>) -> Option<ProgressCallback<'a>> {
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

/// `_route_critical_stage` without escape or bypass reservations: planar
/// promotion, then critical routing on the placed board.
fn route_critical_stage(
    placement: &PlacementRoutingResult,
    unified_options: &UnifiedRouterOptions,
    unified_fanout: Option<&FanoutOptions>,
    pair_state_limit: Option<i64>,
    plane_stitch_options: Option<&PlaneStitchOptions>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<(CriticalRoutingResult, Option<Reserved>)> {
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
    let mut reserved = match unified_fanout {
        Some(fanout) if unified_options.escapes_first => {
            plan_ordinary_escapes(board, fanout, on_progress, &halos, &planar)?
        }
        _ => None,
    };
    if !board.decoupling_links.is_empty() && unified_options.pre_route_decouplers {
        return copper_pcb::not_implemented("bypass reservations (flow._plan_decouplers)");
    }
    let critical_inputs = |keepouts: &[copper_physical::CopperKeepout],
                           on_progress: &mut Option<ProgressCallback<'_>>| {
        let mut source = board.clone();
        source.copper_keepouts.extend(keepouts.iter().cloned());
        emit(on_progress, "bypass_reservation", "bound", vec![("mode", text("keepouts")), ("reason", Value::Null)]);
        source
    };
    let source = critical_inputs(reserved.as_ref().map_or(&[][..], |r| &r.2[..]), on_progress);
    let return_options = critical_return_plane_options(&source, plane_stitch_options, &halos);
    let options = crate::critical::CriticalOptions {
        pair_state_limit,
        plane_stitch_options: return_options.as_ref(),
        unified_options: Some(unified_options),
        ..Default::default()
    };
    let mut critical =
        crate::critical::route_critical_nets(source, &placement.global_route, &options, reborrow(on_progress))?;
    if let Some(planned) = reserved.as_mut() {
        let failed: IndexSet<String> =
            critical.nets.iter().filter(|item| !item.connected).flat_map(|item| item.nets.iter().cloned()).collect();
        if !failed.is_empty() {
            // Critical nets come first: free the reservations around the pads
            // of any critical net they blocked and route again; keep whichever
            // attempt connects more critical nets.
            let kept = release_reservations(board, &planned.2, &failed, crate::nm_from_mm(2));
            if kept.len() < planned.2.len() {
                if unified_options.retain_escape_copper {
                    return copper_pcb::not_implemented(
                        "retained ordinary escape copper (flow.bind_ordinary_escape_copper)",
                    );
                }
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
                let retry_source = critical_inputs(&kept, on_progress);
                let retry = crate::critical::route_critical_nets(
                    retry_source,
                    &placement.global_route,
                    &options,
                    reborrow(on_progress),
                )?;
                let failures =
                    |result: &CriticalRoutingResult| result.nets.iter().filter(|item| !item.connected).count();
                if failures(&retry) < failures(&critical) {
                    critical = retry;
                    planned.2 = kept;
                }
            }
        }
    }
    Ok((critical, reserved))
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
            placement_feedback: None,
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
    };
    if unified_options.direct {
        emit(
            &mut on_progress,
            "unified_direct",
            "started",
            vec![("reason", text("all mutable nets negotiate together; only source-fixed copper is retained"))],
        );
    }
    let routed = route_tail(&placement, array_plans.as_ref(), &tail, &mut on_progress)?;
    let feedback_budget =
        options.placement_feedback_trials.max(options.detailed_feedback_trials + options.critical_feedback_trials);
    let mut placement = placement;
    let (mut critical, mut detailed, mut drc) = (routed.critical, routed.detailed, routed.drc);
    let (mut plane_stitch, mut duplicate_stitch) = (routed.plane_stitch, routed.duplicate);
    let placement_feedback = if feedback_budget != 0 {
        let state = FeedbackState {
            board: placement.board.clone(),
            global_route: placement.global_route.clone(),
            critical,
            detailed,
            drc,
            deferred_nets: routed.deferred,
            payload: (plane_stitch, duplicate_stitch),
        };
        let mut rerun = |trial_board: &PhysicalBoard,
                         trial_global: &GlobalRoutingResult,
                         progress: &mut Option<ProgressCallback<'_>>|
         -> Result<FeedbackState<TailPayload>> {
            if unified_options.save_inputs_path.is_some() {
                return copper_pcb::not_implemented("congestion trials beside --unified-save-inputs");
            }
            let mut trial = placement.clone();
            trial.board = trial_board.clone();
            trial.global_route = trial_global.clone();
            let (trial_plans, _) = plan_array_frames(trial_board, escape_plan_options, progress)?;
            let routed = route_tail(&trial, trial_plans.as_ref(), &tail, progress)?;
            Ok(FeedbackState {
                board: trial_board.clone(),
                global_route: trial_global.clone(),
                critical: routed.critical,
                detailed: routed.detailed,
                drc: routed.drc,
                deferred_nets: routed.deferred,
                payload: (routed.plane_stitch, routed.duplicate),
            })
        };
        let score = |state: &FeedbackState<TailPayload>| feedback_score(&state.critical, &state.detailed, &state.drc);
        let (best, outcome) = crate::congestion_feedback::run_placement_feedback(
            state,
            feedback_budget,
            options.detailed_feedback_movement_nm,
            &placement_options,
            &global_options,
            &mut rerun,
            &score,
            &mut on_progress,
        )?;
        if outcome.accepted_moves != 0 {
            placement.board = best.board;
            placement.global_route = best.global_route;
            placement.placement_candidate = format!("{}-congestion", placement.placement_candidate);
        }
        (critical, detailed, drc) = (best.critical, best.detailed, best.drc);
        (plane_stitch, duplicate_stitch) = best.payload;
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
        placement_feedback,
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
}

/// The plane stitch and duplicate-pad stitch of one tail pass.
type TailPayload = (Option<PlaneStitchResult>, DuplicatePadStitchResult);

struct TailResult {
    critical: CriticalRoutingResult,
    plane_stitch: Option<PlaneStitchResult>,
    deferred: IndexSet<String>,
    detailed: DetailedRoutingResult,
    drc: DrcReport,
    duplicate: DuplicatePadStitchResult,
}

/// Reservations, critical routing, held copper, islands, dogbones, plane
/// stitch, ordinary routing and closure on `placement`.
fn route_tail(
    placement: &PlacementRoutingResult,
    array_plans: Option<&IndexMap<String, ArrayEscape>>,
    tail: &TailInputs<'_>,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<TailResult> {
    let (critical, reserved) = if tail.unified_options.direct {
        (direct_critical(placement)?, None)
    } else {
        route_critical_stage(
            placement,
            tail.unified_options,
            tail.unified_fanout,
            tail.pair_state_limit,
            tail.critical_plane_options,
            on_progress,
        )?
    };
    let (mut critical, mut plane_stitch) =
        complete_critical_stage(critical, tail.unified_options, tail.plane_stitch_options, on_progress, array_plans)?;
    let pre_detailed = plane_stitch.as_ref().map_or(&critical.board, |stitch| &stitch.board).clone();
    let deferred = copper_pcb::zones::deferred_zone_nets(&pre_detailed)?;
    let pre_detailed = release_escape_masks(
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
    Ok(TailResult { critical, plane_stitch, deferred, detailed, drc, duplicate })
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
type Reserved = (Vec<TrackSegment>, Vec<Via>, Vec<copper_physical::CopperKeepout>);

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

/// `_copper_keepouts`: keepouts covering planned copper plus clearance.
fn copper_keepouts(
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
        keepouts.push(keepout(format!("{prefix}-via-{index}"), layers, capsule(via.position, via.position, radius)));
    }
    keepouts
}

/// `_route_ordinary_escapes`: escapes of `nets` on the macro frame.
fn route_ordinary_escapes(
    board: &PhysicalBoard,
    fanout_options: &FanoutOptions,
    nets: &IndexSet<String>,
    keepouts: &[copper_physical::CopperKeepout],
) -> Result<(Vec<TrackSegment>, Vec<Via>, Vec<copper_physical::CopperKeepout>, Vec<copper_pcb::board::PadReference>)> {
    let mut frame = copper_drc::macros::materialize_hard_macros(copper_pcb::hard_macros::macro_source(board.clone())?)?;
    frame.copper_keepouts.extend(keepouts.iter().cloned());
    let escapes = crate::fanout::route_fanout(&frame, Some(fanout_options), Some(nets))?;
    let tracks = added_elements(&escapes.board.tracks, &frame.tracks);
    let vias = added_elements(&escapes.board.vias, &frame.vias);
    let reserved = copper_keepouts(board, &tracks, &vias, "escape-reserve", Some(&frame));
    Ok((tracks, vias, reserved, escapes.pending_pads))
}

/// `_plan_ordinary_escapes`: escapes for crowded ordinary pins, and the
/// keepouts that reserve them, or None.
fn plan_ordinary_escapes(
    board: &PhysicalBoard,
    fanout_options: &FanoutOptions,
    on_progress: &mut Option<ProgressCallback<'_>>,
    keepouts: &[copper_physical::CopperKeepout],
    exclude: &[String],
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
        })
        .map(|net| net.name.clone())
        .collect();
    if ordinary.is_empty() {
        return Ok(None);
    }
    emit(on_progress, "ordinary_escapes", "started", vec![("nets", int(ordinary.len() as i64))]);
    match route_ordinary_escapes(board, fanout_options, &ordinary, keepouts) {
        Err(Error::Value(message)) => {
            emit(on_progress, "ordinary_escapes", "finished", vec![("skipped", text(&message))]);
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
                "finished",
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
) -> Vec<copper_physical::CopperKeepout> {
    let poses: IndexMap<&str, _> = board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    let mut boxes = Vec::new();
    for net in board.nets.iter().filter(|n| nets.contains(&n.name)) {
        let points: Vec<Point> = net
            .pads
            .iter()
            .map(|pad| {
                let pose = poses[pad.component.as_str()];
                let land = board.footprints[&pose.footprint].pads.iter().find(|l| l.number == pad.pad).expect("land");
                copper_pcb::transform::transformed_local_point(pose, land.position)
            })
            .collect();
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
fn track_is_contained(candidate: &TrackSegment, existing: &[TrackSegment]) -> bool {
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
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    keepouts: &[copper_physical::CopperKeepout],
) -> Result<PhysicalBoard> {
    let temporary: IndexSet<&str> = keepouts.iter().map(|k| k.id.as_str()).collect();
    let mut base = board.clone();
    base.copper_keepouts.retain(|k| !temporary.contains(k.id.as_str()));
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
fn release_escape_masks(
    reserved: Option<&Reserved>,
    critical: &mut CriticalRoutingResult,
    plane_stitch: &mut Option<PlaneStitchResult>,
    pre_detailed: PhysicalBoard,
    unified_options: &UnifiedRouterOptions,
    on_progress: &mut Option<ProgressCallback<'_>>,
) -> Result<PhysicalBoard> {
    let Some((tracks, vias, keepouts)) = reserved else { return Ok(pre_detailed) };
    let temporary: IndexSet<&str> = keepouts.iter().map(|k| k.id.as_str()).collect();
    critical.board.copper_keepouts.retain(|k| !temporary.contains(k.id.as_str()));
    if let Some(stitch) = plane_stitch.as_mut() {
        stitch.board.copper_keepouts.retain(|k| !temporary.contains(k.id.as_str()));
    }
    let mut pre_detailed = pre_detailed;
    pre_detailed.copper_keepouts.retain(|k| !temporary.contains(k.id.as_str()));
    if unified_options.planned_escapes_as_copper && (!tracks.is_empty() || !vias.is_empty()) {
        let joined = join_reserved_copper(&pre_detailed, tracks, vias, &[])?;
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

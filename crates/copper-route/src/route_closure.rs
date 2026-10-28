//! Physical-land closure shared by full and subset routing transactions
//! (`pcbir.route_closure`).

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use copper_drc::Policy;
use copper_drc::digest::physical_board_digest;
use copper_pcb::board::{PadReference, PhysicalBoard};
use copper_pcb::zones::distribution_zone_nets;
use copper_physical::pyfloat::{hypot, round_int};

use crate::Result;
use crate::detailed::{DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus};
use crate::drc_report::{DrcReport, run_physical_drc};
use crate::pad_stitch::{DuplicatePadStitchResult, stitch_duplicate_pads};
use crate::plane_verify::PlaneVerification;

/// `repr(DetailedRoutingMetrics)`.
fn metrics_repr(metrics: &DetailedRoutingMetrics) -> String {
    format!(
        "DetailedRoutingMetrics(routed_net_count={}, unrouted_net_count={}, conflict_resource_count={}, \
         total_conflict_overflow={}, track_count={}, via_count={}, total_length_nm={}, passes={})",
        metrics.routed_net_count,
        metrics.unrouted_net_count,
        metrics.conflict_resource_count,
        metrics.total_conflict_overflow,
        metrics.track_count,
        metrics.via_count,
        metrics.total_length_nm,
        metrics.passes
    )
}

/// `reconcile_zone_lands`: unresolved and independently connected zone-pad references.
pub fn reconcile_zone_lands(
    board: &PhysicalBoard,
    pending: &[PadReference],
    evidence: Option<&PlaneVerification>,
) -> Result<(Vec<PadReference>, Vec<PadReference>)> {
    let Some(evidence) = evidence else { return Ok((pending.to_vec(), Vec::new())) };
    if !evidence.zone_connectivity_verified(board)? {
        return Ok((pending.to_vec(), Vec::new()));
    }
    let zone_nets = distribution_zone_nets(board);
    let zone_pads: IndexSet<&PadReference> =
        board.nets.iter().filter(|net| zone_nets.contains(&net.name)).flat_map(|net| net.pads.iter()).collect();
    Ok((
        pending.iter().filter(|pad| !zone_pads.contains(pad)).cloned().collect(),
        pending.iter().filter(|pad| zone_pads.contains(pad)).cloned().collect(),
    ))
}

/// `close_detailed_lands`: stitch before scoring; measure additions and
/// reject stale success flags.
pub fn close_detailed_lands(
    detailed: &DetailedRoutingResult,
    policy: Policy,
) -> Result<(DetailedRoutingResult, DrcReport, DuplicatePadStitchResult)> {
    let mut stitch = stitch_duplicate_pads(&detailed.board)?;
    let mut board = stitch.board.clone();
    let mut drc = run_physical_drc(&board, policy)?;
    let opens: IndexSet<&str> = drc
        .findings()
        .iter()
        .filter(|finding| finding.code == "DRC-OPEN-NET")
        .flat_map(|finding| finding.nets.iter().map(String::as_str))
        .collect();
    let zone_nets = distribution_zone_nets(&board);
    let added = &board.tracks[detailed.board.tracks.len()..];
    let mut net_of: IndexMap<&PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for reference in &net.pads {
            net_of.insert(reference, net.name.as_str());
        }
    }
    let mut open_lands: IndexMap<&str, Vec<String>> = IndexMap::new();
    for reference in &stitch.pending {
        if let Some(&net) = net_of.get(reference)
            && opens.contains(net)
            && !zone_nets.contains(net)
        {
            open_lands.entry(net).or_default().push(format!(
                "lands of {}.{} are not joined by copper; declare internal_pad_groups if the part joins them internally",
                reference.component, reference.pad
            ));
        }
    }
    let nets: Vec<_> = detailed
        .nets
        .iter()
        .map(|item| {
            let mut item = item.clone();
            item.connected = item.connected && !opens.contains(item.net.as_str());
            if let Some(extra) = open_lands.get(item.net.as_str()) {
                item.diagnostics.extend(extra.iter().cloned());
            }
            let own = added.iter().filter(|track| track.net == item.net && !zone_nets.contains(&track.net));
            item.track_count += own.clone().count() as i64;
            item.length_nm += own
                .map(|track| {
                    round_int(hypot(
                        (track.end.x_nm - track.start.x_nm) as f64,
                        (track.end.y_nm - track.start.y_nm) as f64,
                    ))
                })
                .sum::<i64>();
            item
        })
        .collect();
    let mut metrics = detailed.metrics.clone();
    metrics.routed_net_count = nets.iter().filter(|item| item.connected).count() as i64;
    metrics.unrouted_net_count = nets.iter().filter(|item| !item.connected).count() as i64;
    metrics.track_count = nets.iter().map(|item| item.track_count).sum();
    metrics.total_length_nm = nets.iter().map(|item| item.length_nm).sum();
    let success = metrics.unrouted_net_count == 0 && metrics.total_conflict_overflow == 0;
    let mut metadata = board.metadata.clone();
    metadata.insert("detailed_routing".to_owned(), if success { "complete" } else { "partial" }.to_owned());
    if metadata != board.metadata {
        board.metadata = metadata;
        drc = run_physical_drc(&board, policy)?;
    }
    stitch.board = board.clone();
    if board == detailed.board && nets == detailed.nets && metrics == detailed.metrics {
        return Ok((detailed.clone(), drc, stitch));
    }
    let representation = format!(
        "({}, {}, {})",
        copper_json::python_str_repr(&detailed.global_routing_fingerprint),
        copper_json::python_str_repr(&physical_board_digest(&board)),
        metrics_repr(&metrics)
    );
    let fingerprint: String =
        Sha256::digest(representation.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    let mut closed = detailed.clone();
    closed.board = board;
    closed.nets = nets;
    closed.metrics = metrics;
    closed.status = if success { DetailedRoutingStatus::Success } else { DetailedRoutingStatus::Partial };
    closed.routing_fingerprint = fingerprint;
    Ok((closed, drc, stitch))
}

/// `routing_complete_with_fill`: routing completion (not signoff). Deferred-zone
/// findings are accepted only after exact-export-bound native refill proves
/// zero opens and violations.
pub fn routing_complete_with_fill(
    pipeline: &crate::flow::RoutingPipelineResult,
    board: &PhysicalBoard,
    drc: &DrcReport,
    evidence: Option<&PlaneVerification>,
) -> Result<bool> {
    use copper_pcb::board::RouteKind;
    if let Some(evidence) = evidence
        && !(evidence.passed
            && evidence.matches(board)?
            && evidence.findings.is_empty()
            && evidence.unconnected_count == 0
            && evidence.island_count == 0
            && evidence.other_violation_count == 0)
    {
        return Ok(false);
    }
    if pipeline.status == crate::flow::PhysicalFlowStatus::Pass && drc.decision() == copper_drc::Decision::Pass {
        return Ok(true);
    }
    if evidence.is_none() || board.zones.is_empty() {
        return Ok(false);
    }
    if pipeline.critical.status == crate::critical::CriticalRoutingStatus::Failed
        || pipeline.detailed.metrics.total_conflict_overflow != 0
    {
        return Ok(false);
    }
    let zones: IndexSet<&str> = board.zones.iter().map(|z| z.net.as_str()).collect();
    let rules: IndexMap<&str, _> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let results: IndexMap<&str, _> = pipeline.detailed.nets.iter().map(|n| (n.net.as_str(), n)).collect();
    let required = board.nets.iter().filter(|n| {
        n.pads.len() >= 2
            && !zones.contains(n.name.as_str())
            && rules.get(n.name.as_str()).is_none_or(|rule| rule.kind == RouteKind::General)
    });
    if !pipeline.placement_and_global.full_route_certified {
        let global_route = &pipeline.placement_and_global.global_route;
        let mut connected: IndexSet<&str> =
            pipeline.detailed.nets.iter().filter(|n| n.connected).map(|n| n.net.as_str()).collect();
        for item in pipeline.critical.nets.iter().filter(|item| item.connected) {
            connected.extend(item.nets.iter().map(String::as_str));
        }
        if global_route.metrics.total_overflow != 0
            || global_route.routes.iter().any(|route| {
                !route.connected && !connected.contains(route.net.as_str()) && !zones.contains(route.net.as_str())
            })
        {
            return Ok(false);
        }
    }
    for net in required {
        if results.get(net.name.as_str()).is_none_or(|result| !result.connected) {
            return Ok(false);
        }
    }
    if pipeline.detailed.nets.iter().any(|n| !n.connected && !zones.contains(n.net.as_str())) {
        return Ok(false);
    }
    for finding in &drc.report.findings {
        if finding.severity != copper_drc::Severity::Error || finding.code == "DRC-ROUTE-INCOMPLETE" {
            continue;
        }
        if finding.code == "DRC-OPEN-NET"
            && !finding.nets.is_empty()
            && finding.nets.iter().all(|net| zones.contains(net.as_str()))
        {
            continue;
        }
        return Ok(false);
    }
    Ok(true)
}

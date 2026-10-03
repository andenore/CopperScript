"""Physical-land closure shared by full and subset routing transactions."""
from dataclasses import replace
from hashlib import sha256
from math import hypot
from typing import TYPE_CHECKING

from .detailed import DetailedRoutingResult, DetailedRoutingStatus
from .drc import PhysicalDrcPolicy, PhysicalDrcReport, physical_board_digest, run_physical_drc
from .pad_stitch import DuplicatePadStitchResult, stitch_duplicate_pads
from .physical import PadReference, PhysicalBoard

if TYPE_CHECKING:
    from .plane_verify import PlaneVerification
    from .flow import RoutingPipelineResult


def routing_complete_with_fill(
    pipeline: "RoutingPipelineResult", board: PhysicalBoard,
    drc: PhysicalDrcReport, evidence: "PlaneVerification | None",
) -> bool:
    """Routing completion, not manufacturing signoff or a replacement token.

    An intent-only zone necessarily leaves the explicit-copper checker open.
    Accept only those deferred-zone findings after exact-export-bound native
    refill proves zero opens and zero violations. Failed signal searches,
    package preflight, geometric findings and stale evidence still fail closed.
    The original physical DRC report/token is never changed.
    """
    from .critical import CriticalRoutingStatus
    from .drc import DrcDisposition, DrcSeverity, DrcDecision
    from .flow import PhysicalFlowStatus
    from .physical import RouteKind
    if evidence is not None and not (
        evidence.passed and evidence.matches(board) and not evidence.findings
        and evidence.unconnected_count == evidence.island_count == evidence.other_violation_count == 0
    ):
        return False
    if pipeline.status is PhysicalFlowStatus.PASS and drc.decision is DrcDecision.PASS:
        return True
    if evidence is None or not board.zones:
        return False
    if (not pipeline.placement_and_global.full_route_certified
            or pipeline.critical.status is CriticalRoutingStatus.FAILED
            or pipeline.package_access is not None and not pipeline.package_access.ready
            or pipeline.detailed.metrics.total_conflict_overflow):
        return False
    zones = {z.net for z in board.zones}
    rules = {r.net: r for r in board.net_routing_rules}
    required = {n.name for n in board.nets if len(n.pads) >= 2 and n.name not in zones
                and (n.name not in rules or rules[n.name].kind is RouteKind.GENERAL)}
    results = {n.net: n for n in pipeline.detailed.nets}
    if any(name not in results or not results[name].connected for name in required):
        return False
    if any(not n.connected and (n.net not in zones
               or "zone net awaits verified fill and pad stitching" not in n.diagnostics)
           for n in pipeline.detailed.nets):
        return False
    for finding in drc.findings:
        if finding.disposition is DrcDisposition.WAIVED:
            return False
        if finding.severity is not DrcSeverity.ERROR:
            continue
        if finding.code == "DRC-ROUTE-INCOMPLETE":
            continue
        if finding.code == "DRC-OPEN-NET" and finding.nets and set(finding.nets) <= zones:
            continue
        return False
    return True


def reconcile_zone_lands(
    board: PhysicalBoard, pending: tuple[PadReference, ...],
    evidence: "PlaneVerification | None",
) -> tuple[tuple[PadReference, ...], tuple[PadReference, ...]]:
    """Return unresolved and independently connected zone-pad references.

    This only annotates helper work, never changes native findings or signoff.
    Non-zone pads cannot be resolved by a plane-closure shortcut.
    """
    if evidence is None or not evidence.zone_connectivity_verified(board):
        return pending, ()
    zone_nets = {zone.net for zone in board.zones}
    zone_pads = {pad for net in board.nets if net.name in zone_nets for pad in net.pads}
    return (tuple(pad for pad in pending if pad not in zone_pads),
            tuple(pad for pad in pending if pad in zone_pads))


def close_detailed_lands(
    detailed: DetailedRoutingResult, policy: PhysicalDrcPolicy | None = None,
) -> tuple[DetailedRoutingResult, PhysicalDrcReport, DuplicatePadStitchResult]:
    """Stitch before scoring; measure additions and reject stale success flags.

    This does not promote failed searches or deferred zone nets to success.
    Existing diagnostics and resource overflow remain meaningful. Independent
    fill evidence is not substituted into native DRC or a signoff token.
    """
    stitch = stitch_duplicate_pads(detailed.board)
    board = stitch.board
    drc = run_physical_drc(board, policy=policy)
    opens = {net for finding in drc.findings if finding.code == "DRC-OPEN-NET"
             for net in finding.nets}
    zone_nets = {zone.net for zone in board.zones}
    added = board.tracks[len(detailed.board.tracks):]
    nets = tuple(replace(
        item, connected=item.connected and item.net not in opens,
        track_count=item.track_count + sum(track.net == item.net for track in added
                                          if track.net not in zone_nets),
        length_nm=item.length_nm + sum(round(hypot(
            track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm))
            for track in added if track.net == item.net and track.net not in zone_nets),
    ) for item in detailed.nets)
    metrics = replace(
        detailed.metrics, routed_net_count=sum(item.connected for item in nets),
        unrouted_net_count=sum(not item.connected for item in nets),
        track_count=sum(item.track_count for item in nets),
        total_length_nm=sum(item.length_nm for item in nets),
    )
    success = metrics.unrouted_net_count == 0 and metrics.total_conflict_overflow == 0
    metadata = {**board.metadata, "detailed_routing": "complete" if success else "partial"}
    if metadata != dict(board.metadata):
        board = replace(board, metadata=metadata)
        drc = run_physical_drc(board, policy=policy)
    if board == detailed.board and nets == detailed.nets and metrics == detailed.metrics:
        return detailed, drc, replace(stitch, board=board)
    fingerprint = sha256(repr((detailed.global_routing_fingerprint,
                              physical_board_digest(board), metrics)).encode()).hexdigest()
    return replace(
        detailed, board=board, nets=nets, metrics=metrics,
        status=DetailedRoutingStatus.SUCCESS if success else DetailedRoutingStatus.PARTIAL,
        routing_fingerprint=fingerprint,
    ), drc, replace(stitch, board=board)

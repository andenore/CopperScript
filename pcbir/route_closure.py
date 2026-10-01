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

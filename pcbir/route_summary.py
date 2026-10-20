"""Pipeline-independent completion metrics for a routed board.

The benchmark ladder compares runs of different pipelines with these numbers,
so they depend only on the final copper, never on stage-internal bookkeeping.
A plane (zone) net is counted separately. Without native fill evidence its
explicit-copper opens are an upper bound, because a refill can join them.
"""

from __future__ import annotations

from collections import Counter

from .physical import PhysicalBoard


def connection_summary(board: PhysicalBoard, drc=None, *, native=None, estimate_planes: bool = True) -> dict:
    """Return connection, open, copper and DRC counts for ``board``.

    ``drc`` is an optional :class:`PhysicalDrcReport` for the same board.
    ``native`` is an optional :class:`PlaneVerification` of the filled board.
    """

    from .drc import explicit_copper_connectivity

    graph = explicit_copper_connectivity(board)
    plane_nets = _plane_nets(board)
    signal = {"nets": 0, "connections": 0, "opens": 0, "open_nets": []}
    plane = {"nets": 0, "connections": 0, "explicit_opens": 0}
    for net in sorted(board.nets, key=lambda item: item.name):
        if len(net.pads) < 2:
            continue
        roots = set()
        isolated = 0
        for pad in net.pads:
            nodes = graph.pad_nodes.get(pad, ())
            if nodes:
                roots.update(graph.roots[node] for node in nodes)
            else:
                isolated += 1
        components = len(roots) + isolated
        opens = max(0, components - 1)
        if net.name in plane_nets:
            plane["nets"] += 1
            plane["connections"] += len(net.pads) - 1
            plane["explicit_opens"] += opens
            continue
        signal["nets"] += 1
        signal["connections"] += len(net.pads) - 1
        signal["opens"] += opens
        if opens:
            signal["open_nets"].append(net.name)
    tracks = board.tracks
    length = sum(_length(track.start, track.end) for track in tracks)
    result = {
        "schema": "copperscript-route-summary/v1",
        "signal": signal,
        "plane": plane,
        "copper": {"tracks": len(tracks), "vias": len(board.vias),
                   "track_length_mm": round(length / 1_000_000, 3)},
    }
    if estimate_planes and board.zones:
        from .plane_estimate import estimate_plane_connectivity
        estimate = estimate_plane_connectivity(board)
        result["plane"]["estimated_opens"] = estimate.opens
        estimated = estimate.report()
        result["plane"]["estimated_open_nets"] = estimated["open_nets"]
        result["plane"]["estimated_open_pads"] = estimated["open_pads"]
    if drc is not None:
        codes = Counter(finding.code for finding in drc.findings
                        if finding.severity.value == "error" and finding.disposition.value == "active")
        codes.pop("DRC-OPEN-NET", None)  # Opens are counted above, per connection.
        codes.pop("DRC-ROUTE-INCOMPLETE", None)  # Likewise (it also flags unfilled zone nets).
        result["drc"] = {"decision": drc.decision.value, "errors_by_code": dict(sorted(codes.items())),
                         "errors": sum(codes.values())}
    if native is not None:
        result["native"] = {"passed": native.passed, "unconnected": native.unconnected_count,
                            "islands": native.island_count, "violations": native.other_violation_count}
    total = signal["connections"]
    result["signal"]["completion"] = round(1 - signal["opens"] / total, 4) if total else 1.0
    return result


def _plane_nets(board: PhysicalBoard) -> set[str]:
    """Nets with any declared zone: their connectivity is the fill's."""
    return {zone.net for zone in board.zones}


def _length(start, end) -> int:
    dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
    return int(round((dx * dx + dy * dy) ** 0.5))

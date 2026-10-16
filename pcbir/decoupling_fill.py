"""Post-fill continuity observations, separate from local bypass path witnesses.

These observations never authorize removal of local routes, waive bypass gates,
or qualify a high-frequency current loop. Native DRC remains independent.
"""
from dataclasses import dataclass

from .physical import DecouplingLink, PadReference, PhysicalBoard
from .filled_connectivity import filled_component_roots, native_item_identity


@dataclass(frozen=True, slots=True)
class DecouplingFillAssessment:
    link: DecouplingLink
    status: str
    supply_connected: bool | None = None
    return_connected: bool | None = None
    return_targets: tuple[PadReference, ...] = ()
    reason: str = ""
    qualification: str = (
        "Physical copper continuity observation only. Local supply/return path, "
        "loop impedance, component internals and high-frequency performance unqualified."
    )


def assess_decoupling_fill(board: PhysicalBoard, evidence):
    """Observe declared pin-associated bypasses on one exact native-filled export.

    Uses actual positive-area fragment contacts and plated copper. Ambiguous
    terminal identities, stale exports and unavailable geometry stay visible.
    Incomplete unrelated nets do not erase an observed local connection.
    """
    links = board.decoupling_links
    if not links:
        return ()

    def unsupported(reason):
        return tuple(DecouplingFillAssessment(link, "unsupported", reason=reason) for link in links)

    if not evidence.matches(board):
        return unsupported("stale_source_or_export")
    if not evidence.filled_board_digest or not evidence.report_digest:
        return unsupported("missing_native_artifact_binding")
    if any(item.kind == "unknown" for finding in evidence.native_findings if finding.category == "unconnected" for item in finding.items):
        return unsupported("unmapped_native_object")
    try:
        from shapely.geometry import MultiPoint  # noqa: F401: required by contact grouping
    except ImportError:
        return unsupported("filled_geometry_dependency_unavailable")

    from .plane_verify import native_object_map
    nets = frozenset(n for link in links for n in (link.net, link.return_net))
    roots = filled_component_roots(board, evidence, nets)
    terminals = {}
    for item in native_object_map(board).values():
        if item.kind == "pad":
            terminals.setdefault(PadReference(item.component, item.pad), []).append(item)

    def identity(reference, net):
        items = terminals.get(reference, ())
        if len(items) != 1 or items[0].net != net:
            return None
        key = native_item_identity(items[0])
        return key if key in roots else None

    results = []
    for link in links:
        supply = identity(link.capacitor, link.net), identity(link.target, link.net)
        returned = identity(link.return_pad, link.return_net)
        target_net = next((net for net in board.nets if net.name == link.return_net), None)
        targets = tuple(sorted(p for p in target_net.pads if p.component == link.target.component)) if target_net else ()
        target_ids = tuple(identity(target, link.return_net) for target in targets)
        if None in supply or returned is None or None in target_ids:
            results.append(DecouplingFillAssessment(link, "unsupported", reason="ambiguous_or_missing_terminal"))
            continue
        supplied = roots[supply[0]] == roots[supply[1]]
        if not targets:
            results.append(DecouplingFillAssessment(link, "unsupported", supplied,
                reason="no_declared_return_terminal_on_target"))
            continue
        joined = tuple(target for target, key in zip(targets, target_ids, strict=True)
                       if roots[key] == roots[returned])
        results.append(DecouplingFillAssessment(link,
            "connected" if supplied and joined else "open", supplied, bool(joined), joined,
            "" if supplied and joined else "supply_or_return_not_connected_by_supported_copper"))
    return tuple(results)

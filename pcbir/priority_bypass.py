"""Retain explicit higher-priority bypass ownership through clean probes."""
from dataclasses import replace
from .physical import RouteKind, TrackSegment
from .fanout import FanoutResult


def priority_bypass_reservations(owner, original):
    """Select already verified paths; never generate another trial or a label exemption."""
    from .bypass_reservations import verified_bypass_reservations
    from .decoupling_paths import refresh_crosslayer_witnesses
    from .package_access import _verified_bypass_links
    rules = {r.net: r for r in owner.net_routing_rules}
    threshold = max((r.priority for r in rules.values() if r.kind is not RouteKind.GENERAL), default=0)
    eligible = {l for l in owner.decoupling_links if l.net in rules
                and rules[l.net].kind is RouteKind.GENERAL
                and rules[l.net].priority > max(0, threshold)}
    if not eligible or verified_bypass_reservations(owner, original)[2] is not None:
        return None
    paths, via_paths = {}, {}
    verified = _verified_bypass_links(original.board, {l.net for l in eligible},
        witness_paths=paths, witness_vias=via_paths,
        admitted_witnesses=original.decoupling_witnesses)
    selected = eligible & verified
    if not selected:
        return None
    track_ids = {id(t) for l in selected for t in paths.get(l, ())}
    via_ids = {id(v) for l in selected for v in via_paths.get(l, ())}
    witnesses = tuple(w for w in original.decoupling_witnesses if w.link in selected)
    # Preserve original admitted paths even if fresh canonical selection differs.
    for witness in witnesses:
        for path in (witness.supply_path, witness.return_path):
            for step in path.steps:
                (track_ids if isinstance(step.edge, TrackSegment) else via_ids).add(id(step.edge))
    tracks = tuple(t for t in original.created_tracks if id(t) in track_ids)
    vias = tuple(v for v in original.created_vias if id(v) in via_ids)
    board = replace(owner, tracks=(*owner.tracks, *tracks), vias=(*owner.vias, *vias))
    witnesses, reason = refresh_crosslayer_witnesses(board, witnesses)
    if reason or not selected <= _verified_bypass_links(board, {l.net for l in selected}):
        return None
    result = FanoutResult(board, {}, (), len(tracks), len(vias), created_tracks=tracks,
        created_vias=vias, decoupling_tracks=tracks, decoupling_vias=vias,
        decoupling_witnesses=witnesses)
    return result if verified_bypass_reservations(owner, result)[2] is None else None


def merge_retained_bypasses(owner, frame, proposed, retained):
    """Strip disposable obstacles, retaining only freshly proved exact ownership."""
    from .bypass_reservations import verified_bypass_reservations
    from .decoupling_paths import refresh_crosslayer_witnesses
    if retained is None:
        return replace(proposed, board=replace(owner,
            tracks=(*owner.tracks, *proposed.created_tracks),
            vias=(*owner.vias, *proposed.created_vias)))
    if verified_bypass_reservations(owner, retained)[2] is not None:
        return None
    if (any(sum(x is t for x in frame.tracks) != 1 for t in retained.created_tracks)
            or any(sum(x is v for x in frame.vias) != 1 for v in retained.created_vias)
            or refresh_crosslayer_witnesses(frame, retained.decoupling_witnesses)[1]):
        return None
    tracks = (*retained.created_tracks, *proposed.created_tracks)
    vias = (*retained.created_vias, *proposed.created_vias)
    return replace(proposed, board=replace(owner, tracks=(*owner.tracks, *tracks),
        vias=(*owner.vias, *vias)), created_tracks=tracks, created_vias=vias,
        added_track_count=len(tracks), added_via_count=len(vias),
        decoupling_tracks=(*retained.decoupling_tracks, *proposed.decoupling_tracks),
        decoupling_vias=(*retained.decoupling_vias, *proposed.decoupling_vias),
        decoupling_witnesses=(*retained.decoupling_witnesses, *proposed.decoupling_witnesses))

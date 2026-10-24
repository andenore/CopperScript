"""Fresh read-only admission of declared ordinary bypass contributions.

This recognizes existing source-associated geometry. It never generates copper,
accepts routing flags as proof, or grants ownership of an immutable prefix.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import fields, replace

from .fanout import FanoutResult
from .physical import PhysicalBoard, RouteKind, TrackSegment, Via


def restore_bypass_reservation_edges(reserved: FanoutResult) -> FanoutResult:
    """Restore unique edge identities after decoding or fixed-prefix rematerialization.

    This changes no value, context digest, counter, contact or ownership claim.
    Missing/duplicate occurrences fail closed. The caller must still run ordinary
    fresh critical-prefix/association/geometry admission on the restored value.
    Only the bypass-only prefix emitted by the standard flow is supported here.
    """
    if not isinstance(reserved, FanoutResult) or not isinstance(reserved.board, PhysicalBoard):
        raise ValueError("critical replay requires a typed bypass reservation")
    from .decoupling_paths import DecouplingCrossLayerWitness, DecouplingCopperPath, DecouplingPathStep
    for sequence, kind in ((reserved.created_tracks,TrackSegment),(reserved.decoupling_tracks,TrackSegment),
                           (reserved.created_vias,Via),(reserved.decoupling_vias,Via)):
        if not isinstance(sequence,tuple) or any(not isinstance(edge,kind) for edge in sequence):
            raise ValueError("critical replay has untyped bypass contribution edges")
    if (not isinstance(reserved.decoupling_witnesses,tuple)
            or any(not isinstance(w,DecouplingCrossLayerWitness)
                   or any(not isinstance(p,DecouplingCopperPath) or not isinstance(p.steps,tuple)
                          or any(not isinstance(s,DecouplingPathStep) or not isinstance(s.edge,(TrackSegment,Via))
                                 for s in p.steps) for p in (w.supply_path,w.return_path))
                   for w in reserved.decoupling_witnesses)):
        raise ValueError("critical replay has an untyped bypass witness")
    if (reserved.routing_accesses or reserved.pending_pads
            or Counter(reserved.created_tracks) != Counter(reserved.decoupling_tracks)
            or Counter(reserved.created_vias) != Counter(reserved.decoupling_vias)):
        raise ValueError("critical replay requires an exact bypass-only reservation")
    edges = {}
    for edge in (*reserved.board.tracks, *reserved.board.vias):
        edges.setdefault(edge, []).append(edge)
    def original(edge):
        found = edges.get(edge, ())
        if len(found) != 1:
            raise ValueError("bypass replay has a missing or ambiguous original edge occurrence")
        return found[0]
    def path(value):
        return replace(value, steps=tuple(replace(step, edge=original(step.edge)) for step in value.steps))
    witnesses = tuple(replace(w, supply_path=path(w.supply_path), return_path=path(w.return_path))
                      for w in reserved.decoupling_witnesses)
    return replace(reserved, created_tracks=tuple(map(original, reserved.created_tracks)),
        created_vias=tuple(map(original, reserved.created_vias)),
        decoupling_tracks=tuple(map(original, reserved.decoupling_tracks)),
        decoupling_vias=tuple(map(original, reserved.decoupling_vias)), decoupling_witnesses=witnesses)


def verified_bypass_reservations(
    source: PhysicalBoard, reserved: FanoutResult,
) -> tuple[tuple[TrackSegment, ...], tuple[Via, ...], str | None]:
    """Return exact witnessed generated feeds, or no exemptions and a reason.

    Ordinary non-feed contributions are not exemptions. The critical owner must
    retain its normal protected-net, exit, profile and hard-geometry checks.
    Source-side witnesses are freshly checked too, so new ordinary copper may
    not obstruct an already verified sibling association.
    """
    def reject(reason):
        return (), (), reason

    if not isinstance(source, PhysicalBoard) or not isinstance(reserved, FanoutResult):
        return reject("bypass reservations require a typed source and FanoutResult")
    if not isinstance(reserved.board, PhysicalBoard) or type(reserved.board) is not type(source):
        return reject("bypass reservations have no typed physical board")
    created_tracks, created_vias = reserved.created_tracks, reserved.created_vias
    declared_tracks = reserved.decoupling_tracks
    declared_vias = getattr(reserved, "decoupling_vias", ())
    if any(not isinstance(items, tuple) for items in
           (created_tracks, created_vias, declared_tracks, declared_vias)):
        return reject("bypass reservation contribution sequences must be immutable tuples")
    if (any(not isinstance(t, TrackSegment) for t in (*created_tracks, *declared_tracks))
            or any(not isinstance(v, Via) for v in (*created_vias, *declared_vias))):
        return reject("bypass reservations contain untyped copper")
    if (type(reserved.added_track_count) is not int
            or type(reserved.added_via_count) is not int
            or reserved.added_track_count != len(created_tracks)
            or reserved.added_via_count != len(created_vias)):
        return reject("bypass reservation counters disagree with generated ownership")
    intent_matches = all(getattr(reserved.board, field.name) == getattr(source, field.name)
                         for field in fields(source) if field.name not in {"tracks", "vias"})
    if (not intent_matches
            or reserved.board.tracks != (*source.tracks, *created_tracks)
            or reserved.board.vias != (*source.vias, *created_vias)):
        return reject("bypass reservations changed source intent or exact ordered copper prefix")
    rules = {rule.net: rule for rule in source.net_routing_rules}
    critical_nets = {rule.net for rule in source.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    if any(item.net in critical_nets for item in (*created_tracks, *created_vias)):
        return reject("ordinary bypass reservations contain critical-kind generated copper")
    generated_counts, source_counts = Counter(created_tracks), Counter(source.tracks)
    if (len(set(declared_tracks)) != len(declared_tracks)
            or any(generated_counts[t] != 1 or source_counts[t] for t in declared_tracks)):
        return reject("declared bypass tracks have missing, immutable or ambiguous ownership")
    generated_vias, source_vias = Counter(created_vias), Counter(source.vias)
    if (len(set(declared_vias)) != len(declared_vias)
            or any(generated_vias[v] != 1 or source_vias[v] for v in declared_vias)):
        return reject("declared bypass vias have missing, immutable or ambiguous ownership")
    nets = frozenset(link.net for link in source.decoupling_links)
    # Share the exact fresh association predicate, including complete return
    # paths for opposite-face links. Net names and flags grant no exemptions.
    from .package_access import _verified_bypass_links
    original = _verified_bypass_links(source, nets)
    paths, via_paths = {}, {}
    current = _verified_bypass_links(reserved.board, nets,
                                     witness_paths=paths, witness_vias=via_paths)
    if not original <= current:
        return reject("ordinary reservations obstruct a verified source bypass association")
    ordinary_links = {link for link in current
                      if link.net not in rules or rules[link.net].kind is RouteKind.GENERAL}
    from .decoupling_paths import refresh_crosslayer_witnesses
    admitted, reason = refresh_crosslayer_witnesses(reserved.board,reserved.decoupling_witnesses)
    if reason:
        return reject("invalid original admitted bypass witness: " + reason)
    if any(w.link not in ordinary_links for w in admitted):
        return reject("original admitted bypass witness has no current complete ordinary association")
    witnessed_tracks = {track for link, path in paths.items() if link in ordinary_links
                        for track in path if track.net in {link.net, link.return_net}}
    witnessed_vias = {via for link, path in via_paths.items() if link in ordinary_links
                      for via in path if via.net in {link.net, link.return_net}}
    witnessed_track_ids = {id(t) for t in witnessed_tracks}
    witnessed_via_ids = {id(v) for v in witnessed_vias}
    for witness in admitted:
        for path in (witness.supply_path,witness.return_path):
            for step in path.steps:
                (witnessed_track_ids if isinstance(step.edge,TrackSegment) else witnessed_via_ids).add(id(step.edge))
    if (any(id(track) not in witnessed_track_ids for track in declared_tracks)
            or any(id(via) not in witnessed_via_ids for via in declared_vias)):
        return reject("declared generated bypass copper is absent from a fresh source witness")
    return declared_tracks, declared_vias, None


def rebind_bypass_keepout_removal(source, reserved, updated_source):
    """Re-prove unchanged bypass copper after an explicit keepout-only removal.

    The original reservation must already be admissible. No source rule, pose,
    copper occurrence or ownership changes are allowed. Paths are derived again
    from the new board; an old context digest is never overwritten or waived.
    Failure returns no replacement and leaves the original value untouched.
    """
    from .hard_macros import macro_source, materialize_hard_macros
    from .decoupling_paths import verify_crosslayer_decoupling
    from .routing_clearance import RoutingClearanceIndex

    if not isinstance(source, PhysicalBoard) or not isinstance(updated_source, PhysicalBoard):
        return None, "keepout rebind requires typed sources"
    if any(getattr(source, f.name) != getattr(updated_source, f.name)
           for f in fields(source) if f.name != "copper_keepouts"):
        return None, "keepout rebind changed non-keepout source intent"
    before, after = source.copper_keepouts, updated_source.copper_keepouts
    if (len(set(before)) != len(before) or len(set(after)) != len(after)
            or not set(after) <= set(before)
            or tuple(k for k in before if k in set(after)) != after):
        return None, "keepout rebind is not an ordered removal"
    try:
        frame = materialize_hard_macros(macro_source(source))
        updated = materialize_hard_macros(macro_source(updated_source))
        original = restore_bypass_reservation_edges(reserved)
    except (TypeError, ValueError) as exc:
        return None, str(exc)
    _, _, reason = verified_bypass_reservations(frame, original)
    if reason:
        return None, "keepout rebind original rejected: " + reason
    candidate = replace(original, board=replace(updated,
        tracks=(*updated.tracks, *original.created_tracks),
        vias=(*updated.vias, *original.created_vias)))
    # Rematerialization changes immutable edge identities, never their values.
    try:
        candidate = restore_bypass_reservation_edges(candidate)
    except ValueError as exc:
        return None, str(exc)
    index = RoutingClearanceIndex(candidate.board)
    witnesses = []
    for old in candidate.decoupling_witnesses:
        measured = verify_crosslayer_decoupling(candidate.board, old.link,
            return_target=old.return_target, index=index)
        if measured.witness is None:
            return None, "keepout rebind path rejected: " + measured.reason
        witnesses.append(measured.witness)
    candidate = replace(candidate, decoupling_witnesses=tuple(witnesses))
    _, _, reason = verified_bypass_reservations(updated, candidate)
    return (candidate, None) if reason is None else (None, reason)

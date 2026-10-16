"""Fresh read-only admission of declared ordinary bypass contributions.

This recognizes existing source-associated geometry. It never generates copper,
accepts routing flags as proof, or grants ownership of an immutable prefix.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import fields

from .fanout import FanoutResult
from .physical import PhysicalBoard, RouteKind, TrackSegment, Via


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

"""Exact generated-owner proposals around disposable paired contact copper.

This creates ordinary launch reservations only. Callers spend an existing
bounded package-pattern trial and revalidate the complete critical/contact/
boundary pipeline before adopting a proposal. No probe copper is returned.
"""
from collections import Counter
from dataclasses import replace
from types import MappingProxyType

from .bypass_reservations import verified_bypass_reservations
from .drc import DrcSeverity, run_physical_drc
from .fanout import FanoutOptions, FanoutResult, route_fanout
from .hard_macros import validate_hard_macros
from .pad_via_arrays import require_via_in_pad_arrays
from .physical import PhysicalBoard, RouteKind, TrackSegment, Via
from .pin_escape import verified_fanout_path, verified_routing_access
from .routing_clearance import RoutingClearanceIndex


def _blockers(index, tracks, vias):
    found = set()
    for track in tracks:
        found.update(index.track_blocker_records(track))
    for via in vias:
        found.update(index.via_blocker_records(via))
    return frozenset(found)


def _hard_findings(board):
    return frozenset(f.fingerprint for f in run_physical_drc(board).findings
                     if f.severity is DrcSeverity.ERROR
                     and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})


def _valid_exits(board, result):
    names = {pad: net.name for net in board.nets for pad in net.pads}
    index = RoutingClearanceIndex(board)
    if any(pad not in names or verified_fanout_path(board, pad, names[pad], point, index) is None
           for pad, point in result.accesses.items()):
        return False
    return all(pad in names and verified_routing_access(board, pad, names[pad], access, index) is not None
               for pad, access in (result.surface_accesses or {}).items())


def regenerate_contact_owners(
    board: PhysicalBoard, baseline: FanoutResult, differential_group,
    contact_tracks: tuple[TrackSegment, ...], contact_vias: tuple[Via, ...],
    settings: FanoutOptions, *, maximum_owner_nets: int,
) -> FanoutResult | None:
    """Rebuild intersecting generated ordinary owners, preserving exact inputs.

    The contact proposal must come from the caller's existing paired search.
    Geometry identities distinguish generated launches from source/macro/plane/
    array copper, even when identical occurrences share a net. Net-granular
    release never assigns shared stubs to guessed pad owners. Original verified
    bypass associations require fresh exact witnesses in the complete trial.
    Late, already-reserved boundary owners require their own transaction and
    are conservatively unsupported here.
    """
    if maximum_owner_nets <= 0 or baseline.boundary_accesses:
        return None
    tracks, vias = tuple(contact_tracks), tuple(contact_vias)
    group = tuple(differential_group)
    rules = {rule.net: rule for rule in board.net_routing_rules}
    if len(group) != 2 or len(set(group)) != 2 or any(name not in rules for name in group):
        return None
    first, second = (rules[name] for name in group)
    if (first.kind is not RouteKind.DIFFERENTIAL or second.kind is not RouteKind.DIFFERENTIAL
            or first.differential_partner != second.net or second.differential_partner != first.net
            or not tracks or {track.net for track in tracks} != set(group)):
        return None
    return_nets = {rule.return_via_net for rule in (first, second) if rule.require_return_vias}
    if any(via.net not in set(group) | return_nets for via in vias):
        return None
    if (baseline.board.tracks != (*board.tracks, *baseline.created_tracks)
            or baseline.board.vias != (*board.vias, *baseline.created_vias)
            or replace(baseline.board, tracks=board.tracks, vias=board.vias) != board
            or Counter(baseline.decoupling_tracks) - Counter(baseline.created_tracks)):
        return None
    if verified_bypass_reservations(board, baseline)[2] is not None:
        return None
    try:
        validate_hard_macros(board)
        require_via_in_pad_arrays(board, "contact-owner proposal")
    except ValueError:
        return None
    if not _valid_exits(baseline.board, baseline):
        return None
    source_index = RoutingClearanceIndex(board)
    # Locked bits in the combined index are not mutability authority. Source
    # identities always remain protected; only the explicit fanout contribution
    # can authorize regeneration of a geometrically equal generated owner.
    if _blockers(source_index, tracks, vias):
        return None
    baseline_records = _blockers(RoutingClearanceIndex(baseline.board), tracks, vias)
    generated_index = RoutingClearanceIndex(board)
    for track in baseline.created_tracks:
        generated_index.add_track(track, locked=True)
    for via in baseline.created_vias:
        generated_index.add_via(via, locked=True)
    generated_records = _blockers(generated_index, tracks, vias)
    generated_identities = {record.identity for record in generated_records}
    if (not baseline_records or any(record.identity not in generated_identities or record.net is None
                                    for record in baseline_records)):
        return None
    selected = frozenset(record.net for record in baseline_records)
    created_names = {item.net for item in (*baseline.created_tracks, *baseline.created_vias)}
    if (len(selected) > maximum_owner_nets or not selected <= created_names
            or any(name in rules and rules[name].kind is not RouteKind.GENERAL for name in selected)):
        return None
    # Import lazily because the package controller integrates this proposal.
    # This is the existing exact predicate, not a new semantic interpretation.
    from .package_access import _verified_bypass_links
    names = {pad: net.name for net in board.nets for pad in net.pads}
    all_names = frozenset(net.name for net in board.nets)
    baseline_paths, baseline_via_paths = {}, {}
    required_bypass = _verified_bypass_links(baseline.board, all_names,
        witness_paths=baseline_paths, witness_vias=baseline_via_paths,
        admitted_witnesses=baseline.decoupling_witnesses)
    generated_tracks = frozenset(baseline.created_tracks)
    generated_vias = frozenset(baseline.created_vias)
    # A return owner shared with an unselected supply cohort cannot be released
    # independently. Never enlarge the measured owner set to make it mutable.
    if any(link.net not in selected and (
            any(t in generated_tracks and t.net in selected for t in baseline_paths.get(link, ()))
            or any(v in generated_vias and v.net in selected for v in baseline_via_paths.get(link, ())))
           for link in required_bypass):
        return None
    kept_tracks = tuple(track for track in baseline.created_tracks if track.net not in selected)
    kept_vias = tuple(via for via in baseline.created_vias if via.net not in selected)
    probe = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))
    before_hard = _hard_findings(board)
    if _hard_findings(probe) - before_hard:
        return None
    held = replace(probe, tracks=(*probe.tracks, *kept_tracks), vias=(*probe.vias, *kept_vias))
    try:
        rebuilt = route_fanout(held, settings, only_nets=selected)
    except ValueError:
        return None  # An invalid trial never acquires source ownership.
    if (rebuilt.board.tracks != (*held.tracks, *rebuilt.created_tracks)
            or rebuilt.board.vias != (*held.vias, *rebuilt.created_vias)
            or replace(rebuilt.board, tracks=held.tracks, vias=held.vias) != held
            or Counter(rebuilt.decoupling_tracks) - Counter(rebuilt.created_tracks)):
        return None
    if verified_bypass_reservations(held, rebuilt)[2] is not None:
        return None
    rebuilt_paths, rebuilt_via_paths = {}, {}
    current_bypass = _verified_bypass_links(rebuilt.board, all_names,
        witness_paths=rebuilt_paths, witness_vias=rebuilt_via_paths,
        admitted_witnesses=rebuilt.decoupling_witnesses)
    declared_tracks = frozenset(rebuilt.decoupling_tracks)
    declared_vias = frozenset(getattr(rebuilt, "decoupling_vias", ()))
    returned_tracks = {t for link in current_bypass if link.net in selected
                       for t in rebuilt_paths.get(link, ()) if t.net == link.return_net}
    returned_vias = {v for link in current_bypass if link.net in selected
                     for v in rebuilt_via_paths.get(link, ()) if v.net == link.return_net}
    # Reconstructed generated edges of a selected association must retain their
    # explicit semantic ownership, including its return vias. A complete path
    # alone cannot silently turn a declared bypass edge into ordinary copper.
    rebuilt_generated_tracks = frozenset(rebuilt.created_tracks)
    rebuilt_generated_vias = frozenset(rebuilt.created_vias)
    if any(link.net in selected and (
            any(t in rebuilt_generated_tracks and t not in declared_tracks
                for t in rebuilt_paths.get(link, ()))
            or any(v in rebuilt_generated_vias and v not in declared_vias
                   for v in rebuilt_via_paths.get(link, ())))
           for link in current_bypass):
        return None
    if (any(t.net not in selected and (t not in declared_tracks or t not in returned_tracks)
            for t in rebuilt.created_tracks)
            or any(v.net not in selected and (v not in declared_vias or v not in returned_vias)
                   for v in rebuilt.created_vias)
            or not required_bypass <= current_bypass
            or _hard_findings(rebuilt.board) - before_hard):
        return None
    combined_tracks = (*kept_tracks, *rebuilt.created_tracks)
    combined_vias = (*kept_vias, *rebuilt.created_vias)
    def keep(mapping):
        return {pad: value for pad, value in (mapping or {}).items() if names.get(pad) not in selected}
    proposed = replace(rebuilt,
        board=replace(board, tracks=(*board.tracks, *combined_tracks), vias=(*board.vias, *combined_vias)),
        accesses=MappingProxyType({**keep(baseline.accesses), **rebuilt.accesses}),
        surface_accesses=MappingProxyType({**keep(baseline.surface_accesses), **(rebuilt.surface_accesses or {})}),
        boundary_accesses=None,
        pending_pads=tuple(pad for pad in baseline.pending_pads if names.get(pad) not in selected)
                     + rebuilt.pending_pads,
        created_tracks=combined_tracks, created_vias=combined_vias,
        added_track_count=len(combined_tracks), added_via_count=len(combined_vias),
        pin_analysis=tuple(item for item in baseline.pin_analysis if names.get(item.pad) not in selected)
                     + rebuilt.pin_analysis,
        decoupling_pending=tuple(item for item in baseline.decoupling_pending if names.get(item[0]) not in selected)
                           + rebuilt.decoupling_pending,
        decoupling_tracks=tuple(track for track in baseline.decoupling_tracks if track.net not in selected)
                          + rebuilt.decoupling_tracks,
        decoupling_vias=tuple(via for via in getattr(baseline, "decoupling_vias", ()) if via.net not in selected)
                        + getattr(rebuilt, "decoupling_vias", ()))
    from .decoupling_paths import refresh_crosslayer_witnesses
    replacing = frozenset(link for link in required_bypass if link.net in selected)
    inherited, reason = refresh_crosslayer_witnesses(proposed.board,baseline.decoupling_witnesses,
                                                    replacing_links=replacing)
    if reason:
        return None
    merged, reason = refresh_crosslayer_witnesses(proposed.board,(*inherited,*rebuilt.decoupling_witnesses))
    if reason:
        return None
    proposed = replace(proposed,decoupling_witnesses=merged)
    if (verified_bypass_reservations(board, proposed)[2] is not None
            or not baseline.escaped_pads <= proposed.escaped_pads
            or not {item.pad for item in baseline.pin_analysis} <= {item.pad for item in proposed.pin_analysis}
            or not set(proposed.pending_pads) <= set(baseline.pending_pads)
            or not _valid_exits(rebuilt.board, proposed)
            or not _valid_exits(proposed.board, proposed)
            or not required_bypass <= _verified_bypass_links(proposed.board, all_names)
            or _hard_findings(proposed.board) - before_hard):
        return None
    # The immutable prefix and every unrelated generated occurrence survive;
    # contact tracks/vias are never appended to ordinary ownership.
    if (proposed.board.tracks[:len(board.tracks)] != board.tracks
            or proposed.board.vias[:len(board.vias)] != board.vias
            or Counter(kept_tracks) - Counter(proposed.created_tracks)
            or Counter(kept_vias) - Counter(proposed.created_vias)
            or Counter(t for t in baseline.decoupling_tracks if t.net not in selected)
               - Counter(proposed.decoupling_tracks)
            or Counter(v for v in getattr(baseline, "decoupling_vias", ()) if v.net not in selected)
               - Counter(getattr(proposed, "decoupling_vias", ()))
            or any(proposed.accesses.get(pad) != value for pad, value in keep(baseline.accesses).items())
            or any((proposed.surface_accesses or {}).get(pad) != value
                   for pad, value in keep(baseline.surface_accesses).items())):
        return None
    return proposed

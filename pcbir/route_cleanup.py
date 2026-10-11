"""Conservative cleanup of *owned* ordinary tracks, never input copper.

Split exact centre-line junctions and overlapping runs before leaf pruning.
Pads, vias, immutable copper and ambiguous off-centre contacts are protected.
Zone outlines are not connectivity evidence; zone-net tracks are untouched.
No new copper, shortcut, via or relaxed clearance is introduced here.
"""
from collections import Counter, defaultdict, deque
from dataclasses import dataclass, replace
from math import isqrt

from .copper_connectivity import CopperContact, copper_contacts_overlap, via_copper_contact
from .geometry import (RoundedConvexShape, SpatialIndex, SpatialItem,
                       orientation, point_on_segment, shapes_clear)
from .physical import BoardSide, CopperLayer, PadKind, PhysicalBoard, Point, TrackSegment, Via
from .routing_clearance import RoutingClearanceIndex


def ripup_mutable_nets(board: PhysicalBoard, names: frozenset[str], *,
                       mutable_tracks: Counter, mutable_vias: Counter) -> PhysicalBoard:
    """Remove explicitly owned ordinary routes for a subsequent repair stage.

    This is an incomplete routing transaction, not connectivity evidence.
    Unknown, critical, plane and immutable copper must never be evicted here.
    """
    from .physical import RouteKind
    from .zone_geometry import distribution_zone_nets
    if not names or not names <= {n.name for n in board.nets}:
        raise ValueError('ripup requires known net names')
    rules = {r.net: r for r in board.net_routing_rules}
    if names & distribution_zone_nets(board) or any(
            name in rules and rules[name].kind is not RouteKind.GENERAL for name in names):
        raise ValueError('ripup supports ordinary non-plane nets only')
    tracks = tuple(t for t in board.tracks if t.net in names)
    vias = tuple(v for v in board.vias if v.net in names)
    if Counter(tracks) - mutable_tracks or Counter(vias) - mutable_vias:
        raise ValueError('ripup cannot remove immutable input copper')
    result = replace(board, tracks=tuple(t for t in board.tracks if t.net not in names),
                     vias=tuple(v for v in board.vias if v.net not in names),
                     metadata={**board.metadata, 'detailed_routing': 'partial',
                               'fabrication_ready': 'false'})
    from .hard_macros import validate_hard_macros
    from .pad_via_arrays import require_via_in_pad_arrays
    validate_hard_macros(result)
    require_via_in_pad_arrays(result, 'ordinary net ripup')
    return result


def remove_redundant_ordinary_copper(board: PhysicalBoard, tracks: tuple[TrackSegment, ...],
                                     vias: tuple[Via, ...] = (), *,
                                     mutable_tracks: Counter, mutable_vias: Counter) -> PhysicalBoard | None:
    """Trial-delete identified ordinary copper only with exact pad connectivity.

    Unlike leaf pruning, this can prove an entire obsolete overlapping escape
    redundant. None means the proof failed; the input is always preserved.
    Filled-plane contacts, critical routes, macros and required arrays stay out.
    """
    from .physical import RouteKind
    from .zone_geometry import distribution_zone_nets
    from .drc import explicit_copper_connectivity
    from .hard_macros import validate_hard_macros
    from .pad_via_arrays import require_via_in_pad_arrays
    names = frozenset(t.net for t in (*tracks, *vias))
    rules = {r.net: r for r in board.net_routing_rules}
    if not names or names & distribution_zone_nets(board) or any(
            name in rules and rules[name].kind is not RouteKind.GENERAL for name in names):
        raise ValueError('redundant cleanup supports ordinary non-plane copper only')
    if (Counter(tracks) - Counter(board.tracks) or Counter(vias) - Counter(board.vias)
            or Counter(tracks) - mutable_tracks or Counter(vias) - mutable_vias):
        raise ValueError('redundant cleanup requires exact mutable input copper')
    nets = tuple(n for n in board.nets if n.name in names)
    if len(nets) != len(names):
        raise ValueError('redundant cleanup requires known nets')
    before = explicit_copper_connectivity(board, only_nets=names)
    if not all(before.net_connected(n) for n in nets):
        return None
    remaining_tracks, remaining_vias = Counter(tracks), Counter(vias)
    def retained(objects, remaining):
        result = []
        for item in objects:
            if remaining[item]:
                remaining[item] -= 1
            else:
                result.append(item)
        return tuple(result)
    candidate = replace(board, tracks=retained(board.tracks, remaining_tracks),
                        vias=retained(board.vias, remaining_vias))
    validate_hard_macros(candidate)
    require_via_in_pad_arrays(candidate, 'redundant ordinary copper cleanup')
    after = explicit_copper_connectivity(candidate, only_nets=names)
    return candidate if all(after.net_connected(n) for n in nets) else None


def prune_track_stubs(
    board: PhysicalBoard, tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...] = (), *, clearance: RoutingClearanceIndex | None = None,
    fixed_tracks: tuple[TrackSegment, ...] | None = None,
    fixed_vias: tuple[Via, ...] | None = None, trim_overhangs: bool = False,
) -> tuple[TrackSegment, ...]:
    """Prune unanchored leaves in supplied tracks; board copper is immutable.

    Keep uncertain physical contacts instead of mistaking pad-edge connections
    or an interior branch for a stub. Vias are intentionally not deleted: plane
    stitching and reference-via ownership belong to their respective stages.
    ``fixed_tracks``/``fixed_vias`` replace the board's own copper as the
    immutable context; pads still come from ``board`` (or ``clearance``).
    With ``trim_overhangs``, owned runs are also split at same-net land
    centres, and a piece whose only contact is one same-net land (at a land
    centre end) or one via (at its centre end) is an overhang: only that end
    is protected, so a dead end beyond the land or via can be pruned.
    Pieces of an ordinary breakout net that touch only through the width of
    collinear pieces between them are joined by those pieces: such cuts are
    neck-down boundaries (plan R1), not uncertain contacts.
    """
    if not tracks:
        return tracks
    from .zone_geometry import distribution_zone_nets
    zone_nets = distribution_zone_nets(board)
    groups = defaultdict(list)
    for track in tracks:
        groups[track.net, track.layer].append(track)
    clearance = clearance or RoutingClearanceIndex(board)
    fixed_tracks = board.tracks if fixed_tracks is None else fixed_tracks
    fixed_vias = board.vias if fixed_vias is None else fixed_vias
    result = []
    layers = board.stackup.copper_layers
    for (net, layer), owned in groups.items():
        if net in zone_nets:
            result.extend(owned)
            continue
        locked = tuple(t for t in fixed_tracks if t.net == net and t.layer == layer)
        contacts = tuple(v for v in (*fixed_vias, *vias) if v.net == net
                         and min(layers.index(v.from_layer), layers.index(v.to_layer))
                         <= layers.index(layer)
                         <= max(layers.index(v.from_layer), layers.index(v.to_layer)))
        points = {p for t in (*owned, *locked) for p in (t.start, t.end)}
        points.update(v.position for v in contacts)
        centres = _land_centres(board, net, layer) if trim_overhangs else {}
        points.update(p for found in centres.values() for p in found)
        ordered_points = sorted(points, key=lambda p: (p.x_nm, p.y_nm))
        point_index = SpatialIndex(SpatialItem(str(i), RoundedConvexShape((p,)).bounds)
                                   for i, p in enumerate(ordered_points))
        pieces = {}
        original_keys = []
        for track in owned:
            # All cuts lie on the original segment, so its copper union stays
            # identical. Canonical keys also collapse reversed duplicate runs.
            nearby = (ordered_points[int(i)] for i in point_index.query(
                RoundedConvexShape((track.start, track.end)).bounds))
            cuts = sorted((p for p in nearby if point_on_segment(p, track.start, track.end)),
                          key=lambda p: (p.x_nm, p.y_nm))
            keys = []
            for start, end in zip(cuts, cuts[1:]):
                key = (start, end, track.width_nm)
                keys.append(key)
                pieces.setdefault(key, TrackSegment(net, start, end, track.width_nm, layer))
            original_keys.append(keys)
        active = list(pieces.values())
        # Round radii *up* here: uncertain half-nanometre contacts must be kept.
        shapes = [RoundedConvexShape((t.start, t.end), (t.width_nm + 1) // 2) for t in active]
        fixed_shapes = [RoundedConvexShape((t.start, t.end), (t.width_nm + 1) // 2) for t in locked]
        fixed_shapes.extend(RoundedConvexShape((v.position,), (v.size_nm + 1) // 2) for v in contacts)
        fixed_index = SpatialIndex(SpatialItem(str(i), s.bounds) for i, s in enumerate(fixed_shapes))
        shape_index = SpatialIndex(SpatialItem(str(i), s.bounds) for i, s in enumerate(shapes))
        protected = set()
        adjacency = defaultdict(set)
        edge_ids = {}
        edge_members = defaultdict(set)
        for index, track in enumerate(active):
            # Different-width pieces on the same centre line are one graph
            # edge, not two connections that could hide a retraced dead end.
            edge = edge_ids.setdefault((track.start, track.end), index)
            edge_members[edge].add(index)
            adjacency[track.start].add(edge)
            adjacency[track.end].add(edge)
            shape = shapes[index]
            # Protect whole pad-contacting pieces, including mid-segment lands.
            if not trim_overhangs:
                if (not clearance.pad_copper_clear(shape, (layer,))
                        or any(not shapes_clear(shape, fixed_shapes[int(i)], 1)
                               for i in fixed_index.query(shape.bounds.expanded(1)))):
                    protected.update((track.start, track.end))
                continue
            lands = [item for item in clearance._overlapping_objects(shape, (layer,))
                     if item.is_pad and not shapes_clear(shape, item.shape, 1)]
            fixed = [int(i) for i in fixed_index.query(shape.bounds.expanded(1))
                     if not shapes_clear(shape, fixed_shapes[int(i)], 1)]
            if not lands and not fixed:
                continue
            anchors = set()
            if len(lands) + len(fixed) == 1:
                if lands and lands[0].net == net:
                    anchors = centres.get(lands[0].pad_reference, set())
                elif fixed and fixed[0] >= len(locked):
                    anchors = {contacts[fixed[0] - len(locked)].position}
            ends = {track.start, track.end} & anchors
            protected.update(ends if len(ends) == 1 else (track.start, track.end))
        for index, first in enumerate(active):
            for identity in shape_index.query(shapes[index].bounds.expanded(1)):
                other = int(identity)
                if other <= index:
                    continue
                second = active[other]
                if {first.start, first.end}.intersection((second.start, second.end)):
                    continue  # Exact split junction is represented by adjacency.
                if not shapes_clear(shapes[index], shapes[other], 1):
                    if (net in clearance.breakout.ordinary
                            and _bridged(first, second, active, adjacency)):
                        continue
                    # Non-grid crossings/annular or width-only contacts are not
                    # safely represented by the centre-line graph. Preserve them.
                    protected.update((first.start, first.end, second.start, second.end))
        pending = deque(sorted((p for p, edges in adjacency.items()
                                if len(edges) == 1 and p not in protected),
                               key=lambda p: (p.x_nm, p.y_nm)))
        removed = set()
        while pending:
            point = pending.popleft()
            if point in protected or len(adjacency[point]) != 1:
                continue
            index = next(iter(adjacency[point]))
            removed.update(edge_members[index])
            track = active[index]
            for endpoint in (track.start, track.end):
                adjacency[endpoint].discard(index)
                if len(adjacency[endpoint]) == 1 and endpoint not in protected:
                    pending.append(endpoint)
        retained = {key for index, key in enumerate(pieces) if index not in removed}
        emitted = set()
        for original, keys in zip(owned, original_keys):
            remaining = [key for key in keys if key in retained and key not in emitted]
            # Preserve original segment identity/orientation when its entire
            # copper survives. Reserved access proofs refer to exact segments.
            if len(remaining) == len(keys):
                result.append(original)
            else:
                result.extend(pieces[key] for key in remaining)
            emitted.update(remaining)
    return tuple(result)


def _bridged(first: TrackSegment, second: TrackSegment, active, adjacency) -> bool:
    """Whether collinear pieces join two disjoint pieces of one centre line."""
    a, b = first.start, first.end
    if orientation(a, b, second.start) or orientation(a, b, second.end):
        return False
    dx, dy = b.x_nm - a.x_nm, b.y_nm - a.y_nm

    def along(point: Point) -> int:
        return (point.x_nm - a.x_nm) * dx + (point.y_nm - a.y_nm) * dy
    low, high = sorted((first.start, first.end), key=along)
    other_low, other_high = sorted((second.start, second.end), key=along)
    if along(high) <= along(other_low):
        cursor, target = high, other_low
    elif along(other_high) <= along(low):
        cursor, target = other_high, low
    else:
        return False  # Overlapping runs, not a cut.
    sign = 1 if along(target) > along(cursor) else -1
    while cursor != target:
        step = None
        for edge in adjacency.get(cursor, ()):
            piece = active[edge]
            far = piece.end if piece.start == cursor else piece.start
            if (not orientation(a, b, far)
                    and 0 < sign * (along(far) - along(cursor))
                    and sign * (along(far) - along(target)) <= 0):
                step = far
                break
        if step is None:
            return False
        cursor = step
    return True


def _land_centres(board: PhysicalBoard, net: str, layer: CopperLayer) -> dict:
    """Centres of ``net``'s physical lands on ``layer``, by land reference."""
    from .physical import PadReference
    from .placement import transformed_local_point
    numbers: dict[str, set[str]] = {}
    for item in board.nets:
        if item.name == net:
            for pad in item.pads:
                numbers.setdefault(pad.component, set()).add(pad.pad)
    centres: dict = {}
    for placement in board.placements:
        if placement.reference not in numbers:
            continue
        footprint = board.footprints[placement.footprint]
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        assigned = set(numbers[placement.reference])
        for group in footprint.internal_pad_groups:
            if assigned.intersection(group.numbers):
                assigned.update(group.numbers)
        for pad in footprint.pads:
            if pad.number in assigned and (pad.kind is PadKind.THROUGH_HOLE
                                           or pad.kind is PadKind.SMD and layer is side):
                centres.setdefault(PadReference(placement.reference, pad.number), set()).add(
                    transformed_local_point(placement, pad.position))
    return centres


# Released copper must not leave connectivity that depends on a grazing touch:
# every contact is judged with tracks and vias shrunk by this much.
CONTACT_MARGIN_NM = 1_000


@dataclass(frozen=True, slots=True)
class EscapeChain:
    """Owned copper joining one pad to its reserved package port (CS-153).

    ``land`` runs from the pad to the launch, ``witness`` from the launch to
    the port; ``via`` is the owned launch via, if any.
    """
    net: str
    land: tuple[TrackSegment, ...]
    witness: tuple[TrackSegment, ...]
    via: Via | None = None


def release_unused_escapes(
    board: PhysicalBoard, tracks: tuple[TrackSegment, ...], vias: tuple[Via, ...],
    chains: tuple[EscapeChain, ...], owned_tracks: Counter, *,
    mutable_tracks: Counter, removable_vias: Counter,
    clearance: RoutingClearanceIndex | None = None, settled: set[str] | None = None,
) -> tuple[tuple[TrackSegment, ...], tuple[Via, ...]]:
    """Remove reserved escape copper that a connected route does not need.

    ``tracks``/``vias`` are the complete final copper and ``board`` supplies
    pads, zones and stackup. Only occurrences in ``owned_tracks`` and each
    chain's own via may be released. Per net, the copper is first settled:
    leaves of ``mutable_tracks`` are pruned and ``removable_vias`` left with
    contacts on fewer than two layers are deleted, repeatedly. Then, per
    chain in the given order, try to release land, via and witness; else via
    and witness; else the witness, settling again after each trial. A trial
    is kept only if, with every contact judged on copper shrunk by
    ``CONTACT_MARGIN_NM``, the net stays connected, gains no copper island,
    and gains no open track end or single-layer via. Other nets are never
    read or changed; a net that is not robustly connected is left as it is.
    The names of settled nets are added to ``settled`` when it is given.
    """
    if not chains:
        return tracks, vias
    nets = {net.name: net for net in board.nets}
    clearance = clearance or RoutingClearanceIndex(board)
    order = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    by_net: dict[str, list[EscapeChain]] = defaultdict(list)
    for chain in chains:
        by_net[chain.net].append(chain)
    replaced: dict[str, tuple[list[TrackSegment], list[Via]]] = {}
    for name in sorted(by_net):
        net = nets.get(name)
        if net is None:
            continue
        pads = _net_pad_shapes(board, name)
        settle = _Settler(board, name, net, pads, order, clearance,
                          Counter({t: c for t, c in mutable_tracks.items() if t.net == name}),
                          Counter({v: c for v, c in removable_vias.items() if v.net == name}))
        # Trials remove exact chain occurrences from the unsettled copper.
        raw = ([t for t in tracks if t.net == name], [v for v in vias if v.net == name])
        current = settle(*raw)
        score = settle.score(*current)
        if score is None:
            continue
        available = Counter({t: c for t, c in owned_tracks.items() if t.net == name})
        for chain in by_net[name]:
            tiers = []
            for removal, via in (((*chain.land, *chain.witness), chain.via),
                                 (chain.witness, chain.via), (chain.witness, None)):
                if removal and (removal, via) not in tiers:
                    tiers.append((removal, via))
            for removal, via in tiers:
                wanted = Counter(removal)
                present = Counter(raw[0])
                if (any(wanted[t] > min(available[t], present[t]) for t in wanted)
                        or via is not None and via not in raw[1]):
                    continue
                trial_tracks = list(raw[0])
                for track in removal:
                    trial_tracks.remove(track)
                trial_vias = list(raw[1])
                if via is not None:
                    trial_vias.remove(via)
                trial = settle(trial_tracks, trial_vias)
                trial_score = settle.score(*trial)
                if trial_score is None or any(new > old for new, old in zip(trial_score, score)):
                    continue
                raw, current, score = (trial_tracks, trial_vias), trial, trial_score
                available -= wanted
                break
        current, score = _prune_open_ends(settle, current, score, trimmable=available)
        replaced[name] = _cut_cycles(settle, current, score)
        if settled is not None:
            settled.add(name)
    if not replaced:
        return tracks, vias
    # Keep every other net's occurrences in their original order.
    remaining_tracks = {name: Counter(copper[0]) for name, copper in replaced.items()}
    remaining_vias = {name: Counter(copper[1]) for name, copper in replaced.items()}
    kept_tracks, kept_vias = [], []
    for track in tracks:
        budget = remaining_tracks.get(track.net)
        if budget is None:
            kept_tracks.append(track)
        elif budget[track]:
            budget[track] -= 1
            kept_tracks.append(track)
    for name, budget in sorted(remaining_tracks.items()):
        kept_tracks.extend(budget.elements())  # Pruned pieces of split tracks.
    for via in vias:
        budget = remaining_vias.get(via.net)
        if budget is None:
            kept_vias.append(via)
        elif budget[via]:
            budget[via] -= 1
            kept_vias.append(via)
    return tuple(kept_tracks), tuple(kept_vias)


def _prune_open_ends(settle: "_Settler", current, score, *, rounds: int = 8,
                     trimmable: Counter | None = None):
    """Remove owned pad-contacting leaves only after proving robust connectivity.

    The centre-line pruner deliberately keeps ambiguous pad-edge contacts,
    including coincident lands. The complete-net score can establish whether
    such a piece is redundant without weakening that conservative local test.

    A ``trimmable`` (owned escape) track whose open end runs past a land of
    its net that it crosses, for example a dogbone whose via was deleted
    because the route reached that land, is cut back to the land's centre
    instead (``_trimmed_to_land``).
    """
    trimmable = Counter(trimmable or ())
    for _ in range(rounds):
        if not score[1]:
            break
        budget = Counter(settle.mutable)
        spare = Counter(trimmable)
        candidates = set(settle.open_track_ends(*current))
        for index, track in enumerate(current[0]):
            owned = bool(budget[track])
            budget[track] -= owned
            escape = not owned and bool(spare[track])
            spare[track] -= escape
            if not (owned or escape) or index not in candidates:
                continue
            rest = current[0][:index] + current[0][index + 1:]
            trials = ([[*rest, piece] for piece in _trimmed_to_land(settle, track)]
                      if escape else [rest])
            for tracks in trials:
                trial = settle(tracks, current[1])
                trial_score = settle.score(*trial)
                if (trial_score is not None and trial_score[1] < score[1]
                        and all(new <= old for new, old in zip(trial_score, score))):
                    break
            else:
                continue
            current, score = trial, trial_score
            trimmable -= Counter((track,))
            break
        else:
            break
    return current, score


def trim_dangling_overhangs(board: PhysicalBoard, protected: Counter) -> PhysicalBoard:
    """Cut routed tracks back to the land they cross when their far end is open.

    A final sweep over ordinary nets: a track with an open end that crosses a
    land of its net (for example a fanout dogbone whose launch via a later
    stage deleted because the route already reached that land) keeps only its
    piece up to that land's centre (``_prune_open_ends``). A net changes only
    if it stays connected and loses open ends. ``protected`` occurrences
    (input and hard-macro copper) and critical, zone and incomplete nets
    never change.
    """
    from .physical import RouteKind

    skip = {zone.net for zone in board.zones} | {
        rule.net for rule in board.net_routing_rules if rule.kind is not RouteKind.GENERAL}
    nets = {net.name: net for net in board.nets}
    order = {layer: index for index, layer in enumerate(board.stackup.copper_layers)}
    by_net: dict[str, list[TrackSegment]] = defaultdict(list)
    for track in board.tracks:
        by_net[track.net].append(track)
    clearance = None
    replaced: dict[str, list[TrackSegment]] = {}
    for name in sorted(by_net):
        owned = Counter(by_net[name]) - protected
        if name in skip or name not in nets or not owned:
            continue
        clearance = clearance or RoutingClearanceIndex(board)
        settle = _Settler(board, name, nets[name], _net_pad_shapes(board, name), order, clearance,
                          Counter(), Counter())
        current = (list(by_net[name]), [via for via in board.vias if via.net == name])
        score = settle.score(*current)
        if score is None or not score[1]:
            continue
        trimmed, trimmed_score = _prune_open_ends(settle, current, score, trimmable=owned)
        if trimmed_score[1] < score[1]:
            replaced[name] = list(trimmed[0])
    if not replaced:
        return board
    tracks = [track for track in board.tracks if track.net not in replaced]
    for name in sorted(replaced):
        tracks.extend(replaced[name])
    return replace(board, tracks=tuple(tracks))


def _trimmed_to_land(settle: "_Settler", track: TrackSegment) -> list[TrackSegment]:
    """``track`` cut back to the centre of each same-net land it crosses.

    Each piece keeps one end of ``track`` and stops where the land's centre
    projects onto it, on the same octilinear direction in whole nanometres;
    lands at either end are skipped.
    """
    pieces = []
    shape = _robust_shape(track)
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    steps = max(abs(dx), abs(dy))
    if not steps or (dx and dy and abs(dx) != abs(dy)):
        return pieces
    for layers, pad in settle.pads:
        if track.layer not in layers or shapes_clear(shape, pad, 1):
            continue
        cx = sum(p.x_nm for p in pad.spine) // len(pad.spine)
        cy = sum(p.y_nm for p in pad.spine) // len(pad.spine)
        # Projection onto the segment, counted in octilinear steps.
        ux, uy = (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)
        along = ((cx - track.start.x_nm) * ux + (cy - track.start.y_nm) * uy) // (abs(ux) + abs(uy))
        if not 0 < along < steps:
            continue
        cut = Point(track.start.x_nm + ux * along, track.start.y_nm + uy * along)
        if shapes_clear(RoundedConvexShape((cut,), 0), pad, 1):
            continue  # The centre projects outside the land: an edge contact.
        for keep in (track.start, track.end):
            piece = replace(track, start=keep, end=cut)
            if piece not in pieces:
                pieces.append(piece)
    return pieces


def _cut_cycles(settle: "_Settler", current, score, *, rounds: int = 8):
    """Remove redundant disposable branches that close same-net loops.

    Each round tries every disposable track carrying a cycle edge of the
    centre-line graph (lands and vias merge their contacts); the settled
    removal with the shortest remaining copper wins if it lowers the cycle
    rank and does not worsen the robust score. Bounded and deterministic.
    """
    for _ in range(rounds):
        rank, cyclic = _cycle_edges(settle, *current)
        if not rank:
            break
        best = None
        budget = Counter(settle.mutable)
        for index, track in enumerate(current[0]):
            if not budget[track] or not cyclic[index]:
                continue
            trial_tracks = current[0][:index] + current[0][index + 1:]
            trial = settle(trial_tracks, current[1])
            trial_score = settle.score(*trial)
            if (trial_score is None or any(new > old for new, old in zip(trial_score, score))
                    or _cycle_edges(settle, *trial)[0] >= rank):
                continue
            length = sum(isqrt((t.end.x_nm - t.start.x_nm) ** 2 + (t.end.y_nm - t.start.y_nm) ** 2)
                         for t in trial[0])
            if best is None or length < best[0]:
                best = (length, trial, trial_score)
        if best is None:
            break
        _, current, score = best
    return current


def _cycle_edges(settle: "_Settler", tracks, vias) -> tuple[int, list[bool]]:
    """Cycle rank of the net's centre-line graph and, per track, whether one
    of its pieces lies on a cycle (is not a bridge)."""
    points = defaultdict(set)
    for track in tracks:
        points[track.layer].update((track.start, track.end))
    for via in vias:
        for layer in settle.order:
            if settle.covers(via, layer):
                points[layer].add(via.position)
    parent: dict = {}

    def find(node):
        parent.setdefault(node, node)
        while parent[node] != node:
            parent[node] = parent[parent[node]]
            node = parent[node]
        return node

    def union(first, second):
        first, second = find(first), find(second)
        if first != second:
            parent[max(first, second, key=repr)] = min(first, second, key=repr)
    pieces = []
    for index, track in enumerate(tracks):
        cuts = sorted((p for p in points[track.layer] if point_on_segment(p, track.start, track.end)),
                      key=lambda p: (p.x_nm, p.y_nm))
        pieces.extend((index, (track.layer, a), (track.layer, b)) for a, b in zip(cuts, cuts[1:]))
    nodes = {node for _, a, b in pieces for node in (a, b)}
    for node in sorted(nodes, key=repr):
        find(node)
    for via in vias:
        for layer in settle.order:
            if (layer, via.position) in nodes:
                union(("via", via.position), (layer, via.position))
    for number, (layers, shape) in enumerate(settle.pads):
        for layer, point in nodes:
            if layer in layers and not shapes_clear(shape, RoundedConvexShape((point,)), 1):
                union(("land", number), (layer, point))
    edges = [(index, find(a), find(b)) for index, a, b in pieces]
    adjacency = defaultdict(list)
    for number, (_, a, b) in enumerate(edges):
        if a != b:
            adjacency[a].append((b, number))
            adjacency[b].append((a, number))
    # Iterative bridge search (Tarjan) over the multigraph of pieces.
    order, low, bridges, counter = {}, {}, set(), 0
    for root in sorted(adjacency, key=repr):
        if root in order:
            continue
        order[root] = low[root] = counter
        counter += 1
        stack = [(root, None, iter(adjacency[root]))]
        while stack:
            node, via_edge, children = stack[-1]
            advanced = False
            for neighbour, edge in children:
                if edge == via_edge:
                    continue
                if neighbour in order:
                    low[node] = min(low[node], order[neighbour])
                else:
                    order[neighbour] = low[neighbour] = counter
                    counter += 1
                    stack.append((neighbour, edge, iter(adjacency[neighbour])))
                    advanced = True
                    break
            if not advanced:
                stack.pop()
                if stack:
                    parent_node = stack[-1][0]
                    low[parent_node] = min(low[parent_node], low[node])
                    if low[node] > order[parent_node]:
                        bridges.add(via_edge)
    loops = [number for number, (_, a, b) in enumerate(edges) if a != b]
    rank = len(loops) - len(adjacency) + _components(adjacency)
    cyclic = [False] * len(tracks)
    for number in loops:
        if number not in bridges:
            cyclic[edges[number][0]] = True
    return rank, cyclic


def _components(adjacency) -> int:
    seen, count = set(), 0
    for root in adjacency:
        if root in seen:
            continue
        count += 1
        stack = [root]
        seen.add(root)
        while stack:
            for neighbour, _ in adjacency[stack.pop()]:
                if neighbour not in seen:
                    seen.add(neighbour)
                    stack.append(neighbour)
    return count


class _Settler:
    """Cleanup simulation and robust contact score for one net's copper."""

    def __init__(self, board, name, net, pads, order, clearance, mutable, removable):
        self.board, self.name, self.net, self.pads = board, name, net, pads
        self.order, self.clearance = order, clearance
        self.mutable, self.removable = mutable, removable

    def __call__(self, tracks, vias):
        tracks, vias = list(tracks), list(vias)
        for _ in range(8):
            budget = Counter(self.mutable)
            mutable, fixed = [], []
            for track in tracks:
                if budget[track]:
                    budget[track] -= 1
                    mutable.append(track)
                else:
                    fixed.append(track)
            pruned = prune_track_stubs(self.board, tuple(mutable), clearance=self.clearance,
                                       fixed_tracks=tuple(fixed), fixed_vias=tuple(vias),
                                       trim_overhangs=True)
            tracks = [*fixed, *pruned]
            # Pieces of split tracks are as disposable as their originals.
            self.mutable += Counter(pruned) - Counter(mutable)
            budget = Counter(self.removable)
            dead = []
            for via in vias:
                if budget[via] and len(self.via_layers(via, tracks)) < 2:
                    budget[via] -= 1
                    dead.append(via)
            if not dead and Counter(pruned) == Counter(mutable):
                break
            for via in dead:
                vias.remove(via)
        return tracks, vias

    def via_layers(self, via, tracks):
        contact = via_copper_contact(via, self.board.stackup.copper_layers)
        layers = {t.layer for t in tracks if copper_contacts_overlap(contact,
                  CopperContact("track", t.net, (t.layer,), _robust_shape(t)))}
        contact = replace(contact, shape=_robust_via(via))
        layers.update(layer for pad_layers, pad in self.pads for layer in pad_layers
                      if copper_contacts_overlap(contact, CopperContact("pad", via.net, (layer,), pad)))
        return layers

    def covers(self, via, layer) -> bool:
        low, high = sorted((self.order[via.from_layer], self.order[via.to_layer]))
        return low <= self.order[layer] <= high

    def score(self, tracks, vias):
        """(islands, open track ends, single-layer vias), or None if open."""
        from .drc import explicit_copper_connectivity
        graph = explicit_copper_connectivity(
            self.board, only_nets=frozenset((self.name,)),
            tracks=tuple(replace(t, width_nm=max(1, t.width_nm - 2 * CONTACT_MARGIN_NM))
                         for t in tracks),
            vias=tuple(replace(v, size_nm=max(v.drill_nm + 1, v.size_nm - 2 * CONTACT_MARGIN_NM))
                       for v in vias))
        if not graph.net_connected(self.net):
            return None
        single = sum(len(self.via_layers(via, tracks)) < 2 for via in vias)
        return len(set(graph.roots.values())), len(self.open_track_ends(tracks, vias)), single

    def open_track_ends(self, tracks, vias):
        """Track indices, repeated if both robust endpoints are unconnected."""
        open_ends = []
        by_layer = defaultdict(list)
        for index, track in enumerate(tracks):
            by_layer[track.layer].append((index, _robust_shape(track)))
        for via in vias:
            for layer in self.order:
                if self.covers(via, layer):
                    by_layer[layer].append((-1, _robust_via(via)))
        for layers, pad in self.pads:
            for layer in layers:
                by_layer[layer].append((-1, pad))
        for index, track in enumerate(tracks):
            for point in (track.start, track.end):
                end = RoundedConvexShape((point,), max(1, track.width_nm // 2 - CONTACT_MARGIN_NM))
                if not any(other != index and not shapes_clear(end, shape, 1)
                           for other, shape in by_layer[track.layer]):
                    open_ends.append(index)
        return open_ends


def _robust_shape(track: TrackSegment) -> RoundedConvexShape:
    return RoundedConvexShape((track.start, track.end), max(1, track.width_nm // 2 - CONTACT_MARGIN_NM))


def _robust_via(via: Via) -> RoundedConvexShape:
    return RoundedConvexShape((via.position,), max(1, via.size_nm // 2 - CONTACT_MARGIN_NM))


def _net_pad_shapes(board: PhysicalBoard, net: str):
    """(layers, shape) of every physical land of ``net``, with group members."""
    from .drc import placed_pad_shape
    from .placement import transformed_local_point
    numbers: dict[str, set[str]] = {}
    for item in board.nets:
        if item.name == net:
            for pad in item.pads:
                numbers.setdefault(pad.component, set()).add(pad.pad)
    shapes = []
    for placement in board.placements:
        if placement.reference not in numbers:
            continue
        footprint = board.footprints[placement.footprint]
        side = CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK
        assigned = set(numbers[placement.reference])
        for group in footprint.internal_pad_groups:
            if assigned.intersection(group.numbers):
                assigned.update(group.numbers)  # Declared internal connections.
        for pad in footprint.pads:
            if (pad.number not in assigned
                    or pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}):
                continue
            layers = (side,) if pad.kind is PadKind.SMD else board.stackup.copper_layers
            shapes.append((layers, placed_pad_shape(
                transformed_local_point(placement, pad.position), pad, placement)))
    return shapes

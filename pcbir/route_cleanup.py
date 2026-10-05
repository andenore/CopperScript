"""Conservative cleanup of *owned* ordinary tracks, never input copper.

Split exact centre-line junctions and overlapping runs before leaf pruning.
Pads, vias, immutable copper and ambiguous off-centre contacts are protected.
Zone outlines are not connectivity evidence; zone-net tracks are untouched.
No new copper, shortcut, via or relaxed clearance is introduced here.
"""
from collections import defaultdict, deque

from .geometry import (RoundedConvexShape, SpatialIndex, SpatialItem,
                       point_on_segment, shapes_clear)
from .physical import PhysicalBoard, Point, TrackSegment, Via
from .routing_clearance import RoutingClearanceIndex


def prune_track_stubs(
    board: PhysicalBoard, tracks: tuple[TrackSegment, ...],
    vias: tuple[Via, ...] = (), *, clearance: RoutingClearanceIndex | None = None,
) -> tuple[TrackSegment, ...]:
    """Prune unanchored leaves in supplied tracks; board copper is immutable.

    Keep uncertain physical contacts instead of mistaking pad-edge connections
    or an interior branch for a stub. Vias are intentionally not deleted: plane
    stitching and reference-via ownership belong to their respective stages.
    """
    if not tracks:
        return tracks
    zone_nets = {zone.net for zone in board.zones}
    groups = defaultdict(list)
    for track in tracks:
        groups[track.net, track.layer].append(track)
    clearance = clearance or RoutingClearanceIndex(board)
    result = []
    layers = board.stackup.copper_layers
    for (net, layer), owned in groups.items():
        if net in zone_nets:
            result.extend(owned)
            continue
        locked = tuple(t for t in board.tracks if t.net == net and t.layer == layer)
        contacts = tuple(v for v in (*board.vias, *vias) if v.net == net
                         and min(layers.index(v.from_layer), layers.index(v.to_layer))
                         <= layers.index(layer)
                         <= max(layers.index(v.from_layer), layers.index(v.to_layer)))
        points = {p for t in (*owned, *locked) for p in (t.start, t.end)}
        points.update(v.position for v in contacts)
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
            if (not clearance.pad_copper_clear(shape, (layer,))
                    or any(not shapes_clear(shape, fixed_shapes[int(i)], 1)
                           for i in fixed_index.query(shape.bounds.expanded(1)))):
                protected.update((track.start, track.end))
        for index, first in enumerate(active):
            for identity in shape_index.query(shapes[index].bounds.expanded(1)):
                other = int(identity)
                if other <= index:
                    continue
                second = active[other]
                if {first.start, first.end}.intersection((second.start, second.end)):
                    continue  # Exact split junction is represented by adjacency.
                if not shapes_clear(shapes[index], shapes[other], 1):
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

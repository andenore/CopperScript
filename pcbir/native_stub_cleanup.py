"""Conservative trimming of exactly identified native track dead ends.

The proposal only removes copper outside declared same-net zone boundaries.
Width-only contacts allow only a whole-segment removal trial, never guessed
cuts. Immutable copper and critical nets stay intact.
Only the caller's fresh native refill can accept the proposal.
"""
from collections import Counter
from dataclasses import replace

from .geometry import (RoundedConvexShape, point_on_segment,
                       shapes_clear)
from .physical import Point, RouteKind


def _junctions(a, b, c, d):
    """Exact integer centre-line contacts; None means no representable junction."""
    rx, ry = b.x_nm-a.x_nm, b.y_nm-a.y_nm
    sx, sy = d.x_nm-c.x_nm, d.y_nm-c.y_nm
    denominator = rx*sy-ry*sx
    if not denominator:
        points = {p for p in (a,b,c,d) if point_on_segment(p,a,b) and point_on_segment(p,c,d)}
        return points or None
    numerator = (c.x_nm-a.x_nm)*sy-(c.y_nm-a.y_nm)*sx
    x, y = a.x_nm*denominator+rx*numerator, a.y_nm*denominator+ry*numerator
    if x % denominator or y % denominator:
        return None
    p = Point(x//denominator,y//denominator)
    return {p} if point_on_segment(p,a,b) and point_on_segment(p,c,d) else None


def native_stub_candidate(board, evidence, protected, *, diagnostics=None):
    """Return an exact copper-subset trial, never acceptance or fill evidence."""
    from .plane_verify import native_object_map
    from .hard_macros import resolved_macro_geometry
    from .routing_clearance import RoutingClearanceIndex
    from .route_cleanup import _land_centres
    from .drc import explicit_copper_connectivity

    def reject(reason, **details):
        if diagnostics is not None:
            diagnostics.append({'reason': reason, **details})
        return None

    objects = native_object_map(board)
    findings = evidence.native_findings
    if (not evidence.matches(board) or evidence.unconnected_count or evidence.island_count
            or not findings or len(findings)>64 or len(findings) != evidence.other_violation_count
            or any(f.category != 'violation' or f.type != 'track_dangling' or len(f.items)!=1
                   for f in findings)):
        return reject('native_evidence_not_eligible')
    fixed = protected.copy()
    for macro in board.hard_macros:
        if macro.cluster in board.materialized_macros:
            fixed |= Counter(resolved_macro_geometry(board,macro)[0])
    critical = {r.net for r in board.net_routing_rules if r.kind is not RouteKind.GENERAL}
    indices = set()
    for f in findings:
        item=f.items[0]
        if (objects.get(item.uuid)!=item or item.kind!='segment' or item.index is None
                or item.net in critical):
            return reject('unbound_or_critical_item', uuid=item.uuid)
        indices.add(item.index)
    if len(indices)!=len(findings):
        return reject('duplicate_native_item')
    clearance = RoutingClearanceIndex(board)
    layers = board.stackup.copper_layers
    replacements = {}
    for index in sorted(indices):
        track=board.tracks[index]
        if fixed[track]:
            return reject('protected_track', index=index)
        shape=RoundedConvexShape((track.start,track.end),(track.width_nm+1)//2)
        # Even a bounding-box overlap is left to the zone owner. This permits
        # remote explicit feeders, never changes a thermal or plane contact.
        if any(z.net==track.net and track.layer in z.layers
               and shape.bounds.intersects(RoundedConvexShape(z.outline.outer.vertices).bounds) for z in board.zones):
            return reject('declared_zone_contact', index=index)
        anchors=set()
        whole_only=False
        for other_index,other in enumerate(board.tracks):
            if other_index==index or other.net!=track.net or other.layer!=track.layer:
                continue
            other_shape=RoundedConvexShape((other.start,other.end),(other.width_nm+1)//2)
            if shapes_clear(shape,other_shape,1):
                continue
            points=_junctions(track.start,track.end,other.start,other.end)
            if points is None:
                whole_only=True
            else:
                anchors.update(points)
        for via in board.vias:
            low,high=sorted((layers.index(via.from_layer),layers.index(via.to_layer)))
            if via.net!=track.net or not low<=layers.index(track.layer)<=high:
                continue
            if shapes_clear(shape,RoundedConvexShape((via.position,),(via.size_nm+1)//2),1):
                continue
            if not point_on_segment(via.position,track.start,track.end):
                return reject('off_centre_via', index=index, via=str(via.position))
            anchors.add(via.position)
        centres=_land_centres(board,track.net,track.layer)
        for land in clearance._overlapping_objects(shape,(track.layer,)):
            if not land.is_pad or shapes_clear(shape,land.shape,1):
                continue
            points={p for p in centres.get(land.pad_reference,())
                    if point_on_segment(p,track.start,track.end)}
            if land.net!=track.net or not points:
                return reject('off_centre_pad', index=index, pad=str(land.pad_reference))
            anchors.update(points)
        if whole_only:
            # Do not invent a cut point for a width-only junction. A full
            # removal still needs unchanged explicit partitions below and a
            # completely clean, freshly matched native result at acceptance.
            replacements[index]=None
            continue
        if not anchors:
            return reject('no_anchors', index=index)
        dx,dy=track.end.x_nm-track.start.x_nm,track.end.y_nm-track.start.y_nm
        ordered=sorted(anchors,key=lambda p:(p.x_nm-track.start.x_nm)*dx+(p.y_nm-track.start.y_nm)*dy)
        replacements[index]=(None if len(ordered)==1 else replace(track,start=ordered[0],end=ordered[-1]))
    candidate=replace(board,tracks=tuple(replacements.get(i,t) for i,t in enumerate(board.tracks)
                                         if replacements.get(i,t) is not None))
    if candidate.tracks==board.tracks:
        return reject('no_removable_overhang')
    # Preserve every explicit pad partition, including partially plane-connected
    # nets. Fresh native verification separately proves all filled connections.
    names=frozenset(board.tracks[i].net for i in indices)
    before=explicit_copper_connectivity(board,only_nets=names)
    after=explicit_copper_connectivity(candidate,only_nets=names)
    for net in board.nets:
        if net.name not in names:
            continue
        nodes=tuple(node for pad in net.pads for node in before.pad_nodes.get(pad,()))
        if any(before.pad_nodes.get(pad)!=after.pad_nodes.get(pad) for pad in net.pads):
            return reject('changed_pad_inventory', net=net.name)
        for a in nodes:
            for b in nodes:
                if (before.roots[a]==before.roots[b]) != (after.roots[a]==after.roots[b]):
                    return reject('changed_explicit_partition', net=net.name)
    return candidate

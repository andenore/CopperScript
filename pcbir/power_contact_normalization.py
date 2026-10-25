"""Normalize prospective area contacts that serve only as planar junctions.

Only newly proposed vias are eligible. Source copper and required arrays remain
untouched, and the complete proposal still needs fresh native acceptance.
"""
from dataclasses import replace
from fractions import Fraction

from .copper_connectivity import copper_contacts_overlap, copper_contact_roots
from .drc import _explicit_copper_contacts
from .geometry import SpatialIndex, SpatialItem
from .physical import Point, TrackSegment


def normalize_planar_contacts(source, tracks, vias, options):
    """Replace a one-layer annular junction by checked centreline connections.

    Unsupported contact types, nonintegral projections, other-layer zones and
    blocked bridges preserve the original proposal. This finite pass visits at
    most ``maximum_new_contacts`` prospective vias once; it performs no maze
    search or source/contact-owner release.
    """
    from .power_geometry import _AreaClearance, _zone_geometry
    from shapely.geometry import Point as ShapePoint
    tracks, vias = tuple(tracks), tuple(vias)
    changed = 0
    for via in tuple(vias)[:options.maximum_new_contacts]:
        if via in source.vias:
            continue
        board = replace(source, tracks=(*source.tracks, *tracks), vias=(*source.vias, *vias))
        contacts, _, _ = _explicit_copper_contacts(board, only_nets=frozenset({via.net}),
                                                   include_internal_connections=False)
        index = SpatialIndex(SpatialItem(c.identity, c.shape.bounds) for c in contacts)
        by_id = {c.identity: c for c in contacts}
        identity = f'via:{len(source.vias)+vias.index(via)}'
        contact = by_id[identity]
        touching = tuple(by_id[i] for i in sorted(index.query(contact.shape.bounds))
                         if i != identity and copper_contacts_overlap(contact, by_id[i]))
        layers = {layer for c in touching for layer in c.layers}
        if (not touching or len(touching) > 32 or len(layers) != 1
                or any(not c.identity.startswith('track:') for c in touching)):
            continue
        layer = next(iter(layers))
        # Outlines do not prove conductivity, but can represent intended access
        # to another layer. Keep that transition until native fill resolves it.
        if any(z.net == via.net and any(l != layer for l in z.layers)
               and _zone_geometry(z).distance(ShapePoint(via.position.x_nm, via.position.y_nm)) <= via.size_nm/2
               for z in source.zones):
            continue
        rule = next((r for r in source.net_routing_rules if r.net == via.net), None)
        if rule and rule.allowed_layers and layer not in rule.allowed_layers:
            continue
        width = max(source.rules.minimum_track_width_nm, (rule.width_nm or 0) if rule else 0)
        additions = []
        for c in touching:
            a, b = c.shape.spine
            dx, dy = b.x_nm-a.x_nm, b.y_nm-a.y_nm
            if not dx and not dy:
                break
            t = min(Fraction(1), max(Fraction(0), Fraction(
                (via.position.x_nm-a.x_nm)*dx+(via.position.y_nm-a.y_nm)*dy, dx*dx+dy*dy)))
            x, y = a.x_nm+t*dx, a.y_nm+t*dy
            if x.denominator != 1 or y.denominator != 1:
                break
            point = Point(int(x), int(y))
            if ((point.x_nm-via.position.x_nm)**2+(point.y_nm-via.position.y_nm)**2)*4 > via.size_nm**2:
                break
            if point != via.position:
                additions.append(TrackSegment(via.net, via.position, point, width, layer))
        else:
            remaining = list(vias)
            remaining.remove(via)
            held = replace(board, vias=(*source.vias, *remaining))
            clearance = _AreaClearance(held, options)
            additions = tuple(dict.fromkeys(additions))
            if not all(clearance.can_track(t.net,t.start,t.end,t.width_nm,t.layer) for t in additions):
                continue
            trial = replace(held, tracks=(*held.tracks, *additions))
            objects, _, _ = _explicit_copper_contacts(trial, only_nets=frozenset({via.net}),
                                                       include_internal_connections=False)
            roots = copper_contact_roots(objects)
            # Every conductor previously joined by this annulus must still be
            # joined through real copper, not just equal net names.
            if len({roots[c.identity] for c in touching}) != 1:
                continue
            tracks, vias = (*tracks, *additions), tuple(remaining)
            changed += 1
    return tuple(tracks), tuple(vias), changed

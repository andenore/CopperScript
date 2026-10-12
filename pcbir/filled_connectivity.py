"""Conservative component grouping for fresh native-fill repair proposals.

This is a search filter, never a replacement for independent native DRC. Bare
copper contacts are shared with DRC; source-zone outlines and installed-part
internal edges do not join components. Unsupported fills remain ungrouped.
"""
from math import cos, pi

from .copper_connectivity import copper_contact_roots
from .drc import _explicit_copper_contacts


def native_item_identity(item):
    if item.kind == "pad":
        return f"pad:{item.component}.{item.pad}:{item.index}"
    if item.kind == "segment":
        return f"track:{item.index}"
    if item.kind == "via":
        return f"via:{item.index}"
    return None


def filled_component_roots(board, evidence, nets):
    """Join positive-area fill contacts on their actual layer, including holes.

    Rounded copper uses an inscribed approximation; drills use a circumscribed
    one. A missed marginal contact leaves extra proposals, not a false join.
    No buffer(0) healing or boundary-only contact is permitted.
    """
    contacts, _, groups = _explicit_copper_contacts(board, only_nets=nets,
        include_internal_connections=False)
    parent = copper_contact_roots(contacts, groups)

    def root(identity):
        while parent[identity] != identity:
            parent[identity] = parent[parent[identity]]
            identity = parent[identity]
        return identity

    def join(a, b):
        a, b = root(a), root(b)
        parent[max(a, b)] = min(a, b)

    if not evidence.matches(board):
        return parent
    try:
        from shapely.geometry import MultiPoint
        from .rf_geometry import native_fill_polygon
    except ImportError:
        return parent

    def shape_geometry(shape, *, drill=False):
        spine = MultiPoint([(p.x_nm, p.y_nm) for p in shape.spine]).convex_hull
        radius = shape.radius_nm
        if radius:
            # 16 segments per quadrant. Expand ONLY the subtracted drilled void.
            spine = spine.buffer(radius / cos(pi / 64) if drill else radius, quad_segs=16)
        return spine

    geometries = {}
    for contact in contacts:
        copper = shape_geometry(contact.shape)
        drill = shape_geometry(contact.drill, drill=True) if contact.drill else None
        geometries[contact.identity] = (copper, drill)
    fills = []
    for fragment in evidence.filled_regions:
        if fragment.net not in nets:
            continue
        try:
            geometry = native_fill_polygon({"outer": fragment.vertices_nm, "holes": []})
        except ValueError:
            continue
        if geometry.geom_type != "Polygon":
            # Disconnected faces in one native walk are not one component.
            continue
        identity = f"fill:{fragment.fragment_digest}"
        parent[identity] = identity
        fills.append((fragment, identity, geometry))
        for contact in contacts:
            if contact.net != fragment.net or fragment.layer not in {p.value for p in contact.layers}:
                continue
            copper, drill = geometries[contact.identity]
            if not geometry.intersects(copper):
                continue
            overlap = geometry.intersection(copper)
            if drill is not None and fragment.layer not in {p.value for p in contact.capped_layers}:
                overlap = overlap.difference(drill)
            if overlap.area > 0:
                join(identity, contact.identity)
    for index, (fragment, identity, geometry) in enumerate(fills):
        for other, other_id, other_geometry in fills[:index]:
            if (fragment.net == other.net and fragment.layer == other.layer
                    and root(identity) != root(other_id)
                    and geometry.intersects(other_geometry)
                    and geometry.intersection(other_geometry).area > 0):
                join(identity, other_id)
    return {identity: root(identity) for identity in parent}

"""Adapter-neutral vector CAM connectivity/DFM algorithms.

Input copper is FINAL positive copper after polarity composition and drilling,
not net-labelled source zone outlines. Coordinates and clearances are nm. A
qualified Gerber geometry adapter is still required to establish those inputs.
No raster is used, and no invalid polygon is silently healed. Straight polygon
geometry only: adapters must bound curve approximation independently.
"""
from __future__ import annotations

from .qualification import check
from .power_integrity import number


def verify_vector_copper(layers: dict, contacts: list, plated_links: list, *, minimum_clearance_nm: int) -> list[dict]:
    """Recover connected copper components and compare the intended net partition.

    contacts: net, layer, outer, holes; polygons describing actual conductive
    contact surfaces (an annulus for a drilled pad), NOT arbitrary probe radii.
    links: layer -> {outer, holes} conductive barrel-contact surfaces. All surfaces
    must be fully covered by final copper. Unconnected inner layers may be omitted
    only when extraction proves no copper touches the barrel on that layer.
    """
    from shapely.geometry import Polygon
    from shapely.ops import unary_union
    number(minimum_clearance_nm, "minimum_clearance_nm", allow_zero=True)
    if not layers or not contacts:
        return [check("cam.vector-connectivity", "incomplete", "Copper and expected contact surfaces required")]
    if not plated_links:
        # Empty is valid only for a board without plated inter-layer connections;
        # the adapter must verify the complete drill inventory separately.
        plated_links = []
    def polygon(record):
        shape = Polygon(record["outer"], record.get("holes", []))
        if shape.is_empty or not shape.is_valid or shape.area <= 0:
            raise ValueError("unsupported/invalid CAM polygon")
        return shape
    components = {}
    for layer, records in layers.items():
        merged = unary_union([polygon(record) for record in records])
        parts = [merged] if merged.geom_type == "Polygon" else list(merged.geoms) if merged.geom_type == "MultiPolygon" else []
        if records and not parts:
            raise ValueError("CAM copper did not normalize to polygons")
        for index, shape in enumerate(parts):
            components[(layer, index)] = shape
    parent = {key: key for key in components}
    def root(key):
        while parent[key] != key:
            parent[key] = parent[parent[key]]
            key = parent[key]
        return key
    def join(keys):
        for key in keys[1:]:
            parent[root(key)] = root(keys[0])
    defects = []
    for index, link in enumerate(plated_links):
        surfaces = link.get("surfaces", {})
        if len(surfaces) < 2 or set(surfaces) - set(layers):
            raise ValueError("plated link needs at least two valid layer surfaces")
        attached = []
        for layer, record in surfaces.items():
            surface = polygon(record)
            keys = [key for key, shape in components.items() if key[0] == layer and shape.covers(surface)]
            if len(keys) != 1:
                defects.append(f"plated link {index}: missing/incomplete annulus on {layer}")
            else:
                attached.append(keys[0])
        if len(attached) == len(surfaces):
            join(attached)
    labelled = []
    for index, contact in enumerate(contacts):
        if not isinstance(contact.get("net"), str) or not contact["net"] or contact["layer"] not in layers:
            raise ValueError("contact requires intended net and valid layer")
        surface = polygon(contact)
        keys = [key for key, shape in components.items() if key[0] == contact["layer"] and shape.covers(surface)]
        if len(keys) != 1:
            defects.append(f"contact {index}: missing/incomplete contact copper")
        else:
            labelled.append((contact["net"], root(keys[0])))
    root_nets, net_roots = {}, {}
    for net, key in labelled:
        root_nets.setdefault(key, set()).add(net)
        net_roots.setdefault(net, set()).add(key)
    defects += [f"short: {sorted(nets)}" for nets in root_nets.values() if len(nets) > 1]
    defects += [f"open: {net} spans {len(keys)} copper components" for net, keys in net_roots.items() if len(keys) > 1]
    unassigned = [key for key in components if root(key) not in root_nets]
    clearances = []
    keys = sorted(components)
    for index, first in enumerate(keys):
        for second in keys[index + 1:]:
            if first[0] != second[0]:
                continue
            a, b = root_nets.get(root(first)), root_nets.get(root(second))
            if a and b and a == b and len(a) == 1:
                continue
            distance = components[first].distance(components[second])
            if distance < minimum_clearance_nm:
                clearances.append((first, second, distance))
    return [check("cam.vector-connectivity", "fail" if defects else "incomplete" if unassigned else "pass",
                  "Vector copper partition; depends on complete qualified adapter extraction",
                  defects=defects, unassigned_components=unassigned, copper_components=len(components)),
            check("cam.vector-clearance", "fail" if clearances else "incomplete" if unassigned else "pass",
                  "Planar component spacing; same-net spacing/creepage require separate rules",
                  violations=clearances, minimum_clearance_nm=minimum_clearance_nm)]


def rectangular_paste_area_ratio(*, width_nm, height_nm, stencil_thickness_nm) -> float:
    width = number(width_nm, "width_nm")
    height = number(height_nm, "height_nm")
    thickness = number(stencil_thickness_nm, "stencil_thickness_nm")
    return width * height / (2 * (width + height) * thickness)


def minimum_polygon_web(polygons: list, *, minimum_web_nm: int) -> dict:
    """Spacing between separate positive mask openings; not a solder bridge proof."""
    from shapely.geometry import Polygon
    number(minimum_web_nm, "minimum_web_nm", allow_zero=True)
    if not polygons:
        return check("cam.mask-web", "incomplete", "No mask opening geometry supplied")
    shapes = [Polygon(record["outer"], record.get("holes", [])) for record in polygons]
    if any(shape.is_empty or not shape.is_valid for shape in shapes):
        raise ValueError("invalid mask opening polygon")
    bad = [(a, b) for a in range(len(shapes)) for b in range(a + 1, len(shapes))
           if shapes[a].distance(shapes[b]) < minimum_web_nm]
    return check("cam.mask-web", "fail" if bad else "pass", "Positive opening-to-opening web check", violations=bad)

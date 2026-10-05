"""Derived physical scene and pad-island ratsnest, independent of UI transport."""
from __future__ import annotations

from dataclasses import replace
import json
from math import cos, hypot, isqrt, radians, sin

from ..drc import explicit_copper_connectivity, placed_pad_shape
from ..geometry import bounds
from ..hard_macros import materialize_hard_macros
from ..physical import (FootprintArc, FootprintCircle, FootprintLayer,
                        FootprintLine, FootprintPolygon, FootprintRectangle,
                        PadKind, PadReference, PadShape, PhysicalBoard, Point)
from ..placement import (PlacementPlannerOptions, placement_solution_is_hard_legal,
                         placement_solution_is_legal, relative_placement_violations,
                         transformed_footprint_polygon, transformed_local_point)
from .islands import nearest_island_tree

SCHEMA = "copperscript-editor-scene/v0.1"


def _point(point: Point) -> list[int]:
    return [point.x_nm, point.y_nm]


def _shape(shape) -> dict:
    return {"spine": [_point(p) for p in shape.spine], "radius_nm": shape.radius_nm}


def _preview_copper(board):
    if not board.hard_macros:
        return board, None
    try:
        return materialize_hard_macros(board), None
    except ValueError as exc:
        # Inspection-grid poses may not yet form a legal macro. Keep the viewer
        # usable for legalization, but grant no private-copper connectivity.
        return board, str(exc)


def ratsnest(board: PhysicalBoard, *, only_nets: frozenset[str] | None = None) -> list[dict]:
    """Deterministic Euclidean MST between explicit pad connectivity islands.

    Nearest physical pad centres define inter-island distance. Duplicate numbers
    alone never join islands; the shared exact graph owns all connectivity.
    Unfilled zones do not join anything. This is not a route or DRC certificate.
    """
    board, _ = _preview_copper(board)
    graph = explicit_copper_connectivity(board, only_nets=only_nets)
    assigned = {p: n.name for n in board.nets if only_nets is None or n.name in only_nets for p in n.pads}
    by_net: dict[str, list[tuple[str, str, str, str, Point]]] = {}
    for pose in sorted(board.placements, key=lambda p: p.reference):
        for index, pad in enumerate(board.footprints[pose.footprint].pads):
            net = assigned.get(PadReference(pose.reference, pad.number))
            if net is None or pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                continue
            identity = f"pad:{pose.reference}.{pad.number}:{index}"
            by_net.setdefault(net, []).append((graph.roots[identity], identity,
                pose.reference, pad.number, transformed_local_point(pose, pad.position)))
    for index, track in enumerate(board.tracks):
        if only_nets is not None and track.net not in only_nets:
            continue
        identity = f"track:{index}"
        for end, point in enumerate((track.start, track.end)):
            by_net.setdefault(track.net, []).append((graph.roots[identity], f"{identity}:{end}",
                                                    "", "", point))
    for index, via in enumerate(board.vias):
        if only_nets is not None and via.net not in only_nets:
            continue
        identity = f"via:{index}"
        by_net.setdefault(via.net, []).append((graph.roots[identity], identity, "", "", via.position))
    result = []
    for net, lands in sorted(by_net.items()):
        for distance, _, _, _, _, a, b in nearest_island_tree(lands):
            result.append({"net": net, "distance_squared_nm": str(distance),
                "from": {"land": a[1], "reference": a[2], "pad": a[3], "position": _point(a[4])},
                "to": {"land": b[1], "reference": b[2], "pad": b[3], "position": _point(b[4])}})
    return result


class RatsnestCache:
    """Recompute only nets incident to changed immutable placements.

    Copper/macro/inventory changes invalidate all roots. Cache reuse never
    substitutes approximate contact or stale connectivity for the exact graph.
    """
    def __init__(self):
        self.board = None
        self.edges = {}
        self.recomputed_nets = ()

    def get(self, board):
        old = self.board
        nets = {n.name for n in board.nets} | {t.net for t in board.tracks} | {v.net for v in board.vias}
        if (old is None or old.nets != board.nets or old.footprints != board.footprints
                or old.tracks != board.tracks or old.vias != board.vias
                or old.hard_macros != board.hard_macros or board.hard_macros
                or old.rigid_clusters != board.rigid_clusters):
            affected = nets
            self.edges.clear()
        else:
            previous = {p.reference: p for p in old.placements}
            changed = {p.reference for p in board.placements if previous.get(p.reference) != p}
            changed |= previous.keys() - {p.reference for p in board.placements}
            affected = {n.name for n in board.nets if any(p.component in changed for p in n.pads)}
        self.recomputed_nets = tuple(sorted(affected))
        if affected:
            fresh = ratsnest(board, only_nets=frozenset(affected))
            for net in affected:
                self.edges[net] = [edge for edge in fresh if edge["net"] == net]
        self.board = board
        return [edge for net in sorted(self.edges) for edge in self.edges[net]]


def _body_outline(footprint) -> tuple[Point, ...]:
    """Return an origin-preserving visual body box for an imported footprint.

    ``PhysicalFootprint.body_size`` is a compact legacy size and does not carry
    the local origin of an offset connector.  KiCad connector footprints often
    put their anchor on pad 1, so centering that size at the placement origin
    draws a misleading body box.  Prefer actual non-courtyard graphics and pad
    envelopes when available, retaining the centered fallback for proxy and
    minimal footprints.
    """

    graphics = [
        graphic for graphic in footprint.graphics
        if getattr(graphic, "layer", None) is not FootprintLayer.COURTYARD
    ]
    if not graphics:
        half_width = (footprint.body_size.width_nm + 1) // 2
        half_height = (footprint.body_size.height_nm + 1) // 2
        return (Point(-half_width, -half_height), Point(half_width, -half_height),
                Point(half_width, half_height), Point(-half_width, half_height))

    points: list[Point] = []

    def add(point: Point, radius: int = 0) -> None:
        points.extend((Point(point.x_nm - radius, point.y_nm - radius),
                       Point(point.x_nm + radius, point.y_nm + radius)))

    for pad in footprint.pads:
        angle = radians(float(pad.rotation_degrees))
        half_x, half_y = pad.size.width_nm / 2, pad.size.height_nm / 2
        for x, y in ((-half_x, -half_y), (half_x, -half_y),
                     (half_x, half_y), (-half_x, half_y)):
            add(Point(round(pad.position.x_nm + x * cos(angle) - y * sin(angle)),
                      round(pad.position.y_nm + x * sin(angle) + y * cos(angle))))

    for graphic in graphics:
        width = getattr(graphic, "width_nm", 0) // 2
        if isinstance(graphic, FootprintLine):
            add(graphic.start, width)
            add(graphic.end, width)
        elif isinstance(graphic, FootprintRectangle):
            for point in (graphic.start,
                          Point(graphic.end.x_nm, graphic.start.y_nm),
                          graphic.end,
                          Point(graphic.start.x_nm, graphic.end.y_nm)):
                add(point, width)
        elif isinstance(graphic, FootprintCircle):
            radius = round(hypot(graphic.end.x_nm - graphic.center.x_nm,
                                 graphic.end.y_nm - graphic.center.y_nm)) + width
            add(graphic.center, radius)
        elif isinstance(graphic, FootprintArc):
            for point in (graphic.start, graphic.midpoint, graphic.end):
                add(point, width)
        elif isinstance(graphic, FootprintPolygon):
            for point in graphic.points:
                add(point, width)

    if not points:
        half_width = (footprint.body_size.width_nm + 1) // 2
        half_height = (footprint.body_size.height_nm + 1) // 2
        return (Point(-half_width, -half_height), Point(half_width, -half_height),
                Point(half_width, half_height), Point(-half_width, half_height))
    min_x = min(point.x_nm for point in points)
    min_y = min(point.y_nm for point in points)
    max_x = max(point.x_nm for point in points)
    max_y = max(point.y_nm for point in points)
    return (Point(min_x, min_y), Point(max_x, min_y),
            Point(max_x, max_y), Point(min_x, max_y))


def _metadata_map(board: PhysicalBoard, key: str) -> dict:
    try:
        value = json.loads(board.metadata.get(key, "{}"))
    except (TypeError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def board_scene(board: PhysicalBoard, *, source_revision: str, revision: int = 0,
                session_locks: frozenset[str] = frozenset(),
                options: PlacementPlannerOptions | None = None, ratsnest_cache: RatsnestCache | None = None) -> dict:
    rules = {rule.reference: rule for rule in board.placement_rules}
    profile_roles = _metadata_map(board, "mechanical_connector_roles")
    pad_names = _metadata_map(board, "pad_names")
    pose_map = {pose.reference: pose for pose in board.placements}
    relative_violations = relative_placement_violations(board, pose_map)
    violating_references = {
        reference for violation in relative_violations
        for reference in violation.get("references", ())
    }
    assigned = {pad: net.name for net in board.nets for pad in net.pads}
    components = []
    for pose in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[pose.footprint]
        from ..mechanical_assembly import component_height
        rule = rules.get(pose.reference)
        pads = []
        for index, pad in enumerate(footprint.pads):
            position = transformed_local_point(pose, pad.position)
            drill = None
            if pad.drill:
                drill = _shape(placed_pad_shape(position, replace(pad,
                    size=pad.drill, kind=PadKind.SMD, drill=None,
                    shape=PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm else PadShape.OVAL), pose))
            pads.append({"number": pad.number, "name": pad_names.get(pose.reference, {}).get(pad.number),
                "land": f"pad:{pose.reference}.{pad.number}:{index}",
                "kind": pad.kind.value, "net": assigned.get(PadReference(pose.reference, pad.number)),
                "shape": _shape(placed_pad_shape(position, pad, pose)), "drill": drill})
        body = tuple(transformed_local_point(pose, point) for point in _body_outline(footprint))
        components.append({"reference": pose.reference, "hierarchy": pose.reference.split("/")[:-1],
            "source_attachment": next((a.id for a in board.attachments if a.reference==pose.reference),None),
            "height_nm": component_height(board,pose),
            "profile_role": profile_roles.get(pose.reference),
            "footprint": pose.footprint, "position": _point(pose.position),
            "rotation": str(pose.rotation_degrees), "side": pose.side.value,
            "value": pose.value, "body": [_point(p) for p in body],
            "courtyard": [_point(p) for p in transformed_footprint_polygon(board, pose)],
            "pads": pads, "source_position_locked": bool(rule and rule.fixed_position is not None),
            "source_rotation_locked": bool(rule and rule.fixed_rotation_degrees is not None),
            "source_side_locked": bool(rule and rule.side is not None),
            "session_locked": pose.reference in session_locks,
            "constraint_warning": pose.reference in violating_references,
            "constraint_warning_count": sum(pose.reference in violation.get("references", ())
                                              for violation in relative_violations),
            "allowed_orientations": [str(x) for x in rule.allowed_orientations] if rule else ["0", "90", "180", "270"],
            "macro": next((c.name for c in board.rigid_clusters
                if any(m.reference == pose.reference for m in c.members)), None)})
    extent = bounds(board.outline.vertices)
    circle = board.outline.circular_boundary
    from ..mechanical_references import reference_scene
    warning_data = {k: board.metadata[k] for k in ("footprint_import_warnings", "omitted_components",
            "omitted_constraint_targets", "prototype_footprints") if k in board.metadata}
    if relative_violations:
        warning_data["relative_placement"] = (
            f"{len(relative_violations)} relative placement constraint(s) are outside tolerance; "
            "temporary editor poses remain movable."
        )
    scene = {"schema": SCHEMA, "board": board.name, "source_revision": source_revision,
        "revision": revision, "units": "nm", "source_writable": False,
        "mechanical_provenance": json.loads(board.metadata.get("mechanical_provenance", "{}")),
        "capabilities": {"auto_place": True, "pose_preview": True, "session_locks": True,
                         "source_save": False, "mechanical_edit": False},
        "bounds": [extent.min_x, extent.min_y, extent.max_x, extent.max_y],
        "outline": {"vertices": [_point(p) for p in board.outline.vertices],
            "path": _outline_path(board.outline.boundary_path),
            "circle": {"center": _point(circle.center), "radius_nm": circle.radius_nm} if circle else None,
            "cutouts": [{"id": c.id, "vertices": [_point(p) for p in c.vertices]} for c in board.outline.cutouts]},
        "holes": [{"id": h.id, "position": _point(h.position), "diameter_nm": h.diameter_nm,
                   "head_clearance_radius_nm": h.head_clearance_radius_nm} for h in board.mechanical_holes],
        "slots": [{"id":s.id,"start":_point(s.start),"end":_point(s.end),"width_nm":s.width_nm} for s in board.mechanical_slots],
        "references": [reference_scene(r) for r in board.mechanical_references],
        "datums": [{"id": d.id, "position": _point(d.position), "relative_to": d.relative_to} for d in board.datums],
        "boundary_edges": [{"id": e.id, "start": _point(e.start), "end": _point(e.end)} for e in board.boundary_edges],
        "attachments": [{"id": a.id, "reference": a.reference, "target": a.target, "position": _point(a.position), "anchor": a.anchor} for a in board.attachments],
        "body_overhangs": [{"id":a.id,"reference":a.reference,"edge":a.edge,"reason":a.reason,"vertices":[_point(p) for p in _overhang_band(board,a)]} for a in board.body_overhangs],
        "assembly_envelopes": [{"id":a.id,"side":a.side.value,"maximum_height_nm":a.maximum_height_nm,"vertices":[_point(p) for p in a.outline.vertices]} for a in board.assembly_envelopes],
        "assembly_access": [{"id":a.id,"reference":a.reference,"purpose":a.purpose,"side":_access_side(a,next(p for p in board.placements if p.reference==a.reference)),
                             "vertices":[_point(transformed_local_point(next(p for p in board.placements if p.reference==a.reference),v)) for v in a.outline.vertices]} for a in board.assembly_access],
        "keepouts": [{"name": k.name, "side": k.side.value if k.side else "both",
                      "vertices": [_point(p) for p in k.outline.vertices]} for k in board.keepouts],
        "copper_keepouts": [{"name": k.id, "layers": [layer.value for layer in k.layers],
                             "vertices": [_point(p) for p in k.outline.outer.vertices],
                             "holes": [[_point(p) for p in h.vertices] for h in k.outline.holes]}
                            for k in board.copper_keepouts],
        "regions": [{"name": r.name, "side": r.side.value if r.side else "both",
                     "vertices": [_point(p) for p in r.outline.vertices]} for r in board.regions],
        "components": components, "nets": [n.name for n in sorted(board.nets, key=lambda n: n.name)],
        "zone_nets": sorted({z.net for z in board.zones}),
        "power_nets": [],
        "ratsnest": ratsnest_cache.get(board) if ratsnest_cache else ratsnest(board),
        "warnings": warning_data,
        "placement_constraints": {"violations": list(relative_violations),
            "violating_components": sorted(violating_references)},
        "placement_legal": placement_solution_is_legal(board,
            {p.reference: p for p in board.placements},
            replace(options or PlacementPlannerOptions(), fixed_references=session_locks)),
        "placement_hard_legal": placement_solution_is_hard_legal(board,
            {p.reference: p for p in board.placements},
            replace(options or PlacementPlannerOptions(), fixed_references=session_locks)),
        "notice": "Session preview only: source is read-only; no routing or manufacturing signoff."}
    _, macro_error = _preview_copper(board)
    if macro_error:
        scene["warnings"]["macro_copper_not_materialized"] = macro_error
    scene["net_costs"] = net_costs(scene["ratsnest"])
    return scene


def _access_side(access,pose):
    return pose.side.value if access.side=='component' else ('back' if pose.side.value=='front' else 'front')


def _outline_path(path):
    if path is None:return None
    from ..physical import BoundaryArc
    from ..mechanical_curves import arc_angles
    result=[]
    for segment in path.segments:
        item={'id':segment.id,'kind':'line','start':_point(segment.start),'end':_point(segment.end)}
        if isinstance(segment,BoundaryArc):
            cx,cy,r2,angle,sweep=arc_angles(segment)
            item.update(kind='arc',mid=_point(segment.mid),radius_nm=float(r2)**.5,sweep=sweep>0)
        result.append(item)
    return result


def _overhang_band(board,policy):
    from ..mechanical_assembly import expanded_body_outline
    expanded=expanded_body_outline(board.outline,board.boundary_edges,policy)
    edge=next(e for e in board.boundary_edges if e.id==policy.edge)
    length=abs(edge.end.x_nm-edge.start.x_nm)+abs(edge.end.y_nm-edge.start.y_nm)
    dx,dy=(edge.end.x_nm-edge.start.x_nm)//length,(edge.end.y_nm-edge.start.y_nm)//length
    a=Point(edge.start.x_nm+dx*policy.start_nm,edge.start.y_nm+dy*policy.start_nm)
    b=Point(edge.start.x_nm+dx*policy.end_nm,edge.start.y_nm+dy*policy.end_nm)
    # Exact axis-aligned rectangle: only the two exterior points are new.
    outside=[p for p in expanded.vertices if p not in board.outline.vertices and p not in (a,b)]
    return (a,b,*sorted(outside,key=lambda p:abs(p.x_nm-b.x_nm)+abs(p.y_nm-b.y_nm)))


def net_costs(edges):
    """Remaining straight-line MST length, not a predicted detailed route cost."""
    costs = {}
    for edge in edges:
        cost = costs.setdefault(edge["net"], {"airwires": 0, "length_nm": 0})
        cost["airwires"] += 1
        cost["length_nm"] += isqrt(int(edge["distance_squared_nm"]))
    return costs

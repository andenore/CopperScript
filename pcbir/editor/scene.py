"""Derived physical scene and pad-island ratsnest, independent of UI transport."""
from __future__ import annotations

from dataclasses import replace
import json

from ..drc import explicit_copper_connectivity, placed_pad_shape
from ..geometry import bounds
from ..hard_macros import materialize_hard_macros
from ..physical import PadKind, PadReference, PadShape, PhysicalBoard, Point
from ..placement import (PlacementPlannerOptions, placement_solution_is_legal,
                         transformed_footprint_polygon, transformed_local_point)

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


def ratsnest(board: PhysicalBoard) -> list[dict]:
    """Deterministic Euclidean MST between explicit pad connectivity islands.

    Nearest physical pad centres define inter-island distance. Duplicate numbers
    alone never join islands; the shared exact graph owns all connectivity.
    Unfilled zones do not join anything. This is not a route or DRC certificate.
    """
    board, _ = _preview_copper(board)
    graph = explicit_copper_connectivity(board)
    assigned = {p: n.name for n in board.nets for p in n.pads}
    by_net: dict[str, list[tuple[str, str, str, str, Point]]] = {}
    for pose in sorted(board.placements, key=lambda p: p.reference):
        for index, pad in enumerate(board.footprints[pose.footprint].pads):
            net = assigned.get(PadReference(pose.reference, pad.number))
            if net is None or pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                continue
            identity = f"pad:{pose.reference}.{pad.number}:{index}"
            by_net.setdefault(net, []).append((graph.roots[identity], identity,
                pose.reference, pad.number, transformed_local_point(pose, pad.position)))
    result = []
    for net, lands in sorted(by_net.items()):
        roots = sorted({land[0] for land in lands})
        parent = {root: root for root in roots}

        def find(root):
            while root != parent[root]:
                parent[root] = parent[parent[root]]
                root = parent[root]
            return root

        nearest = {}
        for i, left in enumerate(lands):
            for right in lands[i + 1:]:
                if left[0] == right[0]:
                    continue
                a, b = sorted((left, right), key=lambda land: (land[0], land[1]))
                distance = (a[4].x_nm - b[4].x_nm) ** 2 + (a[4].y_nm - b[4].y_nm) ** 2
                edge = (distance, a[0], b[0], a[1], b[1], a, b)
                key = a[0], b[0]
                if key not in nearest or edge[:5] < nearest[key][:5]:
                    nearest[key] = edge
        for distance, ar, br, _, _, a, b in sorted(nearest.values(), key=lambda edge: edge[:5]):
            left, right = find(ar), find(br)
            if left == right:
                continue
            parent[max(left, right)] = min(left, right)
            result.append({"net": net, "distance_squared_nm": str(distance),
                "from": {"land": a[1], "reference": a[2], "pad": a[3], "position": _point(a[4])},
                "to": {"land": b[1], "reference": b[2], "pad": b[3], "position": _point(b[4])}})
    return result


def board_scene(board: PhysicalBoard, *, source_revision: str, revision: int = 0,
                session_locks: frozenset[str] = frozenset(),
                options: PlacementPlannerOptions | None = None) -> dict:
    rules = {rule.reference: rule for rule in board.placement_rules}
    profile_roles = json.loads(board.metadata.get("mechanical_connector_roles", "{}"))
    assigned = {pad: net.name for net in board.nets for pad in net.pads}
    components = []
    for pose in sorted(board.placements, key=lambda item: item.reference):
        footprint = board.footprints[pose.footprint]
        rule = rules.get(pose.reference)
        pads = []
        for index, pad in enumerate(footprint.pads):
            position = transformed_local_point(pose, pad.position)
            drill = None
            if pad.drill:
                drill = _shape(placed_pad_shape(position, replace(pad,
                    size=pad.drill, kind=PadKind.SMD, drill=None,
                    shape=PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm else PadShape.OVAL), pose))
            pads.append({"number": pad.number, "land": f"pad:{pose.reference}.{pad.number}:{index}",
                "kind": pad.kind.value, "net": assigned.get(PadReference(pose.reference, pad.number)),
                "shape": _shape(placed_pad_shape(position, pad, pose)), "drill": drill})
        w, h = footprint.body_size.width_nm // 2, footprint.body_size.height_nm // 2
        body = tuple(transformed_local_point(pose, Point(x, y)) for x, y in
                     ((-w, -h), (w, -h), (w, h), (-w, h)))
        components.append({"reference": pose.reference, "hierarchy": pose.reference.split("/")[:-1],
            "profile_role": profile_roles.get(pose.reference),
            "footprint": pose.footprint, "position": _point(pose.position),
            "rotation": str(pose.rotation_degrees), "side": pose.side.value,
            "value": pose.value, "body": [_point(p) for p in body],
            "courtyard": [_point(p) for p in transformed_footprint_polygon(board, pose)],
            "pads": pads, "source_position_locked": bool(rule and rule.fixed_position is not None),
            "source_rotation_locked": bool(rule and rule.fixed_rotation_degrees is not None),
            "session_locked": pose.reference in session_locks,
            "allowed_orientations": [str(x) for x in rule.allowed_orientations] if rule else ["0", "90", "180", "270"],
            "macro": next((c.name for c in board.rigid_clusters
                if any(m.reference == pose.reference for m in c.members)), None)})
    extent = bounds(board.outline.vertices)
    circle = board.outline.circular_boundary
    scene = {"schema": SCHEMA, "board": board.name, "source_revision": source_revision,
        "revision": revision, "units": "nm", "source_writable": False,
        "mechanical_provenance": json.loads(board.metadata.get("mechanical_provenance", "{}")),
        "capabilities": {"auto_place": True, "pose_preview": True, "session_locks": True,
                         "source_save": False, "mechanical_edit": False},
        "bounds": [extent.min_x, extent.min_y, extent.max_x, extent.max_y],
        "outline": {"vertices": [_point(p) for p in board.outline.vertices],
            "circle": {"center": _point(circle.center), "radius_nm": circle.radius_nm} if circle else None,
            "cutouts": [{"id": c.id, "vertices": [_point(p) for p in c.vertices]} for c in board.outline.cutouts]},
        "holes": [{"id": h.id, "position": _point(h.position), "diameter_nm": h.diameter_nm,
                   "head_clearance_radius_nm": h.head_clearance_radius_nm} for h in board.mechanical_holes],
        "keepouts": [{"name": k.name, "side": k.side.value if k.side else "both",
                      "vertices": [_point(p) for p in k.outline.vertices]} for k in board.keepouts],
        "copper_keepouts": [{"name": k.id, "layers": [layer.value for layer in k.layers],
                             "vertices": [_point(p) for p in k.outline.outer.vertices],
                             "holes": [[_point(p) for p in h.vertices] for h in k.outline.holes]}
                            for k in board.copper_keepouts],
        "regions": [{"name": r.name, "side": r.side.value if r.side else "both",
                     "vertices": [_point(p) for p in r.outline.vertices]} for r in board.regions],
        "components": components, "nets": [n.name for n in sorted(board.nets, key=lambda n: n.name)],
        "zone_nets": sorted({z.net for z in board.zones}), "ratsnest": ratsnest(board),
        "warnings": {k: board.metadata[k] for k in ("footprint_import_warnings", "omitted_components",
            "omitted_constraint_targets", "prototype_footprints") if k in board.metadata},
        "placement_legal": placement_solution_is_legal(board,
            {p.reference: p for p in board.placements},
            replace(options or PlacementPlannerOptions(), fixed_references=session_locks)),
        "notice": "Session preview only: source is read-only; no routing or manufacturing signoff."}
    _, macro_error = _preview_copper(board)
    if macro_error:
        scene["warnings"]["macro_copper_not_materialized"] = macro_error
    return scene

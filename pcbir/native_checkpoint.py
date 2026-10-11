"""Read-only source/land/copper checkpoint for front-mounted native boards.

This is NOT a full native importer or a qualification gate: outlines, zones,
stackup materials, masks and filled connectivity still need independent audits.
Source zones are retained as intent, never as observed fill. Via process/finish
intent can be rebound only to an observed matching hole; the PCB does not prove
filled/capped fabrication. No missing owner copper is synthesized on readback.
"""
from collections import defaultdict, deque
from dataclasses import replace
from decimal import Decimal
from pathlib import Path

from .backends.kicad_pcb import kicad_reference
from .hard_macros import macro_source, materialize_hard_macros, validate_hard_macros
from .importers.kicad_mod import _children, _first, _parse_sexpr
from .pad_via_arrays import require_via_in_pad_arrays
from .physical import (BoardSide, CopperLayer, CopperPolygon, PadKind, PadReference,
                       PadShape, Point, PolygonRing, TrackSegment, Via, nm_from_mm)
from .placement import placement_rejection_reasons


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _angle(at):
    angle = Decimal(at[3]) if len(at) > 3 else Decimal(0)
    return (angle % 360 + 360) % 360


def _net(item, native_nets):
    node = _first(item, "net")
    if node is None:
        return None
    _require(node[1] in native_nets, f"Unknown native net: {node[1]}")
    name = native_nets[node[1]]
    _require(len(node) < 3 or node[2] == name, "Inconsistent native net mapping")
    return name


def _track_key(track):
    endpoints = sorted(((track.start.x_nm, track.start.y_nm), (track.end.x_nm, track.end.y_nm)))
    return track.net, tuple(endpoints), track.width_nm, track.layer


def _via_key(via):
    return (via.net, via.position, via.size_nm, via.drill_nm,
            tuple(sorted((via.from_layer.value, via.to_layer.value))))


def _polygon_key(polygon):
    vertices = tuple((p.x_nm, p.y_nm) for p in polygon.outline.vertices)
    # Ring starting vertex and winding are not copper identity.
    rings = [v[i:] + v[:i] for v in (vertices, vertices[::-1]) for i in range(len(v))]
    return polygon.net, polygon.layer, min(rings)


def _observed_owners(observed, required, key, kind):
    """Rebind intent one occurrence at a time, never append absent geometry."""
    owners = defaultdict(deque)
    for item in required:
        owners[key(item)].append(item)
    result = []
    for item in observed:
        matches = owners.get(key(item))
        result.append(matches.popleft() if matches else item)
    _require(not any(owners.values()), f"Missing source-owned native {kind}")
    return tuple(result)


def _validate_lands(board, native_footprints, native_nets):
    expected_nets = {p: n.name for n in board.nets for p in n.pads}
    for pose, fp in native_footprints:
        ref = pose.reference
        footprint = board.footprints[pose.footprint]
        native_pads = _children(fp, "pad")
        _require(len(native_pads) == len(footprint.pads), f"Changed lands: {ref}")
        native_pads.sort(key=lambda p: (p[1], Point.mm(*_first(p, "at")[1:3]).x_nm,
                                      Point.mm(*_first(p, "at")[1:3]).y_nm))
        source_pads = sorted(footprint.pads, key=lambda p: (p.number, p.position.x_nm, p.position.y_nm))
        for native, pad in zip(native_pads, source_pads):
            label = f"{ref}.{pad.number}"
            kind = ("connect" if pad.connector_contact else "smd") if pad.kind is PadKind.SMD else {
                PadKind.APERTURE: "smd", PadKind.THROUGH_HOLE: "thru_hole",
                PadKind.NON_PLATED_THROUGH_HOLE: "np_thru_hole"}[pad.kind]
            shape = "rect" if pad.shape is PadShape.RECTANGLE else pad.shape.value
            _require(native[1] == pad.number, f"Changed pad mapping: {label}")
            _require(native[2:4] == [kind, shape], f"Changed pad shape/kind: {label}")
            at = _first(native, "at")
            _require(Point.mm(*at[1:3]) == pad.position, f"Changed land position: {label}")
            _require(_angle(at) == (pad.rotation_degrees + pose.rotation_degrees + 360) % 360,
                     f"Changed land angle: {label}")
            size = _first(native, "size")
            _require(tuple(nm_from_mm(v) for v in size[1:3]) == (pad.size.width_nm, pad.size.height_nm),
                     f"Changed land size: {label}")
            copper = {v for v in _first(native, "layers")[1:] if v.endswith(".Cu")}
            expected_copper = (set() if pad.kind is PadKind.APERTURE else {"F.Cu"}) if pad.kind in {
                PadKind.SMD, PadKind.APERTURE} else {"*.Cu"}
            _require(copper == expected_copper, f"Changed land copper layers: {label}")
            drill = _first(native, "drill")
            if pad.drill is None:
                _require(drill is None, f"Changed land drill: {label}")
            else:
                values = drill[2:] if drill is not None and drill[1] == "oval" else drill[1:] if drill else []
                _require(len(values) in (1, 2), f"Unsupported land drill: {label}")
                dimensions = tuple(nm_from_mm(v) for v in values)
                if len(dimensions) == 1:
                    dimensions *= 2
                _require(dimensions == (pad.drill.width_nm, pad.drill.height_nm), f"Changed land drill: {label}")
            if pad.shape is PadShape.ROUNDRECT:
                ratio = _first(native, "roundrect_rratio")
                _require(ratio is not None and Decimal(ratio[1]) * 1_000_000 == pad.roundrect_ratio_ppm,
                         f"Changed roundrect radius: {label}")
            expected = expected_nets.get(PadReference(ref, pad.number))
            if pad.kind in {PadKind.APERTURE, PadKind.NON_PLATED_THROUGH_HOLE}:
                expected = None
            _require(_net(native, native_nets) == expected, f"Changed land net: {label}")


def read_front_native_checkpoint(source, path: Path, *, include_copper=True):
    """Bind legal native poses and actual explicit copper to authoritative source.

    Supports standard exported front lands, straight tracks, vias and zero-stroke
    filled netted polygons. Polygon occurrences must match authored geometry.
    Unsupported native copper fails closed. No file writes, routing, pose repair,
    filled-zone evidence or production approval occurs here.

    With include_copper=False this returns source-owned copper at the observed
    poses, not observed native copper; only pose/land checks apply in that mode.
    """
    path = Path(path)
    tree = _parse_sexpr(path.read_text(encoding="utf-8"), str(path))
    _require(tree[0] == "kicad_pcb", "Expected native PCB")
    _require(not source.zone_fills, "Checkpoint source must be unfilled")
    if source.materialized_macros:
        source = macro_source(source)
    poses = {kicad_reference(p.reference): p for p in source.placements}
    _require(len(poses) == len(source.placements), "Native reference mapping collides")
    seen, native_footprints, candidate_poses = set(), [], {}
    for fp in _children(tree, "footprint"):
        references = [p[2] for p in _children(fp, "property") if p[1] == "Reference"]
        _require(len(references) == 1, "Missing or duplicate native Reference property")
        ref = references[0]
        _require(ref in poses and ref not in seen, f"Unexpected native reference: {ref}")
        seen.add(ref)
        original = poses[ref]
        _require(original.side is BoardSide.FRONT and _first(fp, "layer")[1] == "F.Cu",
                 "Native checkpoint supports front-mounted parts only")
        at = _first(fp, "at")
        pose = replace(original, position=Point.mm(*at[1:3]), rotation_degrees=_angle(at))
        candidate_poses[pose.reference] = pose
        native_footprints.append((pose, fp))
    _require(seen == set(poses), "Missing native components")
    reasons = placement_rejection_reasons(source, candidate_poses)
    _require(not reasons, f"Illegal native placement: {reasons}")
    board = replace(source, placements=tuple(candidate_poses[p.reference] for p in source.placements))
    board = materialize_hard_macros(board)
    known = {n.name for n in board.nets}
    native_nets = {"0": None, "": None, **{name: name for name in known}}
    for n in _children(tree, "net"):
        _require(n[2] in known or n[2] == "", f"Unexpected native net: {n[2]}")
        value = n[2] or None
        _require(n[1] not in native_nets or native_nets[n[1]] == value, "Conflicting native net mapping")
        native_nets[n[1]] = value
    _validate_lands(board, native_footprints, native_nets)
    layers = _first(tree, "layers")
    _require(layers is not None and tuple(n[1] for n in layers[1:] if n[1].endswith(".Cu")) ==
             tuple(l.value for l in board.stackup.copper_layers), "Changed native copper layer order")
    # Do not ignore copper primitives that our physical track representation lacks.
    for item in tree[1:]:
        if not isinstance(item, list):
            continue
        layer = _first(item, "layer")
        if item[0] == "arc" or (item[0].startswith("gr_") and item[0] != "gr_poly"
                               and layer is not None and layer[1].endswith(".Cu")):
            raise ValueError(f"Unsupported native copper primitive: {item[0]}")
    if include_copper:
        polygons = []
        for item in _children(tree, "gr_poly"):
            layer = _first(item, "layer")[1]
            if not layer.endswith(".Cu"):
                continue
            fill, stroke = _first(item, "fill"), _first(item, "stroke")
            _require(fill is not None and fill[1] == "yes" and stroke is not None and
                     nm_from_mm(_first(stroke, "width")[1]) == 0, "Unsupported native copper polygon")
            net = _net(item, native_nets)
            _require(net is not None, "Unnetted native copper polygon is unsupported")
            pts = _first(item, "pts")
            _require(pts is not None and all(p[0] == "xy" for p in pts[1:]),
                     "Unsupported native copper polygon vertices")
            polygons.append(CopperPolygon(f"native-polygon-{len(polygons)}", net, CopperLayer(layer),
                PolygonRing(tuple(Point.mm(*p[1:3]) for p in pts[1:]))))
        tracks = []
        for segment in _children(tree, "segment"):
            start, end = _first(segment, "start"), _first(segment, "end")
            net = _net(segment, native_nets)
            _require(net is not None, "Unnetted native track is unsupported")
            tracks.append(TrackSegment(net, Point.mm(*start[1:3]),
                Point.mm(*end[1:3]), nm_from_mm(_first(segment, "width")[1]),
                CopperLayer(_first(segment, "layer")[1])))
        vias = []
        for item in _children(tree, "via"):
            _require(not any(v in {"blind", "micro"} for v in item if isinstance(v, str)),
                     "Unsupported native via technology")
            layers = _first(item, "layers")
            net = _net(item, native_nets)
            _require(net is not None, "Unnetted native via is unsupported")
            vias.append(Via(net, Point.mm(*_first(item, "at")[1:3]),
                nm_from_mm(_first(item, "size")[1]), nm_from_mm(_first(item, "drill")[1]),
                CopperLayer(layers[1]), CopperLayer(layers[2])))
        board = replace(board,
            tracks=_observed_owners(tracks, board.tracks, _track_key, "track"),
            vias=_observed_owners(vias, board.vias, _via_key, "via"),
            polygons=_observed_owners(polygons, board.polygons, _polygon_key, "polygon"))
        validate_hard_macros(board)
        require_via_in_pad_arrays(board, "native checkpoint readback")
    scope = "front poses/lands and explicit copper" if include_copper else "front poses/lands only"
    return replace(board, metadata={**board.metadata, "fabrication_ready": "false",
        "native_copper_checked": "true" if include_copper else "false",
        "native_checkpoint_scope": scope + "; source zones are intent only"})

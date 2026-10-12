"""Cache invalidation and exact broad-phase regression oracles."""
from dataclasses import replace
import random
from types import SimpleNamespace

import pytest

from pcbir import (BoardOutline, BoardSide, CopperLayer, CopperZone, FootprintPad, PadKind, PadReference,
                   PadShape, PadViaInPadRule, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                   Placement, Point, PolygonRing, PolygonWithHoles, Size, Stackup, TrackSegment, Via)
from pcbir._cache import IdentityKey
from pcbir.drc import placed_pad_shape
from pcbir.geometry import RoundedConvexShape, circle_inside_shape, shapes_clear
from pcbir.hard_macros import _macro_smd_lands, _validate_macro_via_permissions
from pcbir.placement import (_lands_meet_macro_copper, _local_land_shapes,
                             _legal, PlacementPlannerOptions, transformed_local_point)


def board_fixture():
    fp = PhysicalFootprint("pad", (FootprintPad("1", Point(0, 0), Size(1000001, 1200001)),), Size.mm(2, 2))
    board = PhysicalBoard("cache", BoardOutline.rectangle(30, 30), {fp.name: fp},
        (Placement("U", fp.name, Point.mm(10, 10)), Placement("R", fp.name, Point.mm(20, 20))),
        (PhysicalNet("GND", (PadReference("U", "1"),)), PhysicalNet("X", (PadReference("R", "1"),))),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                        CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK)),
        metadata={"fabrication_profile": "jlcpcb-six-layer"},
        via_in_pad_rules=(PadViaInPadRule(PadReference("U", "1")),),
        zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(
            PolygonRing((Point.mm(1, 1), Point.mm(29, 1), Point.mm(29, 29), Point.mm(1, 29))))),))
    macro = SimpleNamespace(pad_bindings=(SimpleNamespace(pad=PadReference("U", "1"), net="GND"),))
    via = Via("GND", Point.mm(10, 10), 300001, 200000, finish="filled-capped")
    return board, macro, via


def test_permission_and_process_are_rechecked_after_geometry_cache_hit():
    board, macro, via = board_fixture()
    _macro_smd_lands.cache_clear()
    _validate_macro_via_permissions(board, macro, (via,))
    _validate_macro_via_permissions(replace(board, name="same-geometry"), macro, (via,))
    assert _macro_smd_lands.cache_info().hits
    with pytest.raises(ValueError, match="explicit permission"):
        _validate_macro_via_permissions(replace(board, via_in_pad_rules=()), macro, (via,))
    with pytest.raises(ValueError, match="explicit permission"):
        _validate_macro_via_permissions(board, SimpleNamespace(pad_bindings=()), (via,))
    with pytest.raises(ValueError, match="six-layer"):
        _validate_macro_via_permissions(replace(board, metadata={}), macro, (via,))
    with pytest.raises(ValueError, match="six-layer"):
        _validate_macro_via_permissions(board, macro, (replace(via, net="X"),))
    with pytest.raises(ValueError, match="contained"):
        _validate_macro_via_permissions(board, macro, (replace(via, position=Point.mm(10.45, 10)),))


def test_land_snapshot_changes_with_placement_and_footprint_geometry():
    board, macro, via = board_fixture()
    _validate_macro_via_permissions(board, macro, (via,))
    moved = replace(board, placements=(board.placements[0],
        replace(board.placements[1], position=via.position)))
    with pytest.raises(ValueError, match="explicit permission"):
        _validate_macro_via_permissions(moved, macro, (via,))
    fp = board.footprints["pad"]
    tiny = replace(fp, pads=(replace(fp.pads[0], size=Size(200001, 200001)),))
    with pytest.raises(ValueError, match="contained"):
        _validate_macro_via_permissions(replace(board, footprints={fp.name: tiny}), macro, (via,))
    duplicate = replace(fp, pads=(*fp.pads, fp.pads[0]))
    with pytest.raises(ValueError, match="multiple pad lands"):
        _validate_macro_via_permissions(replace(board, footprints={fp.name: duplicate}), macro, (via,))


def test_courtyard_cache_rechecks_same_named_footprint_and_changed_side():
    board, _, _ = board_fixture()
    candidate, other = board.placements
    options = PlacementPlannerOptions()
    assert _legal(candidate, {other.reference: other}, board, options)
    # Preserve the name/pad geometry but change assembly geometry. An identity
    # key must retain the new courtyard instead of reusing the earlier pose.
    fp = replace(board.footprints["pad"], body_size=Size.mm(16, 16))
    changed = replace(board, footprints={fp.name: fp})
    assert not _legal(candidate, {other.reference: other}, changed, options)
    assert _legal(candidate, {other.reference: replace(other, side=BoardSide.BACK)}, changed, options)


def test_local_shape_translation_matches_direct_geometry_at_arbitrary_angles():
    rng = random.Random(19)
    for kind in PadShape:
        for side in BoardSide:
            for angle in (0, 45, 90, "12.3456789", "359.999"):
                pad = FootprintPad("1", Point(733333, -923457), Size(1234567, 987655),
                                   shape=kind, rotation_degrees="17.3", roundrect_ratio_ppm=173333)
                pose = Placement("U", "pad", Point(rng.randrange(-10**9, 10**9),
                    rng.randrange(-10**9, 10**9)), angle, side)
                local, _ = _local_land_shapes(IdentityKey((pad,)), pose.rotation_degrees, side)
                shape = local[0][1]
                translated = RoundedConvexShape(tuple(Point(p.x_nm + pose.position.x_nm,
                    p.y_nm + pose.position.y_nm) for p in shape.spine), shape.radius_nm)
                assert translated == placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose)


def linear_collision(board, pose, tracks, vias):
    """Exhaustive narrow phase with no cached shapes or bounds rejection."""
    owners = {pad: net.name for net in board.nets for pad in net.pads}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    stack = board.stackup.copper_layers
    side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    def clearance(first, second):
        return max((board.rules.minimum_clearance_nm,
            *(rules[n].clearance_nm or 0 for n in (first, second) if n in rules)))
    for pad in board.footprints[pose.footprint].pads:
        if pad.kind is PadKind.APERTURE:
            continue
        shape = placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose)
        net = owners.get(PadReference(pose.reference, pad.number))
        layers = {side} if pad.kind is PadKind.SMD else set(stack)
        for track in tracks:
            if track.layer in layers and track.net != net and not shapes_clear(shape,
                RoundedConvexShape((track.start, track.end), track.width_nm // 2), clearance(net, track.net)):
                return True
        for via in vias:
            low, high = sorted((stack.index(via.from_layer), stack.index(via.to_layer)))
            if via.net == net or not layers & set(stack[low:high + 1]):
                continue
            if (not shapes_clear(shape, RoundedConvexShape((via.position,), via.size_nm // 2), clearance(net, via.net))
                or not shapes_clear(shape, RoundedConvexShape((via.position,), via.drill_nm // 2),
                                    board.rules.minimum_hole_clearance_nm)):
                return True
    return False


def test_placement_broad_phase_matches_linear_oracle_with_changing_rules_and_geometry():
    from pcbir import NetRoutingRule
    board, _, _ = board_fixture()
    rng = random.Random(1701)
    for i in range(120):
        pad = replace(board.footprints["pad"].pads[0], shape=rng.choice(tuple(PadShape)),
                      rotation_degrees=rng.randrange(360))
        if i % 5 == 0:
            pad = replace(pad, kind=PadKind.THROUGH_HOLE, drill=Size(300000, 300000))
        fp = replace(board.footprints["pad"], pads=(pad,))
        bb = replace(board, footprints={fp.name: fp}, via_in_pad_rules=(), rules=replace(board.rules,
            minimum_hole_clearance_nm=rng.randrange(200001, 800000)),
            net_routing_rules=(NetRoutingRule("X", clearance_nm=rng.randrange(200001, 700000)),))
        pose = replace(bb.placements[i % 2], rotation_degrees=rng.randrange(360),
                       side=rng.choice(tuple(BoardSide)), position=Point.mm(10 + rng.random(), 10 + rng.random()))
        tracks = (TrackSegment(rng.choice(("GND", "X")), Point.mm(9, 10.2), Point.mm(12, 11.1),
                               rng.randrange(200000, 400000), rng.choice(tuple(CopperLayer))),)
        # Restrict random spans to the declared stack.
        if tracks[0].layer not in bb.stackup.copper_layers:
            tracks = (replace(tracks[0], layer=CopperLayer.FRONT),)
        vias = (Via(rng.choice(("GND", "X")), Point.mm(10.8, 10.5), 800001, 400001,
                    CopperLayer.INTERNAL_1 if i % 3 else CopperLayer.FRONT, CopperLayer.BACK),)
        assert _lands_meet_macro_copper(bb, pose, tracks, vias) == linear_collision(bb, pose, tracks, vias)

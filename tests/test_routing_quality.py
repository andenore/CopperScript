from dataclasses import replace
import pytest

from pcbir.physical import (BoardOutline, BoardSide, CopperLayer, DecouplingLink,
    FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
    Placement, Point, Size, TrackSegment, ComponentPlacementRule, CopperKeepout,
    PolygonRing, PolygonWithHoles, nm_from_mm)
from pcbir.route_quality import proper_same_net_crossing
from pcbir.fanout import FanoutOptions, _candidates, _reserve, route_fanout
from pcbir.escape_assignment import EscapeConflicts
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.decoupling import route_decouplers, improve_decoupling_placement
from pcbir.placement import PlacementPlannerOptions, transformed_pad_position
from pcbir import compile_source, prototype_physicalize


def wire(a, b, layer=CopperLayer.FRONT):
    return TrackSegment("P", Point.mm(*a), Point.mm(*b), nm_from_mm("0.2"), layer)


@pytest.mark.parametrize("other,expected", [
    (wire((2, 0), (0, 2)), True),
    (wire((2, 2), (3, 0)), False),  # Shared endpoint.
    (wire((1, 1), (2, 0)), False),  # T junction.
    (wire((.5, .5), (3, 3)), False),  # Common trunk.
    (wire((2, 0), (0, 2), CopperLayer.BACK), False),
])
def test_crossing_is_quality_not_electrical_clearance(other, expected):
    assert proper_same_net_crossing(wire((0, 0), (2, 2)), other) is expected


def test_greedy_and_joint_assignment_reject_the_same_net_x():
    board = PhysicalBoard("quality", BoardOutline.rectangle(10, 10), {}, (), (PhysicalNet("P", ()),))
    first, second = (wire((2, 2), (4, 4)),), (wire((4, 2), (2, 4)),)
    index = RoutingClearanceIndex(board)
    tracks, vias = [], []
    assert _reserve(board, index, tracks, vias, (first, None))
    assert not _reserve(board, index, tracks, vias, (second, None))
    conflicts = EscapeConflicts(board, 10)
    assert not conflicts.compatible((first, None), (second, None))
    assert conflicts.compatible((first, None), ((wire((4, 4), (5, 3)),), None))


def test_straight_outward_precedes_diagonal_even_at_farther_radius():
    settings = FanoutOptions(maximum_radius_nm=nm_from_mm("1"))
    points = tuple(_candidates(Point.mm(6, 5.1), Point.mm(5, 5), settings))
    assert points.index(Point.mm(7, 5.1)) < points.index(Point.mm(6.5, 5.6))
    rotated = tuple(_candidates(Point.mm(6, 6), Point.mm(5, 5), settings, diagonal_normal=True))
    assert rotated[0] == Point.mm(6.5, 6.5)


def bypass_board():
    chip = PhysicalFootprint("chip", (FootprintPad("1", Point.mm(1, 0), Size.mm(.4, .6)),
                                     FootprintPad("2", Point.mm(-1, 0), Size.mm(.4, .6))), Size.mm(2, 2))
    cap = PhysicalFootprint("cap", (FootprintPad("1", Point.mm(-.5, 0), Size.mm(.5, .5)),
                                   FootprintPad("2", Point.mm(.5, 0), Size.mm(.5, .5))), Size.mm(1.5, .8))
    return PhysicalBoard("bypass", BoardOutline.rectangle(25, 25), {"chip": chip, "cap": cap},
                         (Placement("U", "chip", Point.mm(8, 8)), Placement("C", "cap", Point.mm(15, 15))),
                         (PhysicalNet("P", (PadReference("U", "1"), PadReference("C", "1"))),
                          PhysicalNet("G", (PadReference("C", "2"),))),
                         decoupling_links=(DecouplingLink(PadReference("C", "1"), PadReference("U", "1"),
                                                         PadReference("C", "2"), "P", "G"),))


def test_bypass_placement_moves_feed_land_near_actual_pin_without_constraint():
    board = bypass_board()
    options = PlacementPlannerOptions(component_clearance_nm=nm_from_mm("0.2"))
    poses = {p.reference: p for p in board.placements}
    result, moved = improve_decoupling_placement(board, poses, options)
    assert moved == 1 and result["U"] == poses["U"]
    a, b = transformed_pad_position(board, result["U"], "1"), transformed_pad_position(board, result["C"], "1")
    assert abs(a.x_nm-b.x_nm)+abs(a.y_nm-b.y_nm) < nm_from_mm(3)
    fixed = replace(board, placement_rules=(ComponentPlacementRule("C", fixed_position=poses["C"].position),))
    assert improve_decoupling_placement(fixed, poses, options) == (poses, 0)


def test_default_planner_consumes_decoupling_role_without_distance_rules():
    from pcbir.placement import generate_placement_candidates
    board = bypass_board()
    board = replace(board, placement_rules=(ComponentPlacementRule("U", fixed_position=board.placements[0].position),))
    candidates = generate_placement_candidates(board, PlacementPlannerOptions(
        candidate_count=1, analytical_iterations=0, refinement_passes=0, escape_spacing_passes=0))
    for candidate in candidates:
        poses = {p.reference: p for p in candidate.placements}
        a, b = transformed_pad_position(board, poses["U"], "1"), transformed_pad_position(board, poses["C"], "1")
        assert abs(a.x_nm-b.x_nm)+abs(a.y_nm-b.y_nm) < nm_from_mm(3)


def test_direct_surface_route_reserved_before_via_fanout_and_is_idempotent():
    board = bypass_board()
    result = route_decouplers(board)
    assert not result.pending and result.created_tracks and not result.board.vias
    assert route_decouplers(result.board).created_tracks == ()
    fanout = route_fanout(board, FanoutOptions(minimum_component_pads=2, maximum_neighbor_distance_nm=nm_from_mm(3)))
    assert result.created_tracks == fanout.created_tracks
    assert not fanout.created_vias


def test_opposite_side_bypass_is_explicitly_pending_and_never_moved_after_route():
    board = bypass_board()
    board = replace(board, placements=(board.placements[0], replace(board.placements[1], side=BoardSide.BACK)))
    result = route_decouplers(board)
    assert result.pending and not result.created_tracks and not result.board.vias
    routed = route_decouplers(bypass_board()).board
    with pytest.raises(ValueError, match="unrouted"):
        improve_decoupling_placement(routed, {p.reference: p for p in routed.placements}, PlacementPlannerOptions())


def test_bypass_maze_reaches_exact_off_grid_capacitor_without_vias():
    from pcbir.pin_escape import checked_access_paths
    board = bypass_board()
    obstacle = TrackSegment("OBSTACLE", Point.mm(11, 7.6), Point.mm(11, 8.4),
                            nm_from_mm("0.2"), CopperLayer.FRONT)
    board = replace(board, placements=(board.placements[0],
        replace(board.placements[1], position=Point.mm(13.037, 8.023))),
        nets=(*board.nets, PhysicalNet("OBSTACLE", ())), tracks=(obstacle,))
    index = RoutingClearanceIndex(board)
    start, end = Point.mm(9, 8), Point.mm(12.537, 8.023)
    assert not tuple(checked_access_paths(board, index, "P", start, end,
                                         board.rules.default_track_width_nm, CopperLayer.FRONT))
    result = route_decouplers(board)
    assert not result.pending and not result.board.vias
    assert result.created_tracks[0].start == start
    assert result.created_tracks[-1].end == end
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer)
               for t in result.created_tracks)
    assert route_decouplers(result.board).created_tracks == ()


def test_surface_maze_is_bounded_and_cannot_cross_keepout():
    from pcbir.surface_path import surface_path_between
    board = bypass_board()
    wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing(
        (Point.mm(11, 1), Point.mm(12, 1), Point.mm(12, 24), Point.mm(11, 24)))))
    board = replace(board, copper_keepouts=(wall,))
    index = RoutingClearanceIndex(board)
    assert surface_path_between(board, index, "P", Point.mm(9, 8), Point.mm(14.5, 15),
        board.rules.default_track_width_nm, CopperLayer.FRONT, state_budget=40) is None
    with pytest.raises(ValueError, match="bounds"):
        surface_path_between(board, index, "P", Point.mm(9, 8), Point.mm(14.5, 15),
            board.rules.default_track_width_nm, CopperLayer.FRONT, state_budget=0)


def test_placement_prefers_actual_direct_access_not_just_nearest_pad_distance():
    from pcbir.pin_escape import checked_access_paths
    board = bypass_board()
    chip = replace(board.footprints["chip"], pads=(
        FootprintPad("1", Point.mm(1, 0), Size.mm(.8, .225)),
        FootprintPad("2", Point.mm(1, .4), Size.mm(.8, .225))))
    board = replace(board, footprints={**board.footprints, "chip": chip},
        rules=replace(board.rules, minimum_clearance_nm=nm_from_mm("0.09")),
        nets=(board.nets[0], replace(board.nets[1], pads=(*board.nets[1].pads, PadReference("U", "2")))))
    poses, moved = improve_decoupling_placement(board, {p.reference: p for p in board.placements},
                                               PlacementPlannerOptions())
    assert moved == 1
    scratch = replace(board, placements=tuple(poses.values()))
    start = transformed_pad_position(scratch, poses["U"], "1")
    end = transformed_pad_position(scratch, poses["C"], "1")
    assert tuple(checked_access_paths(scratch, RoutingClearanceIndex(scratch), "P", start, end,
                                     scratch.rules.default_track_width_nm, CopperLayer.FRONT))


def test_decoupler_placement_respects_earlier_fixed_bypass_corridor():
    board = bypass_board()
    chip = replace(board.footprints["chip"], pads=(
        FootprintPad("1", Point.mm(1, -.2), Size.mm(.8, .225)),
        FootprintPad("2", Point.mm(1, .2), Size.mm(.8, .225))))
    first = replace(board.placements[1], reference="C1", position=Point.mm(11, 7), rotation_degrees=90)
    second = replace(board.placements[1], reference="C2")
    links = (DecouplingLink(PadReference("C1", "1"), PadReference("U", "1"),
                           PadReference("C1", "2"), "P1", "G"),
             DecouplingLink(PadReference("C2", "1"), PadReference("U", "2"),
                           PadReference("C2", "2"), "P2", "G"))
    board = replace(board, footprints={**board.footprints, "chip": chip},
        placements=(board.placements[0], first, second),
        rules=replace(board.rules, minimum_clearance_nm=nm_from_mm("0.09")),
        nets=(PhysicalNet("P1", (links[0].target, links[0].capacitor)),
              PhysicalNet("P2", (links[1].target, links[1].capacitor)),
              PhysicalNet("G", (links[0].return_pad, links[1].return_pad))),
        placement_rules=(ComponentPlacementRule("C1", fixed_position=first.position, fixed_rotation_degrees=90),),
        decoupling_links=links)
    poses, moved = improve_decoupling_placement(board, {p.reference: p for p in board.placements},
                                               PlacementPlannerOptions())
    assert poses["C1"] == first and moved == 1
    result = route_decouplers(replace(board, placements=tuple(poses.values())))
    assert not result.pending and not result.board.vias
    index = RoutingClearanceIndex(result.board)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer)
               for t in result.created_tracks)


@pytest.mark.parametrize("zone", [False, True])
def test_package_preflight_preserves_the_decoupling_prefix(zone):
    from pcbir.package_access import preflight_package_access, PackageAccessOptions
    from pcbir.routing import route_global
    from pcbir.physical import CopperZone
    board = bypass_board()
    if zone:
        board = replace(board, zones=(CopperZone("rail", "P", (CopperLayer.BACK,),
            PolygonWithHoles(PolygonRing((Point.mm(1, 1), Point.mm(24, 1), Point.mm(24, 24), Point.mm(1, 24))))),))
    prefix = route_decouplers(board).created_tracks
    result = preflight_package_access(board, route_global(board), FanoutOptions(),
                                     options=PackageAccessOptions(maximum_pattern_trials=0))
    assert all(t in result.board.tracks for t in prefix)
    assert all(t in result.fanout.created_tracks for t in prefix)
    assert not result.hard_findings


def test_nested_module_decoupling_endpoint_is_qualified():
    from pcbir.elaborate import elaborate, _expand_one, _Template
    body = compile_source(SOURCE)
    template = _Template("Bypass", {}, elaborate(body))
    parent = elaborate(replace(body, components=(), nets=(), supplies=()))
    combined = _expand_one(parent, "M", template)
    capacitor = next(c for c in combined.components if c.ref == "M/C")
    assert capacitor.properties["decouples"] == "M/U.IN"
    nested = _expand_one(parent, "N", _Template("Nested", {}, combined))
    assert next(c for c in nested.components if c.ref == "N/M/C").properties["decouples"] == "N/M/U.IN"


SOURCE = '''board Test {
    use library "tiny";
    component U: REGULATOR_3V3 { footprint = "SOT-23-3"; }
    component C: CAPACITOR { footprint = "0402"; role = decoupling; decouples = "U.IN"; }
    net P { U.IN; C.1; } net G { U.GND; C.2; }
    supply GROUND { net = G; voltage = 0V; external = true; }
}'''


def test_decoupling_role_lowers_to_actual_numbered_lands_and_binds_fingerprints():
    from pcbir.routing import _placement_fingerprint
    from pcbir.drc import physical_board_digest
    board = prototype_physicalize(compile_source(SOURCE))
    assert len(board.decoupling_links) == 1
    assert board.decoupling_links[0].target.component == "U"
    no_role = replace(board, decoupling_links=())
    assert _placement_fingerprint(board) != _placement_fingerprint(no_role)
    assert physical_board_digest(board) != physical_board_digest(no_role)


@pytest.mark.parametrize("source", [
    SOURCE.replace('"U.IN"', '"MISSING.IN"'),
    SOURCE.replace('"U.IN"', '"U.OUT"'),
    SOURCE.replace("CAPACITOR", "RESISTOR"),
])
def test_invalid_decoupling_associations_do_not_silently_fall_back(source):
    with pytest.raises((ValueError, KeyError)):
        prototype_physicalize(compile_source(source))

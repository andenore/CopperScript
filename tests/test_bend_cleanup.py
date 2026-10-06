from dataclasses import replace
from math import hypot

from pcbir import (
    BoardOutline, CopperLayer, CopperZone, DetailedRouterOptions, FootprintPad, GlobalRouterOptions,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing,
    PolygonWithHoles, Size, TrackSegment, Via, nm_from_mm, route_detailed, route_global,
    run_physical_drc,
)
from pcbir.drc import explicit_copper_connectivity
from pcbir.geometry import RoundedConvexShape
from pcbir.route_smoothing import smooth_owned_tracks, turn_score
from pcbir.routing_clearance import RoutingClearanceIndex


def board(*pads, nets=None):
    footprint = PhysicalFootprint("one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                                  Size.mm(1, 1))
    placements = tuple(Placement(f"J{i}", footprint.name, Point.mm(*xy)) for i, xy in enumerate(pads, 1))
    nets = nets or (PhysicalNet("S", tuple(PadReference(p.reference, "1") for p in placements)),
                    PhysicalNet("X", ()))
    return PhysicalBoard("bends", BoardOutline.rectangle(20, 12), {footprint.name: footprint},
                         placements, nets)


def track(a, b, net="S", width="0.2"):
    return TrackSegment(net, Point.mm(*a), Point.mm(*b), nm_from_mm(width), CopperLayer.FRONT)


def chain(*points, net="S"):
    return tuple(track(a, b, net) for a, b in zip(points, points[1:]))


def smooth(base, owned):
    final = replace(base, tracks=(*base.tracks, *owned))
    return smooth_owned_tracks(base, owned, RoutingClearanceIndex(final))


def sharp_turns(tracks):
    ends = {}
    for t in tracks:
        ends.setdefault((t.layer, t.start), []).append((t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm))
        ends.setdefault((t.layer, t.end), []).append((t.start.x_nm - t.end.x_nm, t.start.y_nm - t.end.y_nm))
    return sum(turn_score(*vectors) > 0 for vectors in ends.values() if len(vectors) == 2)


def length(tracks):
    return sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm) for t in tracks)


def clean(base, tracks):
    findings = run_physical_drc(replace(base, tracks=(*base.tracks, *tracks))).findings
    return [f.code for f in findings if f.severity.value == "error" and f.code != "DRC-ROUTE-INCOMPLETE"]


def test_short_leg_jog_that_chamfering_cannot_fit_becomes_one_45_degree_transition():
    base = board((3, 6), (17, 6.3))
    owned = chain((3, 6), (8, 6), (8, 6.3), (17, 6.3))
    result = smooth(base, owned)
    assert sharp_turns(owned) == 2 and sharp_turns(result) == 0
    assert length(result) < length(owned)
    assert all(t.start.x_nm == t.end.x_nm or t.start.y_nm == t.end.y_nm
               or abs(t.end.x_nm - t.start.x_nm) == abs(t.end.y_nm - t.start.y_nm) for t in result)
    assert clean(base, result) == []
    # Deterministic and idempotent: nothing left to improve keeps identities.
    assert smooth(base, owned) == result
    assert smooth(base, result) == result


def test_confirmed_hairpin_detour_is_short_cut_without_losing_either_pad():
    base = board((3, 6), (6, 9))
    owned = chain((3, 6), (9, 6), (9, 6.6), (6, 6.6), (6, 9))
    result = smooth(base, owned)
    assert sharp_turns(result) == 0
    assert length(result) < 0.5 * length(owned)
    assert clean(base, result) == []


def test_corner_hemmed_in_by_foreign_copper_is_constrained_and_kept():
    obstacle = chain((3.5, 6.6), (9.4, 6.6), (9.4, 10.6), net="X")
    base = replace(board((3, 6), (10, 10)), tracks=obstacle)
    owned = chain((3, 6), (10, 6), (10, 10))
    index = RoutingClearanceIndex(base)
    assert all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in owned)
    assert smooth(base, owned) == owned


def test_via_corner_branch_and_immutable_copper_are_anchors():
    base = board((3, 6), (10, 10), (14, 6))
    owned = chain((3, 6), (10, 6), (10, 10))
    via = Via("S", Point.mm(10, 6), nm_from_mm(.6), nm_from_mm(.3), CopperLayer.FRONT, CopperLayer.BACK)
    assert smooth(replace(base, vias=(via,)), owned) == owned
    branch = (*owned, track((10, 6), (14, 6)))
    assert smooth(base, branch) == branch
    # Immutable same-net copper is never an input and anchors what touches it.
    locked = replace(base, tracks=(track((10, 6), (14, 6)),))
    assert smooth(locked, owned) == owned


def test_mid_chain_same_net_pad_contact_is_never_bypassed():
    base = board((3, 6), (17, 6), (5, 3))
    owned = chain((3, 6), (5, 6), (5, 3), (10, 3), (10, 6), (17, 6))
    result = smooth(base, owned)
    routed = replace(base, tracks=result)
    assert clean(base, result) == []
    assert explicit_copper_connectivity(routed).net_connected(routed.nets[0])
    # The straight (3, 6) -> (17, 6) shortcut would drop the middle land.
    index = RoutingClearanceIndex(board((5, 3)))
    assert any(not index.pad_copper_clear(RoundedConvexShape((t.start, t.end), t.width_nm // 2), (t.layer,))
               for t in result)
    assert length(result) <= length(owned)


def test_straight_and_45_degree_routes_and_zone_nets_are_unchanged():
    base = board((3, 6), (17, 6))
    owned = chain((3, 6), (5, 6), (7, 8), (17, 8), (17, 6))
    straight = chain((3, 6), (17, 6))
    assert smooth(base, straight) == straight
    ground = (PhysicalNet("S", (PadReference("J1", "1"), PadReference("J2", "1"))),)
    zoned = replace(base, zones=(CopperZone("plane", "S", (CopperLayer.BACK,), PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(19, 1), Point.mm(19, 11), Point.mm(1, 11))))),), nets=ground)
    assert smooth(zoned, owned) == owned


def test_detailed_router_smoothing_option_keeps_connectivity_and_never_lengthens():
    base = board((3, 3), (17, 9))
    guides = route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    options = DetailedRouterOptions(pitch_nm=nm_from_mm(".5"), maximum_search_states=20_000)
    plain = route_detailed(base, guides, options)
    smoothed = route_detailed(base, guides, replace(options, route_smoothing=True))
    assert smoothed.metrics.unrouted_net_count == plain.metrics.unrouted_net_count == 0
    assert smoothed.metrics.total_length_nm <= plain.metrics.total_length_nm
    assert sharp_turns(smoothed.board.tracks) <= sharp_turns(plain.board.tracks)
    assert clean(base, smoothed.board.tracks) == []
    assert route_detailed(base, guides, replace(options, route_smoothing=True)).board == smoothed.board

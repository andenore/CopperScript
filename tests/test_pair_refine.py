from dataclasses import replace

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad, NetRoutingRule,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    PolygonRing, PolygonWithHoles, RouteKind, Size, TrackSegment, route_global,
    run_physical_drc, nm_from_mm)
from pcbir.critical import _improve_pair_spine, _route_pair, _track_length, _validate_candidate
from pcbir.pair_search import PairSearchCandidate, _lane_paths, _ports, _tracks
from pcbir.pair_refine import PairRefinementStats, _compact, paired_shortcuts
from pcbir.routing_clearance import RoutingClearanceIndex


def fixture():
    fp = PhysicalFootprint("pair", tuple(FootprintPad(str(i + 1), Point.mm(0, y), Size.mm(.4, .4))
                                         for i, y in enumerate((-.5, .5))), Size.mm(1, 2))
    rules = tuple(NetRoutingRule(name, RouteKind.DIFFERENTIAL, differential_partner=partner,
        width_nm=nm_from_mm(.25), pair_gap_nm=nm_from_mm(.2), max_vias=0,
        max_skew_nm=nm_from_mm(10), maximum_uncoupled_length_nm=nm_from_mm(10))
                  for name, partner in (("A", "B"), ("B", "A")))
    board = PhysicalBoard("PairRefine", BoardOutline.rectangle(40, 32), {fp.name: fp},
        (Placement("J1", fp.name, Point.mm(5, 12)), Placement("J2", fp.name, Point.mm(35, 12))),
        tuple(PhysicalNet(name, (PadReference("J1", str(i + 1)), PadReference("J2", str(i + 1))))
              for i, name in enumerate(("A", "B"))), net_routing_rules=rules)
    index = RoutingClearanceIndex(board)
    width, offset = nm_from_mm(.25), nm_from_mm(.225)
    start = next(p for p in _ports(board, index, "A", "B", Point.mm(5, 11.5), Point.mm(5, 12.5),
        "J1", width, offset, board.rules.minimum_clearance_nm, CopperLayer.FRONT) if p.center == Point.mm(6, 12))
    end = next(p for p in _ports(board, index, "A", "B", Point.mm(35, 11.5), Point.mm(35, 12.5),
        "J2", width, offset, board.rules.minimum_clearance_nm, CopperLayer.FRONT) if p.center == Point.mm(34, 12))
    points = tuple(Point.mm(x, y) for x, y in ((6, 12), (7, 12), (8, 12), (10, 12), (14, 16),
        (14, 20), (18, 24), (24, 24), (28, 20), (28, 16), (32, 12), (34, 12)))
    lanes = _lane_paths(points, start.heading, (end.heading + 4) % 8, offset, start.sign)
    assert lanes is not None
    tracks = tuple((*lead, *_tracks(name, lane, width, CopperLayer.FRONT),
                    *(replace(t, start=t.end, end=t.start) for t in reversed(tail)))
                   for name, lane, lead, tail in zip(("A", "B"), lanes,
                                                   (start.first, start.second), (end.first, end.second)))
    candidate = PairSearchCandidate(*tracks, 12, 1, points, start, end)
    return board, candidate, rules


def test_joint_shortcuts_shorten_detour_preserve_exact_ports_and_are_deterministic():
    board, candidate, rules = fixture()
    before = board
    stats = PairRefinementStats()
    proposals = list(paired_shortcuts(board, candidate, *rules, stats=stats))
    assert proposals == list(paired_shortcuts(board, candidate, *rules))
    assert stats.candidates == len(proposals) > 0 and stats.attempts <= 256
    assert all(_track_length(new) <= _track_length(old) for new, old in zip(proposals[-1], (candidate.first, candidate.second)))
    assert sum(map(_track_length, proposals[-1])) < sum(map(_track_length, (candidate.first, candidate.second)))
    for proposal in proposals:
        for lane, lead, tail in zip(proposal, (candidate.start_port.first, candidate.start_port.second),
                                   (candidate.end_port.first, candidate.end_port.second)):
            assert lane[:len(lead)] == lead
            assert lane[-len(tail):] == tuple(replace(t, start=t.end, end=t.start) for t in reversed(tail))
        assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
                       for f in run_physical_drc(replace(board, tracks=(*proposal[0], *proposal[1]))).findings)
    assert board == before and not board.tracks


@pytest.mark.parametrize("obstacle", ["keepout", "reservation"])
def test_shortcut_cannot_cut_a_keepout_or_reserved_foreign_route(obstacle):
    board, candidate, rules = fixture()
    if obstacle == "keepout":
        board = replace(board, copper_keepouts=(CopperKeepout("block", (CopperLayer.FRONT,),
            PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in ((19, 8), (21, 8), (21, 18), (19, 18)))))),))
    else:
        board = replace(board, nets=(*board.nets, PhysicalNet("FOREIGN", ())),
            tracks=(TrackSegment("FOREIGN", Point.mm(20, 8), Point.mm(20, 18), nm_from_mm(.5), CopperLayer.FRONT),))
    proposals = list(paired_shortcuts(board, candidate, *rules))
    assert proposals
    for proposal in proposals:
        routed = replace(board, tracks=(*board.tracks, *proposal[0], *proposal[1]))
        assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-KEEPOUT", "DRC-OPEN-NET"}
                       for f in run_physical_drc(routed).findings)


def test_refinement_is_bounded_and_missing_provenance_fails_closed():
    board, candidate, rules = fixture()
    stats = PairRefinementStats()
    list(paired_shortcuts(board, candidate, *rules, maximum_attempts=2, stats=stats))
    assert stats.attempts <= 2
    spent = PairRefinementStats(attempts=2)
    assert not list(paired_shortcuts(board, candidate, *rules, maximum_attempts=2, stats=spent))
    assert spent.attempts == 2
    assert not list(paired_shortcuts(board, replace(candidate, spine=()), *rules))
    assert not list(paired_shortcuts(board, replace(candidate, start_port=None), *rules))
    assert not list(paired_shortcuts(board, replace(candidate, first=candidate.first[1:]), *rules))
    assert not list(paired_shortcuts(board, replace(candidate, spine=(candidate.spine[0], candidate.spine[-1])), *rules))
    with pytest.raises(ValueError):
        list(paired_shortcuts(board, candidate, *rules, maximum_attempts=0))
    points = (Point(0, 0), Point(1, 0), Point(2, 0), Point(1, 0))
    assert _compact(points) == (Point(0, 0), Point(2, 0), Point(1, 0))


@pytest.mark.parametrize("defect", ["gap", "profile", "native_open", "width", "layer"])
def test_owner_preserves_complete_accepted_incumbent_on_bad_shortcut(monkeypatch, defect):
    board, candidate, rules = fixture()
    if defect == "profile":
        rules = tuple(replace(rule, max_skew_nm=100) for rule in rules)
        board = replace(board, net_routing_rules=rules)
    routes = {r.net: r for r in route_global(board).routes}
    result, tracks, vias = _route_pair(board, *rules, routes, exact_tracks=(candidate.first, candidate.second))
    result, tracks, vias = _validate_candidate(board, result, tracks, vias, [], [])
    assert result.connected
    proposal = list(paired_shortcuts(board, candidate, *rules))[-1]
    if defect == "gap":
        proposal = proposal[0], proposal[0]
    elif defect == "profile":
        # Accepted incumbent fits its own skew; the shorter path need not fit.
        lane = list(proposal[1])
        i, edge = next((i, t) for i, t in enumerate(lane) if t.start.y_nm == t.end.y_nm and t.end.x_nm - t.start.x_nm > nm_from_mm(2))
        bump = Point(edge.start.x_nm + nm_from_mm(.1), edge.start.y_nm + nm_from_mm(.1))
        far = Point(edge.end.x_nm - nm_from_mm(.1), bump.y_nm)
        lane[i:i + 1] = [replace(edge, end=bump), replace(edge, start=bump, end=far), replace(edge, start=far)]
        proposal = proposal[0], tuple(lane)
        rejected = _route_pair(board, *rules, routes, exact_tracks=proposal)[0]
        assert any("skew" in diagnostic for diagnostic in rejected.diagnostics)
    elif defect == "width":
        proposal = tuple(tuple(replace(t, width_nm=nm_from_mm(.2)) for t in lane) for lane in proposal)
    elif defect == "layer":
        proposal = tuple(tuple(replace(t, layer=CopperLayer.BACK) for t in lane) for lane in proposal)
    else:
        proposal = proposal[0][1:], proposal[1]
    monkeypatch.setattr("pcbir.critical.paired_shortcuts", lambda *_args, **_kwargs: iter((proposal,)))
    selected, selected_tracks, selected_vias = _improve_pair_spine(board, board, *rules, routes, candidate,
                                                               result, tracks, vias, [], [])
    assert selected == result and selected_tracks == tracks and selected_vias == vias

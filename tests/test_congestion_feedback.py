import json
from dataclasses import replace
from math import hypot

import numpy as np
import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, CopperZone, FootprintPad,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, PlacementPlannerOptions,
    PlacementTarget, Point, PolygonRing, PolygonWithHoles, RigidPlacementCluster, RigidPlacementMember,
    Size, Stackup, TrackSegment, GlobalRouterOptions, nm_from_mm)
from pcbir.clusters import footprint_geometry_digest
from pcbir.placement import placement_solution_is_legal
from pcbir.routing import GlobalRoutingMetrics, GlobalRoutingResult, GlobalRoutingStatus
import pcbir.congestion_feedback as cf
from pcbir.congestion_feedback import (UTILISATION_SENTINEL, CongestionMap, Proposal, estimate_demand,
    propose_moves, screen)

MM = nm_from_mm(1)


def cmap(demand, capacity=None, **kw):
    demand = np.asarray(demand, dtype=float)
    capacity = np.ones_like(demand) if capacity is None else capacity
    ny, nx = demand.shape
    return CongestionMap(tile_nm=kw.pop("tile_nm", MM), x0_nm=kw.pop("x0_nm", 0), y0_nm=kw.pop("y0_nm", 0),
                         nx=nx, ny=ny, demand=demand, capacity=capacity, source=kw.pop("source", "rudy"), **kw)


def rectangle(x0, y0, x1, y1):
    return PolygonWithHoles(PolygonRing((Point.mm(x0, y0), Point.mm(x1, y0), Point.mm(x1, y1), Point.mm(x0, y1))))


# ---------------------------------------------------------------- CongestionMap


def test_utilisation_sentinel_and_empty_tiles():
    m = cmap([[1.0, 0.0], [0.0, 2.0]], np.array([[0.0, 0.0], [1.0, 4.0]]))
    u = m.utilisation()
    assert u[0, 0] == UTILISATION_SENTINEL and np.isfinite(u).all()
    assert u[0, 1] == 0.0 and u[1, 0] == 0.0 and u[1, 1] == 0.5
    with pytest.raises(ValueError):
        m.demand[0, 0] = 5.0  # frozen arrays
    with pytest.raises(ValueError):
        cmap(np.zeros((2, 2)), np.ones((3, 2)))


def test_tile_lookup_bounds_and_value():
    m = cmap(np.arange(6, dtype=float).reshape(2, 3), x0_nm=10 * MM, y0_nm=20 * MM)
    assert m.tile_of(Point.mm(11.5, 21.2)) == (1, 1)
    assert m.tile_of(Point.mm(-50, 100)) == (0, 1)  # clipped to the lattice
    assert m.tile_bounds(2, 1) == (12 * MM, 21 * MM, 13 * MM, 22 * MM)
    assert m.value_at(Point.mm(12.5, 20.5)) == 2.0
    with pytest.raises(IndexError):
        m.tile_bounds(3, 0)


def test_gradient_points_down_a_ramp_and_is_zero_when_flat():
    ramp_x = np.tile(np.arange(9, dtype=float), (9, 1))  # utilisation grows with x
    dx, dy = cmap(ramp_x).gradient_at(Point.mm(4.5, 4.5))
    assert dx == pytest.approx(-1.0) and dy == pytest.approx(0.0, abs=1e-9)
    dx, dy = cmap(ramp_x.T).gradient_at(Point.mm(4.5, 4.5))  # grows with y
    assert dy == pytest.approx(-1.0) and dx == pytest.approx(0.0, abs=1e-9)
    diagonal = ramp_x + ramp_x.T
    dx, dy = cmap(diagonal).gradient_at(Point.mm(0.5, 0.5), radius_tiles=3)  # clipped corner window
    assert dx == pytest.approx(-2 ** -0.5) and dy == pytest.approx(-2 ** -0.5)
    assert hypot(dx, dy) == pytest.approx(1.0)
    assert cmap(np.full((5, 5), 0.3)).gradient_at(Point.mm(2.5, 2.5)) == (0.0, 0.0)


def test_gradient_ignores_tiles_without_capacity():
    demand = np.zeros((5, 5))
    demand[:, 3:] = 0.0
    demand[:, :3] = np.array([0.0, 1.0, 2.0])  # rises toward the board edge at x = 3
    capacity = np.ones((5, 5))
    capacity[:, 3:] = 0.0  # off the board: zero demand, but no capacity either
    dx, _ = cmap(demand, capacity).gradient_at(Point.mm(2.5, 2.5))
    assert dx == pytest.approx(-1.0)


def test_hot_tiles_order_is_utilisation_then_row_then_column():
    demand = np.zeros((3, 3))
    demand[2, 0] = demand[0, 2] = demand[1, 1] = 0.5
    demand[2, 2] = 0.9
    m = cmap(demand)
    assert m.hot_tiles(10) == ((2, 2, 0.9), (2, 0, 0.5), (1, 1, 0.5), (0, 2, 0.5))
    assert m.hot_tiles(2) == ((2, 2, 0.9), (2, 0, 0.5)) and m.hot_tiles(0) == ()


def test_combined_requires_same_lattice_and_keeps_own_capacity():
    p, q = Point.mm(1, 1), Point.mm(2, 2)
    a = cmap(np.ones((2, 2)), np.full((2, 2), 4.0), source="unified_router", failed_points=(("A", p),))
    b = cmap(np.full((2, 2), 2.0), np.full((2, 2), 9.0), failed_points=(("A", p), ("B", q)))
    c = a.combined(b, 0.5)
    assert c.source == "combined" and np.array_equal(c.demand, np.full((2, 2), 2.0))
    assert np.array_equal(c.capacity, a.capacity)
    assert c.failed_points == (("A", p), ("B", q))
    with pytest.raises(ValueError):
        a.combined(cmap(np.ones((2, 2)), x0_nm=1), 1.0)
    with pytest.raises(ValueError):
        a.combined(cmap(np.ones((2, 3))), 1.0)


def test_to_json_round_trips_the_lattice_and_maps_compare_by_value():
    m = cmap(np.array([[0.0, 3.0]]), x0_nm=-5 * MM, y0_nm=7 * MM, failed_points=(("N", Point.mm(1, 2)),))
    document = json.loads(m.to_json())
    again = CongestionMap(**document["lattice"], demand=m.demand, capacity=m.capacity, source=document["source"],
                          failed_points=tuple((item["net"], Point(item["x_nm"], item["y_nm"]))
                                              for item in document["failed_points"]))
    assert again == m and hash(again) == hash(m)
    assert document["hot_tiles"] == [{"ix": 1, "iy": 0, "utilisation": 3.0}]
    assert m != cmap(np.array([[0.0, 2.0]]), x0_nm=-5 * MM, y0_nm=7 * MM)


# ---------------------------------------------------------------- estimator


def pad_footprint(name="pad"):
    return PhysicalFootprint(name, (FootprintPad("1", Point(0, 0), Size.mm(.4, .4)),), Size.mm(1, 1))


def pad_board(nets, *, outline=None, **kw):
    fp = pad_footprint()
    placements, physical = [], []
    for net, points in nets.items():
        refs = []
        for index, (x, y) in enumerate(points):
            ref = f"{net}_{index}"
            placements.append(Placement(ref, fp.name, Point.mm(x, y)))
            refs.append(PadReference(ref, "1"))
        physical.append(PhysicalNet(net, tuple(refs)))
    return PhysicalBoard("Estimate", outline or BoardOutline.rectangle(30, 30), {fp.name: fp},
                         tuple(placements), tuple(physical), **kw)


def test_estimate_peaks_where_bounding_boxes_overlap():
    board = pad_board({"X": ((4.5, 4.5), (16.5, 16.5)), "Y": ((10.5, 10.5), (25.5, 25.5))})
    m = estimate_demand(board, MM)
    assert m.source == "rudy" and (m.nx, m.ny) == (30, 30)
    demand = m.demand.copy()
    for placement in board.placements:  # leave out the pin-density tiles
        ix, iy = m.tile_of(placement.position)
        demand[iy, ix] = 0.0
    iy, ix = np.unravel_index(np.argmax(demand), demand.shape)
    assert 10 <= ix <= 16 and 10 <= iy <= 16
    assert demand[13, 13] == pytest.approx(demand[6, 6] + demand[20, 20])
    assert m.demand[6, 6] > 0 and m.demand[2, 25] == 0.0


def test_estimate_is_in_lane_cells():
    board = pad_board({"S": ((5.5, 10.5), (20.5, 10.5))})
    m = estimate_demand(board, MM)
    # Box 15 mm + 1 mm by 1 mm (clamped): (16 + 1) mm of lane-wide wire, plus a
    # 0.4 mm pin stub per pad (single-pad footprints use their pad size as pitch).
    assert m.demand.sum() == pytest.approx((16 + 1) / 0.45 + 2 * 0.4 / 0.45)
    # Two signal layers of empty 1 mm tiles.
    assert m.capacity[25, 25] == pytest.approx(2 * (1 / 0.45) ** 2)


def test_zone_net_is_skipped_but_its_pads_keep_a_tap():
    nets = {"GND": ((4.5, 4.5), (25.5, 25.5))}
    plain = estimate_demand(pad_board(nets), MM)
    zoned = estimate_demand(pad_board(nets, zones=(CopperZone("gnd", "GND", (CopperLayer.BACK,),
                                                              rectangle(0, 0, 30, 30)),)), MM)
    assert plain.demand[15, 15] > 0 and zoned.demand[15, 15] == 0.0
    assert zoned.demand[4, 4] > 0 and zoned.demand.sum() < plain.demand.sum()
    only = estimate_demand(pad_board({**nets, "S": ((5.5, 20.5), (9.5, 20.5))}), MM, nets={"S"})
    assert only.demand[15, 15] == 0.0 and only.demand[20, 7] > 0


def test_capacity_zero_outside_outline_and_lower_under_pads_and_planes():
    triangle = BoardOutline((Point.mm(0, 0), Point.mm(20, 0), Point.mm(0, 20)))
    m = estimate_demand(pad_board({"S": ((3.5, 3.5), (8.5, 3.5))}, outline=triangle), MM)
    full = 2 * (1 / 0.45) ** 2
    assert m.capacity[18, 18] == 0.0 and m.capacity[10, 1] == pytest.approx(full)
    assert 0 < m.capacity[9, 10] < full  # on the diagonal edge
    grid = PhysicalFootprint("grid", tuple(FootprintPad(str(i * 4 + j), Point.mm(-.75 + .5 * i, -.75 + .5 * j),
                                                       Size.mm(.35, .35)) for i in range(4) for j in range(4)),
                             Size.mm(2.5, 2.5))
    board = PhysicalBoard("Pads", BoardOutline.rectangle(10, 10), {"grid": grid},
                          (Placement("U", "grid", Point.mm(5, 5)),), ())
    pads = estimate_demand(board, MM)
    assert pads.capacity[4, 4] < pads.capacity[1, 1] == pytest.approx(full)
    # 0.15 mm gaps hold no 0.25 mm sample spacing: the front is full, the back is free.
    assert pads.capacity[4, 4] == pytest.approx(full / 2)
    four = replace(board, stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                          CopperLayer.INTERNAL_2, CopperLayer.BACK)),
                   nets=(PhysicalNet("GND", ()),),
                   zones=(CopperZone("gnd", "GND", (CopperLayer.INTERNAL_1,), rectangle(0, 0, 10, 10)),))
    assert estimate_demand(four, MM).capacity[1, 1] == pytest.approx(3 * (1 / 0.45) ** 2)


# ---------------------------------------------------------------- proposer


def crowded_board():
    fp = PhysicalFootprint("r", (FootprintPad("1", Point.mm(-.5, 0), Size.mm(.5, .5)),
                                 FootprintPad("2", Point.mm(.5, 0), Size.mm(.5, .5))), Size.mm(2, 1))
    poses = (("A", 6, 8), ("B", 6, 10), ("C", 9, 9), ("D", 32, 12))
    board = PhysicalBoard("Crowded", BoardOutline.rectangle(40, 20), {fp.name: fp},
        tuple(Placement(ref, fp.name, Point.mm(x, y)) for ref, x, y in poses),
        (PhysicalNet("AD", (PadReference("A", "2"), PadReference("D", "1"))),
         PhysicalNet("BC", (PadReference("B", "2"), PadReference("C", "1"))),
         PhysicalNet("AC", (PadReference("A", "1"), PadReference("C", "2")))))
    assert placement_solution_is_legal(board, {p.reference: p for p in board.placements})
    return board


def hot_map(center=(7, 9), size=(40, 20)):
    ys, xs = np.mgrid[0:size[1], 0:size[0]]
    demand = 3.0 * np.exp(-((xs + .5 - center[0]) ** 2 + (ys + .5 - center[1]) ** 2) / 18.0)
    return cmap(demand)


def test_failed_net_part_moves_away_from_the_hot_tile():
    board = crowded_board()
    congestion = estimate_demand(board, MM)
    (hx, hy, _), *_ = congestion.hot_tiles(1)
    assert 4 <= hx <= 10 and 7 <= hy <= 11  # the crowded side is hot
    proposals = propose_moves(board, congestion, {"AD"}, PlacementPlannerOptions(), step_nm=MM)
    assert proposals and proposals[0].reference == "A" and proposals[0].rank[0] == 0
    centre = Point((2 * hx + 1) * MM // 2, (2 * hy + 1) * MM // 2)
    old = {p.reference: p for p in board.placements}["A"].position
    for proposal in (p for p in proposals if p.reference == "A" and p.delta_nm != (0, 0)):
        new = {p.reference: p for p in proposal.board.placements}["A"].position
        assert (hypot(new.x_nm - centre.x_nm, new.y_nm - centre.y_nm)
                > hypot(old.x_nm - centre.x_nm, old.y_nm - centre.y_nm))
        assert proposal.delta_nm == (new.x_nm - old.x_nm, new.y_nm - old.y_nm)
        assert all(value % MM == 0 for value in proposal.delta_nm)  # placement grid
    # Synthetic hotspot: the move is down its gradient.
    moved = [p for p in propose_moves(board, hot_map(), {"AD"}, PlacementPlannerOptions(), step_nm=MM,
                                      rotations=False) if p.reference == "A"]
    assert moved and all(p.delta_nm[0] < 0 and p.delta_nm[1] < 0 for p in moved)
    assert [p.rank[2] for p in moved] == sorted(p.rank[2] for p in moved)
    # Rotations stay at the current position; hot-tile parts follow failed-net parts.
    rotated = [p for p in proposals if p.delta_nm == (0, 0)]
    assert rotated and all(p.rotation_degrees != 0 for p in rotated)
    groups = [p.rank[0] for p in proposals]
    assert groups == sorted(groups) and 1 in groups


def test_fixed_parts_are_never_proposed_and_limit_bounds_the_result():
    board = crowded_board()
    board = replace(board, placement_rules=(ComponentPlacementRule("B", fixed_position=Point.mm(6, 10)),))
    options = PlacementPlannerOptions(fixed_references=frozenset({"C"}))
    proposals = propose_moves(board, hot_map(), {"AD", "BC", "AC"}, options, step_nm=MM)
    assert proposals and {p.reference for p in proposals} <= {"A", "D"}
    for proposal in proposals:
        poses = {p.reference: p for p in proposal.board.placements}
        assert poses["B"] == board.placements[1] and poses["C"] == board.placements[2]
    assert len(propose_moves(board, hot_map(), {"AD"}, options, step_nm=MM, limit=3)) == 3


def test_rigid_cluster_member_moves_with_its_cluster():
    board = crowded_board()
    fp = board.footprints["r"]
    digest = footprint_geometry_digest(fp)
    cluster = RigidPlacementCluster("unit", PlacementTarget("A"),
        (RigidPlacementMember("A", fp.name, digest, Point(0, 0)),
         RigidPlacementMember("E", fp.name, digest, Point.mm(0, -3))), "synthetic fixture", (0, 180))
    # B steps aside so the unit's 180 degree turn (E to the far side of A) is legal.
    placements = (board.placements[0], replace(board.placements[1], position=Point.mm(10, 13)),
                  *board.placements[2:], Placement("E", fp.name, Point.mm(6, 5)))
    board = replace(board, placements=placements, rigid_clusters=(cluster,))
    proposals = [p for p in propose_moves(board, hot_map(), {"AD"}, PlacementPlannerOptions(), step_nm=MM)
                 if p.reference == "A"]
    assert {p.delta_nm == (0, 0) for p in proposals} == {True, False}  # moves and a rotation
    for proposal in proposals:
        poses = {p.reference: p for p in proposal.board.placements}
        if proposal.delta_nm != (0, 0):
            assert poses["E"].position == Point(6 * MM + proposal.delta_nm[0], 5 * MM + proposal.delta_nm[1])
        else:  # a 180 degree unit rotation swings E about A
            assert poses["E"].position == Point.mm(6, 11) and poses["E"].rotation_degrees == 180


def test_nothing_is_proposed_beneath_accepted_copper_and_proposals_are_deterministic():
    board = crowded_board()
    track = TrackSegment("AD", Point.mm(6.5, 8), Point.mm(31.5, 12), nm_from_mm(".25"), CopperLayer.FRONT)
    assert propose_moves(replace(board, tracks=(track,)), hot_map(), {"AD"}, None, step_nm=MM) == ()
    congestion = estimate_demand(board, MM)
    first = propose_moves(board, congestion, {"AD"}, PlacementPlannerOptions(), step_nm=MM)
    second = propose_moves(board, estimate_demand(board, MM), {"AD"}, PlacementPlannerOptions(), step_nm=MM)
    assert first == second and len(first) > 1
    assert [p.rank for p in first] == sorted(p.rank for p in first)
    with pytest.raises(ValueError):
        propose_moves(board, congestion, {"AD"}, None, step_nm=0)


# ---------------------------------------------------------------- screener


def fake_route(unrouted, overflow):
    metrics = GlobalRoutingMetrics(routed_net_count=3 - unrouted, unrouted_net_count=unrouted,
                                   total_overflow=overflow, maximum_overflow=overflow, overfull_resource_count=0,
                                   maximum_utilization_ppm=0, total_length_nm=0, proposed_via_count=0, iterations=1)
    return GlobalRoutingResult(GlobalRoutingStatus.SUCCESS, (), (), metrics, "p", "r")


def test_screen_orders_by_global_score_and_keep_bounds_the_result(monkeypatch):
    board = crowded_board()
    proposals = propose_moves(board, hot_map(), {"AD", "BC"}, PlacementPlannerOptions(), step_nm=MM, rotations=False)
    assert len(proposals) >= 4
    routed = []

    def route(trial, options):
        index = next(i for i, p in enumerate(proposals) if p.board == trial)
        routed.append(index)
        return fake_route(0, 5 - index) if index < 5 else fake_route(1, 0)

    monkeypatch.setattr(cf, "route_global", route)
    events = []
    best = screen(board, proposals, GlobalRouterOptions(), keep=2,
                  on_progress=lambda phase, event, details: events.append((phase, event, dict(details))))
    assert len(best) == 2 and len(routed) == 4  # only 2 * keep pay for global routing
    scores = [route.metrics.total_overflow for _, route in best]
    assert scores == sorted(scores)
    assert best[0][1].metrics.total_overflow == min(5 - i if i < 5 else 99 for i in routed)
    assert [event[:2] for event in events] == [("congestion_screen", "started"), ("congestion_screen", "finished")]
    assert events[0][2]["proposal_count"] == len(proposals) and events[1][2]["kept_count"] == 2
    assert screen(board, proposals, None, keep=0) == ()
    assert len(screen(board, proposals[:1], None, keep=3)) == 1


def test_screen_runs_real_global_routing_and_prefers_fewer_overflows():
    board = crowded_board()
    options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2), maximum_iterations=2)
    proposals = propose_moves(board, hot_map(), {"AD"}, PlacementPlannerOptions(), step_nm=MM, limit=4)
    best = screen(board, proposals, options, keep=2)
    assert 1 <= len(best) <= 2 and all(isinstance(p, Proposal) for p, _ in best)
    from pcbir.routeflow import _route_score
    assert [_route_score(route) for _, route in best] == sorted(_route_score(route) for _, route in best)

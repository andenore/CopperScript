"""Escape-first order, transactional placement and no-area-search gate."""
from dataclasses import replace

import pytest

import pcbir.fanout as fanout
from pcbir import (BoardOutline, ComponentPlacementRule, CopperLayer, FootprintPad, GlobalRouterOptions, NetRoutingRule, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, RouteKind, Size, nm_from_mm, route_critical_nets, route_global)
from pcbir.drc import run_physical_drc
from pcbir.flow import run_routing_pipeline
from pcbir.routeflow import FeedbackStatus, PlacementRoutingResult


def fixture(critical=False):
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, 1), Size.mm(.3, .3))), Size.mm(2, 3))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    board = PhysicalBoard("access", BoardOutline.rectangle(16, 16),
        {package.name: package, one.name: one},
        (Placement("U", package.name, Point.mm(5, 7)),
         Placement("JA", one.name, Point.mm(12, 6)),
         Placement("JB", one.name, Point.mm(12, 8))),
        (PhysicalNet("A", (PadReference("U", "1"), PadReference("JA", "1"))),
         PhysicalNet("B", (PadReference("U", "2"), PadReference("JB", "1")))),
        net_routing_rules=(NetRoutingRule("A", RouteKind.CRITICAL, max_vias=0),) if critical else (),
        placement_rules=(ComponentPlacementRule("U", allowed_orientations=(0, 45)),))
    fanout_options = fanout.FanoutOptions(minimum_component_pads=2,
        maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False)
    guides_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2))
    return board, route_global(board, guides_options), fanout_options, guides_options


@pytest.mark.parametrize("defect", ["stale_placement", "stale_guides", "unowned_track", "protected_net", "short"])
def test_critical_router_rejects_stale_unowned_or_illegal_reservations(defect):
    board, guides, options, _ = fixture(critical=True)
    exits = fanout.route_fanout(board, options)
    if defect == "stale_placement":
        exits = replace(exits, board=replace(exits.board, placements=(
            replace(board.placements[0], position=Point.mm(6, 7)), *board.placements[1:])))
    elif defect == "stale_guides":
        guides = replace(guides, placement_fingerprint="stale")
    elif defect == "unowned_track":
        exits = replace(exits, created_tracks=())
    else:
        t = exits.created_tracks[0]
        t = replace(t, net="A") if defect == "protected_net" else replace(t, width_nm=nm_from_mm(4))
        tracks = (t, *exits.created_tracks[1:])
        exits = replace(exits, board=replace(exits.board, tracks=tracks), created_tracks=tracks)
    if defect == "short":
        result = route_critical_nets(board, guides, reserved_accesses=exits)
        assert result.status.value == "failed" and result.locked_tracks == exits.created_tracks
        assert any("DRC-" in diagnostic for item in result.nets for diagnostic in item.diagnostics)
    else:
        with pytest.raises(ValueError):
            route_critical_nets(board, guides, reserved_accesses=exits)


def test_pipeline_routes_area_only_after_successful_preflight(monkeypatch):
    board, guides, options, _ = fixture(critical=True)
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    result = run_routing_pipeline(board, fanout_options=options)
    assert result.package_access.ready and result.detailed.metrics.passes == 1
    assert result.critical.reserved_track_count > 0


def pattern_trap():
    """An ordinary dogbone blocks a length-constrained critical launch.

    Real radial domains and exact critical routing are used, not a mocked
    connected flag. There is a legal alternative ordinary via on the other side.
    """
    board, _, settings, global_options = fixture(critical=True)
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, -.4), Size.mm(.3, .3))), Size.mm(2, 3))
    board = replace(board, footprints={"two": package, "one": board.footprints["one"]},
        placements=(board.placements[0], replace(board.placements[1], position=Point.mm(3, 6)),
                    replace(board.placements[2], position=Point.mm(13, 6.6))),
        net_routing_rules=(NetRoutingRule("A", RouteKind.CRITICAL, max_vias=0,
            allowed_layers=(CopperLayer.FRONT,), max_length_nm=nm_from_mm(2)),))
    return board, route_global(board, global_options), settings


@pytest.mark.parametrize("budget", [0, 1])
def test_pipeline_obeys_pattern_budget_before_starting_area(monkeypatch, budget):
    board, guides, settings = pattern_trap()
    monkeypatch.setattr("pcbir.flow.optimize_placement_for_routing", lambda *_:
        PlacementRoutingResult(FeedbackStatus.PASS, board, guides, "fixture", (), 0, True))
    result = run_routing_pipeline(board, fanout_options=settings)
    # Without a pattern trial A stays over its length budget: a warning, not a block.
    assert result.package_access.ready
    assert result.package_access.over_budget_critical_nets == (frozenset() if budget else {"A"})
    assert len(result.package_access.pattern_trials) == budget


def test_pre_existing_footprint_finding_does_not_fail_package_reservations():
    from pcbir import PadKind, PadShape
    board, _, options, guides_options = fixture(critical=True)
    # A footprint whose own land sits on top of its locating hole, away from
    # the package: a hard finding that exists before any routing copper.
    land = FootprintPad("1", Point.mm(0, 0), Size.mm("0.6", "0.6"))
    hole = FootprintPad("", Point.mm("0.4", 0), Size.mm("0.4", "0.4"),
                        kind=PadKind.NON_PLATED_THROUGH_HOLE, shape=PadShape.CIRCLE,
                        drill=Size.mm("0.4", "0.4"), has_solder_paste=False)
    offender = PhysicalFootprint("land-near-hole", (land, hole), Size.mm(2, 2))
    board = replace(board, footprints={**board.footprints, offender.name: offender},
                    placements=(*board.placements, Placement("X1", offender.name, Point.mm(2, 14))))
    assert "DRC-HOLE-CLEARANCE" in {f.code for f in run_physical_drc(board).findings}
    guides = route_global(board, guides_options)
    exits = fanout.route_fanout(board, options)
    result = route_critical_nets(board, guides, reserved_accesses=exits)
    assert all(item.nets != ("<package-reservations>",) for item in result.nets)
    assert result.status.value != "failed" and all(item.connected for item in result.nets)
    # Final verification still reports the footprint's own finding.
    assert "DRC-HOLE-CLEARANCE" in {f.code for f in run_physical_drc(result.board).findings}



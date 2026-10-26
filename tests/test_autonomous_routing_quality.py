"""Generic integration reproductions; no board-local repairs or mocked search.

Synthetic lands are test geometry, not new reusable component definitions.
Native refill tests use the actual installed tool and skip when unavailable.
"""
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir import (BoardOutline, CopperKeepout, CopperLayer, CopperZone, FootprintPad, GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, PolygonRing, PolygonWithHoles, Size, Stackup, nm_from_mm)
from pcbir.decoupling import route_decouplers
from pcbir.drc import explicit_copper_connectivity, run_physical_drc
from pcbir.fanout import FanoutOptions
from pcbir.flow import run_routing_pipeline
from pcbir.physical import DecouplingLink
from pcbir.placement import PlacementPlannerOptions, transformed_pad_position
from pcbir.filled_feedback import FilledCopperFeedbackOptions
from pcbir.route_quality import proper_same_net_crossing
from pcbir.routeflow import PlacementRoutingFeedbackOptions


def terminal():
    return PhysicalFootprint("terminal", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),), Size.mm(1, 1))


def rectangle(x0, y0, x1, y1):
    return PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
        ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))))


def hard_findings(board):
    return tuple(f for f in run_physical_drc(board).findings
        if f.severity.value == "error" and f.code not in
        {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"})


def pipeline(board, *, movable=(), defer_zones=True, plane_options=None, legacy_gates=False):
    return run_routing_pipeline(board,
        placement_options=PlacementPlannerOptions(candidate_count=1,
            analytical_iterations=0, refinement_passes=0, escape_spacing_passes=0,
            fixed_references=frozenset(p.reference for p in board.placements
                                       if p.reference not in movable)),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
        fanout_options=FanoutOptions(minimum_component_pads=2,
            maximum_neighbor_distance_nm=nm_from_mm(3), two_leg_escapes=False),
        plane_stitch_options=plane_options)


@pytest.fixture
def native_cli():
    executable = Path(shutil.which("kicad-cli") or
                      "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not executable.is_file():
        pytest.skip("KiCad CLI not installed; fragmented-fill proof requires native refill")
    return executable


def adjacent_board(rotation, renamed):
    chip = PhysicalFootprint("paired-lands", (
        FootprintPad("1", Point.mm(1, "-0.3"), Size.mm("0.3", "0.3")),
        FootprintPad("2", Point.mm(1, "0.3"), Size.mm("0.3", "0.3"))), Size.mm(2, 2))
    land = terminal()
    refs = ("IC_72", "LOAD_4") if renamed else ("DEVICE", "LOAD")
    net = "SUPPLY_Z" if renamed else "POWER"
    def rotated(x, y):
        x, y = nm_from_mm(x) - nm_from_mm(10), nm_from_mm(y) - nm_from_mm(10)
        for _ in range(rotation // 90):
            x, y = y, -x  # Physical placements use clockwise board coordinates.
        return Point(x + nm_from_mm(10), y + nm_from_mm(10))
    return PhysicalBoard("adjacent-power-exits", BoardOutline.rectangle(20, 20),
        {chip.name: chip, land.name: land},
        (Placement(refs[0], chip.name, rotated(6, 10), rotation_degrees=rotation),
         Placement(refs[1], land.name, rotated(16, 10), rotation_degrees=rotation)),
        (PhysicalNet(net, (PadReference(refs[0], "1"), PadReference(refs[0], "2"),
                           PadReference(refs[1], "1"))),))


@pytest.mark.parametrize("rotation", (0, 90, 180, 270))
@pytest.mark.parametrize("renamed", (False, True))
def test_no_dogbone_site_uses_real_surface_fallback_without_weakening_via_rules(rotation, renamed):
    from pcbir.fanout import route_fanout
    board = adjacent_board(rotation, renamed)
    board = replace(board, copper_keepouts=(CopperKeepout("no-via-process-region",
        board.stackup.copper_layers, rectangle(0,0,20,20), block_tracks=False, block_zones=False),))
    options = FanoutOptions(minimum_component_pads=2, two_leg_escapes=False)
    old_policy = route_fanout(board, replace(options, surface_fallback=False))
    assert old_policy.pending_pads and not old_policy.created_vias
    exits = route_fanout(board, options)
    assert not exits.pending_pads and exits.surface_accesses and not exits.created_vias
    assert exits.board.copper_keepouts == board.copper_keepouts and not hard_findings(exits.board)
    result = pipeline(board)
    assert result.package_access.ready and result.detailed.nets[0].connected
    assert not result.board.vias and result.board.placements == board.placements
    assert not hard_findings(result.board)


@pytest.mark.parametrize("rotation", (0, 90, 180, 270))
@pytest.mark.parametrize("renamed", (False, True))
def test_standard_pipeline_cleans_redundant_adjacent_power_exits_without_x(rotation, renamed):
    board = adjacent_board(rotation, renamed)
    result = pipeline(board)
    assert result.package_access.ready and result.detailed.nets[0].connected
    launches = result.package_access.fanout
    assert launches.created_vias  # Test actual cleanup, not a no-fanout shortcut.
    assert not any(proper_same_net_crossing(a, b) for a in launches.created_tracks
                   for b in launches.created_tracks)
    # The first unobstructed exit follows the package's outward normal.
    pad = transformed_pad_position(board, board.placements[0], "1")
    first = next(t for t in launches.created_tracks if t.start == pad)
    assert (first.start.y_nm == first.end.y_nm if rotation % 180 == 0
            else first.start.x_nm == first.end.x_nm)
    assert not result.board.vias  # All terminals connect on the surface.
    assert not any(proper_same_net_crossing(a, b) for a in result.board.tracks
                   for b in result.board.tracks)
    assert explicit_copper_connectivity(result.board).net_connected(board.nets[0])
    assert result.board.placements == board.placements and not hard_findings(result.board)
    assert result == pipeline(board)


@pytest.mark.parametrize("renamed", (False, True))
def test_blocked_area_keeps_reserved_exits_instead_of_treating_them_as_redundant(renamed):
    board = adjacent_board(0, renamed)
    board = replace(board, copper_keepouts=(CopperKeepout("uncrossable-divider",
        board.stackup.copper_layers, rectangle("11.5", 0, "12.5", 20)),))
    result = pipeline(board, legacy_gates=True)
    assert not result.package_access.ready and result.detailed.metrics.passes == 0
    assert not result.detailed.nets[0].connected
    launches = result.package_access.fanout
    assert launches.created_tracks and launches.created_vias
    assert all(t in result.board.tracks for t in launches.created_tracks)
    assert all(v in result.board.vias for v in launches.created_vias)
    assert not explicit_copper_connectivity(result.board).net_connected(board.nets[0])
    assert result.board.placements == board.placements and not hard_findings(result.board)
    assert result.board.metadata["fabrication_ready"] == "false"


@pytest.mark.parametrize("renamed", (False, True))
def test_standard_pipeline_moves_semantic_decoupler_and_preserves_surface_feed(renamed):
    chip = PhysicalFootprint("chip", (
        FootprintPad("1", Point.mm(1, 0), Size.mm("0.4", "0.6")),
        FootprintPad("2", Point.mm(-1, 0), Size.mm("0.4", "0.6"))), Size.mm(2, 2))
    cap = PhysicalFootprint("bypass", (
        FootprintPad("1", Point.mm("-0.5", 0), Size.mm("0.5", "0.5")),
        FootprintPad("2", Point.mm("0.5", 0), Size.mm("0.5", "0.5"))), Size.mm("1.5", "0.8"))
    refs = ("IC_72", "PASSIVE_8") if renamed else ("DEVICE", "CAP")
    power, ground = ("SUPPLY_Z", "RETURN_X") if renamed else ("POWER", "GND")
    link = DecouplingLink(PadReference(refs[1], "1"), PadReference(refs[0], "1"),
                          PadReference(refs[1], "2"), power, ground)
    board = PhysicalBoard("semantic-bypass", BoardOutline.rectangle(20, 20),
        {chip.name: chip, cap.name: cap},
        (Placement(refs[0], chip.name, Point.mm(6, 10)),
         Placement(refs[1], cap.name, Point.mm(15, 15))),
        (PhysicalNet(power, (link.target, link.capacitor)),
         PhysicalNet(ground, (PadReference(refs[0], "2"), link.return_pad))),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                        CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        zones=(CopperZone("return-plane", ground, (CopperLayer.INTERNAL_1,),
                          rectangle(1, 1, 19, 19)),), decoupling_links=(link,))
    # Only semantic intent: no per-board max-distance/placement rescue selector.
    result = pipeline(board, movable=(refs[1],))
    assert result.package_access.ready
    assert result.board.placements[0] == board.placements[0]
    poses = {p.reference: p for p in result.board.placements}
    assert poses[refs[1]] != board.placements[1]
    start = transformed_pad_position(result.board, poses[refs[0]], "1")
    end = transformed_pad_position(result.board, poses[refs[1]], "1")
    assert abs(start.x_nm - end.x_nm) + abs(start.y_nm - end.y_nm) < nm_from_mm(3)
    assert next(n for n in result.detailed.nets if n.net == power).connected
    bypass = route_decouplers(result.board)
    assert not bypass.pending and not bypass.created_tracks
    assert {link.target, link.capacitor} <= bypass.connected_pads
    prefix = tuple(t for t in result.package_access.fanout.created_tracks if t.net == power)
    assert prefix and all(t.layer is CopperLayer.FRONT and t in result.board.tracks for t in prefix)
    assert result.board.vias and all(v.net == ground for v in result.board.vias)
    assert not hard_findings(result.board)
    assert not result.board.zone_fills and result.board.metadata["fabrication_ready"] == "false"

"""Board material, router acceptance and independent KiCad/Excellon probes."""
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (
    BoardCutout, BoardOutline, BoardSide, CopperLayer, DetailedRouterOptions,
    FootprintPad, GlobalRouterOptions,
    KiCadPcbBackend, MechanicalHole, PadKind, PadReference, PadShape,
    PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, PlacementKeepout,
    PlacementRegion, Point, Size, TrackSegment, Via, nm_from_mm,
    parse_xnc, reconcile_drills, route_detailed, route_global, run_physical_drc,
    write_kicad_project,
)
from pcbir.drc import DrcCompleteness, PhysicalDrcPolicy, physical_board_digest
from pcbir.geometry import RoundedConvexShape
from pcbir.mechanical import hole_shape, point_in_material, shape_in_board
from pcbir.mechanical_example import build_mechanical_example
from pcbir.physical import PolygonRing, PolygonWithHoles, ZoneFillResult
from pcbir.placement import placement_solution_is_legal
from pcbir.process_drc import (
    FabricationAssemblyProfile, ProcessCapability, ProcessGateStatus, run_process_drc,
)
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.surface_path import _track_inside_board, via_inside_board


def ring(*points):
    return tuple(Point.mm(x, y) for x, y in points)


def cutout(name="window", points=((12, 7), (14, 7), (14, 11), (12, 11))):
    return BoardCutout(name, ring(*points))


def board_fixture():
    return build_mechanical_example()


@pytest.mark.parametrize("points", [
    ((0, 0), (5, 0), (5, 0), (0, 5)),
    ((0, 0), (1, 0), (2, 0)),
    ((0, 0), (5, 5), (0, 4), (4, 0)),
    ((0, 0), (5, 0), (3, 0), (3, 5), (0, 5)),
])
def test_invalid_boundary_is_rejected(points):
    with pytest.raises(ValueError):
        BoardOutline(ring(*points))


def test_closed_ring_normalizes_and_winding_is_not_semantic():
    vertices = ring((0, 0), (10, 0), (10, 10), (0, 10))
    assert BoardOutline((*vertices, vertices[0])).vertices == vertices
    reverse = BoardOutline(tuple(reversed(vertices)))
    assert reverse.vertices == tuple(reversed(vertices))


@pytest.mark.parametrize("cutouts", [
    (cutout(points=((29, 2), (31, 2), (31, 4), (29, 4))),),
    (cutout(points=((0, 2), (2, 2), (2, 4), (0, 4))),),
    (cutout(), cutout()),
    (cutout(), cutout("nested", ((12.5, 8), (13, 8), (13, 9), (12.5, 9)))),
    (cutout(), cutout("touch", ((14, 8), (16, 8), (16, 9), (14, 9)))),
])
def test_invalid_cutouts_are_rejected(cutouts):
    with pytest.raises(ValueError):
        BoardOutline(BoardOutline.rectangle(30, 24).vertices, cutouts)


def test_regions_do_not_silently_ignore_cutouts():
    outline = board_fixture().outline
    for constructor in (PlacementRegion, PlacementKeepout):
        with pytest.raises(ValueError, match="do not support cutouts"):
            constructor("region", outline)


@pytest.mark.parametrize("holes", [
    (MechanicalHole("outside", Point.mm(29, 4), nm_from_mm(3)),),
    (MechanicalHole("window", Point.mm(13, 9), nm_from_mm(1)),),
    (MechanicalHole("a", Point.mm(4, 4), nm_from_mm(3)),
     MechanicalHole("b", Point.mm(5, 4), nm_from_mm(3))),
    (MechanicalHole("a", Point.mm(4, 4), nm_from_mm(1)),
     MechanicalHole("a", Point.mm(8, 4), nm_from_mm(1))),
])
def test_invalid_holes_are_rejected(holes):
    with pytest.raises(ValueError):
        replace(board_fixture(), mechanical_holes=holes)


def test_odd_hole_radius_rounds_outward_and_tangency_requires_clearance():
    assert hole_shape(MechanicalHole("odd", Point(0, 0), 3)).radius_nm == 2
    board = board_fixture()
    hole = board.mechanical_holes[0]
    shape = RoundedConvexShape((Point(hole.position.x_nm + hole.diameter_nm // 2 + 100,
                                    hole.position.y_nm),))
    assert shape_in_board(board, shape, hole_clearance_nm=100)
    assert not shape_in_board(board, shape, hole_clearance_nm=101)


@pytest.mark.parametrize("diameter, radius", [(0, 0), (True, 0), (2.5, 0), (3, -1), (3, 1)])
def test_hole_dimensions_are_integer_positive_and_head_encloses_hole(diameter, radius):
    with pytest.raises(ValueError):
        MechanicalHole("invalid", Point.mm(5, 5), diameter, radius)


def test_endpoints_are_insufficient_for_material_containment():
    board = board_fixture()
    for start, end in ((Point.mm(5, 10), Point.mm(25, 10)),  # window
                       (Point.mm(18, 20), Point.mm(25, 14))):  # concave notch
        assert point_in_material(board, start) and point_in_material(board, end)
        assert not shape_in_board(board, RoundedConvexShape((start, end)))
    assert not shape_in_board(board, RoundedConvexShape(ring((10, 6), (16, 6), (16, 12), (10, 12))))
    assert not point_in_material(board, Point.mm(4, 4))


def test_placement_blocks_voids_and_screw_head_space_on_both_sides():
    board = board_fixture()
    for point in (Point.mm(13, 9), Point.mm(4, 4), Point.mm(22, 20)):
        pose = replace(board.placements[0], position=point)
        assert not placement_solution_is_legal(board, {"J1": pose, "J2": board.placements[1]})
    head = replace(board.mechanical_holes[0], head_clearance_radius_nm=nm_from_mm(4))
    headed = replace(board, mechanical_holes=(head, board.mechanical_holes[1]))
    for side in (BoardSide.FRONT, BoardSide.BACK):
        pose = replace(board.placements[0], position=Point.mm(7, 4), side=side)
        poses = {"J1": pose, "J2": board.placements[1]}
        assert placement_solution_is_legal(board, poses)
        assert not placement_solution_is_legal(headed, poses)


def test_router_and_soft_ripup_cannot_cross_mechanical_obstacles():
    board = board_fixture()
    index = RoutingClearanceIndex(board)
    track = TrackSegment("SIGNAL", Point.mm(5, 10), Point.mm(25, 10),
                         board.rules.default_track_width_nm, CopperLayer.FRONT)
    assert not index.can_track(track.net, track.start, track.end, track.width_nm, track.layer)
    assert index.blocking_track_nets(track)[1]
    assert not _track_inside_board(board, track.start, track.end, track.width_nm)
    for point in (Point.mm(13, 9), Point.mm(4, 4)):
        via = Via("SIGNAL", point, nm_from_mm("0.6"), nm_from_mm("0.3"))
        assert not index.can_via(via.net, point, via.size_nm, via.from_layer, via.to_layer)
        assert index.blocking_via_nets(via)[1]
        assert not via_inside_board(board, point, via.size_nm)


def test_drc_catches_cutout_crossings_and_pad_to_mechanical_hole():
    board = board_fixture()
    track = TrackSegment("SIGNAL", Point.mm(5, 10), Point.mm(25, 10),
                         nm_from_mm("0.25"), CopperLayer.FRONT)
    report = run_physical_drc(replace(board, tracks=(track,)),
                              policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert any(f.code == "DRC-BOARD-EDGE" for f in report.findings)
    bad = replace(board, placements=(replace(board.placements[0], position=Point.mm(4, 4)),
                                    board.placements[1]))
    assert any(f.code == "DRC-HOLE-CLEARANCE" for f in run_physical_drc(bad).findings)


def test_drill_spacing_between_npths_and_to_plated_objects_is_independent_of_net():
    board = board_fixture()
    hole = board.mechanical_holes[0]
    near = MechanicalHole("near", Point.mm(7.3, 4), hole.diameter_nm)
    close = replace(board, mechanical_holes=(*board.mechanical_holes, near))
    assert any(f.code == "DRC-DRILL-SPACING" for f in run_physical_drc(close).findings)
    via = Via("SIGNAL", Point.mm(5.85, 4), nm_from_mm("0.8"), nm_from_mm("0.4"))
    assert any(f.code == "DRC-DRILL-SPACING" for f in run_physical_drc(
        replace(board, vias=(via,))).findings)
    fp = board.footprints["probe"]
    plated = replace(fp, pads=(replace(fp.pads[0], kind=PadKind.THROUGH_HOLE,
                                       drill=Size.mm("0.3", "0.3")),))
    close_pad = replace(board, footprints={plated.name: plated}, placements=(
        replace(board.placements[0], position=Point.mm(5.85, 4)), board.placements[1]))
    assert any(f.code == "DRC-DRILL-SPACING" for f in run_physical_drc(close_pad).findings)


def test_identity_includes_holes_cutouts_and_head_clearance():
    board = board_fixture()
    variants = (replace(board, mechanical_holes=()),
                replace(board, outline=BoardOutline(board.outline.vertices)),
                replace(board, mechanical_holes=(replace(board.mechanical_holes[0],
                        head_clearance_radius_nm=nm_from_mm(3)), board.mechanical_holes[1])))
    global_options = GlobalRouterOptions(tile_size_nm=nm_from_mm(2))
    original = route_global(board, global_options).placement_fingerprint
    for variant in variants:
        assert physical_board_digest(variant) != physical_board_digest(board)
        assert route_global(variant, global_options).placement_fingerprint != original


def test_zone_material_coverage_fails_closed_until_qualified():
    board = board_fixture()
    fill = ZoneFillResult("ground", CopperLayer.FRONT, "digest", "test", "1",
                          (PolygonWithHoles(PolygonRing(board.outline.vertices)),))
    # Use an existing signal zone identity: PhysicalBoard validates fill ownership.
    from pcbir.physical import CopperZone
    zone = CopperZone("ground", "SIGNAL", (CopperLayer.FRONT,),
                      PolygonWithHoles(PolygonRing(board.outline.vertices)))
    report = run_physical_drc(replace(board, zones=(zone,), zone_fills=(fill,)))
    assert report.completeness is DrcCompleteness.INCOMPLETE
    assert any(f.code == "DRC-MECHANICAL-FILL-UNSUPPORTED" for f in report.findings)


def test_npth_requires_separate_provenance_bound_process_limit():
    cap = ProcessCapability(nm_from_mm("0.2"), "test-process", "1")
    profile = FabricationAssemblyProfile("probe", cap, cap, cap,
                                         ProcessCapability(1, "test-process", "1"))
    board = board_fixture()
    assert run_process_drc(board, profile).fabrication is ProcessGateStatus.INCOMPLETE
    qualified = replace(profile, minimum_non_plated_drill_nm=cap)
    assert run_process_drc(board, qualified).fabrication is ProcessGateStatus.PASS
    bad_limit = replace(profile, minimum_non_plated_drill_nm=replace(cap, value=nm_from_mm(4)))
    assert run_process_drc(board, bad_limit).fabrication is ProcessGateStatus.FAIL


@pytest.fixture(scope="module")
def routed_probe():
    board = board_fixture()
    global_result = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, global_result, DetailedRouterOptions(maximum_passes=2))
    assert result.status.value == "success"
    assert result.board.tracks
    for track in result.board.tracks:
        assert shape_in_board(result.board, RoundedConvexShape((track.start, track.end),
                              (track.width_nm + 1) // 2),
                              result.board.rules.minimum_clearance_nm,
                              result.board.rules.minimum_hole_clearance_nm)
    assert not run_physical_drc(result.board).findings
    return result.board


def test_mechanical_export_is_deterministic_and_does_not_mutate_ir(routed_probe):
    backend = KiCadPcbBackend()
    first = backend.generate(routed_probe)
    assert first == backend.generate(routed_probe)
    pcb = first.artifacts[0].content
    assert pcb.count('(layer "Edge.Cuts")') == 10
    assert pcb.count('np_thru_hole circle') == 2
    assert pcb.count("exclude_from_bom") == 2
    assert len(routed_probe.placements) == 2 and len(routed_probe.mechanical_holes) == 2


def test_screw_head_export_warns_about_manual_kicad_changes(routed_probe):
    holes = (replace(routed_probe.mechanical_holes[0], head_clearance_radius_nm=nm_from_mm(3)),
             routed_probe.mechanical_holes[1])
    manifest = KiCadPcbBackend().generate(replace(routed_probe, mechanical_holes=holes))
    assert any("Screw-head clearance" in w for w in manifest.warnings)


def test_manufacturing_cannot_bypass_pending_outline_and_tooling_qualification(routed_probe, tmp_path):
    from pcbir.manufacturing import build_manufacturing_release
    def should_not_run(*args):
        pytest.fail("unqualified mechanical geometry must fail before external tools run")
    with pytest.raises(ValueError, match="independent outline/tooling qualification"):
        build_manufacturing_release(routed_probe, run_physical_drc(routed_probe).token,
                                    tmp_path / "release", kicad_cli=Path("kicad-cli"),
                                    runner=should_not_run)
    assert not (tmp_path / "release").exists()


def test_native_kicad_outline_clearance_and_excellon_roundtrip(routed_probe, tmp_path):
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("requires KiCad CLI for independent mechanical qualification")
    pcb = tmp_path / "mechanical.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(routed_probe), pcb)
    report_path = tmp_path / "drc.json"
    subprocess.run([cli, "pcb", "drc", "--format", "json", "-o", str(report_path), str(pcb)],
                   check=True, capture_output=True, timeout=60)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert not report["violations"] and not report["unconnected_items"]
    drills = tmp_path / "drills"
    drills.mkdir()
    subprocess.run([cli, "pcb", "export", "drill", "--output", str(drills),
                    "--format", "excellon", "--excellon-units", "mm",
                    "--excellon-separate-th", str(pcb)],
                   check=True, capture_output=True, timeout=60)
    programs = tuple(parse_xnc(p, plated="-NPTH" not in p.stem) for p in drills.glob("*.drl"))
    assert programs and reconcile_drills(routed_probe, programs).passed
    assert not reconcile_drills(routed_probe, ()).passed
    assert not reconcile_drills(routed_probe, tuple(replace(p, plated=True) for p in programs)).passed


def test_multiple_unnumbered_footprint_holes_reconcile_actual_land_positions():
    from pcbir.cam_qualification import DrillHit, NormalizedDrillProgram
    pad = FootprintPad("", Point.mm(-2, 0), Size.mm(1, 1),
                       kind=PadKind.NON_PLATED_THROUGH_HOLE, shape=PadShape.CIRCLE,
                       drill=Size.mm(1, 1), has_solder_paste=False)
    fp = PhysicalFootprint("two-holes", (pad, replace(pad, position=Point.mm(2, 0))), Size.mm(5, 2))
    board = PhysicalBoard("holes", BoardOutline.rectangle(20, 20), {fp.name: fp},
                          (Placement("MH", fp.name, Point.mm(10, 10), rotation_degrees=90),), ())
    program = NormalizedDrillProgram(tuple(DrillHit("T1", nm_from_mm(1), nm_from_mm(10),
                    -nm_from_mm(y)) for y in (8, 12)), "mm", False)
    assert reconcile_drills(board, (program,)).passed

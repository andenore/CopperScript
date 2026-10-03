from dataclasses import replace
from decimal import Decimal
import json
from math import hypot
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (
    BoardCutout, BoardOutline, BoardSide, CopperLayer, KiCadPcbBackend, MechanicalHole,
    Point, TrackSegment, check, compile_file, nm_from_mm, write_kicad_project,
)
from pcbir.drc import PhysicalDrcPolicy, physical_board_digest, placed_pad_shape, run_physical_drc
from pcbir.geometry import RoundedConvexShape, segment_distance_squared, shapes_clear
from pcbir.mechanical import point_in_material, ring_edges, shape_in_board
from pcbir.mechanical_example import build_mechanical_example
from pcbir.placement import placement_solution_is_legal, transformed_local_point
from pcbir.round_led_example import SOURCE, main, make_example, placement_svg, stitch_ground_pours
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.surface_path import _track_inside_board, via_inside_board


def test_circle_query_ring_is_certified_inscribed_not_authoritative_export_geometry():
    outline = BoardOutline.circle(50, center=Point.mm(25, 25))
    circle = outline.circular_boundary
    assert circle.radius_nm == nm_from_mm(25)
    offsets = tuple(Point(p.x_nm-circle.center.x_nm, p.y_nm-circle.center.y_nm)
                    for p in outline.vertices)
    assert len(offsets) <= 4096
    assert all(p.x_nm ** 2 + p.y_nm ** 2 <= circle.radius_nm ** 2 for p in offsets)
    assert all(segment_distance_squared(Point(0, 0), Point(0, 0), a, b)
               >= (circle.radius_nm-circle.maximum_chord_error_nm) ** 2
               for a, b in ring_edges(offsets))
    with pytest.raises(ValueError, match="authoritative circle"):
        replace(outline, circular_boundary=replace(circle, radius_nm=nm_from_mm(26)))


@pytest.mark.parametrize("diameter", [0, -1, "0.000003"])
def test_invalid_circle_dimensions_reject(diameter):
    with pytest.raises(ValueError):
        BoardOutline.circle(diameter)


def test_circle_material_uses_exact_disk_even_near_chord_midpoint():
    board = replace(build_mechanical_example(), outline=BoardOutline.circle(50), mechanical_holes=())
    # Tangent disk and capsule on the actual circle; radial margin must not
    # inherit the old inscribed polygon, bounding rectangle, or mesh tolerance.
    inside = Point.mm(48.8, 25)
    outside = Point(inside.x_nm + 1, inside.y_nm)
    assert shape_in_board(board, RoundedConvexShape((inside,), nm_from_mm(1)), nm_from_mm("0.2"))
    assert not shape_in_board(board, RoundedConvexShape((outside,), nm_from_mm(1)), nm_from_mm("0.2"))
    assert point_in_material(board, Point.mm(50, 25))
    assert not point_in_material(board, Point.mm(49, 49))
    assert not via_inside_board(board, Point.mm(49.9, 25), nm_from_mm("0.8"))
    assert not _track_inside_board(board, Point.mm(49, 49), Point.mm(48, 48), nm_from_mm("0.25"))
    index = RoutingClearanceIndex(board)
    assert not index.can_track("SIGNAL", Point.mm(45, 45), Point.mm(46, 46), nm_from_mm("0.25"), CopperLayer.FRONT)
    assert physical_board_digest(board) != physical_board_digest(replace(
        board, outline=BoardOutline(board.outline.vertices)))


def test_circle_board_edge_drc_does_not_accept_bounding_box_corner():
    board = replace(build_mechanical_example(), outline=BoardOutline.circle(50), mechanical_holes=())
    track = TrackSegment("SIGNAL", Point.mm(45, 45), Point.mm(46, 46), nm_from_mm("0.25"), CopperLayer.FRONT)
    report = run_physical_drc(replace(board, tracks=(track,)),
                             policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert any(f.code == "DRC-BOARD-EDGE" for f in report.findings)


def test_circle_keeps_cutout_and_round_hole_material_exclusions():
    window = BoardCutout("window", tuple(Point.mm(x, y) for x, y in
                                        ((15, 15), (18, 15), (18, 18), (15, 18))))
    board = replace(build_mechanical_example(), outline=BoardOutline.circle(50, cutouts=(window,)),
                    mechanical_holes=(MechanicalHole("mount", Point.mm(35, 25), nm_from_mm(3)),))
    assert not point_in_material(board, Point.mm(16, 16))
    assert not point_in_material(board, Point.mm(35, 25))
    assert point_in_material(board, Point.mm(25, 25))
    assert not shape_in_board(board, RoundedConvexShape((Point.mm(12, 16), Point.mm(20, 16)),
                                                       nm_from_mm("0.1")))


def test_circle_manufacturing_gate_stays_closed(tmp_path):
    from pcbir.manufacturing import build_manufacturing_release
    board = replace(build_mechanical_example(), outline=BoardOutline.circle(50), mechanical_holes=())
    def should_not_run(*args):
        pytest.fail("unqualified circles must fail before external manufacturing tools run")
    with pytest.raises(ValueError, match="independent outline/tooling qualification"):
        build_manufacturing_release(board, run_physical_drc(board).token,
                                    tmp_path / "release", kicad_cli=Path("kicad-cli"),
                                    runner=should_not_run)
    assert not (tmp_path / "release").exists()


def test_round_example_has_twelve_unique_active_low_channels_and_primary_supply():
    board = compile_file(SOURCE, locked=True, offline=True)
    assert check(board) == []
    assert len(board.components) == 34
    assert next(c for c in board.components if c.ref == "U1").part == "vertical.NRF52832_QFAA"
    assert {s.net for s in board.supplies} == {"VBAT", "GND"}
    assert all(s.externally_driven for s in board.supplies)
    pins = set()
    for i in range(1, 13):
        cathode = next(n for n in board.nets if n.name == f"LED{i}_K")
        assert (f"LED{i}", "K") in {(p.component, p.pin) for p in cathode.endpoints}
        pin = next(p.pin for p in cathode.endpoints if p.component == "U1")
        assert pin not in pins and pin not in {"SWDIO", "SWDCLK", "P0_21_NRESET", "P0_09_NFC1", "P0_10_NFC2"}
        assert pin.startswith("P0_")
        pins.add(pin)
        anode = next(n for n in board.nets if n.name == f"LED{i}_A")
        assert {(p.component, p.pin) for p in anode.endpoints} == {(f"LED{i}", "A"), (f"R_LED{i}", "2")}
        resistor = next(c for c in board.components if c.ref == f"R_LED{i}")
        assert resistor.value.base_value == 10000


def test_round_nrf_ldo_support_and_debug_leave_radio_and_dcdc_unused():
    board = compile_file(SOURCE, locked=True, offline=True)
    def endpoints(name):
        return {(p.component, p.pin) for n in board.nets if n.name == name for p in n.endpoints}
    assert endpoints("VBAT") >= {("U1", "VDD_1"), ("U1", "VDD_2"), ("U1", "VDD_3")}
    assert endpoints("GND") >= {("U1", "VSS_1"), ("U1", "VSS_2"), ("U1", "VSS_EP")}
    for name, value in (("DEC1", "1E-7"), ("DEC3", "1E-10"), ("DEC4", "1E-6")):
        assert endpoints(name) == {("U1", name), (f"C_{name}", "1")}
        assert (f"C_{name}", "2") in endpoints("GND")
        assert next(c for c in board.components if c.ref == f"C_{name}").value.base_value == Decimal(value)
    assert endpoints("SWDIO") == {("U1", "SWDIO"), ("J_SWD", "SWDIO")}
    assert endpoints("SWDCLK") == {("U1", "SWDCLK"), ("J_SWD", "SWDCLK")}
    assert endpoints("RESET") == {("U1", "P0_21_NRESET"), ("J_SWD", "RESET"), ("R_RESET", "2")}
    used = set().union(*(endpoints(n.name) for n in board.nets))
    assert not {("U1", p) for p in ("ANT", "XC1", "XC2", "DEC2", "DCC", "NC_44")} & used


@pytest.fixture(scope="module")
def placed_board():
    roots = (Path("C:/Program Files/KiCad/10.0/share/kicad/footprints"),)
    if not roots[0].is_dir():
        pytest.skip("optional installed KiCad footprint library")
    return make_example(roots, offline=True)


def test_round_placement_and_battery_side_are_fixed_not_baked_into_electrical_ir(placed_board):
    board = placed_board
    assert placement_solution_is_legal(board, {p.reference: p for p in board.placements})
    for pose in board.placements:
        if pose.reference.startswith("LED"):
            assert abs(hypot(pose.position.x_nm-nm_from_mm(25), pose.position.y_nm-nm_from_mm(25))
                       - nm_from_mm(22)) < 2
        if pose.reference == "BT1":
            assert pose.side is BoardSide.BACK and pose.position == Point.mm(29, 25)
        if pose.reference == "U1":
            assert pose.side is BoardSide.FRONT and pose.position == Point.mm("13.5", 25)
    assert len(board.placement_rules) == 34
    assert board.stackup.copper_layers == (CopperLayer.FRONT, CopperLayer.BACK)
    assert board.rules.minimum_track_width_nm == nm_from_mm("0.15")
    assert board.rules.minimum_clearance_nm == nm_from_mm("0.15")
    assert not board.via_in_pad_rules
    assert not board.tracks and not board.vias and not board.zone_fills
    assert {l for z in board.zones for l in z.layers} == {CopperLayer.BACK}
    assert "inspection only" in placement_svg(board)
    assert "nRF52832" in placement_svg(board)
    assert "rear CR2032 holder" in placement_svg(board)


def test_offset_mcu_pad_projection_is_clear_of_every_rear_battery_contact(placed_board):
    poses = {p.reference: p for p in placed_board.placements}
    def shapes(ref):
        pose = poses[ref]
        return tuple(placed_pad_shape(transformed_local_point(pose, pad.position), pad, pose)
                     for pad in placed_board.footprints[pose.footprint].pads)
    assert all(shapes_clear(mcu, contact, placed_board.rules.minimum_clearance_nm)
               for mcu in shapes("U1") for contact in shapes("BT1"))


def test_round_native_circle_export_and_placed_drc(placed_board, tmp_path):
    manifest = KiCadPcbBackend().generate(placed_board)
    assert manifest == KiCadPcbBackend().generate(placed_board)
    assert manifest.artifacts[0].content.count('(layer "Edge.Cuts")') == 1
    assert '(gr_circle\n    (center 25 25)\n    (end 50 25)' in manifest.artifacts[0].content
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not Path(cli).is_file():
        pytest.skip("optional native KiCad verification")
    pcb = tmp_path / "round.kicad_pcb"
    write_kicad_project(manifest, pcb)
    report_path = tmp_path / "drc.json"
    subprocess.run([cli, "pcb", "drc", "--refill-zones", "--save-board", "--format", "json",
                    "--severity-all", "-o", str(report_path), str(pcb)],
                   check=True, capture_output=True, timeout=60)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert not report["violations"]
    assert report["unconnected_items"]  # Placed != routed; no false signoff.


def test_round_example_cli_profiling_and_outputs(placed_board, tmp_path):
    assert main(["--footprint-root", "C:/Program Files/KiCad/10.0/share/kicad/footprints",
                 "--offline", "--output-dir", str(tmp_path)]) == 0
    summary = json.loads((tmp_path / "summary.json").read_text())
    assert summary["routing"] == "placed" and not summary["fabrication_ready"]
    for name in ("placement-front.svg", "placement-back.svg", "round-led-ring.kicad_pcb",
                 "profile.pstats", "profile.txt", "physical-drc.json"):
        assert (tmp_path / name).is_file()


def test_outer_plane_escape_is_opt_in_and_reserves_legal_ground_before_signals(placed_board):
    from pcbir.plane import stitch_zone_pads
    assert not stitch_zone_pads(placed_board).board.vias
    board, count = stitch_ground_pours(placed_board)
    assert count > 0 and board.metadata["plane_stitching"] == "pad-escapes-only"
    assert board.tracks and all(t.net == "GND" for t in board.tracks)
    assert all(v.net == "GND" and v.finish == "standard" for v in board.vias)
    report = run_physical_drc(board)
    assert not {f.code for f in report.findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-VIA-PAD-OVERLAP", "DRC-HOLE-CLEARANCE", "DRC-BOARD-EDGE"}
    repeat, repeated_count = stitch_ground_pours(board)
    assert repeated_count == 0 and repeat.vias == board.vias


def test_interrupted_rerun_does_not_reuse_an_old_success(tmp_path, monkeypatch):
    import pcbir.round_led_example as example
    (tmp_path / "summary.json").write_text(json.dumps({"native_routing_complete": True}))
    def fail(*args, **kwargs):
        raise ValueError("test interrupted generation")
    monkeypatch.setattr(example, "make_example", fail)
    with pytest.raises(ValueError, match="interrupted generation"):
        main(["--footprint-root", str(tmp_path), "--output-dir", str(tmp_path)])
    summary = json.loads((tmp_path / "summary.json").read_text())
    assert summary["routing"] == "running"
    assert not summary["native_routing_complete"] and not summary["fabrication_ready"]

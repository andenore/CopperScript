"""Required via-in-pad arrays are fixed, fully checked owner copper."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
import json
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

from pcbir import (
    BoardOutline, CopperKeepout, CopperLayer, CopperZone, DesignRules, FootprintPad,
    PadReference, PadShape, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, PolygonRing, PolygonWithHoles, PrototypePhysicalOptions, Size, Stackup,
    TrackSegment, Via, compile_source, nm_from_mm, prototype_physicalize, run_physical_drc,
)
from pcbir.hard_macros import macro_source, materialize_hard_macros
from pcbir.pad_via_arrays import (
    materialize_via_in_pad_arrays, via_in_pad_array_report, via_in_pad_array_vias,
)
from pcbir.physical import PadViaInPadRule
from pcbir.plane import stitch_zone_pads

ROOT = Path(__file__).resolve().parents[1]
EP = PadReference("U1", "17")
SIX = Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
               CopperLayer.INTERNAL_3, CopperLayer.INTERNAL_4, CopperLayer.BACK))


def _array_board(ep_mm: str = "3.1", **rule) -> PhysicalBoard:
    """A QFN-16 with an exposed GND land between two resistors."""
    pads = []
    reach = Decimal(ep_mm) / 2 + Decimal("0.55")  # 0.15 mm from the exposed land
    for side, (x, y) in enumerate(((-1, 0), (0, 1), (1, 0), (0, -1))):
        for index in range(4):
            offset = (index - Decimal("1.5")) * Decimal("0.65")
            along = Point.mm(x * reach, offset) if x else Point.mm(offset, y * reach)
            size = Size.mm("0.8", "0.3") if x else Size.mm("0.3", "0.8")
            pads.append(FootprintPad(str(side * 4 + index + 1), along, size))
    pads.append(FootprintPad("17", Point(0, 0), Size.mm(ep_mm, ep_mm), shape=PadShape.RECTANGLE))
    qfn = PhysicalFootprint("test/qfn16-ep", tuple(pads), Size.mm(2 * reach + 2, 2 * reach + 2))
    resistor = PhysicalFootprint("test/0603", (
        FootprintPad("1", Point.mm("-0.8", 0), Size.mm("0.8", "0.9")),
        FootprintPad("2", Point.mm("0.8", 0), Size.mm("0.8", "0.9"))), Size.mm("3.2", "1.8"))
    zone = CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing((
        Point.mm(1, 1), Point.mm(29, 1), Point.mm(29, 19), Point.mm(1, 19)))))
    board = PhysicalBoard(
        "ViaArray", BoardOutline.rectangle(30, 20), {qfn.name: qfn, resistor.name: resistor},
        (Placement("U1", qfn.name, Point.mm(15, 10)),
         Placement("R1", resistor.name, Point.mm(7, 10)),
         Placement("R2", resistor.name, Point.mm(23, 10))),
        (PhysicalNet("GND", (EP, PadReference("U1", "4"), PadReference("R1", "1"))),
         PhysicalNet("A", (PadReference("U1", "1"), PadReference("R1", "2"))),
         PhysicalNet("B", (PadReference("U1", "9"), PadReference("R2", "1")))),
        stackup=SIX, zones=(zone,), metadata={"fabrication_profile": "jlcpcb-six-layer"},
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.09"),
                          minimum_track_width_nm=nm_from_mm("0.09"),
                          default_track_width_nm=nm_from_mm("0.2")),
    )
    return replace(board, via_in_pad_rules=(PadViaInPadRule(EP, **rule),)) if rule else board


def _hard(board: PhysicalBoard) -> list[str]:
    return [f"{item.code}: {item.message}" for item in run_physical_drc(board).findings
            if item.severity.value == "error"
            and item.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]


def _source(extra: str) -> str:
    return '''board Test { use library "tiny";
        component BT1: CAPACITOR;
        net GND { BT1.2; }
        constraint copper_zone(GND) { layers = "In1.Cu"; }
        constraint via_in_pad(BT1.2) { process = "filled-capped"; ''' + extra + ''' }
    }'''


def _lower(extra: str) -> PhysicalBoard:
    return prototype_physicalize(compile_source(_source(extra)), PrototypePhysicalOptions(
        copper_layers=6, fabrication_profile="jlcpcb-six-layer"))


def test_source_lowers_array_parameters_onto_the_permission():
    board = _lower("rows = 2; columns = 2; pitch = 0.5mm;")
    assert board.via_in_pad_rules == (PadViaInPadRule(
        PadReference("BT1", "2"), "filled-capped", 2, 2, nm_from_mm("0.5")),)
    assert _lower("rows = 1; columns = 1;").via_in_pad_rules[0].pitch_nm is None


@pytest.mark.parametrize("extra,message", [
    ("rows = 2;", "together"),
    ("columns = 2;", "together"),
    ("rows = 0; columns = 1;", "positive integers"),
    ("rows = 2; columns = 1.5;", "positive integers"),
    ("pitch = 0.5mm;", "requires rows and columns"),
    ("rows = 1; columns = 1; spacing = 1mm;", "unknown via_in_pad parameter"),
    # The proxy land is 1 x 1 mm: these fail its fit or the 0.45 mm drill spacing.
    ("rows = 2; columns = 2; pitch = 0.8mm;", "BT1.2 site row 1 column 1 does not fit"),
    ("rows = 3; columns = 3;", "below the 0.45 mm drill spacing"),
    ("rows = 2; columns = 2; pitch = 0.4mm;", "below the 0.45 mm drill spacing"),
])
def test_invalid_array_parameters_fail_closed(extra, message):
    with pytest.raises(ValueError, match=message):
        _lower(extra)


def test_rule_requires_both_counts_and_a_positive_pitch():
    for kwargs in ({"rows": 2}, {"rows": 0, "columns": 2}, {"pitch_nm": 500_000},
                   {"rows": 1, "columns": 1, "pitch_nm": 0}, {"rows": True, "columns": 1}):
        with pytest.raises(ValueError, match="via-in-pad array"):
            PadViaInPadRule(EP, **kwargs)
    board = _array_board()
    footprint = board.footprints["test/qfn16-ep"]
    split = replace(footprint, pads=(*footprint.pads, replace(footprint.pads[-1], position=Point.mm(0, 4))))
    with pytest.raises(ValueError, match="exactly one SMD land"):
        replace(board, footprints={**board.footprints, split.name: split},
                via_in_pad_rules=(PadViaInPadRule(EP, rows=1, columns=1),))


@pytest.mark.parametrize("ep_mm,count,pitch_mm", [("3.1", 3, "1.03"), ("5.1", 4, "1.27")])
def test_default_grid_is_centred_and_evenly_spread(ep_mm, count, pitch_mm):
    board = materialize_via_in_pad_arrays(_array_board(ep_mm, rows=count, columns=count))
    pitch = nm_from_mm(pitch_mm)
    offsets = [(2 * index - count + 1) * pitch // 2 for index in range(count)]
    assert [via.position for via in board.vias] == [
        Point(nm_from_mm(15) + dx, nm_from_mm(10) + dy) for dy in offsets for dx in offsets]
    assert {(via.size_nm, via.drill_nm, via.finish) for via in board.vias} == {
        (nm_from_mm("0.30"), nm_from_mm("0.20"), "filled-capped")}
    assert board.metadata["via_in_pad_count"] == str(count * count)
    assert not _hard(board)


def test_explicit_pitch_and_rotation_follow_the_land():
    board = _array_board(rows=2, columns=3, pitch_nm=nm_from_mm("0.9"))
    upright = {(v.position.x_nm, v.position.y_nm) for v in via_in_pad_array_vias(board)}
    assert upright == {(nm_from_mm(15) + dx, nm_from_mm(10) + dy)
                       for dx in (-900_000, 0, 900_000) for dy in (-450_000, 450_000)}
    turned = replace(board, placements=(replace(board.placements[0], rotation_degrees=90),
                                        *board.placements[1:]))
    assert {(v.position.x_nm, v.position.y_nm) for v in via_in_pad_array_vias(turned)} == {
        (nm_from_mm(15) + dy, nm_from_mm(10) + dx)
        for dx in (-900_000, 0, 900_000) for dy in (-450_000, 450_000)}


def test_permission_without_array_is_unchanged():
    board = _array_board()
    permitted = replace(board, via_in_pad_rules=(PadViaInPadRule(EP),))
    # Placement and routing fingerprints embed this historical repr.
    assert repr(PadViaInPadRule(EP)) == (
        "PadViaInPadRule(pad=PadReference(component='U1', pad='17'), process='filled-capped')")
    assert materialize_via_in_pad_arrays(permitted) is permitted
    assert materialize_hard_macros(permitted) is permitted
    assert macro_source(permitted) is permitted
    assert via_in_pad_array_vias(permitted) == () and not via_in_pad_array_report(permitted)
    from pcbir.drc import physical_board_digest
    assert physical_board_digest(permitted) == physical_board_digest(board)
    assert physical_board_digest(_array_board(rows=1, columns=1)) != physical_board_digest(board)
    # A permission alone never adds array copper to a stitched board.
    assert stitch_zone_pads(permitted).board.vias == stitch_zone_pads(board).board.vias


def test_array_is_the_rebuildable_prefix_and_the_pad_contact():
    source = _array_board(rows=3, columns=3)
    board = materialize_hard_macros(source)
    assert len(board.vias) == 9 and materialize_hard_macros(board) is board
    assert macro_source(board) == replace(source, metadata=board.metadata)
    assert materialize_hard_macros(macro_source(board)) == board
    only_ep = replace(board, nets=(PhysicalNet("GND", (EP,)), *board.nets[1:]))
    stitched = stitch_zone_pads(only_ep)
    assert stitched.stitched_pads == (EP,) and not stitched.pending_pads
    assert stitched.added_track_count == stitched.added_via_count == 0
    # An unrouted caller still gets the array, never a second contact.
    assert stitch_zone_pads(replace(source, nets=only_ep.nets)).board.vias == board.vias
    with pytest.raises(ValueError, match="only partly present"):
        materialize_via_in_pad_arrays(replace(board, vias=board.vias[:4]))
    from pcbir.detailed import route_detailed
    from pcbir.fanout import route_fanout
    from pcbir.routing import route_global
    with pytest.raises(ValueError, match="place required via-in-pad arrays before package escape"):
        route_fanout(source)
    with pytest.raises(ValueError, match="before detailed routing"):
        route_detailed(source, route_global(source))


def _blocked(board: PhysicalBoard, message: str) -> None:
    with pytest.raises(ValueError, match=message):
        materialize_via_in_pad_arrays(board)


def test_any_failed_site_rejects_the_whole_array_by_name():
    board = _array_board(rows=3, columns=3)
    corner = "U1.17: site row 1 column 1 at \\(13.97, 8.97\\) mm"
    keepout = CopperKeepout("probe", (CopperLayer.INTERNAL_3,), PolygonWithHoles(PolygonRing((
        Point.mm("13.8", "8.8"), Point.mm("14.1", "8.8"), Point.mm("14.1", "9.1"), Point.mm("13.8", "9.1")))),
        block_tracks=False, block_vias=True, block_zones=False)
    _blocked(replace(board, copper_keepouts=(keepout,)), corner + " violates a keepout")
    _blocked(replace(board, tracks=(TrackSegment("A", Point.mm(13, "8.97"), Point.mm(14, "8.97"),
                                                 nm_from_mm("0.1"), CopperLayer.INTERNAL_2),)),
             corner + " violates a keepout, copper or drill clearance")
    _blocked(replace(board, vias=(Via("GND", Point.mm("13.97", "8.97"), nm_from_mm("0.3"),
                                      nm_from_mm("0.2"), CopperLayer.FRONT, CopperLayer.BACK),)),
             corner + " coincides with an existing via")
    small = replace(board.zones[0], outline=PolygonWithHoles(PolygonRing((
        Point.mm(14, 1), Point.mm(29, 1), Point.mm(29, 19), Point.mm(14, 19)))))
    _blocked(replace(board, zones=(small,)), corner + " lies outside the inner GND zone")
    # A same-net land is still another land: the annulus may touch only its own.
    footprint = board.footprints["test/qfn16-ep"]
    spur = replace(footprint, pads=(*footprint.pads, FootprintPad(
        "18", Point.mm("-1.03", "-1.25"), Size.mm("0.3", "0.3"))))
    _blocked(replace(board, footprints={**board.footprints, spur.name: spur},
                     nets=(replace(board.nets[0], pads=(*board.nets[0].pads, PadReference("U1", "18"))),
                           *board.nets[1:])),
             corner + " touches another land")
    assert not board.vias


def test_routing_treats_the_array_as_fixed_copper():
    from pcbir import (DetailedRouterOptions, GlobalRouterOptions, PlaneStitchOptions,
                       PlacementPlannerOptions, run_routing_pipeline)
    from pcbir.fanout import FanoutOptions
    board = _array_board(rows=3, columns=3)
    # C runs between the resistors, straight across the package footprint.
    board = replace(board, nets=(board.nets[0], board.nets[2],
        PhysicalNet("C", (PadReference("R1", "2"), PadReference("R2", "2")))))
    result = run_routing_pipeline(
        board,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
            refinement_passes=0, fixed_references=frozenset({"U1", "R1", "R2"})),
        global_options=GlobalRouterOptions(tile_size_nm=nm_from_mm(2), maximum_iterations=2),
        detailed_options=DetailedRouterOptions(pitch_nm=nm_from_mm("0.25"), defer_zone_nets=True),
        fanout_options=FanoutOptions())
    arrays = materialize_via_in_pad_arrays(board).vias
    assert result.package_access is not None and result.package_access.ready
    assert all(item.connected for item in result.detailed.nets if item.net != "GND")
    assert result.board.vias[:len(arrays)] == arrays
    assert result.board.metadata["via_in_pad_count"] == "9"
    assert not _hard(result.board)
    # It threads between the array's rows on an inner layer, clear of every drill.
    assert any(track.net == "C" and track.layer is not CopperLayer.FRONT
               and min(track.start.x_nm, track.end.x_nm) < nm_from_mm(15) < max(track.start.x_nm, track.end.x_nm)
               and abs(track.start.y_nm - nm_from_mm(10)) < nm_from_mm("1.55")
               for track in result.board.tracks)
    # The array is the pad's only contact: stitching it alone adds nothing.
    alone = stitch_zone_pads(result.board, PlaneStitchOptions(only_pads=frozenset({EP})))
    assert alone.stitched_pads == (EP,) and alone.added_via_count == alone.added_track_count == 0
    final = stitch_zone_pads(result.board)
    assert not final.pending_pads and not _hard(final.board)
    in_land = [via for via in final.board.vias
               if max(abs(via.position.x_nm - nm_from_mm(15)),
                      abs(via.position.y_nm - nm_from_mm(10))) <= nm_from_mm("1.55")]
    assert in_land == list(arrays)
    executable = shutil.which("kicad-cli")
    if executable is None:
        pytest.skip("KiCad CLI not installed")
    from pcbir.plane_verify import verify_filled_planes
    evidence = verify_filled_planes(final.board, kicad_cli=Path(executable))
    assert evidence.passed, evidence.findings


def test_hard_macro_pad_array_is_committed_with_owner_copper(tmp_path):
    from hashlib import sha256
    from pcbir.clusters import footprint_geometry_digest
    from pcbir.hard_macros import bind_hard_macro
    from test_hard_macros import fixture
    original, asset, _ = fixture(tmp_path)
    land = replace(original.footprints["test"], pads=(FootprintPad("1", Point(0, 0), Size.mm(1, 1)),))
    for member in asset["members"]:
        member["footprint_digest"] = footprint_geometry_digest(land)
    path = tmp_path / "ground.json"
    path.write_text(json.dumps(asset))
    board = replace(original, footprints={land.name: land}, nets=(replace(original.nets[0], name="GND"),),
                    stackup=SIX, metadata={"fabrication_profile": "jlcpcb-six-layer"},
                    zones=(CopperZone("ground", "GND", (CopperLayer.INTERNAL_1,), PolygonWithHoles(
                        PolygonRing((Point.mm(1, 1), Point.mm(29, 1), Point.mm(29, 29), Point.mm(1, 29))))),))

    def bind(pad: str, **rule) -> PhysicalBoard:
        ruled = replace(board, via_in_pad_rules=(PadViaInPadRule(PadReference(pad, "1"), **rule),))
        return bind_hard_macro(ruled, path, expected_sha256=sha256(path.read_bytes()).hexdigest(),
                               name="ground", bindings={"chip": "U1", "passive": "R1"},
                               net_bindings={"signal": "GND"})

    result = materialize_hard_macros(bind("U1", rows=2, columns=2))
    assert result.materialized_macros == ("ground",) and len(result.tracks) == 1
    assert result.vias == via_in_pad_array_vias(result) and len(result.vias) == 4
    assert materialize_hard_macros(macro_source(result)) == result
    assert not _hard(result)
    assert stitch_zone_pads(result).stitched_pads == (PadReference("R1", "1"), PadReference("U1", "1"))
    # Array copper obeys the macro's private region like any other new copper.
    with pytest.raises(ValueError, match="R1.1: site row 1 column 1 .* violates a keepout"):
        materialize_hard_macros(bind("R1", rows=1, columns=1))


_EXPOSED_LAND = """(footprint "EP_Test"
  (version 20240108)
  (generator "CopperScript-test")
  (layer "F.Cu")
  (attr smd)
  (fp_rect (start -2.6 -2.2) (end 2.6 2.2) (stroke (width 0.05) (type default))
    (fill none) (layer "F.CrtYd"))
  (pad "1" smd roundrect (at -2.0 0) (size 0.6 0.8) (layers "F.Cu" "F.Paste" "F.Mask")
    (roundrect_rratio 0.2))
  (pad "2" smd rect (at 0.5 0) (size 3.1 3.1) (layers "F.Cu" "F.Paste" "F.Mask"))
)
"""


def _project(directory: Path, array: str) -> Path:
    directory.mkdir(exist_ok=True)
    (directory / "EP_Test.kicad_mod").write_text(_EXPOSED_LAND)
    shutil.copy(ROOT / "examples/resolved_footprint_board/footprints/R_0402_CopperScript.kicad_mod",
                directory)
    source = directory / "board.copper"
    source.write_text('''board ArrayBoard {
    use library "tiny";
    component U1: CAPACITOR { value = 1uF; footprint = "EP_Test.kicad_mod"; }
    component R1: RESISTOR { value = 1kohm; footprint = "R_0402_CopperScript.kicad_mod"; }
    component R2: RESISTOR { value = 1kohm; footprint = "R_0402_CopperScript.kicad_mod"; }
    net GND { U1.2; R1.1; R2.1; }
    net SIG { U1.1; R1.2; }
    constraint copper_zone(GND) { layers = "In1.Cu"; }
    constraint via_in_pad(U1.2) { process = "filled-capped"; ''' + array + ''' }
}
''')
    return source


def _route_report(source: Path) -> dict:
    report = source.parent / "route.json"
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "route-board", str(source),
         "--layers", "6", "--fab-profile", "jlcpcb-six-layer", "--width-mm", "40",
         "--height-mm", "30", "--fanout", "--pitch-mm", "0.5",
         "--report", str(report), "-o", str(source.parent / "out.kicad_pcb")],
        cwd=ROOT, text=True, capture_output=True, check=False)
    assert result.returncode in {0, 1}, result.stdout + result.stderr
    return json.loads(report.read_text())


def test_route_and_preflight_reports_list_each_array(tmp_path):
    source = _project(tmp_path / "array", "rows = 3; columns = 3;")
    data = _route_report(source)
    [array] = data["via_in_pad_arrays"]
    assert {key: array[key] for key in ("pad", "rows", "columns", "count", "pitch_nm", "placed")} == {
        "pad": "U1.2", "rows": 3, "columns": 3, "count": 9, "pitch_nm": 1_030_000, "placed": True}
    assert len(set(map(tuple, array["positions_nm"]))) == 9 and not array["explicit_pitch"]
    assert data["fabrication_requirements"][0]["count"] == 9
    assert "U1.2" in data["plane_stitch"]["stitched_pads"]
    assert not [item for item in data["drc"]["findings"] if item["severity"] == "error"
                and item["code"] not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]
    sidecar = json.loads((source.parent / "out.via-process.json").read_text())
    assert sorted(via["position_nm"] for via in sidecar["vias"]) == sorted(array["positions_nm"])
    from pcbir.critical_preflight import main
    for extra in ([], ["--package-access"]):
        report = source.parent / "preflight.json"
        assert main([str(source), "--report", str(report), *extra]) in {0, 1}
        [preflight] = json.loads(report.read_text())["via_in_pad_arrays"]
        assert preflight["count"] == 9 and preflight["placed"]
    # A permission alone adds no report section.
    assert "via_in_pad_arrays" not in _route_report(_project(tmp_path / "plain", ""))

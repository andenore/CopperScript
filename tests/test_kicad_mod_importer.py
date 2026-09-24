from decimal import Decimal
import json
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (
    BoardOutline,
    BoardSide,
    CopperLayer,
    FootprintArc,
    FootprintCircle,
    FootprintLayer,
    FootprintLine,
    FootprintPolygon,
    FootprintRectangle,
    KiCadModImportError,
    KiCadPcbBackend,
    PadKind,
    PadReference,
    PhysicalBoard,
    PhysicalNet,
    Placement,
    Point,
    TrackSegment,
    load_kicad_mod,
    parse_kicad_mod,
    run_physical_drc,
    nm_from_mm,
)
from pcbir.placement import placement_solution_is_legal, resolved_copper_keepouts


ROOT = Path(__file__).parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "footprints" / "R_0402_Test.kicad_mod"


def test_imports_common_kicad_8_footprint_geometry() -> None:
    result = load_kicad_mod(FIXTURE)
    footprint = result.footprint

    assert result.source_version == "20240108"
    assert result.warnings == ()
    assert footprint.name == "R_0402_Test"
    assert footprint.source_library_id == "R_0402_Test"
    assert footprint.metadata["source_format"] == "kicad_mod"
    assert len(footprint.metadata["source_sha256"]) == 64
    assert len(footprint.pads) == 2
    assert footprint.pads[0].position == Point.mm(-0.65, 0)
    assert footprint.pads[0].roundrect_ratio_ppm == 200_000
    assert footprint.pads[1].rotation_degrees == Decimal(90)
    assert footprint.pads[1].has_solder_paste is False
    assert {type(graphic) for graphic in footprint.graphics} == {
        FootprintLine,
        FootprintRectangle,
        FootprintCircle,
        FootprintArc,
        FootprintPolygon,
    }
    assert any(
        graphic.layer is FootprintLayer.COURTYARD
        for graphic in footprint.graphics
    )
    assert len(footprint.courtyard) == 4


def test_imported_footprint_round_trips_through_kicad_pcb_backend() -> None:
    footprint = load_kicad_mod(FIXTURE).footprint
    board = PhysicalBoard(
        name="ImportedFootprint",
        outline=BoardOutline.rectangle(20, 10),
        footprints={footprint.name: footprint},
        placements=(
            Placement("R1", footprint.name, Point.mm(10, 5), value="10 kohm"),
        ),
        nets=(
            PhysicalNet("LEFT", (PadReference("R1", "1"),)),
            PhysicalNet("RIGHT", (PadReference("R1", "2"),)),
        ),
    )

    pcb = KiCadPcbBackend().generate(board).artifacts[0].content

    assert '(footprint "R_0402_Test"' in pcb
    assert '(fp_line' in pcb
    assert '(fp_rect' in pcb
    assert '(fp_circle' in pcb
    assert '(fp_arc' in pcb
    assert '(fp_poly' in pcb
    assert '(layer "F.CrtYd")' in pcb
    assert '(at 0.65 0 90)' in pcb
    pad_two = pcb.split('(pad "2"', 1)[1].split("    )", 1)[0]
    assert '"F.Paste"' not in pad_two


def test_imports_oval_through_hole_drill() -> None:
    source = """(footprint "Slot"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" thru_hole oval
        (at 0 0)
        (size 2 3)
        (drill oval 1 2)
        (layers "*.Cu" "*.Mask")))
    """

    pad = parse_kicad_mod(source).footprint.pads[0]

    assert pad.kind is PadKind.THROUGH_HOLE
    assert pad.drill is not None
    assert pad.drill.width_nm == 1_000_000
    assert pad.drill.height_nm == 2_000_000


def test_allows_repeated_pad_numbers_and_empty_npth_numbers() -> None:
    source = """(footprint "MechanicalAndStacked"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" smd rect (at -1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "1" smd rect (at 1 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (pad "" np_thru_hole circle
        (at 0 2) (size 2 2) (drill 1) (layers "*.Cu" "*.Mask"))
      (pad "" np_thru_hole circle
        (at 0 -2) (size 2 2) (drill 1) (layers "*.Cu" "*.Mask")))
    """

    footprint = parse_kicad_mod(source).footprint

    assert [pad.number for pad in footprint.pads] == ["1", "1", "", ""]
    assert all(
        pad.kind is PadKind.NON_PLATED_THROUGH_HOLE
        for pad in footprint.pads[2:]
    )

    board = PhysicalBoard(
        "RepeatedPadDrc",
        BoardOutline.rectangle(20, 10),
        {footprint.name: footprint},
        (Placement("J1", footprint.name, Point.mm(10, 5)),),
        (PhysicalNet("GND", (PadReference("J1", "1"),)), PhysicalNet("OTHER", ())),
        tracks=(TrackSegment("OTHER", Point.mm(11, 4), Point.mm(11, 6),
                             nm_from_mm("0.25"), CopperLayer.FRONT),),
    )
    assert any(item.code == "DRC-SHORT" for item in run_physical_drc(board).findings)


def test_imports_kicad10_paste_apertures_and_pad_fabrication_metadata() -> None:
    source = """(footprint "ThermalPackage"
      (version 20260206)
      (generator "test")
      (layer "F.Cu")
      (clearance 0.2)
      (fp_rect (start -1 -1) (end 1 1)
        (stroke (width 0) (type solid)) (fill yes) (layer "F.Mask"))
      (pad "" smd roundrect (at 0 0) (size 0.8 0.8)
        (layers "F.Paste") (roundrect_rratio 0.2))
      (pad "1" smd rect (at 0 0) (size 2 2)
        (property pad_prop_heatsink)
        (layers "F.Cu" "F.Mask")
        (zone_connect 2))
      (pad "2" thru_hole circle (at 3 0) (size 1.5 1.5)
        (drill 0.8) (layers "*.Cu" "*.Mask") (remove_unused_layers yes)))
    """

    footprint = parse_kicad_mod(source).footprint

    assert footprint.clearance_nm == 200_000
    assert footprint.graphics[0].layer is FootprintLayer.SOLDER_MASK
    assert footprint.pads[0].kind is PadKind.APERTURE
    assert footprint.pads[0].number == ""
    assert footprint.pads[1].heatsink
    assert footprint.pads[1].zone_connection.value == "thermal"
    assert footprint.pads[2].remove_unused_layers


def test_rejects_back_side_footprint_until_layer_side_is_in_ir() -> None:
    source = """(footprint "BackSide"
      (version 20240108)
      (generator "test")
      (layer "B.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "B.Cu" "B.Mask")))
    """

    with pytest.raises(KiCadModImportError, match="only front-side footprints"):
        parse_kicad_mod(source)


def test_rejects_unsupported_custom_pad_instead_of_losing_geometry() -> None:
    source = """(footprint "Custom"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (pad "1" smd custom
        (at 0 0)
        (size 1 1)
        (layers "F.Cu" "F.Mask")
        (options (clearance outline) (anchor rect))
        (primitives (gr_poly (pts (xy 0 0) (xy 1 0) (xy 0 1)) (width 0) (fill yes)))))
    """

    with pytest.raises(KiCadModImportError, match="pad shape 'custom'"):
        parse_kicad_mod(source)


def test_preserves_embedded_keepout_zone() -> None:
    source = """(footprint "Socket"
      (version 20260206)
      (layer "F.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (zone (layers "F.Cu" "F.CrtYd")
        (keepout (tracks not_allowed) (vias not_allowed)
          (pads not_allowed) (copperpour not_allowed) (footprints allowed))
        (polygon (pts (xy 0 0) (xy 1 0) (xy 1 1)))))
    """

    footprint = parse_kicad_mod(source).footprint
    assert len(footprint.keepouts) == 1
    keepout = footprint.keepouts[0]
    assert keepout.block_tracks and keepout.block_vias
    assert keepout.block_pads and keepout.block_zones
    assert keepout.outline.outer.vertices == (
        Point.mm(0, 0), Point.mm(1, 0), Point.mm(1, 1)
    )


def test_preserves_footprint_placement_keepout() -> None:
    source = """(footprint "Socket" (layer "F.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (zone (layer "F.Cu")
        (keepout (tracks not_allowed) (vias not_allowed)
          (pads not_allowed) (copperpour not_allowed) (footprints not_allowed))
        (polygon (pts (xy 0 0) (xy 1 0) (xy 1 1)))))
    """
    assert parse_kicad_mod(source).footprint.keepouts[0].block_footprints


def test_connector_contact_pad_round_trips_without_paste() -> None:
    source = '''(footprint "Pogo" (layer "F.Cu")
      (pad "1" connect circle (at 0 0) (size 0.8 0.8)
        (layers "F.Cu" "F.Mask")))'''
    footprint = parse_kicad_mod(source).footprint
    assert footprint.pads[0].connector_contact
    assert not footprint.pads[0].has_solder_paste
    board = PhysicalBoard(
        name="PogoBoard", outline=BoardOutline.rectangle(20, 20),
        footprints={footprint.name: footprint},
        placements=(Placement("J1", footprint.name, Point.mm(10, 10)),), nets=(),
    )
    pcb = KiCadPcbBackend().generate(board).artifacts[0].content
    assert '(pad "1" connect circle' in pcb
    assert '(layers "F.Cu" "F.Mask")' in pcb


def test_footprint_local_placement_keepout_blocks_other_component() -> None:
    contact = parse_kicad_mod('''(footprint "Target" (layer "F.Cu")
      (pad "1" connect circle (at 0 0) (size 0.8 0.8)
        (layers "F.Cu" "F.Mask"))
      (zone (layer "F.Cu")
        (keepout (tracks allowed) (vias not_allowed) (pads allowed)
          (copperpour not_allowed) (footprints not_allowed))
        (polygon (pts (xy 2 0) (xy 4 0) (xy 4 2) (xy 2 2)))))''').footprint
    small = parse_kicad_mod('''(footprint "Small" (layer "F.Cu")
      (pad "1" smd rect (at 0 0) (size 0.5 0.5)
        (layers "F.Cu" "F.Mask" "F.Paste")))''').footprint
    placements = (
        Placement("J1", "Target", Point.mm(10, 10)),
        Placement("R1", "Small", Point.mm(13, 11)),
    )
    board = PhysicalBoard(
        name="LocalPlacementKeepout", outline=BoardOutline.rectangle(30, 30),
        footprints={"Target": contact, "Small": small}, placements=placements, nets=(),
    )
    assert not placement_solution_is_legal(board, {p.reference: p for p in placements})
    assert any(item.code == "DRC-PLACEMENT" for item in run_physical_drc(board).findings)
    pcb = KiCadPcbBackend().generate(board).artifacts[0].content
    assert '(footprints not_allowed)' in pcb


def test_kicad10_jumper_setting_must_not_change_pad_connectivity() -> None:
    source = """(footprint "Jumpers" (layer "F.Cu")
      (duplicate_pad_numbers_are_jumpers no) (embedded_fonts no)
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask")))
    """
    assert parse_kicad_mod(source).warnings == ()
    with pytest.raises(KiCadModImportError, match="jumper-linked duplicate pads"):
        parse_kicad_mod(source.replace("jumpers no", "jumpers yes"))


def test_footprint_keepout_moves_with_front_and_back_placements() -> None:
    source = """(footprint "Socket" (layer "F.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (zone (layer "F.Cu")
        (keepout (tracks not_allowed) (vias not_allowed)
          (pads not_allowed) (copperpour not_allowed) (footprints allowed))
        (polygon (pts (xy 1 0) (xy 2 0) (xy 2 1) (xy 1 1)))))
    """
    footprint = parse_kicad_mod(source).footprint
    board = PhysicalBoard(
        name="SocketKeepouts",
        outline=BoardOutline.rectangle(30, 20),
        footprints={footprint.name: footprint},
        placements=(
            Placement("J1", footprint.name, Point.mm(10, 5)),
            Placement("J2", footprint.name, Point.mm(20, 5), side=BoardSide.BACK),
        ),
        nets=(),
    )

    keepouts = resolved_copper_keepouts(board)
    assert keepouts[0].layers == (CopperLayer.FRONT,)
    assert keepouts[0].outline.outer.vertices[0] == Point.mm(11, 5)
    assert keepouts[1].layers == (CopperLayer.BACK,)
    assert keepouts[1].outline.outer.vertices[0] == Point.mm(19, 5)
    pcb = KiCadPcbBackend().generate(board).artifacts[0].content
    assert '(name "J1/keepout-0")' in pcb
    assert '(name "J2/keepout-0")' in pcb
    assert pcb.count("\n    (zone") == 2
    assert "\n  (zone" not in pcb
    assert '(layer "B.Cu")' in pcb


def test_footprint_local_keepout_exempts_own_pad_but_blocks_track() -> None:
    footprint = parse_kicad_mod('''(footprint "LocalRule" (layer "F.Cu")
      (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask"))
      (zone (layer "F.Cu")
        (keepout (tracks not_allowed) (vias not_allowed)
          (pads not_allowed) (copperpour not_allowed) (footprints allowed))
        (polygon (pts (xy -0.5 -0.5) (xy 0.5 -0.5)
                      (xy 0.5 0.5) (xy -0.5 0.5)))))''').footprint
    board = PhysicalBoard(
        name="LocalRuleBoard", outline=BoardOutline.rectangle(20, 20),
        footprints={footprint.name: footprint},
        placements=(Placement("J1", footprint.name, Point.mm(10, 10)),),
        nets=(PhysicalNet("SIGNAL", (PadReference("J1", "1"),)),),
    )
    assert not any(
        item.code == "DRC-COPPER-KEEPOUT"
        for item in run_physical_drc(board).findings
    )
    routed = PhysicalBoard(
        name=board.name, outline=board.outline, footprints=board.footprints,
        placements=board.placements, nets=board.nets,
        tracks=(TrackSegment("SIGNAL", Point.mm(9, 10), Point.mm(11, 10),
                             nm_from_mm("0.2"), CopperLayer.FRONT),),
    )
    assert any(
        item.code == "DRC-COPPER-KEEPOUT"
        for item in run_physical_drc(routed).findings
    )


def test_installed_kicad_accepts_imported_sim_and_rf_keepouts(tmp_path: Path) -> None:
    footprint_root = Path("C:/Program Files/KiCad/10.0/share/kicad/footprints")
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not footprint_root.is_dir() or not Path(cli).is_file():
        pytest.skip("KiCad 10 is not installed")
    sim = load_kicad_mod(
        footprint_root / "Connector_Card.pretty" / "nanoSIM_GCT_SIM8060-6-0-14-00.kicad_mod"
    ).footprint
    rf = load_kicad_mod(
        footprint_root / "Connector_Coaxial.pretty" / "U.FL_Hirose_U.FL-R-SMT-1_Vertical.kicad_mod"
    ).footprint
    board = PhysicalBoard(
        name="ImportedKeepouts",
        outline=BoardOutline.rectangle(50, 30),
        footprints={sim.name: sim, rf.name: rf},
        placements=(
            Placement("J_SIM", sim.name, Point.mm(15, 15)),
            Placement("J_RF", rf.name, Point.mm(35, 15)),
        ),
        nets=(),
    )
    assert not any(
        item.code == "DRC-COPPER-KEEPOUT"
        for item in run_physical_drc(board).findings
    )
    pcb = tmp_path / "ImportedKeepouts.kicad_pcb"
    pcb.write_text(KiCadPcbBackend().generate(board).artifacts[0].content, encoding="utf-8")
    result = subprocess.run(
        [cli, "pcb", "export", "svg", "--mode-multi", "--layers", "F.Cu", "--output", str(tmp_path), str(pcb)],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_installed_tag_connect_target_imports_and_exports(tmp_path: Path) -> None:
    footprint_root = Path("C:/Program Files/KiCad/10.0/share/kicad/footprints")
    cli = shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
    if not footprint_root.is_dir() or not Path(cli).is_file():
        pytest.skip("KiCad 10 is not installed")
    footprint = load_kicad_mod(
        footprint_root / "Connector.pretty" /
        "Tag-Connect_TC2050-IDC-FP_2x05_P1.27mm_Vertical.kicad_mod"
    ).footprint
    assert len([pad for pad in footprint.pads if pad.connector_contact]) == 10
    assert footprint.keepouts[0].block_footprints
    assert footprint.exclude_from_bom and footprint.exclude_from_pos_files
    board = PhysicalBoard(
        name="TagConnectTarget", outline=BoardOutline.rectangle(30, 20),
        footprints={footprint.name: footprint},
        placements=(Placement("J1", footprint.name, Point.mm(15, 10)),), nets=(),
    )
    assert not any(item.code == "DRC-PLACEMENT" for item in run_physical_drc(board).findings)
    pcb = tmp_path / "TagConnectTarget.kicad_pcb"
    pcb.write_text(KiCadPcbBackend().generate(board).artifacts[0].content, encoding="utf-8")
    assert '(attr smd exclude_from_bom exclude_from_pos_files)' in pcb.read_text(encoding="utf-8")
    result = subprocess.run(
        [cli, "pcb", "export", "svg", "--mode-multi", "--layers", "F.Cu", "--output", str(tmp_path), str(pcb)],
        capture_output=True, text=True, timeout=30, check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    drc_path = tmp_path / "tag_drc.json"
    drc = subprocess.run(
        [cli, "pcb", "drc", "--format", "json", "--output", str(drc_path), str(pcb)],
        capture_output=True, text=True, timeout=30, check=False,
    )
    assert drc.returncode == 0, drc.stdout + drc.stderr
    report = json.loads(drc_path.read_text(encoding="utf-8"))
    assert "footprint in keepout" not in str(report).lower()


def test_strict_mode_promotes_loss_warning_to_error() -> None:
    source = """(footprint "WithModel"
      (version 20240108)
      (generator "test")
      (layer "F.Cu")
      (model "part.step" (offset (xyz 0 0 0))))
    """

    result = parse_kicad_mod(source)
    assert result.warnings == (
        "ignored 3D model reference",
        "footprint contains no pads",
    )
    with pytest.raises(KiCadModImportError, match="ignored 3D model"):
        parse_kicad_mod(source, strict=True)

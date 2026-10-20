"""Placed KiCad boards import as unrouted physical IR, exactly or with recorded approximations."""
from dataclasses import replace
from decimal import Decimal
import json
from pathlib import Path

import pytest

from pcbir.backends import KiCadPcbBackend
from pcbir.drc import EXTERNAL_PLACEMENT_KEY, PhysicalDrcPolicy, run_physical_drc
from pcbir.importers.kicad_pcb import KiCadPcbImportError, load_kicad_pcb, parse_kicad_pcb
from pcbir.loader import load_design
from pcbir.physical import BoardSide, CopperLayer, PadReference, PadShape, Point, nm_from_mm
from pcbir.physicalize import PrototypePhysicalOptions, prototype_physicalize
from pcbir.placement import transformed_local_point

ROOT = Path(__file__).parents[1]


@pytest.fixture(scope="module")
def exported(tmp_path_factory):
    design = load_design(ROOT / "examples" / "valid_board" / "board.copper")
    physical = prototype_physicalize(design, PrototypePhysicalOptions())
    rotations, sides = (0, 90, 180, 270, 45), (BoardSide.FRONT, BoardSide.BACK)
    board = replace(physical, placements=tuple(
        replace(p, rotation_degrees=Decimal(rotations[i % 5]), side=sides[(i // 2) % 2])
        for i, p in enumerate(physical.placements)))
    directory = tmp_path_factory.mktemp("export")
    for artifact in KiCadPcbBackend().generate(board).artifacts:
        if artifact.name.endswith((".kicad_pcb", ".kicad_pro")):
            content = artifact.content if isinstance(artifact.content, str) else artifact.content.decode()
            (directory / ("board" + Path(artifact.name).suffix)).write_text(content)
    return board, directory / "board.kicad_pcb"


def _orientation(placement, pad):
    """Board-space pad angle in [0, 360); a back-side pad's local angle is mirrored."""
    sign = 1 if placement.side is BoardSide.FRONT else -1
    return ((placement.rotation_degrees + sign * pad.rotation_degrees) % 360 + 360) % 360


def test_copperscript_export_round_trips_placements_pads_nets_and_outline(exported):
    board, path = exported
    result = load_kicad_pcb(path)
    imported = result.board
    source = {p.reference: p for p in board.placements}
    placed = {p.reference: p for p in imported.placements}
    assert source.keys() == placed.keys()
    for reference, original in source.items():
        copy = placed[reference]
        assert (copy.position, copy.side, copy.rotation_degrees) == (
            original.position, original.side, original.rotation_degrees % 360)
        original_pads = {pad.number: pad for pad in board.footprints[original.footprint].pads}
        copy_pads = {pad.number: pad for pad in imported.footprints[copy.footprint].pads}
        assert original_pads.keys() == copy_pads.keys()
        for number, pad in original_pads.items():
            twin = copy_pads[number]
            a, b = transformed_local_point(original, pad.position), transformed_local_point(copy, twin.position)
            assert abs(a.x_nm - b.x_nm) <= 1 and abs(a.y_nm - b.y_nm) <= 1
            assert (twin.size, twin.kind, twin.shape, twin.drill) == (pad.size, pad.kind, pad.shape, pad.drill)
            if pad.shape is not PadShape.CIRCLE:
                assert (_orientation(original, pad) - _orientation(copy, twin)) % 180 == 0
    assert {n.name: set(n.pads) for n in board.nets if n.pads} == {n.name: set(n.pads) for n in imported.nets}
    assert imported.outline.vertices == board.outline.vertices
    assert imported.stackup.copper_layers == board.stackup.copper_layers
    # Rules come from the exported .kicad_pro next to the board.
    assert imported.rules.minimum_clearance_nm == board.rules.minimum_clearance_nm
    assert imported.rules.default_track_width_nm == board.rules.default_track_width_nm
    assert imported.metadata["imported_unrouted"] == "true"
    assert not imported.tracks and not imported.vias


LEGACY = """(kicad_pcb (version 20171130) (host pcbnew "(5.1.12)")
  (general (thickness 1.6))
  (layers (0 Top signal) (31 Bottom signal) (36 B.SilkS user) (44 Edge.Cuts user))
  (setup (last_trace_width 0.25))
  (net 0 "") (net 1 GND) (net 2 SIG)
  (net_class Default "default" (clearance 0.15) (trace_width 0.2) (via_dia 0.6) (via_drill 0.3)
    (add_net GND) (add_net SIG))
  (module R_0603 (layer Bottom) (tedit 0) (tstamp 1) (at 20 15 90)
    (fp_text reference R1 (at 0 0) (layer B.SilkS) (effects (font (size 1 1) (thickness 0.15)) (justify mirror)))
    (fp_line (start -1 -0.5) (end 1 -0.5) (layer B.SilkS) (width 0.12))
    (fp_arc (start 0 0) (end 1 0) (angle 90) (layer B.SilkS) (width 0.12))
    (pad 1 smd rect (at -0.8 0.2 120) (size 0.8 0.9) (layers Bottom B.Paste B.Mask) (net 1 GND))
    (pad 2 smd trapezoid (at 0.8 0.2 120) (size 0.8 0.9) (rect_delta 0 0.2) (layers Bottom B.Paste B.Mask) (net 2 SIG)))
  (module J_EDGE (layer Top) (tedit 0) (tstamp 2) (at 10 10 0)
    (fp_text reference J1 (at 0 0) (layer F.SilkS) (effects (font (size 1 1) (thickness 0.15))))
    (pad 1 thru_hole circle (at 0 0) (size 1.7 1.7) (drill 1) (layers *.Cu *.Mask F.SilkS) (net 2 SIG))
    (pad 2 smd rect (at 2 0) (size 1 2) (layers Bottom B.Mask) (net 1 GND)))
  (gr_line (start 0 5) (end 0 25) (layer Edge.Cuts) (width 0.1))
  (gr_line (start 5 30) (end 35 30) (layer Edge.Cuts) (width 0.1))
  (gr_line (start 40 25) (end 40 0) (layer Edge.Cuts) (width 0.1))
  (gr_line (start 40 0) (end 5 0) (layer Edge.Cuts) (width 0.1))
  (gr_arc (start 5 5) (end 5 0) (angle -90) (layer Edge.Cuts) (width 0.1))
  (gr_arc (start 5 25) (end 0 25) (angle -90) (layer Edge.Cuts) (width 0.1))
  (gr_arc (start 35 25) (end 35 30) (angle -90) (layer Edge.Cuts) (width 0.1))
  (segment (start 10 10) (end 20 15) (width 0.2) (layer Top) (net 2))
  (via (at 12 12) (size 0.6) (drill 0.3) (layers Top Bottom) (net 1))
)"""


def test_kicad5_board_maps_custom_layers_legacy_arcs_back_side_and_companion_pads():
    result = parse_kicad_pcb(LEGACY, source="legacy", name="legacy")
    board = result.board
    assert board.stackup.copper_layers == (CopperLayer.FRONT, CopperLayer.BACK)
    placements = {p.reference: p for p in board.placements}
    resistor = placements["R1"]
    assert resistor.side is BoardSide.BACK and resistor.rotation_degrees == Decimal(270)
    pad = next(p for p in board.footprints[resistor.footprint].pads if p.number == "1")
    # KiCad board-space pad position is footprint origin + R(-90)(-0.8, 0.2).
    assert transformed_local_point(resistor, pad.position) == Point.mm("20.2", "15.8")
    assert _orientation(resistor, pad) % 180 == Decimal(120) % 180
    # A front connector's bottom-side surface pad becomes a back-side companion.
    companion = placements["J1~opposite"]
    assert companion.side is BoardSide.BACK and companion.position == placements["J1"].position
    contact = board.footprints[companion.footprint].pads[0]
    assert transformed_local_point(companion, contact.position) == Point.mm(12, 10)
    nets = {net.name: set(net.pads) for net in board.nets}
    assert nets["GND"] == {PadReference("R1", "1"), PadReference("J1~opposite", "2")}
    assert nets["SIG"] == {PadReference("R1", "2"), PadReference("J1", "1")}
    # Legacy arcs close the rounded outline; three corners are arcs.
    assert board.outline.boundary_path is not None
    assert sum(type(s).__name__ == "BoundaryArc" for s in board.outline.boundary_path.segments) == 3
    # Legacy net class rules.
    assert board.rules.minimum_clearance_nm == nm_from_mm("0.15")
    assert board.rules.default_track_width_nm == nm_from_mm("0.2")
    assert board.rules.default_via_size_nm == nm_from_mm("0.6")
    # Approximations and dropped routing are recorded, never silent.
    assert any(item.startswith("trapezoid pad as bounding rectangle") for item in result.approximations)
    assert any(item.startswith("opposite-side surface pads") for item in result.approximations)
    assert dict(result.dropped) == {"segment": 1, "via": 1}
    assert "trapezoid" in board.metadata["import_approximations"]


ZONES = """(kicad_pcb (version 20241229) (generator "test")
  (general (thickness 1.6))
  (layers (0 "F.Cu" signal) (4 "In1.Cu" signal) (6 "In2.Cu" signal) (2 "B.Cu" signal) (25 "Edge.Cuts" user))
  (net 0 "") (net 1 "GND") (net 2 "OBSTACLE")
  (footprint "TP" (layer "F.Cu") (at 5 5)
    (property "Reference" "TP1" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "GND")))
  (footprint "TP" (layer "F.Cu") (at 19 5)
    (property "Reference" "TP2" (at 0 0 0) (layer "F.SilkS"))
    (pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "GND")))
  (gr_rect (start 0 0) (end 20 10) (stroke (width 0.1) (type default)) (layer "Edge.Cuts"))
  (zone (net 1) (net_name "GND") (layers "In1.Cu") (name "plane") (priority 1)
    (connect_pads (clearance 0.3)) (min_thickness 0.2)
    (polygon (pts (xy 0 0) (xy 20 0) (xy 20 10) (xy 0 10))))
  (zone (net 2) (net_name "OBSTACLE") (layer "B.Cu")
    (polygon (pts (xy 8 2) (xy 12 2) (xy 12 8) (xy 8 8))))
  (zone (net 0) (net_name "") (layers "F.Cu" "B.Cu") (name "rule")
    (keepout (tracks not_allowed) (vias not_allowed) (pads allowed) (copperpour allowed) (footprints allowed))
    (polygon (pts (xy 1 8) (xy 3 8) (xy 3 9) (xy 1 9))))
  (segment (start 5 5) (end 19 5) (width 0.25) (layer "F.Cu") (net 1))
)"""


def test_kicad9_zones_rule_areas_and_padless_net_copper():
    result = parse_kicad_pcb(ZONES, source="zones")
    board = result.board
    assert board.stackup.copper_layers == (CopperLayer.FRONT, CopperLayer.INTERNAL_1,
                                           CopperLayer.INTERNAL_2, CopperLayer.BACK)
    assert len(board.zones) == 1
    plane = board.zones[0]
    assert (plane.net, plane.layers, plane.priority, plane.clearance_nm, plane.minimum_width_nm) == (
        "GND", (CopperLayer.INTERNAL_1,), 1, nm_from_mm("0.3"), nm_from_mm("0.2"))
    keepouts = {k.id: k for k in board.copper_keepouts}
    rule = keepouts["rule"]
    assert rule.layers == (CopperLayer.FRONT, CopperLayer.BACK)
    assert rule.block_tracks and rule.block_vias and not rule.block_pads and not rule.block_zones
    # Copper on a net without pads is an obstacle for every net.
    assert any(k.layers == (CopperLayer.BACK,) for k in keepouts.values() if k.id != "rule")
    assert "zone on a net without pads as keepout: 1" in result.approximations
    assert board.outline.vertices == (Point.mm(0, 0), Point.mm(20, 0), Point.mm(20, 10), Point.mm(0, 10))
    assert dict(result.dropped) == {"segment": 1}


def test_imported_placement_is_external_to_placement_legality_but_copper_is_checked():
    result = parse_kicad_pcb(ZONES, source="zones")
    board = result.board
    assert board.metadata[EXTERNAL_PLACEMENT_KEY] == "external"
    report = run_physical_drc(board, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    coverage = {item.check: item.status.value for item in report.coverage}
    assert coverage["placement_legality"] == "not_applicable"
    assert "DRC-PLACEMENT" not in {finding.code for finding in report.findings}
    # TP2 sits 0.5 mm from the board edge, inside the planner's 2 mm default.
    checked = replace(board, metadata={k: v for k, v in board.metadata.items() if k != EXTERNAL_PLACEMENT_KEY})
    report = run_physical_drc(checked, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert "DRC-PLACEMENT" in {finding.code for finding in report.findings}


def test_unrepresentable_double_sided_surface_pad_is_reported():
    text = ZONES.replace('(pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu" "F.Mask") (net 1 "GND")))\n  (gr_rect',
                         '(pad "1" smd circle (at 0 0) (size 1 1) (layers "F.Cu" "B.Cu") (net 1 "GND")))\n  (gr_rect')
    with pytest.raises(KiCadPcbImportError, match="not on its footprint's side"):
        parse_kicad_pcb(text, source="double")


def test_project_net_classes_become_per_net_rules(exported, tmp_path):
    _, path = exported
    project = json.loads(path.with_suffix(".kicad_pro").read_text())
    settings = project.setdefault("net_settings", {})
    classes = settings.setdefault("classes", [])
    classes.append({"name": "Power", "clearance": 0.3, "track_width": 0.5, "via_diameter": 0.8, "via_drill": 0.4})
    first_net = load_kicad_pcb(path).board.nets[0].name
    settings["netclass_patterns"] = [{"netclass": "Power", "pattern": first_net[:2] + "*"}]
    (tmp_path / "board.kicad_pcb").write_text(path.read_text())
    (tmp_path / "board.kicad_pro").write_text(json.dumps(project))
    board = load_kicad_pcb(tmp_path / "board.kicad_pcb").board
    rule = next(rule for rule in board.net_routing_rules if rule.net == first_net)
    assert (rule.width_nm, rule.clearance_nm) == (nm_from_mm("0.5"), nm_from_mm("0.3"))

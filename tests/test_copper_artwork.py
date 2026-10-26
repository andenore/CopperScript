"""Opt-in decorative copper is exported and reserved without an electrical net."""
from dataclasses import replace
import os
from pathlib import Path
import shutil
import subprocess

import pytest

from pcbir import (
    ArtworkMaterial,
    BoardOutline,
    BoardSide,
    CopperLayer,
    GlobalRouterOptions,
    PhysicalFootprint,
    FootprintPad,
    PhysicalNet,
    Placement,
    Point,
    Size,
    PadReference,
    PlacementPlannerOptions,
    PlacementRoutingFeedbackOptions,
    run_routing_pipeline,
    compile_design_source,
    compile_source,
)
from pcbir.backends.kicad_pcb import KiCadPcbBackend
from pcbir.drc import DrcCompleteness, physical_board_digest, run_physical_drc
from pcbir.physical import CopperPolygon, DesignRules, MechanicalHole, PolygonRing, TrackSegment
from pcbir.physicalize import prototype_physicalize
from pcbir.placement import placement_solution_is_legal
from pcbir.serializer import board_to_dict
from pcbir.silkscreen import SilkscreenText, artwork_bounds
from pcbir.syntax import CopperScriptError


def board(body=""):
    return prototype_physicalize(compile_design_source(f"board Art {{ {body} }}"))


def pcb(value):
    return KiCadPcbBackend().generate(value).artifacts[0].content


LINE = '''silkscreen { branding { enabled = false; }
    line L { start = (9mm, 10mm); end = (11mm, 10mm); material = copper; thickness = 0.25mm; }
}'''


def test_material_is_opt_in_per_item_with_no_new_electrical_net():
    default = board()
    assert all(i.material is ArtworkMaterial.SILKSCREEN for i in default.silkscreen)
    assert not default.copper_keepouts
    value = board('''silkscreen { branding { enabled = false; }
        logo CU { position = (15mm, 15mm); material = copper; }
        text T { text = "silk"; position = (30mm, 15mm); }
    }''')
    assert not value.nets and not value.placements
    assert all(i.material is ArtworkMaterial.COPPER for i in value.silkscreen if i.id.startswith("CU:"))
    assert value.silkscreen[-1].material is ArtworkMaterial.SILKSCREEN
    assert len(value.copper_keepouts) == 4
    assert "(gr_poly" in pcb(value) and '(layer "F.Cu")' in pcb(value)


def test_default_branding_can_be_copper_and_exposed_on_the_back():
    source = '''board B { silkscreen { branding {
        material = copper; exposed = true; side = back;
    } } }'''
    design = compile_design_source(source)
    value = prototype_physicalize(design)
    assert all(i.material is ArtworkMaterial.COPPER and i.exposed for i in value.silkscreen)
    assert len(value.copper_keepouts) == len(value.silkscreen) == 6
    assert all(k.layers == (CopperLayer.BACK,) for k in value.copper_keepouts)
    assert '(layer "B.Mask")' in pcb(value) and '(justify mirror)' in pcb(value)
    assert '(layer "B.SilkS")' not in pcb(value)
    assert board_to_dict(design)["silkscreen"]["branding"]["material"] == "copper"
    assert board_to_dict(design)["silkscreen"]["branding"]["exposed"] is True
    assert design.electrical == compile_source("board B {}")


def test_mask_is_separate_explicit_opt_in_and_never_adds_paste():
    covered, exposed = board(LINE), board(LINE.replace("material = copper;", "material = copper; exposed = true;"))
    assert len(covered.silkscreen) == len(exposed.silkscreen) == 1
    # The layers appear once in the header; exposed artwork adds an actual drawing.
    assert pcb(exposed).count('(layer "F.Mask")') == pcb(covered).count('(layer "F.Mask")')+1
    assert '(layer "F.Paste")' not in pcb(exposed)
    assert physical_board_digest(covered) != physical_board_digest(exposed)


@pytest.mark.parametrize("properties, message", [
    ('material = plastic;', "ArtworkMaterial"),
    ('exposed = true;', "requires material"),
    ('material = copper; exposed = 1;', "boolean"),
    ('material = F.Cu;', "ArtworkMaterial"),
    ('material = copper; net = GND;', "unknown"),
])
def test_invalid_material_options_are_source_located(properties, message):
    with pytest.raises(CopperScriptError, match=message) as exc:
        compile_source(f'board B {{ silkscreen {{ text T {{ text = "X"; position = (10mm, 10mm); {properties} }} }} }}', "art.copper")
    assert exc.value.location.filename == "art.copper"


def test_reservations_refresh_after_artwork_or_rule_changes_and_cannot_be_removed():
    value = board(LINE)
    assert replace(value, copper_keepouts=()).copper_keepouts == value.copper_keepouts
    thicker_rules = replace(value, rules=replace(value.rules, minimum_clearance_nm=500_000))
    assert thicker_rules.copper_keepouts != value.copper_keepouts
    changed = replace(value, silkscreen=tuple(replace(i, side=BoardSide.BACK) for i in value.silkscreen))
    assert changed.copper_keepouts[0].layers == (CopperLayer.BACK,)
    silk = replace(value, silkscreen=tuple(replace(i, material=ArtworkMaterial.SILKSCREEN) for i in value.silkscreen))
    assert not silk.copper_keepouts


def test_foreign_tracks_are_blocked_on_the_artwork_layer_only():
    value = replace(board(LINE), nets=(PhysicalNet("N", ()),),
        tracks=(TrackSegment("N", Point.mm(10, 8), Point.mm(10, 12), 250_000, CopperLayer.FRONT),))
    assert any(f.code == "DRC-COPPER-KEEPOUT" for f in run_physical_drc(value).findings)
    back = replace(value, tracks=tuple(replace(t, layer=CopperLayer.BACK) for t in value.tracks))
    assert not any(f.code == "DRC-COPPER-KEEPOUT" for f in run_physical_drc(back).findings)


def test_component_placement_fixed_copper_and_holes_respect_artwork():
    value = board(LINE)
    fp = PhysicalFootprint("part", (FootprintPad("1", Point(0,0), Size.mm(1,1)),), Size.mm(2,2))
    placement = Placement("U", "part", Point.mm(10,10))
    occupied = replace(value, footprints={"part":fp}, placements=(placement,))
    assert not placement_solution_is_legal(occupied, {"U":placement})
    assert any(f.code == "DRC-COPPER-KEEPOUT" for f in run_physical_drc(occupied).findings)
    # Decorative drawings must not relax the ownership of electrical polygons.
    with pytest.raises(ValueError, match="materialized hard-macro owner"):
        replace(value, nets=(PhysicalNet("N", ()),), polygons=(CopperPolygon("P", "N", CopperLayer.FRONT,
            PolygonRing(tuple(Point.mm(x,y) for x,y in ((9,9),(11,9),(11,11),(9,11))))),))
    drilled = replace(value, mechanical_holes=(MechanicalHole("H", Point.mm(10,10), 1_000_000),))
    assert any(f.code == "DRC-ARTWORK-HOLE" for f in run_physical_drc(drilled).findings)


def test_text_and_exposed_mask_do_not_silently_pass_physical_signoff():
    value = board('silkscreen { branding { material = copper; exposed = true; } }')
    report = run_physical_drc(value)
    assert report.completeness is DrcCompleteness.INCOMPLETE
    unsupported = {c.check for c in report.coverage if c.status.value == "unsupported"}
    assert {"copper_artwork_glyphs", "copper_artwork_mask"} <= unsupported


def test_copper_curves_reserve_full_circles_and_stroke_width_is_checked():
    value = board('''silkscreen { branding { enabled = false; }
        arc A { start = (8mm, 5mm); mid = (10mm, 3mm); end = (12mm, 5mm); material = copper; thickness = 0.25mm; }
        circle C { center = (20mm, 10mm); radius = 1mm; material = copper; thickness = 0.1mm; }
    }''')
    arc = next(i for i in value.silkscreen if i.id == "A")
    assert artwork_bounds(arc) == (7_875_000, 2_875_000, 12_125_000, 7_125_000)
    assert any(f.code == "DRC-ARTWORK-WIDTH" and f.objects == ("artwork:C",) for f in run_physical_drc(value).findings)


def test_normalized_fill_overlapping_artwork_is_rejected():
    from pcbir.physical import CopperZone, PolygonWithHoles, ZoneFillResult
    value = board(LINE)
    area = PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((8,8),(12,8),(12,12),(8,12)))))
    zone = CopperZone("Z", "N", (CopperLayer.FRONT,), area)
    filled = replace(value, nets=(PhysicalNet("N",()),), zones=(zone,),
        zone_fills=(ZoneFillResult("Z",CopperLayer.FRONT,"fixture-digest","fixture","1",(area,)),))
    assert any(f.code == "DRC-ARTWORK-CLEARANCE" for f in run_physical_drc(filled).findings)


@pytest.mark.parametrize("layout", ["horizontal", "stacked"])
def test_native_text_glyphs_are_inside_the_conservative_envelope(tmp_path, layout):
    interpreter = os.environ.get("KICAD_PYTHON")
    if not interpreter:
        pytest.skip("set KICAD_PYTHON for native copper artwork validation")
    value = board('''silkscreen { branding { material = copper; exposed = true; side = back; LAYOUT }
        text T { text = "Wide MW, Rev 1\\n123456789"; position = (35mm, 25mm); size = 1mm; material = copper; rotation = 37; }
    }'''.replace("LAYOUT", f"layout = {layout};"))
    path = tmp_path / "art.kicad_pcb"
    path.write_text(pcb(value), encoding="utf-8")
    probe = '''import pcbnew,sys,json
b=pcbnew.LoadBoard(sys.argv[1])
out={}
for item in b.GetDrawings():
 if isinstance(item,pcbnew.PCB_TEXT) and item.GetLayer() in (pcbnew.F_Cu,pcbnew.B_Cu):
  box=item.GetBoundingBox()
  out[item.GetText()]=[box.GetX(),box.GetY(),box.GetRight(),box.GetBottom()]
  assert item.IsLocked()
assert any(i.GetLayer()==pcbnew.B_Mask for i in b.GetDrawings())
pcbnew.SaveBoard(sys.argv[2],b)
assert len(b.GetDrawings())==len(pcbnew.LoadBoard(sys.argv[2]).GetDrawings())
print(json.dumps(out))
'''
    result = subprocess.run([interpreter,"-c",probe,str(path),str(tmp_path/"saved.kicad_pcb")],capture_output=True,text=True)
    assert result.returncode == 0, result.stdout+result.stderr
    import json
    actual = json.loads(result.stdout)
    for item in value.silkscreen:
        if not isinstance(item, SilkscreenText):
            continue
        left,low,right,high = artwork_bounds(item)
        x,y,end_x,end_y = actual[item.text]
        assert left <= x <= end_x <= right and low <= y <= end_y <= high


def test_standard_pipeline_routes_around_copper_artwork_without_moving_it():
    value = board('''silkscreen { branding { enabled = false; }
        rectangle ART { start = (9mm, 4mm); end = (11mm, 8mm); filled = true; material = copper; }
    }''')
    fp = PhysicalFootprint("one-pad", (FootprintPad("1",Point(0,0),Size.mm("0.6","0.6")),),Size.mm(1,1))
    value = replace(value, outline=BoardOutline.rectangle(20,12), footprints={fp.name:fp},
        placements=(Placement("J1",fp.name,Point.mm(3,6)), Placement("J2",fp.name,Point.mm(17,6))),
        nets=(PhysicalNet("SIGNAL",(PadReference("J1","1"),PadReference("J2","1"))),))
    result = run_routing_pipeline(value,
        placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0, refinement_passes=0),
        global_options=GlobalRouterOptions(tile_size_nm=2_000_000),
        feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))
    assert result.board.tracks
    assert result.board.silkscreen == value.silkscreen
    assert not any(f.code == "DRC-COPPER-KEEPOUT" for f in result.drc.findings)


def test_native_copper_and_mask_gerbers_are_nonempty(tmp_path):
    interpreter = os.environ.get("KICAD_PYTHON")
    cli = shutil.which("kicad-cli")
    if not cli and interpreter:
        candidate = Path(interpreter).with_name("kicad-cli.exe")
        cli = str(candidate) if candidate.is_file() else None
    if not cli:
        pytest.skip("native KiCad CLI is unavailable")
    source = tmp_path/"art.kicad_pcb"
    source.write_text(pcb(board('silkscreen { branding { material = copper; exposed = true; } }')),encoding="utf-8")
    output = tmp_path/"gerbers"
    result = subprocess.run([cli,"pcb","export","gerbers","--layers","F.Cu,F.Mask","--output",str(output),str(source)],capture_output=True,text=True)
    assert result.returncode == 0, result.stdout+result.stderr
    layers = [p for p in output.iterdir() if p.suffix in {".gtl",".gts",".gbr"}]
    assert len(layers) == 2 and all("D01*" in p.read_text() for p in layers)

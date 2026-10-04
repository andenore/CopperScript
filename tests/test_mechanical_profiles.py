"""Reusable mechanical contracts never add electrical connectivity."""
from dataclasses import replace
from decimal import Decimal
import json
import subprocess
from pathlib import Path

import pytest

from pcbir.compiler import compile_design_file, compile_design_source
from pcbir.editor.scene import board_scene
from pcbir.editor.source import SourceEditError, SourceSnapshot, fixed_placement_patch
from pcbir.elaborate import elaborate
from pcbir.footprints import FootprintResolver
from pcbir.parser import parse
from pcbir.physical import BoardSide, Point
from pcbir.physicalize import prototype_physicalize, resolved_physicalize
from pcbir.placement import transformed_local_point
from pcbir.serializer import board_to_dict
from pcbir.syntax import CopperScriptError


PROFILE = '''board_profile Carrier {
    outline rectangle { width = 50mm; height = 35mm; }
    hole H1 { position = (4mm,4mm); diameter = 2mm; }
    connector debug { anchor_pad = "1"; position = (10mm,10mm); rotation = 90; side = front; }
    keepout HOST polygon { vertices = [(25mm,20mm),(40mm,20mm),(40mm,30mm),(25mm,30mm)]; side = back; }
    copper_keepout ANT rectangle { origin = (40mm,5mm); width = 5mm; height = 5mm; layers = "F.Cu,B.Cu"; }
}'''
BOARD = '''board B { import mechanical "./mechanics"; use library "standard";
    component J1: RESISTOR { footprint = "0603"; }
    net N { J1.1; J1.2; }
    mechanical { use mechanical.Carrier as host { debug = J1; }
        hole H1 { position = (45mm,4mm); diameter = 2mm; }
    }
}'''


def project(tmp_path, profile=PROFILE, board=BOARD, extras=None):
    package = tmp_path / "mechanics"
    package.mkdir(exist_ok=True)
    (package / "carrier.copper").write_text(profile, encoding="utf-8")
    for filename, source in (extras or {}).items():
        (package / filename).write_text(source, encoding="utf-8")
    path = tmp_path / "board.copper"
    path.write_text(board, encoding="utf-8")
    return path


def test_relative_import_without_manifest_locked_offline_and_separate_ir(tmp_path):
    path = project(tmp_path)
    raw = path.read_bytes()
    result = compile_design_file(path, locked=True, offline=True)
    assert result.mechanical.outline.vertices[2] == Point.mm(50,35)
    assert {h.id for h in result.mechanical.holes} == {"host/H1", "H1"}
    assert [c.ref for c in result.electrical.components] == ["J1"]
    assert len(result.electrical.nets) == 1 and not result.electrical.constraints
    assert "mechanical" not in board_to_dict(result.electrical)
    assert not (tmp_path / "copper.lock").exists()
    assert path.read_bytes() == raw
    assert result.electrical.dependencies[0].checksum.startswith("sha256:")
    data = board_to_dict(result)["mechanical"]
    assert data["connectors"][0]["reference"] == "J1"
    imported = [s for s in data["provenance"]["features"] if s["read_only"]]
    assert imported and all(s["source"]["filename"].endswith("carrier.copper") for s in imported)
    json.dumps(data)


@pytest.mark.parametrize("side", ["front", "back"])
@pytest.mark.parametrize("rotation", [0,45,90,180,270])
def test_pad_anchor_transform_and_lock_retention(tmp_path, side, rotation):
    profile = PROFILE.replace("rotation = 90", f"rotation = {rotation}").replace("side = front", f"side = {side}")
    design = compile_design_file(project(tmp_path, profile))
    board = prototype_physicalize(design)
    pose = board.placements[0]
    fp = board.footprints[pose.footprint]
    anchor = next(p for p in fp.pads if p.number == "1")
    assert transformed_local_point(pose, anchor.position) == Point.mm(10,10)
    assert pose.rotation_degrees == Decimal(rotation) and pose.side == BoardSide(side)
    assert board.placement_rules[0].fixed_position == pose.position
    scene = board_scene(board, source_revision="test")
    assert scene["components"][0]["profile_role"] == "host/debug"
    assert scene["components"][0]["source_position_locked"]
    assert scene["mechanical_provenance"]["profiles"][0]["profile"] == "mechanical.Carrier"


def test_keepouts_reach_physical_ir_and_kicad(tmp_path):
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    board = prototype_physicalize(compile_design_file(project(tmp_path)))
    assert board.keepouts[0].name == "host/HOST"
    assert board.keepouts[0].side == BoardSide.BACK
    assert board.copper_keepouts[0].id == "host/ANT"
    assert board.copper_keepouts[0].block_tracks
    text = KiCadPcbBackend().generate(board).artifacts[0].content
    assert "keepout" in text and "Edge.Cuts" in text


@pytest.mark.parametrize("change,message", [
    (lambda b:b.replace("debug = J1;", ""), "requires exactly"),
    (lambda b:b.replace("debug = J1;", "debug = J1; extra = J1;"), "requires exactly"),
    (lambda b:b.replace("debug = J1;", "debug = UNKNOWN;"), "unknown component"),
    (lambda b:b.replace("mechanical.Carrier", "mechanical.Missing"), "unknown board profile"),
    (lambda b:b.replace("hole H1 { position", "outline circle { diameter = 50mm; } hole H1 { position"), "exactly one"),
    (lambda b:b.replace("hole H1 { position", "use mechanical.Carrier as host { debug = J1; } hole H1 { position"), "duplicate profile instance"),
])
def test_use_errors_are_source_located(tmp_path, change, message):
    with pytest.raises(CopperScriptError, match=message) as exc:
        compile_design_file(project(tmp_path, board=change(BOARD)))
    assert exc.value.location.filename.endswith(".copper")


@pytest.mark.parametrize("change,message", [
    (lambda p:p.replace('anchor_pad = "1";', 'anchor_pad = "999";'), "exactly one electrical"),
    (lambda p:p.replace('anchor_pad = "1";', ''), "missing mechanical property"),
    (lambda p:p.replace('rotation = 90;', 'rotation = true;'), "numeric degrees"),
    (lambda p:p.replace('side = front;', 'side = both;'), "valid BoardSide"),
    (lambda p:p.replace('position = (10mm,10mm);', 'position = (10V,10mm);'), "unsupported Length"),
    (lambda p:p.replace('anchor_pad = "1";', 'anchor_pad = "1"; component = J1;'), "explicit bindings"),
    (lambda p:p.replace('layers = "F.Cu,B.Cu";', 'layers = "In1.Cu";'), "outside the stackup"),
    (lambda p:p.replace('layers = "F.Cu,B.Cu";', 'layers = "F.Cu,B.Cu"; block_vias = 1;'), "booleans"),
])
def test_profile_validation_errors(tmp_path, change, message):
    with pytest.raises((CopperScriptError, ValueError), match=message):
        prototype_physicalize(compile_design_file(project(tmp_path, change(PROFILE))))


@pytest.mark.parametrize("constraint", [
    'constraint fixed_placement(J1) { x = 30mm; y = 10mm; rotation = 90; side = front; }',
    'constraint allowed_orientations(J1) { values = "0,180"; }',
    'constraint fixed_placement(J1) { x = 10mm; y = 10mm; rotation = 0; side = back; }',
])
def test_connector_conflicts_do_not_depend_on_order(tmp_path, constraint):
    for reverse in (False, True):
        source = BOARD.replace("net N", constraint + " net N") if reverse else BOARD[:-1] + constraint + "}"
        with pytest.raises(ValueError, match="conflict|allowed orientations"):
            prototype_physicalize(compile_design_file(project(tmp_path, board=source)))


def test_no_footprint_is_not_silently_omitted(tmp_path):
    board = BOARD.replace('{ footprint = "0603"; }', ';')
    design = compile_design_file(project(tmp_path, board=board))
    electrical = replace(design.electrical, library={name: replace(part, footprints=())
                         for name, part in design.electrical.library.items()})
    with pytest.raises(ValueError, match="selected footprint"):
        prototype_physicalize(replace(design, electrical=electrical))


def test_optional_exact_footprint_requirement(tmp_path):
    bad = PROFILE.replace('anchor_pad = "1";', 'anchor_pad = "1"; footprint = "other:wrong";')
    with pytest.raises(ValueError, match="requires footprint"):
        prototype_physicalize(compile_design_file(project(tmp_path, bad)))


def test_profile_composition_retains_instances_and_remaps_roles(tmp_path):
    parent = '''board_profile Carrier { use Frame as frame {} use Header as debug { port = debug; } }'''
    extra = {"frame.copper": 'board_profile Frame { outline rectangle { width = 50mm; height = 35mm; } hole H1 { position = (4mm,4mm); diameter = 2mm; } }',
             "header.copper": 'board_profile Header { connector port { anchor_pad = "1"; position = (10mm,10mm); rotation = 90; side = front; } }'}
    design = compile_design_file(project(tmp_path, parent, extras=extra))
    assert [p.name for p in design.mechanical.profiles] == ["host", "host/frame", "host/debug"]
    assert design.mechanical.connectors[0].role == "host/debug/port"
    assert dict(design.mechanical.profiles[0].bindings) == {"debug": "J1"}
    assert dict(design.mechanical.profiles[-1].bindings) == {"port": "J1"}
    assert design.mechanical.holes[0].id == "host/frame/H1"


def test_nested_relative_imports_and_sibling_paths(tmp_path):
    (tmp_path / "copper.mod").write_text("module test/project\n")
    sibling = tmp_path / "common"
    sibling.mkdir()
    (sibling / "frame.copper").write_text('board_profile Frame { outline rectangle { width = 50mm; height = 35mm; } }')
    parent = 'board_profile Carrier { import base "../common"; use base.Frame {} connector debug { anchor_pad = "1"; position = (10mm,10mm); rotation = 90; side = front; } }'
    design = compile_design_file(project(tmp_path, parent))
    assert len(design.mechanical.profiles) == 2
    assert len(design.electrical.dependencies) == 2


def test_local_import_cycle_and_profile_cycle(tmp_path):
    with pytest.raises(CopperScriptError, match="cyclic board profile"):
        compile_design_file(project(tmp_path, 'board_profile Carrier { use Carrier {} }'))
    with pytest.raises(CopperScriptError, match="cyclic package import"):
        compile_design_file(project(tmp_path, 'board_profile Carrier { import self "./."; use self.Carrier {} }'))


def test_relative_import_escape_rejected(tmp_path):
    inside = tmp_path / "project"
    inside.mkdir()
    path = project(inside, board=BOARD.replace('"./mechanics"', '"../outside"'))
    (tmp_path / "outside").mkdir()
    with pytest.raises(CopperScriptError, match="stay inside"):
        compile_design_file(path)


def test_locked_remote_style_profile_inventory_is_authenticated(tmp_path):
    path = project(tmp_path, board=BOARD.replace('"./mechanics"', '"github.com/test/profiles/mechanics"'))
    (tmp_path / "copper.mod").write_text('module test/board\nrequire github.com/test/profiles v1\nreplace github.com/test/profiles => .\n')
    compile_design_file(path, offline=True)
    assert (tmp_path / "copper.lock").exists()
    compile_design_file(path, locked=True, offline=True)
    (tmp_path / "mechanics/carrier.copper").write_text(PROFILE + "\n// Changed.")
    with pytest.raises(CopperScriptError, match="lock mismatch"):
        compile_design_file(path, locked=True, offline=True)


def test_hierarchical_binding_preserves_electrical_hierarchy(tmp_path):
    path = project(tmp_path, board=BOARD.replace('component J1: RESISTOR { footprint = "0603"; }', 'module HOST: mechanical.Host;').replace('net N { J1.1; J1.2; }', '').replace('debug = J1;', 'debug = "HOST/J1";'),
        extras={"host.copper": 'module Host { use library "standard"; component J1: RESISTOR { footprint = "0603"; } }'})
    design = compile_design_file(path)
    assert design.electrical.module_instances[0].ref == "HOST"
    assert elaborate(design.electrical).components[0].ref == "HOST/J1"
    assert prototype_physicalize(design).placements[0].reference == "HOST/J1"


def test_editor_patch_cannot_add_competing_imported_pose(tmp_path):
    path = project(tmp_path)
    with pytest.raises(SourceEditError, match="imported board profile"):
        fixed_placement_patch(SourceSnapshot(path.read_bytes(), str(path)), "J1", x_nm=1,y_nm=2)


def test_profile_cannot_declare_electrical_data():
    for declaration in ('component J1: RESISTOR;', 'net N {}', 'mechanical {}'):
        with pytest.raises(CopperScriptError, match="unknown mechanical declaration"):
            parse(f"board_profile B {{ {declaration} }}")


@pytest.mark.skipif(not Path('C:/Program Files/KiCad/10.0/share/kicad/footprints').is_dir(), reason="installed KiCad footprints needed")
@pytest.mark.parametrize("side", ["front", "back"])
@pytest.mark.parametrize("rotation", [0,45,90])
def test_real_footprint_anchor_and_native_export(tmp_path, side, rotation):
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    source = BOARD.replace('"0603"', '"Resistor_SMD:R_0603_1608Metric"')
    profile = PROFILE.replace('side = front;', f'side = {side};').replace('rotation = 90;', f'rotation = {rotation};')
    design = compile_design_file(project(tmp_path, profile, source))
    board = resolved_physicalize(design, FootprintResolver(tmp_path,
        search_roots=(Path('C:/Program Files/KiCad/10.0/share/kicad/footprints'),)))
    pose = board.placements[0]
    pad = next(p for p in board.footprints[pose.footprint].pads if p.number == "1")
    assert transformed_local_point(pose, pad.position) == Point.mm(10,10)
    manifest = KiCadPcbBackend().generate(board)
    output = tmp_path / 'anchor.kicad_pcb'
    write_kicad_project(manifest, output)
    native = Path('C:/Program Files/KiCad/10.0/bin/python.exe')
    if native.is_file():
        script = 'import json, pcbnew, sys; b=pcbnew.LoadBoard(sys.argv[1]); print(json.dumps([[p.GetPosition().x,p.GetPosition().y] for f in b.GetFootprints() if f.GetReference()=="J1" for p in f.Pads() if p.GetNumber()=="1"]))'
        actual = subprocess.run([str(native), '-c', script, str(output)], capture_output=True, text=True, timeout=30, check=True)
        points = json.loads(actual.stdout)
        assert len(points) == 1
        assert abs(points[0][0]-10000000) <= 1 and abs(points[0][1]-10000000) <= 1


def test_duplicate_physical_anchor_land_rejected(tmp_path, monkeypatch):
    import pcbir.physicalize as lowering
    provider = lowering._proxy_footprint
    def duplicate(part, component, selected):
        fp = provider(part, component, selected)
        p = next(p for p in fp.pads if p.number == "1")
        return replace(fp, pads=(*fp.pads, replace(p, position=Point.mm(3,0))))
    monkeypatch.setattr(lowering, "_proxy_footprint", duplicate)
    with pytest.raises(ValueError, match="exactly one electrical physical land"):
        prototype_physicalize(compile_design_file(project(tmp_path)))


def test_conflicting_duplicate_fixed_constraints_rejected(tmp_path):
    board = 'board B { use library "standard"; component R1: RESISTOR; constraint fixed_placement(R1) { x = 10mm; y = 10mm; } constraint fixed_placement(R1) { x = 12mm; y = 10mm; } }'
    with pytest.raises(ValueError, match="conflicting fixed_position"):
        prototype_physicalize(compile_design_source(board))


def test_duplicate_rules_fail_disjoint_rules_compose(tmp_path):
    profile = PROFILE.replace('hole H1', 'rules { minimum_clearance = 0.15mm; } hole H1')
    board = BOARD.replace('hole H1', 'rules { minimum_track_width = 0.15mm; } hole H1')
    result = compile_design_file(project(tmp_path, profile, board))
    assert len(result.mechanical.rule_overrides) == 2
    with pytest.raises(CopperScriptError, match="conflicting mechanical rule ownership"):
        compile_design_file(project(tmp_path, profile, board.replace('minimum_track_width', 'minimum_clearance')))


def test_local_content_digest_tracks_edits_without_lock(tmp_path):
    path = project(tmp_path)
    a = compile_design_file(path, locked=True, offline=True)
    (tmp_path / 'mechanics/carrier.copper').write_text(PROFILE + '\n// revision')
    b = compile_design_file(path, locked=True, offline=True)
    assert a.electrical.dependencies[0].checksum != b.electrical.dependencies[0].checksum
    assert a.electrical.components == b.electrical.components
    assert a.electrical.nets == b.electrical.nets


def test_duplicate_profile_exports_rejected(tmp_path):
    with pytest.raises(CopperScriptError, match="duplicate board profile names"):
        compile_design_file(project(tmp_path, extras={"duplicate.copper": PROFILE}))


def test_shared_bound_component_and_duplicate_roles_rejected(tmp_path):
    duplicate = 'connector second { anchor_pad = "1"; position = (20mm,10mm); rotation = 90; side = front; }'
    profile = PROFILE[:-1] + duplicate + "}"
    with pytest.raises(CopperScriptError, match="same component"):
        compile_design_file(project(tmp_path, profile, BOARD.replace('debug = J1;', 'debug = J1; second = J1;')))
    with pytest.raises(CopperScriptError, match="duplicate connector roles"):
        compile_design_file(project(tmp_path, profile.replace('connector second', 'connector debug')))


def test_relative_symlink_guard(tmp_path, monkeypatch):
    path = project(tmp_path)
    original = Path.is_symlink
    monkeypatch.setattr(Path, 'is_symlink', lambda p: p.name == 'mechanics' or original(p))
    with pytest.raises(CopperScriptError, match="stay inside"):
        compile_design_file(path)


@pytest.mark.parametrize("version", ["v1", "workspace"])
def test_external_relative_import_retains_digest_and_rejects_mid_resolution_change(tmp_path, version):
    from pcbir.packages import PackageResolver
    from pcbir.syntax import SourceLocation
    dep = tmp_path / 'dep'
    package = dep / 'mechanics'
    child = package / 'child'
    child.mkdir(parents=True)
    entry = package / 'parent.copper'
    entry.write_text('board_profile Parent {}')
    asset = child / 'child.copper'
    asset.write_text('board_profile Child {}')
    (tmp_path/'copper.mod').write_text(f'module test/root\nrequire github.com/test/dep {version}\nreplace github.com/test/dep => ./dep\n')
    resolver = PackageResolver.for_source(tmp_path/'board.copper', SourceLocation(str(tmp_path/'board.copper'),0,1,1))
    parent = resolver.resolve('github.com/test/dep/mechanics', SourceLocation(str(tmp_path/'board.copper'),0,1,1))
    relative = resolver.resolve('./child', SourceLocation(str(entry),0,1,1))
    assert relative.checksum == parent.checksum and relative.version == version
    asset.write_text('board_profile Child {} // Changed')
    with pytest.raises(CopperScriptError, match="changed during relative import"):
        resolver.resolve('./child', SourceLocation(str(entry),0,1,1))


def test_auto_placement_retains_imported_connector_pose(tmp_path):
    from pcbir.layout import plan_placement
    from pcbir.placement import PlacementPlannerOptions
    board = prototype_physicalize(compile_design_file(project(tmp_path)))
    placed = plan_placement(board, PlacementPlannerOptions(candidate_count=1, analytical_iterations=3, refinement_passes=0)).board
    assert placed.placements == board.placements


def test_imported_copper_keepouts_block_tracks_and_vias(tmp_path):
    from pcbir.routing_clearance import RoutingClearanceIndex
    from pcbir.physical import CopperLayer
    board = prototype_physicalize(compile_design_file(project(tmp_path)))
    index = RoutingClearanceIndex(board)
    for layer in (CopperLayer.FRONT, CopperLayer.BACK):
        assert not index.can_track('N', Point.mm(40.5,5.5), Point.mm(44.5,5.5), 200000, layer)
        assert index.can_track('N', Point.mm(30,5.5), Point.mm(31.5,5.5), 200000, layer)
    assert not index.can_via('N', Point.mm(42,7), 600000, CopperLayer.FRONT, CopperLayer.BACK)

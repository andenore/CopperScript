from dataclasses import replace
from pathlib import Path

import pytest

from pcbir.compiler import compile_design_source
from pcbir.editor.session import EditorError, EditorSession, StaleRevision
from pcbir.editor.source import (SourceEditError, SourceSnapshot, declaration_index,
    fixed_placement_patch, mechanical_patch)
from pcbir.editor.transactions import SourceWorkspace, electrical_identity
from pcbir.physical import BoardSide, Point
from pcbir.physicalize import prototype_physicalize
from pcbir.placement import PlacementPlannerOptions, placement_solution_is_legal
from pcbir.parser import parse
from pcbir.syntax import CopperScriptError


SOURCE = '''// Preserve Ω and comments.
board EditorSave {
    use library "standard";
    component R1: RESISTOR;
    component R2: RESISTOR;
    net A { R1.1; R2.1; }
    net B { R1.2; R2.2; }
    mechanical {
        outline rectangle { width = 40mm; height = 30mm; }
        hole H1 { position = (3mm,3mm); diameter = 1mm; }
    }
    constraint fixed_placement(R1) { x = 8mm; y = 8mm; rotation = 0; side = front; }
    constraint fixed_placement(R2) { x = 20mm; y = 10mm; rotation = 0; side = front; }
    constraint allowed_orientations(R1) { values = "0,90,180,270"; }
} // End.
'''


def session(tmp_path, raw=None, *, input_paths=()):
    source = tmp_path / "board.copper"
    source.write_bytes(raw or SOURCE.encode())
    design = compile_design_source(SourceSnapshot(source.read_bytes()).text, str(source), offline=True)
    board = prototype_physicalize(design)
    return EditorSession(board, source, PlacementPlannerOptions(candidate_count=1,
        analytical_iterations=2, refinement_passes=0), workspace=SourceWorkspace(source, design,
        prototype_physicalize, input_paths=input_paths))


def op(s, action, **fields):
    return s.operation(dict(action=action, revision=s.revision, **fields))


def lock(s, *, locks=None, x=9000000, rotation=90):
    return op(s, "prepare_lock", reference="R1", x_nm=x, y_nm=8000000,
              rotation=rotation, side="back", locks=locks or ["position", "rotation", "side"])


def save(s):
    return op(s, "save_source", review_id=s.source_pending.id)


def test_review_save_preserves_bytes_and_electrical_identity(tmp_path):
    s = session(tmp_path, b"\xef\xbb\xbf" + SOURCE.replace("\n", "\r\n").encode())
    raw = s.source.read_bytes()
    original = s.workspace.identity
    result = lock(s)
    assert s.source.read_bytes() == raw
    assert result["scene"]["source_review"]["diff"]
    assert result["source_preview"]["components"][0]["position"] == [9000000, 8000000]
    save(s)
    assert s.source.read_bytes() == raw.replace(b"x = 8mm", b"x = 9mm", 1).replace(
        b"rotation = 0; side = front", b"rotation = 90; side = back", 1)
    assert s.workspace.identity == original
    design = compile_design_source(SourceSnapshot(s.source.read_bytes()).text, str(s.source), offline=True)
    assert electrical_identity(design.electrical) == original
    reopened = prototype_physicalize(design)
    assert reopened.placements[0].position == Point.mm(9, 8)
    assert reopened.placements[0].side is BoardSide.BACK
    assert s.scene()["outputs_stale"]
    assert not s.state.board.tracks and not s.state.board.zone_fills
    assert not list(tmp_path.glob(".copper-edit-*"))
    assert not s.source.with_name("board.copper.editor-lock").exists()


@pytest.mark.parametrize("locks", [["position"], ["rotation"], ["side"], ["position", "side"], []])
def test_partial_pose_locks_are_independent(tmp_path, locks):
    s = session(tmp_path)
    op(s, "prepare_lock", reference="R1", x_nm=9000000, y_nm=8000000,
       rotation=90, side="back", locks=locks)
    save(s)
    rule = next((r for r in s.state.board.placement_rules if r.reference == "R1"), None)
    assert bool(rule and rule.fixed_position is not None) == ("position" in locks)
    assert bool(rule and rule.fixed_rotation_degrees is not None) == ("rotation" in locks)
    assert bool(rule and rule.side is not None) == ("side" in locks)
    assert len(s.state.board.placement_rules) == 2  # Orientation rule survives unlocking.


def test_position_only_allows_rotation_not_translation(tmp_path):
    s = session(tmp_path)
    lock(s, locks=["position"]); save(s)
    poses = {p.reference: p for p in s.state.board.placements}
    poses["R1"] = replace(poses["R1"], rotation_degrees=90, side=BoardSide.BACK)
    assert placement_solution_is_legal(s.state.board, poses, s.options)
    poses["R1"] = replace(poses["R1"], position=Point.mm(10, 8))
    assert not placement_solution_is_legal(s.state.board, poses, s.options)


def test_source_undo_redo_are_persisted_and_revision_checked(tmp_path):
    s = session(tmp_path)
    raw = s.source.read_bytes()
    lock(s); save(s)
    saved = s.source.read_bytes()
    assert not s.undo_stack
    op(s, "undo_source")
    assert s.source.read_bytes() == raw
    op(s, "redo_source")
    assert s.source.read_bytes() == saved
    s.source.write_bytes(saved + b"// external\n")
    with pytest.raises(StaleRevision, match="externally"):
        op(s, "undo_source")
    assert s.source.read_bytes().endswith(b"// external\n")


def test_external_change_between_review_and_save_never_wins(tmp_path):
    s = session(tmp_path)
    lock(s)
    external = s.source.read_bytes() + b"// External edit.\n"
    s.source.write_bytes(external)
    with pytest.raises(StaleRevision):
        save(s)
    assert s.source.read_bytes() == external
    op(s, "reload_source")
    assert s.source_pending is None and not s.scene()["source_stale"]
    assert not s.source_undo
    lock(s); save(s)
    assert b"// External edit." in s.source.read_bytes()


def test_exclusive_lock_and_failed_cas_leave_external_bytes_untouched(tmp_path):
    s = session(tmp_path)
    lock(s)
    path = s.source.with_name("board.copper.editor-lock")
    path.write_bytes(b"owned")
    with pytest.raises(SourceEditError, match="another editor"):
        save(s)
    assert path.read_bytes() == b"owned"
    path.unlink()
    review = s.source_pending
    s.source.write_bytes(b"external")
    with pytest.raises(SourceEditError, match="since review"):
        s.workspace.save(review)
    assert s.source.read_bytes() == b"external" and not path.exists()


def test_review_id_must_match_and_blocks_other_mutations(tmp_path):
    s = session(tmp_path)
    lock(s)
    with pytest.raises(EditorError, match="save or discard"):
        op(s, "auto_place")
    with pytest.raises(StaleRevision, match="review changed"):
        op(s, "save_source", review_id="wrong")
    op(s, "discard_source")
    assert s.source.read_bytes() == SOURCE.encode()
    assert not s.source_pending


def test_inputs_modified_after_review_are_rejected(tmp_path):
    path = tmp_path / "footprint.kicad_mod"
    path.write_bytes(b"input")
    s = session(tmp_path, input_paths=(path,))
    lock(s)
    path.write_bytes(b"modified")
    with pytest.raises(SourceEditError, match="inputs changed"):
        save(s)
    assert s.source.read_bytes() == SOURCE.encode()


def test_electrical_changes_cannot_be_reviewed(tmp_path):
    s = session(tmp_path)
    raw = SOURCE.replace("R2.1", "R2.2").encode()
    with pytest.raises(SourceEditError, match="electrical"):
        s.workspace.review(SourceSnapshot(s.source.read_bytes()), raw, s.state.board, s.options)


@pytest.mark.parametrize("x,rotation", [(100000000, 90), (9000000, 45)])
def test_illegal_geometry_or_angle_has_no_side_effect(tmp_path, x, rotation):
    s = session(tmp_path)
    with pytest.raises(ValueError, match="violates|allowed"):
        lock(s, x=x, rotation=rotation)
    assert s.source.read_bytes() == SOURCE.encode() and s.source_pending is None


def test_mechanical_review_save_history_and_comment_preservation(tmp_path):
    s = session(tmp_path)
    op(s, "prepare_mechanical", kind="outline", name="", shape="circle",
       parameters={"diameter": "60mm", "center": "(20mm,15mm)"}, remove=False)
    assert s.source.read_bytes() == SOURCE.encode()
    save(s)
    assert s.state.board.outline.circular_boundary
    op(s, "undo_source")
    assert s.source.read_bytes() == SOURCE.encode()
    op(s, "prepare_mechanical", kind="hole", name="H1", shape="",
       parameters={"position": "(3mm,3mm)", "diameter": "1.2mm", "head_clearance_radius": "2mm"}, remove=False)
    save(s)
    assert s.state.board.mechanical_holes[0].diameter_nm == 1200000
    op(s, "prepare_mechanical", kind="hole", name="H1", shape="", parameters={}, remove=True)
    save(s)
    assert not s.state.board.mechanical_holes


@pytest.mark.parametrize("parameters", [
    {"position": "(8mm,8mm)", "diameter": "2mm"},
    {"position": "(50mm,50mm)", "diameter": "2mm"},
])
def test_hole_outside_material_or_under_component_is_rejected(tmp_path, parameters):
    s = session(tmp_path)
    with pytest.raises((ValueError, CopperScriptError)):
        op(s, "prepare_mechanical", kind="hole", name="H2", shape="", parameters=parameters, remove=False)
    assert s.source.read_bytes() == SOURCE.encode()


def test_generic_span_index_nested_arrays_strings_and_trivia():
    raw = SOURCE.replace("diameter = 1mm", "diameter = 1 // Comment\nmm").encode()
    snapshot = SourceSnapshot(raw)
    index = declaration_index(snapshot)
    block = next(n for n in index.children if n.header == ("mechanical",))
    assert [n.header for n in block.children] == [("outline", "rectangle"), ("hole", "H1")]
    candidate = mechanical_patch(snapshot, kind="hole", name="H1", parameters={
        "position": "(4mm,4mm)", "diameter": "2mm"}).apply(snapshot)
    assert b"// Comment\n" in candidate
    assert b"// Preserve" in candidate


def test_hierarchical_target_maps_to_board_owned_constraint_without_import_writes(tmp_path):
    package = tmp_path / "components"
    package.mkdir()
    imported = package / "module.copper"
    imported.write_text('module Passive { use library "standard"; component R1: RESISTOR; }')
    raw = SOURCE.replace("component R1: RESISTOR;", 'import p "./components"; module M: p.Passive;').replace(
        "R1.1;", "").replace("R1.2;", "").replace("placement(R1)", "placement(M/R1)").replace(
        "orientations(R1)", "orientations(M/R1)").encode()
    before = imported.read_bytes()
    s = session(tmp_path, raw)
    op(s, "prepare_lock", reference="M/R1", x_nm=9000000, y_nm=8000000,
       rotation=90, side="front", locks=["position", "rotation"])
    save(s)
    assert b"fixed_placement(M/R1)" in s.source.read_bytes()
    assert imported.read_bytes() == before
    assert next(p for p in s.state.board.placements if p.reference == "M/R1").position == Point.mm(9, 8)


def test_hierarchy_inventory_is_required_and_unknown_nested_targets_rejected(tmp_path):
    snapshot = SourceSnapshot(b'board B { import p "./p"; module M: p.Passive; }')
    with pytest.raises(SourceEditError, match="direct component"):
        fixed_placement_patch(snapshot, "M/R1", rotation=90)
    candidate = fixed_placement_patch(snapshot, "M/R1", rotation=90,
                                     resolved_references=frozenset({"M/R1"})).apply(snapshot)
    assert b"fixed_placement(M/R1)" in candidate
    with pytest.raises(SourceEditError):
        fixed_placement_patch(snapshot, "M/R2", rotation=90, resolved_references=frozenset({"M/R1"}))


@pytest.mark.parametrize("parameters", [{"width": "40mm; } hole BAD { diameter = 1mm"},
                                       {"width; height": "40mm"}])
def test_mechanical_literals_cannot_inject_other_declarations(parameters):
    with pytest.raises((SourceEditError, CopperScriptError)):
        mechanical_patch(SourceSnapshot(SOURCE.encode()), kind="outline", shape="rectangle", parameters=parameters)


def test_invalid_external_source_still_has_a_reloadable_scene(tmp_path):
    s = session(tmp_path)
    s.source.write_text("unfinished board {")
    assert s.scene()["source_stale"]
    assert len(s.scene()["components"]) == 2


def test_pad_only_keepout_is_checked_before_save(tmp_path):
    s = session(tmp_path)
    with pytest.raises(SourceEditError, match="copper keepout"):
        op(s, "prepare_mechanical", kind="copper_keepout", name="ANTENNA", shape="rectangle",
           parameters={"origin": "(5mm,5mm)", "width": "6mm", "height": "6mm", "layers": '"F.Cu"',
                       "block_pads": "true", "block_footprints": "false"}, remove=False)
    assert s.source.read_bytes() == SOURCE.encode()


def test_auto_placement_and_save_do_not_freeze_other_components(tmp_path):
    raw = SOURCE.replace('constraint fixed_placement(R2) { x = 20mm; y = 10mm; rotation = 0; side = front; }', '')
    s = session(tmp_path, raw.encode())
    op(s, "auto_place"); op(s, "apply")
    # A rotation-only edit at the existing anchor can keep all temporary seeds.
    op(s, "prepare_lock", reference="R1", x_nm=8000000, y_nm=8000000,
       rotation=0, side="back", locks=["position", "rotation", "side"])
    save(s)
    assert b"fixed_placement(R2)" not in s.source.read_bytes()
    assert not s.state.locks


def test_native_kicad_roundtrip_of_saved_mechanics(tmp_path):
    import json
    import os
    import subprocess
    executable = Path(os.environ.get("KICAD_PYTHON", "C:/Program Files/KiCad/10.0/bin/python.exe"))
    if not executable.exists():
        pytest.skip("KiCad Python needed for native outline/drill regression")
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    s = session(tmp_path)
    op(s, "prepare_mechanical", kind="outline", name="", shape="circle",
       parameters={"diameter": "60mm", "center": "(20mm,15mm)"}, remove=False)
    save(s)
    op(s, "prepare_mechanical", kind="hole", name="H1", shape="",
       parameters={"position": "(3mm,3mm)", "diameter": "2.4mm"}, remove=False)
    save(s)
    pcb = tmp_path / "saved.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(s.state.board), pcb)
    code = """import pcbnew, json, sys
b=pcbnew.LoadBoard(sys.argv[1])
print(json.dumps({'circles': sum(d.GetShape()==pcbnew.SHAPE_T_CIRCLE for d in b.GetDrawings() if d.GetLayer()==pcbnew.Edge_Cuts),
 'npth': sorted(p.GetDrillSize().x for f in b.GetFootprints() for p in f.Pads() if p.GetAttribute()==pcbnew.PAD_ATTRIB_NPTH)}))
"""
    result = subprocess.run([str(executable), "-c", code, str(pcb)], check=True, capture_output=True, text=True)
    assert json.loads(result.stdout) == {"circles": 1, "npth": [2400000]}

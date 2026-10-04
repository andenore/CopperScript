from dataclasses import replace
from hashlib import sha256
import json
from pathlib import Path
import shutil
import subprocess
from types import SimpleNamespace

import pytest

from pcbir.editor.overlay import (RoutedOverlay, _digest, electrical_digest, export_overlay,
                                  pose_records, projection_digest, write_intent)
from pcbir.editor.session import EditorSession
from pcbir.editor.scene import net_costs, ratsnest
from pcbir.physical import (CopperLayer, CopperZone, MechanicalHole, Point, PolygonRing,
                            PolygonWithHoles, TrackSegment)
from test_mechanical_editor import board_fixture, request


def fixture(tmp_path, *, verified=True, opens=None):
    source = tmp_path / "source.copper"
    source.write_text("// Physical overlay fixture; never executed.\n")
    original = board_fixture()
    routed = replace(original, tracks=(TrackSegment("SIGNAL", original.placements[0].position,
                               original.placements[1].position, 200000, CopperLayer.FRONT),))
    intent_path = tmp_path / "board.editor-intent.json"
    write_intent(source, original, routed, intent_path)
    intent = json.loads(intent_path.read_text())
    pcb = tmp_path / "board.kicad_pcb"
    pcb.write_text("native fixture bytes")
    native = {"kicad_version": "test", "layers": ["F.Cu", "B.Cu"], "poses": pose_records(routed),
              "tracks": intent["tracks"], "vias": [],
              "fills": [{"net": "SIGNAL", "layer": "B.Cu", "outer": [[0, 0], [100, 0], [100, 100]],
                         "holes": [[[20, 20], [30, 20], [20, 30]]]}]}
    for pose in native["poses"]:
        pose["reference"] = intent["native_references"][pose["reference"]]
    report = tmp_path / "kicad-drc.json"
    report.write_text(json.dumps({"coordinate_units": "mm", "violations": [], "unconnected_items": opens or []}))
    run = {"editor_intent_sha256": sha256(intent_path.read_bytes()).hexdigest(),
           "draft_board_sha256": sha256(pcb.read_bytes()).hexdigest()}
    if verified:
        run.update(filled_board_sha256=sha256(pcb.read_bytes()).hexdigest(),
                   native_drc_sha256=sha256(report.read_bytes()).hexdigest())
    run_path = tmp_path / "run.json"
    run_path.write_text(json.dumps(run))
    output = tmp_path / "editor-overlay.json"
    calls = []
    def runner(command, **options):
        calls.append((command, options))
        return SimpleNamespace(returncode=0, stdout=json.dumps(native), stderr="")
    export_overlay(run_path, "explicit-kicad-python", output, runner=runner)
    return original, routed, source, output, native, calls


def test_export_is_content_bound_and_never_runs_manifest_commands(tmp_path):
    original, routed, source, output, native, calls = fixture(tmp_path)
    overlay = RoutedOverlay.load(output)
    assert calls[0][0][0] == "explicit-kicad-python"
    assert calls[0][0][1].endswith("native_overlay.py")
    assert calls[0][1]["timeout"] == 60
    assert overlay.data["native"]["fills"][0]["holes"]
    assert overlay.fresh(original, sha256(source.read_bytes()).hexdigest())
    assert len(ratsnest(overlay.copper_board(original))) == 1
    with pytest.raises(FileExistsError):
        export_overlay(tmp_path / "run.json", "python", output,
                       runner=lambda *a, **k: SimpleNamespace(returncode=0, stdout=json.dumps(native), stderr=""))


def test_placement_seed_stale_display_no_false_connectivity_after_edit(tmp_path):
    board, _, source, output, _, _ = fixture(tmp_path)
    overlay = RoutedOverlay.load(output)
    session = EditorSession(board, source, overlay=overlay)
    initial = session.scene()
    assert initial["ratsnest"] == []
    assert initial["net_costs"] == {}
    assert initial["connectivity_basis"] == "native saved-board DRC"
    moved = request(session, "move", reference="TP1", x_nm=10000000, y_nm=12000000,
                    rotation="0", side="front")["preview"]
    assert moved["routed_overlay"]["stale"]
    assert not moved["routed_overlay"]["connectivity_credit"]
    assert len(moved["ratsnest"]) == 2
    assert not session.state.board.tracks
    request(session, "discard")
    assert not session.scene()["routed_overlay"]["stale"]


def test_unverified_fill_never_closes_airwires(tmp_path):
    board, _, source, output, _, _ = fixture(tmp_path, verified=False)
    scene = EditorSession(board, source, overlay=RoutedOverlay.load(output)).scene()
    assert not scene["routed_overlay"]["connectivity_credit"]
    assert len(scene["ratsnest"]) == 1
    assert scene["net_costs"]["SIGNAL"]["airwires"] == 1


def test_external_source_edit_cannot_retain_native_connectivity_credit(tmp_path):
    board, _, source, output, _, _ = fixture(tmp_path)
    session = EditorSession(board, source, overlay=RoutedOverlay.load(output))
    source.write_text("// External edit")
    scene = session.scene()
    assert scene["source_stale"]
    assert scene["routed_overlay"]["stale"]
    assert not scene["routed_overlay"]["connectivity_credit"]
    assert len(scene["ratsnest"]) == 2


def test_native_remaining_connections_are_retained(tmp_path):
    opens = [{"description": "Unconnected", "items": [{"pos": {"x": 9, "y": 12}}]}]
    board, _, source, output, _, _ = fixture(tmp_path, opens=opens)
    scene = EditorSession(board, source, overlay=RoutedOverlay.load(output)).scene()
    assert scene["routed_overlay"]["evidence"]["remaining"] == opens
    assert len(scene["ratsnest"]) == 1


@pytest.mark.parametrize("changed", ["source", "geometry", "footprint", "rules", "electrical"])
def test_input_change_marks_overlay_stale(tmp_path, changed):
    board, _, source, output, _, _ = fixture(tmp_path)
    overlay = RoutedOverlay.load(output)
    revision = sha256(source.read_bytes()).hexdigest()
    identity = None
    if changed == "source":
        revision = "0" * 64
    elif changed == "geometry":
        board = replace(board, mechanical_holes=(MechanicalHole("H1", Point.mm(2, 2), 1000000),))
    elif changed == "footprint":
        fp = board.footprints["terminal"]
        board = replace(board, footprints={"terminal": replace(fp, pads=(replace(fp.pads[0], position=Point.mm(.1, 0)),))})
    elif changed == "rules":
        board = replace(board, rules=replace(board.rules, minimum_clearance_nm=300000))
    else:
        identity = "f" * 64
    assert not overlay.fresh(board, revision, identity)
    assert not overlay.scene(board, revision, identity)["connectivity_credit"]


@pytest.mark.parametrize("changed", ["pcb", "intent", "overlay", "drc"])
def test_modified_artifacts_are_rejected(tmp_path, changed):
    _, _, _, output, native, _ = fixture(tmp_path)
    target = {"pcb": "board.kicad_pcb", "intent": "board.editor-intent.json",
              "overlay": output.name, "drc": "kicad-drc.json"}[changed]
    path = tmp_path / target
    path.write_bytes(path.read_bytes() + b" ")
    if changed == "intent":
        with pytest.raises(ValueError, match="intent"):
            export_overlay(tmp_path / "run.json", "python", tmp_path / "new.json")
    else:
        with pytest.raises(ValueError):
            RoutedOverlay.load(output)


def test_native_geometry_mismatch_rejected_and_no_output_overwrite(tmp_path):
    _, _, _, output, native, _ = fixture(tmp_path)
    native["tracks"][0]["width_nm"] += 1000
    with pytest.raises(ValueError, match="copper"):
        export_overlay(tmp_path / "run.json", "python", tmp_path / "new.json",
                       runner=lambda *a, **k: SimpleNamespace(returncode=0, stdout=json.dumps(native), stderr=""))
    assert not (tmp_path / "new.json").exists()
    with pytest.raises(ValueError, match="beside"):
        export_overlay(tmp_path / "run.json", "python", tmp_path / "elsewhere/new.json")


@pytest.mark.parametrize("value", [True, 1.5, 10**13])
def test_malformed_overlay_coordinates_rejected(tmp_path, value):
    _, _, _, output, _, _ = fixture(tmp_path)
    data = json.loads(output.read_text())
    data["native"]["fills"][0]["outer"][0][0] = value
    data["content_digest"] = _digest({k: v for k, v in data.items() if k != "content_digest"})
    with pytest.raises(ValueError, match="integer"):
        RoutedOverlay.from_data(data)


def test_source_changed_during_routing_cannot_publish_intent(tmp_path):
    board = board_fixture()
    source = tmp_path / "source.copper"
    source.write_text("source")
    with pytest.raises(ValueError, match="during routing"):
        write_intent(source, board, board, tmp_path / "intent.json", source_revision="0" * 64)
    assert not (tmp_path / "intent.json").exists()


def test_native_filled_polygon_extraction(tmp_path):
    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    python = cli.with_name("python.exe") if cli.suffix == ".exe" else Path("/usr/bin/python3")
    if not cli.is_file() or not python.is_file():
        pytest.skip("native KiCad CLI/Python not installed")
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    board = board_fixture()
    zone = CopperZone("SIGNAL_PLANE", "SIGNAL", (CopperLayer.FRONT,),
                      PolygonWithHoles(PolygonRing(board.outline.vertices)))
    board = replace(board, zones=(zone,), mechanical_holes=(MechanicalHole("H1", Point.mm(16, 12), 2000000),))
    source = tmp_path / "source.copper"
    source.write_text("// Native integration fixture")
    write_intent(source, board, board, tmp_path / "board.editor-intent.json")
    pcb, drc = tmp_path / "board.kicad_pcb", tmp_path / "kicad-drc.json"
    write_kicad_project(KiCadPcbBackend().generate(board), pcb)
    result = subprocess.run([str(cli), "pcb", "drc", "--refill-zones", "--save-board", "--format", "json",
                             "--output", str(drc), str(pcb)], capture_output=True, timeout=60)
    assert result.returncode == 0, result.stderr
    run = {"editor_intent_sha256": sha256((tmp_path / "board.editor-intent.json").read_bytes()).hexdigest(),
           "filled_board_sha256": sha256(pcb.read_bytes()).hexdigest(), "native_drc_sha256": sha256(drc.read_bytes()).hexdigest()}
    (tmp_path / "run.json").write_text(json.dumps(run))
    output = export_overlay(tmp_path / "run.json", python, tmp_path / "native-overlay.json")
    overlay = RoutedOverlay.load(output)
    assert overlay.data["native"]["fills"]
    assert overlay.data["evidence"]["verified"]
    # KiCad may fracture holes into an outer ring with a zero-width slit rather
    # than separate interior rings. Either representation must keep the NPTH dry.
    from pcbir.geometry import point_in_polygon
    assert not any(point_in_polygon(Point.mm(16, 12), tuple(Point(*p) for p in f["outer"]))
                   and not any(point_in_polygon(Point.mm(16, 12), tuple(Point(*p) for p in h)) for h in f["holes"])
                   for f in overlay.data["native"]["fills"])
    assert not overlay.data["evidence"]["remaining"]

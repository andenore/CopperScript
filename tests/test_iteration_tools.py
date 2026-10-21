"""Scene readers, state files, grouped run issues and window views."""

import json
import subprocess
import sys
from dataclasses import replace
from decimal import Decimal
from pathlib import Path

import pytest

from pcbir.backends import KiCadPcbBackend
from pcbir.loader import load_design
from pcbir.physical import BoardSide
from pcbir.physicalize import PrototypePhysicalOptions, prototype_physicalize
from pcbir.run_issues import filter_issues, find_issue, issues_document, load_run, render_issues, run_issues
from pcbir.scene import Scene, ScenePad, scene_from_kicad_pcb, scene_from_physical
from pcbir.state_codec import StateError, describe_state, load_state, save_state
from pcbir.view import ViewRequest, align_reference, layers_in_window, render, window_around

ROOT = Path(__file__).parents[1]


@pytest.fixture(scope="module")
def board():
    design = load_design(ROOT / "examples" / "valid_board" / "board.copper")
    physical = prototype_physicalize(design, PrototypePhysicalOptions())
    rotations, sides = (0, 90, 180, 270, 45), (BoardSide.FRONT, BoardSide.BACK)
    return replace(physical, placements=tuple(
        replace(p, rotation_degrees=Decimal(rotations[i % 5]), side=sides[(i // 2) % 2])
        for i, p in enumerate(physical.placements)))


@pytest.fixture(scope="module")
def run_dir(board, tmp_path_factory):
    directory = tmp_path_factory.mktemp("run")
    pcb = next(a for a in KiCadPcbBackend().generate(board).artifacts if a.name.endswith(".kicad_pcb"))
    (directory / "board.kicad_pcb").write_text(pcb.content if isinstance(pcb.content, str) else pcb.content.decode())
    scene = scene_from_physical(board)
    first, second = scene.pads[0], scene.pads[1]
    nm = lambda p: [round(p[0] * 1e6), round(p[1] * 1e6)]
    report = {
        "schema": "copperscript-route-board/v0.1", "status": "fail", "routing_complete": False,
        "package_access": {"pending_pads": [f"{first.ref}.{first.pad}"], "failed_critical_nets": []},
        "plane_verification": {"native_findings": [
            {"type": "unconnected_items", "description": "Missing connection between items", "items": [
                {"kind": "pad", "component": first.ref, "pad": first.pad, "net": "N1", "layers": ["F.Cu"],
                 "positions_nm": [nm(first.center)]},
                {"kind": "pad", "component": second.ref, "pad": second.pad, "net": "N1", "layers": ["F.Cu"],
                 "positions_nm": [nm(second.center)]}]},
            {"type": "via_dangling", "description": "Via is not connected", "items": [
                {"kind": "via", "net": "N2", "layers": ["F.Cu", "B.Cu"], "positions_nm": [[5_000_000, 5_000_000]]}]},
        ]},
        "drc": {"findings": [{"code": "DRC-OPEN-NET", "severity": "error", "nets": ["N1"], "objects": [],
                              "message": "net 'N1' is not connected"}]},
        "detailed": {"nets": [{"net": n, "connected": False,
                               "diagnostics": ["area routing blocked: package-access preflight incomplete"]}
                              for n in ("N1", "N2", "N3")]},
    }
    (directory / "route-report.json").write_text(json.dumps(report))
    return directory


def test_kicad_and_physical_scenes_agree_on_rotated_and_back_side_pads(board, run_dir):
    physical = scene_from_physical(board)
    native = scene_from_kicad_pcb(run_dir / "board.kicad_pcb")
    by_key = {(p.ref, p.pad): p for p in native.pads}
    assert len(by_key) == len(physical.pads) > 0
    for pad in physical.pads:
        other = by_key[(pad.ref, pad.pad)]
        assert abs(pad.center[0] - other.center[0]) < 1e-5 and abs(pad.center[1] - other.center[1]) < 1e-5
        assert pad.layers == other.layers
        box = lambda poly: (min(x for x, _ in poly), min(y for _, y in poly), max(x for x, _ in poly),
                            max(y for _, y in poly))
        assert max(abs(a - b) for a, b in zip(box(pad.polygon), box(other.polygon))) < 0.02
    assert {f.side for f in native.footprints.values()} == {"front", "back"}


def test_state_files_round_trip_and_refuse_foreign_types(board, tmp_path):
    path = save_state(tmp_path / "board.state.json", board, note="trial 1")
    assert load_state(path) == board
    assert describe_state(path)["metadata"] == {"note": "trial 1"}
    document = json.loads(path.read_text())
    document["value"] = {"$type": "os:system", "fields": {}}
    path.write_text(json.dumps(document))
    with pytest.raises(StateError):
        load_state(path)  # digest mismatch is caught first
    from pcbir.state_codec import decode
    with pytest.raises(StateError, match="only pcbir types"):
        decode({"$type": "os:system", "fields": {}})


def test_run_issues_group_locate_and_link_causes(run_dir):
    run = load_run(run_dir)
    issues = run_issues(run)
    codes = [i.code for i in issues]
    assert codes == ["PENDING-PACKAGE-EXIT", "UNCONNECTED_ITEMS", "VIA_DANGLING", "DRC-OPEN-NET", "NET-UNROUTED"]
    pending = find_issue(issues, "p1")
    assert pending.window() is not None and pending.objects
    routing = find_issue(issues, "R1")
    assert routing.count == 3 and routing.caused_by == "P1"
    assert [i.id for i in filter_issues(issues, net="N2")] == ["N2", "R1"]
    text = render_issues(issues_document(run, issues))
    assert "findings: PENDING-PACKAGE-EXIT 1" in text and "<- P1" in text


def test_balanced_limit_keeps_every_stage_visible(run_dir):
    run = load_run(run_dir)
    document = issues_document(run, run_issues(run), limit=3)
    assert {item["id"][0] for item in document["issues"]} == {"P", "N", "O"} and document["omitted"] == 2


def test_view_renders_png_and_legend(run_dir, tmp_path):
    pytest.importorskip("PIL")
    run = load_run(run_dir)
    scene = run.scene()
    issue = find_issue(run_issues(run), "N1")
    window = window_around(issue.points)
    request = ViewRequest(window, layers_in_window(scene, window, issue.nets), tuple(issue.nets), issue.points, "N1")
    path, lines = render(scene, request, tmp_path / "n1.png")
    from PIL import Image
    with Image.open(path) as image:
        assert image.width > 300 and image.height > 300
    assert lines[0].startswith("window x ") and any(line.startswith("markers") for line in lines)


def test_reference_alignment_recovers_rotation_and_mirroring():
    def scene_with(pads, side_mirror=False, angle=0.0, offset=(0.0, 0.0)):
        from math import cos, radians, sin
        a = radians(angle)
        result = Scene("s", ("F.Cu", "In1.Cu", "B.Cu"))
        for number, (x, y) in pads.items():
            if side_mirror:
                x = -x
            q = (cos(a) * x - sin(a) * y + offset[0], sin(a) * x + cos(a) * y + offset[1])
            result.pads.append(ScenePad("U1", number, "", ("B.Cu" if side_mirror else "F.Cu",), (q,), q))
        return result
    layout = {"1": (0, 0), "2": (1, 0), "3": (1, 2), "4": (-1, 3)}
    ours = scene_with(layout)
    theirs = scene_with(layout, angle=90, offset=(10, 5))
    _, alignment = align_reference(theirs, "U1", ours, "U1")
    assert alignment.rms_mm < 1e-6 and not alignment.mirrored and alignment.angle_degrees in (270.0, -90.0 % 360)
    mirrored = scene_with(layout, side_mirror=True, angle=30, offset=(-4, 2))
    mapped, alignment = align_reference(mirrored, "U1", ours, "U1")
    assert alignment.mirrored and alignment.rms_mm < 1e-6
    assert {p.layers for p in mapped.pads} == {("F.Cu",)}  # back-side reference lands map to our top


def test_cli_issues_and_view(run_dir, tmp_path):
    pytest.importorskip("PIL")
    listed = subprocess.run([sys.executable, "-m", "copperscript", "issues", str(run_dir), "--json"],
                            text=True, capture_output=True, check=True)
    document = json.loads(listed.stdout)
    assert document["schema"] == "copperscript.run-issues/v1" and document["issue_count"] == 5
    import hashlib
    assert document["board_sha256"] == hashlib.sha256((run_dir / "board.kicad_pcb").read_bytes()).hexdigest()
    output = tmp_path / "p1.png"
    viewed = subprocess.run([sys.executable, "-m", "copperscript", "view", str(run_dir), "--issue", "P1",
                             "-o", str(output)], text=True, capture_output=True)
    assert viewed.returncode == 0, viewed.stdout + viewed.stderr
    assert output.is_file() and "markers (red rings): 1" in viewed.stdout
    missing = subprocess.run([sys.executable, "-m", "copperscript", "view", str(run_dir), "--issue", "Z9"],
                             text=True, capture_output=True)
    assert missing.returncode == 2 and "known: P1" in missing.stdout


def test_state_decoding_cannot_reach_imported_callables():
    from pcbir.state_codec import decode
    payloads = [
        {"$enum": "pcbir.build:subprocess.Popen", "value": ["true"]},
        {"$type": "pcbir.build:subprocess.Popen", "fields": {}},
        {"$type": "pcbir.physical:Decimal", "fields": {}},
        {"$enum": "pcbir.physical:Point", "value": 1},
        {"$type": "pcbir.missing_module:X", "fields": {}},
        {"$type": "pcbir.physical:Point", "fields": {"x_nm": 1, "y_nm": 2, "__class__": 3}},
    ]
    for payload in payloads:
        with pytest.raises(StateError):
            decode(payload)


def test_large_metadata_state_is_still_recognised(board, tmp_path):
    from pcbir.state_codec import is_state_file
    path = save_state(tmp_path / "big.state.json", board, note="x" * 20000)
    assert is_state_file(path) and load_state(path) == board
    assert not is_state_file(ROOT / "pyproject.toml")


def test_layout_reports_and_malformed_sections(tmp_path, run_dir):
    layout = tmp_path / "layout"
    layout.mkdir()
    (layout / "board.kicad_pcb").write_text((run_dir / "board.kicad_pcb").read_text())
    (layout / "layout-report.json").write_text(json.dumps({
        "schema": "copperscript-layout-report/v0.1",
        "gates": [{"stage": "placement", "status": "fail", "summary": "2 overlaps"}],
        "findings": [{"code": "PLACEMENT-OVERLAP", "stage": "placement", "status": "fail", "message": "overlap"},
                     {"code": "OK", "stage": "placement", "status": "pass", "message": "fine"}]}))
    run = load_run(layout)
    document = issues_document(run, run_issues(run))
    assert [i["id"] for i in document["issues"]] == ["L1", "F1"] and document["status"] == "layout-fail"
    odd = tmp_path / "odd"
    odd.mkdir()
    (odd / "route-report.json").write_text(json.dumps({"schema": "copperscript-route-board/v0.1", "drc": [],
                                                       "detailed": {"nets": [{"net": "X"}]}}))
    assert run_issues(load_run(odd)) == []


def test_arcs_follow_the_circle_and_pad_angles_default_to_zero():
    from pcbir.scene import arc_points
    points = arc_points((1.0, 0.0), (0.0, 1.0), (-1.0, 0.0))
    assert len(points) > 5 and all(abs((x * x + y * y) ** 0.5 - 1) < 1e-9 for x, y in points)
    assert min(y for _, y in points) > -1e-9  # passes through the mid point's side
    assert arc_points((0, 0), (1, 1), (2, 2)) == [(0, 0), (2, 2)]


def test_pad_without_angle_is_absolute_zero(tmp_path):
    pcb = tmp_path / "rot.kicad_pcb"
    pcb.write_text('''(kicad_pcb (version 20240108) (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
  (net 0 "") (net 1 "A")
  (footprint "X" (layer "F.Cu") (at 10 10 90) (property "Reference" "U1")
    (pad "1" smd rect (at 1 0) (size 2 0.5) (layers "F.Cu") (net 1 "A"))))''')
    pad = scene_from_kicad_pcb(pcb).pads[0]
    xs = [x for x, _ in pad.polygon]
    ys = [y for _, y in pad.polygon]
    assert abs(pad.center[0] - 10) < 1e-9 and abs(pad.center[1] - 9) < 1e-9  # offset rotates with the part
    assert max(xs) - min(xs) > max(ys) - min(ys)  # absolute angle 0: still horizontal


def test_route_report_placement_findings_become_located_warnings(tmp_path, run_dir):
    run_copy = tmp_path / "placed"
    run_copy.mkdir()
    (run_copy / "board.kicad_pcb").write_text((run_dir / "board.kicad_pcb").read_text())
    report = json.loads((run_dir / "route-report.json").read_text())
    ref = load_run(run_dir).scene().pads[0].ref
    report["placement_findings"] = [
        {"code": "REMOTE_FIXED_PART", "status": "warn", "message": "fixed 20 mm from its partner", "subject": ref},
        {"code": "AUTO_PAIR_CORRIDORS", "status": "pass", "message": "2 corridors"}]
    (run_copy / "route-report.json").write_text(json.dumps(report))
    issues = run_issues(load_run(run_copy))
    warning = find_issue(issues, "W1")
    assert warning.stage == "placement" and warning.code == "REMOTE_FIXED_PART"
    assert warning.objects == [ref] and warning.window() is not None
    assert [i.id for i in issues if i.stage == "placement"] == ["W1"]


def test_estimated_plane_opens_are_located_when_no_native_check_ran(tmp_path, run_dir):
    run_copy = tmp_path / "estimated"
    run_copy.mkdir()
    (run_copy / "board.kicad_pcb").write_text((run_dir / "board.kicad_pcb").read_text())
    report = json.loads((run_dir / "route-report.json").read_text())
    pad = load_run(run_dir).scene().pads[0]
    report["plane_verification"] = {}
    report["summary"] = {"plane": {"estimated_open_pads": {"GND": [f"{pad.ref}.{pad.pad}"]}}}
    (run_copy / "route-report.json").write_text(json.dumps(report))
    issue = find_issue(run_issues(load_run(run_copy)), "E1")
    assert issue.code == "PLANE-ESTIMATED-OPEN" and issue.nets == ["GND"] and issue.window() is not None
    # A native fill check supersedes the estimate.
    assert all(i.stage != "plane" for i in run_issues(load_run(run_dir)))

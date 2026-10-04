from dataclasses import replace
from hashlib import sha256
from http.client import HTTPConnection
from importlib.resources import files
import json
from pathlib import Path
from threading import Thread

import pytest

from pcbir.cli import main
from pcbir.editor.scene import board_scene, ratsnest
from pcbir.editor.server import create_server
from pcbir.editor.session import EditorError, EditorSession, StaleRevision
from pcbir.physical import (BoardOutline, BoardSide, ComponentPlacementRule,
    CopperLayer, CopperZone, FootprintPad, InternalPadGroup, MechanicalHole,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point,
    PolygonRing, PolygonWithHoles, Size, TrackSegment)
from pcbir.placement import PlacementPlannerOptions, transformed_local_point


def board_fixture():
    footprint = PhysicalFootprint("terminal", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    return PhysicalBoard("EditorTest", BoardOutline.rectangle(24, 18), {"terminal": footprint},
        tuple(Placement(ref, "terminal", Point.mm(x, y)) for ref, x, y in
              (("J1", 5, 5), ("U/R1", 12, 5), ("TP1", 9, 12))),
        (PhysicalNet("SIGNAL", tuple(PadReference(ref, "1") for ref in ("J1", "U/R1", "TP1"))),),
        placement_rules=(ComponentPlacementRule("J1", fixed_position=Point.mm(5, 5)),))


def session_fixture(tmp_path, board=None):
    source = tmp_path / "board.copper"
    source.write_bytes(b"// Pure physical-IR test; editor never writes source.\r\n")
    return EditorSession(board or board_fixture(), source,
        PlacementPlannerOptions(candidate_count=1, analytical_iterations=3, refinement_passes=0))


def request(session, action, **fields):
    return session.operation({"action": action, "revision": session.revision, **fields})


def test_scene_uses_physical_geometry_and_explicit_scope():
    board = board_fixture()
    board = replace(board, outline=BoardOutline.circle(30), mechanical_holes=(
        MechanicalHole("H1", Point.mm(15, 15), 1000000, 2000000),))
    scene = board_scene(board, source_revision="f" * 64)
    assert scene["outline"]["circle"] == {"center": [15000000, 15000000], "radius_nm": 15000000}
    assert scene["holes"][0]["diameter_nm"] == 1000000
    assert scene["source_writable"] is False
    assert not scene["capabilities"]["mechanical_edit"]
    assert scene["components"][0]["source_position_locked"]
    assert next(c for c in scene["components"] if c["reference"] == "U/R1")["hierarchy"] == ["U"]
    json.dumps(scene, allow_nan=False)


@pytest.mark.parametrize("side", [BoardSide.FRONT, BoardSide.BACK])
@pytest.mark.parametrize("rotation", [0, 45, 90])
def test_scene_pad_transform_matches_shared_geometry(side, rotation):
    board = board_fixture()
    fp = replace(board.footprints["terminal"], pads=(FootprintPad("1", Point.mm(2, 1), Size.mm(.5, .5)),))
    pose = replace(board.placements[1], rotation_degrees=rotation, side=side)
    board = replace(board, footprints={"terminal": fp}, placements=(board.placements[0], pose, board.placements[2]))
    component = next(c for c in board_scene(board, source_revision="x")["components"] if c["reference"] == pose.reference)
    expected = transformed_local_point(pose, fp.pads[0].position)
    xs, ys = zip(*component["pads"][0]["shape"]["spine"])
    assert abs(sum(xs) // len(xs) - expected.x_nm) <= 1
    assert abs(sum(ys) // len(ys) - expected.y_nm) <= 1


def test_ratsnest_deterministic_short_tree_and_copper_reuse():
    board = board_fixture()
    wires = ratsnest(board)
    assert len(wires) == 2
    assert wires == ratsnest(replace(board, placements=tuple(reversed(board.placements))))
    assert len({(w["from"]["land"], w["to"]["land"]) for w in wires}) == 2
    a, b = board.placements[:2]
    joined = replace(board, tracks=(TrackSegment("SIGNAL", a.position, b.position, 200000, CopperLayer.FRONT),))
    assert len(ratsnest(joined)) == 1
    zone = CopperZone("z", "SIGNAL", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(board.outline.vertices)))
    assert ratsnest(replace(board, zones=(zone,))) == wires


def test_repeated_numbers_require_explicit_internal_connection_and_nearest_land():
    board = board_fixture()
    bridge = replace(board.footprints["terminal"], pads=(
        FootprintPad("1", Point.mm(-2, 0), Size.mm(.5, .5)),
        FootprintPad("1", Point.mm(2, 0), Size.mm(.5, .5))), body_size=Size.mm(5, 1))
    board = replace(board, footprints={"terminal": bridge}, placements=(board.placements[0],),
        nets=(PhysicalNet("SIGNAL", (PadReference("J1", "1"),)),))
    wires = ratsnest(board)
    assert len(wires) == 1  # Same number, physically disconnected lands.
    connected = replace(board, footprints={"terminal": replace(bridge,
        internal_pad_groups=(InternalPadGroup(("1",)),))})
    assert not ratsnest(connected)
    terminal = PhysicalFootprint("single", (FootprintPad("1", Point(0, 0), Size.mm(.5, .5)),), Size.mm(1, 1))
    connected = replace(connected, footprints={**connected.footprints, "single": terminal},
        placements=(*connected.placements, Placement("TP", "single", Point.mm(12, 5))),
        nets=(PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("TP", "1"))),))
    wire = ratsnest(connected)[0]
    assert [7000000, 5000000] in (wire["from"]["position"], wire["to"]["position"])


def test_macro_copper_removes_airwires_and_pose_moves_whole_unit(tmp_path):
    from test_hard_macros import fixture
    original, _, bind = fixture(tmp_path)
    board = bind()
    assert ratsnest(board) == []
    session = session_fixture(tmp_path, board)
    result = request(session, "move", reference="U1", x_nm=11000000, y_nm=10000000, rotation="0", side="front")
    positions = {c["reference"]: c["position"] for c in result["preview"]["components"]}
    assert positions == {"R1": [13000000, 10000000], "U1": [11000000, 10000000]}
    assert not session.state.board.tracks
    assert not result["preview"]["ratsnest"]
    request(session, "apply")
    request(session, "lock", reference="U1", locked=True)
    assert session.state.locks == frozenset(("U1", "R1"))


def test_unplaced_macro_is_visible_without_false_private_copper_credit(tmp_path):
    from test_hard_macros import fixture
    _, _, bind = fixture(tmp_path)
    board = bind()
    board = replace(board, placements=(board.placements[0], replace(board.placements[1], position=Point.mm(15, 10))))
    scene = board_scene(board, source_revision="x")
    assert len(scene["components"]) == 2
    assert scene["warnings"]["macro_copper_not_materialized"]
    assert len(scene["ratsnest"]) == 1


def test_auto_placement_is_preview_preserves_locks_and_does_not_freeze_all(tmp_path):
    session = session_fixture(tmp_path)
    before = session.source.read_bytes()
    request(session, "lock", reference="TP1", locked=True)
    initial = session.state
    result = request(session, "auto_place")
    assert session.state is initial
    assert result["preview"]["placement_legal"]
    assert session.pending.locks == frozenset(("TP1",))
    for ref in ("J1", "TP1"):
        assert next(p for p in session.pending.board.placements if p.reference == ref) == next(p for p in initial.board.placements if p.reference == ref)
    request(session, "apply")
    assert session.state.locks == frozenset(("TP1",))
    assert session.source.read_bytes() == before
    assert session.revision == 3
    request(session, "undo")
    assert session.state == initial
    request(session, "redo")
    assert session.state.locks == frozenset(("TP1",))


def test_manual_move_preview_apply_discard_history_and_failed_rollback(tmp_path):
    session = session_fixture(tmp_path)
    original = session.state
    result = request(session, "move", reference="U/R1", x_nm=14000000, y_nm=5000000, rotation="90", side="front")
    assert result["preview"]["placement_legal"]
    assert session.state is original
    request(session, "discard")
    assert session.pending is None and not session.undo_stack
    request(session, "move", reference="U/R1", x_nm=14000000, y_nm=5000000, rotation="90", side="front")
    request(session, "apply")
    changed = session.state
    with pytest.raises(EditorError, match="violates"):
        request(session, "move", reference="U/R1", x_nm=5000000, y_nm=5000000, rotation="0", side="front")
    assert session.state is changed and session.pending is None
    request(session, "undo")
    assert session.state == original
    request(session, "lock", reference="TP1", locked=True)
    assert not session.redo_stack


def test_preview_changes_revision_so_other_clients_cannot_apply_old_candidate(tmp_path):
    session = session_fixture(tmp_path)
    request(session, "move", reference="U/R1", x_nm=14000000, y_nm=5000000, rotation="0", side="front")
    preview_revision = session.revision
    request(session, "move", reference="U/R1", x_nm=16000000, y_nm=5000000, rotation="0", side="front")
    with pytest.raises(StaleRevision):
        session.operation({"action": "apply", "revision": preview_revision})
    assert session.state.board.placements[1].position == Point.mm(12, 5)
    request(session, "apply")
    assert session.state.board.placements[1].position == Point.mm(16, 5)


def test_fixed_source_and_session_lock_cannot_be_bypassed(tmp_path):
    session = session_fixture(tmp_path)
    with pytest.raises(EditorError, match="violates"):
        request(session, "move", reference="J1", x_nm=7000000, y_nm=5000000, rotation="0", side="front")
    request(session, "lock", reference="TP1", locked=True)
    with pytest.raises(EditorError, match="unlock"):
        request(session, "move", reference="TP1", x_nm=9000000, y_nm=14000000, rotation="0", side="front")
    request(session, "lock", reference="TP1", locked=False)
    assert not session.state.locks


def test_failed_auto_place_preserves_accepted_history(tmp_path):
    board = board_fixture()
    board = replace(board, placement_rules=(ComponentPlacementRule("J1", fixed_position=Point.mm(-2, 0)),))
    session = session_fixture(tmp_path, board)
    before = session.state
    with pytest.raises(ValueError):
        request(session, "auto_place")
    assert session.state is before and not session.undo_stack and session.pending is None


@pytest.mark.parametrize("change", ["session", "source"])
def test_stale_operations_rejected_without_mutation(tmp_path, change):
    session = session_fixture(tmp_path)
    revision = session.revision
    if change == "source":
        session.source.write_bytes(b"// externally modified\n")
    else:
        request(session, "lock", reference="TP1", locked=True)
    before = session.state
    with pytest.raises(StaleRevision):
        session.operation({"action": "auto_place", "revision": revision})
    assert session.state is before


@pytest.mark.parametrize("field,value", [("x_nm", True), ("y_nm", 1.5), ("rotation", "NaN"),
    ("rotation", "Infinity"), ("side", "other"), ("reference", "missing"), ("x_nm", 10**13)])
def test_malformed_pose_rejected(tmp_path, field, value):
    session = session_fixture(tmp_path)
    fields = dict(reference="U/R1", x_nm=14000000, y_nm=5000000, rotation="0", side="front")
    fields[field] = value
    with pytest.raises(EditorError):
        request(session, "move", **fields)
    assert session.revision == 0


@pytest.fixture
def http_editor(tmp_path):
    session = session_fixture(tmp_path)
    server = create_server(session)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield server, session
    server.shutdown()
    server.server_close()
    thread.join(timeout=3)


def http_request(server, method="GET", path="/api/scene", body=None, headers=None):
    connection = HTTPConnection("127.0.0.1", server.server_port, timeout=10)
    defaults = {"X-Copper-Token": server.editor_token}
    if body is not None:
        defaults["Content-Type"] = "application/json"
    defaults.update(headers or {})
    connection.request(method, path, body=body, headers=defaults)
    response = connection.getresponse()
    content = response.read()
    status = response.status
    connection.close()
    return status, content


def test_http_assets_and_authenticated_scene(http_editor):
    server, session = http_editor
    assert server.server_address[0] == "127.0.0.1"
    for path in ("/", "/editor.js", "/editor.css"):
        assert http_request(server, path=path)[0] == 200
    code, body = http_request(server)
    assert code == 200 and json.loads(body)["source_revision"] == sha256(session.source.read_bytes()).hexdigest()
    assert http_request(server, path="/../../board.copper")[0] == 404


@pytest.mark.parametrize("headers", [{"Host": "evil.example"}, {"Origin": "https://evil.example"},
                                    {"X-Copper-Token": "wrong"}, {"Origin": "null"}])
def test_http_rejects_cross_origin_host_and_bad_capability(http_editor, headers):
    assert http_request(http_editor[0], headers=headers)[0] == 403


@pytest.mark.parametrize("body", ['[]', '{"action":"lock","action":"undo"}',
    '{"action":"auto_place","revision":NaN}', '{"action":"save","revision":0}',
    '{"action":"auto_place","revision":0,"shell":"not executable"}', 'x' * 4097])
def test_http_rejects_malformed_unbounded_and_unknown_operations(http_editor, body):
    assert http_request(http_editor[0], "POST", "/api/operation", body)[0] == 400


def test_http_success_and_stale_revision(http_editor):
    server, session = http_editor
    body = json.dumps({"action": "lock", "revision": 0, "reference": "TP1", "locked": True})
    assert http_request(server, "POST", "/api/operation", body)[0] == 200
    assert http_request(server, "POST", "/api/operation", body)[0] == 409
    assert session.revision == 1


def test_cli_scene_export_is_deterministic_and_does_not_overwrite_source(tmp_path):
    source = Path("examples/mechanical_outline.copper")
    original = source.read_bytes()
    output = tmp_path / "scene.json"
    args = ["edit-mechanical", str(source), "--allow-proxy-footprints", "--scene-output", str(output)]
    assert main(args) == 0
    scene = json.loads(output.read_text())
    assert scene["outline"]["cutouts"][0]["id"] == "window"
    assert len(scene["holes"]) == 2
    assert main(args) == 2
    assert main([*args[:-1], str(source)]) == 2
    assert source.read_bytes() == original


def test_bundled_assets_have_no_external_script_dependencies():
    assets = files("pcbir.editor").joinpath("assets")
    for name in ("index.html", "editor.js", "editor.css"):
        text = assets.joinpath(name).read_text(encoding="utf-8")
        assert text.strip()
        assert "https://" not in text

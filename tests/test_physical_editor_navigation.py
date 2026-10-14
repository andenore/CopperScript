"""Real loopback transport for source-owned definition navigation."""
from http.client import HTTPConnection
import json
from threading import Thread
from urllib.parse import urlsplit, parse_qs

import pytest

from pcbir.editor.server import create_server
from test_macro_definitions import definition_project
from test_mechanical_profiles import PROFILE


class ParentSession:
    def __init__(self, source):
        self.source = source
        self.closed = False
    def scene(self):
        return {"source_features": [{"definition_path": str(self.source)}]}
    def close(self):
        self.closed = True


def request(port, token, method="GET", endpoint="/api/scene", body=None):
    connection = HTTPConnection("127.0.0.1", port, timeout=3)
    headers = {"X-Copper-Token": token}
    if body is not None:
        headers["Content-Type"] = "application/json"
        body = json.dumps(body)
    try:
        connection.request(method, endpoint, body=body, headers=headers)
        response = connection.getresponse()
        return response.status, json.loads(response.read())
    finally:
        connection.close()


@pytest.fixture
def navigation_server(tmp_path):
    source = tmp_path / "profile.copper"
    source.write_text(PROFILE, encoding="utf-8")
    (tmp_path / "copper.mod").write_text("module test/navigation\n", encoding="utf-8")
    session = ParentSession(source)
    server = create_server(session)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server, session
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
        assert not thread.is_alive()


@pytest.mark.parametrize("body", [{"path": "arbitrary.copper"},
    {"feature_index": 0, "path": "arbitrary.copper"}, {"feature_index": True},
    {"feature_index": -1}, {"feature_index": 1}, {"feature_index": "0"}])
def test_navigation_rejects_arbitrary_paths_and_invalid_indices(navigation_server, body):
    server, _ = navigation_server
    status, response = request(server.server_port, server.editor_token, "POST", "/api/open-definition", body)
    assert status == 400
    assert response["error"]


def test_navigation_requires_parent_capability(navigation_server):
    server, _ = navigation_server
    status, _ = request(server.server_port, "wrong", "POST", "/api/open-definition", {"feature_index": 0})
    assert status == 403


@pytest.mark.parametrize("kind", ["macro", "board_profile"])
def test_navigation_opens_real_definition_child_reuses_and_closes_it(tmp_path, monkeypatch, kind):
    if kind == "macro":
        _, source, _, _ = definition_project(tmp_path)
    else:
        source = tmp_path / "profile.copper"
        source.write_text(PROFILE, encoding="utf-8")
        (tmp_path / "copper.mod").write_text("module test/navigation\n", encoding="utf-8")
    parent = ParentSession(source)
    server = create_server(parent)
    thread = Thread(target=server.serve_forever, daemon=True)
    children = []
    original_create = create_server
    def track_child(session, port=0):
        child = original_create(session, port)
        children.append(child)
        return child
    monkeypatch.setattr("pcbir.editor.server.create_server", track_child)
    thread.start()
    try:
        status, result = request(server.server_port, server.editor_token, "POST", "/api/open-definition", {"feature_index": 0})
        assert status == 200, result
        url = urlsplit(result["url"])
        token = parse_qs(url.fragment)["token"][0]
        assert token != server.editor_token
        status, scene = request(url.port, token)
        assert status == 200
        assert scene["document_kind"] == kind
        assert scene["source_features"]
        assert request(url.port, server.editor_token)[0] == 403
        assert request(server.server_port, server.editor_token, "POST", "/api/open-definition", {"feature_index": 0})[1] == result
        assert len(children) == 1
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
    assert parent.closed
    assert not thread.is_alive()
    assert children[0].socket.fileno() == -1
    with pytest.raises(OSError):
        request(url.port, token)

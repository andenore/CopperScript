from hashlib import sha256
import json
import subprocess
import sys

import pytest

from pcbir.editor.document import DocumentHost, compiler_fingerprint, text_edits
from pcbir.editor.source import SourceEditError
from test_editor_transactions import SOURCE


def fixture(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text(SOURCE, encoding="utf-8")
    host = DocumentHost(source, allow_proxy=True)
    host.open(1, SOURCE)
    return host, source


def operation(host, action, **fields):
    return host.request({"method": "operation", "version": host.version,
                         "operation": {"action": action, "revision": host.session.revision, **fields}})


def test_in_memory_document_edits_require_native_host_commit(tmp_path):
    host, source = fixture(tmp_path)
    original = source.read_bytes()
    result = operation(host, "prepare_lock", reference="R1", x_nm=9000000, y_nm=8000000,
                       rotation=0, side="front", locks=["position", "rotation", "side"])
    assert result["scene"]["document_host"]
    commit = operation(host, "save_source", review_id=host.session.source_pending.id)["document_edit"]
    assert commit["version"] == 1
    assert commit["source_revision"] == sha256(SOURCE.encode()).hexdigest()
    text = SOURCE
    for edit in reversed(commit["edits"]):
        text = text[:edit["start"]] + edit["text"] + text[edit["end"]:]
    assert "x = 9mm" in text
    assert source.read_bytes() == original
    assert host.session.source_pending is not None  # native commit not acknowledged yet
    with pytest.raises(SourceEditError, match="document host"):
        host.session.workspace.save(host.session.source_pending)
    host.open(2, text)  # native document change/save, or an unsaved text-buffer edit
    assert host.scene()["components"][0]["source_position_locked"]
    assert source.read_bytes() == original
    host.open(3, SOURCE)  # native Undo
    assert host.text == SOURCE
    host.close()


def test_native_source_links_and_unsaved_text_are_authoritative(tmp_path):
    host, source = fixture(tmp_path)
    host.open(2, SOURCE.replace("x = 8mm", "x = 9mm"))
    scene = host.scene()
    r1 = next(c for c in scene["components"] if c["reference"] == "R1")
    assert r1["position"][0] == 9000000
    assert r1["source_link"]["line"] == 4
    assert not scene["can_source_undo"]
    assert "x = 8mm" in source.read_text(encoding="utf-8")


def test_stale_review_cannot_commit_newer_document(tmp_path):
    host, source = fixture(tmp_path)
    operation(host, "prepare_lock", reference="R1", x_nm=9000000, y_nm=8000000,
              rotation=0, side="front", locks=["position"])
    review = host.session.source_pending.id
    host.open(2, SOURCE + "// Native text edit\n")
    with pytest.raises(ValueError, match="changed"):
        host.request({"method": "operation", "version": 1, "operation": {"action": "save_source", "revision": 1, "review_id": review}})
    with pytest.raises(ValueError, match="review changed"):
        operation(host, "save_source", review_id=review)


def test_invalid_unsaved_source_recovers_at_new_document_version(tmp_path):
    host, source = fixture(tmp_path)
    with pytest.raises(Exception):
        host.open(2, "invalid text")
    assert host.session is None
    with pytest.raises(ValueError):
        host.request({"method": "scene"})
    host.open(3, SOURCE)
    assert host.scene()["document_version"] == 3
    assert source.read_text(encoding="utf-8") == SOURCE


@pytest.mark.parametrize("action", ["undo_source", "redo_source", "reload_source"])
def test_backend_cannot_bypass_native_source_history(tmp_path, action):
    host, _ = fixture(tmp_path)
    with pytest.raises(ValueError, match="native VS Code"):
        operation(host, action)


@pytest.mark.parametrize("message", [None, {}, {"method": "open", "version": 1, "text": SOURCE, "filename": "other"},
                                    {"method": "hello", "command": "untrusted"}, {"method": "read", "path": "anything"}])
def test_protocol_has_no_arbitrary_paths_or_execution(tmp_path, message):
    host, _ = fixture(tmp_path)
    with pytest.raises(ValueError):
        host.request(message)


@pytest.mark.parametrize("version", [True, 1.2, -1, 0, 1])
def test_document_versions_are_strict_and_monotonic(tmp_path, version):
    host, _ = fixture(tmp_path)
    with pytest.raises(ValueError):
        host.open(version, SOURCE)


def test_utf16_minimal_span_keeps_unicode_and_unrelated_text():
    before = "// 🟢 Ω\r\nx = 8mm; // keep\r\n"
    after = before.replace("8mm", "10mm")
    edit, = text_edits(before, after)
    raw = before.encode("utf-16-le")
    changed = raw[:edit["start"]*2] + edit["text"].encode("utf-16-le") + raw[edit["end"]*2:]
    assert changed.decode("utf-16-le") == after
    assert edit["text"] == "10"


def test_stdio_protocol_real_process_no_source_writes(tmp_path):
    source = tmp_path / "board.copper"
    source.write_text(SOURCE, encoding="utf-8")
    requests = [{"method": "hello"}, {"method": "open", "version": 1, "text": SOURCE}, {"method": "scene"}]
    result = subprocess.run([sys.executable, "-u", "-m", "pcbir.editor.document", str(source), "--allow-proxy-footprints"],
        input="".join(json.dumps({"id": i, "request": request}) + "\n" for i, request in enumerate(requests)),
        capture_output=True, text=True, encoding="utf-8", timeout=30)
    assert result.returncode == 0, result.stderr
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(responses) == 3
    assert responses[0]["result"]["compiler_digest"] == compiler_fingerprint()
    assert len(responses[1]["result"]["scene"]["components"]) == 2
    assert responses[2]["result"]["scene"]["document_host"]
    assert source.read_text(encoding="utf-8") == SOURCE

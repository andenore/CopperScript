"""Checked dialect profiles never inherit user startup or source-selected binaries."""
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from pcbir.simulation import SimulationError
from pcbir.simulation.models import load_registry
from pcbir.simulation.ngspice import _configuration, runtime_profile


def model(dialect="pspice", libraries=("analog", "xtradev")):
    return SimpleNamespace(dialect=dialect, code_models=libraries)


def test_engine_owned_libraries_are_required_hashed_and_copied(tmp_path, monkeypatch):
    engine = tmp_path / "engine/bin/ngspice"
    root = tmp_path / "engine/lib/ngspice"
    root.mkdir(parents=True)
    for name in ("analog", "xtradev"):
        (root / f"{name}.cm").write_bytes(name.encode())
    circuit = SimpleNamespace(models=(model(),))
    profile = runtime_profile([circuit], engine)
    assert profile["dialect"] == "pspice"
    assert [entry["name"] for entry in profile["code_models"]] == ["analog", "xtradev"]
    monkeypatch.setenv("SPICE_SCRIPTS", "untrusted")
    monkeypatch.setenv("NGSPICE_INPUT_DIR", "untrusted")
    folder = tmp_path / "output with spaces"
    folder.mkdir()
    env = _configuration(folder, profile)
    assert env["SPICE_SCRIPTS"] == str(folder / "initialization")
    assert "NGSPICE_INPUT_DIR" not in env
    startup = (folder / "initialization/spinit").read_text()
    assert "set ngbehavior=ps" in startup and "codemodel $copper_cm_0" in startup
    for entry in profile["code_models"]:
        copied = folder / "initialization" / (entry["name"] + ".cm")
        assert hashlib.sha256(copied.read_bytes()).hexdigest() == entry["sha256"]
    (root / "analog.cm").write_bytes(b"changed")
    with pytest.raises(SimulationError, match="changed after identification"):
        _configuration(folder, profile)


def test_missing_engine_library_and_mixed_profiles_fail_before_execution(tmp_path):
    circuit = SimpleNamespace(models=(model(),))
    with pytest.raises(SimulationError, match="analog.cm was not found"):
        runtime_profile([circuit], tmp_path / "bin/ngspice")
    mixed = SimpleNamespace(models=(model(), model("native", ())))
    with pytest.raises(SimulationError, match="mixed native and pspice"):
        runtime_profile([mixed], tmp_path / "bin/ngspice")


def test_native_startup_loads_no_optional_engine_code(tmp_path):
    profile = runtime_profile([SimpleNamespace(models=())], tmp_path / "bin/ngspice")
    env = _configuration(tmp_path, profile)
    assert profile == {"dialect": "native", "code_models": []}
    assert "codemodel" not in (tmp_path / "initialization/spinit").read_text()
    assert env["LC_NUMERIC"] == "C"


def test_profile_rejects_arbitrary_binary_names(tmp_path):
    with pytest.raises(SimulationError, match="unsupported engine code-model"):
        _configuration(tmp_path, {"dialect": "pspice", "code_models": [{"name": "../../payload", "path": "unused", "sha256": "unused"}]})


@pytest.mark.parametrize("fields", [
    {"dialect": "ltspice"}, {"dialect": []}, {"dialect": None},
    {"dialect": "pspice", "code_models": ["../../payload"]},
    {"dialect": "native", "code_models": ["analog"]},
    {"dialect": "pspice", "code_models": True},
])
def test_registry_rejects_unknown_dialects_and_source_binary_paths(tmp_path, fields):
    content = b".subckt TEST IN OUT\nR1 IN OUT 1000\n.ends TEST\n"
    (tmp_path / "model.lib").write_bytes(content)
    entry = {"kind": "subcircuit", "name": "TEST", "terminals": ["IN", "OUT"],
             "pin_map": {"IN": "1", "OUT": "2"}, "supported_parts": ["TEST"],
             "analyses": ["op"], "ngspice_versions": [46, 47], "entrypoint": "model.lib",
             "files": [{"path": "model.lib", "sha256": hashlib.sha256(content).hexdigest()}],
             "license": "MIT", **fields}
    registry = tmp_path / "registry.json"
    registry.write_text(json.dumps({"schema_version": 1, "models": {"test": entry}}))
    with pytest.raises(SimulationError):
        load_registry(registry)

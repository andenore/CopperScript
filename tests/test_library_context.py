from pathlib import Path
import hashlib
import json

import pytest

from pcbir.cli import main
from pcbir import compile_file
from pcbir.library_context import (
    diagnostics, index_library, inspect, project_catalog, render_page, search, write_index,
)
from pcbir.syntax import CopperScriptError


@pytest.fixture
def libraries(tmp_path):
    root = tmp_path / "project"
    root.mkdir()
    for name in ("alpha", "beta"):
        library = tmp_path / name
        (library / "parts").mkdir(parents=True)
        (library / "copper.mod").write_text(f"module github.com/vendor/{name}\n", encoding="utf-8")
        (library / "parts" / "sensor.copper").write_text(
            '// Illustrative fixture; requires project validation.\n'
            'part Sensor { category = "sensor.temperature"; '
            f'manufacturer = "Fixture {name.upper()}-TEMP"; source_url = "https://example.test/#facts"; '
            'pin VDD { number = "1"; voltage_min = 1.8V; voltage_max = 3.6V; domains = "power"; directions = "input"; } '
            'pin GND { number = "2"; domains = "ground"; directions = "input"; } }', encoding="utf-8")
    (root / "copper.mod").write_text(
        'module example/board\n'
        'require github.com/vendor/alpha v1\nreplace github.com/vendor/alpha => ../alpha\n'
        'require github.com/vendor/beta v2\nreplace github.com/vendor/beta => ../beta\n', encoding="utf-8")
    return root


def catalog(root):
    return project_catalog(root, locked=False)


def test_multi_provider_search_is_bounded_and_keeps_same_name_identities(libraries):
    data = catalog(libraries)
    first = search(data, "Sensor temperature", limit=1)
    assert first["total"] == 2 and first["omitted"] == 1
    assert first["next_offset"] == 1
    second = search(data, "Sensor temperature", limit=1, offset=first["next_offset"])
    assert second["next_offset"] is None
    assert first["items"][0]["selector"] != second["items"][0]["selector"]
    assert first["items"][0]["version"] == "v1"
    assert second["items"][0]["version"] == "v2"
    assert search(data, "ALPHA-TEMP")["total"] == 1
    assert search(data, "no-match")["items"] == []


def test_selected_contract_keeps_exact_limits_and_comments(libraries):
    data = catalog(libraries)
    result = inspect(data, "github.com/vendor/alpha/parts::Sensor")
    assert result["identity"]["source_sha256"].startswith("sha256:")
    assert result["evidence_level"] == "source_declarations_only"
    assert result["comment_count"] == 1  # URL and hash in a string are not comments.
    assert result["items"][0]["type"] == "SourceNote"
    interface = inspect(data, result["identity"]["selector"], section="interfaces")
    assert interface["items"][0]["attributes"]["voltage_min"] == "1.8 V"
    assert interface["items"][0]["attributes"]["voltage_max"] == "3.6 V"
    assert "BETA-TEMP" not in render_page(result)


def test_stale_selection_and_asset_changes_are_detected(libraries):
    before = catalog(libraries)
    entry = before.index["entries"][0]
    asset = libraries.parent / "alpha" / "parts" / "package.kicad_mod"
    asset.write_text('(footprint "fixture")', encoding="utf-8")
    # Locked discovery does not quietly change project intent or lock bytes.
    lock = (libraries / "copper.lock").read_bytes()
    with pytest.raises(CopperScriptError, match="lock mismatch"):
        project_catalog(libraries)
    assert (libraries / "copper.lock").read_bytes() == lock
    after = catalog(libraries)
    with pytest.raises(ValueError, match="changed since selection"):
        inspect(after, entry["selector"], expected_digest=entry["content_digest"])


def test_locked_offline_discovery_does_not_write_source_or_lock(libraries):
    catalog(libraries)
    paths = [p for p in libraries.parent.rglob("*") if p.is_file()]
    before = {p: p.read_bytes() for p in paths}
    result = project_catalog(libraries)
    inspect(result, result.index["entries"][0]["selector"], section="source")
    assert before == {p: p.read_bytes() for p in paths}


def test_portable_index_is_deterministic_and_safe_to_export(libraries, tmp_path):
    root = libraries.parent / "alpha"
    data = index_library(root)
    assert data.index == index_library(root).index
    output = tmp_path / "alpha-index.json"
    write_index(data, output)
    assert json.loads(output.read_text())["entries"][0]["package"] == "github.com/vendor/alpha/parts"
    with pytest.raises(ValueError, match="new file"):
        write_index(data, output)
    with pytest.raises(ValueError, match="outside"):
        write_index(data, root / "copper.index.json")


def test_invalid_source_reports_incomplete_without_hiding_valid_matches(libraries):
    bad = libraries.parent / "beta" / "parts" / "bad.copper"
    bad.write_text('part Broken { pin ', encoding="utf-8")
    data = catalog(libraries)
    page = search(data, "Sensor")
    assert page["total"] == 2 and not page["discovery_complete"]
    errors = diagnostics(data, offset=0, limit=2, max_chars=3000)
    assert errors["total"] == 1 and "bad.copper" in errors["items"][0]["source"]


def test_duplicate_exports_are_not_silently_selected(libraries):
    source = libraries.parent / "alpha" / "parts" / "sensor.copper"
    source.with_name("duplicate.copper").write_bytes(source.read_bytes())
    data = catalog(libraries)
    assert not data.index["complete"]
    assert any(d["code"] == "DUPLICATE_EXPORT" for d in data.index["diagnostics"])
    with pytest.raises(ValueError, match="ambiguous"):
        inspect(data, "github.com/vendor/alpha/parts::Sensor", kind="part")


def test_kind_disambiguates_part_and_module_names(libraries):
    path = libraries.parent / "alpha" / "parts" / "module.copper"
    path.write_text('module Sensor { port IN: power_in; }', encoding="utf-8")
    data = catalog(libraries)
    with pytest.raises(ValueError, match="ambiguous"):
        inspect(data, "github.com/vendor/alpha/parts::Sensor")
    assert inspect(data, "github.com/vendor/alpha/parts::Sensor", kind="module")["identity"]["kind"] == "module"
    assert search(data, "Sensor", kind="part")["total"] == 2


def test_provider_filter_avoids_unrelated_broken_or_uncached_modules(libraries):
    with (libraries / "copper.mod").open("a") as stream:
        stream.write('require github.com/uncached/library v1\n')
    data = project_catalog(libraries, locked=False, providers=("github.com/vendor/alpha",))
    assert len(data.index["entries"]) == 1
    assert len(project_catalog(libraries, locked=False, providers=("github.com/vendor/alpha",) * 2).index["entries"]) == 1
    with pytest.raises(ValueError, match="not declared"):
        project_catalog(libraries, providers=("github.com/unknown/library",))


@pytest.mark.parametrize("options", [{"limit": 0}, {"limit": 101}, {"limit": True},
    {"offset": -1}, {"offset": True}, {"offset": 3}, {"max_chars": 100}, {"max_chars": 100000}])
def test_page_bounds_rejected(libraries, options):
    with pytest.raises(ValueError):
        search(catalog(libraries), **options)


def test_large_contract_is_paginated_without_silent_loss(libraries):
    path = libraries.parent / "alpha" / "parts" / "large.copper"
    path.write_text('part Large { ' + ''.join(f'pin P{i} {{ number = "{i}"; }} ' for i in range(120)) + '}', encoding="utf-8")
    data = catalog(libraries)
    offset, rows = 0, []
    while True:
        page = inspect(data, "github.com/vendor/alpha/parts::Large", section="interfaces", offset=offset, max_chars=3000)
        assert len(render_page(page)) <= 3000
        rows.extend(page["items"])
        if page["next_offset"] is None:
            assert page["page_complete"]
            break
        assert page["next_offset"] > offset
        offset = page["next_offset"]
    assert [r["name"] for r in rows] == [f"P{i}" for i in range(120)]


def test_oversized_record_is_explicit_and_can_be_retrieved_with_larger_budget(libraries):
    path = libraries.parent / "alpha" / "parts" / "large.copper"
    path.write_text('part Large { obligation = "' + 'x' * 6000 + '"; }', encoding="utf-8")
    data = catalog(libraries)
    page = inspect(data, "github.com/vendor/alpha/parts::Large", section="properties", max_chars=3000)
    assert page["items"][0]["needs_source"] and not page["page_complete"]
    assert len(render_page(page)) <= 3000
    full = inspect(data, "github.com/vendor/alpha/parts::Large", section="properties", max_chars=12000)
    assert len(full["items"][0]["value"]) == 6000 and full["page_complete"]


def test_cross_library_assembly_uses_existing_host_version_policy(libraries):
    root = libraries.parent / "alpha"
    (root / "assemblies").mkdir()
    (root / "assemblies" / "monitor.copper").write_text(
        'module Monitor { import sensor "github.com/vendor/beta/parts"; '
        'port VIN: power_in; port GND: power_in; component U1: sensor.Sensor; '
        'net VIN { port.VIN; U1.VDD; } net GND { port.GND; U1.GND; } '
        'constraint max_distance(U1, port.VIN) { distance = 3.5mm; mode = external; } }', encoding="utf-8")
    board = libraries / "board.copper"
    board.write_text('board B { import a "github.com/vendor/alpha/assemblies"; module M: a.Monitor; '
                     'net VIN { M.VIN; } net GND { M.GND; } }', encoding="utf-8")
    compiled = compile_file(board, offline=True)
    assert {d.module_path for d in compiled.dependencies} == {"github.com/vendor/alpha", "github.com/vendor/beta"}
    data = project_catalog(libraries)
    page = inspect(data, "github.com/vendor/alpha/assemblies::Monitor", section="dependencies")
    assert page["items"][0]["status"] == "resolved" and page["items"][0]["version"] == "v2"
    requirements = inspect(data, "github.com/vendor/alpha/assemblies::Monitor", section="requirements")
    assert requirements["items"][0]["parameters"] == {"distance": "3.5 mm", "mode": "external"}
    # An undeclared cross-library import remains an actionable blocker.
    (libraries / "copper.mod").write_text('module example/board\nrequire github.com/vendor/alpha v1\nreplace github.com/vendor/alpha => ../alpha\n')
    page = inspect(catalog(libraries), "github.com/vendor/alpha/assemblies::Monitor", section="dependencies")
    assert page["items"][0]["status"] == "blocked" and page["items"][0]["code"] == "PKG002"


def test_device_contract_is_explicit_dependency(libraries):
    path = libraries.parent / "alpha" / "parts" / "sensor.copper"
    path.write_text('part Sensor { device = "SensorDie"; pin VDD { number = "1"; } }', encoding="utf-8")
    data = catalog(libraries)
    row = inspect(data, "github.com/vendor/alpha/parts::Sensor", section="dependencies")["items"][0]
    assert row["selector"] == "github.com/vendor/alpha/parts::SensorDie"


def test_decoupling_associations_appear_as_requirements(libraries):
    path = libraries.parent / "alpha" / "parts" / "support.copper"
    path.write_text('module Support { use library "tiny"; component C: CAPACITOR { role = decoupling; decouples = "U.VDD"; } }')
    page = inspect(catalog(libraries), "github.com/vendor/alpha/parts::Support", section="requirements")
    assert page["items"][0]["attributes"]["decouples"] == "U.VDD"


def test_cli_default_is_locked_offline_and_compact(libraries, capsys):
    assert main(["library", "search", "Sensor", "--project", str(libraries)]) == 2
    assert "locked resolution" in capsys.readouterr().out
    assert main(["library", "search", "Sensor", "--project", str(libraries), "--update-lock", "--limit", "1"]) == 0
    page = json.loads(capsys.readouterr().out)
    assert len(page["items"]) == 1
    # Targeted inspection doesn't try an unrelated newly declared cache miss.
    with (libraries / "copper.mod").open("a") as stream:
        stream.write('require github.com/uncached/lib v1\n')
    entry = page["items"][0]
    assert main(["library", "inspect", entry["selector"], "--project", str(libraries),
                 "--expect-digest", entry["content_digest"], "--section", "interfaces"]) == 0
    inspected = json.loads(capsys.readouterr().out)
    assert inspected["total"] == 2


def test_remote_locked_cache_inventory_is_verified(libraries):
    module, version = "github.com/vendor/remote", "v3"
    cache = libraries / ".copper-cache" / "pkg" / hashlib.sha256(f"{module}@{version}".encode()).hexdigest()[:20]
    (cache / "parts").mkdir(parents=True)
    source = cache / "parts" / "part.copper"
    source.write_text('part R { category = "passive.resistor"; }')
    with (libraries / "copper.mod").open("a") as stream:
        stream.write(f'require {module} {version}\n')
    data = catalog(libraries)
    assert search(data, "resistor")["total"] == 1
    source.write_text('part R { category = "changed"; }')
    with pytest.raises(CopperScriptError, match="lock mismatch"):
        project_catalog(libraries)


def test_index_cli_export_and_discovery_diagnostics(libraries, tmp_path, capsys):
    output = tmp_path / "index.json"
    assert main(["library", "index", str(libraries.parent / "alpha"), "-o", str(output)]) == 0
    assert output.exists()
    capsys.readouterr()
    catalog(libraries)
    assert main(["library", "diagnostics", "--project", str(libraries)]) == 0
    assert json.loads(capsys.readouterr().out)["total"] == 0


def test_search_matches_declared_limits_not_only_preview_metadata(libraries):
    data = catalog(libraries)
    assert search(data, "voltage_max 3.6 V")["total"] == 2
    path = libraries.parent / "alpha" / "parts" / "module.copper"
    path.write_text('module Power { port VOUT: power_out; }')
    result = search(catalog(libraries), "power_out", kind="module")
    assert result["items"][0]["capabilities"] == ["power_out"]


def test_nested_module_shadowing_matches_compiler_resolution(libraries):
    nested = libraries.parent / "nested"
    nested.mkdir()
    (nested / "part.copper").write_text('part Special { category = "nested.sensor"; }')
    with (libraries / "copper.mod").open("a") as stream:
        stream.write('require github.com/vendor/alpha/parts v3\n'
                     'replace github.com/vendor/alpha/parts => ../nested\n')
    data = catalog(libraries)
    names = [e["selector"] for e in data.index["entries"]]
    assert "github.com/vendor/alpha/parts::Sensor" not in names
    assert "github.com/vendor/alpha/parts::Special" in names


def test_symlink_assets_are_rejected_if_available(libraries):
    path = libraries.parent / "alpha" / "parts" / "alias.copper"
    try:
        path.symlink_to(path.with_name("sensor.copper"))
    except OSError:
        pytest.skip("creating symlinks is unavailable on this host")
    with pytest.raises(CopperScriptError, match="symlinks"):
        catalog(libraries)


def test_targeted_cli_rejects_stale_content_digest(libraries, capsys):
    data = catalog(libraries)
    entry = data.index["entries"][0]
    assert main(["library", "inspect", entry["selector"], "--project", str(libraries),
                 "--expect-digest", "sha256:" + "0" * 64]) == 2
    assert "changed since selection" in capsys.readouterr().out


def test_member_selection_keeps_limits_and_reports_unselected_and_missing(libraries):
    data = catalog(libraries)
    page = inspect(data, "github.com/vendor/alpha/parts::Sensor", section="interfaces", members=("VDD", "MISSING"))
    assert page["total"] == 1 and page["section_total"] == 2 and page["unselected"] == 1
    assert page["unmatched_members"] == ["MISSING"]
    assert page["items"][0]["attributes"]["voltage_max"] == "3.6 V"
    assert inspect(data, "github.com/vendor/alpha/parts::Sensor", section="interfaces", members=("absent",))["total"] == 0
    with pytest.raises(ValueError, match="not source/notes"):
        inspect(data, "github.com/vendor/alpha/parts::Sensor", section="source", members=("VDD",))


def test_member_selection_by_constraint_target_does_not_drop_external_mode(libraries):
    path = libraries.parent / "alpha" / "parts" / "module.copper"
    path.write_text('module Power { constraint max_distance(U1.VDD, C1) { distance = 2.5mm; mode = external; } }')
    page = inspect(catalog(libraries), "github.com/vendor/alpha/parts::Power", section="requirements", members=("U1.VDD",))
    assert page["items"][0]["parameters"]["mode"] == "external"
    assert page["items"][0]["parameters"]["distance"] == "2.5 mm"


def test_member_cli_limits_output_to_requested_pins(libraries, capsys):
    catalog(libraries)
    assert main(["library", "inspect", "github.com/vendor/alpha/parts::Sensor", "--project", str(libraries),
                 "--section", "interfaces", "--member", "GND"]) == 0
    page = json.loads(capsys.readouterr().out)
    assert [r["name"] for r in page["items"]] == ["GND"]
    assert page["unselected"] == 1 and page["unmatched_members"] == []

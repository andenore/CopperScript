import subprocess
import sys
import json
from pathlib import Path


ROOT = Path(__file__).parents[1]


def run_cli(example: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "copperscript", "check", f"examples/{example}"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_cli_accepts_valid_copper_board() -> None:
    result = run_cli("valid_board/board.copper")
    assert result.returncode == 0
    assert "passed ERC" in result.stdout


def test_cli_rejects_invalid_copper_board() -> None:
    result = run_cli("invalid_board/board.copper")
    assert result.returncode == 1
    assert "OUTPUT_CONFLICT" in result.stdout
    assert "I2C_MISSING_PULLUP" in result.stdout


def test_cli_footprint_audit_json_is_machine_readable() -> None:
    command = [sys.executable, "-m", "copperscript", "audit-footprints",
               "examples/valid_board/board.copper", "--json"]
    first = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    second = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    assert first.stdout == second.stdout
    document = json.loads(first.stdout)
    assert document["schema"] == "copperscript-footprint-audit/v0.1"
    assert document["total"] == len(document["entries"])
    assert first.returncode == (0 if document["passed"] else 1)


def test_cli_compiles_to_json() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "compile", "examples/valid_board/board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert '"schema": "copperscript-ir/v0.1"' in result.stdout
    assert '"name": "ValidSensorBoard"' in result.stdout
    assert '"name": "STM32_LIKE"' in result.stdout


def test_cli_json_preserves_hierarchy() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "compile", "examples/hierarchical_board/board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert '"module_definitions"' in result.stdout
    assert '"module": "power.Buck5VTo3V3"' in result.stdout
    assert '"ref": "PWR"' in result.stdout
    assert '"ref": "PWR/U1"' not in result.stdout


def test_cli_rejects_python_board_sources() -> None:
    result = run_cli("../tests/fixtures/python_ir/valid_board.py")
    assert result.returncode == 2
    assert "expected .copper" in result.stdout


def test_cli_runs_power_state_analysis() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "copperscript", "power-check", "examples/valid_board/board.copper"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert "passed power-state analysis" in result.stdout


def test_cli_exports_kicad_schematic(tmp_path: Path) -> None:
    output = tmp_path / "valid.kicad_sch"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad",
            "examples/valid_board/board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "Generated KiCad 8.0 schematic" in result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_sch")


def test_cli_refuses_kicad_export_when_erc_fails(tmp_path: Path) -> None:
    output = tmp_path / "invalid.kicad_sch"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad",
            "examples/invalid_board/board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 1
    assert "stopped because ERC reported errors" in result.stdout
    assert not output.exists()


def test_cli_exports_prototype_kicad_pcb(tmp_path: Path) -> None:
    output = tmp_path / "valid.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/valid_board/board.copper",
            "-o",
            str(output),
            "--allow-proxy-footprints",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "Generated KiCad 8.0 PCB" in result.stdout
    assert "proxy footprints" in result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_pcb")
    assert output.with_suffix(".kicad_pro").is_file()


def test_cli_exports_pcb_with_resolved_kicad_mod(tmp_path: Path) -> None:
    output = tmp_path / "resolved.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/resolved_footprint_board/board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    assert "proxy footprints" not in result.stdout
    content = output.read_text(encoding="utf-8")
    assert '(footprint "CopperScript:R_0402_CopperScript_kicad_mod__' in content
    assert (tmp_path / "fp-lib-table").is_file()
    assert len(list((tmp_path / "CopperScript.pretty").glob("*.kicad_mod"))) == 1
    assert '(fp_rect' in content


def test_cli_exports_provisional_four_layer_board(tmp_path: Path) -> None:
    output = tmp_path / "four-layer.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "export-kicad-pcb",
            "examples/resolved_footprint_board/board.copper", "--layers", "4",
            "-o", str(output),
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 0, result.stdout
    content = output.read_text(encoding="utf-8")
    assert '(1 "In1.Cu" signal)' in content
    assert '(2 "In2.Cu" signal)' in content


def test_cli_accepts_explicit_four_layer_fabrication_profile(tmp_path: Path) -> None:
    output = tmp_path / "four-layer-profile.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "export-kicad-pcb",
            "examples/resolved_footprint_board/board.copper", "--layers", "4",
            "--fab-profile", "jlcpcb-four-layer", "-o", str(output),
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 0, result.stdout
    assert output.exists()
    project = json.loads(output.with_suffix(".kicad_pro").read_text(encoding="utf-8"))
    assert project["net_settings"]["classes"][0]["clearance"] == 0.09
    assert project["board"]["design_settings"]["rules"]["min_track_width"] == 0.09


def test_cli_exports_provisional_six_layer_board(tmp_path: Path) -> None:
    output = tmp_path / "six-layer.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "export-kicad-pcb",
            "examples/resolved_footprint_board/board.copper", "--layers", "6",
            "--fab-profile", "jlcpcb-six-layer", "-o", str(output),
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 0, result.stdout
    content = output.read_text(encoding="utf-8")
    assert '(3 "In3.Cu" signal)' in content
    assert '(4 "In4.Cu" signal)' in content
    project = json.loads(output.with_suffix(".kicad_pro").read_text(encoding="utf-8"))
    assert project["net_settings"]["classes"][0]["clearance"] == 0.09
    assert project["board"]["design_settings"]["rules"]["min_track_width"] == 0.09


def test_cli_does_not_silently_fall_back_to_proxy_footprints(
    tmp_path: Path,
) -> None:
    output = tmp_path / "unresolved.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "export-kicad-pcb",
            "examples/valid_board/board.copper",
            "-o",
            str(output),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 2
    assert "cannot resolve footprint" in result.stdout
    assert not output.exists()


def test_cli_plans_layout_and_writes_readiness_report(tmp_path: Path) -> None:
    output = tmp_path / "planned.kicad_pcb"
    report = tmp_path / "layout-report.json"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "plan-layout",
            "examples/valid_board/board.copper",
            "-o",
            str(output),
            "--report",
            str(report),
            "--allow-proxy-footprints",
            "--candidates",
            "2",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    assert output.read_text(encoding="utf-8").startswith("(kicad_pcb")
    assert output.with_suffix(".kicad_pro").is_file()
    document = json.loads(report.read_text(encoding="utf-8"))
    assert document["schema"] == "copperscript-layout-report/v0.1"
    assert 1 <= len(document["candidates"]) <= 2
    assert document["selected_candidate"].startswith("candidate-")
    assert "PLACE:" in result.stdout
    assert "ROUTE: not_run" in result.stdout
    assert "VERIFY: blocked" in result.stdout
    assert "Pareto candidate" in result.stdout


def test_cli_writes_global_routing_guides(tmp_path: Path) -> None:
    output = tmp_path / "global-route.json"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "route-global",
            "examples/valid_board/board.copper",
            "-o",
            str(output),
            "--allow-proxy-footprints",
            "--candidates",
            "1",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0, result.stdout
    document = json.loads(output.read_text(encoding="utf-8"))
    assert document["schema"] == "copperscript-global-route/v0.1"
    assert document["status"] == "success"
    assert document["routes"]
    assert "GLOBAL ROUTE: success" in result.stdout


def test_cli_reports_physical_route_and_drc_without_claiming_fabrication(tmp_path: Path) -> None:
    report = tmp_path / "route-report.json"
    pcb = tmp_path / "route-draft.kicad_pcb"
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "route-board",
            "examples/valid_board/board.copper", "--allow-proxy-footprints",
            "--candidates", "1", "--passes", "1", "--pitch-mm", "1",
            "--fanout",
            "--stitch-zone-pads", "--plane-stitch-step-mm", "0.25",
            "--plane-stitch-radius-mm", "5",
            "--critical-feedback-trials", "1",
            "--report", str(report), "-o", str(pcb),
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode in {0, 1}, result.stdout + result.stderr
    document = json.loads(report.read_text(encoding="utf-8"))
    assert document["schema"] == "copperscript-route-board/v0.1"
    assert document["erc_pass"]
    assert isinstance(document["fanout"]["pin_access_analysis"], list)
    assert set(document["fanout"]["assignment"]) == {"pair_checks", "pair_queries", "broad_phase_accepts", "native_accepted", "expanded_pads", "trials"}
    assert all(set(item) == {"pad", "legal_candidate_count", "selected_candidate_index", "diagnostic", "two_leg_candidate_count"}
               for item in document["fanout"]["pin_access_analysis"])
    assert document["fabrication_ready"] is False
    # With fanout, critical/access failures share the escape-first controller.
    assert "critical_placement_feedback" not in document
    assert document["package_access"]["status"] in {"ready", "blocked"}
    assert document["package_access"]["stage_order"][0] == "ordinary_package_exits"
    assert "trials" in document["package_access"]
    assert document["package_access"]["pattern_trial_limit"] == 2
    assert isinstance(document["package_access"]["pattern_trials"], list)
    boundary = document["package_access"]["boundary"]
    assert "not committed" in boundary["scope"]
    assert boundary["status"] in {"ready", "blocked"}
    assert isinstance(boundary["ports"], list) and isinstance(boundary["pin_analysis"], list)
    assert "package_boundary_access" in document["package_access"]["stage_order"]
    if document["package_access"]["status"] == "blocked":
        assert document["package_access"]["ordinary_area_started"] is False
        assert document["detailed"]["metrics"]["passes"] == 0
    assert document["detailed"]["status"] in {"success", "partial"}
    assert document["drc"]["decision"] in {"pass", "fail", "incomplete"}
    assert "signal_track_length_nm" in document["route_geometry"]
    assert "signal_layer_length_nm" in document["route_geometry"]
    assert document["connectivity"]["native_fill_verified"] is False
    zones = set(document["route_geometry"]["zone_nets_deferred"])
    assert set(document["connectivity"]["unrouted_ordinary_nets"]).isdisjoint(zones)
    assert set(document["connectivity"]["deferred_zone_nets"]) <= zones
    assert document["plane_stitch"]["zone_fill_verified"] is False
    assert document["plane_stitch"]["step_nm"] == 250_000
    assert document["plane_stitch"]["maximum_radius_nm"] == 5_000_000
    assert document["plane_stitch"]["maximum_contact_radius_nm"] == 0
    assert "trials" in document["zone_escape_feedback"]
    assert document["zone_escape_feedback"]["dependency_expansion_limit"] == 2
    assert document["zone_escape_feedback"]["incremental_placement_enabled"] is True
    assert all("changed_references" in trial and "rebuilt_zone_nets" in trial
               for trial in document["zone_escape_feedback"]["trials"])
    assert all("repair_nets" in trial and "dependency_expansions" in trial and "strategy" in trial
               for trial in document["zone_escape_feedback"]["trials"])
    assert all("failed_signals" in trial and "decision" in trial
               for trial in document["zone_escape_feedback"]["trials"])
    assert pcb.read_text(encoding="utf-8").startswith("(kicad_pcb")
    assert pcb.with_suffix(".kicad_pro").is_file()
    assert "BOARD ROUTE:" in result.stdout


def test_cli_requires_plane_stitch_and_six_layers_for_ground_via_in_pad() -> None:
    command = [
        sys.executable, "-m", "copperscript", "route-board",
        "examples/valid_board/board.copper", "--allow-proxy-footprints",
        "--ground-via-in-pad",
    ]
    without_stitch = subprocess.run(
        command, cwd=ROOT, text=True, capture_output=True, check=False,
    )
    assert without_stitch.returncode != 0
    assert "requires a declared copper zone" in without_stitch.stdout

    without_profile = subprocess.run(
        [*command, "--stitch-zone-pads"], cwd=ROOT,
        text=True, capture_output=True, check=False,
    )
    assert without_profile.returncode != 0
    assert "six-layer profile" in without_profile.stdout


def test_cli_rejects_unknown_selective_plane_pad() -> None:
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "route-board",
            "examples/valid_board/board.copper", "--allow-proxy-footprints",
            "--stitch-zone-pads", "--early-plane-pad", "UNKNOWN.1",
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 2
    assert "not a zone-net pad" in result.stdout


def test_cli_rejects_nonpositive_plane_search_step() -> None:
    result = subprocess.run(
        [
            sys.executable, "-m", "copperscript", "route-board",
            "examples/valid_board/board.copper", "--plane-stitch-step-mm", "0",
        ],
        cwd=ROOT, text=True, capture_output=True, check=False,
    )

    assert result.returncode == 2
    assert "expected a positive finite length" in result.stderr


def test_cli_checks_kicad_mod_footprint() -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "copperscript",
            "check-footprint",
            "tests/fixtures/footprints/R_0402_Test.kicad_mod",
            "--strict",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )

    assert result.returncode == 0
    assert "R_0402_Test: 2 pads, 6 graphics" in result.stdout

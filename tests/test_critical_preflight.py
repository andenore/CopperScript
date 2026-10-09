from pathlib import Path
import json

from pcbir.critical_preflight import main


ROOT = Path(__file__).resolve().parents[1]


def test_package_access_gate_reports_pending_identities_and_exports_contacts(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from dataclasses import replace
    from pcbir.physical import PadReference, nm_from_mm

    original = preflight.preflight_package_access
    seen = []
    def inspect(board, guides, fanout_options, plane_options, **kwargs):
        assert plane_options.include_surface_zones
        assert plane_options.maximum_contact_radius_nm == nm_from_mm("5")
        assert plane_options.preferred_ground_pads == frozenset(next(
            net.pads for net in board.nets if net.name == "GND"
        ))
        result = original(board, guides, fanout_options, plane_options, **kwargs)
        # A failed contact must fail the gate even when the critical set is empty.
        result = replace(result, plane_stitch=replace(result.plane_stitch,
            pending_pads=(PadReference("R1", "1"),)))
        seen.append(result.board)
        return result
    monkeypatch.setattr(preflight, "preflight_package_access", inspect)
    backend = preflight.KiCadPcbBackend.generate
    def inspect_export(self, board):
        assert board is seen[0]
        return backend(self, board)
    monkeypatch.setattr(preflight.KiCadPcbBackend, "generate", inspect_export)
    report = tmp_path / "access.json"
    output = tmp_path / "access.kicad_pcb"
    assert main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--package-access", "--stitch-surface-zones", "--plane-contact-radius-mm", "5",
        "--report", str(report), "-o", str(output)]) == 1
    data = json.loads(report.read_text())
    assert data["stage"] == "package_access_complete" and data["complete"]
    assert not data["fabrication_ready"] and not data["package_access"]["ready"]
    assert "R1.1" in data["package_access"]["pending_pads"]
    assert data["package_access_progress"] and output.is_file()


def test_package_access_interruption_keeps_checkpoint(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    def interrupt(*args, on_progress):
        on_progress("ordinary_package_exits", "started", {})
        raise ValueError("bounded interruption")
    monkeypatch.setattr(preflight, "preflight_package_access", interrupt)
    report = tmp_path / "access.json"
    assert main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--package-access", "--report", str(report)]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "ordinary_package_exits_started" and not data["complete"]
    assert not data["fabrication_ready"]
    assert data["package_access_progress"][0]["elapsed_seconds"] >= 0


def test_preflight_binds_scenes_before_placement_and_preserves_owner_copper(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from test_hard_macros import fixture
    from pcbir.hard_macros import resolved_macro_geometry
    original, _, bind = fixture(tmp_path)
    bound = bind()
    scene = tmp_path / "scene.json"
    scene.write_text('{}\n')
    seen = []
    monkeypatch.setattr(preflight, "prototype_physicalize", lambda *args: original)
    def apply(board, path, **kwargs):
        assert board is original and path == scene and kwargs == {"locked": True, "offline": True}
        seen.append(path)
        return bound
    monkeypatch.setattr(preflight, "apply_hard_macro_scene", apply)
    critical = preflight.route_critical_nets
    def inspect(board, global_route, **kwargs):
        assert board.hard_macros == bound.hard_macros
        result = critical(board, global_route, **kwargs)
        assert result.board.materialized_macros == ("unit",)
        assert result.board.tracks == resolved_macro_geometry(result.board, result.board.hard_macros[0])[0]
        return result
    monkeypatch.setattr(preflight, "route_critical_nets", inspect)
    report = tmp_path / "preflight.json"
    main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--locked", "--offline", "--hard-macro", str(scene), "--report", str(report)])
    assert seen == [scene]
    data = json.loads(report.read_text())
    assert data["complete"] and data["hard_macros"][0]["scene"] == str(scene)


def test_preflight_exports_partial_artifact_and_never_claims_full_signoff(tmp_path) -> None:
    report = tmp_path / "preflight.json"
    output = tmp_path / "preflight.kicad_pcb"
    code = main([
        str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report), "-o", str(output),
    ])
    data = json.loads(report.read_text())
    # Even an empty critical stage cannot promote an uncertified global route.
    assert code == (0 if data["global_route_certified"] else 1)
    assert data["stage"] == "critical_complete" and data["complete"]
    assert data["fabrication_ready"] is False
    assert "DRC-ROUTE-INCOMPLETE" in {item["code"] for item in data["native_drc"]["findings"]}
    assert data["phase_seconds"].keys() == {"load_and_resolve", "placement_and_global", "critical"}
    assert output.is_file() and output.with_suffix(".kicad_pro").is_file()


def test_preflight_retains_global_checkpoint_on_critical_interruption(tmp_path, monkeypatch) -> None:
    import pcbir.critical_preflight as preflight

    def interrupt(*args, **kwargs):
        raise ValueError("critical search interrupted")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "global_complete"
    assert data["complete"] is False and data["fabrication_ready"] is False
    assert "global_route" in data and "critical" not in data


def test_preflight_keeps_running_group_checkpoint_on_interruption(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight

    def interrupt(*args, on_progress):
        on_progress("started", ("USB_DM", "USB_DP"), None)
        raise ValueError("bounded test interruption")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_group_running" and data["complete"] is False
    assert data["critical_progress"] == [{"nets": ["USB_DM", "USB_DP"], "state": "running"}]
    assert data["fabrication_ready"] is False and "critical" not in data


def test_completed_group_checkpoint_retains_metrics_but_not_full_signoff(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from pcbir.critical import CriticalNetResult

    def interrupt(*args, on_progress):
        result = CriticalNetResult(("MATCH",), True, 2, 0, (1000000,), 0,
                                   strategy="local_surface_tree", local_candidate_attempts=1,
                                   guide_length_nm=5000000)
        on_progress("started", result.nets, None)
        on_progress("finished", result.nets, result)
        raise ValueError("interrupted after a completed group")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_group_complete" and data["complete"] is False
    group = data["critical_progress"][0]
    assert group["state"] == "finished" and group["seconds"] >= 0
    assert group["result"]["guide_length_nm"] == 5000000
    assert group["result"]["local_candidate_attempts"] == 1
    assert data["fabrication_ready"] is False and "critical" not in data


def test_preflight_forwards_opt_in_feedback_and_exports_selected_result(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight
    from pcbir.critical_feedback import improve_critical_placement

    seen = []
    def observe(board, global_route, critical, **kwargs):
        seen.append(kwargs["maximum_trials"])
        return improve_critical_placement(board, global_route, critical, **kwargs)

    monkeypatch.setattr(preflight, "improve_critical_placement", observe)
    report = tmp_path / "feedback.json"
    main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
          "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
          "--critical-feedback-trials", "1", "--report", str(report)])
    data = json.loads(report.read_text())
    assert seen == [1] and data["critical_placement_accepted_moves"] == 0
    assert data["complete"] and not data["fabrication_ready"]


def test_feedback_interruption_retains_baseline_and_proposed_pose(tmp_path, monkeypatch):
    import pcbir.critical_preflight as preflight

    def interrupt(board, global_route, critical, *, on_trial_started, **kwargs):
        on_trial_started(1, board.placements[0].reference, board)
        raise ValueError("trial interrupted before global completion")

    monkeypatch.setattr(preflight, "improve_critical_placement", interrupt)
    report = tmp_path / "feedback.json"
    assert main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
          "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
          "--critical-feedback-trials", "1", "--report", str(report)]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "critical_placement_trial_running" and not data["complete"]
    assert "critical_baseline" in data and "critical" not in data
    assert data["critical_placement_running"]["index"] == 1
    assert data["critical_placement_feedback"] == [] and not data["fabrication_ready"]


def test_preflight_rejects_negative_feedback_before_loading(tmp_path):
    assert main([str(tmp_path / "does-not-exist.copper"), "--critical-feedback-trials", "-1",
                 "--report", str(tmp_path / "report.json")]) == 2


def test_preflight_physicalizes_the_mechanical_block(tmp_path, monkeypatch) -> None:
    import pcbir.critical_preflight as preflight
    from pcbir import nm_from_mm

    board_file = tmp_path / "board.copper"
    board_file.write_text("""board Stack {
        mechanical {
            outline rectangle { width = 30mm; height = 20mm; }
            stackup {
                copper F.Cu { thickness = 0.035mm; }
                dielectric P1 { thickness = 0.2mm; er = 4.3; }
                copper In1.Cu { thickness = 0.0175mm; }
                dielectric C1 { thickness = 1.065mm; er = 4.6; }
                copper In2.Cu { thickness = 0.0175mm; }
                dielectric P2 { thickness = 0.2mm; er = 4.3; }
                copper B.Cu { thickness = 0.035mm; }
            }
        }
    }""")
    seen = []

    def capture(board, *args, **kwargs):
        seen.append(board)
        raise ValueError("stop after physicalization")

    monkeypatch.setattr(preflight, "optimize_placement_for_routing", capture)
    assert main([str(board_file), "--allow-proxy-footprints", "--layers", "4",
                 "--fab-profile", "generic", "--report", str(tmp_path / "report.json")]) == 2
    (board,) = seen
    # The declared outline and stack-up reach the routed board, not defaults.
    assert max(point.x_nm for point in board.outline.vertices) == nm_from_mm("30")
    assert max(point.y_nm for point in board.outline.vertices) == nm_from_mm("20")
    assert len(board.stackup.physical_layers) == 7

from pathlib import Path
import json

from pcbir.critical_preflight import main


ROOT = Path(__file__).resolve().parents[1]


def test_preflight_exports_partial_artifact_and_never_claims_full_signoff(tmp_path) -> None:
    report = tmp_path / "preflight.json"
    output = tmp_path / "preflight.kicad_pcb"
    code = main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
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

    def interrupt(*args):
        raise ValueError("critical search interrupted")

    monkeypatch.setattr(preflight, "route_critical_nets", interrupt)
    report = tmp_path / "checkpoint.json"
    assert main([
        str(ROOT / "examples" / "valid_board.copper"), "--allow-proxy-footprints",
        "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
        "--report", str(report),
    ]) == 2
    data = json.loads(report.read_text())
    assert data["stage"] == "global_complete"
    assert data["complete"] is False and data["fabrication_ready"] is False
    assert "global_route" in data and "critical" not in data

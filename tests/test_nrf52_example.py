from dataclasses import replace
import json
from pathlib import Path
import subprocess

import pytest

from pcbir import check, compile_file
from pcbir.drc import PhysicalDrcPolicy, run_physical_drc
from examples.nrf52_example import SOURCE, make_example
from pcbir.physical import PadReference, Point
from pcbir.placement import placement_solution_is_legal

ROOT = Path(__file__).resolve().parents[1]


def endpoints(board, name):
    net = next(n for n in board.nets if n.name == name)
    return {(p.component, p.pin) for p in net.endpoints}


def test_coin_cell_example_is_electrically_complete_and_uses_real_macro_roles():
    board = compile_file(SOURCE, locked=True, offline=True)
    assert check(board) == []
    assert len(board.components) == 27
    assert endpoints(board, "VBAT") >= {("BT1", "POS"), ("U_NRF", "VDD_1"),
        ("U_NRF", "VDD_2"), ("U_NRF", "VDD_3"), ("J_SWD", "VREF")}
    assert endpoints(board, "GND") >= {("BT1", "NEG"), ("J_SWD", "GND_DETECT"),
        ("X1", "CASE_2"), ("X1", "CASE_4")}
    for number, pin in ((1, "P0_11"), (2, "P0_12")):
        assert endpoints(board, f"BUTTON{number}") == {
            ("U_NRF", pin), (f"SW_{number}", "A"), (f"R_BUTTON{number}", "2")}
    for number, pin in ((1, "P0_13"), (2, "P0_14")):
        assert endpoints(board, f"LED{number}") == {("U_NRF", pin), (f"R_LED{number}", "1")}
        assert endpoints(board, f"LED{number}_A") == {(f"R_LED{number}", "2"), (f"LED_{number}", "A")}
    assert endpoints(board, "NRF_RF_RAW") == {("U_NRF", "ANT"), ("C_BT_MATCH", "2"), ("L_BT_MATCH", "1")}
    assert endpoints(board, "ANT_FEED") == {("L_ANT_SERIES", "2"), ("ANT_BT", "FEED")}
    all_endpoints = set().union(*(endpoints(board, net.name) for net in board.nets))
    assert not {("U_NRF", "DCC"), ("U_NRF", "DEC2"), ("U_NRF", "NC_44"),
                ("ANT_BT", "NC"), ("J_SWD", "NC_8")} & all_endpoints
    assert {pin.name: pin.number for pin in board.library["nrf52_support.REFERENCE_LED_0603"].pins.values()} == {"K": "1", "A": "2"}


def test_battery_presence_is_explicit_not_inferred_from_passive_holder():
    board = compile_file(SOURCE, locked=True, offline=True)
    board = replace(board, supplies=tuple(s for s in board.supplies if s.net != "VBAT"))
    assert any(d.code == "UNSOURCED_POWER_INPUT" for d in check(board))


def installed_roots():
    roots = (Path("C:/Program Files/KiCad/10.0/share/kicad/footprints"),)
    if not all(p.is_dir() for p in roots):
        pytest.skip("optional installed KiCad footprints")
    return roots


def test_placed_example_preserves_macro_and_reports_unrouted_circuit():
    board = make_example(installed_roots())
    assert placement_solution_is_legal(board, {p.reference: p for p in board.placements})
    assert len(board.tracks) == 20 and len(board.vias) == 2
    assert board.materialized_macros == ("nordic-antenna-trial",)
    antenna = next(p for p in board.placements if p.reference == "ANT_BT")
    assert antenna.position == Point.mm(48.5, 3.25)
    assert all(PadReference("ANT_BT", "2") not in n.pads for n in board.nets)
    report = run_physical_drc(board, policy=PhysicalDrcPolicy(require_completed_detailed_route=False))
    assert report.findings and {f.code for f in report.findings} == {"DRC-OPEN-NET"}
    assert board.metadata["fabrication_ready"] == "false"


def test_coin_cell_export_has_locked_macro_no_kicad_geometry_violations(tmp_path):
    from pcbir.backends.kicad_pcb import KiCadPcbBackend
    from pcbir.backends.kicad_project import write_kicad_project
    board = make_example(installed_roots())
    path = tmp_path / "example.kicad_pcb"
    write_kicad_project(KiCadPcbBackend().generate(board), path)
    assert path.read_text(encoding="utf-8").count("(locked yes)") == 29
    cli = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        return
    report_path = tmp_path / "drc.json"
    subprocess.run([str(cli), "pcb", "drc", "--format", "json", "-o", str(report_path), str(path)],
                   check=True, capture_output=True)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert not report["violations"]
    assert report["unconnected_items"]  # Non-RF routing is deliberately not claimed.


def test_actual_package_access_and_battery_ground_no_longer_block():
    from pcbir.fanout import route_fanout
    from pcbir.plane import stitch_zone_pads
    from pcbir.routing import route_global, GlobalRoutingStatus
    board = make_example(installed_roots(),offline=True)
    assert {rule.pad for rule in board.via_in_pad_rules} == {PadReference("BT1","2")}
    assert route_global(board).status is GlobalRoutingStatus.SUCCESS
    fanout = route_fanout(board)
    assert not fanout.pending_pads
    assert {PadReference("U_NRF","26"),PadReference("U_NRF","33")} <= set(fanout.accesses)
    stitched = stitch_zone_pads(fanout.board)
    assert not stitched.pending_pads
    capped = [v for v in stitched.board.vias if v.finish == "filled-capped"]
    assert len(capped) == 1 and capped[0].position == Point.mm(15,25)
    assert not {f.code for f in run_physical_drc(stitched.board).findings} & {
        "DRC-SHORT","DRC-CLEARANCE","DRC-VIA-PAD-OVERLAP","DRC-HOLE-CLEARANCE"}


def test_example_cli_retains_incomplete_signoff_and_performance_profile(tmp_path):
    from examples.nrf52_example import main
    args = ["--output-dir", str(tmp_path)]
    for root in installed_roots():
        args.extend(("--footprint-root", str(root)))
    assert main(args) == 0  # Successfully generated an explicitly placed draft.
    report = json.loads((tmp_path / "drc.json").read_text(encoding="utf-8"))
    assert report["decision"] == "fail" and report["completeness"] == "incomplete"
    assert "DRC-ROUTE-INCOMPLETE" in {f["code"] for f in report["findings"]}
    assert json.loads((tmp_path / "erc.json").read_text(encoding="utf-8"))["findings"] == []
    assert (tmp_path / "profile.pstats").is_file()
    assert "make_example" in (tmp_path / "profile.txt").read_text(encoding="utf-8")

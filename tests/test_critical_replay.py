from dataclasses import replace
import json

import pytest

from pcbir import route_global, route_critical_nets, TrackSegment, Point, CopperLayer
from pcbir.cli import main
from pcbir.physical import NetRoutingRule, RouteKind, PhysicalNet, PadReference, Placement
from pcbir.workbench import save_state
from test_critical_routing import _pair_board


def test_selected_jobs_keep_all_rules_and_reject_unowned_input_copper():
    board = _pair_board()
    footprint = next(iter(board.footprints))
    track = TrackSegment("CLK", Point.mm(8, 4), Point.mm(30, 4), 250000, CopperLayer.FRONT)
    board = replace(board,
        placements=(*board.placements, Placement("C1", footprint, Point.mm(8, "4.5")),
                    Placement("C2", footprint, Point.mm(30, "4.5"))),
        nets=(*board.nets, PhysicalNet("CLK", (PadReference("C1", "1"), PadReference("C2", "1")))),
        net_routing_rules=(*board.net_routing_rules,
            NetRoutingRule("CLK", kind=RouteKind.CLOCK, allowed_layers=(CopperLayer.FRONT,), max_vias=0)))
    guides = route_global(board)
    result = route_critical_nets(board, guides, only_nets=frozenset({"USB_DP"}))
    assert [item.nets for item in result.nets] == [("USB_DM", "USB_DP")]
    assert result.nets[0].connected
    assert board.tracks == ()
    assert result.board.nets == board.nets
    with pytest.raises(ValueError, match="no copper except immutable"):
        route_critical_nets(replace(board, tracks=(track,)), guides, only_nets=frozenset({"USB_DP"}))
    assert result.board.net_routing_rules == board.net_routing_rules
    with pytest.raises(ValueError, match="unknown critical nets"):
        route_critical_nets(board, guides, only_nets=frozenset({"missing"}))


def test_critical_replay_cli_is_explicitly_diagnostic_and_writes_locatable_report(tmp_path):
    board = _pair_board()
    state = tmp_path / "inputs.state.json"
    save_state(state, {"board": board, "global_route": route_global(board)})
    report, output = tmp_path / "route-report.json", tmp_path / "board.kicad_pcb"
    assert main(["replay-critical", str(state), "--only", "USB_DP", "--report", str(report),
                 "-o", str(output)]) == 0
    document = json.loads(report.read_text())
    assert document["fabrication_ready"] is False
    assert document["native_verification"] == "not_run"
    assert document["pair_state_limit"] == 6000
    assert len(document["critical"]["nets"]) == 1
    assert document["critical"]["nets"][0]["connected"]
    assert output.is_file()
    assert main(["replay-critical", str(state), "--only", "missing"]) == 2
    with pytest.raises(SystemExit):
        main(["replay-critical", str(state), "--pair-states", "-1"])


def test_unified_clock_fallback_keeps_the_real_clock_profile(monkeypatch):
    import pcbir.detailed as detailed
    from pcbir.critical import _route_single_exact
    from pcbir.unified_router import UnifiedRouterOptions
    board = _pair_board()
    rule = NetRoutingRule("USB_DP", kind=RouteKind.CLOCK, width_nm=250000,
        allowed_layers=(CopperLayer.FRONT,), max_vias=0, max_length_nm=40000000)
    board = replace(board, net_routing_rules=(rule,))
    guides = route_global(board)
    def forbidden(*args, **kwargs):
        raise AssertionError("unified clock fallback used the legacy detailed engine")
    monkeypatch.setattr(detailed, "route_detailed", forbidden)
    result, tracks, vias = _route_single_exact(board, rule, guides, [], [],
        unified_options=UnifiedRouterOptions(time_budget_s=10))
    assert result.connected and result.strategy == "unified_clock"
    assert tracks and not vias and all(t.layer is CopperLayer.FRONT for t in tracks)
    assert sum(abs(t.end.x_nm-t.start.x_nm)+abs(t.end.y_nm-t.start.y_nm) for t in tracks) <= rule.max_length_nm
    assert board.tracks == () and board.net_routing_rules == (rule,)


def test_failed_critical_replay_is_located_for_issues_and_view(tmp_path):
    from test_local_pair_branches import support_fixture
    from pcbir.workbench import load_run, run_issues
    board = support_fixture()
    board = replace(board, placements=tuple(replace(p, position=Point.mm(20, 4))
        if p.reference == "SUP_P" else p for p in board.placements))
    guides = route_global(board)
    guides = replace(guides, routes=tuple(replace(g, connected=False) for g in guides.routes))
    state = tmp_path / "inputs.state.json"
    save_state(state, {"board": board, "global_route": guides})
    assert main(["replay-critical", str(state), "--report", str(tmp_path/"route-report.json"),
                 "-o", str(tmp_path/"board.kicad_pcb")]) == 0
    failures = [i for i in run_issues(load_run(tmp_path)) if i.stage == "critical"]
    assert len(failures) == 1 and failures[0].id == "C1"
    assert set(failures[0].nets) == {"USB_DM", "USB_DP"}
    assert failures[0].window() is not None and "unsupported" in failures[0].title


def test_port_inventory_is_bounded_and_outside_routing_identity():
    from pcbir.critical import _fingerprint
    from pcbir.pair_search import PairPortExample, PairPortInventory, PairSearchStats, paired_candidates
    board = _pair_board()
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    original = result.nets[0]
    stats = PairSearchStats()
    routes = {route.net: route for route in guides.routes}
    tuple(paired_candidates(board, *board.net_routing_rules, routes["USB_DP"], routes["USB_DM"],
                           stats=stats, maximum_states=100, maximum_searches=1))
    assert stats.port_inventory
    assert len(stats.port_inventory) <= 24
    assert all(len(port.examples) <= 16 and port.departable_count <= port.port_count
               for port in stats.port_inventory)
    diagnostic = PairPortInventory("J1", CopperLayer.FRONT, 250000, 1, 1,
        (PairPortExample(Point.mm(5, 12), 0, 1, True),))
    reported = replace(original, pair_ports=(diagnostic,))
    assert reported == original
    assert _fingerprint("guide", [], [], [reported]) == _fingerprint("guide", [], [], [original])

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


@pytest.mark.parametrize("kind", (RouteKind.CLOCK, RouteKind.CRITICAL))
@pytest.mark.parametrize("length_limit", (1000000, 40000000))
def test_unified_single_fallback_keeps_the_real_source_profile(monkeypatch, kind, length_limit):
    import pcbir.detailed as detailed
    from pcbir.critical import _route_single_exact
    from pcbir.unified_router import UnifiedRouterOptions
    board = _pair_board()
    rule = NetRoutingRule("USB_DP", kind=kind, width_nm=250000,
        allowed_layers=(CopperLayer.FRONT,), max_vias=0, max_length_nm=length_limit)
    board = replace(board, net_routing_rules=(rule,))
    guides = route_global(board)
    def forbidden(*args, **kwargs):
        raise AssertionError("selected unified fallback used the legacy detailed engine")
    monkeypatch.setattr(detailed, "route_detailed", forbidden)
    result, tracks, vias = _route_single_exact(board, rule, guides, [], [],
        unified_options=UnifiedRouterOptions(time_budget_s=10))
    assert result.strategy == (
        "unified_clock" if kind is RouteKind.CLOCK else "unified_critical")
    assert result.connected is (length_limit == 40000000)
    if result.connected:
        assert tracks and all(t.layer is CopperLayer.FRONT for t in tracks)
    else:
        assert tracks == ()
    assert not vias
    assert sum(abs(t.end.x_nm-t.start.x_nm)+abs(t.end.y_nm-t.start.y_nm) for t in tracks) <= rule.max_length_nm
    assert board.tracks == () and board.net_routing_rules == (rule,)


def test_unified_critical_fallback_connects_every_tree_terminal(monkeypatch):
    import pcbir.detailed as detailed
    from pcbir.critical import _route_single_exact
    from pcbir.unified_router import UnifiedRouterOptions
    from test_local_critical import fixture, hard_findings

    board, guides = fixture(tree=True)
    rule = board.net_routing_rules[0]
    def forbidden(*args, **kwargs):
        raise AssertionError("critical tree fell back to the legacy engine")
    monkeypatch.setattr(detailed, "route_detailed", forbidden)
    result, tracks, vias = _route_single_exact(board, rule, guides, [], [],
        unified_options=UnifiedRouterOptions(time_budget_s=10))
    assert result.connected and result.strategy == "unified_critical"
    assert tracks and not vias
    assert not hard_findings(replace(board, tracks=tracks, vias=vias))
    assert board.tracks == () and len(board.nets[0].pads) == 3


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


def _single_critical_board(length_limit=40000000):
    board = _pair_board()
    return replace(board, net_routing_rules=tuple(
        NetRoutingRule(name, kind=RouteKind.CRITICAL, priority=80, width_nm=250000,
            allowed_layers=(CopperLayer.FRONT,), max_vias=0,
            max_length_nm=length_limit if name == "USB_DM" else 40000000)
        for name in ("USB_DM", "USB_DP")))


@pytest.mark.parametrize("length_limit", (1000000, 40000000))
def test_critical_cohort_uses_shared_engine_and_preserves_profile(monkeypatch, length_limit):
    import pcbir.unified_router as unified
    from test_local_critical import hard_findings
    board = _single_critical_board(length_limit)
    calls = []
    original = unified.route_unified
    def record(search_board, guide, options, **kwargs):
        calls.append(kwargs["only_nets"])
        assert search_board.net_routing_rules == board.net_routing_rules
        return original(search_board, guide, options, **kwargs)
    monkeypatch.setattr(unified, "route_unified", record)
    result = route_critical_nets(board, route_global(board),
        unified_options=unified.UnifiedRouterOptions(time_budget_s=10))
    assert calls == ([frozenset({"USB_DM", "USB_DP"})]
        + ([frozenset({"USB_DM"})] if length_limit == 1000000 else []))
    outcomes = {item.nets[0]: item for item in result.nets}
    assert outcomes["USB_DP"].connected
    assert outcomes["USB_DM"].connected is (length_limit == 40000000)
    assert all(item.strategy == "unified_critical_cohort" for item in result.nets)
    assert not result.board.vias
    assert all(t.layer is CopperLayer.FRONT for t in result.board.tracks)
    if length_limit == 1000000:
        assert all(t.net != "USB_DM" for t in result.board.tracks)
    else:
        assert not hard_findings(result.board)
    assert board.tracks == ()


@pytest.mark.parametrize("kind", (RouteKind.CRITICAL, RouteKind.CLOCK))
def test_failed_cohort_owner_retries_without_reopening_accepted_peer(monkeypatch, kind):
    import pcbir.unified_router as unified
    import pcbir.detailed as detailed
    from test_local_critical import hard_findings
    board = _single_critical_board()
    board = replace(board, net_routing_rules=tuple(replace(r, kind=kind)
        for r in board.net_routing_rules))
    original = unified.route_unified
    accepted = []
    calls = []
    def bounded_miss(search_board, guide, options, **kwargs):
        selected = kwargs["only_nets"]
        calls.append(selected)
        if len(selected) == 1:
            assert search_board.tracks == tuple(accepted)
        result = original(search_board, guide, options, **kwargs)
        if len(selected) > 1:
            accepted.extend(t for t in result.board.tracks if t.net == "USB_DP")
            result = replace(result,
                board=replace(result.board, tracks=tuple(accepted)),
                nets=tuple(replace(n, connected=False, track_count=0, length_nm=0)
                    if n.net == "USB_DM" else n for n in result.nets))
        return result
    def forbidden(*args, **kwargs):
        raise AssertionError("cohort retry used legacy routing")
    monkeypatch.setattr(unified, "route_unified", bounded_miss)
    monkeypatch.setattr(detailed, "route_detailed", forbidden)
    result = route_critical_nets(board, route_global(board),
        unified_options=unified.UnifiedRouterOptions(time_budget_s=10))
    assert calls == [frozenset({"USB_DM", "USB_DP"}), frozenset({"USB_DM"})]
    assert all(n.connected for n in result.nets)
    assert next(n for n in result.nets if n.nets == ("USB_DM",)).strategy == "unified_critical_cohort_retry"
    assert tuple(t for t in result.board.tracks if t.net == "USB_DP") == tuple(accepted)
    assert not hard_findings(result.board)
    assert not board.tracks and not result.board.vias


@pytest.mark.parametrize("damage", ("prefix", "owner"))
def test_critical_cohort_rejects_changed_prefix_or_foreign_owner(monkeypatch, damage):
    import pcbir.unified_router as unified
    from pcbir.critical import _route_critical_cohort
    board = _single_critical_board()
    prefix = TrackSegment("USB_DP", Point.mm(5, "11.5"), Point.mm(6, "11.5"),
        250000, CopperLayer.FRONT)
    board = replace(board, nets=(*board.nets, PhysicalNet("foreign", ())))
    original = unified.route_unified
    def corrupt(search_board, guide, options, **kwargs):
        result = original(search_board, guide, options, **kwargs)
        tracks = result.board.tracks
        if damage == "prefix":
            tracks = tracks[1:]
        else:
            tracks = (*tracks, replace(prefix, net="foreign"))
        return replace(result, board=replace(result.board, tracks=tracks))
    monkeypatch.setattr(unified, "route_unified", corrupt)
    results = _route_critical_cohort(board, board.net_routing_rules, route_global(board),
        [prefix], [], unified.UnifiedRouterOptions(time_budget_s=10))
    assert all(not result.connected and not tracks and not vias
               for result, tracks, vias in results.values())
    assert all("changed input copper" in result.diagnostics[-1]
               for result, _, _ in results.values())
    assert all(result.strategy == "unified_critical_cohort_integrity_rejected"
               for result, _, _ in results.values())


def test_corrupt_cohort_is_not_hidden_by_a_single_net_retry(monkeypatch):
    import pcbir.unified_router as unified
    board = _single_critical_board()
    board = replace(board, nets=(*board.nets, PhysicalNet("foreign", ())))
    original = unified.route_unified
    calls = []
    def corrupt(search_board, guide, options, **kwargs):
        calls.append(kwargs["only_nets"])
        result = original(search_board, guide, options, **kwargs)
        foreign = replace(result.board.tracks[0], net="foreign")
        return replace(result, board=replace(result.board,
            tracks=(*result.board.tracks, foreign)))
    monkeypatch.setattr(unified, "route_unified", corrupt)
    result = route_critical_nets(board, route_global(board),
        unified_options=unified.UnifiedRouterOptions(time_budget_s=10))
    assert calls == [frozenset({"USB_DM", "USB_DP"})]
    assert all(not n.connected for n in result.nets)
    assert result.board.tracks == board.tracks == ()


@pytest.mark.parametrize("first_kind", (RouteKind.CRITICAL, RouteKind.CLOCK))
def test_clock_and_data_priorities_negotiate_together(monkeypatch, first_kind):
    import pcbir.unified_router as unified
    board = _single_critical_board()
    board = replace(board, net_routing_rules=(replace(board.net_routing_rules[0],kind=first_kind),
        replace(board.net_routing_rules[1], priority=70)))
    calls=[]
    original=unified.route_unified
    def record(board,guide,options,**kwargs):
        calls.append(kwargs["only_nets"])
        return original(board,guide,options,**kwargs)
    monkeypatch.setattr(unified,"route_unified",record)
    result = route_critical_nets(board, route_global(board),
        unified_options=unified.UnifiedRouterOptions(time_budget_s=10))
    assert calls == [frozenset({"USB_DM","USB_DP"})]
    assert all(item.connected for item in result.nets)
    assert all(item.strategy == "unified_critical_cohort" for item in result.nets)


@pytest.mark.parametrize("retry", (False, True))
def test_shared_clock_cohort_still_tunes_declared_matching_group(monkeypatch, retry):
    import pcbir.unified_router as unified
    from pcbir.physical import NetMatchGroup
    from pcbir import run_physical_drc
    from pcbir.unified_router import UnifiedRouterOptions
    board = _single_critical_board()
    source_fp = next(iter(board.footprints.values()))
    target_fp = replace(source_fp, name="offset-target", pads=tuple(
        replace(p, position=Point.mm(-4, "0.5")) if p.number == "2" else p
        for p in source_fp.pads))
    board = replace(board, footprints={**board.footprints,target_fp.name:target_fp},
        placements=tuple(replace(p,footprint=target_fp.name) if p.reference=="J2" else p
            for p in board.placements),
        net_routing_rules=tuple(replace(r,kind=RouteKind.CLOCK,
            priority=80 if r.net=="USB_DM" else 70,tuning_amplitude_limit_nm=1000000)
            for r in board.net_routing_rules),
        match_groups=(NetMatchGroup("matched-clocks",("USB_DM","USB_DP"),1000000),))
    if retry:
        original = unified.route_unified
        def miss(search_board, guide, options, **kwargs):
            result = original(search_board, guide, options, **kwargs)
            if len(kwargs["only_nets"]) > 1:
                result = replace(result, board=replace(result.board,
                    tracks=tuple(t for t in result.board.tracks if t.net != "USB_DM")),
                    nets=tuple(replace(n, connected=False, track_count=0, length_nm=0)
                        if n.net == "USB_DM" else n for n in result.nets))
            return result
        monkeypatch.setattr(unified, "route_unified", miss)
    result = route_critical_nets(board,route_global(board),
        unified_options=UnifiedRouterOptions(time_budget_s=10))
    assert all(n.connected for n in result.nets)
    assert not [f for f in run_physical_drc(result.board).findings
                if f.severity.value=="error" and f.code!="DRC-ROUTE-INCOMPLETE"]
    lengths=[n.lengths_nm[0] for n in result.nets]
    assert max(lengths)-min(lengths)<=1000000
    assert board.tracks==() and result.board.match_groups==board.match_groups

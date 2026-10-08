"""D-PHY plan W1: rejection reasons, tracebacks, coarse transitions, lane table."""
from __future__ import annotations

from dataclasses import replace
from decimal import Decimal
import json
from math import sqrt
from pathlib import Path

from pcbir import (
    ComponentPlacementRule, CopperKeepout, CopperLayer, GlobalRouterOptions, GlobalViaProposal,
    NetRoutingRule, PhysicalNet, Point, PolygonRing, PolygonWithHoles, RouteKind, Stackup,
    StackupLayer, StackupLayerKind, TrackSegment, Via, nm_from_mm, route_critical_nets,
    route_global, run_physical_drc,
)
from pcbir.critical import (
    CriticalNetResult, _budget_diagnostics, _reject_reserved_plane_tracks, _rejection_gate,
    _rejection_summary, _route_pair, critical_lane_table,
)
from pcbir.engineering import propagation_delay
from pcbir.progress import critical_progress

from test_critical_routing import _pair_board

ROOT = Path(__file__).resolve().parents[1]


def _bent_pair_board(**profile):
    """J2 rotated a quarter turn: the joint lanes bend, so they differ in length."""
    board = _pair_board()
    board = replace(
        board,
        placements=(board.placements[0], replace(board.placements[1], position=Point.mm(20, 20),
                                                 rotation_degrees=90)),
        placement_rules=(ComponentPlacementRule("J2", allowed_orientations=(90,)),),
    )
    return replace(board, net_routing_rules=tuple(
        replace(rule, maximum_uncoupled_length_nm=nm_from_mm(10), **profile)
        for rule in board.net_routing_rules))


# D1 ---------------------------------------------------------------------------

def test_every_candidate_gate_has_a_named_first_failing_reason() -> None:
    rule = NetRoutingRule("A", RouteKind.CRITICAL, max_vias=0, max_length_nm=nm_from_mm(1),
                          allowed_layers=(CopperLayer.FRONT,))
    track = TrackSegment("A", Point.mm(0, 0), Point.mm(5, 0), nm_from_mm("0.25"), CopperLayer.BACK)
    via = Via("A", Point.mm(5, 0), nm_from_mm("0.8"), nm_from_mm("0.4"),
              CopperLayer.FRONT, CopperLayer.BACK)
    length, vias = _budget_diagnostics(rule, (track,), (via,))
    assert _rejection_gate((length,))[0] == "length_budget"
    assert _rejection_gate((vias,))[0] == "via_budget"
    board = _pair_board()
    empty = CriticalNetResult(("A",), True, 1, 0, (), 0)
    plane, _, _ = _reject_reserved_plane_tracks(board, empty, (track,), (), {"A": rule})
    assert _rejection_gate(plane.diagnostics) == ("plane_reservation", plane.diagnostics[0])

    first, second = sorted(board.net_routing_rules, key=lambda item: item.net)
    tight = [replace(item, max_skew_nm=nm_from_mm("0.1"), maximum_uncoupled_length_nm=nm_from_mm(1))
             for item in (first, second)]
    width = nm_from_mm("0.25")
    lanes = ((TrackSegment(first.net, Point.mm(5, "12.5"), Point.mm(30, "12.5"), width, CopperLayer.FRONT),),
             (TrackSegment(second.net, Point.mm(5, "11.5"), Point.mm(35, "11.5"), width, CopperLayer.FRONT),))
    guides = {route.net: route for route in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))).routes}
    skewed, _, _ = _route_pair(board, *tight, guides, exact_tracks=lanes)
    # The skew gate is evaluated before the uncoupled-length gate.
    assert [_rejection_gate((item,))[0] for item in skewed.diagnostics] == ["skew", "uncoupled_length"]
    assert _rejection_gate(skewed.diagnostics)[0] == "skew"
    unpaired, _, _ = _route_pair(board, first, second, guides, exact_tracks=lanes,
                                 exact_via_pairs=((replace(via, net=first.net),
                                                   replace(via, net=second.net)),))
    assert "via_pairing" in {_rejection_gate((item,))[0] for item in unpaired.diagnostics}
    returns = [replace(item, require_return_vias=True, return_via_net="GND",
                       maximum_return_via_distance_nm=nm_from_mm(1)) for item in (first, second)]
    missing, _, _ = _route_pair(board, *returns, guides, exact_tracks=lanes,
                                exact_via_pairs=((replace(via, net=first.net),
                                                  replace(via, net=second.net)),))
    assert "return_via" in {_rejection_gate((item,))[0] for item in missing.diagnostics}

    # Native DRC codes are reported verbatim. An open pair net is the
    # connectivity gate, reported only when no geometric gate failed.
    assert _rejection_gate(("DRC-CLEARANCE: tracks 1 and 2",)) == (
        "DRC-CLEARANCE", "DRC-CLEARANCE: tracks 1 and 2")
    assert _rejection_gate(("DRC-OPEN-NET: A is open", "DRC-SHORT: tracks 3 and 4"))[0] == "DRC-SHORT"
    assert _rejection_gate(("DRC-OPEN-NET: A is open",)) == ("connectivity", "DRC-OPEN-NET: A is open")
    assert _rejection_gate(("hard-macro copper cannot be modified",))[0] == "hard_macro"


def test_rejection_summary_is_counted_ordered_and_bounded() -> None:
    rejected = [("skew", "pair skew 5 nm exceeds 1 nm"), ("DRC-CLEARANCE", "DRC-CLEARANCE: a"),
                ("DRC-CLEARANCE", "DRC-CLEARANCE: b"), ("connectivity", "DRC-OPEN-NET: x"),
                ("DRC-CLEARANCE", "DRC-CLEARANCE: a"), ("skew", "pair skew 6 nm exceeds 1 nm")]
    summary, examples = _rejection_summary(rejected)
    assert summary == (("DRC-CLEARANCE", 3), ("skew", 2), ("connectivity", 1))
    # One example per gate first, in summary order; never more than three.
    assert examples == ("DRC-CLEARANCE: a", "pair skew 5 nm exceeds 1 nm", "DRC-OPEN-NET: x")
    assert _rejection_summary(rejected) == (summary, examples)
    single = _rejection_summary([("skew", "s1"), ("skew", "s2"), ("skew", "s1"), ("skew", "s3"),
                                 ("skew", "s4")])
    assert single == ((("skew", 5),), ("s1", "s2", "s3"))
    assert _rejection_summary([]) == ((), ())


def test_rejected_exact_pair_candidates_are_counted_in_report_and_progress() -> None:
    board = _bent_pair_board(max_skew_nm=nm_from_mm("0.1"))
    guides = route_global(board)
    events = []
    result = route_critical_nets(board, guides, pair_state_limit=2000,
                                 on_progress=critical_progress(lambda *event: events.append(event)))
    pair = result.nets[0]
    assert not pair.connected and pair.candidate_attempts > 0
    # Without tuning every exact candidate fails the skew gate first.
    assert pair.rejections == (("skew", pair.candidate_attempts),)
    assert 1 <= len(pair.rejection_examples) <= 3
    assert all(example.startswith("pair skew ") for example in pair.rejection_examples)
    assert pair.diagnostics[-1].endswith(f"none accepted (first-failing gates: skew={pair.candidate_attempts})")
    document = json.loads(result.to_json())["nets"][0]
    assert document["rejections"] == {"skew": pair.candidate_attempts}
    assert document["rejection_examples"] == list(pair.rejection_examples)
    finished = events[-1][2]
    assert finished["rejections"] == {"skew": pair.candidate_attempts}
    assert route_critical_nets(board, guides, pair_state_limit=2000) == result


def test_drc_rejections_are_recorded_even_when_a_later_candidate_is_accepted() -> None:
    # One 45-degree-cornered bump of at most 0.5 mm compensates the bend.
    board = _bent_pair_board(max_skew_nm=nm_from_mm("0.1"), tuning_amplitude_limit_nm=nm_from_mm("0.5"))
    guides = route_global(board)
    # A keep-out exactly where the surface candidates place their skew bump.
    site = CopperKeepout("bump-site", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm("6.3", "10.9"), Point.mm("7.5", "10.9"), Point.mm("7.5", "11.45"),
        Point.mm("6.3", "11.45")))))
    result = route_critical_nets(replace(board, copper_keepouts=(site,)), guides, pair_state_limit=2000)
    pair = result.nets[0]
    assert pair.connected
    assert dict(pair.rejections).get("DRC-COPPER-KEEPOUT", 0) > 0
    assert pair.rejection_examples[0].startswith("DRC-COPPER-KEEPOUT: ")
    assert not any(f.code == "DRC-COPPER-KEEPOUT" for f in run_physical_drc(result.board).findings)


# D2 ---------------------------------------------------------------------------

def test_preflight_debug_prints_the_full_traceback(tmp_path, monkeypatch, capsys) -> None:
    import pcbir.critical_preflight as preflight

    def explode(*args, **kwargs):
        raise ValueError("max() iterable argument is empty")

    monkeypatch.setattr(preflight, "route_critical_nets", explode)
    argv = [str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
            "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
            "--report", str(tmp_path / "report.json")]
    assert preflight.main(argv) == 2
    quiet = capsys.readouterr()
    assert "CRITICAL PREFLIGHT ERROR: max() iterable argument is empty" in quiet.out
    assert "Traceback" not in quiet.out + quiet.err
    assert preflight.main([*argv, "--debug"]) == 2
    loud = capsys.readouterr()
    assert "Traceback (most recent call last)" in loud.err
    assert "in explode" in loud.err
    assert "CRITICAL PREFLIGHT ERROR: max() iterable argument is empty" in loud.out


def test_route_board_debug_prints_the_full_traceback(monkeypatch, capsys) -> None:
    import pcbir.cli as cli

    def explode(*args, **kwargs):
        raise ValueError("max() iterable argument is empty")

    monkeypatch.setattr(cli, "run_routing_pipeline", explode)
    argv = ["route-board", str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints"]
    assert cli.main(argv) == 2
    quiet = capsys.readouterr()
    assert "ROUTING ERROR: max() iterable argument is empty" in quiet.out
    assert "Traceback" not in quiet.out + quiet.err
    assert cli.main([*argv, "--debug"]) == 2
    loud = capsys.readouterr()
    assert "Traceback (most recent call last)" in loud.err and "in explode" in loud.err
    assert "ROUTING ERROR: max() iterable argument is empty" in loud.out


def test_preflight_reports_lanes_and_rejections_from_the_critical_stage(tmp_path, monkeypatch, capsys):
    import pcbir.critical_preflight as preflight

    pair_board = _pair_board()
    routed = route_critical_nets(pair_board, route_global(
        pair_board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))))

    def critical(board, global_route, *, on_progress):
        rejected = replace(routed.nets[0], rejections=(("skew", 2),), rejection_examples=("pair skew",))
        on_progress("started", rejected.nets, None)
        on_progress("finished", rejected.nets, rejected)
        return routed

    monkeypatch.setattr(preflight, "route_critical_nets", critical)
    report = tmp_path / "report.json"
    preflight.main([str(ROOT / "examples/valid_board/board.copper"), "--allow-proxy-footprints",
                    "--layers", "2", "--fab-profile", "generic", "--router-iterations", "1",
                    "--report", str(report)])
    data = json.loads(report.read_text())
    assert data["critical_progress"][0]["result"]["rejections"] == {"skew": 2}
    lanes = {lane["net"]: lane for lane in data["critical"]["lanes"]}
    assert set(lanes) == {"USB_DM", "USB_DP"}
    assert lanes["USB_DP"]["delay_reason"] == "no stack-up declared"
    out = capsys.readouterr().out
    assert "lane USB_DP: " in out and "delay n/a (no stack-up declared)" in out


# D3 ---------------------------------------------------------------------------

def _with_guide_via(guides, net: str, position: Point):
    return replace(guides, routes=tuple(
        replace(route, vias=(GlobalViaProposal(route.net, position, CopperLayer.FRONT,
                                               CopperLayer.BACK, "test-via"),))
        if route.net == net else route for route in guides.routes))


def test_coarse_guide_emits_no_transitions_for_a_via_free_or_single_layer_pair() -> None:
    for profile in ({"max_vias": 0}, {"allowed_layers": (CopperLayer.FRONT,)}):
        board = _pair_board()
        board = replace(board, net_routing_rules=tuple(replace(rule, **profile)
                                                       for rule in board.net_routing_rules))
        guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
        guides = _with_guide_via(guides, "USB_DM", Point.mm(20, 12))
        proposed, tracks, vias = _route_pair(board, *board.net_routing_rules,
                                             {route.net: route for route in guides.routes})
        assert not proposed.connected and tracks == vias == ()
        assert proposed.diagnostics == (
            "coarse pair guide proposes 1 layer transition(s) but the pair profile allows none",)
        result = route_critical_nets(board, guides)
        assert result.nets[0].connected and result.locked_vias == ()


def test_coarse_guide_transition_beside_pads_is_never_emitted() -> None:
    board = _pair_board()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    # Next to J1's lands: the offset signal vias would overlap the pads.
    guides = _with_guide_via(guides, "USB_DM", Point.mm("5.5", 12))
    proposed, tracks, vias = _route_pair(board, *board.net_routing_rules,
                                         {route.net: route for route in guides.routes})
    assert not proposed.connected and tracks == vias == ()
    assert proposed.diagnostics == (
        "coarse guide transition for USB_DP at (5275000, 12000000) nm violates pad clearance "
        "or a via keep-out",)
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected
    assert not {f.code for f in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-HOLE-CLEARANCE"}


def test_coarse_guide_transition_in_a_via_keepout_is_never_emitted() -> None:
    board = _pair_board()
    keepout = CopperKeepout("via-fence", (CopperLayer.FRONT, CopperLayer.BACK), PolygonWithHoles(
        PolygonRing((Point.mm(19, 11), Point.mm(21, 11), Point.mm(21, 13), Point.mm(19, 13)))),
        block_tracks=False, block_vias=True)
    board = replace(board, copper_keepouts=(keepout,))
    guides = _with_guide_via(route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))),
                             "USB_DM", Point.mm(20, 12))
    proposed, _, vias = _route_pair(board, *board.net_routing_rules,
                                    {route.net: route for route in guides.routes})
    assert vias == () and "violates pad clearance or a via keep-out" in proposed.diagnostics[0]


# D4 ---------------------------------------------------------------------------

def test_lane_table_lists_length_vias_and_layers_without_inventing_delay() -> None:
    board = _pair_board()
    result = route_critical_nets(board, route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5"))))
    lanes = json.loads(result.to_json())["lanes"]
    assert [lane["net"] for lane in lanes] == ["USB_DM", "USB_DP"]
    for lane, length in zip(lanes, result.nets[0].lengths_nm):
        assert lane["group"] == ["USB_DM", "USB_DP"] and lane["connected"]
        assert lane["routed_length_nm"] == length > 0
        assert lane["layers"] == ["F.Cu"] and lane["layer_lengths_nm"] == {"F.Cu": length}
        assert lane["via_count"] == 0
        assert lane["estimated_delay_ps"] is None and lane["delay_evidence_grade"] is None
        assert lane["delay_reason"] == "no stack-up declared"


def _four_layer_stackup(prepreg_er: Decimal | None = Decimal("4.1")) -> Stackup:
    copper = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)
    layers = (
        StackupLayer("F.Cu", StackupLayerKind.COPPER, nm_from_mm("0.035"), CopperLayer.FRONT),
        StackupLayer("P1", StackupLayerKind.DIELECTRIC, nm_from_mm("0.1"), relative_permittivity=prepreg_er),
        StackupLayer("In1.Cu", StackupLayerKind.COPPER, nm_from_mm("0.0175"), CopperLayer.INTERNAL_1),
        StackupLayer("C1", StackupLayerKind.DIELECTRIC, nm_from_mm("1.2"), relative_permittivity=Decimal("4.5")),
        StackupLayer("In2.Cu", StackupLayerKind.COPPER, nm_from_mm("0.0175"), CopperLayer.INTERNAL_2),
        StackupLayer("P2", StackupLayerKind.DIELECTRIC, nm_from_mm("0.1"), relative_permittivity=Decimal("4.1")),
        StackupLayer("B.Cu", StackupLayerKind.COPPER, nm_from_mm("0.035"), CopperLayer.BACK),
    )
    return Stackup(copper, sum(layer.thickness_nm for layer in layers), layers)


def test_lane_table_estimates_screening_delay_from_the_declared_stackup() -> None:
    width = nm_from_mm("0.25")
    tracks = (
        TrackSegment("USB_DP", Point.mm(5, "11.5"), Point.mm(10, "11.5"), width, CopperLayer.FRONT),
        TrackSegment("USB_DP", Point.mm(10, "11.5"), Point.mm(30, "11.5"), width, CopperLayer.INTERNAL_1),
    )
    via = Via("USB_DP", Point.mm(10, "11.5"), nm_from_mm("0.8"), nm_from_mm("0.4"),
              CopperLayer.FRONT, CopperLayer.BACK)
    gnd = Via("GND", Point.mm(10, 10), nm_from_mm("0.8"), nm_from_mm("0.4"),
              CopperLayer.FRONT, CopperLayer.BACK)
    board = _pair_board()
    board = replace(board, stackup=_four_layer_stackup(), nets=(*board.nets, PhysicalNet("GND", ())),
                    tracks=tracks, vias=(via, gnd))
    pair = CriticalNetResult(("USB_DM", "USB_DP"), True, 2, 1, (0, nm_from_mm(25)), 0)
    batch = CriticalNetResult(("<critical-batch>",), False, 0, 0, (), 0)
    dm, dp = critical_lane_table(board, (pair, batch))
    assert dm["routed_length_nm"] == 0 and dm["layers"] == [] and dm["via_count"] == 0
    assert dm["estimated_delay_ps"] is None and dm["delay_reason"] == "no routed copper"
    assert dp["routed_length_nm"] == nm_from_mm(25) and dp["via_count"] == 1
    assert dp["layers"] == ["F.Cu", "In1.Cu"]
    assert dp["layer_lengths_nm"] == {"F.Cu": nm_from_mm(5), "In1.Cu": nm_from_mm(20)}
    # Outer layer: Hammerstad-Jensen microstrip over the 0.1 mm prepreg.
    # Inner layer: thickness-weighted permittivity of the prepreg and core.
    microstrip = (4.1 + 1) / 2 + (4.1 - 1) / 2 / sqrt(1 + 12 * 0.1 / 0.25)
    stripline = (4.1 * 0.1 + 4.5 * 1.2) / 1.3
    assert dp["effective_permittivity"] == {"F.Cu": round(microstrip, 4), "In1.Cu": round(stripline, 4)}
    expected = sum(propagation_delay(length, Decimal(f"{er:.6f}")).value
                   for length, er in ((nm_from_mm(5), microstrip), (nm_from_mm(20), stripline)))
    assert dp["estimated_delay_ps"] == float((expected * Decimal(10) ** 12).quantize(Decimal("0.001")))
    assert 170 < dp["estimated_delay_ps"] < 172
    assert dp["delay_evidence_grade"] == "screening" and dp["delay_reason"] is None
    missing = replace(board, stackup=_four_layer_stackup(None))
    _, lane = critical_lane_table(missing, (pair,))
    assert lane["estimated_delay_ps"] is None
    assert lane["delay_reason"] == "dielectric P1 declares no relative permittivity"

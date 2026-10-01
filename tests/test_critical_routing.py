from __future__ import annotations

from dataclasses import replace
from hashlib import sha256
import pytest

from pcbir import (
    BoardOutline,
    ComponentPlacementRule,
    CriticalRoutingStatus,
    CopperLayer,
    FootprintPad,
    GlobalRouterOptions,
    GlobalViaProposal,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    RouteKind,
    Size,
    Stackup,
    nm_from_mm,
    route_critical_nets,
    route_global,
    TrackSegment, CopperKeepout, PolygonRing, PolygonWithHoles,
    DetailedRouterOptions, route_detailed, run_physical_drc,
)
from pcbir.critical import _route_pair, _tune_pair, _validate_candidate, _coupled_length


def _pair_board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/two-pad-pair",
        (
            FootprintPad("1", Point.mm(0, "-0.5"), Size.mm("0.4", "0.4")),
            FootprintPad("2", Point.mm(0, "0.5"), Size.mm("0.4", "0.4")),
        ),
        Size.mm(1, 2),
    )
    return PhysicalBoard(
        "CriticalPair",
        BoardOutline.rectangle(40, 25),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(5, 12)),
            Placement("J2", footprint.name, Point.mm(35, 12)),
        ),
        (
            PhysicalNet("USB_DP", (PadReference("J1", "1"), PadReference("J2", "1"))),
            PhysicalNet("USB_DM", (PadReference("J1", "2"), PadReference("J2", "2"))),
        ),
        net_routing_rules=(
            NetRoutingRule(
                "USB_DP",
                RouteKind.DIFFERENTIAL,
                priority=100,
                width_nm=nm_from_mm("0.25"),
                differential_partner="USB_DM",
                pair_gap_nm=nm_from_mm("0.2"),
                max_skew_nm=nm_from_mm("10"),
                maximum_uncoupled_length_nm=nm_from_mm("10"),
                target_impedance_ohms=90,
            ),
            NetRoutingRule(
                "USB_DM",
                RouteKind.DIFFERENTIAL,
                priority=100,
                width_nm=nm_from_mm("0.25"),
                differential_partner="USB_DP",
                pair_gap_nm=nm_from_mm("0.2"),
                max_skew_nm=nm_from_mm("10"),
                maximum_uncoupled_length_nm=nm_from_mm("10"),
                target_impedance_ohms=90,
            ),
        ),
    )


def test_critical_pair_is_routed_as_locked_exact_copper() -> None:
    board = _pair_board()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    first = route_critical_nets(board, guides)
    second = route_critical_nets(board, guides)

    assert first == second
    assert first.status is CriticalRoutingStatus.WARNING
    assert len(first.nets) == 1
    assert first.nets[0].connected
    assert first.nets[0].strategy == "aligned_pair"
    assert first.nets[0].nets == ("USB_DM", "USB_DP")
    assert first.locked_tracks
    assert first.nets[0].coupled_length_nm > 0
    assert len(first.nets[0].uncoupled_lengths_nm) == 2
    assert first.board.tracks == first.locked_tracks
    assert first.board.metadata["detailed_routing"] == "partial"
    assert "field-solver" in first.nets[0].assumptions[0]
    assert not {finding.code for finding in run_physical_drc(first.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }
    assert all(track.start.x_nm == track.end.x_nm or track.start.y_nm == track.end.y_nm
               or abs(track.start.x_nm - track.end.x_nm) == abs(track.start.y_nm - track.end.y_nm)
               for track in first.locked_tracks)


def test_critical_single_net_enforces_length_budget() -> None:
    board = _pair_board()
    critical = replace(
        board,
        net_routing_rules=(
            NetRoutingRule(
                "USB_DP",
                RouteKind.CRITICAL,
                width_nm=nm_from_mm("0.25"),
                max_length_nm=nm_from_mm("1"),
            ),
        ),
    )
    guides = route_global(critical, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    result = route_critical_nets(critical, guides)

    assert result.status is CriticalRoutingStatus.FAILED
    assert "exceeds" in result.nets[0].diagnostics[0]


def test_pair_rules_must_be_symmetric() -> None:
    board = replace(_pair_board(), net_routing_rules=(_pair_board().net_routing_rules[0],))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))

    result = route_critical_nets(board, guides)

    assert result.status is CriticalRoutingStatus.FAILED
    assert "symmetric" in result.nets[0].diagnostics[0]


def test_pair_enforces_measured_uncoupled_budget() -> None:
    board = _pair_board()
    rules = tuple(replace(rule, maximum_uncoupled_length_nm=nm_from_mm("0.5")) for rule in board.net_routing_rules)
    board = replace(board, net_routing_rules=rules)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    result = route_critical_nets(board, guides)
    assert result.status is CriticalRoutingStatus.FAILED
    assert any("uncoupled length" in item for item in result.nets[0].diagnostics)
    assert result.locked_tracks == result.locked_vias == ()


def test_symmetric_pair_needs_no_tuning_and_binds_external_evidence() -> None:
    board = _pair_board()
    evidence = sha256(b"field solver report").hexdigest()
    rules = tuple(
        replace(rule, max_skew_nm=nm_from_mm("0.1"),
                maximum_uncoupled_length_nm=nm_from_mm("20"),
                tuning_amplitude_limit_nm=nm_from_mm("2"),
                impedance_evidence_digest=evidence)
        for rule in board.net_routing_rules
    )
    board = replace(board, net_routing_rules=rules)
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    result = route_critical_nets(board, guides)
    pair = result.nets[0]
    assert pair.connected
    assert pair.skew_nm <= nm_from_mm("0.1")
    assert pair.tuned_length_nm == 0
    assert pair.evidence_digests == (evidence,)
    assert not any("field-solver" in item for item in pair.assumptions)


def test_pair_rejects_overlapping_return_via_and_floating_transition() -> None:
    board = _pair_board()
    board = replace(
        board,
        nets=(*board.nets, PhysicalNet("GND", ())),
        net_routing_rules=tuple(
            replace(rule, require_return_vias=True, return_via_net="GND",
                    maximum_return_via_distance_nm=nm_from_mm("1"))
            for rule in board.net_routing_rules
        ),
    )
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    pair_name = min(rule.net for rule in board.net_routing_rules)
    routes = tuple(
        replace(route, vias=(GlobalViaProposal(route.net, Point.mm(20, 12),
                                               CopperLayer.FRONT,
                                               CopperLayer.BACK,
                                               "test-via"),))
        if route.net == pair_name else route
        for route in guides.routes
    )
    guides = replace(guides, routes=routes)
    proposed, _, proposed_vias = _route_pair(
        board, *board.net_routing_rules, {route.net: route for route in guides.routes},
    )
    assert proposed.return_via_count == 1
    assert sum(via.net == "GND" for via in proposed_vias) == 1
    rejected, rejected_tracks, rejected_vias = _validate_candidate(
        board, proposed, _route_pair(board, *board.net_routing_rules,
                                    {route.net: route for route in guides.routes})[1],
        proposed_vias, [], [],
    )
    assert rejected.candidate_rejected and not rejected.connected
    assert rejected_tracks == rejected_vias == ()
    result = route_critical_nets(board, guides)
    pair = result.nets[0]
    # Global transitions are guides, not mandatory copper. A legal surface-only
    # joint repair has no actual transition requiring a return via.
    assert pair.connected and pair.strategy == "joint_pair_refined"
    assert pair.pair_refinement_candidates > 0
    assert pair.return_via_count == 0
    assert result.locked_vias == ()
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for f in run_physical_drc(result.board).findings)


def test_tuning_is_bounded_but_requires_candidate_geometry_validation() -> None:
    first = [TrackSegment("A", Point.mm(5, 5), Point.mm(15, 5), nm_from_mm("0.25"), CopperLayer.FRONT)]
    second = [TrackSegment("B", Point.mm(5, 8), Point.mm(17, 8), nm_from_mm("0.25"), CopperLayer.FRONT)]
    assert _tune_pair(first, second, nm_from_mm("0.1"), nm_from_mm("0.4")) == nm_from_mm("0.8")
    assert max(track.end.y_nm for track in first) == nm_from_mm("5.4")


@pytest.mark.parametrize("obstacle", ("pad", "keepout", "edge", "pair_clearance"))
def test_pair_rejects_unsafe_exact_geometry_atomically(obstacle) -> None:
    board = _pair_board()
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    if obstacle == "pad":
        blocker = PhysicalFootprint(
            "blocker", (FootprintPad("1", Point(0, 0), Size.mm(1, 1)),), Size.mm(1, 1),
        )
        board = replace(board, footprints={**board.footprints, "blocker": blocker},
                        placements=(*board.placements, Placement("X", "blocker", Point.mm(20, 12))))
    elif obstacle == "keepout":
        wall = CopperKeepout("wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
            Point.mm(19, 10), Point.mm(21, 10), Point.mm(21, 14), Point.mm(19, 14),
        ))))
        board = replace(board, copper_keepouts=(wall,))
    elif obstacle == "edge":
        board = replace(board, outline=BoardOutline.rectangle(40, "12.3"))
    else:
        board = replace(board, net_routing_rules=tuple(
            replace(rule, pair_gap_nm=nm_from_mm("0.05")) for rule in board.net_routing_rules))
    proposed, tracks, vias = _route_pair(board, *board.net_routing_rules,
                                       {route.net: route for route in guides.routes})
    result, tracks, vias = _validate_candidate(board, proposed, tracks, vias, [], [])
    assert not result.connected and result.candidate_rejected and result.diagnostics
    assert tracks == vias == ()


def test_joint_pair_search_handles_staggered_midpoints_and_pad_pitches() -> None:
    board = _pair_board()
    footprint = replace(board.footprints[board.placements[0].footprint], name="wide-pair", pads=(
        FootprintPad("1", Point.mm(0, "-0.8"), Size.mm("0.4", "0.4")),
        FootprintPad("2", Point.mm(0, "0.8"), Size.mm("0.4", "0.4")),
    ))
    board = replace(board, footprints={**board.footprints, footprint.name: footprint},
                    placements=(board.placements[0], replace(board.placements[1],
                                footprint=footprint.name, position=Point.mm(35, 14))))
    result = route_critical_nets(board, route_global(board))
    assert result.nets[0].connected
    assert result.nets[0].strategy == "joint_pair_refined"
    assert result.nets[0].coupled_length_nm > nm_from_mm(20)
    assert result.locked_vias == ()
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
                   for f in run_physical_drc(result.board).findings)


def test_coupled_measurement_is_invariant_under_split_parallel_edges() -> None:
    width, spacing = nm_from_mm("0.2"), nm_from_mm("0.4")
    a = [TrackSegment("A", Point.mm(5, 5), Point.mm(25, 5), width, CopperLayer.FRONT)]
    b = [TrackSegment("B", Point.mm(7, "5.4"), Point.mm(23, "5.4"), width, CopperLayer.FRONT)]
    split = [replace(b[0], end=Point.mm(15, "5.4")), replace(b[0], start=Point.mm(15, "5.4"))]
    assert _coupled_length(a, b, spacing) == _coupled_length(a, split, spacing) == nm_from_mm(16)


def test_joint_search_is_bounded_fail_closed_and_does_not_mutate_input() -> None:
    from pcbir.pair_search import PairSearchStats, paired_candidates

    board = _pair_board()
    guides = {route.net: route for route in route_global(board).routes}
    wall = CopperKeepout("full-wall", (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
        Point.mm(19, 0), Point.mm(21, 0), Point.mm(21, 25), Point.mm(19, 25),
    ))))
    board = replace(board, copper_keepouts=(wall,))
    snapshot = board
    stats = PairSearchStats()
    assert list(paired_candidates(board, *board.net_routing_rules,
                guides["USB_DP"], guides["USB_DM"], maximum_searches=2,
                maximum_states=12, stats=stats)) == []
    assert 0 < stats.searches <= 2
    assert stats.searches <= stats.expanded_states <= 24
    assert stats.candidates == 0
    assert board == snapshot and board.tracks == board.vias == ()
    with pytest.raises(ValueError, match="bounds"):
        list(paired_candidates(board, *board.net_routing_rules,
             guides["USB_DP"], guides["USB_DM"], maximum_states=0))


def test_joint_search_supports_45_degree_rotated_terminal_rows() -> None:
    board = _pair_board()
    board = replace(board, placements=tuple(replace(placement, rotation_degrees=45)
                                          for placement in board.placements),
                    placement_rules=tuple(ComponentPlacementRule(placement.reference,
                        allowed_orientations=(45,)) for placement in board.placements))
    guides = route_global(board)
    # A coarse floating transition is a proposal, not required copper. Force
    # the joint fallback rather than accidentally exercising the old guide.
    guides = replace(guides, routes=tuple(replace(route, vias=(GlobalViaProposal(
        route.net, Point.mm(20, 12), CopperLayer.FRONT, CopperLayer.BACK,
        "test-via"),)) if route.net == "USB_DM" else route for route in guides.routes))
    first = route_critical_nets(board, guides)
    second = route_critical_nets(board, guides)
    assert first == second
    assert first.nets[0].connected and first.nets[0].strategy == "joint_pair_refined"
    assert first.nets[0].search_states > 0 and first.nets[0].candidate_attempts > 0
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
                   for f in run_physical_drc(first.board).findings)


def test_joint_miters_preserve_clearance_at_45_degree_corners() -> None:
    from pcbir.pair_search import _lane_paths, _legal, _tracks
    from pcbir.routing_clearance import RoutingClearanceIndex

    board = _pair_board()
    width, gap = nm_from_mm("0.25"), nm_from_mm("0.2")
    lanes = _lane_paths((Point.mm(10, 5), Point.mm(15, 5),
                         Point.mm(18, 8), Point.mm(18, 15)), 0, 2, (width + gap) // 2, 1)
    assert lanes is not None
    first, second = (_tracks("USB_DP", lanes[0], width, CopperLayer.FRONT),
                     _tracks("USB_DM", lanes[1], width, CopperLayer.FRONT))
    assert _legal(board, RoutingClearanceIndex(board), first, second,
                  board.rules.minimum_clearance_nm)
    assert _coupled_length(list(first), list(second), width + gap) > nm_from_mm(10)


def test_joint_search_does_not_invent_copper_behind_a_tapered_port() -> None:
    from pcbir.pair_search import PairSearchStats, _ports, _search, _lane_paths, _legal, _tracks
    from pcbir.routing_clearance import RoutingClearanceIndex

    board = _pair_board()
    board = replace(board, copper_keepouts=(CopperKeepout("behind-port",
        (CopperLayer.FRONT,), PolygonWithHoles(PolygonRing((
            Point.mm("4.1", "11.7"), Point.mm("4.7", "11.7"),
            Point.mm("4.7", "12.3"), Point.mm("4.1", "12.3"),
        )))),))
    width, offset = nm_from_mm("0.25"), nm_from_mm("0.225")
    clearance, layer = board.rules.minimum_clearance_nm, CopperLayer.FRONT
    index = RoutingClearanceIndex(board)
    start = next(p for p in _ports(board, index, "USB_DP", "USB_DM",
        Point.mm(5, "11.5"), Point.mm(5, "12.5"), "J1", width, offset, clearance, layer)
        if p.center == Point.mm("5.5", 12))
    end = next(p for p in _ports(board, index, "USB_DP", "USB_DM",
        Point.mm(35, "11.5"), Point.mm(35, "12.5"), "J2", width, offset, clearance, layer)
        if p.center == Point.mm("34.5", 12))
    escaped = replace(board, tracks=(*start.first, *start.second, *end.first, *end.second))
    index = RoutingClearanceIndex(escaped)
    fictitious = _lane_paths((Point.mm("4.5", 12), start.center, Point.mm("6.5", 12)),
                             0, 0, offset, start.sign)
    assert not _legal(escaped, index,
        _tracks("USB_DP", fictitious[0], width, layer),
        _tracks("USB_DM", fictitious[1], width, layer), clearance)
    candidate = _search(escaped, index, "USB_DP", "USB_DM", start, end,
                        width, offset, clearance, layer, nm_from_mm(1), 100, PairSearchStats())
    assert candidate is not None
    routed = replace(escaped, tracks=(*escaped.tracks, *candidate[0], *candidate[1]))
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
                   for f in run_physical_drc(routed).findings)


def test_subset_repair_cannot_reroute_or_prune_a_locked_pair() -> None:
    board = _pair_board()
    guides = route_global(board)
    critical = route_critical_nets(board, guides)
    assert critical.nets[0].connected
    repaired = route_detailed(
        critical.board, guides, DetailedRouterOptions(maximum_passes=1),
        only_nets=frozenset({"USB_DP", "USB_DM"}),
        fanout_accesses={PadReference("J1", "1"): Point.mm(6, 12)},
    )
    assert repaired.nets == ()
    assert repaired.board.tracks == critical.locked_tracks
    assert repaired.board.vias == critical.locked_vias


def test_single_ended_critical_net_repairs_stale_guide_with_exact_clearance() -> None:
    base = _pair_board()
    board = replace(base, net_routing_rules=(NetRoutingRule(
        "USB_DP", RouteKind.CRITICAL, allowed_layers=(CopperLayer.FRONT,), max_vias=0,
    ),))
    guides = route_global(board)
    blocker = PhysicalFootprint(
        "blocker", (FootprintPad("1", Point(0, 0), Size.mm(2, 2)),), Size.mm(2, 2),
    )
    board = replace(board, footprints={**board.footprints, "blocker": blocker},
                    placements=(*board.placements, Placement("X", "blocker", Point.mm(20, "11.5"))))
    result = route_critical_nets(board, guides)
    assert result.nets[0].connected
    assert result.nets[0].strategy == "exact_single_net"
    assert result.locked_tracks
    assert result.locked_vias == ()
    assert not any(finding.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for finding in run_physical_drc(result.board).findings)


def test_later_single_critical_route_cannot_cut_a_reserved_pair() -> None:
    base = _pair_board()
    terminal = PhysicalFootprint(
        "terminal", (FootprintPad("1", Point(0, 0), Size.mm("0.4", "0.4")),), Size.mm(1, 1),
    )
    board = replace(
        base, footprints={**base.footprints, "terminal": terminal},
        placements=(*base.placements, Placement("S1", "terminal", Point.mm(20, 5)),
                    Placement("S2", "terminal", Point.mm(20, 20))),
        nets=(*base.nets, PhysicalNet("CROSS", (PadReference("S1", "1"), PadReference("S2", "1")))),
        net_routing_rules=(*base.net_routing_rules, NetRoutingRule(
            "CROSS", RouteKind.CRITICAL, allowed_layers=(CopperLayer.FRONT,), max_vias=0)),
    )
    guides = route_global(board)
    result = route_critical_nets(board, guides)
    pair = route_critical_nets(base, route_global(base))
    assert result.nets[0].connected
    assert tuple(track for track in result.locked_tracks if track.net != "CROSS") == pair.locked_tracks
    assert not any(finding.code in {"DRC-SHORT", "DRC-CLEARANCE"}
                   for finding in run_physical_drc(result.board).findings)
    assert result.nets[1].strategy == "exact_single_net"


def test_critical_transition_without_special_technology_is_through_via() -> None:
    base = _pair_board()
    board = replace(
        base,
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        net_routing_rules=(NetRoutingRule("USB_DP", RouteKind.CRITICAL),),
    )
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2.5")))
    guides = replace(guides, routes=tuple(
        replace(route, vias=(GlobalViaProposal(
            route.net, Point.mm(20, 12), CopperLayer.FRONT,
            CopperLayer.INTERNAL_1, "test-via",
        ),)) if route.net == "USB_DP" else route
        for route in guides.routes
    ))

    result = route_critical_nets(board, guides)

    assert result.locked_vias
    assert all(
        via.from_layer is CopperLayer.FRONT and via.to_layer is CopperLayer.BACK
        for via in result.locked_vias
    )

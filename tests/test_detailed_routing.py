from __future__ import annotations

from dataclasses import replace
import json
import pcbir.detailed as detailed_module

from pcbir import (
    BoardOutline,
    CriticalRoutingStatus,
    CopperLayer,
    CopperKeepout,
    CopperZone,
    DetailedRouterOptions,
    DetailedRoutingStatus,
    DesignRules,
    FootprintPad,
    GlobalRouterOptions,
    NetRoutingRule,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    PlacementKeepout,
    Point,
    PolygonRing,
    PolygonWithHoles,
    RouteKind,
    Size,
    Stackup,
    nm_from_mm,
    route_critical_nets,
    route_detailed,
    route_global,
    run_physical_drc,
    TrackSegment,
    Via,
)
from pcbir.detailed import (
    DetailedNetResult,
    DetailedNode,
    DetailedRoutingMetrics,
    _NetAttempt,
    _Pass,
    _build_grid,
    _edge_resources,
    _merge_collinear_tracks,
    _net_search_options,
    _pass_heuristic_weight,
    _repair_from_passes,
    _route_net,
)
from pcbir.routing_clearance import RoutingClearanceIndex


def _board() -> PhysicalBoard:
    footprint = PhysicalFootprint(
        "test/one-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(1, 1),
    )
    return PhysicalBoard(
        "DetailedRoute",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )


def test_detailed_router_materializes_deterministic_exact_copper() -> None:
    board = _board()
    global_route = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))
    )
    options = DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"))

    first = route_detailed(board, global_route, options)
    second = route_detailed(board, global_route, options)

    assert first == second
    assert first.status is DetailedRoutingStatus.SUCCESS
    assert first.board.tracks
    assert first.board.metadata["detailed_routing"] == "complete"
    assert first.board.vias == ()
    assert any(
        track.start.x_nm != track.end.x_nm and track.start.y_nm != track.end.y_nm
        for track in first.board.tracks
    ) or len(first.board.tracks) < 28
    assert json.loads(first.to_json())["schema"] == "copperscript-detailed-route/v0.1"


def test_open_board_prefers_long_45_degree_and_straight_segments() -> None:
    base = _board()
    board = replace(base, placements=(
        replace(base.placements[0], position=Point.mm(3, 3)),
        replace(base.placements[1], position=Point.mm(15, 9)),
    ))
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert len(result.board.tracks) <= 4
    assert any(
        abs(track.end.x_nm - track.start.x_nm) >= nm_from_mm(4)
        and abs(track.end.x_nm - track.start.x_nm)
            == abs(track.end.y_nm - track.start.y_nm)
        for track in result.board.tracks
    )
    assert all(
        track.start.x_nm == track.end.x_nm
        or track.start.y_nm == track.end.y_nm
        or abs(track.end.x_nm - track.start.x_nm)
            == abs(track.end.y_nm - track.start.y_nm)
        for track in result.board.tracks
    )


def test_straight_runs_merge_without_erasing_a_branch() -> None:
    layer = CopperLayer.FRONT
    tracks = (
        TrackSegment("SIGNAL", Point.mm(1, 1), Point.mm(2, 1), nm_from_mm("0.2"), layer),
        TrackSegment("SIGNAL", Point.mm(2, 1), Point.mm(3, 1), nm_from_mm("0.2"), layer),
        TrackSegment("SIGNAL", Point.mm(3, 1), Point.mm(4, 1), nm_from_mm("0.2"), layer),
        TrackSegment("SIGNAL", Point.mm(2, 1), Point.mm(2, 2), nm_from_mm("0.2"), layer),
    )

    merged = _merge_collinear_tracks(tracks)
    assert len(merged) == 3
    assert any({track.start, track.end} == {Point.mm(2, 1), Point.mm(4, 1)}
               for track in merged)
    assert any({track.start, track.end} == {Point.mm(1, 1), Point.mm(2, 1)}
               for track in merged)


def test_budget_exhaustion_retries_with_orthogonal_search(monkeypatch) -> None:
    board = _board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    original = detailed_module._search
    modes: list[bool] = []

    def forced_octilinear_exhaustion(*args, **kwargs):
        mode = args[10].octilinear_search
        modes.append(mode)
        if mode:
            raise detailed_module._SearchBudgetExceeded
        return original(*args, **kwargs)

    monkeypatch.setattr(detailed_module, "_search", forced_octilinear_exhaustion)
    result = route_detailed(board, guide, DetailedRouterOptions(
        maximum_passes=1, pitch_nm=nm_from_mm(1),
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert modes == [True, False]
    assert result.nets[0].orthogonal_mode_used


def test_high_fanout_net_uses_orthogonal_first_mode() -> None:
    options = DetailedRouterOptions()
    assert _net_search_options(options, 15).octilinear_search
    chosen = _net_search_options(options, 16)
    assert not chosen.octilinear_search
    assert chosen.bend_cost == 5


def test_detailed_router_reports_search_budget_without_emitting_partial_copper() -> None:
    board = _board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))

    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1, maximum_search_states=1,
    ))

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert "search budget" in result.nets[0].diagnostics[0]
    assert not result.board.tracks


def test_same_net_candidate_vias_must_meet_drill_spacing() -> None:
    board = _board()
    clearance = RoutingClearanceIndex(board)
    first = Via(
        "SIGNAL", Point.mm(10, 6), nm_from_mm("0.8"), nm_from_mm("0.4"),
        CopperLayer.FRONT, CopperLayer.BACK,
    )
    too_close = replace(first, position=Point.mm("10.5", 6))
    legal = replace(first, position=Point.mm(12, 6))

    assert clearance.can_via(first.net, first.position, first.size_nm,
                             first.from_layer, first.to_layer)
    assert clearance.can_via(too_close.net, too_close.position, too_close.size_nm,
                             too_close.from_layer, too_close.to_layer)
    assert not clearance.candidate_vias_clear((first, too_close))
    assert clearance.candidate_via_conflict(
        (first, too_close), allow_movable_conflicts=True,
    ) == too_close
    assert clearance.candidate_vias_clear((first, legal))


def test_zone_net_is_deferred_without_claiming_unfilled_copper_connected() -> None:
    base = _board()
    zone = CopperZone(
        "plane", "SIGNAL", (CopperLayer.BACK,),
        PolygonWithHoles(PolygonRing((
            Point.mm(1, 1), Point.mm(19, 1),
            Point.mm(19, 11), Point.mm(1, 11),
        ))),
    )
    board = replace(base, zones=(zone,))
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))

    result = route_detailed(board, guide, DetailedRouterOptions(maximum_passes=2))

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert result.metrics.unrouted_net_count == 1
    assert "awaits verified fill" in result.nets[0].diagnostics[0]
    assert result.board.tracks == ()
    assert "DRC-OPEN-NET" in {
        finding.code for finding in run_physical_drc(result.board).findings
    }


def test_per_net_grid_does_not_cross_product_unrelated_pad_coordinates() -> None:
    base = _board()
    footprint = next(iter(base.footprints.values()))
    board = replace(
        base,
        placements=(*base.placements, Placement(
            "X1", footprint.name, Point.mm("8.37", "4.19"),
        )),
        nets=(*base.nets, PhysicalNet("OTHER", (PadReference("X1", "1"),))),
    )
    options = DetailedRouterOptions(pitch_nm=nm_from_mm(1))
    signal_grid = _build_grid(board, options, board.nets[0].pads)
    other_grid = _build_grid(board, options, board.nets[1].pads)

    assert nm_from_mm("8.37") not in signal_grid.xs
    assert nm_from_mm("4.19") not in signal_grid.ys
    assert nm_from_mm("8.37") in other_grid.xs
    assert nm_from_mm("4.19") in other_grid.ys
    assert _edge_resources(
        signal_grid, DetailedNode(0, 0, 0), DetailedNode(0, 1, 0),
    ) == _edge_resources(
        other_grid, DetailedNode(0, 0, 0), DetailedNode(0, 1, 0),
    )


def test_repair_combines_clear_routes_from_different_passes() -> None:
    base = _board()
    footprint = next(iter(base.footprints.values()))
    board = replace(
        base,
        placements=(*base.placements,
            Placement("J3", footprint.name, Point.mm(3, 9)),
            Placement("J4", footprint.name, Point.mm(17, 9))),
        nets=(*base.nets, PhysicalNet(
            "OTHER", (PadReference("J3", "1"), PadReference("J4", "1")),
        )),
    )
    signal = _NetAttempt(
        DetailedNetResult("SIGNAL", True, 1, 0, nm_from_mm(14), 0),
        (TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
                      nm_from_mm("0.25"), CopperLayer.FRONT),),
        (), frozenset(),
    )
    other = _NetAttempt(
        DetailedNetResult("OTHER", True, 1, 0, nm_from_mm(14), 0),
        (TrackSegment("OTHER", Point.mm(3, 9), Point.mm(17, 9),
                      nm_from_mm("0.25"), CopperLayer.FRONT),),
        (), frozenset(),
    )
    missing_signal = _NetAttempt(
        DetailedNetResult("SIGNAL", False, 0, 0, 0, 0, ("failed",)),
        (), (), frozenset(),
    )
    missing_other = replace(missing_signal, result=replace(
        missing_signal.result, net="OTHER",
    ))
    metrics = DetailedRoutingMetrics(1, 1, 0, 0, 1, 0, nm_from_mm(14), 1)
    first = _Pass((signal, missing_other), {}, metrics)
    second = _Pass((missing_signal, other), {}, metrics)

    repaired = _repair_from_passes(
        board, list(board.nets), {}, {}, first, [first, second],
        DetailedRouterOptions(maximum_passes=2),
    )

    assert repaired.metrics.routed_net_count == 2
    assert repaired.metrics.unrouted_net_count == 0
    assert repaired.metrics.total_conflict_overflow == 0


def test_repair_expands_budget_only_for_remaining_open_nets(monkeypatch) -> None:
    board = _board()
    missing = _NetAttempt(
        DetailedNetResult("SIGNAL", False, 0, 0, 0, 0, ("failed",)),
        (), (), frozenset(),
    )
    metrics = DetailedRoutingMetrics(0, 1, 0, 0, 0, 0, 0, 1)
    best = _Pass((missing,), {}, metrics)
    retry_failure = replace(missing, result=replace(
        missing.result, diagnostics=("repair budget exhausted",),
    ))
    observed: list[int] = []

    def fake_route_net(*args, **kwargs):
        observed.append(args[-1].maximum_search_states)
        return retry_failure

    monkeypatch.setattr(detailed_module, "_route_net", fake_route_net)
    repaired = _repair_from_passes(
        board, list(board.nets), {}, {}, best, [best, best],
        DetailedRouterOptions(
            maximum_passes=2, maximum_search_states=10,
            repair_budget_multiplier=5,
        ),
    )

    assert observed == [50]
    assert repaired.nets[0].result.diagnostics == ("repair budget exhausted",)


def test_repair_refines_grid_only_after_exhaustive_no_path(monkeypatch) -> None:
    board = _board()
    missing = _NetAttempt(
        DetailedNetResult("SIGNAL", False, 0, 0, 0, 0, ("initial failure",)),
        (), (), frozenset(),
    )
    metrics = DetailedRoutingMetrics(0, 1, 0, 0, 0, 0, 0, 1)
    best = _Pass((missing,), {}, metrics)
    no_path = replace(missing, result=replace(
        missing.result, diagnostics=("detailed search cannot reach J2.1",),
    ))
    connected = replace(missing, result=replace(
        missing.result, connected=True, diagnostics=(),
    ))
    observed: list[int] = []

    def fake_route_net(*args, **kwargs):
        observed.append(args[-1].pitch_nm)
        return no_path if len(observed) == 1 else connected

    monkeypatch.setattr(detailed_module, "_route_net", fake_route_net)
    repaired = _repair_from_passes(
        board, list(board.nets), {}, {}, best, [best],
        DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=1),
    )

    assert observed == [nm_from_mm(1), nm_from_mm("0.5")]
    assert repaired.nets[0].result.connected


def test_later_passes_diversify_search_weight_deterministically() -> None:
    options = DetailedRouterOptions(heuristic_weight_percent=100)

    assert [_pass_heuristic_weight(options, index) for index in range(1, 9)] == [
        100, 100, 150, 200, 250, 300, 150, 200,
    ]


def test_clearance_index_distinguishes_movable_from_locked_blockers() -> None:
    board = _board()
    candidate = TrackSegment(
        "OTHER", Point.mm(10, 3), Point.mm(10, 9),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    crossing = TrackSegment(
        "SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    clearance = RoutingClearanceIndex(board)
    clearance.add_track(crossing)

    assert clearance.blocking_track_nets(candidate) == (
        frozenset({"SIGNAL"}), False,
    )
    locked = RoutingClearanceIndex(replace(board, tracks=(crossing,)))
    assert locked.blocking_track_nets(candidate) == (frozenset(), True)


def test_soft_search_proposes_removable_conflict_but_never_locked_conflict() -> None:
    base = _board()
    wall = TrackSegment(
        "BLOCKER", Point.mm(10, 0), Point.mm(10, 12),
        nm_from_mm("0.5"), CopperLayer.FRONT,
    )
    board = replace(
        base,
        nets=(*base.nets, PhysicalNet("BLOCKER", ())),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", allowed_layers=(CopperLayer.FRONT,),
        ),),
    )
    guide = next(item for item in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
    ).routes if item.net == "SIGNAL")
    options = DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=1,
    )
    grid = _build_grid(board, options, board.nets[0].pads)
    movable = RoutingClearanceIndex(board)
    movable.add_track(wall)
    hard_attempt = _route_net(
        board, grid, "SIGNAL", board.nets[0].pads,
        board.net_routing_rules[0], guide, {}, {}, movable, options,
    )
    soft_attempt = _route_net(
        board, grid, "SIGNAL", board.nets[0].pads,
        board.net_routing_rules[0], guide, {}, {}, movable, options,
        allow_movable_conflicts=True,
    )
    locked_board = replace(board, tracks=(wall,))
    locked_attempt = _route_net(
        locked_board, _build_grid(locked_board, options, locked_board.nets[0].pads),
        "SIGNAL", locked_board.nets[0].pads,
        locked_board.net_routing_rules[0], guide, {}, {},
        RoutingClearanceIndex(locked_board), options,
        allow_movable_conflicts=True,
    )

    assert not hard_attempt.result.connected
    assert soft_attempt.result.connected
    assert any(movable.blocking_track_nets(track)[0] for track in soft_attempt.tracks)
    assert not locked_attempt.result.connected


def test_repair_rips_up_one_blocker_and_reroutes_it() -> None:
    base = _board()
    footprint = next(iter(base.footprints.values()))
    board = replace(
        base,
        placements=(*base.placements,
            Placement("J3", footprint.name, Point.mm(10, 3)),
            Placement("J4", footprint.name, Point.mm(10, 9))),
        nets=(*base.nets, PhysicalNet(
            "OTHER", (PadReference("J3", "1"), PadReference("J4", "1")),
        )),
    )
    signal_track = TrackSegment(
        "SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    other_track = TrackSegment(
        "OTHER", Point.mm(10, 3), Point.mm(10, 9),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    )
    signal = _NetAttempt(
        DetailedNetResult("SIGNAL", True, 1, 0, nm_from_mm(14), 0),
        (signal_track,), (), frozenset(),
    )
    other = _NetAttempt(
        DetailedNetResult("OTHER", True, 1, 0, nm_from_mm(6), 0),
        (other_track,), (), frozenset(),
    )
    missing_signal = _NetAttempt(
        DetailedNetResult("SIGNAL", False, 0, 0, 0, 0, ("failed",)),
        (), (), frozenset(),
    )
    missing_other = replace(missing_signal, result=replace(
        missing_signal.result, net="OTHER",
    ))
    metrics = DetailedRoutingMetrics(1, 1, 0, 0, 1, 0, nm_from_mm(14), 1)
    first = _Pass((signal, missing_other), {}, metrics)
    second = _Pass((missing_signal, other), {}, metrics)
    guides = {item.net: item for item in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
    ).routes}

    repaired = _repair_from_passes(
        board, list(board.nets), {}, guides, first, [first, second],
        DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=2),
    )

    assert repaired.metrics.routed_net_count == 2
    tracks = tuple(track for item in repaired.nets for track in item.tracks)
    vias = tuple(via for item in repaired.nets for via in item.vias)
    assert not {finding.code for finding in run_physical_drc(
        replace(board, tracks=tracks, vias=vias),
    ).findings} & {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}


def test_transactional_repair_can_displace_three_blockers(monkeypatch) -> None:
    base = _board()
    footprint = next(iter(base.footprints.values()))
    placements = list(base.placements)
    nets = list(base.nets)
    blockers = []
    for index, x in enumerate((6, 10, 14), 1):
        placements.extend((
            Placement(f"B{index}A", footprint.name, Point.mm(x, 3)),
            Placement(f"B{index}B", footprint.name, Point.mm(x, 9)),
        ))
        name = f"BLOCK{index}"
        nets.append(PhysicalNet(name, (
            PadReference(f"B{index}A", "1"), PadReference(f"B{index}B", "1"),
        )))
        track = TrackSegment(name, Point.mm(x, 3), Point.mm(x, 9),
                             nm_from_mm("0.25"), CopperLayer.FRONT)
        blockers.append(_NetAttempt(
            DetailedNetResult(name, True, 1, 0, nm_from_mm(6), 0),
            (track,), (), frozenset(),
        ))
    board = replace(base, placements=tuple(placements), nets=tuple(nets))
    signal_track = TrackSegment("SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
                                nm_from_mm("0.25"), CopperLayer.FRONT)
    signal = _NetAttempt(
        DetailedNetResult("SIGNAL", True, 1, 0, nm_from_mm(14), 0),
        (signal_track,), (), frozenset(),
    )
    missing_signal = detailed_module._failed("SIGNAL", "unrouted")
    missing_blockers = tuple(detailed_module._failed(item.result.net, "unrouted")
                             for item in blockers)
    first = _Pass((missing_signal, *blockers), {},
                  DetailedRoutingMetrics(3, 1, 0, 0, 3, 0, nm_from_mm(18), 1))
    second = _Pass((signal, *missing_blockers), {},
                   DetailedRoutingMetrics(1, 3, 0, 0, 1, 0, nm_from_mm(14), 2))
    guides = {item.net: item for item in route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)),
    ).routes}
    original_route_net = detailed_module._route_net
    def only_candidate_for_signal(*args, **kwargs):
        if args[2] == "SIGNAL":
            return detailed_module._failed("SIGNAL", "no alternate path")
        return original_route_net(*args, **kwargs)
    monkeypatch.setattr(detailed_module, "_route_net", only_candidate_for_signal)
    repaired = _repair_from_passes(
        board, list(board.nets), {}, guides, first, [first, second],
        DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
                              maximum_ripup_blockers=3),
    )
    assert repaired.metrics.routed_net_count == 4
    all_tracks = tuple(track for item in repaired.nets for track in item.tracks)
    all_vias = tuple(via for item in repaired.nets for via in item.vias)
    assert not {item.code for item in run_physical_drc(
        replace(board, tracks=all_tracks, vias=all_vias),
    ).findings} & {"DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"}
    limited = _repair_from_passes(
        board, list(board.nets), {}, guides, first, [first, second],
        DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
                              maximum_ripup_blockers=2),
    )
    assert limited.metrics.unrouted_net_count >= 1


def test_detailed_router_preserves_critical_copper() -> None:
    board = replace(
        _board(),
        net_routing_rules=(NetRoutingRule("SIGNAL", RouteKind.CRITICAL),),
    )
    global_route = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"))
    )
    critical = route_critical_nets(board, global_route)

    assert critical.status is not CriticalRoutingStatus.FAILED
    result = route_detailed(critical.board, global_route)

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert result.locked_track_count == len(critical.board.tracks)
    assert result.board.tracks == critical.board.tracks


def test_soft_layer_costs_fall_back_when_they_strand_a_signal(monkeypatch) -> None:
    board = _board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    original_route_net = detailed_module._route_net

    def biased_search_exhausts(*args, **kwargs):
        if args[9].layer_preference_cost:
            return detailed_module._failed(args[2], "biased search exhausted")
        return original_route_net(*args, **kwargs)

    monkeypatch.setattr(detailed_module, "_route_net", biased_search_exhausts)
    routed = route_detailed(
        board, guide,
        DetailedRouterOptions(
            pitch_nm=nm_from_mm("0.5"), maximum_passes=1,
            layer_preference_cost=4,
        ),
    )
    assert routed.status is DetailedRoutingStatus.SUCCESS
    assert routed.metrics.unrouted_net_count == 0


def test_detailed_router_repair_subset_preserves_other_copper() -> None:
    board = _board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    untouched = route_detailed(board, guide, only_nets=frozenset())
    assert untouched.nets == ()
    assert untouched.board.tracks == board.tracks
    assert untouched.board.vias == board.vias
    repaired = route_detailed(board, guide, only_nets=frozenset({"SIGNAL"}))
    assert repaired.status is DetailedRoutingStatus.SUCCESS
    assert [item.net for item in repaired.nets] == ["SIGNAL"]


def test_detailed_router_fails_closed_without_a_connected_guide() -> None:
    board = replace(
        _board(),
        copper_keepouts=(
            CopperKeepout("wall", (CopperLayer.FRONT, CopperLayer.BACK),
                PolygonWithHoles(PolygonRing((
                    Point.mm(8, 0), Point.mm(12, 0),
                    Point.mm(12, 12), Point.mm(8, 12),
                ))),
            ),
        ),
    )
    global_route = route_global(
        board,
        GlobalRouterOptions(
            tile_size_nm=nm_from_mm("2"), maximum_iterations=2
        ),
    )

    result = route_detailed(board, global_route)

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert result.metrics.unrouted_net_count == 1
    assert "missing connected global guide" in result.nets[0].diagnostics[0]
    assert result.board.metadata["fabrication_ready"] == "false"


def test_placement_only_keepout_does_not_block_copper_routing() -> None:
    board = replace(
        _board(), keepouts=(PlacementKeepout(
            "assembly-space", BoardOutline.rectangle(
                4, 12, origin=Point.mm(8, 0),
            ),
        ),),
    )
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1,
    ))

    assert guide.routes[0].connected
    assert result.status is DetailedRoutingStatus.SUCCESS
    assert "DRC-OPEN-NET" not in {
        finding.code for finding in run_physical_drc(result.board).findings
    }


def test_detailed_router_may_cross_surface_courtyard_without_extra_vias() -> None:
    base = _board()
    wall = PhysicalFootprint(
        "test/surface-wall",
        (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
        Size.mm(4, 12),
    )
    board = replace(
        base,
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
        footprints={**base.footprints, wall.name: wall},
        placements=(*base.placements, Placement("W1", wall.name, Point.mm(10, 6))),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_1),
        ),),
    )
    guide = route_global(
        board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2"), maximum_iterations=1)
    )
    result = route_detailed(
        board, guide, DetailedRouterOptions(pitch_nm=nm_from_mm("0.5"), maximum_passes=1)
    )

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert all(track.layer is CopperLayer.FRONT for track in result.board.tracks)
    assert result.board.vias == ()


def test_detailed_router_connects_distinct_pads_snapped_to_one_grid_node() -> None:
    footprint = PhysicalFootprint(
        "test/tiny-pad",
        (FootprintPad("1", Point(0, 0), Size.mm("0.1", "0.1")),),
        Size.mm("0.1", "0.1"),
    )
    board = PhysicalBoard(
        "SharedAccess",
        BoardOutline.rectangle(20, 12),
        {footprint.name: footprint},
        (
            Placement("J1", footprint.name, Point.mm("3.1", 6)),
            Placement("J2", footprint.name, Point.mm("3.4", 6)),
        ),
        (PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),),
    )
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(
        board, guide, DetailedRouterOptions(pitch_nm=nm_from_mm(1), maximum_passes=1)
    )

    assert result.status is DetailedRoutingStatus.SUCCESS
    # The off-axis access now uses two octilinear legs, not an oblique chord.
    assert len(result.board.tracks) == 3
    assert all(detailed_module._octilinear(track.start, track.end) for track in result.board.tracks)
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }


def test_detailed_router_walks_around_foreign_pad_with_exact_clearance() -> None:
    base = _board()
    blocker = PhysicalFootprint(
        "test/blocker",
        (FootprintPad("1", Point(0, 0), Size.mm(2, 2)),),
        Size.mm(2, 2),
    )
    board = replace(
        base,
        footprints={**base.footprints, blocker.name: blocker},
        placements=(*base.placements, Placement("X1", blocker.name, Point.mm(10, 6))),
        nets=(*base.nets, PhysicalNet("BLOCKER", (PadReference("X1", "1"),))),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", allowed_layers=(CopperLayer.FRONT,),
        ),),
    )
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }
    progressive = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=2,
        progressive_guides=True,
    ))
    assert progressive.status is DetailedRoutingStatus.SUCCESS
    assert "DRC-OPEN-NET" not in {
        finding.code for finding in run_physical_drc(progressive.board).findings
    }


def test_unconnected_pad_is_still_a_copper_obstacle() -> None:
    base = _board()
    blocker = PhysicalFootprint(
        "test/unconnected-blocker",
        (FootprintPad("NC", Point(0, 0), Size.mm(2, 2)),),
        Size.mm(2, 2),
    )
    board = replace(
        base,
        footprints={**base.footprints, blocker.name: blocker},
        placements=(*base.placements, Placement("X1", blocker.name, Point.mm(10, 6))),
        net_routing_rules=(NetRoutingRule(
            "SIGNAL", allowed_layers=(CopperLayer.FRONT,),
        ),),
    )
    direct = replace(board, tracks=(TrackSegment(
        "SIGNAL", Point.mm(3, 6), Point.mm(17, 6),
        nm_from_mm("0.25"), CopperLayer.FRONT,
    ),))
    assert "DRC-SHORT" in {
        finding.code for finding in run_physical_drc(direct).findings
    }

    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=1,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }


def test_pad_center_grid_escapes_fine_pitch_at_coarse_global_pitch() -> None:
    array = PhysicalFootprint(
        "test/fine-pitch-array",
        (
            FootprintPad("L", Point.mm("-0.75", 0), Size.mm("0.375", "0.375")),
            FootprintPad("S", Point.mm("-0.25", 0), Size.mm("0.375", "0.375")),
            FootprintPad("R", Point.mm("0.25", 0), Size.mm("0.375", "0.375")),
        ),
        Size.mm(2, 1),
    )
    target = PhysicalFootprint(
        "test/target",
        (FootprintPad("1", Point(0, 0), Size.mm("0.5", "0.5")),),
        Size.mm(1, 1),
    )
    board = PhysicalBoard(
        "FinePitchAccess",
        BoardOutline.rectangle(20, 12),
        {array.name: array, target.name: target},
        (
            Placement("U1", array.name, Point.mm(10, 6)),
            Placement("J1", target.name, Point.mm(17, 6)),
        ),
        (PhysicalNet("SIGNAL", (
            PadReference("U1", "S"), PadReference("J1", "1"),
        )),),
        rules=DesignRules(
            minimum_clearance_nm=nm_from_mm("0.09"),
            default_track_width_nm=nm_from_mm("0.20"),
        ),
    )
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS, result.nets
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }


def test_detailed_router_fails_closed_at_unavoidable_locked_track() -> None:
    base = _board()
    board = replace(
        base,
        placements=(
            Placement("J1", next(iter(base.footprints)), Point.mm(3, 3)),
            Placement("J2", next(iter(base.footprints)), Point.mm(17, 9)),
        ),
        nets=(
            PhysicalNet("SIGNAL", (PadReference("J1", "1"), PadReference("J2", "1"))),
            PhysicalNet("BLOCKER", ()),
        ),
        tracks=(TrackSegment(
            "BLOCKER", Point.mm(0, 6), Point.mm(20, 6),
            nm_from_mm("0.5"), CopperLayer.FRONT,
        ),),
        net_routing_rules=(
            NetRoutingRule("SIGNAL", allowed_layers=(CopperLayer.FRONT,)),
            NetRoutingRule("BLOCKER", RouteKind.CRITICAL),
        ),
    )
    guide = route_global(replace(board, tracks=()), GlobalRouterOptions(
        tile_size_nm=nm_from_mm(2), maximum_iterations=1,
    ))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=6,
    ))

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert result.metrics.passes == 6
    assert not any(track.net == "SIGNAL" for track in result.board.tracks)
    assert result == route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=6,
    ))


def test_multiterminal_cleanup_keeps_exact_branch_junctions() -> None:
    base = _board()
    footprint_name = next(iter(base.footprints))
    board = replace(
        base,
        placements=(*base.placements, Placement("J3", footprint_name, Point.mm(10, 25))),
        outline=BoardOutline.rectangle(20, 30),
        nets=(PhysicalNet("SIGNAL", (
            PadReference("J1", "1"), PadReference("J2", "1"), PadReference("J3", "1"),
        )),),
    )
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(pitch_nm=nm_from_mm(1)))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert "DRC-OPEN-NET" not in {
        finding.code for finding in run_physical_drc(result.board).findings
    }
    constrained = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), constrained_pins_first=True,
    ))
    assert constrained.status is DetailedRoutingStatus.SUCCESS
    assert "DRC-OPEN-NET" not in {
        finding.code for finding in run_physical_drc(constrained.board).findings
    }


def test_detailed_router_does_not_cut_through_track_keepout() -> None:
    base = _board()
    keepout = CopperKeepout(
        "center",
        (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing((
            Point.mm(8, 4), Point.mm(12, 4),
            Point.mm(12, 8), Point.mm(8, 8),
        ))),
    )
    board = replace(base, copper_keepouts=(keepout,))
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm("0.5"), maximum_passes=1,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS
    assert "DRC-COPPER-KEEPOUT" not in {
        finding.code for finding in run_physical_drc(result.board).findings
    }


def test_progressive_guide_can_project_corridor_to_signal_layer() -> None:
    base = replace(
        _board(),
        stackup=Stackup((
            CopperLayer.FRONT, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.BACK,
        )),
    )
    guide = route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    assert all(segment.layer is CopperLayer.FRONT for segment in guide.routes[0].segments)
    board = replace(base, copper_keepouts=(CopperKeepout(
        "surface-wall", (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing((
            Point.mm(9, 0), Point.mm(11, 0),
            Point.mm(11, 12), Point.mm(9, 12),
        ))),
    ),))
    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1,
        maximum_search_states=1000, progressive_guides=True,
    ))

    assert result.status is DetailedRoutingStatus.SUCCESS, result.nets
    assert any(track.layer is CopperLayer.INTERNAL_1 for track in result.board.tracks)
    assert len(result.board.vias) >= 2
    assert not {finding.code for finding in run_physical_drc(result.board).findings} & {
        "DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET",
    }

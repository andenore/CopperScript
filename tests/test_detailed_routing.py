from __future__ import annotations

from dataclasses import replace
import json

from pcbir import (
    BoardOutline,
    CriticalRoutingStatus,
    CopperLayer,
    CopperKeepout,
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
)


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


def test_detailed_router_reports_search_budget_without_emitting_partial_copper() -> None:
    board = _board()
    guide = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))

    result = route_detailed(board, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1, maximum_search_states=1,
    ))

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert "search budget" in result.nets[0].diagnostics[0]
    assert not result.board.tracks


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


def test_detailed_router_fails_closed_without_a_connected_guide() -> None:
    board = replace(
        _board(),
        keepouts=(
            PlacementKeepout(
                "wall", BoardOutline.rectangle(4, 12, origin=Point.mm(8, 0))
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


def test_detailed_router_uses_inner_copper_beneath_surface_footprint() -> None:
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
    assert any(track.layer is CopperLayer.INTERNAL_1 for track in result.board.tracks)
    assert result.board.vias
    assert all(
        via.from_layer is CopperLayer.FRONT and via.to_layer is CopperLayer.BACK
        for via in result.board.vias
    )


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
    assert len(result.board.tracks) == 2


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
        pitch_nm=nm_from_mm("0.5"), maximum_passes=4,
    ))

    assert result.status is DetailedRoutingStatus.PARTIAL
    assert result.metrics.passes == 4
    assert not any(track.net == "SIGNAL" for track in result.board.tracks)


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

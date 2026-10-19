from __future__ import annotations

from dataclasses import replace

from pcbir import (
    BoardOutline,
    CopperLayer,
    DesignRules,
    FootprintPad,
    GlobalRouterOptions,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
    Stackup,
    TrackSegment,
    nm_from_mm,
    route_global,
    run_physical_drc,
)
from pcbir.route_summary import connection_summary
from pcbir.unified_router import UnifiedRouterOptions, route_unified


def _footprint() -> PhysicalFootprint:
    return PhysicalFootprint("test/one-pad", (FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),),
                             Size.mm(1, 1))


def _board(nets: dict[str, tuple[tuple[float, float], ...]], *, layers: int = 2,
           tracks: tuple[TrackSegment, ...] = ()) -> PhysicalBoard:
    footprint = _footprint()
    placements, physical_nets = [], []
    for name, points in nets.items():
        pads = []
        for index, (x, y) in enumerate(points):
            reference = f"{name}_{index}"
            placements.append(Placement(reference, footprint.name, Point.mm(x + 3, y + 3)))
            pads.append(PadReference(reference, "1"))
        physical_nets.append(PhysicalNet(name, tuple(pads)))
    return PhysicalBoard(
        "Unified", BoardOutline.rectangle(26, 26), {footprint.name: footprint},
        tuple(placements), tuple(physical_nets),
        stackup=Stackup.standard(layers) if layers != 2 else Stackup(),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.15"), minimum_track_width_nm=nm_from_mm("0.15"),
                          default_track_width_nm=nm_from_mm("0.2"), default_via_size_nm=nm_from_mm("0.6"),
                          default_via_drill_nm=nm_from_mm("0.3"), minimum_hole_clearance_nm=nm_from_mm("0.25")),
        tracks=tracks,
    )


def _route(board: PhysicalBoard, **options):
    # Global guides are planned without existing copper, as in the pipeline.
    global_route = route_global(replace(board, tracks=(), vias=()),
                                GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    result = route_unified(board, global_route, UnifiedRouterOptions(**options))
    drc = run_physical_drc(result.board)
    return result, drc, connection_summary(result.board, drc)


def test_single_net_routes_with_exact_clean_copper() -> None:
    result, drc, summary = _route(_board({"A": ((3, 10), (17, 10))}))
    assert summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0
    assert all(net.connected for net in result.nets)
    assert result.board.metadata["detailed_router"] == "unified"


def test_crossing_nets_negotiate_through_a_second_layer() -> None:
    # A horizontal and a vertical net must cross: one needs vias.
    board = _board({"H": ((2, 10), (18, 10)), "V": ((10, 2), (10, 18))})
    result, drc, summary = _route(board)
    assert summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0
    assert result.metrics.via_count >= 2


def test_many_parallel_crossings_converge_without_conflicts() -> None:
    nets = {f"H{i}": ((2, 4 + 2 * i), (18, 4 + 2 * i)) for i in range(5)}
    nets.update({f"V{i}": ((4 + 2.5 * i, 2), (4 + 2.5 * i, 18)) for i in range(5)})
    result, drc, summary = _route(_board(nets))
    assert summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0


def test_existing_copper_is_fixed_and_avoided() -> None:
    wall = tuple(TrackSegment("BLOCK", Point.mm(13, 3.5), Point.mm(13, 22.5), nm_from_mm("0.3"), layer)
                 for layer in (CopperLayer.FRONT, CopperLayer.BACK))
    board = _board({"A": ((3, 10), (17, 10)), "BLOCK": ((10, 0.5), (10, 19.5))}, tracks=wall)
    result, drc, summary = _route(board)
    # The wall net is complete already; A must go around it, not through.
    assert summary["drc"]["errors"] == 0
    assert set(wall) <= set(result.board.tracks)


def test_unroutable_net_is_reported_not_raised() -> None:
    # A pad outside every routable layer region is reported as open.
    board = _board({"A": ((3, 10), (17, 10))})
    result, drc, summary = _route(board, maximum_iterations=1, maximum_expansions=5)
    assert summary["signal"]["connections"] == 1
    assert not result.nets[0].connected or summary["signal"]["opens"] == 0


def test_pad_groups_sharing_an_attach_node_both_keep_their_lead_ins() -> None:
    # A fine-pitch connector row on one net (like a board-to-board supply
    # row): neighbouring pads attach to the same grid node, and each needs
    # its own lead-in. Regression: only one of them was emitted.
    row = PhysicalFootprint("test/row", tuple(
        FootprintPad(str(i + 1), Point.mm(0, f"{0.4 * i:.1f}"), Size.mm("0.2", "0.2")) for i in range(5)),
        Size.mm("0.6", "2.2"))
    one = _footprint()
    board = PhysicalBoard(
        "Unified", BoardOutline.rectangle(26, 26), {row.name: row, one.name: one},
        (Placement("J", row.name, Point.mm("11.5", "12.1")), Placement("T", one.name, Point.mm(20, 13))),
        (PhysicalNet("ROW", tuple(PadReference("J", str(i + 1)) for i in range(5)) + (PadReference("T", "1"),)),),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.1"), minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.15"), default_via_size_nm=nm_from_mm("0.6"),
                          default_via_drill_nm=nm_from_mm("0.3"), minimum_hole_clearance_nm=nm_from_mm("0.25")))
    result, drc, summary = _route(board)
    assert summary["signal"]["opens"] == 0
    assert summary["drc"]["errors"] == 0


def test_native_kernel_matches_python_search(monkeypatch) -> None:
    import pcbir.native as native
    if native.astar() is None:
        import pytest
        pytest.skip("no native kernel for this platform")
    nets = {f"H{i}": ((2, 4 + 2 * i), (18, 4 + 2 * i)) for i in range(4)}
    nets.update({f"V{i}": ((4 + 3 * i, 2), (4 + 3 * i, 18)) for i in range(4)})
    board = _board(nets)
    with_kernel, _, summary = _route(board)
    monkeypatch.setattr(native, "astar", lambda: None)
    without, _, summary_python = _route(board)
    assert summary["signal"]["opens"] == summary_python["signal"]["opens"] == 0
    assert set(with_kernel.board.tracks) == set(without.board.tracks)
    assert set(with_kernel.board.vias) == set(without.board.vias)

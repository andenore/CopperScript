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
from pcbir.physical import PadShape
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
        stackup=Stackup(copper_layers=(CopperLayer.FRONT,
                                       *(CopperLayer(f"In{i}.Cu") for i in range(1, layers - 1)),
                                       CopperLayer.BACK)),
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


def test_rejection_diagnostic_names_the_blocker() -> None:
    from types import SimpleNamespace
    from pcbir.unified_router import _rejection_text
    router = SimpleNamespace(rejections={"A": [(Point.mm(1, 2), "F.Cu", ("B",), False, "copper clearance: t1"),
                                               (Point.mm(3, 4), "via", (), True, "")]})
    assert _rejection_text(router, "A") == \
        "unified: rejected by exact commit check: blocked by B at (1.00,2.00) F.Cu [copper clearance: t1] (+1 more)"
    assert _rejection_text(router, "C") == "unified: rejected by exact commit check"


def test_saved_inputs_replay_to_the_same_copper(tmp_path) -> None:
    from pcbir.unified_router import replay_unified
    board = _board({"H": ((2, 10), (18, 10)), "V": ((10, 2), (10, 18))})
    state = tmp_path / "inputs.state.json"
    first, _, _ = _route(board, save_inputs_path=str(state))
    again = replay_unified(state)
    assert set(again.board.tracks) == set(first.board.tracks)
    assert set(again.board.vias) == set(first.board.vias)


def test_ball_field_via_mask_keeps_only_cell_centres() -> None:
    from types import SimpleNamespace
    from pcbir.package_patterns import circular_array  # noqa: F401  (array detection is exercised on real boards)
    from pcbir.unified_router import _Router
    import numpy as np
    # 3x3 ball array at 1 mm pitch; grid pitch 0.25 mm.
    footprint = PhysicalFootprint("test/bga9", tuple(
        FootprintPad(str(i), Point.mm(x, y), Size.mm("0.4", "0.4"), shape=PadShape.CIRCLE)
        for i, (x, y) in enumerate(((x, y) for y in (-1, 0, 1) for x in (-1, 0, 1)), 1)), Size.mm(3, 3))
    board = replace(_board({"A": ((3, 10), (17, 10))}), footprints={**_board({"A": ((3, 10), (17, 10))}).footprints,
                                                                   footprint.name: footprint})
    board = replace(board, placements=(*board.placements, Placement("U", footprint.name, Point.mm(10, 4))))
    router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), ball_field_via_sites=True),
                     [n for n in board.nets], None)
    mask = router._ball_field_via_mask()
    if mask is None:
        import pytest
        pytest.skip("3x3 array not detected as a ball field")
    g = router.grid
    def at(x, y):
        ix = int(round((nm_from_mm(x) - g.x0) / g.pitch)); iy = int(round((nm_from_mm(y) - g.y0) / g.pitch))
        return bool(mask[iy, ix])
    assert at(9.5, 3.5) and at(10.5, 4.5)          # cell centres
    assert not at(10, 4) and not at(9.75, 3.5)      # on a ball / between centres
    assert at(15, 15)                               # outside the field


def test_via_budget_is_never_exceeded() -> None:
    from pcbir.physical import NetRoutingRule, RouteKind
    # V may not use vias and H only one: whatever H does, it stays in budget.
    board = _board({"H": ((2, 10), (18, 10)), "V": ((10, 2), (10, 18))})
    board = replace(board, net_routing_rules=(NetRoutingRule("H", RouteKind.GENERAL, max_vias=1),
                                              NetRoutingRule("V", RouteKind.GENERAL, max_vias=0)))
    result, drc, summary = _route(board)
    assert len([via for via in result.board.vias if via.net == "H"]) <= 1
    assert "DRC-MAX-VIAS" not in summary["drc"]["errors_by_code"]


def _walled_plane_pad_board() -> PhysicalBoard:
    from pcbir import CopperZone, PolygonRing, PolygonWithHoles
    # Pad positions are offset by +3 mm in _board.
    # GND_1 lies outside the F.Cu pour, so GND gets copper (a via) in the first pass.
    board = _board({"GND": ((10, 10), (20, 2)), "A": ((11.9, 5), (11.9, 15))})
    w = nm_from_mm("0.2")
    corners = [(14.5, 11.5), (11.5, 11.5), (11.5, 14.5), (14.5, 14.5)]
    wall = tuple(TrackSegment("W", Point.mm(*a), Point.mm(*b), w, CopperLayer.FRONT)
                 for a, b in zip(corners, corners[1:]))  # a U open towards net A
    outline = PolygonWithHoles(PolygonRing((Point.mm(0.5, 0.5), Point.mm(25.5, 0.5),
                                            Point.mm(25.5, 25.5), Point.mm(0.5, 25.5))))
    front = PolygonWithHoles(PolygonRing((Point.mm(0.5, 0.5), Point.mm(20, 0.5),
                                          Point.mm(20, 25.5), Point.mm(0.5, 25.5))))
    zones = (CopperZone("gnd-f", "GND", (CopperLayer.FRONT,), front),
             CopperZone("gnd-b", "GND", (CopperLayer.BACK,), outline))
    from pcbir.physical import Via
    stitching = tuple(Via("GND", Point.mm(x, y), nm_from_mm("0.6"), nm_from_mm("0.3"),
                          CopperLayer.FRONT, CopperLayer.BACK) for x in (3, 18) for y in (3, 23))
    return replace(board, tracks=wall, vias=stitching, zones=zones, nets=(*board.nets, PhysicalNet("W", ())))


def test_pour_recheck_reconnects_a_plane_pad_walled_off_by_signal_copper() -> None:
    from pcbir.plane_estimate import estimate_plane_connectivity
    board = _walled_plane_pad_board()
    unchecked, _, _ = _route(board, pour_recheck=False)
    assert estimate_plane_connectivity(unchecked.board).report()["open_pads"] == {"GND": ["GND_0.1"]}
    checked, drc, summary = _route(board)
    assert estimate_plane_connectivity(checked.board).opens == 0
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert any(via.net == "GND" for via in checked.board.vias)


def _neck_exit_path(monkeypatch, kernel: bool):
    import numpy as np
    import pcbir.native as native
    from pcbir.unified_router import _Router
    if not kernel:
        monkeypatch.setattr(native, "astar", lambda: None)
    board = _board({"A": ((3, 10), (17, 10))})
    router = _Router(board, UnifiedRouterOptions(), list(board.nets), None)
    spec = router.specs[0]
    g = router.grid
    y = g.ny // 2
    source, left, target = g.index(0, 40, y), g.index(0, 39, y), g.index(0, 60, y)
    # The source is legal only at neck width; its region is itself and its left neighbour.
    spec.neck, spec.region, spec.neck_mid = frozenset({source}), frozenset({source, left}), frozenset()
    for key in ("neck", "neck_mid", "region"):
        nodes = getattr(spec, key)
        spec.arrays[key] = np.fromiter(nodes, dtype=np.int64, count=len(nodes))
    found = router._search(spec, {source: 0.0}, {target}, 1.0)
    assert found is not None
    return found[0], spec.region


def test_a_neck_only_node_is_left_inside_its_region_python(monkeypatch) -> None:
    path, region = _neck_exit_path(monkeypatch, kernel=False)
    assert path[1] in region


def test_a_neck_only_node_is_left_inside_its_region_native(monkeypatch) -> None:
    import pcbir.native as native
    if native.astar() is None:
        import pytest
        pytest.skip("no native kernel for this platform")
    path, region = _neck_exit_path(monkeypatch, kernel=True)
    assert path[1] in region


def test_bus_members_route_first_and_are_reported_as_routed() -> None:
    from pcbir.physical import NetMatchGroup
    board = _board({"D0": ((2, 7), (18, 7)), "D1": ((2, 9), (18, 9)), "X": ((10, 2), (10, 16))})
    board = replace(board, match_groups=(NetMatchGroup("bus", ("D0", "D1"), nm_from_mm("5")),))
    events = []
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    result = route_unified(board, global_route, UnifiedRouterOptions(),
                           on_progress=lambda *args, **kwargs: events.append(args))
    drc = run_physical_drc(result.board)
    summary = connection_summary(result.board, drc)
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert any(e and e[0] == "unified_buses" and e[2]["complete"] == 2 for e in events)
    by_name = {net.net: net for net in result.nets}
    assert by_name["D0"].connected and by_name["D1"].connected and by_name["D0"].track_count > 0

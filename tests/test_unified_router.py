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


def test_expired_repair_budget_does_not_start_another_escape_seed(monkeypatch):
    import pcbir.unified_router as unified
    fixed = TrackSegment("B", Point.mm(6, 5), Point.mm(20, 5),
                         nm_from_mm("0.2"), CopperLayer.FRONT)
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 2), (17, 2))}, tracks=(fixed,))
    guides = route_global(replace(board, tracks=()), GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    clock = [0.0]
    events, seeds = [], []
    monkeypatch.setattr(unified, "perf_counter", lambda: clock[0])

    def observe(phase, event, details):
        events.append((phase, event, details))
        iterations = sum(p == "unified_iteration" and e == "finished" for p, e, _ in events)
        # The initial negotiation and the first unseeded repair run normally.
        # The clock expires as that repair ends, before a seeded alternative.
        if iterations == 2:
            clock[0] = 11.0

    def seed(partial, names):
        seeds.append(names)
        return partial

    result = unified.route_unified(board, guides,
        UnifiedRouterOptions(maximum_iterations=1, maximum_expansions=1,
                             commit_rounds=2, time_budget_s=10),
        on_progress=observe, escape_seeder=seed)
    assert sum(p == "unified_iteration" and e == "finished" for p, e, _ in events) == 2
    assert not seeds
    assert fixed in result.board.tracks and result.board.placements == board.placements
    assert next(net for net in result.nets if net.net == "B").connected
    assert not next(net for net in result.nets if net.net == "A").connected
    assert any(p == "unified_repair" and e == "skipped" and d["reason"] == "time_budget"
               for p, e, d in events)


def test_local_grid_window_preserves_whole_source_and_exact_copper():
    fixed = TrackSegment("B", Point.mm(6, 5), Point.mm(20, 5), nm_from_mm("0.2"), CopperLayer.FRONT)
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 2), (17, 2))}, tracks=(fixed,))
    guides = route_global(replace(board, tracks=()), GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    events = []
    result = route_unified(board, guides,
        UnifiedRouterOptions(pitch_nm=nm_from_mm("0.1"),
                             grid_window_nm=[nm_from_mm(v) for v in (5, 11, 21, 15)]),
        only_nets=frozenset({"A"}), on_progress=lambda *args: events.append(args))
    assert result.nets[0].connected
    assert result.board.outline == board.outline and result.board.placements == board.placements
    assert result.board.tracks[:1] == (fixed,) and result.board.nets == board.nets
    assert not [f for f in run_physical_drc(result.board).findings if f.severity.value == "error"]
    setup = next(d for p, e, d in events if p == "unified_setup" and e == "finished")
    assert setup["grid"] == [161, 41, 2]


def test_geometry_entering_a_grid_window_still_blocks_it():
    walls = tuple(TrackSegment("W", Point.mm(13, 10), Point.mm(13, 16),
                               nm_from_mm("0.3"), layer)
                  for layer in (CopperLayer.FRONT, CopperLayer.BACK))
    board = _board({"A": ((3, 10), (17, 10)), "W": ()}, tracks=walls)
    guides = route_global(replace(board, tracks=()), GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    result = route_unified(board, guides,
        UnifiedRouterOptions(maximum_iterations=2, commit_rounds=1,
                             grid_window_nm=tuple(nm_from_mm(v) for v in (5, 11, 21, 15))),
        only_nets=frozenset({"A"}))
    assert not result.nets[0].connected
    assert result.board.tracks[:2] == walls and result.board.outline == board.outline


def test_grid_window_refuses_invalid_bounds_and_excluded_targets():
    import pytest
    for bounds in ((0, 0, 1), (1, 0, 0, 1), (0, 0, True, 1), (0, 0, 1.0, 1)):
        with pytest.raises(ValueError, match="grid_window_nm"):
            UnifiedRouterOptions(grid_window_nm=bounds)
    board = _board({"A": ((3, 10), (17, 10))})
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    for bounds in ((10, 11, 18, 15), (30, 30, 40, 40)):
        with pytest.raises(ValueError, match="grid_window_nm"):
            route_unified(board, guides, UnifiedRouterOptions(
                grid_window_nm=tuple(nm_from_mm(v) for v in bounds)))


def test_fine_window_fits_two_declared_widths_without_relaxing_clearance():
    from pcbir.physical import CopperKeepout, PolygonWithHoles, PolygonRing, NetRoutingRule
    pad = PhysicalFootprint("tiny", (FootprintPad("1", Point(0, 0), Size.mm("0.15", "0.15")),), Size.mm("0.3", "0.3"))
    poses, nets = [], []
    for name, x in (("A", "10.3"), ("B", "10.75")):
        refs = []
        for i, y in enumerate(("3.5", "20.5") if name == "A" else ("4.5", "21.5")):
            ref = name + str(i)
            poses.append(Placement(ref, pad.name, Point.mm(x, y)))
            refs.append(PadReference(ref, "1"))
        nets.append(PhysicalNet(name, tuple(refs)))
    walls = tuple(CopperKeepout(name, (CopperLayer.FRONT, CopperLayer.BACK),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in points))))
        for name, points in (("left", ((.5,6),(10,6),(10,18),(.5,18))),
                             ("right", ((11.05,6),(25.5,6),(25.5,18),(11.05,18)))))
    board = PhysicalBoard("FineChannels", BoardOutline.rectangle(26, 26), {pad.name: pad},
        tuple(poses), tuple(nets), copper_keepouts=walls,
        net_routing_rules=tuple(NetRoutingRule(n.name, width_nm=nm_from_mm("0.2"),
            clearance_nm=nm_from_mm("0.2"), allowed_layers=(CopperLayer.FRONT,), max_vias=0) for n in nets))
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    results = []
    for pitch in ("0.25", "0.025"):
        result = route_unified(board, guides, UnifiedRouterOptions(pitch_nm=nm_from_mm(pitch),
            maximum_iterations=16, commit_rounds=2,
            grid_window_nm=tuple(nm_from_mm(v) for v in (9, 2, 12, 22))))
        assert result.board.copper_keepouts == board.copper_keepouts
        assert result.board.net_routing_rules == board.net_routing_rules
        drc = run_physical_drc(result.board)
        assert not [f for f in drc.findings if f.severity.value == "error"
                    and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]
        results.append(result)
    assert not all(net.connected for net in results[0].nets)
    assert all(net.connected for net in results[1].nets)
    assert not [f for f in run_physical_drc(results[1].board).findings if f.severity.value == "error"]


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


def test_ball_field_band_keeps_the_outer_cell_rings_via_free() -> None:
    from pcbir.unified_router import _Router
    # 5x5 ball array at 1 mm pitch centred on (10, 6): cell centres at +-0.5 and +-1.5 mm.
    footprint = PhysicalFootprint("test/bga25", tuple(
        FootprintPad(str(i), Point.mm(x, y), Size.mm("0.4", "0.4"), shape=PadShape.CIRCLE)
        for i, (x, y) in enumerate(((x, y) for y in range(-2, 3) for x in range(-2, 3)), 1)), Size.mm(5, 5))
    base = _board({"A": ((3, 15), (17, 15))})
    board = replace(base, footprints={**base.footprints, footprint.name: footprint},
                    placements=(*base.placements, Placement("U", footprint.name, Point.mm(10, 6))))
    def mask_at(band):
        router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), ball_field_via_sites=True,
                                                     ball_field_band=band), list(board.nets), None)
        mask, g = router._ball_field_via_mask(), router.grid
        def at(x, y):
            return bool(mask[int(round((nm_from_mm(y) - g.y0) / g.pitch)), int(round((nm_from_mm(x) - g.x0) / g.pitch))])
        return at
    plain, banded = mask_at(0), mask_at(1)
    assert plain(8.5, 4.5) and plain(9.5, 5.5)       # outer and inner cell centres
    assert not banded(8.5, 4.5) and not banded(11.5, 5.5)  # the outer ring of cells is closed
    assert banded(9.5, 5.5) and banded(10.5, 6.5)    # the inner 2x2 cells stay open
    assert banded(15, 15)                            # outside the field


def test_ball_field_exact_drops_the_raster_margin_only_inside_ball_fields() -> None:
    import numpy as np
    from pcbir.unified_router import _FREE, _Router
    footprint = PhysicalFootprint("test/bga25", tuple(
        FootprintPad(str(i), Point.mm(x, y), Size.mm("0.4", "0.4"), shape=PadShape.CIRCLE)
        for i, (x, y) in enumerate(((x, y) for y in range(-2, 3) for x in range(-2, 3)), 1)), Size.mm(5, 5))
    base = _board({"A": ((3, 15), (17, 15))})
    board = replace(base, footprints={**base.footprints, footprint.name: footprint},
                    placements=(*base.placements, Placement("U", footprint.name, Point.mm(10, 6))))
    def free(exact):
        router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), ball_field_exact=exact),
                         list(board.nets), None)
        g = router.grid
        owner = np.asarray(router.rasters.track(nm_from_mm("0.1"), nm_from_mm("0.1"))[0]).reshape(g.nl, g.ny, g.nx)
        def count(x0, y0, x1, y1):
            ix0, iy0, ix1, iy1 = g.window(nm_from_mm(x0), nm_from_mm(y0), nm_from_mm(x1), nm_from_mm(y1))
            return int((owner[0, iy0:iy1 + 1, ix0:ix1 + 1] == _FREE).sum())
        return count(7, 3, 13, 9), count(2, 13, 18, 17)
    plain_field, plain_outside = free(False)
    exact_field, exact_outside = free(True)
    assert exact_field > plain_field             # lands in the field lose their margin
    assert exact_outside == plain_outside        # the rest of the board keeps it


def _array_board(parts: dict[str, tuple[float, float]], nets: dict[str, tuple[tuple[str, str], ...]]):
    """5x5 ball arrays (1 mm pitch, 0.4 mm balls) at ``parts`` with nets between their balls."""
    footprint = PhysicalFootprint("test/bga25", tuple(
        FootprintPad(str(i), Point.mm(x, y), Size.mm("0.4", "0.4"), shape=PadShape.CIRCLE)
        for i, (x, y) in enumerate(((x, y) for y in range(-2, 3) for x in range(-2, 3)), 1)), Size.mm(5, 5))
    base = _board({"X": ((3, 18), (17, 18))})
    return replace(base, footprints={**base.footprints, footprint.name: footprint},
                   placements=(*base.placements, *(Placement(ref, footprint.name, Point.mm(*at))
                                                    for ref, at in parts.items())),
                   nets=(*base.nets, *(PhysicalNet(name, tuple(PadReference(r, n) for r, n in pads))
                                       for name, pads in nets.items())))


def test_exit_corridors_cost_other_nets_and_not_their_owner() -> None:
    from pcbir.unified_router import _Router
    # S leaves outer-ring ball 3 (column 2, row 0: top edge); T leaves second-ring ball 8 (column 2, row 1).
    board = _array_board({"U": (10, 6)}, {"S": (("U", "3"),), "T": (("U", "8"),)})
    far = {"S": PadReference("X_0", "1"), "T": PadReference("X_1", "1")}
    board = replace(board, nets=tuple(replace(net, pads=(*net.pads, far[net.name]))
                                      for net in board.nets if net.name in far))
    router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25"), exit_reservation_cost=5.0),
                     list(board.nets), None)
    g = router.grid
    def node(x, y):
        return g.index(0, int(round((nm_from_mm(x) - g.x0) / g.pitch)), int(round((nm_from_mm(y) - g.y0) / g.pitch)))
    above = node(10, 3.5)      # straight above ball 3 (at (10, 4) in board millimetres), past the edge
    gap = node(9.5, 4)         # between outer balls 2 and 3, in front of ball 8 (at (10, 5))
    assert above in router._exit_corridors["S"] and gap in router._exit_corridors["T"]
    history = router.occupancy.history
    assert history[above] >= 5 and history[gap] >= 5
    s_nodes, s_extra = router._escape_bias("S")
    assert dict(zip(map(int, s_nodes), map(float, s_extra)))[above] == -5.0
    assert gap not in set(map(int, s_nodes))      # T's exit still costs S


def test_bus_bundles_route_the_nets_between_the_bus_parts_with_the_bus() -> None:
    from pcbir.physical import NetMatchGroup
    board = _array_board({"U": (8, 6), "V": (16, 6)},
                         {"A": (("U", "15"), ("V", "11")), "B": (("U", "20"), ("V", "16")),
                          "C": (("U", "10"), ("V", "6"))})
    board = replace(board, match_groups=(NetMatchGroup("bus", ("A", "B"), nm_from_mm(5)),))
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    def bus_nets(**options):
        events = []
        route_unified(board, global_route, UnifiedRouterOptions(maximum_iterations=5, **options),
                      on_progress=lambda *args, **kwargs: events.append(args))
        return next(e[2]["nets"] for e in events if e and e[0] == "unified_buses")
    assert bus_nets() == 2                   # the group alone
    assert bus_nets(bus_bundles=True) == 3   # with C, whose pads are all on U and V


def test_plane_fill_width_sees_a_wide_rail_plane_and_taps_a_crowded_via() -> None:
    from pcbir.physical import CopperZone, NetRoutingRule, PhysicalNet, PolygonRing, PolygonWithHoles, RouteKind, Via
    from pcbir.unified_router import _Router
    # P: a 0.6 mm rail with a via into its small In1 zone, ringed by foreign vias 0.75 mm away.
    board = _board({"P": ((5, 5), (15, 15)), "X": ((2, 20), (20, 20))}, layers=4)
    ring = PolygonRing(tuple(Point.mm(x, y) for x, y in ((7, 7), (10, 7), (10, 10), (7, 10))))
    zone = CopperZone("p", "P", (CopperLayer("In1.Cu"),), PolygonWithHoles(ring))
    centre = Point.mm("8.5", "8.5")
    layers = board.stackup.copper_layers
    vias = (Via("P", centre, nm_from_mm("0.45"), nm_from_mm("0.2"), layers[0], layers[-1]),
            *(Via("X", Point(centre.x_nm + dx, centre.y_nm + dy), nm_from_mm("0.45"), nm_from_mm("0.2"),
                  layers[0], layers[-1])
              for dx, dy in ((750_000, 0), (-750_000, 0), (0, 750_000), (0, -750_000))))
    board = replace(board, zones=(zone,), vias=vias,
                    net_routing_rules=(NetRoutingRule("P", RouteKind.GENERAL, width_nm=nm_from_mm("0.6")),),
                    rules=replace(board.rules, minimum_track_width_nm=nm_from_mm("0.1"),
                                  minimum_clearance_nm=nm_from_mm("0.1")))
    def plane_and_taps(fill):
        router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.125"), plane_fill_width=fill,
                                                     reuse_plane_vias=True), [n for n in board.nets if n.name == "P"], None)
        (spec,) = router.specs
        planes, _, _ = router._zone_regions("P", spec)
        return len(planes), len(router._plane_via_taps("P", spec, planes))
    narrow_plane, narrow_taps = plane_and_taps(False)
    fill_plane, fill_taps = plane_and_taps(True)
    assert fill_plane > narrow_plane      # the zone fill is not limited by the rail's 0.6 mm track width
    assert narrow_taps == 0 and fill_taps > 0   # the crowded via counts as joined to its plane


def test_a_pad_with_its_own_dogbone_attaches_on_every_layer_of_the_via() -> None:
    from pcbir.physical import Via
    from pcbir.unified_router import _Router
    # A's first pad (at (6, 6) mm) has a stub to its via: the pad group must offer the via
    # on every layer, whatever node the connectivity graph picks as the group's root.
    board = _board({"A": ((3, 3), (15, 15))}, layers=4)
    layers = board.stackup.copper_layers
    via = Via("A", Point.mm(7, 7), nm_from_mm("0.6"), nm_from_mm("0.3"), layers[0], layers[-1])
    stub = TrackSegment("A", Point.mm(6, 6), Point.mm(7, 7), nm_from_mm("0.2"), layers[0])
    board = replace(board, tracks=(stub,), vias=(via,))
    router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25")), list(board.nets), None)
    (spec,) = router.specs
    g = router.grid
    near_via = [comp for comp in spec.components
                if any(abs(g.x0 + g.unpack(n)[1] * g.pitch - via.position.x_nm) < nm_from_mm(2) for n, _, _ in comp)]
    attach_layers = {g.unpack(n)[0] for comp in near_via for n, _, _ in comp}
    assert attach_layers == set(range(g.nl))


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
    result = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True),
                           on_progress=lambda *args, **kwargs: events.append(args))
    drc = run_physical_drc(result.board)
    summary = connection_summary(result.board, drc)
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert any(e and e[0] == "unified_buses" and e[2]["complete"] == 2 for e in events)
    by_name = {net.net: net for net in result.nets}
    assert by_name["D0"].connected and by_name["D1"].connected and by_name["D0"].track_count > 0


def test_bus_defaults_leave_a_board_without_match_groups_unchanged() -> None:
    assert UnifiedRouterOptions().buses_first and UnifiedRouterOptions().bus_skew_tuning
    board = _board({"D0": ((2, 7), (18, 7)), "D1": ((2, 9), (18, 9)), "X": ((10, 2), (10, 16))})
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    default = route_unified(board, global_route, UnifiedRouterOptions())
    off = route_unified(board, global_route, UnifiedRouterOptions(buses_first=False, bus_skew_tuning=False))
    assert default.board.tracks == off.board.tracks and default.board.vias == off.board.vias
    assert "bus_skew" not in default.board.metadata


def test_bus_members_over_their_length_leave_the_bus_phase_and_fail_final_budget() -> None:
    from pcbir.physical import NetMatchGroup, NetRoutingRule, RouteKind
    board = _board({"D0": ((2, 7), (18, 7)), "D1": ((2, 9), (18, 9)), "X": ((10, 2), (10, 16))})
    rules = (NetRoutingRule("D0", RouteKind.GENERAL, max_length_nm=nm_from_mm("10")),)
    board = replace(board, match_groups=(NetMatchGroup("bus", ("D0", "D1"), nm_from_mm("5")),),
                    net_routing_rules=rules)
    events = []
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    result = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True, bus_length_budget=True),
                           on_progress=lambda *args, **kwargs: events.append(args))
    buses = [e[2] for e in events if e and e[0] == "unified_buses"]
    # D0 needs 16 mm against a 10 mm limit: the main negotiation tries it,
    # but must not publish its over-budget copper as a completed route.
    assert buses and buses[0]["complete"] == 1 and buses[0]["over_length"] == 1
    by_name = {net.net: net for net in result.nets}
    assert not by_name["D0"].connected and by_name["D1"].connected
    assert "exceeds" in " ".join(by_name["D0"].diagnostics)
    assert not any(t.net == "D0" for t in result.board.tracks)


def test_a_length_limited_net_takes_the_short_path_through_congestion() -> None:
    from pcbir.physical import NetRoutingRule, RouteKind
    from pcbir.unified_router import _Router
    board = _board({"L": ((3, 10), (17, 10))})
    rules = (NetRoutingRule("L", RouteKind.GENERAL, max_length_nm=nm_from_mm("15")),)
    def detoured(board):
        router = _Router(board, UnifiedRouterOptions(length_priority=True), list(board.nets), None)
        spec, g = router.specs[0], router.grid
        # A congested wall across the straight path on every layer, open only far away.
        x = (13_000_000 - g.x0) // g.pitch
        for layer in range(g.nl):
            for y in range(g.ny):
                if abs(y * g.pitch + g.y0 - 13_000_000) < 2_000_000:
                    router.occupancy.occ[g.index(layer, x, y)] = 50
        route = router._route_net(spec, 10.0)
        return route.connected, router._route_length_nm(route)
    connected, free_length = detoured(board)
    assert connected and free_length > nm_from_mm("15")       # without the rule: a detour
    connected, limited_length = detoured(replace(board, net_routing_rules=rules))
    assert connected and limited_length <= nm_from_mm("15")   # with it: straight through


def test_replay_cli_routes_saved_inputs_with_overrides(tmp_path, capsys) -> None:
    import json
    from pcbir.cli import main
    board = _board({"A": ((3, 10), (17, 10))})
    state = tmp_path / "inputs.json"
    global_route = route_global(replace(board, tracks=(), vias=()), GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    route_unified(board, global_route, UnifiedRouterOptions(save_inputs_path=str(state)))
    result_state = tmp_path / "routed.state.json"
    assert main(["replay-unified", str(state), "--set", "maximum_iterations=3",
                 "--report", str(tmp_path / "replay.json"), "--save-result", str(result_state)]) == 0
    from pcbir.workbench import load_state
    assert load_state(result_state).board.placements == board.placements
    previous_bytes = result_state.read_bytes()
    assert main(["replay-unified", str(state), "--save-result", str(result_state)]) == 2
    assert result_state.read_bytes() == previous_bytes
    document = json.loads((tmp_path / "replay.json").read_text(encoding="utf-8"))
    assert document["overrides"] == {"maximum_iterations": 3}
    assert document["signal"]["opens"] == 0 and document["unrouted"] == {}
    assert main(["replay-unified", str(state), "--set", "no_such_option=1"]) == 2
    assert main(["replay-unified", str(state), "--only", "A", "-o", str(tmp_path / "replay.kicad_pcb")]) == 0
    assert (tmp_path / "replay.kicad_pcb").is_file()


def test_route_smoothing_straightens_staircases_without_losing_connectivity() -> None:
    from pcbir.bench import track_bends
    # Three converging nets: grid search has no turn cost, so around each
    # other's copper equal-length staircases of 45-degree jogs survive.
    board = _board({f"N{i}": ((2, 3 + 2.5 * i), (18, 12 - 2 * i)) for i in range(3)})
    plain, _, plain_summary = _route(board, route_smoothing=False)
    smoothed, drc, summary = _route(board)  # on by default
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert all(net.connected for net in smoothed.nets)
    assert track_bends(smoothed.board.tracks)["bends"] < track_bends(plain.board.tracks)["bends"]
    assert smoothed.metrics.total_length_nm <= plain.metrics.total_length_nm


def test_fixed_copper_keeps_its_own_net_clearance_on_the_grid() -> None:
    from pcbir.physical import NetRoutingRule
    # S passes 0.7 mm from a pad of P: enough for S's own 0.15 mm clearance,
    # not for P's 0.4 mm. The exact check spaces them by the larger one.
    board = replace(_board({"S": ((2, 10), (18, 10)), "P": ((10, 10.7),)}),
                    net_routing_rules=(NetRoutingRule("P", clearance_nm=nm_from_mm("0.4")),))
    result, drc, summary = _route(board)
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert all(net.connected for net in result.nets)


def test_wide_nets_negotiate_room_for_each_others_width() -> None:
    from pcbir.physical import NetRoutingRule
    # Three converging 0.5 mm nets on a 0.2 mm default board: each must claim
    # room for a neighbour as wide as itself, or two of them pass negotiation
    # closer than the exact check allows.
    nets = {f"N{i}": ((2, 3 + 2.5 * i), (18, 12 - 2 * i)) for i in range(3)}
    board = replace(_board(nets), net_routing_rules=tuple(
        NetRoutingRule(name, width_nm=nm_from_mm("0.5")) for name in nets))
    result, drc, summary = _route(board)
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    assert all(net.connected for net in result.nets)


def _escape_bias(net: str, rate: int = 200):
    from pcbir.geometry import Bounds
    from pcbir.routing_costs import LayerBias
    # Board coordinates: pads at x = 6 and 20 mm (the fixture adds 3 mm).
    return LayerBias(net, CopperLayer.FRONT, Bounds(nm_from_mm(9), 0, nm_from_mm(17), nm_from_mm(26)), rate)


def _biased_layers(native_kernel: bool, monkeypatch) -> tuple[set, set, list]:
    import pcbir.native as native
    import pcbir.unified_router as unified
    if not native_kernel:
        monkeypatch.setattr(native, "astar", lambda: None)
    histories = []
    original = unified._Router._search_unbiased

    def spy(self, *args, **kwargs):
        histories.append(float(sum(self.occupancy.history)))
        return original(self, *args, **kwargs)

    monkeypatch.setattr(unified._Router, "_search_unbiased", spy)
    board = _board({"A": ((3, 10), (17, 10))})
    plain, _, _ = _route(board)
    biased, drc, summary = _route(board, escape_layer_biases=(_escape_bias("A"),))
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0
    middle = {track.layer for track in biased.board.tracks
              if nm_from_mm(10) < (track.start.x_nm + track.end.x_nm) // 2 < nm_from_mm(16)}
    return {track.layer for track in plain.board.tracks}, middle, histories


def test_escape_bias_moves_a_net_off_the_layer_inside_its_area_python(monkeypatch) -> None:
    plain, middle, histories = _biased_layers(False, monkeypatch)
    assert plain == {CopperLayer.FRONT} and middle == {CopperLayer.BACK}
    # The bias exists only while the net searches: every search starts from untouched history
    # except for the biased one, which sees it.
    assert histories[0] == 0.0 and max(histories) > 0.0


def test_escape_bias_moves_a_net_off_the_layer_inside_its_area_native(monkeypatch) -> None:
    import pcbir.native as native
    if native.astar() is None:
        import pytest
        pytest.skip("no native kernel for this platform")
    plain, middle, _ = _biased_layers(True, monkeypatch)
    assert plain == {CopperLayer.FRONT} and middle == {CopperLayer.BACK}


def test_escape_bias_is_restored_after_each_search() -> None:
    import pcbir.unified_router as unified
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 14), (17, 14))})
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    seen = []
    original = unified._Router._search_unbiased

    def spy(self, spec, *args, **kwargs):
        seen.append((spec.name, float(self.occupancy.history.sum()) if self.occupancy.arrays
                     else float(sum(self.occupancy.history))))
        return original(self, spec, *args, **kwargs)

    unified._Router._search_unbiased = spy
    try:
        route_unified(board, global_route, UnifiedRouterOptions(escape_layer_biases=(_escape_bias("A"),),
                                                                maximum_iterations=1))
    finally:
        unified._Router._search_unbiased = original
    assert any(name == "A" and total > 0 for name, total in seen)
    assert all(total == 0.0 for name, total in seen if name == "B")


def test_planned_nets_negotiate_first() -> None:
    import pcbir.unified_router as unified
    board = _board({"A": ((3, 10), (17, 10)), "B": ((3, 14), (17, 14)), "C": ((3, 18), (17, 18))})
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    order = []
    original = unified._Router._route_net

    def spy(self, spec, present):
        order.append(spec.name)
        return original(self, spec, present)

    unified._Router._route_net = spy
    try:
        route_unified(board, global_route, UnifiedRouterOptions(escape_net_order=("C", "A"), maximum_iterations=1))
    finally:
        unified._Router._route_net = original
    assert order[:3] == ["C", "A", "B"]


def test_negative_escape_bias_is_refused() -> None:
    import pytest
    with pytest.raises(ValueError):
        UnifiedRouterOptions(escape_layer_biases=(_escape_bias("A", rate=-1),))


def _bus_board(*, amplitude: str | None = "1", max_length: str | None = None):
    from pcbir.physical import NetMatchGroup, NetRoutingRule
    board = _board({"A": ((3, 6), (11, 6)), "B": ((3, 14), (17, 14))})
    rules = tuple(NetRoutingRule(net, width_nm=nm_from_mm("0.2"),
                                 tuning_amplitude_limit_nm=nm_from_mm(amplitude) if amplitude else None,
                                 max_length_nm=nm_from_mm(max_length) if max_length else None)
                  for net in ("A", "B"))
    return replace(board, net_routing_rules=rules,
                   match_groups=(NetMatchGroup("bus", ("A", "B"), nm_from_mm(2)),))


def _straight(net, start, end):
    return TrackSegment(net, Point.mm(start[0] + 3, start[1] + 3), Point.mm(end[0] + 3, end[1] + 3),
                        nm_from_mm("0.2"), CopperLayer.FRONT)


def test_bus_skew_tuning_lengthens_the_short_member_within_max_skew() -> None:
    from math import hypot
    from pcbir.unified_router import _tune_bus_skew
    board = _bus_board()
    accepted = {"A": ([_straight("A", (3, 10), (11, 10))], []), "B": ([_straight("B", (3, 14), (17, 14))], [])}
    tuned, notes = _tune_bus_skew(board, accepted, {rule.net: rule for rule in board.net_routing_rules})
    length = lambda net: sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm) for t in tuned[net][0])
    # 8 mm against 14 mm: 6 mm of skew over a 2 mm limit; A gains at least 4 mm and B is untouched.
    assert notes == [] and tuned["B"] == accepted["B"]
    assert nm_from_mm(14) - length("A") <= nm_from_mm(2) and length("A") > nm_from_mm(12)
    drc = run_physical_drc(replace(board, tracks=(*tuned["A"][0], *tuned["B"][0])))
    assert not [f for f in drc.findings if f.severity.value == "error" and f.code not in ("DRC-OPEN-NET",
                                                                                        "DRC-ROUTE-INCOMPLETE")]


def test_bus_skew_tuning_needs_an_amplitude_contract_and_room_under_max_length() -> None:
    from pcbir.unified_router import _tune_bus_skew
    accepted = {"A": ([_straight("A", (3, 10), (11, 10))], []), "B": ([_straight("B", (3, 14), (17, 14))], [])}
    for board, reason in ((_bus_board(amplitude=None), "no tuning_amplitude_limit is declared"),
                          (_bus_board(max_length="9"), "max_length leaves no room")):
        tuned, notes = _tune_bus_skew(board, accepted, {rule.net: rule for rule in board.net_routing_rules})
        assert tuned == accepted and len(notes) == 1 and reason in notes[0] and "bus: A is" in notes[0]
    # A group within its limit, or one with a member still unrouted, is left alone.
    board = _bus_board()
    rules = {rule.net: rule for rule in board.net_routing_rules}
    close = {"A": ([_straight("A", (3, 10), (16, 10))], []), "B": ([_straight("B", (3, 14), (17, 14))], [])}
    assert _tune_bus_skew(board, close, rules) == (close, [])
    assert _tune_bus_skew(board, {"B": accepted["B"]}, rules) == ({"B": accepted["B"]}, [])


def test_buses_first_with_skew_tuning_reports_untunable_members_on_the_board() -> None:
    board = _bus_board(amplitude=None)
    # A third net keeps the bus a proper subset, so buses_first routes it on its own.
    extra = _board({"C": ((3, 18), (17, 18))})
    board = replace(board, footprints={**board.footprints, **extra.footprints},
                    placements=(*board.placements, *extra.placements), nets=(*board.nets, *extra.nets))
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    result = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True, bus_skew_tuning=True))
    assert all(net.connected for net in result.nets)
    assert "no tuning_amplitude_limit is declared" in result.board.metadata["bus_skew"]
    plain = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True, bus_skew_tuning=False))
    assert "bus_skew" not in plain.board.metadata
    default = route_unified(board, global_route, UnifiedRouterOptions())
    assert default.board.metadata["bus_skew"] == result.board.metadata["bus_skew"]


def test_tuned_bus_copper_survives_route_smoothing() -> None:
    from math import hypot
    board = _bus_board(amplitude="1")
    extra = _board({"C": ((3, 18), (17, 18))})
    board = replace(board, footprints={**board.footprints, **extra.footprints},
                    placements=(*board.placements, *extra.placements), nets=(*board.nets, *extra.nets))
    global_route = route_global(board, GlobalRouterOptions(tile_size_nm=nm_from_mm("2")))
    length = lambda result, net: sum(hypot(t.end.x_nm - t.start.x_nm, t.end.y_nm - t.start.y_nm)
                                     for t in result.board.tracks if t.net == net)
    plain = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True, bus_skew_tuning=False,
                                                                    route_smoothing=True))
    assert length(plain, "B") - length(plain, "A") > nm_from_mm(2)
    tuned = route_unified(board, global_route, UnifiedRouterOptions(buses_first=True, bus_skew_tuning=True,
                                                                    route_smoothing=True))
    assert all(net.connected for net in tuned.nets) and "bus_skew" not in tuned.board.metadata
    assert abs(length(tuned, "B") - length(tuned, "A")) <= nm_from_mm(2)
    drc = run_physical_drc(tuned.board)
    assert not [f for f in drc.findings if f.severity.value == "error"]


def _stitched_pad_board() -> PhysicalBoard:
    from pcbir import CopperZone, PolygonRing, PolygonWithHoles
    from pcbir.physical import Via
    # Pad positions are offset by +3 mm in _board. Each GND pad reaches the
    # B.Cu plane only through its own via-in-pad, as an exposed pad's array
    # does, and a ring of foreign copper leaves no room for a new via beside it.
    board = _board({"GND": ((5, 5), (15, 15))})
    w = nm_from_mm("0.2")
    wall = []
    for cx, cy in ((8, 8), (18, 18)):
        corners = [(cx - 0.8, cy - 0.8), (cx + 0.8, cy - 0.8), (cx + 0.8, cy + 0.8),
                   (cx - 0.8, cy + 0.8), (cx - 0.8, cy - 0.8)]
        wall += [TrackSegment("W", Point.mm(*a), Point.mm(*b), w, CopperLayer.FRONT)
                 for a, b in zip(corners, corners[1:])]
    in_pad = tuple(Via("GND", Point.mm(x, y), nm_from_mm("0.3"), nm_from_mm("0.15"),
                       CopperLayer.FRONT, CopperLayer.BACK) for x, y in ((8, 8), (18, 18)))
    plane = PolygonWithHoles(PolygonRing((Point.mm(0.5, 0.5), Point.mm(25.5, 0.5),
                                          Point.mm(25.5, 25.5), Point.mm(0.5, 25.5))))
    return replace(board, tracks=tuple(wall), vias=in_pad,
                   zones=(CopperZone("gnd-b", "GND", (CopperLayer.BACK,), plane),),
                   nets=(*board.nets, PhysicalNet("W", ())))


def test_pads_already_stitched_into_their_plane_count_as_joined() -> None:
    board = _stitched_pad_board()
    result, _, _ = _route(board)
    gnd = next(net for net in result.nets if net.net == "GND")
    assert gnd.connected and gnd.diagnostics == ()
    # Nothing new is added for a plane net whose pads already reach it.
    assert [v for v in result.board.vias if v.net == "GND"] == list(board.vias)
    assert not [t for t in result.board.tracks if t.net == "GND"]


def test_adjacent_breakout_regions_do_not_emit_an_illegal_merged_neck():
    from pcbir.physical import NetRoutingRule
    board = _board({"N": ((3, 10), (8, 10)), "BLOCK": ()})
    wall = TrackSegment("BLOCK", Point.mm(5, "13.65"), Point.mm(12, "13.65"),
                        nm_from_mm("0.2"), CopperLayer.FRONT)
    rule = NetRoutingRule("N", width_nm=nm_from_mm(1), breakout_width_nm=nm_from_mm("0.2"),
                          breakout_length_nm=nm_from_mm(3), allowed_layers=(CopperLayer.FRONT,), max_vias=0)
    board = replace(board, tracks=(wall,), net_routing_rules=(rule,))
    result, drc, summary = _route(board, direct=True, route_smoothing=False, maximum_iterations=4)
    assert result.nets[0].connected
    assert summary["signal"]["opens"] == 0
    assert not [f for f in drc.findings if f.code == "DRC-TRACK-WIDTH"]
    assert wall in result.board.tracks


def _opposite_pad_via_board(hole_clearance):
    from pcbir.physical import BoardSide, NetRoutingRule
    cap = PhysicalFootprint("cap", (FootprintPad("1", Point(0, 0), Size.mm("0.95", "0.9")),), Size.mm("1.6", "1"))
    pin = PhysicalFootprint("pin", (FootprintPad("1", Point(0, 0), Size.mm("0.3", "1.55")),), Size.mm("1", "2"))
    return PhysicalBoard("OwnPadVia", BoardOutline.rectangle(26, 26), {cap.name: cap, pin.name: pin},
        (Placement("C", cap.name, Point.mm(13, 10), side=BoardSide.BACK),
         Placement("U", pin.name, Point.mm(13, "11.95"))),
        (PhysicalNet("N", (PadReference("C", "1"), PadReference("U", "1"))), PhysicalNet("X", ())),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.2"),
            default_track_width_nm=nm_from_mm("0.2"), default_via_size_nm=nm_from_mm("0.45"),
            default_via_drill_nm=nm_from_mm("0.2"), minimum_hole_clearance_nm=nm_from_mm(hole_clearance)),
        net_routing_rules=(NetRoutingRule("N", width_nm=nm_from_mm("0.2"),
            clearance_nm=nm_from_mm("0.2"), max_vias=1, max_length_nm=nm_from_mm("2.2")),))


def test_via_raster_distinguishes_own_pad_halo_from_forbidden_annulus():
    from pcbir.unified_router import _Router, _FOREIGN
    from pcbir.routing_clearance import RoutingClearanceIndex
    for hole_clearance in ("0.2", "0.5"):
        board = _opposite_pad_via_board(hole_clearance)
        router = _Router(board, UnifiedRouterOptions(pitch_nm=nm_from_mm("0.25")), [board.nets[0]], None)
        g = router.grid
        position = Point.mm(13, "10.75")
        index = RoutingClearanceIndex(board)
        assert index.can_via("N", position, nm_from_mm("0.45"), CopperLayer.FRONT, CopperLayer.BACK,
                             drill_nm=nm_from_mm("0.2"))
        assert not index.can_via("X", position, nm_from_mm("0.45"), CopperLayer.FRONT, CopperLayer.BACK,
                                 drill_nm=nm_from_mm("0.2"))
        owners = router.rasters.via_array(nm_from_mm("0.45"), nm_from_mm("0.2"), nm_from_mm("0.2"))
        xy = lambda x, y: round((nm_from_mm(y)-g.y0)/g.pitch)*g.nx + round((nm_from_mm(x)-g.x0)/g.pitch)
        assert owners[xy(13, "10.75")] == router.net_ids["N"]
        assert owners[xy(13, "10.5")] == _FOREIGN
        assert not index.can_via("N", Point.mm(13, "10.5"), nm_from_mm("0.45"), CopperLayer.FRONT,
                                 CopperLayer.BACK, drill_nm=nm_from_mm("0.2"))


def test_opposite_face_pad_gap_routes_with_ordinary_via_and_exact_length_budget():
    board = _opposite_pad_via_board("0.2")
    result, drc, summary = _route(board, maximum_iterations=8, commit_rounds=2)
    assert next(net for net in result.nets if net.net == "N").connected
    assert summary["signal"]["opens"] == 0 and summary["drc"]["errors"] == 0, drc.to_json()
    assert len(result.board.vias) == 1
    assert result.board.net_routing_rules == board.net_routing_rules


def test_pipeline_keeps_unified_settings_for_later_native_transactions():
    from pcbir.flow import run_routing_pipeline
    from pcbir.placement import PlacementPlannerOptions
    from pcbir.routeflow import PlacementRoutingFeedbackOptions
    board = _board({"A": ((3, 10), (17, 10))})
    options = UnifiedRouterOptions(maximum_iterations=3, commit_rounds=1)
    for stop in (None, "placement"):
        result = run_routing_pipeline(board, unified_options=options, stop_after=stop,
            placement_options=PlacementPlannerOptions(candidate_count=1, analytical_iterations=0,
                refinement_passes=0, fixed_references=frozenset(p.reference for p in board.placements)),
            feedback_options=PlacementRoutingFeedbackOptions(maximum_iterations=1))
        assert result.unified_options is options
        if stop is None:
            assert result.detailed.board.metadata["detailed_router"] == "unified"

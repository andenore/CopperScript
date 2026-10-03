"""Off-ray physical escape sites, exact anchor verification and owned cleanup."""
from dataclasses import replace
from pathlib import Path
import shutil
import pytest

from pcbir.fanout import FanoutOptions, route_fanout, _two_leg_candidates
from pcbir.pin_escape import verified_fanout_path
from pcbir.detailed import DetailedRouterOptions, _prune_fanout_copper, route_detailed
from pcbir.routing import GlobalRouterOptions, route_global
from pcbir.routing_clearance import RoutingClearanceIndex
from pcbir.drc import run_physical_drc
from pcbir.physical import (BoardOutline, CopperKeepout, CopperLayer, FootprintPad,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
    Point, PolygonRing, PolygonWithHoles, Size, Stackup, TrackSegment, nm_from_mm)


def base_board(block_first_order=True):
    package = PhysicalFootprint("two", (
        FootprintPad("1", Point.mm(0, -1), Size.mm(.3, .3)),
        FootprintPad("2", Point.mm(0, 1), Size.mm(.3, .3)),
    ), Size.mm(2, 3))
    terminal = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(1, 1))
    board = PhysicalBoard("two-leg", BoardOutline.rectangle(14, 14),
        {"two": package, "one": terminal}, (
            Placement("U", "two", Point.mm(5, 7)), Placement("J", "one", Point.mm(11, 6)),
        ), (PhysicalNet("A", (PadReference("U", "1"), PadReference("J", "1"))),))
    # Only a via at (4,4) fits the window; no radial ray from (5,6) reaches it.
    axes = (0, 3.55, 4.45, 14)
    keepouts = tuple(CopperKeepout(f"via-cell-{i}-{j}", board.stackup.copper_layers,
        PolygonWithHoles(PolygonRing((Point.mm(x,y), Point.mm(xx,y),
                                     Point.mm(xx,yy), Point.mm(x,yy)))),
        block_tracks=False, block_vias=True, block_zones=False)
        for i,(x,xx) in enumerate(zip(axes,axes[1:]))
        for j,(y,yy) in enumerate(zip(axes,axes[1:])) if (i,j) != (1,1))
    if block_first_order:
        keepouts += (CopperKeepout("first diagonal blocked", (CopperLayer.FRONT,),
            PolygonWithHoles(PolygonRing((Point.mm(4.35,5.35), Point.mm(4.45,5.35),
                                         Point.mm(4.45,5.45), Point.mm(4.35,5.45)))),
            block_vias=False, block_zones=False),)
    return replace(board, copper_keepouts=keepouts)


def opts(**kw):
    return FanoutOptions(minimum_component_pads=2, maximum_neighbor_distance_nm=nm_from_mm(3), **kw)


def test_two_leg_escape_reaches_real_off_ray_site_and_tries_other_order():
    board = base_board()
    old = route_fanout(board, opts(two_leg_escapes=False))
    new = route_fanout(board, opts())
    assert old.pending_pads == (PadReference("U", "1"),)
    assert not new.pending_pads
    assert new.accesses[PadReference("U", "1")] == Point.mm(4,4)
    assert new.created_tracks == (TrackSegment("A", Point.mm(5,6), Point.mm(5,5),
        board.rules.default_track_width_nm, CopperLayer.FRONT),
        TrackSegment("A", Point.mm(5,5), Point.mm(4,4), board.rules.default_track_width_nm, CopperLayer.FRONT))
    assert new.pin_analysis[0].two_leg_candidate_count == new.pin_analysis[0].legal_candidate_count == 1
    assert new.added_track_count == 2 and new.added_via_count == 1
    assert new == route_fanout(board, opts())
    assert not any(f.code in {"DRC-COPPER-KEEPOUT", "DRC-CLEARANCE", "DRC-BOARD-EDGE"}
                   for f in run_physical_drc(new.board).findings)


def test_two_leg_site_budget_is_bounded_near_to_far_and_has_no_radial_duplicates():
    settings = opts(maximum_two_leg_candidates=17)
    candidates = tuple(_two_leg_candidates(Point.mm(5,6), Point.mm(5,7), settings))
    assert len(candidates) == len(set(candidates)) == 17
    radii = []
    for point in candidates:
        dx,dy = abs(point.x_nm-nm_from_mm(5)), abs(point.y_nm-nm_from_mm(6))
        assert dx and dy and dx != dy
        radii.append(max(dx,dy))
    assert radii == sorted(radii)
    with pytest.raises(ValueError):
        opts(maximum_two_leg_candidates=0)


def test_bounded_maze_fallback_retains_exact_clearance_and_determinism(monkeypatch):
    import pcbir.fanout as module
    board = base_board()
    # Isolate the fallback owner: empty straight/elbow domains do not grant a
    # via permission or bypass any physical obstacles.
    monkeypatch.setattr(module, "_legal_choices", lambda *args, **kwargs: ())
    settings = opts(maze_escapes=True, joint_escapes=False)
    result = route_fanout(board, settings)
    assert not result.pending_pads and result.added_via_count == 1
    assert result == route_fanout(board, settings)
    anchor = result.accesses[PadReference("U", "1")]
    assert verified_fanout_path(result.board, PadReference("U", "1"), "A", anchor,
                               RoutingClearanceIndex(result.board))
    assert not any(f.code in {"DRC-SHORT", "DRC-CLEARANCE", "DRC-VIA-PAD-OVERLAP",
                              "DRC-COPPER-KEEPOUT", "DRC-BOARD-EDGE"}
                   for f in run_physical_drc(result.board).findings)
    bounded = route_fanout(board, replace(settings, maze_state_budget=1))
    assert bounded.pending_pads and not bounded.created_vias


@pytest.mark.parametrize("defect", [None, "gap", "layer", "net"])
def test_multi_bend_anchor_is_verified_only_through_actual_connected_copper(defect):
    initial = route_fanout(base_board(False), opts())
    board = initial.board
    width = board.rules.default_track_width_nm
    points = tuple(Point.mm(x,y) for x,y in ((5,6),(5,5.5),(4.5,5),(4.5,4.5),(4,4)))
    tracks = tuple(TrackSegment("A", a,b,width,CopperLayer.FRONT) for a,b in zip(points,points[1:]))
    if defect == "gap":
        tracks = (tracks[0], *tracks[2:])
    elif defect == "layer":
        tracks = tuple(replace(t,layer=CopperLayer.BACK) for t in tracks)
    elif defect == "net":
        board = replace(board,nets=(*board.nets,PhysicalNet("other",())))
        tracks = tuple(replace(t,net="other") for t in tracks)
    board = replace(board,tracks=tracks)
    path = verified_fanout_path(board,PadReference("U","1"),"A",Point.mm(4,4),RoutingClearanceIndex(board))
    assert (path == tracks) if defect is None else path is None


@pytest.mark.parametrize("mutation", ["missing", "gap", "layer", "net", "via-net", "span", "oblique"])
def test_claimed_anchor_cannot_skip_missing_wrong_layer_or_net_legs(mutation):
    fanout = route_fanout(base_board(), opts())
    board = fanout.board
    if mutation in {"net", "via-net"}:
        board = replace(board, nets=(*board.nets, PhysicalNet("other", ())))
    if mutation == "missing":
        board = replace(board, tracks=board.tracks[:1])
    elif mutation == "gap":
        last = board.tracks[-1]
        board = replace(board, tracks=(*board.tracks[:-1], replace(last, start=Point(last.start.x_nm+1,last.start.y_nm))))
    elif mutation == "layer":
        board = replace(board, tracks=tuple(replace(t, layer=CopperLayer.BACK) for t in board.tracks))
    elif mutation == "net":
        board = replace(board, tracks=tuple(replace(t, net="other") for t in board.tracks))
    elif mutation == "via-net":
        board = replace(board, vias=(replace(board.vias[0], net="other"),))
    elif mutation == "span":
        board = replace(board, stackup=Stackup((CopperLayer.FRONT,CopperLayer.INTERNAL_1,
                                              CopperLayer.INTERNAL_2,CopperLayer.BACK)),
                        vias=(replace(board.vias[0], from_layer=CopperLayer.INTERNAL_1),))
    elif mutation == "oblique":
        start, anchor = Point.mm(5,6), Point.mm(4,4)
        board = replace(board, tracks=(TrackSegment("A",start,anchor,board.rules.default_track_width_nm,CopperLayer.FRONT),))
    assert verified_fanout_path(board, PadReference("U","1"), "A", Point.mm(4,4), RoutingClearanceIndex(board)) is None


def test_reversed_input_segments_verified_without_rewriting_identity():
    fanout = route_fanout(base_board(), opts())
    board = replace(fanout.board, tracks=tuple(replace(t,start=t.end,end=t.start) for t in fanout.board.tracks))
    path = verified_fanout_path(board, PadReference("U","1"), "A", Point.mm(4,4), RoutingClearanceIndex(board))
    assert path == board.tracks


def test_detailed_route_consumes_verified_two_leg_anchor_and_prunes_only_owned_via():
    base = base_board()
    guide = route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    fanout = route_fanout(base, opts())
    result = route_detailed(fanout.board, guide, DetailedRouterOptions(maximum_passes=1),
        fanout_accesses=fanout.accesses,
        fanout_created_tracks=fanout.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in fanout.created_vias))
    assert result.nets[0].connected, result.nets[0].diagnostics
    assert all(t in result.board.tracks for t in fanout.created_tracks)
    assert not any(f.code == "DRC-OPEN-NET" for f in run_physical_drc(result.board).findings)


def test_failed_full_cleanup_removes_owned_two_legs_not_preexisting_duplicates():
    fanout = route_fanout(base_board(), opts())
    duplicate = fanout.created_tracks[0]
    tracks = (duplicate, *fanout.board.tracks)
    retained, vias = _prune_fanout_copper(fanout.board, tracks, fanout.board.vias,
        fanout.accesses, frozenset(), frozenset((v.net,v.position) for v in fanout.created_vias), fanout.created_tracks)
    assert retained == (duplicate,) and not vias
    unknown_tracks, unknown_vias = _prune_fanout_copper(fanout.board, tracks,
        fanout.board.vias, fanout.accesses, frozenset(), None)
    assert unknown_tracks == tracks and unknown_vias == fanout.board.vias


def test_failed_subset_preserves_even_explicitly_owned_input_paths():
    base = base_board()
    fanout = route_fanout(base, opts())
    guide = replace(route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))),routes=())
    result = route_detailed(fanout.board, guide, fanout_accesses=fanout.accesses,
        fanout_created_tracks=fanout.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in fanout.created_vias),
        only_nets=frozenset({"A"}))
    assert not result.nets[0].connected
    assert result.board.tracks == fanout.board.tracks and result.board.vias == fanout.board.vias


def test_failed_full_route_prunes_only_explicitly_owned_two_leg_input():
    base = base_board()
    fanout = route_fanout(base, opts())
    guide = replace(route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2))),routes=())
    result = route_detailed(fanout.board, guide, fanout_accesses=fanout.accesses,
        fanout_created_tracks=fanout.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in fanout.created_vias))
    assert not result.nets[0].connected
    assert not result.board.tracks and not result.board.vias
    unknown = route_detailed(fanout.board, guide, fanout_accesses=fanout.accesses)
    assert unknown.board.tracks == fanout.board.tracks and unknown.board.vias == fanout.board.vias


def test_existing_two_leg_escape_reused_without_new_ownership_or_duplicates():
    initial = route_fanout(base_board(), opts())
    repeated = route_fanout(initial.board, opts())
    assert repeated.accesses == initial.accesses
    assert repeated.board == initial.board
    assert not repeated.created_tracks and not repeated.created_vias
    assert repeated.added_track_count == repeated.added_via_count == 0


def test_installed_kicad_accepts_routed_two_leg_escape():
    from pcbir.plane_verify import verify_filled_planes
    executable = shutil.which("kicad-cli")
    if executable is None:
        installed = Path("C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
        if not installed.is_file():
            pytest.skip("KiCad CLI not installed")
        executable = str(installed)
    base = base_board()
    guide = route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))
    fanout = route_fanout(base, opts())
    detailed = route_detailed(fanout.board, guide, DetailedRouterOptions(maximum_passes=1),
        fanout_accesses=fanout.accesses, fanout_created_tracks=fanout.created_tracks,
        fanout_created_vias=frozenset((v.net,v.position) for v in fanout.created_vias))
    assert detailed.nets[0].connected
    evidence = verify_filled_planes(detailed.board, kicad_cli=Path(executable))
    assert evidence.passed, evidence.findings

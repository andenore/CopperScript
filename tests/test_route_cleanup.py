from dataclasses import replace
from collections import Counter

import pytest

from pcbir import (
    BoardOutline, CopperLayer, DetailedRouterOptions, FootprintPad,
    GlobalRouterOptions, PadReference, PhysicalBoard, PhysicalFootprint,
    PhysicalNet, Placement, Point, Size, TrackSegment, Via, nm_from_mm,
    route_detailed, route_global, run_physical_drc,
)
from pcbir.detailed import DetailedNode, _erase_path_loops, _prune_fanout_copper
from pcbir.route_cleanup import EscapeChain, prune_track_stubs, release_unused_escapes


def board():
    footprint = PhysicalFootprint("one-pad", (
        FootprintPad("1", Point(0, 0), Size.mm("0.6", "0.6")),
    ), Size.mm(1, 1))
    return PhysicalBoard("cleanup", BoardOutline.rectangle(20, 12),
        {footprint.name: footprint}, (
            Placement("J1", footprint.name, Point.mm(3, 6)),
            Placement("J2", footprint.name, Point.mm(17, 6)),
        ), (PhysicalNet("S", (PadReference("J1", "1"), PadReference("J2", "1"))),))


def track(a, b, layer=CopperLayer.FRONT, width="0.2"):
    return TrackSegment("S", Point.mm(*a), Point.mm(*b), nm_from_mm(width), layer)


def walk(*nodes):
    return tuple((a, b, 1) for a, b in zip(nodes, nodes[1:]))


def test_erase_revisited_node_and_backtrack_excursions():
    a, b, c, d = (DetailedNode(0, x, 0) for x in range(4))
    path = walk(a, b, c, b, c, d, c, d)
    assert _erase_path_loops(path) == walk(a, b, c, d)
    assert _erase_path_loops(walk(a, b, c, a)) == ()
    assert _erase_path_loops(()) == ()


def test_same_coordinates_on_different_layers_are_not_a_loop():
    path = walk(DetailedNode(0, 0, 0), DetailedNode(1, 0, 0), DetailedNode(1, 1, 0))
    assert _erase_path_loops(path) == path


def test_discontinuous_walk_fails_closed():
    a, b, c, d = (DetailedNode(0, x, 0) for x in range(4))
    with pytest.raises(ValueError, match="continuous"):
        _erase_path_loops(((a, b, 0), (c, d, 0)))


def test_retraced_access_loses_only_the_dead_end_tail():
    copper = (track((3, 6), (5, 6)), track((5, 6), (4, 6)),
              track((4, 6), (4, 7)), track((4, 7), (17, 7)), track((17, 7), (17, 6)))
    result = prune_track_stubs(board(), copper)
    assert not any(Point.mm(5, 6) in (t.start, t.end) for t in result)
    assert track((3, 6), (4, 6)) in result
    assert result == prune_track_stubs(board(), result)
    assert "DRC-OPEN-NET" not in {f.code for f in run_physical_drc(replace(board(), tracks=result)).findings}


def test_recursive_spur_removal_keeps_a_real_interior_t_junction():
    base = board()
    third = Placement("J3", "one-pad", Point.mm(10, 10))
    base = replace(base, placements=(*base.placements, third),
        nets=(replace(base.nets[0], pads=(*base.nets[0].pads, PadReference("J3", "1"))),))
    copper = (track((3, 6), (17, 6)), track((10, 6), (10, 10)),
              track((8, 6), (8, 4)), track((8, 4), (9, 3)))
    result = prune_track_stubs(base, copper)
    assert not any(Point.mm(8, 4) in (t.start, t.end) or Point.mm(9, 3) in (t.start, t.end)
                   for t in result)
    assert track((10, 6), (10, 10)) in result
    assert "DRC-OPEN-NET" not in {f.code for f in run_physical_drc(replace(base, tracks=result)).findings}


def test_reversed_duplicates_collapse_without_losing_a_terminal():
    copper = (track((3, 6), (17, 6)), track((17, 6), (3, 6)))
    assert prune_track_stubs(board(), copper) == copper[:1]


def test_unanchored_straight_and_diagonal_chain_is_removed():
    copper = (track((8, 3), (9, 3)), track((9, 3), (10, 4)))
    assert prune_track_stubs(board(), copper) == ()


def test_different_width_retrace_is_still_a_single_dead_end():
    copper = (track((3, 6), (5, 6), width="0.4"), track((5, 6), (4, 6)),
              track((4, 6), (4, 7)), track((4, 7), (17, 7)), track((17, 7), (17, 6)))
    result = prune_track_stubs(board(), copper)
    assert not any(Point.mm(5, 6) in (t.start, t.end) for t in result)
    assert track((3, 6), (4, 6), width="0.4") in result
    assert "DRC-OPEN-NET" not in {f.code for f in run_physical_drc(replace(board(), tracks=result)).findings}


def test_midsegment_pad_edge_and_non_grid_contacts_are_preserved():
    base = replace(board(), placements=(
        Placement("J1", "one-pad", Point.mm(3, 6)),
        Placement("J2", "one-pad", Point.mm("10.2", "6.2")),
    ))
    copper = (track((3, 6), (12, 6)),)
    assert prune_track_stubs(base, copper) == copper
    # Only the width touches this branch: no exact endpoint/centre-line join.
    copper = (track((3, 6), (17, 6)), track((10, "6.15"), (10, 9)))
    assert prune_track_stubs(board(), copper) == copper


@pytest.mark.parametrize("layer", [CopperLayer.FRONT, CopperLayer.BACK])
def test_via_and_immutable_input_contacts_are_not_deleted(layer):
    via = Via("S", Point.mm(10, 7), nm_from_mm("0.8"), nm_from_mm("0.4"))
    copper = (track((10, 7), (12, 7), layer),)
    assert prune_track_stubs(board(), copper, (via,)) == copper
    locked = track((9, 7), (10, 7), layer)
    assert prune_track_stubs(replace(board(), tracks=(locked,)), copper) == copper


def test_zone_net_contacts_are_left_to_the_plane_owner():
    from pcbir import CopperZone, PolygonRing
    base = board()
    base = replace(base, zones=(CopperZone("power", "S", (CopperLayer.FRONT,),
        PolygonRing((Point.mm(1, 1), Point.mm(19, 1), Point.mm(19, 11), Point.mm(1, 11)))),))
    copper = (track((8, 8), (9, 8)),)
    assert prune_track_stubs(base, copper) == copper


def test_via_does_not_anchor_a_stub_on_a_layer_outside_its_span():
    from pcbir import Stackup
    base = replace(board(), stackup=Stackup((CopperLayer.FRONT,
        CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)))
    via = Via("S", Point.mm(10, 7), nm_from_mm("0.8"), nm_from_mm("0.4"),
              CopperLayer.FRONT, CopperLayer.INTERNAL_1)
    copper = (track((10, 7), (12, 7), CopperLayer.BACK),)
    assert prune_track_stubs(base, copper, (via,)) == ()


def test_successful_owned_fanout_tail_is_trimmed_but_locked_duplicate_survives():
    base = board()
    lead = track((3, 6), (5, 6))
    via = Via("S", Point.mm(5, 6), nm_from_mm("0.8"), nm_from_mm("0.4"))
    base = replace(base, tracks=(lead,), vias=(via,))
    added = (track((5, 6), (4, 6)), track((4, 6), (4, 7)),
             track((4, 7), (17, 7)), track((17, 7), (17, 6)))
    args = ({PadReference("J1", "1"): Point.mm(5, 6)}, frozenset({"S"}),
            frozenset({("S", via.position)}), (lead,))
    result, vias = _prune_fanout_copper(base, (*base.tracks, *added), base.vias, *args)
    assert not vias
    assert not any(Point.mm(5, 6) in (t.start, t.end) for t in result)
    locked = replace(base, tracks=(lead, lead))
    result, _ = _prune_fanout_copper(locked, (*locked.tracks, *added), locked.vias, *args)
    assert lead in result  # One occurrence was input copper, not owned fanout.


@pytest.mark.parametrize("owned_stub", [False, True])
def test_redundant_duplicate_pad_edge_branch_is_removed_only_when_owned(owned_stub):
    base = board()
    footprint = base.footprints["one-pad"]
    duplicate = replace(footprint, pads=(*footprint.pads, replace(footprint.pads[0], number="2")))
    base = replace(base, footprints={footprint.name: duplicate}, nets=(replace(
        base.nets[0], pads=(*base.nets[0].pads, PadReference("J1", "2"), PadReference("J2", "2"))),))
    lead = track((3, 6), (5, 6))
    route = track((5, 6), (17, 6))
    # The branch touches the edge of both coincident lands and the real exit.
    # Its far endpoint has no connection; conservative local pruning keeps it.
    stub = track(("3.4", 6), ("3.8", "6.4"), width="0.25")
    copper = (lead, route, stub)
    assert stub in prune_track_stubs(base, copper, trim_overhangs=True)
    mutable = Counter(copper if owned_stub else (lead, route))
    result, vias = release_unused_escapes(
        base, copper, (), (EscapeChain("S", (lead,), (), None),), Counter((lead,)),
        mutable_tracks=mutable, removable_vias=Counter(),
    )
    assert (stub not in result) is owned_stub
    assert not vias
    # The required land-to-launch path survives; removing it would open J1.
    from pcbir.drc import explicit_copper_connectivity
    assert explicit_copper_connectivity(replace(base, tracks=result)).net_connected(base.nets[0])
    assert any(Point.mm(5, 6) in (t.start, t.end) for t in result)


def test_pad_edge_branch_is_kept_when_it_connects_an_interior_land():
    base = board()
    small = PhysicalFootprint("small", (
        FootprintPad("1", Point(0, 0), Size.mm("0.1", "0.1")),), Size.mm("0.2", "0.2"))
    base = replace(base, footprints={**base.footprints, small.name: small},
        placements=(*base.placements, Placement("J3", small.name, Point.mm("3.6", "6.25"))),
        nets=(replace(base.nets[0], pads=(*base.nets[0].pads, PadReference("J3", "1"))),))
    lead = track((3, 6), (5, 6))
    branch = track(("3.4", 6), ("3.8", "6.4"), width="0.25")
    copper = (lead, track((5, 6), (17, 6)), branch)
    result, _ = release_unused_escapes(
        base, copper, (), (EscapeChain("S", (lead,), (), None),), Counter((lead,)),
        mutable_tracks=Counter(copper), removable_vias=Counter(),
    )
    # The far endpoint is open, but the branch's interior reaches J3.
    assert branch in result
    from pcbir.drc import explicit_copper_connectivity
    assert explicit_copper_connectivity(replace(base, tracks=result)).net_connected(base.nets[0])


@pytest.mark.parametrize("excursion", [False, True])
def test_router_commit_removes_retrace_and_physical_search_loop(monkeypatch, excursion):
    import pcbir.detailed as detail
    base = board()
    guide = route_global(base, GlobalRouterOptions(tile_size_nm=nm_from_mm(2)))

    def node(grid, x, y):
        return DetailedNode(0, grid.xs.index(nm_from_mm(x)), grid.ys.index(nm_from_mm(y)))

    def accesses(base, grid, pad, *args, **kwargs):
        return (node(grid, 5 if pad.component == "J1" else 17, 6),)

    def search(grid, starts, targets, *args, **kwargs):
        nodes = [node(grid, 5, 6), node(grid, 4, 6)]
        if excursion:
            nodes.extend((node(grid, 4, 5), node(grid, 5, 5), node(grid, 5, 6), node(grid, 4, 6)))
        nodes.extend((node(grid, 4, 7), node(grid, 17, 7), node(grid, 17, 6)))
        return walk(*nodes), nodes[0], nodes[-1]

    monkeypatch.setattr(detail, "_access_candidates", accesses)
    monkeypatch.setattr(detail, "_search", search)
    used = []
    edge_resources = detail._edge_resources

    def resources(grid, first, second):
        used.extend((grid.point(first), grid.point(second)))
        return edge_resources(grid, first, second)

    monkeypatch.setattr(detail, "_edge_resources", resources)
    result = route_detailed(base, guide, DetailedRouterOptions(
        pitch_nm=nm_from_mm(1), maximum_passes=1, any_angle_cleanup=False))
    assert result.nets[0].connected
    assert not any(Point.mm(5, 6) in (t.start, t.end) for t in result.board.tracks)
    assert "DRC-OPEN-NET" not in {f.code for f in run_physical_drc(result.board).findings}
    assert result.nets[0].track_count == len(result.board.tracks)
    assert result.metrics.track_count == len(result.board.tracks)
    assert Point.mm(5, 6) not in used


def test_installed_kicad_confirms_retrace_stub_is_removed():
    from pathlib import Path
    import shutil
    from pcbir.plane_verify import verify_filled_planes

    cli = Path(shutil.which("kicad-cli") or "C:/Program Files/KiCad/10.0/bin/kicad-cli.exe")
    if not cli.is_file():
        pytest.skip("KiCad not installed")
    copper = (track((3, 6), (5, 6)), track((5, 6), (4, 6)),
              track((4, 6), (4, 7)), track((4, 7), (17, 7)), track((17, 7), (17, 6)))
    before = verify_filled_planes(replace(board(), tracks=copper), kicad_cli=cli)
    assert before.unconnected_count == 0, before.findings
    # Overlapping reversed tracks can mask a dead end from KiCad's dangling
    # check. The equivalent split/unique copper makes that tail explicit.
    unique = (track((3, 6), (4, 6)), track((4, 6), (5, 6)), *copper[2:])
    dangling = verify_filled_planes(replace(board(), tracks=unique), kicad_cli=cli)
    assert any(f.startswith("track_dangling:") for f in dangling.findings), dangling.findings
    result = prune_track_stubs(board(), copper)
    after = verify_filled_planes(replace(board(), tracks=result), kicad_cli=cli)
    assert after.passed, after.findings

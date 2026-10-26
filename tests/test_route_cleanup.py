from dataclasses import replace
from collections import Counter

import pytest

from pcbir import (BoardOutline, CopperLayer, FootprintPad, PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, Via, nm_from_mm, run_physical_drc)
from pcbir.route_cleanup import prune_track_stubs, trim_dangling_overhangs


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


def test_installed_kicad_confirms_retrace_stub_is_removed():
    from pathlib import Path
    import shutil
    from pcbir.plane_verify import remove_native_dangling_tracks, verify_filled_planes

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
    cleaned, checked = remove_native_dangling_tracks(
        replace(board(), tracks=unique), dangling, Counter(), kicad_cli=cli)
    assert checked.passed and len(cleaned.tracks) < len(unique)
    guarded, _ = remove_native_dangling_tracks(
        replace(board(), tracks=unique), dangling, Counter(unique), kicad_cli=cli)
    assert guarded.tracks == unique
    result = prune_track_stubs(board(), copper)
    after = verify_filled_planes(replace(board(), tracks=result), kicad_cli=cli)
    assert after.passed, after.findings


def _overhang_board(**rule):
    from collections import Counter
    from dataclasses import replace
    from pcbir.physical import (BoardOutline, CopperLayer, FootprintPad, NetRoutingRule, PadReference,
                                PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size,
                                TrackSegment)
    package = PhysicalFootprint("package", (FootprintPad("1", Point(0, 0), Size.mm(.3, .3)),), Size.mm(4, 4))
    one = PhysicalFootprint("one", (FootprintPad("1", Point(0, 0), Size.mm(.6, .6)),), Size.mm(1, 1))
    board = PhysicalBoard("overhang", BoardOutline.rectangle(20, 16), {"package": package, "one": one},
        (Placement("U", "package", Point.mm(10, 8)), Placement("J1", "one", Point.mm(11, 8))),
        (PhysicalNet("A", (PadReference("U", "1"), PadReference("J1", "1"))),),
        net_routing_rules=(NetRoutingRule("A", **rule),) if rule else ())
    # A dogbone from U.1 through J1's land whose launch via is gone.
    stub = TrackSegment("A", Point.mm(10, 8), Point.mm(12, 8), board.rules.default_track_width_nm,
                        CopperLayer.FRONT)
    return replace(board, tracks=(stub,)), stub


def test_final_sweep_cuts_an_open_dogbone_back_to_the_land_it_crosses():
    from collections import Counter
    from dataclasses import replace
    from pcbir.physical import Point
    board, stub = _overhang_board()
    swept = trim_dangling_overhangs(board, Counter())
    assert swept.tracks == (replace(stub, end=Point.mm(11, 8)),)
    # Protected (input or macro) copper and critical nets never change.
    assert trim_dangling_overhangs(board, Counter((stub,))) is board
    from pcbir.physical import RouteKind
    critical, _ = _overhang_board(kind=RouteKind.CRITICAL)
    assert trim_dangling_overhangs(critical, Counter()) is critical

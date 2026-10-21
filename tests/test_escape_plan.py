"""Escape diagnostics (escape planning E0): channel load and two-end crossings."""
from decimal import Decimal

import pytest

from pcbir.escape_plan import EscapePlanOptions, escape_diagnostics
from pcbir.physical import (BoardOutline, BoardSide, CopperLayer, DesignRules, FootprintPad, NetRoutingRule,
                            PadKind, PadReference, PadShape, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                            Placement, Point, Size, nm_from_mm)

RULES = DesignRules(minimum_clearance_nm=nm_from_mm("0.254"), minimum_hole_clearance_nm=200000,
                    minimum_track_width_nm=nm_from_mm("0.254"), default_track_width_nm=nm_from_mm("0.254"),
                    default_via_size_nm=800000, default_via_drill_nm=400000)


def row(name: str, count: int, pitch: str = "1") -> PhysicalFootprint:
    """``count`` SMD pads in one column, numbered from the top (smaller y)."""
    pads = tuple(FootprintPad(str(index + 1), Point.mm(0, Decimal(pitch) * index), Size.mm("0.5", "0.5"))
                 for index in range(count))
    return PhysicalFootprint(name, pads, Size.mm(1, count))


def two_rows(wiring: dict[int, int], *, rotation=0, side=BoardSide.FRONT, swap_sides=False) -> PhysicalBoard:
    """A four-pin column at x=10 and another at x=30; net ``N{a}`` joins A.a to B.(wiring[a])."""
    footprints = {"row": row("row", 4)}
    first, second = (Point.mm(10, 10), Point.mm(30, 10)) if not swap_sides else (Point.mm(30, 10), Point.mm(10, 10))
    placements = (Placement("A", "row", first, Decimal(rotation), side), Placement("B", "row", second))
    nets = tuple(PhysicalNet(f"N{a}", (PadReference("A", str(a)), PadReference("B", str(b))))
                 for a, b in wiring.items())
    return PhysicalBoard("rows", BoardOutline.rectangle(40, 30), footprints, placements, nets, rules=RULES)


def pair(board, options=None):
    pairs = escape_diagnostics(board, options).pairs
    assert len(pairs) == 1
    return pairs[0]


def test_parallel_bundle_does_not_cross():
    result = pair(two_rows({1: 1, 2: 2, 3: 3, 4: 4}))
    assert (result.first, result.second) == ("A", "B")
    assert result.crossing_pairs == 0 and result.layer_moves == () and result.crossing_nets == ()


@pytest.mark.parametrize("swap_sides", (False, True))
def test_swapped_neighbours_cross_once_and_one_net_moves(swap_sides):
    result = pair(two_rows({1: 2, 2: 1, 3: 3, 4: 4}, swap_sides=swap_sides))
    assert result.crossing_pairs == 1
    assert result.crossing_nets == ("N1", "N2")
    assert len(result.layer_moves) == 1 and result.layer_moves[0] in {"N1", "N2"}


def test_reversed_bundle_crosses_every_pair_and_keeps_one_net():
    result = pair(two_rows({1: 4, 2: 3, 3: 2, 4: 1}))
    assert result.crossing_pairs == 6
    assert len(result.layer_moves) == 3


def test_order_is_read_on_the_placed_board():
    # Rotating A by 180° about its first pin reverses its column: the reversed wiring becomes parallel.
    assert pair(two_rows({1: 4, 2: 3, 3: 2, 4: 1}, rotation=180)).crossing_pairs == 0
    assert pair(two_rows({1: 1, 2: 2, 3: 3, 4: 4}, rotation=180)).crossing_pairs == 6


def test_two_row_header_orders_pins_around_its_outline():
    # A 2×4 header (columns 2.54 mm apart) facing a single row: front-column nets in parallel do
    # not cross; a back-column net leaves around the header's end, outside its front neighbour.
    pads = tuple(FootprintPad(str(2 * y + x + 1), Point.mm(Decimal("2.54") * x, Decimal("2.54") * y),
                              Size.mm("1.7272", "1.7272"), kind=PadKind.THROUGH_HOLE, shape=PadShape.RECTANGLE,
                              drill=Size.mm("1.016", "1.016")) for y in range(4) for x in range(2))
    header = PhysicalFootprint("header", pads, Size.mm(5, 10))
    board = two_rows({1: 1})
    board = PhysicalBoard("header", board.outline, {"header": header, "row": row("row", 5, "2")},
                          (Placement("A", "header", Point.mm(10, 10)), Placement("B", "row", Point.mm(30, 9))), (
                              PhysicalNet("TOP", (PadReference("A", "1"), PadReference("B", "1"))),  # back column
                              *(PhysicalNet(f"F{y}", (PadReference("A", str(2 * y + 2)), PadReference("B", str(y + 2))))
                                for y in range(4))), rules=RULES)
    result = pair(board)
    assert result.nets == ("F0", "F1", "F2", "F3", "TOP") and result.crossing_pairs == 0
    swapped = PhysicalBoard("header", board.outline, board.footprints, board.placements, (
        PhysicalNet("TOP", (PadReference("A", "1"), PadReference("B", "2"))),
        PhysicalNet("F0", (PadReference("A", "2"), PadReference("B", "1"))),
        *board.nets[2:]), rules=RULES)
    assert pair(swapped).crossing_pairs == 1


def test_back_side_part_is_mirrored_before_ordering():
    front = pair(two_rows({1: 2, 2: 1, 3: 3, 4: 4}))
    back = pair(two_rows({1: 2, 2: 1, 3: 3, 4: 4}, side=BoardSide.BACK))
    assert front.crossing_pairs == back.crossing_pairs == 1


def test_bundles_hold_only_nets_between_the_two_parts():
    board = two_rows({1: 2, 2: 1, 3: 3, 4: 4})
    # N3 also reaches a third part: it is a tree, not part of the A-B bundle.
    footprints = {**board.footprints, "single": row("single", 1)}
    nets = tuple(PhysicalNet(net.name, net.pads + ((PadReference("C", "1"),) if net.name == "N3" else ()))
                 for net in board.nets)
    board = PhysicalBoard("rows", board.outline, footprints,
                          board.placements + (Placement("C", "single", Point.mm(20, 25)),), nets, rules=RULES)
    assert pair(board).nets == ("N1", "N2", "N4")


def test_net_with_several_pins_leaves_from_the_pin_nearest_the_other_part():
    board = two_rows({2: 2, 3: 3})
    nets = (*board.nets, PhysicalNet("N14", (PadReference("A", "1"), PadReference("A", "4"), PadReference("B", "4"))))
    lowered = (board.placements[0], Placement("B", "row", Point.mm(30, 14)))
    board = PhysicalBoard("rows", board.outline, board.footprints, lowered, nets, rules=RULES)
    # A.4 is nearer B than A.1, so N14 leaves below N3 and meets B.4 below B.3: no crossing.
    assert pair(board).crossing_pairs == 0


def test_pairs_need_the_minimum_shared_nets():
    board = two_rows({1: 2, 2: 1})
    assert len(escape_diagnostics(board, EscapePlanOptions(cluster_pair_minimum_nets=2)).pairs) == 1
    assert escape_diagnostics(board, EscapePlanOptions(cluster_pair_minimum_nets=3)).pairs == ()
    with pytest.raises(ValueError):
        escape_diagnostics(board, EscapePlanOptions(cluster_pair_minimum_nets=0))


def test_crossing_causes_name_every_net_of_a_crossing_bundle():
    diagnostics = escape_diagnostics(two_rows({1: 2, 2: 1, 3: 3, 4: 4}))
    assert diagnostics.net_causes() == {f"N{n}": ["two_end_crossing"] for n in (1, 2, 3, 4)}
    assert escape_diagnostics(two_rows({1: 1, 2: 2, 3: 3, 4: 4})).net_causes() == {}
    report = diagnostics.report()
    assert report["crossing_pairs"] == 1 and report["pairs"][0]["components"] == ["A", "B"]


# --------------------------------------------------------------------------- channel load

def grid(pitch: str, land: str, *, kind=PadKind.SMD, shape=PadShape.CIRCLE) -> PhysicalFootprint:
    pads = tuple(FootprintPad(str(y * 3 + x + 1), Point.mm(Decimal(pitch) * (x - 1), Decimal(pitch) * (y - 1)),
                              Size.mm(land, land), kind=kind, shape=shape,
                              drill=Size.mm("1.016", "1.016") if kind is PadKind.THROUGH_HOLE else None)
                 for y in range(3) for x in range(3))
    return PhysicalFootprint("grid", pads, Size.mm(8, 8))


def grid_board(footprint: PhysicalFootprint, *, centre_rule: NetRoutingRule | None = None) -> PhysicalBoard:
    """A 3×3 field at (10, 10); the centre pin's net N5 and each ring pin's net reach a far part."""
    pads = [pad.number for pad in footprint.pads]
    far = PhysicalFootprint("far", tuple(FootprintPad(n, Point.mm(0, int(n)), Size.mm("0.5", "0.5")) for n in pads),
                            Size.mm(1, 10))
    nets = tuple(PhysicalNet(f"N{n}", (PadReference("U", n), PadReference("J", n))) for n in pads)
    return PhysicalBoard("grid", BoardOutline.rectangle(40, 30), {"grid": footprint, "far": far},
                         (Placement("U", "grid", Point.mm(10, 10)), Placement("J", "far", Point.mm(30, 5))), nets,
                         rules=RULES, net_routing_rules=(centre_rule,) if centre_rule else ())


def cluster(board, component="U"):
    return next(item for item in escape_diagnostics(board).clusters if item.component == component)


def test_header_gap_holds_one_track_per_layer():
    # 2.54 mm pitch, 1.7272 mm lands: 0.8128 mm gaps; ⌊(0.8128 − 0.254)/(0.254 + 0.254)⌋ = 1 track each.
    board = grid_board(grid("2.54", "1.7272", kind=PadKind.THROUGH_HOLE, shape=PadShape.RECTANGLE))
    result = cluster(board)
    assert result.inner_signal_pins == (PadReference("U", "5"),)
    assert [(c.layer, c.demand, c.capacity, c.gaps) for c in result.channels] == [
        (CopperLayer.FRONT, 1, 8, 8), (CopperLayer.BACK, 1, 8, 8)]
    assert result.overflow == 0


def test_tight_ring_overflows_and_wider_rules_shrink_capacity():
    # 1.0 mm pitch, 0.5 mm lands: 0.5 mm gaps hold ⌊(0.5 − 0.254)/0.508⌋ = 0 tracks.
    tight = cluster(grid_board(grid("1", "0.5")))
    assert [(c.layer, c.demand, c.capacity) for c in tight.channels] == [(CopperLayer.FRONT, 1, 0)]
    assert tight.overflow == 1
    diagnostics = escape_diagnostics(grid_board(grid("1", "0.5")))
    assert "channel_overflow" in diagnostics.net_causes()["N5"]
    # 1.6 mm pitch: 1.1 mm gaps hold 1 track each at default rules, none for a 0.6 mm net.
    roomy = cluster(grid_board(grid("1.6", "0.5")))
    assert roomy.channels[0].capacity == 8
    wide = cluster(grid_board(grid("1.6", "0.5"), centre_rule=NetRoutingRule("N5", width_nm=nm_from_mm("0.6"))))
    assert wide.channels[0].capacity == 0 and wide.overflow == 1


def test_through_hole_inner_pins_may_use_any_layer():
    # 2.0 mm pitch, 1.6 mm lands: 0.4 mm gaps hold no 0.254 mm track, on either layer.
    blocked = cluster(grid_board(grid("2", "1.6", kind=PadKind.THROUGH_HOLE)))
    assert [(c.demand, c.capacity) for c in blocked.channels] == [(1, 0), (1, 0)]
    assert blocked.overflow == 1  # one pin short, not one per layer


def test_inner_land_that_holds_a_via_is_not_ring_demand():
    # A 1.0 mm centre land holds the 0.8 mm default via: an exposed pad leaves through its own via.
    pads = grid("1", "0.5").pads
    exposed = tuple(pad if pad.number != "5" else FootprintPad("5", pad.position, Size.mm(1, 1)) for pad in pads)
    result = cluster(grid_board(PhysicalFootprint("grid", exposed, Size.mm(8, 8))))
    assert result.inner_signal_pins == () and result.overflow == 0


def test_zone_nets_and_unconnected_pins_are_not_channel_demand():
    board = grid_board(grid("1", "0.5"))
    lone = PhysicalBoard(board.name, board.outline, board.footprints, board.placements,
                         tuple(net for net in board.nets if net.name != "N5"), rules=RULES)
    assert cluster(lone).inner_signal_pins == () and cluster(lone).channels == ()


def test_diagnostics_are_deterministic_and_leave_the_board_unchanged():
    board = two_rows({1: 3, 2: 1, 3: 4, 4: 2})
    assert escape_diagnostics(board) == escape_diagnostics(board)
    assert escape_diagnostics(board).report() == escape_diagnostics(board).report()


# --------------------------------------------------------------------------- channel model (E1)

from pcbir.escape_plan import Gap, Tile, channel_rules, pad_fields  # noqa: E402

FINE = DesignRules(minimum_clearance_nm=100000, minimum_hole_clearance_nm=100000, minimum_track_width_nm=100000,
                   default_track_width_nm=100000, default_via_size_nm=450000, default_via_drill_nm=200000)


def placed(footprint: PhysicalFootprint, *, rotation=0, rules=RULES) -> PhysicalBoard:
    nets = tuple(PhysicalNet(f"N{pad.number}", (PadReference("U", pad.number),)) for pad in footprint.pads)
    return PhysicalBoard("field", BoardOutline.rectangle(40, 40), {footprint.name: footprint},
                         (Placement("U", footprint.name, Point.mm(20, 20), Decimal(rotation)),), nets, rules=rules)


def lattice(columns: int, rows: int, pitch: str, land: str, *, shape=PadShape.CIRCLE, kind=PadKind.SMD,
            missing=()) -> PhysicalFootprint:
    pads = tuple(FootprintPad(str(y * columns + x + 1), Point.mm(Decimal(pitch) * x, Decimal(pitch) * y),
                              Size.mm(land, land), kind=kind, shape=shape,
                              drill=Size.mm("1.016", "1.016") if kind is PadKind.THROUGH_HOLE else None)
                 for y in range(rows) for x in range(columns) if y * columns + x + 1 not in missing)
    return PhysicalFootprint("lattice", pads, Size.mm(10, 10))


def minimum(field, kind, rules=(nm_from_mm("0.254"), nm_from_mm("0.254"))):
    return field.report(*rules)["minimum_capacity"][kind]


@pytest.mark.parametrize("rotation", (0, 45, 90))
def test_header_lattice_capacities_match_hand_calculation(rotation):
    # 2×4 header, 2.54 mm pitch, 1.7272 mm square lands, 0.254 mm track and clearance:
    # orthogonal gaps 0.8128 mm hold ⌊(0.8128 − 0.254)/0.508⌋ = 1 track; the diagonal gap runs
    # corner to corner, √2 × 0.8128 = 1.1495 mm, and also holds 1.
    header = lattice(2, 4, "2.54", "1.7272", shape=PadShape.RECTANGLE, kind=PadKind.THROUGH_HOLE)
    (field,) = pad_fields(placed(header, rotation=rotation), "U")
    assert (field.kind, field.columns, field.rows, field.pitch_nm) == ("lattice", 2, 4, (2540000, 2540000))
    assert {kind: sum(gap.kind == kind for gap in field.gaps) for kind in ("row", "column", "diagonal")} == {
        "row": 4, "column": 6, "diagonal": 6}
    assert (minimum(field, "row"), minimum(field, "column"), minimum(field, "diagonal")) == (1, 1, 1)
    assert len(field.tiles) == 3
    assert all(set(gap.layers) == {CopperLayer.FRONT, CopperLayer.BACK} for gap in field.gaps)


def test_round_lands_leave_more_room_on_the_diagonal():
    # Round 1.7272 mm lands: diagonal gap √2 × 2.54 − 1.7272 = 1.8649 mm holds 3 tracks.
    (field,) = pad_fields(placed(lattice(2, 4, "2.54", "1.7272", kind=PadKind.THROUGH_HOLE)), "U")
    assert (minimum(field, "row"), minimum(field, "diagonal")) == (1, 3)


def test_bga_capacities_and_via_sites():
    # 0.8 mm pitch, 0.4 mm balls, 0.1 mm track and clearance: orthogonal gap 0.4 mm holds 1 track,
    # diagonal gap 0.7314 mm holds 3. A 0.45 mm via at a tile centre is 0.3657 mm from each ball
    # edge, more than its 0.225 mm radius and 0.1 mm clearance; a 0.6 mm via is not.
    bga = lattice(4, 4, "0.8", "0.4")
    (field,) = pad_fields(placed(bga, rules=FINE), "U")
    fine = (100000, 100000)
    assert (minimum(field, "row", fine), minimum(field, "diagonal", fine)) == (1, 3)
    assert len(field.tiles) == 9 and all(tile.via_site for tile in field.tiles)
    wide_via = DesignRules(minimum_clearance_nm=100000, minimum_hole_clearance_nm=100000,
                           minimum_track_width_nm=100000, default_track_width_nm=100000,
                           default_via_size_nm=600000, default_via_drill_nm=300000)
    (field,) = pad_fields(placed(bga, rules=wide_via), "U")
    assert not any(tile.via_site for tile in field.tiles)


def test_missing_balls_leave_corners_and_their_gaps_open():
    (field,) = pad_fields(placed(lattice(4, 4, "0.8", "0.4", missing=(6,)), rules=FINE), "U")
    assert field.kind == "lattice" and len(field.pads) == 15
    first = next(t for t in field.tiles if (t.column, t.row) == (0, 0))
    assert first.corners[2] is None  # SE corner: lattice position (1, 1), pad 6
    assert first.sides[2] is None and first.sides[3] is None  # E and S sides end at the missing ball
    assert first.diagonals[0] is None and first.diagonals[1] is not None
    # Fewer than half the positions filled is not a lattice: a 4×4 pitch holding its ring and no more
    # than a corner of the middle reads as the outline it is.
    (sparse,) = pad_fields(placed(lattice(4, 4, "0.8", "0.4", missing=(6, 7, 8, 10, 11, 12, 14, 15, 16)),
                                  rules=FINE), "U")
    assert sparse.kind == "perimeter" and sparse.tiles == ()


def test_perimeter_row_and_split_fields():
    sides = ([Point.mm(Decimal("0.5") * k - 1, -2) for k in range(5)]
             + [Point.mm(Decimal("0.5") * k - 1, 2) for k in range(5)]
             + [Point.mm(-2, Decimal("0.5") * k - 1) for k in range(5)]
             + [Point.mm(2, Decimal("0.5") * k - 1) for k in range(5)])
    qfn = PhysicalFootprint("qfn", tuple(FootprintPad(str(i + 1), position, Size.mm("0.25", "0.25"))
                                         for i, position in enumerate(sides)), Size.mm(5, 5))
    (field,) = pad_fields(placed(qfn), "U")
    assert field.kind == "perimeter" and {gap.kind for gap in field.gaps} == {"ring"} and len(field.gaps) == 20
    (single,) = pad_fields(placed(row("row", 4)), "U")
    assert single.kind == "row" and len(single.gaps) == 3
    # Two 1 mm-pitch columns 12 mm apart (a module's opposite edges) are separate fields; 2 mm apart, one.
    far = PhysicalFootprint("edges", tuple(FootprintPad(str(i + 1), Point.mm(12 * (i // 3), i % 3),
                                                        Size.mm("0.5", "0.5")) for i in range(6)), Size.mm(14, 4))
    assert [len(f.pads) for f in pad_fields(placed(far), "U")] == [3, 3]
    near = PhysicalFootprint("edges", tuple(FootprintPad(str(i + 1), Point.mm(2 * (i // 3), i % 3),
                                                         Size.mm("0.5", "0.5")) for i in range(6)), Size.mm(4, 4))
    assert [f.kind for f in pad_fields(placed(near), "U")] == ["lattice"]
    with pytest.raises(ValueError):
        pad_fields(placed(near), "missing")


def test_escape_rules_neck_down_inside_the_breakout_region():
    board = placed(row("row", 2))
    assert channel_rules(board, "N1") == (nm_from_mm("0.254"), nm_from_mm("0.254"))
    wide = NetRoutingRule("N1", width_nm=nm_from_mm("0.5"), clearance_nm=nm_from_mm("0.3"))
    assert channel_rules(board, "N1", wide) == (nm_from_mm("0.5"), nm_from_mm("0.3"))
    necked = NetRoutingRule("N1", width_nm=nm_from_mm("0.5"), clearance_nm=nm_from_mm("0.3"),
                            breakout_length_nm=nm_from_mm(1), breakout_width_nm=nm_from_mm("0.15"),
                            breakout_clearance_nm=nm_from_mm("0.1"))
    # The breakout clearance is still floored at the board minimum.
    assert channel_rules(board, "N1", necked) == (nm_from_mm("0.15"), nm_from_mm("0.254"))


def tile(sides=(1, 1, 1, 1), diagonals=(1, 1), corners=True) -> Tile:
    def gap(capacity, kind):
        # 0.1 mm track and clearance: capacity n needs a (0.2n + 0.1) mm gap.
        return Gap(PadReference("U", "1"), PadReference("U", "2"), kind, 200000 * capacity + 100000,
                   (CopperLayer.FRONT,))
    pad = PadReference("U", "1")
    return Tile(0, 0, (pad,) * 4 if corners else (None,) * 4, tuple(gap(c, "row") for c in sides),
                tuple(gap(c, "diagonal") for c in diagonals), Point(0, 0), False)


def admits(t, flows):
    return t.admits(flows, CopperLayer.FRONT, 100000, 100000)


def test_tile_diagonal_gaps_bound_tracks_between_their_halves():
    # Each side holds one track. Turning round opposite corners (W→N round NW, S→E round SE)
    # both tracks pass between NW and SE: two tracks for a one-track diagonal.
    assert not admits(tile(), {("W", "N"): 1, ("S", "E"): 1})
    assert admits(tile(diagonals=(2, 1)), {("W", "N"): 1, ("S", "E"): 1})
    # Two tracks from a two-track west side turning north and south use different diagonals:
    # a tile capped at its one-track diagonal capacity would refuse them.
    assert admits(tile(sides=(2, 1, 1, 1)), {("W", "N"): 1, ("W", "S"): 1})
    # A straight track crosses both diagonals.
    assert admits(tile(), {("W", "E"): 1})
    assert not admits(tile(diagonals=(1, 0)), {("W", "E"): 1})
    # Side gaps bound every track through them; a pin starts one track, a missing pin none.
    assert not admits(tile(), {("W", "N"): 1, ("W", "E"): 1})
    assert admits(tile(), {("NW", "E"): 1}) and not admits(tile(), {("NW", "E"): 1, ("NW", "S"): 1})
    assert not admits(tile(corners=False), {("NW", "E"): 1})
    # The NW pin lies on the NW–SE diagonal: its track to S crosses only NE–SW.
    assert admits(tile(diagonals=(0, 1)), {("NW", "S"): 1})
    # Gaps on another layer do not bound this one.
    assert tile(sides=(0, 0, 0, 0), diagonals=(0, 0)).admits({("W", "E"): 1}, CopperLayer.BACK, 100000, 100000)
    with pytest.raises(ValueError):
        admits(tile(), {("W", "W"): 1})


def test_cluster_reports_carry_field_summaries():
    # 1 mm pitch, 0.5 mm balls: orthogonal 0.5 mm gaps hold no 0.254 mm track, diagonal 0.914 mm gaps one.
    report = escape_diagnostics(grid_board(grid("1", "0.5"))).report()
    (cluster,) = report["clusters"]
    assert cluster["fields"] == [{"pads": 9, "kind": "lattice", "columns": 3, "rows": 3,
                                  "pitch_nm": [1000000, 1000000], "gaps": {"column": 6, "diagonal": 8, "row": 6},
                                  "minimum_capacity": {"column": 0, "diagonal": 1, "row": 0},
                                  "tiles": 4, "via_sites": 0}]

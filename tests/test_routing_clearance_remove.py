"""``RoutingClearanceIndex.remove`` leaves exactly a fresh index's answers."""
from __future__ import annotations

from random import Random

import pytest

from pcbir import (BoardOutline, BoardSide, CopperLayer, FootprintPad, PadKind,
                   PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                   Placement, Point, Size, Stackup, TrackSegment, Via, nm_from_mm)
from pcbir.physical import DesignRules, NetRoutingRule
from pcbir.geometry import RoundedConvexShape
from pcbir.routing_clearance import RoutingClearanceIndex

F, I1, I2, B = (CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2,
                CopperLayer.BACK)
LAYERS = (F, I1, I2, B)
NETS = ("A", "B", "C", "D")


def board() -> PhysicalBoard:
    """Four layers; SMD, plated and non-plated pads; a clearance and a breakout rule."""
    footprint = PhysicalFootprint("mixed", (
        FootprintPad("1", Point.mm(-1, 0), Size.mm(".6", ".8")),
        FootprintPad("2", Point.mm(1, 0), Size.mm(".6", ".8")),
        FootprintPad("3", Point.mm(0, "1.4"), Size.mm("1.2", "1.2"),
                     kind=PadKind.THROUGH_HOLE, drill=Size.mm(".6", ".6")),
        FootprintPad("", Point.mm(0, "-1.5"), Size.mm(".8", ".8"),
                     kind=PadKind.NON_PLATED_THROUGH_HOLE, drill=Size.mm(".8", ".8")),
    ), Size.mm(3, 4))
    poses = tuple(Placement(f"U{i}", footprint.name, Point.mm(6 + 9 * (i % 3), 7 + 10 * (i // 3)),
                            rotation_degrees=90 if i % 2 else 0,
                            side=BoardSide.BACK if i == 4 else BoardSide.FRONT)
                  for i in range(6))
    pads = {net: [] for net in NETS}
    for i, pose in enumerate(poses):
        pads[NETS[i % 4]].append(PadReference(pose.reference, "1"))
        pads[NETS[(i + 1) % 4]].append(PadReference(pose.reference, "2"))
        pads[NETS[(i + 2) % 4]].append(PadReference(pose.reference, "3"))
    return PhysicalBoard(
        "remove", BoardOutline.rectangle(32, 34), {footprint.name: footprint}, poses,
        tuple(PhysicalNet(net, tuple(refs)) for net, refs in pads.items()),
        stackup=Stackup(LAYERS),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm("0.15"),
                          minimum_track_width_nm=nm_from_mm("0.1"),
                          default_track_width_nm=nm_from_mm("0.2"),
                          default_via_size_nm=nm_from_mm("0.6"),
                          default_via_drill_nm=nm_from_mm("0.3"),
                          minimum_hole_clearance_nm=nm_from_mm("0.25")),
        net_routing_rules=(
            NetRoutingRule("C", clearance_nm=nm_from_mm("0.3")),
            NetRoutingRule("D", width_nm=nm_from_mm("0.3"), clearance_nm=nm_from_mm("0.25"),
                           breakout_length_nm=nm_from_mm("1"),
                           breakout_width_nm=nm_from_mm("0.15"),
                           breakout_clearance_nm=nm_from_mm("0.15")),
        ),
        # Board-source copper is locked and survives every removal.
        tracks=(TrackSegment("A", Point.mm(2, 30), Point.mm(30, 30), nm_from_mm("0.2"), F),),
        vias=(Via("B", Point.mm(16, 31), nm_from_mm("0.6"), nm_from_mm("0.3")),),
    )


def copper(seed: int, count: int = 40) -> list[tuple[TrackSegment | Via, bool]]:
    """Deterministic mixed copper; every fifth item is locked."""
    random = Random(seed)
    result = []
    for ordinal in range(count):
        net = NETS[random.randrange(4)]
        locked = ordinal % 5 == 4
        x, y = random.randrange(2_000, 30_000), random.randrange(2_000, 32_000)
        if random.random() < 0.3:
            span = random.choice(((F, B), (F, I1), (I1, B), (I2, B)))
            result.append((Via(net, Point(x * 1000, y * 1000), nm_from_mm("0.6"),
                               nm_from_mm("0.3"), *span), locked))
        else:
            dx, dy = random.choice(((1, 0), (0, 1), (1, 1), (1, -1)))
            length = random.randrange(1_000, 6_000)
            end = Point((x + dx * length) * 1000, (y + dy * length) * 1000)
            result.append((TrackSegment(net, Point(x * 1000, y * 1000), end,
                                        random.choice((nm_from_mm("0.15"), nm_from_mm("0.3"))),
                                        random.choice(LAYERS)), locked))
    return result


def add(index: RoutingClearanceIndex, items) -> None:
    for item, locked in items:
        if isinstance(item, Via):
            index.add_via(item, locked=locked)
        else:
            index.add_track(item, locked=locked)


def fresh_with(items) -> RoutingClearanceIndex:
    index = RoutingClearanceIndex(board())
    add(index, items)
    return index


PROBE_POINTS = tuple(Point(x * 1000, y * 1000) for x in range(1_500, 31_000, 2_100)
                     for y in range(1_700, 33_000, 2_300))


def answers(index: RoutingClearanceIndex) -> list:
    """Every public decision for a grid of tracks and vias on every layer."""
    result = []
    width = nm_from_mm("0.2")
    for ordinal, start in enumerate(PROBE_POINTS):
        net = ("A", "C", "D", "X")[ordinal % 4]
        for layer in LAYERS:
            for dx, dy in ((1_900_000, 0), (0, 2_100_000), (1_500_000, 1_500_000)):
                end = Point(start.x_nm + dx, start.y_nm + dy)
                track = TrackSegment(net, start, end, width, layer)
                result.append((index.can_track(net, start, end, width, layer),
                               index.blocking_track_nets(track),
                               index.track_blocker_records(track),
                               index.route_pieces(net, start, end, width, layer)))
        for span in ((F, B), (F, I1), (I2, B)):
            via = Via(net, start, nm_from_mm("0.6"), nm_from_mm("0.3"), *span)
            result.append((index.can_via(net, start, via.size_nm, *span, drill_nm=via.drill_nm),
                           index.blocking_via_nets(via), index.via_blocker_records(via),
                           index.via_obstacle_bounds(net, start, via.size_nm, via.drill_nm,
                                                     *span, nm_from_mm(3), 12)))
        shape = RoundedConvexShape((start,), nm_from_mm("0.5"))
        result.append(tuple(index.nearby_copper(shape, LAYERS, nm_from_mm(1))))
    vias = [Via(net, point, nm_from_mm("0.6"), nm_from_mm("0.3"))
            for net, point in zip("ABCD" * 8, PROBE_POINTS[::7])]
    result.append(index.candidate_via_conflict(vias))
    result.append(index.candidate_via_conflict(vias, allow_movable_conflicts=True))
    return result


def assert_equivalent(index: RoutingClearanceIndex, expected_items) -> None:
    fresh = fresh_with(expected_items)
    assert index.additions() == fresh.additions()
    # Live copper, drills and their occurrence labels equal a fresh rebuild.
    assert index._objects == fresh._objects
    assert index._holes == fresh._holes
    for net in (*NETS, "X"):
        assert index.owned_count(net) == fresh.owned_count(net)
    assert answers(index) == answers(fresh)


@pytest.mark.parametrize("seed,owner", [(1, "A"), (2, "C"), (3, "D"), (4, "B")])
def test_remove_leaves_the_answers_of_a_fresh_index(seed, owner):
    items = copper(seed)
    index = fresh_with(items)
    expected_count = sum(1 for item, locked in items if item.net == owner and not locked)
    assert expected_count and index.owned_count(owner) == expected_count
    assert index.remove(owner) == expected_count
    assert index.owned_count(owner) == 0
    remaining = [(item, locked) for item, locked in items
                 if item.net != owner or locked]
    assert_equivalent(index, remaining)


def test_removing_several_owners_and_compaction_keep_fresh_answers():
    items = copper(7, 60)
    index = fresh_with(items)
    remaining = items
    for owner in ("B", "D"):
        assert index.remove(owner) == sum(
            1 for item, locked in remaining if item.net == owner and not locked)
        remaining = [(item, locked) for item, locked in remaining
                     if item.net != owner or locked]
        assert None in index._slots and None in index._hole_slots
        index._compact()  # Normally amortized; exercise the renumbering.
        assert None not in index._slots and None not in index._hole_slots
    # Removal after compaction uses the renumbered slots.
    add(index, [(item, False) for item, locked in items[:10] if item.net == "B"])
    remaining += [(item, False) for item, locked in items[:10] if item.net == "B"]
    assert index.remove("C") == sum(1 for item, locked in remaining if item.net == "C" and not locked)
    remaining = [(item, locked) for item, locked in remaining if item.net != "C" or locked]
    assert_equivalent(index, remaining)


def test_remove_keeps_locked_additions_and_board_copper():
    track = TrackSegment("A", Point.mm(10, 3), Point.mm(14, 3), nm_from_mm("0.2"), F)
    loose = TrackSegment("A", Point.mm(10, 5), Point.mm(14, 5), nm_from_mm("0.2"), F)
    via = Via("A", Point.mm(20, 3), nm_from_mm("0.6"), nm_from_mm("0.3"))
    index = fresh_with([(track, True), (loose, False), (via, True)])
    assert index.owned_count("A") == 1
    assert index.remove("A") == 1
    assert index.remove("A") == 0
    base = RoutingClearanceIndex(board()).additions()
    assert index.additions() == (*base, (track, True), (via, True))
    assert not index.can_track("B", Point.mm(11, 2.9), Point.mm(13, 2.9), nm_from_mm("0.2"), F)
    assert index.can_track("B", Point.mm(11, 5), Point.mm(13, 5), nm_from_mm("0.2"), F)
    assert not index.can_via("B", Point.mm(20, 3.5), nm_from_mm("0.6"), F, B)
    # Board-source copper of net A is locked too.
    assert not index.can_track("B", Point.mm(4, 30), Point.mm(8, 30), nm_from_mm("0.2"), F)
    assert_equivalent(index, [(track, True), (via, True)])


def test_unknown_owner_removes_nothing():
    items = copper(11)
    index = fresh_with(items)
    before = (index.additions(), list(index._objects), list(index._holes),
              {key: list(ids) for key, ids in index._bins.items()})
    assert index.remove("NOPE") == 0
    assert index.remove("<non-plated-hole>") == 0
    assert index.owned_count("NOPE") == 0
    assert (index.additions(), index._objects, index._holes, index._bins) == before


def test_add_remove_add_is_idempotent():
    items = copper(5)
    index = fresh_with(items)
    for _ in range(3):
        index.remove("C")
        add(index, [(item, locked) for item, locked in items if item.net == "C" and not locked])
    expected = ([(item, locked) for item, locked in items if item.net != "C" or locked]
                + [(item, locked) for item, locked in items if item.net == "C" and not locked])
    assert_equivalent(index, expected)


def test_additions_log_keeps_chronological_order_after_removal():
    width = nm_from_mm("0.2")
    log = [(TrackSegment(net, Point.mm(3 + 2 * i, 20), Point.mm(4 + 2 * i, 20), width, F), False)
           for i, net in enumerate("ABABCA")]
    index = fresh_with(log)
    base = len(RoutingClearanceIndex(board()).additions())
    assert index.remove("B") == 2
    assert index.additions()[base:] == tuple(entry for entry in log if entry[0].net != "B")
    # Default occurrence labels are renumbered to the remaining log positions.
    labels = sorted({item.occurrence for item in index._objects if item.occurrence.startswith("addition:")})
    assert labels == [f"addition:{base + i}:track" for i in range(4)]
    assert index.owned_count("A") == 3 and index.owned_count("C") == 1


def test_binned_drills_match_a_linear_scan():
    index = fresh_with(copper(13, 80))
    index.remove("A")
    clearance = index.board.rules.minimum_hole_clearance_nm
    random = Random(3)

    def linear(net, position, radius, span):
        for hole in index._holes:
            distance = ((position.x_nm - hole.position.x_nm) ** 2
                        + (position.y_nm - hole.position.y_nm) ** 2)
            if distance == 0 and hole.net == net and hole.reusable and hole.span == span:
                continue
            required = radius + hole.radius_nm + clearance
            if distance < required * required:
                return False
        return True

    holes = [hole.position for hole in index._holes]
    probes = [Point(random.randrange(0, 32_000_000), random.randrange(0, 34_000_000))
              for _ in range(1500)]
    probes += [Point(p.x_nm + random.randrange(-900_000, 900_000), p.y_nm) for p in holes]
    probes += holes
    for probe in probes:
        for net in NETS:
            for radius in (150_000, 300_000, 2_000_000):
                assert index._hole_clear(net, probe, radius, (F, B)) == linear(net, probe, radius, (F, B))

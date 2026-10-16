from dataclasses import replace
from itertools import product
from math import cos, hypot, radians, sin

import pytest

from pcbir import (BoardOutline, CopperLayer, NetRoutingRule, PhysicalBoard,
                   PhysicalNet, Point, RouteKind, Stackup, nm_from_mm)
from pcbir.physical import DesignRules, MechanicalHole, TrackSegment, Via
from pcbir.pair_search import _HEADS, _Port
from pcbir.pair_vias import _return_positions, _return_via, transition_spacing
from pcbir.routing_clearance import RoutingClearanceIndex


F, B = CopperLayer.FRONT, CopperLayer.BACK


def fixture(angle=0, center=(12.3, 8.7), names=('first', 'second', 'reference')):
    rules = tuple(NetRoutingRule(net, RouteKind.DIFFERENTIAL,
        differential_partner=partner, width_nm=nm_from_mm('.145'),
        pair_gap_nm=nm_from_mm('.15'), clearance_nm=nm_from_mm('.29'),
        allowed_layers=(F, B), max_vias=2, require_return_vias=True,
        return_via_net=names[2], maximum_return_via_distance_nm=nm_from_mm('.75'))
        for net, partner in ((names[0], names[1]), (names[1], names[0])))
    board = PhysicalBoard('BoundaryDomain', BoardOutline.rectangle(40, 25), {}, (),
        tuple(PhysicalNet(net, ()) for net in names),
        stackup=Stackup(copper_layers=(F, CopperLayer.INTERNAL_1,
            CopperLayer.INTERNAL_2, CopperLayer.INTERNAL_3,
            CopperLayer.INTERNAL_4, B)),
        rules=DesignRules(minimum_clearance_nm=nm_from_mm('.1'),
            minimum_hole_clearance_nm=nm_from_mm('.2'),
            minimum_track_width_nm=nm_from_mm('.1'),
            default_via_size_nm=nm_from_mm('.45'),
            default_via_drill_nm=nm_from_mm('.2')),
        net_routing_rules=rules)
    midpoint = Point.mm(*center)
    offset = (transition_spacing(board, *rules)+1)//2
    dx, dy = cos(radians(angle)), sin(radians(angle))
    pair = tuple(Via(net, Point(midpoint.x_nm+round(sign*offset*dx),
                                midpoint.y_nm+round(sign*offset*dy)),
                     nm_from_mm('.45'), nm_from_mm('.2'))
                 for net, sign in ((names[0], -1), (names[1], 1)))
    port = _Port(midpoint, 0, 1, (), (), 0)
    return board, rules, pair, port


def reserved(board, pair):
    index = RoutingClearanceIndex(board)
    for via in pair:
        assert index.can_via(via.net, via.position, via.size_nm, F, B, via.drill_nm)
        index.add_via(via, locked=True)
    return index


def old_positions(port):
    dx, dy = _HEADS[port.heading]
    fixed = [Point(port.center.x_nm+sign*dx*nm_from_mm(distance),
                   port.center.y_nm+sign*dy*nm_from_mm(distance))
             for distance, sign in product(('.8', '1', '1.25', '1.5', '2'), (1, -1))]
    grid = [Point(port.center.x_nm+x*nm_from_mm('.125'),
                  port.center.y_nm+y*nm_from_mm('.125'))
            for x, y in product(range(-16, 17), repeat=2) if x or y]
    return tuple(dict.fromkeys((*fixed, *grid)))


def propose(board, rules, pair, port, index=None):
    return _return_via(board, index or reserved(board, pair), port, pair, *rules, F, B)


def test_old_grid_misses_nonempty_narrow_return_domain():
    board, rules, pair, port = fixture()
    index = reserved(board, pair)
    assert not [p for p in old_positions(port)
                if all(hypot(p.x_nm-v.position.x_nm, p.y_nm-v.position.y_nm)
                       <= nm_from_mm('.75') for v in pair)
                and index.can_via('reference', p, nm_from_mm('.45'), F, B,
                                  nm_from_mm('.2'))]
    returned = propose(board, rules, pair, port, index)
    assert returned is not None and len(returned) == 1
    assert index.candidate_vias_clear((*pair, *returned))
    distances = [hypot(returned[0].position.x_nm-v.position.x_nm,
                       returned[0].position.y_nm-v.position.y_nm) for v in pair]
    assert all(nm_from_mm('.74') <= d <= nm_from_mm('.75') for d in distances)
    assert board.tracks == board.vias == ()


@pytest.mark.parametrize('angle,center,names', [
    (0, (12.3, 8.7), ('P', 'N', 'foil')),
    (90, (24.625, 17.375), ('east', 'west', 'return')),
    (45, (16.01, 11.003), ('odd1', 'odd2', 'shell')),
    (23, (6.127, 5.319), ('renamed-a', 'renamed-b', 'renamed-plane')),
])
def test_boundary_sites_follow_actual_translated_rotated_vias(angle, center, names):
    board, rules, pair, port = fixture(angle, center, names)
    index = reserved(board, pair)
    before = tuple(index._objects)
    first = propose(board, rules, pair, port, index)
    assert first and first == propose(board, rules, pair, port, index)
    assert first[0].net == names[2]
    assert first[0].size_nm == pair[0].size_nm
    assert first[0].drill_nm == pair[0].drill_nm
    assert (first[0].from_layer, first[0].to_layer) == (F, B)
    assert tuple(index._objects) == before
    assert index.candidate_vias_clear((*pair, *first))
    assert all((first[0].position.x_nm-v.position.x_nm)**2
               +(first[0].position.y_nm-v.position.y_nm)**2
               <= nm_from_mm('.75')**2 for v in pair)


@pytest.mark.parametrize('limit', ('.75', '1.5', '3'))
def test_sites_replace_futile_slots_without_increasing_existing_allowance(limit):
    board, rules, pair, port = fixture()
    rules = tuple(replace(r, maximum_return_via_distance_nm=nm_from_mm(limit)) for r in rules)
    board = replace(board, net_routing_rules=rules)
    positions = _return_positions(board, port, pair, *rules, 'reference', nm_from_mm(limit))
    assert len(positions) <= len(old_positions(port)) == 1090
    assert len(positions) == len(set(positions))
    assert positions == _return_positions(board, port, pair, *rules, 'reference', nm_from_mm(limit))
    # All old sites that can actually satisfy exact geometry remain present.
    index = reserved(board, pair)
    legal_old = [p for p in old_positions(port)
                 if all(hypot(p.x_nm-v.position.x_nm, p.y_nm-v.position.y_nm)
                        <= nm_from_mm(limit) for v in pair)
                 and index.can_via('reference', p, nm_from_mm('.45'), F, B, nm_from_mm('.2'))]
    assert set(legal_old).issubset(positions)


def test_actual_via_sizes_not_default_guess_drive_boundary_sites():
    board, rules, pair, port = fixture(17)
    board = replace(board, rules=replace(board.rules, default_via_size_nm=nm_from_mm('.8')))
    pair = (pair[0], replace(pair[1], size_nm=nm_from_mm('.43')))
    index = reserved(board, pair)
    returned = propose(board, rules, pair, port, index)
    assert returned and returned[0].size_nm == nm_from_mm('.45')
    assert index.candidate_vias_clear((*pair, *returned))


def test_impossible_distance_has_no_relaxed_fallback():
    board, rules, pair, port = fixture()
    rules = tuple(replace(r, maximum_return_via_distance_nm=nm_from_mm('.73')) for r in rules)
    board = replace(board, net_routing_rules=rules)
    assert propose(board, rules, pair, port) is None
    assert all(r.maximum_return_via_distance_nm == nm_from_mm('.73') for r in rules)


@pytest.mark.parametrize('process', ('inner_span', 'unknown_technology', 'filled_finish'))
def test_wrong_via_process_cannot_become_a_return(process):
    board, rules, pair, port = fixture()
    index = reserved(board, pair)
    if process == 'inner_span':
        pair = (replace(pair[0], to_layer=CopperLayer.INTERNAL_2), pair[1])
    elif process == 'unknown_technology':
        pair = (replace(pair[0], technology='unqualified-process'), pair[1])
    else:
        pair = (replace(pair[0], finish='filled-capped'), pair[1])
    assert propose(board, rules, pair, port, index) is None


@pytest.mark.parametrize('failure', ('foreign_signal', 'missing_return', 'different_returns'))
def test_wrong_net_binding_fails_closed(failure):
    board, rules, pair, port = fixture()
    index = reserved(board, pair)
    if failure == 'foreign_signal':
        pair = (replace(pair[0], net='unrelated'), pair[1])
    elif failure == 'missing_return':
        rules = tuple(replace(r, return_via_net='not-declared') for r in rules)
    else:
        rules = (rules[0], replace(rules[1], return_via_net='another-return'))
    assert propose(board, rules, pair, port, index) is None


def test_old_geometry_index_cannot_authorize_current_source():
    board, rules, pair, port = fixture()
    index = reserved(board, pair)
    new_source = replace(board, vias=(Via('reference', Point.mm(25, 20),
                                        nm_from_mm('.45'), nm_from_mm('.2')),))
    assert propose(new_source, rules, pair, port, index) is None
    # Source occurrence multiplicity is not collapsed by a set check.
    old_one = replace(new_source, vias=new_source.vias*2)
    assert propose(old_one, rules, pair, port, reserved(new_source, pair)) is None


@pytest.mark.parametrize('blocker', ('inner_copper', 'holes'))
def test_exact_all_layer_or_drill_blockers_reject_boundary_sites(blocker):
    board, rules, pair, port = fixture()
    cx, cy = port.center.x_nm, port.center.y_nm
    if blocker == 'inner_copper':
        copper = tuple(TrackSegment('obstacle', Point(cx-nm_from_mm(2), cy+sign*nm_from_mm('.645')),
                    Point(cx+nm_from_mm(2), cy+sign*nm_from_mm('.645')),
                    nm_from_mm('.1'), CopperLayer.INTERNAL_3) for sign in (1, -1))
        blocked = replace(board, tracks=copper, nets=(*board.nets, PhysicalNet('obstacle', ())))
    else:
        holes = tuple(MechanicalHole(str(sign), Point(cx, cy+sign*nm_from_mm('.645')),
                                     nm_from_mm('.3')) for sign in (1, -1))
        blocked = replace(board, mechanical_holes=holes)
    index = reserved(blocked, pair)
    assert index.candidate_vias_clear(pair)
    assert propose(blocked, rules, pair, port, index) is None
    # An index with extra immutable prefix geometry is permitted, but its
    # constraints still reject returns; none of that prefix is removed.
    if blocker == 'inner_copper':
        source = replace(blocked, tracks=())
        assert propose(source, rules, pair, port, index) is None
    assert blocked.vias == ()

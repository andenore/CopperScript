"""Return contact completion and whole-obligation native admission."""
from dataclasses import replace
from collections import Counter

import pytest

from pcbir import (BoardOutline, CopperLayer, CopperZone, FootprintPad,
                   PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet,
                   Placement, Point, PolygonRing, PolygonWithHoles, Size,
                   Stackup, TrackSegment, Via)
from pcbir.drc import explicit_copper_connectivity, physical_board_digest
from pcbir.plane import PlaneStitchOptions, stitch_zone_pads
from pcbir.power_geometry import PowerAreaOptions


def fixture():
    large = PhysicalFootprint('large', (FootprintPad('1', Point(0, 0), Size.mm(10, 10)),), Size.mm(11, 11))
    small = PhysicalFootprint('small', (FootprintPad('1', Point(0, 0), Size.mm('.5', '.5')),), Size.mm(1, 1))
    outline = BoardOutline.rectangle(24, 20)
    zone = CopperZone('reference', 'RETURN', (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(outline.vertices)))
    via = Via('RETURN', Point.mm('16.5', 10), 600000, 300000, CopperLayer.FRONT, CopperLayer.BACK)
    trace = TrackSegment('RETURN', Point.mm(16, 10), via.position, 250000, CopperLayer.FRONT)
    board = PhysicalBoard('large-return', outline, {'large': large, 'small': small},
        (Placement('EXPOSED', 'large', Point.mm(10, 10)), Placement('PIN', 'small', Point.mm(16, 10))),
        (PhysicalNet('RETURN', (PadReference('EXPOSED', '1'), PadReference('PIN', '1'))),),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, CopperLayer.INTERNAL_2, CopperLayer.BACK)),
        tracks=(trace,), vias=(via,), zones=(zone,))
    options = PowerAreaOptions(allowed_layers=(CopperLayer.FRONT,), maximum_branch_layers=1,
        branch_search_states=100, maximum_layer_alternatives=1)
    return board, options


def test_large_return_land_joins_existing_escape_without_new_via_or_process():
    from pcbir.power_preparation import join_return_contacts
    board, options = fixture()
    original = physical_board_digest(board)
    pending = stitch_zone_pads(board, PlaneStitchOptions(only_pads=frozenset({PadReference('EXPOSED', '1')})))
    assert pending.pending_pads == (PadReference('EXPOSED', '1'),)
    after, observations = join_return_contacts(pending.board, board.nets[0].pads, options)
    graph = explicit_copper_connectivity(after, include_internal_connections=False)
    assert graph.net_connected(after.nets[0])
    assert after.vias == board.vias and after.via_in_pad_rules == board.via_in_pad_rules
    assert after.placements == board.placements and after.zones == board.zones
    assert after.tracks[:len(board.tracks)] == board.tracks
    assert physical_board_digest(board) == original
    assert observations[0].terminals == (PadReference('EXPOSED', '1'),)
    assert observations[0].status == 'joined_existing_component'
    assert observations[0].bridge_layer == CopperLayer.FRONT
    assert after.metadata.get('via_in_pad_process') is None


@pytest.mark.parametrize('policy', ['disabled', 'blocked', 'no_reference', 'wrong_net'])
def test_return_completion_does_not_invent_access_or_cross_owners(policy):
    from pcbir.power_preparation import join_return_contacts
    board, options = fixture()
    if policy == 'disabled':
        options = replace(options, maximum_branch_layers=0)
    elif policy == 'no_reference':
        board = replace(board, zones=())
    elif policy == 'wrong_net':
        board = replace(board, nets=(PhysicalNet('RETURN', (PadReference('EXPOSED', '1'),)),
            PhysicalNet('OTHER', (PadReference('PIN', '1'),))),
            tracks=tuple(replace(t, net='OTHER') for t in board.tracks),
            vias=tuple(replace(v, net='OTHER') for v in board.vias))
    else:
        wall = TrackSegment('WALL', Point.mm('15.4', 0), Point.mm('15.4', 20), 250000, CopperLayer.FRONT)
        board = replace(board, nets=(*board.nets, PhysicalNet('WALL', ())), tracks=(*board.tracks, wall))
    after, observations = join_return_contacts(board, board.nets[0].pads, options)
    assert Counter(after.tracks) == Counter(board.tracks) and after.vias == board.vias
    assert after.nets == board.nets


def test_native_return_admission_requires_every_target_pad_not_one_bypass_return(tmp_path):
    from test_power_preparation import fixture as base_fixture, verifier
    from pcbir.decoupling_paths import _return_targets
    from pcbir.power_preparation import assess_native_return_contacts
    board, _ = base_fixture()
    original = board.footprints['two']
    with_extra = replace(original, name='with-extra', pads=(*original.pads,
        FootprintPad('9', Point.mm(0, 3), Size.mm('.4', '.4'))))
    board = replace(board, footprints={**board.footprints, 'with-extra': with_extra},
        placements=tuple(replace(p, footprint='with-extra') if p.reference == 'U' else p for p in board.placements),
        nets=tuple(replace(n, pads=(*n.pads, PadReference('U', '9'))) if n.name == 'GND' else n for n in board.nets))
    link = board.decoupling_links[0]
    required = frozenset((link.return_pad, *_return_targets(board, link)))
    seeded = stitch_zone_pads(board, PlaneStitchOptions(only_pads=required-{PadReference('U', '9')})).board
    verify, _ = verifier(tmp_path)
    evidence = verify(seeded, 'split-return')
    assert evidence.decoupling_assessments[0].return_connected
    observation = assess_native_return_contacts(seeded, evidence, required)
    assert observation['status'] == 'required_return_contacts_open'
    assert len(observation['nets'][0]['groups']) == 2
    completed = stitch_zone_pads(seeded, PlaneStitchOptions(only_pads=frozenset({PadReference('U', '9')}))).board
    assert assess_native_return_contacts(completed, evidence, required)['status'] == 'fresh_native_evidence_required'
    final = verify(completed, 'complete-return')
    assert assess_native_return_contacts(completed, final, required)['status'] == 'native_connected'
    assert assess_native_return_contacts(completed, final, required|{PadReference('MISSING', '1')})['status'] != 'native_connected'


def test_preparation_completes_large_return_and_rolls_back_when_disabled(tmp_path):
    from test_power_preparation import fixture as base_fixture, verifier
    from pcbir.fanout import FanoutOptions
    from pcbir.power_preparation import prepare_power_distribution
    from pcbir.routing import route_global
    board, options = base_fixture()
    original = board.footprints['two']
    with_ep = replace(original, name='with-ep', pads=(*original.pads,
        FootprintPad('49', Point.mm(0, -6), Size.mm(8, 6))))
    board = replace(board, footprints={**board.footprints, 'with-ep': with_ep},
        placements=tuple(replace(p, footprint='with-ep') if p.reference == 'U' else p for p in board.placements),
        nets=tuple(replace(n, pads=(*n.pads, PadReference('U', '49'))) if n.name == 'GND' else n for n in board.nets))
    options = replace(options, maximum_branch_layers=0)
    verify, _ = verifier(tmp_path)
    failed = prepare_power_distribution(board, route_global(board), options,
        FanoutOptions(), native_verifier=verify)
    assert failed.status == 'native_power_return_rejected'
    assert failed.observation['return_contacts']['status'] == 'required_return_contacts_open'
    assert failed.board is board and not failed.evidence
    assert not failed.return_contact_observations
    accepted = prepare_power_distribution(board, route_global(board),
        replace(options, maximum_branch_layers=1), FanoutOptions(), native_verifier=verify)
    assert accepted.accepted, accepted.report()
    assert accepted.observation['return_contacts']['status'] == 'native_connected'
    assert any(o.terminals == (PadReference('U', '49'),)
               and o.status == 'joined_existing_component' for o in accepted.return_contact_observations)
    assert accepted.board.via_in_pad_rules == board.via_in_pad_rules == ()
    # A local stitch rescan cannot rediscover this distant contact. The normal
    # handoff must use the fresh complete return ledger, preserve its copper,
    # and still test any other requested pad.
    from pcbir.package_access import preflight_prepared_power_access
    from pcbir.power_preparation import required_return_contacts, preparation_preserved
    required = required_return_contacts(accepted.board, accepted.plan)
    rescan = stitch_zone_pads(accepted.board, PlaneStitchOptions(only_pads=required))
    assert PadReference('U', '49') in rescan.pending_pads
    access = preflight_prepared_power_access(board, route_global(board), accepted,
        FanoutOptions(plane_return_nets=frozenset({'RAIL'})),
        PlaneStitchOptions(only_pads=required|{PadReference('J', '2')}),
        only_nets=frozenset({'RAIL'}))
    assert access.ready and not access.pending_pads
    assert access.plane_stitch.stitched_pads == (PadReference('J', '2'),)
    assert preparation_preserved(accepted, access.board)
    from pcbir.power_preparation import assess_native_return_contacts
    fresh = verify(access.board, 'return-handoff')
    assert assess_native_return_contacts(access.board, fresh, required)['status'] == 'native_connected'

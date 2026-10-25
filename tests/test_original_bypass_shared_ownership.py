"""Unselected original return ownership survives total canonical reselection."""
from collections import Counter
from dataclasses import fields, replace

import pytest

from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.contact_owner_regeneration import _valid_exits, regenerate_contact_owners
from pcbir.decoupling_paths import refresh_crosslayer_witnesses, validate_crosslayer_witness
from pcbir.drc import run_physical_drc
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import _partial_ordinary_pattern, _verified_bypass_links
from pcbir.physical import (CopperLayer, FootprintPad, NetRoutingRule, PadReference,
    PhysicalNet, Point, RouteKind, Size, TrackSegment, Via)
from pcbir.placement import transformed_pad_position
from test_decoupling_crosslayer_paths import fixture
from test_admitted_bypass_paths import route_outward_fixture


def scene(renamed):
    source, first = fixture(renamed=renamed)
    chip = source.footprints['chip']
    chip = replace(chip, pads=(*chip.pads,
        FootprintPad('H', Point.mm(-1, 2), Size.mm('.4', '.4'))))
    cap = replace(source.placements[1], position=Point.mm(11, 9))
    other_cap = replace(cap, reference='OTHER_CAP', position=Point.mm(9, 12))
    inverse_cap = replace(cap, reference='INVERSE', position=Point.mm(6, 5))
    other = replace(first, capacitor=PadReference(other_cap.reference, '1'),
                    return_pad=PadReference(other_cap.reference, '2'))
    inverse = replace(first, capacitor=PadReference(inverse_cap.reference, '1'),
        return_pad=PadReference(inverse_cap.reference, '2'),
        target=PadReference(first.target.component, 'G'),
        net=first.return_net, return_net=first.net)
    points = ((11.5, 9), (15, 9), (15, 3), (5.5, 3), (5.5, 5))
    source_branch = tuple(TrackSegment(first.net, Point.mm(*a), Point.mm(*b),
        150_000, CopperLayer.BACK) for a, b in zip(points, points[1:]))
    pair = ('DIFF_A', 'DIFF_B')
    source = replace(source, footprints={**source.footprints, 'chip': chip},
        placements=(source.placements[0], cap, inverse_cap, other_cap),
        tracks=source_branch, decoupling_links=(first, inverse, other),
        nets=(replace(source.nets[0], pads=(*source.nets[0].pads,
                    other.capacitor, inverse.return_pad)),
              replace(source.nets[1], pads=(*source.nets[1].pads,
                    PadReference(first.target.component, 'H'), other.return_pad,
                    inverse.capacitor)), *(PhysicalNet(n, ()) for n in pair)),
        # These are explicit fixture limits before routing, never a trial waiver.
        net_routing_rules=(*tuple(replace(r, max_vias=4, max_length_nm=100_000_000)
                                  for r in source.net_routing_rules),
            *(NetRoutingRule(n, RouteKind.DIFFERENTIAL, width_nm=100_000,
                pair_gap_nm=100_000, differential_partner=partner)
              for n, partner in (pair, tuple(reversed(pair))))))
    generated = route_outward_fixture(source)
    assert not generated.decoupling_pending
    old = generated.decoupling_witnesses[0].supply_path.vias[0]
    other_via = next(v for v in generated.decoupling_vias if v.net == old.net and v is not old)
    shortcut_via = Via(old.net, Point(other_via.position.x_nm,
        other_via.position.y_nm - 500_000), 450_000, 200_000)
    target_pose = next(p for p in source.placements if p.reference == first.target.component)
    target = transformed_pad_position(source, target_pose, first.target.pad)
    # Ordinary exact copper gives the unselected cohort a shorter alternative.
    # It is deliberately not relabelled as generated semantic bypass ownership.
    shortcut = (
        TrackSegment(old.net, source_branch[1].end, shortcut_via.position,
                     150_000, CopperLayer.BACK),
        TrackSegment(old.net, shortcut_via.position, other_via.position,
                     150_000, CopperLayer.BACK),
        TrackSegment(old.net, shortcut_via.position, target,
                     150_000, CopperLayer.FRONT))
    baseline = replace(generated,
        board=replace(generated.board, tracks=(*generated.board.tracks, *shortcut),
                      vias=(*generated.board.vias, shortcut_via)),
        created_tracks=(*generated.created_tracks, *shortcut),
        created_vias=(*generated.created_vias, shortcut_via),
        added_track_count=generated.added_track_count + len(shortcut),
        added_via_count=generated.added_via_count + 1)
    witnesses, reason = refresh_crosslayer_witnesses(baseline.board, baseline.decoupling_witnesses)
    assert reason is None
    baseline = replace(baseline, decoupling_witnesses=witnesses)
    obstacles = (
        TrackSegment(pair[0], Point(old.position.x_nm - 50_000, old.position.y_nm),
            Point(old.position.x_nm + 50_000, old.position.y_nm), 100_000, CopperLayer.FRONT),
        TrackSegment(pair[1], Point.mm(2, 2), Point.mm(2.1, 2), 100_000, CopperLayer.FRONT))
    return source, baseline, old, inverse, pair, obstacles


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('operator', ('partial', 'contact'))
def test_noncanonical_shared_original_via_protects_unselected_cohort(renamed, operator):
    source, baseline, old, inverse, pair, obstacles = scene(renamed)
    board = baseline.board
    assert not [f for f in run_physical_drc(board).findings
                if f.severity.value == 'error'
                and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'}]
    assert all(getattr(source, f.name) == getattr(board, f.name)
               for f in fields(source) if f.name not in {'tracks', 'vias'})
    assert board.tracks == (*source.tracks, *baseline.created_tracks)
    assert board.vias == (*source.vias, *baseline.created_vias)
    assert baseline.added_track_count == len(baseline.created_tracks)
    assert baseline.added_via_count == len(baseline.created_vias)
    assert not Counter(baseline.decoupling_tracks) - Counter(baseline.created_tracks)
    assert not Counter(baseline.decoupling_vias) - Counter(baseline.created_vias)
    assert _valid_exits(board, baseline)
    assert verified_bypass_reservations(source, baseline)[2] is None
    originals = baseline.decoupling_witnesses
    assert len(originals) == 3
    assert any(v is old for v in originals[0].supply_path.vias)
    unselected = next(w for w in originals if w.link == inverse)
    assert unselected.link.net != old.net
    assert any(v is old for v in unselected.return_path.vias)
    for witness in originals:
        assert validate_crosslayer_witness(board, witness) == ''
        for path in (witness.supply_path, witness.return_path):
            nodes = (path.steps[0].start, *(s.end for s in path.steps))
            assert len(nodes) == len(set(nodes))
    names = {link.net for link in source.decoupling_links}
    paths, vias = {}, {}
    canonical = _verified_bypass_links(board, names, witness_paths=paths, witness_vias=vias)
    assert canonical == frozenset(source.decoupling_links)
    # Stronger than selected-cohort reselection: NO fresh canonical association
    # uses this shared via. Only the exact original ledger protects it.
    assert all(v is not old for values in vias.values() for v in values)
    assert _verified_bypass_links(board, names, witness_paths=paths, witness_vias=vias,
        admitted_witnesses=originals) == canonical
    assert any(v is old for v in vias[inverse])
    before = repr(baseline)
    if operator == 'partial':
        answer = _partial_ordinary_pattern(baseline, source,
            replace(source, tracks=(*source.tracks, *obstacles)), FanoutOptions(), 8, True)
        assert answer[0] is None and answer[-1] == 'protected_ordinary_owner'
    else:
        assert regenerate_contact_owners(source, baseline, pair, obstacles, (),
            FanoutOptions(), maximum_owner_nets=8) is None
    assert repr(baseline) == before and baseline.board is board
    assert any(v is old for v in baseline.board.vias)
    assert _valid_exits(baseline.board, baseline)
    assert validate_crosslayer_witness(baseline.board, unselected) == ''
    assert verified_bypass_reservations(source, baseline)[2] is None

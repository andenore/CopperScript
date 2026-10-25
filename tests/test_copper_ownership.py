"""Occurrence authority and dependency closure never infer ownership by net."""
from dataclasses import replace
import pytest

from pcbir.copper_ownership import (CopperUse, begin_copper_ownership, append_copper_ownership,
                                    prepare_copper_trial, record_copper_uses)
from pcbir.physical import CopperLayer, PadReference, Point, TrackSegment, Via
from test_escape_assignment import board


def setup():
    source = board()
    first = TrackSegment('A', Point.mm(5, 6), Point.mm(3, 6), 250000, CopperLayer.FRONT)
    second = TrackSegment('A', Point.mm(3, 6), Point.mm(3, 4), 250000, CopperLayer.FRONT)
    returned = TrackSegment('B', Point.mm(5, 8), Point.mm(3, 8), 250000, CopperLayer.FRONT)
    via = Via('A', Point.mm(3, 4), 800000, 400000)
    uses = (CopperUse('local-loop', 'bypass', (PadReference('U', '1'), PadReference('U', '2')), (first, second, returned), (via,)),
            CopperUse('shared-feed', 'connection', (PadReference('U', '1'), PadReference('JA', '1')), (second,), (via,)))
    after = replace(source, tracks=(first, second, returned), vias=(via,))
    ledger = append_copper_ownership(begin_copper_ownership(source), source, after, 'local-contacts',
                                     uses=uses, trial_mutable=True)
    return source, after, ledger


def test_shared_supply_and_return_dependency_closure_is_complete_and_non_mutating():
    source, after, ledger = setup()
    trial = prepare_copper_trial(after, ledger, ['track:0'])
    assert trial.held == source and trial.original is after and trial.matches()
    assert set(trial.released) == {'track:0', 'track:1', 'track:2', 'via:0'}
    assert set(trial.requirements) == {'local-loop', 'shared-feed'}
    assert len(after.tracks) == 3 and len(after.vias) == 1
    assert ledger.matches(after)


def test_same_net_unrelated_geometry_is_retained_exactly():
    source, after, ledger = setup()
    other = TrackSegment('A', Point.mm(9, 2), Point.mm(10, 2), 250000, CopperLayer.BACK)
    appended = replace(after, tracks=(*after.tracks, other))
    observed = append_copper_ownership(ledger, after, appended, 'other-source',
        uses=(CopperUse('other', 'connection', (PadReference('JA', '1'),), (other,)),), trial_mutable=True)
    trial = prepare_copper_trial(appended, observed, ['via:0'])
    assert trial.held.tracks == (other,) and not trial.held.vias
    assert observed.report()['occurrences'][-1]['requirements'] == ('other',)


@pytest.mark.parametrize('mode', ['frozen', 'source', 'unattributed', 'ambiguous'])
def test_frozen_source_unattributed_and_ambiguous_occurrences_cannot_be_released(mode):
    source, after, ledger = setup()
    if mode == 'frozen':
        ledger = replace(ledger, frozen=True)
    elif mode == 'source':
        ledger = record_copper_uses(begin_copper_ownership(after), after, ledger.uses)
    elif mode == 'unattributed':
        ledger = append_copper_ownership(begin_copper_ownership(source), source, after, 'unknown', trial_mutable=True)
    else:
        duplicate = replace(after, tracks=(*after.tracks, after.tracks[0]))
        ledger = append_copper_ownership(ledger, after, duplicate, 'duplicate', uses=ledger.uses, trial_mutable=True)
        after = duplicate
    with pytest.raises(ValueError):
        prepare_copper_trial(after, ledger, ['track:0'])
    assert ledger.matches(after)


def test_dependency_bound_is_shared_and_does_not_return_partial_release():
    _, after, ledger = setup()
    with pytest.raises(ValueError, match='closure budget'):
        prepare_copper_trial(after, ledger, ['track:0'], maximum_occurrences=3)
    assert ledger.matches(after)
    assert not replace(prepare_copper_trial(after, ledger, ['track:0']), released=('track:0',)).matches()


@pytest.mark.parametrize('change', ['pose', 'copper', 'metadata'])
def test_stale_context_never_authorizes_a_trial(change):
    _, after, ledger = setup()
    if change == 'pose':
        altered = replace(after, placements=tuple(replace(p, position=Point.mm(6, 7)) if p.reference == 'U' else p for p in after.placements))
    elif change == 'copper':
        altered = replace(after, tracks=tuple(reversed(after.tracks)))
    else:
        altered = replace(after, metadata={'changed': 'true'})
    assert not ledger.matches(altered)
    with pytest.raises(ValueError, match='fresh'):
        prepare_copper_trial(altered, ledger, ['track:0'])


def test_capture_rejects_rewrites_and_physical_intent_changes():
    _, after, ledger = setup()
    with pytest.raises(ValueError, match='prefix'):
        append_copper_ownership(ledger, after, replace(after, tracks=after.tracks[:-1]), 'bad')
    with pytest.raises(ValueError, match='physical intent'):
        append_copper_ownership(ledger, after, replace(after, placements=tuple(
            replace(p, position=Point.mm(6, 7)) if p.reference == 'U' else p for p in after.placements)), 'bad')
    with pytest.raises(ValueError, match='absent'):
        record_copper_uses(ledger, after, (CopperUse('fake', 'contact', (PadReference('U', '1'),), (), (Via('A', Point.mm(9, 9), 800000, 400000),)),))


def test_anchor_dependencies_expand_in_both_directions_without_shared_edges():
    source, after, ledger = setup()
    independent = TrackSegment('B', Point.mm(9, 9), Point.mm(10, 9), 250000, CopperLayer.BACK)
    extended = replace(after, tracks=(*after.tracks, independent))
    ledger = append_copper_ownership(ledger, after, extended, 'dependent-bridge', trial_mutable=True,
        uses=(CopperUse('bridge', 'contact', (PadReference('U', '2'),), (independent,), (), ('local-loop',)),))
    for seed in ('track:0', 'track:3'):
        trial = prepare_copper_trial(extended, ledger, (seed,))
        assert trial.held == source and 'bridge' in trial.requirements
        assert trial.matches()


def test_required_array_is_fixed_in_the_ownership_baseline():
    from test_pad_via_arrays import _array_board
    from pcbir.pad_via_arrays import materialize_via_in_pad_arrays
    source = materialize_via_in_pad_arrays(_array_board(rows=2, columns=2, pitch_nm=500000))
    ledger = begin_copper_ownership(source)
    assert len(source.vias) == 4
    for index in range(4):
        with pytest.raises(ValueError, match='protected'):
            prepare_copper_trial(source, ledger, (f'via:{index}',))

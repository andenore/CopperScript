"""Failure evidence remains exact, finite and independent of routing authority."""
from dataclasses import asdict, replace
import json

import pytest

from pcbir.access_diagnostics import AccessRecorder
from pcbir.drc import physical_board_digest
from pcbir.fanout import FanoutOptions, _legal_choices, route_fanout
from pcbir.physical import (CopperLayer, NetRoutingRule, PadReference, Point,
                            TrackSegment, Via)
from pcbir.routing_clearance import RoutingClearanceIndex
from test_interstitial_fanout import fixture
from test_escape_assignment import board, opts, trap_candidates


def test_duplicate_input_and_added_copper_keep_distinct_occurrences():
    base = board()
    blocker = TrackSegment('B', Point.mm(3, 3), Point.mm(3, 9), 150000, CopperLayer.FRONT)
    query = TrackSegment('A', Point.mm(2, 5), Point.mm(4, 5), 150000, CopperLayer.FRONT)
    snapshot = replace(base, tracks=(blocker, blocker))
    index = RoutingClearanceIndex(snapshot)
    records = index.track_blocker_records(query)
    assert {r.occurrence for r in records} == {'track:0', 'track:1'}
    assert all(r.immutable for r in records)
    index.add_track(blocker)
    records = index.track_blocker_records(query)
    assert {r.occurrence for r in records} == {'track:0', 'track:1', 'addition:2:track'}
    assert not next(r for r in records if r.occurrence == 'addition:2:track').immutable
    assert index.board is snapshot and index.board.tracks == (blocker, blocker)


def test_via_probe_reports_same_net_bottom_pad_instead_of_allowing_overlap():
    from pcbir.physical import BoardSide, PhysicalNet, Placement
    base, pose, pad, start = fixture()
    endpoint = Point.mm('5.375', '5.375')
    bottom = Placement('C', pose.footprint, endpoint, side=BoardSide.BACK)
    base = replace(base, placements=(*base.placements, bottom), nets=tuple(
        replace(n, pads=(*n.pads, PadReference('C', '5'))) if n.name == 'N5' else n for n in base.nets))
    recorder = AccessRecorder()
    assert not _legal_choices(base, RoutingClearanceIndex(base), 'N5', start, pose, pad,
                              (endpoint,), recorder=recorder)
    evidence = recorder.finish(physical_board_digest(base), 'sampled_sites_exhausted')
    assert evidence.observed_site_queries == 1
    assert any(b.occurrence == 'pad:C.5:4' and b.net == 'N5'
               and b.reason == 'via annulus must avoid pad copper'
               for event in evidence.samples for b in event.blockers)
    assert evidence.expanded_states is None
    json.dumps(asdict(evidence))


def test_failed_path_retains_emitted_width_and_actual_blocking_occurrence():
    base, pose, pad, start = fixture()
    # The via is clear; a narrow foreign track intersects only its short launch.
    blocker = TrackSegment('N4', Point.mm('5.05', '5.01'), Point.mm('5.05', '5.1'), 50000, CopperLayer.FRONT)
    base = replace(base, tracks=(blocker,))
    recorder = AccessRecorder()
    assert RoutingClearanceIndex(base).can_via('N5', Point.mm('5.375', '5.375'),
        450000, CopperLayer.FRONT, CopperLayer.BACK, drill_nm=200000)
    assert not _legal_choices(base, RoutingClearanceIndex(base), 'N5', start, pose, pad,
                              (Point.mm('5.375', '5.375'),), recorder=recorder)
    evidence = recorder.finish(physical_board_digest(base), 'sampled_sites_exhausted')
    events = [sample for sample in evidence.samples if sample.outcome == 'path_blocked']
    assert events and all(t.width_nm == 150000 for event in events for t in event.tracks)
    assert any(b.occurrence == 'track:0' for event in events for b in event.blockers)


def test_diagnostic_work_limit_does_not_limit_or_relax_the_candidate_search():
    base, pose, pad, start = fixture()
    endpoints = (start, Point.mm('5.75', '5.75'), Point.mm('5.375', '5.375'))
    plain = _legal_choices(base, RoutingClearanceIndex(base), 'N5', start, pose, pad, endpoints)
    for count in (0, 1, 2, 24):
        recorder = AccessRecorder(count)
        observed = _legal_choices(base, RoutingClearanceIndex(base), 'N5', start, pose, pad,
                                  endpoints, recorder=recorder)
        evidence = recorder.finish(physical_board_digest(base), 'candidate_selected')
        assert plain == observed and plain
        assert evidence.observed_site_queries == 3 and len(evidence.samples) <= count
        assert sum(c for _, _, c in evidence.outcome_counts) == 3
    calls = []
    recorder = AccessRecorder(0)
    recorder.record(start, 'coarse', 'via_blocked', blockers=lambda: calls.append(1))
    assert not calls


def test_assignment_samples_survive_exhausted_generation_sample_slots():
    recorder = AccessRecorder(24)
    calls = []
    for phase in ('coarse', 'refined', 'two_leg', 'refined_two_leg'):
        for i in range(100):
            recorder.record(Point(i, i), phase, 'via_blocked', blockers=lambda: calls.append(1) or ())
    for i in range(100):
        recorder.record(Point(i, i), 'assignment', 'selected_escape_conflict', blockers=lambda: calls.append(1) or ())
    evidence = recorder.finish('snapshot', 'selected_escape_conflict')
    assert any(s.phase == 'assignment' for s in evidence.samples)
    assert len(calls) == len(evidence.samples) <= 24
    assert evidence.samples_truncated and sum(n for _, _, n in evidence.outcome_counts) == 500


def test_layer_access_failure_is_not_reported_as_a_geometric_blocker():
    base, pose, pad, start = fixture()
    base = replace(base, net_routing_rules=(NetRoutingRule('N5', allowed_layers=(CopperLayer.FRONT,)),))
    recorder = AccessRecorder()
    assert not _legal_choices(base, RoutingClearanceIndex(base), 'N5', start, pose, pad,
                              (Point.mm('5.375', '5.375'),), recorder=recorder)
    result = recorder.finish(physical_board_digest(base), 'sampled_sites_exhausted')
    assert result.outcome_counts == (('coarse', 'layer_access_unavailable', 1),)
    assert not result.observed_site_queries and not result.samples[0].blockers


def test_conflict_diagnostics_bind_the_proposal_separately_from_immutable_input(monkeypatch):
    import pcbir.fanout as fanout
    monkeypatch.setattr(fanout, '_candidates', trap_candidates)
    base = board()
    result = route_fanout(base, opts(two_leg_escapes=False, joint_escapes=False))
    missing = next(a for a in result.pin_analysis if a.pad in result.pending_pads)
    assert missing.legal_candidate_count == 2
    evidence = missing.access_diagnostics
    assert evidence.source_digest == physical_board_digest(base)
    assignments = [sample for sample in evidence.samples if sample.phase == 'assignment']
    assert assignments and all(s.context_digest == physical_board_digest(result.board) for s in assignments)
    assert all(any(b.net == 'A' and b.occurrence for b in sample.blockers) for sample in assignments)
    assert evidence.stop_reason == 'selected_escape_conflict'
    json.dumps(asdict(evidence))


@pytest.mark.parametrize('count', [-1, True, 1.5, '2'])
def test_diagnostic_bounds_are_explicit(count):
    with pytest.raises(ValueError, match='diagnostic samples'):
        FanoutOptions(diagnostic_samples_per_pad=count)


def test_standard_route_board_exposes_the_extra_site_allowance_explicitly():
    from pcbir.cli import _parser
    parser = _parser()
    assert parser.parse_args(['route-board', 'board.copper']).fanout_geometry_candidates == 0
    assert parser.parse_args(['route-board', 'board.copper', '--fanout-geometry-candidates', '32']).fanout_geometry_candidates == 32
    with pytest.raises(SystemExit):
        parser.parse_args(['route-board', 'board.copper', '--fanout-geometry-candidates', '-1'])


def test_geometry_family_expands_nonempty_conflicting_domains_only_once(monkeypatch):
    import pcbir.fanout as fanout
    monkeypatch.setattr(fanout, '_candidates', lambda *args: (Point.mm(3, 7),))
    calls = []
    def geometry(index, net, *args, **kwargs):
        calls.append(net)
        return (Point.mm(3, 4),) if net == 'A' else (Point.mm(3, 7),)
    monkeypatch.setattr(fanout, 'obstacle_boundary_sites', geometry)
    settings = opts(two_leg_escapes=False, refinement_step_nm=500000, surface_fallback=False)
    old = route_fanout(board(), settings)
    new = route_fanout(board(), replace(settings, maximum_geometry_candidates=1))
    assert old.pending_pads and not new.pending_pads
    assert set(old.escaped_pads) <= new.escaped_pads
    assert new.accesses[PadReference('U', '1')] == Point.mm(3, 4)
    assert calls and all(calls.count(net) == 1 for net in set(calls))

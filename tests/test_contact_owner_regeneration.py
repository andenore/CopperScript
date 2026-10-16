"""Exact contact proposals regenerate generated owners with complete rollback."""
from collections import Counter
from dataclasses import replace

import pytest

import pcbir.contact_owner_regeneration as regeneration
from pcbir.contact_owner_regeneration import regenerate_contact_owners
from pcbir.fanout import route_fanout
from pcbir.hard_macros import materialize_hard_macros, validate_hard_macros
from pcbir.pad_via_arrays import require_via_in_pad_arrays, via_in_pad_array_vias
from pcbir.physical import (ComponentPlacementRule, CopperLayer, NetRoutingRule, PadReference, PhysicalNet,
                           Placement, Point, RouteKind, TrackSegment)
from pcbir.package_access import _verified_bypass_links
from test_package_access import pattern_trap
from test_bypass_owner_regeneration import bypass_fixture
from test_autonomous_plane_access import with_test_owners


def fixture(tmp_path, *, renamed=False, immutable=False, feed=False):
    if feed:
        board, _, _, settings, names = bypass_fixture(renamed=renamed, hits_feed=False)
        primary, ordinary = names['A'], names['B']
    else:
        board, _, settings = pattern_trap()
        primary, ordinary = 'A', 'B'
        if renamed:
            board = replace(board,
                nets=tuple(replace(n, name='CLOCK_X' if n.name == 'A' else 'OWNER_Y') for n in board.nets),
                net_routing_rules=tuple(replace(r, net='CLOCK_X') for r in board.net_routing_rules))
            primary, ordinary = 'CLOCK_X', 'OWNER_Y'
    companion = 'COMPLEMENT_Z'
    board = replace(board,
        placements=(*board.placements, Placement('PAIR_REMOTE', 'one', Point.mm(2, 3)),
                    Placement('PAIR_LOAD', 'one', Point.mm(4, 3)),
                    Placement('QUIET', 'two', Point.mm(5, 13)),
                    Placement('QUIET_LOAD', 'one', Point.mm(13, 14))),
        nets=(*board.nets, PhysicalNet(companion, (PadReference('PAIR_REMOTE', '1'), PadReference('PAIR_LOAD', '1'))),
              PhysicalNet('UNRELATED', (PadReference('QUIET', '1'), PadReference('QUIET_LOAD', '1')))),
        net_routing_rules=(NetRoutingRule(primary, RouteKind.DIFFERENTIAL, width_nm=board.rules.minimum_track_width_nm,
            pair_gap_nm=board.rules.minimum_clearance_nm, differential_partner=companion, max_vias=0, allowed_layers=(CopperLayer.FRONT,)),
            NetRoutingRule(companion, RouteKind.DIFFERENTIAL, width_nm=board.rules.minimum_track_width_nm,
            pair_gap_nm=board.rules.minimum_clearance_nm, differential_partner=primary, max_vias=0, allowed_layers=(CopperLayer.FRONT,))))
    if immutable:
        # Existing synthetic macro/filled-array contracts, never vendor assets.
        if feed:
            raise ValueError('independent macro/array and semantic-feed fixtures')
        board = replace(board, footprints={**board.footprints, 'terminal': board.footprints['one']})
        board = with_test_owners(board, tmp_path, required_array=True)
    board = replace(board, placement_rules=tuple(ComponentPlacementRule(p.reference,
        fixed_position=p.position, allowed_orientations=(p.rotation_degrees,), edge_clearance_nm=250000)
        for p in board.placements))
    if immutable:
        board = materialize_hard_macros(board)
    baseline = route_fanout(board, settings)
    site = next(v.position for v in baseline.created_vias if v.net == ordinary)
    # Legal disposable differential-owner obstacles exercise launch ownership,
    # not an assertion that these partial pieces complete the pair itself.
    tracks = (TrackSegment(primary, Point(site.x_nm-100000, site.y_nm),
                            Point(site.x_nm+100000, site.y_nm), board.rules.minimum_track_width_nm, CopperLayer.FRONT),
              TrackSegment(companion, Point.mm(2, 12), Point.mm('2.2', 12), board.rules.minimum_track_width_nm, CopperLayer.FRONT))
    return board, baseline, (primary, companion), tracks, settings, ordinary


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('immutable', (False, True))
def test_intersecting_owner_only_regenerated_with_exact_source_and_unrelated_prefix(tmp_path, renamed, immutable):
    board, baseline, group, tracks, settings, ordinary = fixture(tmp_path, renamed=renamed, immutable=immutable)
    before = baseline.board
    real = regeneration.route_fanout
    calls = []
    def inspect(source, options, **kwargs):
        calls.append(kwargs['only_nets'])
        return real(source, options, **kwargs)
    proposed = regenerate_contact_owners(board, baseline, group, tracks, (), settings, maximum_owner_nets=8)
    assert proposed is not None
    assert proposed.escaped_pads >= baseline.escaped_pads
    assert proposed.board.placements == board.placements
    assert replace(proposed.board, tracks=board.tracks, vias=board.vias) == board
    assert proposed.board.tracks[:len(board.tracks)] == board.tracks
    assert proposed.board.vias[:len(board.vias)] == board.vias
    assert Counter(t for t in baseline.created_tracks if t.net != ordinary) <= Counter(proposed.created_tracks)
    assert Counter(v for v in baseline.created_vias if v.net != ordinary) <= Counter(proposed.created_vias)
    assert proposed.accesses[PadReference('QUIET', '1')] == baseline.accesses[PadReference('QUIET', '1')]
    assert not any(t.net in group for t in proposed.created_tracks)
    assert all(t not in proposed.board.tracks for t in tracks)
    assert before == baseline.board
    if immutable:
        validate_hard_macros(proposed.board)
        require_via_in_pad_arrays(proposed.board, 'contact regeneration test')
        arrays = Counter(via_in_pad_array_vias(board))
        assert sum(arrays.values()) == 4
        assert all(Counter(proposed.board.vias)[v] == n for v, n in arrays.items())
    # Observe the real bounded router's subset argument on a deterministic replay.
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(regeneration, 'route_fanout', inspect)
        assert proposed == regenerate_contact_owners(board, baseline, group, tracks, (), settings, maximum_owner_nets=8)
    assert calls == [frozenset((ordinary,))]


def test_generated_feed_requires_original_association_and_fresh_exact_witness(tmp_path):
    board, baseline, group, tracks, settings, ordinary = fixture(tmp_path, feed=True)
    required = _verified_bypass_links(baseline.board, {ordinary})
    assert required
    proposed = regenerate_contact_owners(board, baseline, group, tracks, (), settings, maximum_owner_nets=8)
    assert proposed is not None
    assert required <= _verified_bypass_links(proposed.board, {ordinary})
    assert Counter(proposed.decoupling_tracks) <= Counter(proposed.created_tracks)
    anonymous = replace(board, decoupling_links=())
    unverified = replace(baseline, board=replace(baseline.board, decoupling_links=()))
    assert regenerate_contact_owners(anonymous, unverified, group, tracks, (), settings, maximum_owner_nets=8) is None


@pytest.mark.parametrize('defect', ('lost_exit', 'lost_source', 'stale_pose', 'foreign_ownership', 'lost_feed'))
def test_bad_rebuild_rolls_back_all_inputs(tmp_path, monkeypatch, defect):
    board, baseline, group, tracks, settings, ordinary = fixture(tmp_path, feed=defect == 'lost_feed', immutable=defect == 'lost_source')
    before = baseline.board
    real = regeneration.route_fanout
    def corrupt(*args, **kwargs):
        result = real(*args, **kwargs)
        if defect == 'lost_exit':
            pad = next(iter(result.accesses))
            return replace(result, accesses={p:v for p,v in result.accesses.items() if p != pad})
        if defect == 'lost_source':
            return replace(result, board=replace(result.board, vias=result.board.vias[1:]))
        if defect == 'stale_pose':
            poses = tuple(replace(p, position=Point(p.position.x_nm+1000, p.position.y_nm)) if p.reference == 'U' else p
                          for p in result.board.placements)
            return replace(result, board=replace(result.board, placements=poses))
        if defect == 'foreign_ownership':
            return replace(result, created_tracks=(*result.created_tracks, tracks[0]))
        feeds = Counter(result.decoupling_tracks)
        kept = []
        for track in result.board.tracks:
            if feeds[track]:
                feeds[track] -= 1
            else:
                kept.append(track)
        return replace(result, board=replace(result.board, tracks=tuple(kept)))
    monkeypatch.setattr(regeneration, 'route_fanout', corrupt)
    assert regenerate_contact_owners(board, baseline, group, tracks, (), settings, maximum_owner_nets=8) is None
    assert baseline.board == before


def test_immutable_identity_wins_over_identical_generated_geometry(tmp_path):
    board, baseline, group, tracks, settings, ordinary = fixture(tmp_path)
    owned = next(v for v in baseline.created_vias if v.net == ordinary)
    source = replace(board, vias=(*board.vias, owned, owned))
    original = replace(baseline, board=replace(source, tracks=(*source.tracks, *baseline.created_tracks),
                                               vias=(*source.vias, *baseline.created_vias)))
    before = original.board
    assert regenerate_contact_owners(source, original, group, tracks, (), settings, maximum_owner_nets=8) is None
    assert original.board == before and Counter(source.vias)[owned] == 2


def test_disabled_budget_and_stale_ownership_do_not_run_a_trial(tmp_path, monkeypatch):
    board, baseline, group, tracks, settings, _ = fixture(tmp_path)
    def forbidden(*args, **kwargs):
        raise AssertionError('no authorized owner trial')
    monkeypatch.setattr(regeneration, 'route_fanout', forbidden)
    assert regenerate_contact_owners(board, baseline, group, tracks, (), settings, maximum_owner_nets=0) is None
    stale = replace(baseline, created_vias=baseline.created_vias[:-1])
    assert regenerate_contact_owners(board, stale, group, tracks, (), settings, maximum_owner_nets=8) is None


@pytest.mark.parametrize('feed',(False,True))
def test_regeneration_keeps_latest_scoped_late_report_and_exact_pending_ids(tmp_path,monkeypatch,feed):
    from pcbir.fanout_bypass_completion import LateBypassCompletionReport
    board,baseline,group,tracks,settings,ordinary=fixture(tmp_path,feed=feed)
    old=LateBypassCompletionReport(status='completed',passes=1,added_tracks=99,
                                   reason='old whole-fanout stage')
    old_pending=tuple((link,'old selected-owner stage') for link in board.decoupling_links if link.net==ordinary)
    baseline=replace(baseline,late_bypass=old,decoupling_pending_links=old_pending)
    real=regeneration.route_fanout;rebuilt_results=[]
    current=LateBypassCompletionReport(status='not_needed',reason='latest selected-owner stage')
    def rebuilt(*a,**k):
        result=replace(real(*a,**k),late_bypass=current)
        rebuilt_results.append(result)
        return result
    monkeypatch.setattr(regeneration,'route_fanout',rebuilt)
    result=regenerate_contact_owners(board,baseline,group,tracks,(),settings,maximum_owner_nets=8)
    assert result is not None and len(rebuilt_results)==1
    assert result.late_bypass is current and result.late_bypass is not baseline.late_bypass
    assert result.decoupling_pending_links==rebuilt_results[0].decoupling_pending_links
    assert baseline.late_bypass is old and baseline.decoupling_pending_links==old_pending
    assert result.escaped_pads>=baseline.escaped_pads
    if feed:
        assert old_pending and _verified_bypass_links(baseline.board,{ordinary})<=_verified_bypass_links(result.board,{ordinary})

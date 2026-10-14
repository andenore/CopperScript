"""Actual cross-face fanout admission and exact supply/return owner rollback."""
from collections import Counter
from dataclasses import replace

import pytest

import pcbir.contact_owner_regeneration as regeneration
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.fanout import FanoutOptions, route_fanout
from pcbir.package_access import _verified_bypass_links
from pcbir.physical import CopperLayer, NetRoutingRule, PhysicalNet, Point, RouteKind, TrackSegment
from test_decoupling_crosslayer_paths import fixture


def pair_fixture(*, two=False):
    board, link = fixture()
    if two:
        other, sibling = fixture(renamed=True)
        board = replace(board,
            placements=(*board.placements, *(replace(p, position=Point(p.position.x_nm,
                p.position.y_nm+6_000_000)) for p in other.placements)),
            nets=(*board.nets, *other.nets), decoupling_links=(link, sibling),
            net_routing_rules=(*board.net_routing_rules, *other.net_routing_rules))
    # Synthetic disposable pair pieces exercise ownership; they do not claim
    # completion of a source-backed differential engineering contract.
    group = ('PROBE_A', 'PROBE_B')
    board = replace(board, nets=(*board.nets, *(PhysicalNet(name, ()) for name in group)),
        net_routing_rules=(*board.net_routing_rules, *(
        NetRoutingRule(name, RouteKind.DIFFERENTIAL, width_nm=100000,
            differential_partner=group[1-i], pair_gap_nm=100000, max_vias=0,
            allowed_layers=(CopperLayer.FRONT,)) for i, name in enumerate(group))))
    baseline = route_fanout(board)
    return board, link, baseline, group


def contact_at(baseline, group, net):
    site = next(v.position for v in baseline.decoupling_vias if v.net == net)
    return (TrackSegment(group[0], Point(site.x_nm-100000, site.y_nm),
                         Point(site.x_nm+100000, site.y_nm), 100000, CopperLayer.FRONT),
            TrackSegment(group[1], Point.mm(2, 3), Point.mm('2.2', 3),
                         100000, CopperLayer.FRONT))


@pytest.mark.parametrize('renamed', (False, True))
def test_real_fanout_complete_crossface_witness_read_only_admission(monkeypatch, renamed):
    board, link = fixture(renamed=renamed)
    result = route_fanout(board)
    assert len(result.decoupling_vias) == 2 and not result.decoupling_pending
    assert not result.pending_pads and len(result.accesses) == 4
    paths, vias = {}, {}
    assert _verified_bypass_links(result.board, {link.net}, witness_paths=paths,
                                  witness_vias=vias) == {link}
    assert {t.net for t in paths[link]} == {link.net, link.return_net}
    assert {v.net for v in vias[link]} == {link.net, link.return_net}
    assert all(any(t is actual for actual in result.board.tracks) for t in paths[link])
    assert all(any(v is actual for actual in result.board.vias) for v in vias[link])
    def forbidden(*args, **kwargs):
        raise AssertionError('fresh read-only admission must not route')
    monkeypatch.setattr('pcbir.decoupling.route_decouplers', forbidden)
    monkeypatch.setattr('pcbir.decoupling_paths.propose_crosslayer_decoupling', forbidden)
    monkeypatch.setattr('pcbir.fanout.route_fanout', forbidden)
    before = result.board
    admitted = verified_bypass_reservations(board, result)
    assert admitted == (result.decoupling_tracks, result.decoupling_vias, None)
    assert verified_bypass_reservations(board, result) == admitted
    assert all(a is b for a, b in zip(admitted[1], result.decoupling_vias))
    assert result.board is before and not board.tracks and not board.vias


@pytest.mark.parametrize('defect', ('missing_return_track', 'missing_return_via',
    'extra_ground_track', 'extra_ground_via', 'duplicate_via', 'duplicate_claim',
    'immutable_via_claim', 'count', 'stale_pose', 'flag_only'))
def test_crossface_admission_rejects_incomplete_or_forged_semantics(defect):
    source, link = fixture()
    result = route_fanout(source)
    tracks, vias = result.created_tracks, result.created_vias
    declared_tracks, declared_vias = result.decoupling_tracks, result.decoupling_vias
    if defect == 'missing_return_track':
        gone = next(t for t in tracks if t.net == link.return_net)
        tracks = tuple(t for t in tracks if t != gone)
        declared_tracks = tuple(t for t in declared_tracks if t != gone)
    elif defect == 'missing_return_via':
        vias = tuple(v for v in vias if v.net != link.return_net)
        declared_vias = tuple(v for v in declared_vias if v.net != link.return_net)
    elif defect == 'extra_ground_track':
        extra = TrackSegment(link.return_net, Point.mm(20, 15), Point.mm(21, 15),
                             150000, CopperLayer.FRONT)
        tracks = (*tracks, extra); declared_tracks = (*declared_tracks, extra)
    elif defect == 'extra_ground_via':
        extra = replace(next(v for v in vias if v.net == link.return_net), position=Point.mm(20, 15))
        vias = (*vias, extra); declared_vias = (*declared_vias, extra)
    elif defect == 'duplicate_via':
        vias = (*vias, vias[0])
    elif defect == 'duplicate_claim':
        declared_vias = (*declared_vias, declared_vias[0])
    elif defect == 'immutable_via_claim':
        source = replace(source, vias=(vias[0],))
    elif defect == 'flag_only':
        tracks = declared_tracks = ()
        source = replace(source, metadata={**source.metadata, 'connected': True})
    result = replace(result, board=replace(source, tracks=(*source.tracks, *tracks),
                     vias=(*source.vias, *vias)), created_tracks=tracks, created_vias=vias,
                     added_track_count=len(tracks), added_via_count=len(vias),
                     decoupling_tracks=declared_tracks, decoupling_vias=declared_vias)
    if defect == 'count':
        result = replace(result, added_via_count=0)
    elif defect == 'stale_pose':
        result = replace(result, board=replace(result.board, placements=(result.board.placements[0],
            replace(result.board.placements[1], position=Point.mm(14, 9)))))
    admitted = verified_bypass_reservations(source, result)
    assert admitted[0] == admitted[1] == () and admitted[2]


@pytest.mark.parametrize('two', (False, True))
def test_supply_regeneration_preserves_unselected_return_and_sibling_exactly(two, monkeypatch):
    board, link, baseline, group = pair_fixture(two=two)
    contact = contact_at(baseline, group, link.net)
    before = baseline.board
    real = regeneration.route_fanout
    calls = []
    def measure(*args, **kwargs):
        calls.append(kwargs['only_nets'])
        return real(*args, **kwargs)
    monkeypatch.setattr(regeneration, 'route_fanout', measure)
    proposal = regeneration.regenerate_contact_owners(board, baseline, group, contact, (),
        FanoutOptions(), maximum_owner_nets=1)
    assert proposal is not None
    assert calls == [frozenset((link.net,))]
    assert verified_bypass_reservations(board, proposal)[2] is None
    assert _verified_bypass_links(proposal.board, {l.net for l in board.decoupling_links}) == set(board.decoupling_links)
    kept_tracks = tuple(t for t in baseline.decoupling_tracks if t.net != link.net)
    kept_vias = tuple(v for v in baseline.decoupling_vias if v.net != link.net)
    assert Counter(kept_tracks) <= Counter(proposal.decoupling_tracks)
    assert Counter(kept_vias) <= Counter(proposal.decoupling_vias)
    assert all(any(v is actual for actual in proposal.created_vias) for v in kept_vias)
    assert baseline.escaped_pads <= proposal.escaped_pads
    assert all(t not in proposal.board.tracks for t in contact)
    assert replace(proposal.board, tracks=board.tracks, vias=board.vias) == board
    assert baseline.board is before


def test_return_only_collision_cannot_release_unselected_supply_cohort(monkeypatch):
    board, link, baseline, group = pair_fixture()
    contact = contact_at(baseline, group, link.return_net)
    def forbidden(*args, **kwargs):
        raise AssertionError('return-only owner must remain protected')
    monkeypatch.setattr(regeneration, 'route_fanout', forbidden)
    before = baseline.board
    assert regeneration.regenerate_contact_owners(board, baseline, group, contact, (),
        FanoutOptions(), maximum_owner_nets=1) is None
    assert baseline.board is before
    assert _verified_bypass_links(before, {link.net}) == {link}


def test_selected_supply_may_create_only_its_exact_declared_return_without_extra_owner(monkeypatch):
    board, link, full, group = pair_fixture()
    via = next(v for v in full.decoupling_vias if v.net == link.net)
    # A genuinely verified ordinary target launch precedes the incomplete
    # association. It grants ownership of that launch, never a return-net slot.
    track = next(t for t in full.created_tracks if t.net == link.net
                 and t.layer is CopperLayer.FRONT)
    baseline = replace(full, board=replace(board, tracks=(track,), vias=(via,)),
        created_tracks=(track,), created_vias=(via,), added_track_count=1, added_via_count=1,
        decoupling_tracks=(), decoupling_vias=(), accesses={link.target:via.position},
        surface_accesses={}, pin_analysis=(),
        pending_pads=tuple(p for net in board.nets for p in net.pads if p != link.target),
        decoupling_pending=((link.capacitor, 'unrouted association'),))
    assert verified_bypass_reservations(board, baseline)[2] is None
    assert not _verified_bypass_links(baseline.board, {link.net})
    real = regeneration.route_fanout
    calls = []
    def measure(*args, **kwargs):
        calls.append(kwargs['only_nets'])
        return real(*args, **kwargs)
    monkeypatch.setattr(regeneration, 'route_fanout', measure)
    contact = contact_at(full, group, link.net)
    proposal = regeneration.regenerate_contact_owners(board, baseline, group, contact, (),
        FanoutOptions(), maximum_owner_nets=1)
    assert proposal is not None
    assert calls == [frozenset((link.net,))]
    returned = tuple(v for v in proposal.created_vias if v.net == link.return_net)
    assert returned and Counter(returned) <= Counter(proposal.decoupling_vias)
    assert _verified_bypass_links(proposal.board, {link.net}) == {link}
    assert verified_bypass_reservations(board, proposal)[2] is None
    assert all(t not in proposal.board.tracks for t in contact)


@pytest.mark.parametrize('defect', ('via_count', 'extra_ground', 'source_prefix', 'lost_witness',
                                  'lost_semantic_via', 'lost_semantic_track'))
def test_corrupt_crossface_rebuild_never_adopts_partial_ownership(monkeypatch, defect):
    board, link, baseline, group = pair_fixture()
    contact = contact_at(baseline, group, link.net)
    real = regeneration.route_fanout
    def corrupt(*args, **kwargs):
        result = real(*args, **kwargs)
        if defect == 'via_count':
            return replace(result, added_via_count=result.added_via_count+1)
        if defect == 'lost_semantic_via':
            return replace(result, decoupling_vias=())
        if defect == 'lost_semantic_track':
            return replace(result, decoupling_tracks=())
        if defect == 'source_prefix':
            return replace(result, board=replace(result.board, vias=result.board.vias[1:]))
        if defect == 'lost_witness':
            via = next(v for v in result.board.vias if v.net == link.return_net)
            return replace(result, board=replace(result.board, vias=tuple(v for v in result.board.vias if v != via)))
        extra = replace(baseline.decoupling_vias[-1], position=Point.mm(20, 15))
        return replace(result, board=replace(result.board, vias=(*result.board.vias, extra)),
            created_vias=(*result.created_vias, extra), added_via_count=result.added_via_count+1,
            decoupling_vias=(*result.decoupling_vias, extra))
    monkeypatch.setattr(regeneration, 'route_fanout', corrupt)
    before = baseline.board
    assert regeneration.regenerate_contact_owners(board, baseline, group, contact, (),
        FanoutOptions(), maximum_owner_nets=1) is None
    assert baseline.board is before


def test_formal_critical_pair_and_retained_clock_admit_only_complete_crossface_prefix():
    from pcbir import GlobalRouterOptions, route_global, route_critical_nets
    from pcbir.critical_reuse import prepare_single_critical_reuse
    from pcbir.physical import (CopperZone, FootprintPad, PadReference,
        PhysicalFootprint, Placement, PolygonRing, PolygonWithHoles, Size)
    from test_critical_routing import _pair_board
    pair = _pair_board()
    feed, link = fixture()
    terminal = PhysicalFootprint('single_clock', (FootprintPad('1', Point(0, 0),
                                 Size.mm('.4', '.4')),), Size.mm(1, 1))
    board = replace(pair, rules=feed.rules, stackup=feed.stackup,
        footprints={**pair.footprints, **feed.footprints, terminal.name:terminal},
        placements=(*pair.placements, *(replace(p, position=Point(p.position.x_nm,
            p.position.y_nm+10_000_000)) for p in feed.placements),
            Placement('CLK_A', terminal.name, Point.mm(5, 5)),
            Placement('CLK_B', terminal.name, Point.mm(20, 5))),
        nets=(*pair.nets, *feed.nets, PhysicalNet('CLOCK_OWNER',
            (PadReference('CLK_A', '1'), PadReference('CLK_B', '1')))),
        decoupling_links=feed.decoupling_links,
        zones=(CopperZone('ordinary_return_plane', link.return_net,
                         (CopperLayer.INTERNAL_1,), PolygonWithHoles(PolygonRing(pair.outline.vertices))),),
        net_routing_rules=(*pair.net_routing_rules, *feed.net_routing_rules,
            NetRoutingRule('CLOCK_OWNER', RouteKind.CLOCK, width_nm=150000,
                allowed_layers=(CopperLayer.FRONT,), max_vias=0, max_length_nm=30_000_000)))
    ordinary = route_fanout(board, only_nets={link.net})
    assert len(ordinary.decoupling_vias) == 2
    assert verified_bypass_reservations(board, ordinary)[2] is None
    guides = route_global(board, GlobalRouterOptions(tile_size_nm=2_500_000,
                                                   maximum_iterations=1))
    donor = route_critical_nets(board, guides, reserved_accesses=ordinary,
                               pair_state_limit=6000)
    assert all(n.connected for n in donor.nets), donor.to_json()
    assert any(len(n.nets) == 2 for n in donor.nets)
    assert donor.locked_tracks[:len(ordinary.board.tracks)] == ordinary.board.tracks
    assert donor.locked_vias[:len(ordinary.board.vias)] == ordinary.board.vias
    assert _verified_bypass_links(donor.board, {link.net}) == {link}
    prepared = prepare_single_critical_reuse(board, guides, donor, reserved_accesses=ordinary)
    assert prepared.rejection_reason == ''
    assert [c.net for c in prepared.candidates] == ['CLOCK_OWNER']
    replay = route_critical_nets(board, guides, reserved_accesses=ordinary,
        retained_critical=donor, retained_accesses=ordinary, pair_state_limit=6000)
    assert all(n.connected for n in replay.nets)
    assert next(n for n in replay.nets if n.nets == ('CLOCK_OWNER',)).strategy == 'revalidated_single_net'
    assert replay.locked_vias[:len(ordinary.board.vias)] == ordinary.board.vias
    assert _verified_bypass_links(replay.board, {link.net}) == {link}
    extra = replace(ordinary.decoupling_vias[-1], position=Point.mm(30, 20))
    forged = replace(ordinary, board=replace(ordinary.board, vias=(*ordinary.board.vias, extra)),
        created_vias=(*ordinary.created_vias, extra), decoupling_vias=(*ordinary.decoupling_vias, extra),
        added_via_count=ordinary.added_via_count+1)
    with pytest.raises(ValueError, match='fresh source witness'):
        route_critical_nets(board, guides, reserved_accesses=forged, pair_state_limit=6000)
    rejected = prepare_single_critical_reuse(board, guides, donor, reserved_accesses=forged)
    assert not rejected.candidates and 'fresh source witness' in rejected.rejection_reason

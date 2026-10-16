"""Fresh local bypass identities survive every package/placement acceptance."""
from dataclasses import replace
from types import SimpleNamespace
import pytest
from pcbir.package_access import PackageAccessResult, _improvement_outcome, _verified_bypass_links
from pcbir.fanout import FanoutResult
from pcbir.decoupling_paths import propose_crosslayer_decoupling
from test_decoupling_crosslayer_paths import fixture


def result(source, routed, *, failed):
    fanout = FanoutResult(routed, {}, (), len(routed.tracks), len(routed.vias),
        created_tracks=routed.tracks, created_vias=routed.vias)
    return PackageAccessResult(source, SimpleNamespace(), fanout,
        SimpleNamespace(board=routed), None, frozenset(failed), 0)


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('lost_half', ('supply', 'return'))
def test_critical_gain_cannot_replace_a_complete_bypass_with_pad_count_equality(renamed, lost_half):
    source, link = fixture(renamed=renamed)
    routed = propose_crosslayer_decoupling(source, link)
    assert routed.witness is not None
    baseline = result(source, routed.board, failed={'SIGNAL'})
    net = link.net if lost_half == 'supply' else link.return_net
    damaged = replace(routed.board,
        tracks=tuple(t for t in routed.board.tracks if t.net != net),
        vias=tuple(v for v in routed.board.vias if v.net != net))
    candidate = result(source, damaged, failed=())
    assert _verified_bypass_links(baseline.board, {link.net}) == {link}
    assert not _verified_bypass_links(candidate.board, {link.net})
    assert baseline.pending_pads == candidate.pending_pads
    assert baseline.fanout.escaped_pads == candidate.fanout.escaped_pads
    missing = []
    assert _improvement_outcome(baseline, candidate, failed_bypass_capacitors=missing) == 'lost_bypass_connection'
    assert missing == [link.capacitor]
    assert baseline.board is routed.board


@pytest.mark.parametrize('renamed', (False, True))
def test_preserved_bypass_and_critical_gain_can_still_be_accepted(renamed):
    source, link = fixture(renamed=renamed)
    routed = propose_crosslayer_decoupling(source, link)
    assert routed.witness is not None
    baseline = result(source, routed.board, failed={'SIGNAL'})
    candidate = result(source, routed.board, failed=())
    assert _improvement_outcome(baseline, candidate) == 'accepted'


@pytest.mark.parametrize('renamed', (False, True))
@pytest.mark.parametrize('lost_half', ('supply', 'return'))
def test_pattern_losing_a_bypass_skips_doomed_critical_rebuild(monkeypatch, renamed, lost_half):
    import pcbir.package_access as access
    source, link = fixture(renamed=renamed)
    routed = propose_crosslayer_decoupling(source, link)
    assert routed.witness is not None
    baseline = result(source, routed.board, failed={'SIGNAL'})
    baseline = replace(baseline, critical=SimpleNamespace(board=routed.board, nets=(object(),)))
    net = link.net if lost_half == 'supply' else link.return_net
    damaged = replace(routed.board,
        tracks=tuple(t for t in routed.board.tracks if t.net != net),
        vias=tuple(v for v in routed.board.vias if v.net != net))
    proposal = result(source, damaged, failed=()).fanout
    calls = []
    def critical(*args, **kwargs):
        calls.append(args)
        assert len(calls) == 1, 'Lost bypass is known before rebuilding critical routes'
        return SimpleNamespace(board=source)
    monkeypatch.setattr(access, '_critical_search', critical)
    monkeypatch.setattr(access, '_failures', lambda *args: (frozenset(), 0))
    monkeypatch.setattr(access, '_full_ordinary_pattern', lambda *args, **kwargs: (proposal, 'proposed'))
    events = []
    checked = access._negotiate_patterns(baseline, source, access.FanoutOptions(),
        access.PackageAccessOptions(maximum_pattern_trials=1, maximum_partial_owner_nets=0),
        None, lambda *event: events.append(event))
    assert len(calls) == 1
    assert checked.board is baseline.board
    assert checked.fanout is baseline.fanout
    assert len(checked.pattern_trials) == 1
    trial = checked.pattern_trials[0]
    assert trial.outcome == 'lost_bypass_connection'
    assert trial.failed_bypass_capacitors == (link.capacitor,)
    assert not trial.revalidated
    assert any(details.get('failed_bypass_capacitors') == [f'{link.capacitor.component}.{link.capacitor.pad}']
               for _, _, details in events)

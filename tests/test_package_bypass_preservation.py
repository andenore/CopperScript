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

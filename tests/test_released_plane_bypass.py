"""A changed proposal obstacle frame enables one bounded exact bypass retry."""
from dataclasses import replace
import pytest
from test_fanout_late_bypass_completion import incumbent_scene
from pcbir.physical import TrackSegment, Point, CopperLayer
from pcbir.fanout import FanoutOptions
from pcbir.plane import PlaneStitchResult
from pcbir.plane_bypass_transfer import PlaneContactProposal
from pcbir.package_access import _full_ordinary_pattern
from pcbir.bypass_reservations import verified_bypass_reservations
from pcbir.decoupling_paths import verify_crosslayer_decoupling


@pytest.mark.parametrize('released', (False, True))
def test_released_contact_retries_original_pending_link_with_real_held_launches(monkeypatch, released):
    source, link, incumbent, pending = incumbent_scene()
    unused = TrackSegment(link.return_net, Point.mm(20, 18), Point.mm(21, 18),
                          150000, CopperLayer.BACK)
    frame = replace(source, tracks=(unused,) if released else ())
    plane = PlaneStitchResult(frame, (), (), int(released), 0)
    # Retain the real ordinary launch geometry and exact pending association.
    # The first fanout's own late pass is still obstructed in the proposal
    # context. This isolates the rebase boundary; the retry is the real router.
    def prepared(obstacles, options):
        assert obstacles.tracks == frame.tracks
        return replace(incumbent, board=replace(incumbent.board,
            tracks=(*obstacles.tracks, *incumbent.board.tracks)))
    monkeypatch.setattr('pcbir.package_access.route_fanout', prepared)
    result, reason = _full_ordinary_pattern(source, frame, FanoutOptions(),
        plane_contacts=PlaneContactProposal(source, plane))
    assert result is not None, reason
    assert unused not in result.board.tracks
    assert all(a is b for a, b in zip(incumbent.board.tracks, result.board.tracks))
    assert all(a is b for a, b in zip(incumbent.board.vias, result.board.vias))
    if released:
        assert result.late_bypass.trigger == 'released_plane_contacts'
        assert result.late_bypass.passes == 1 and result.late_bypass.status == 'completed'
        assert not result.decoupling_pending_links
        assert verify_crosslayer_decoupling(result.board, link).witness is not None
    else:
        assert result.late_bypass is None
        assert result.decoupling_pending_links == pending
    assert verified_bypass_reservations(source, result)[2] is None
    assert source.tracks == source.vias == ()


def test_unknown_retry_trigger_rejected():
    from pcbir.fanout_bypass_completion import complete_late_bypasses
    _, _, incumbent, pending = incumbent_scene()
    with pytest.raises(ValueError, match='unknown late bypass trigger'):
        complete_late_bypasses(incumbent, pending, FanoutOptions(), trigger='unbounded')

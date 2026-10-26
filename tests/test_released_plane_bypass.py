"""A changed proposal obstacle frame enables one bounded exact bypass retry."""
import pytest
from test_fanout_late_bypass_completion import incumbent_scene
from pcbir.fanout import FanoutOptions


def test_unknown_retry_trigger_rejected():
    from pcbir.fanout_bypass_completion import complete_late_bypasses
    _, _, incumbent, pending = incumbent_scene()
    with pytest.raises(ValueError, match='unknown late bypass trigger'):
        complete_late_bypasses(incumbent, pending, FanoutOptions(), trigger='unbounded')

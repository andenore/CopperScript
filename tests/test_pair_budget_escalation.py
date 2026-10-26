"""An exhausted bounded pair search escalates its state budget before giving up."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from test_critical_diagnostics import _bent_pair_board

from pcbir import nm_from_mm, route_critical_nets, route_global
from pcbir.unified_router import UnifiedRouterOptions


def _route(limit, **kwargs):
    board = _bent_pair_board(max_skew_nm=nm_from_mm(5))
    return route_critical_nets(board, route_global(board), pair_state_limit=limit, **kwargs).nets[0]


def test_an_exhausted_pair_search_escalates_once_and_records_the_bound() -> None:
    # The bent pair needs about 100 states; a 20-state tier is exhausted with
    # no candidate. The escalated tier (10 x 20) connects it with the same
    # ports and gates, and the result records the bound it ended under.
    bounded = _route(20, pair_state_escalation=1)
    assert not bounded.connected and bounded.pair_budget_exhausted and bounded.pair_state_limit == 20
    escalated = _route(20, pair_state_escalation=10)
    assert escalated.connected and escalated.pair_state_limit == 200
    assert 20 < escalated.search_states <= 200 and not escalated.pair_budget_exhausted


def test_escalation_comes_from_the_unified_options() -> None:
    # The unified router's option drives it (default 10), also when the caller
    # passes no options.
    assert UnifiedRouterOptions().critical_pair_state_escalation == 10
    assert _route(20, unified_options=UnifiedRouterOptions(critical_pair_state_escalation=10)).connected
    assert not _route(20, unified_options=UnifiedRouterOptions(critical_pair_state_escalation=1)).connected
    assert _route(20).connected


def test_a_pair_within_its_tier_never_escalates() -> None:
    plain = _route(2000, pair_state_escalation=10)
    assert plain.connected and plain.pair_state_limit == 2000


def test_escalation_must_be_a_positive_factor() -> None:
    import pytest
    with pytest.raises(ValueError):
        UnifiedRouterOptions(critical_pair_state_escalation=0)
    with pytest.raises(ValueError):
        _route(20, pair_state_escalation=0)

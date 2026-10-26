"""Partial ordinary-owner proposals use real clearance and routing geometry."""

import pytest



def test_cli_partial_owner_budget_is_explicit_and_zero_disables():
    from pcbir.cli import _parser
    parser = _parser()
    assert parser.parse_args(["route-board", "board.copper"]).package_partial_owner_nets == 8
    assert parser.parse_args(["route-board", "board.copper", "--package-partial-owner-nets", "0"]).package_partial_owner_nets == 0
    with pytest.raises(SystemExit):
        parser.parse_args(["route-board", "board.copper", "--package-partial-owner-nets", "9"])



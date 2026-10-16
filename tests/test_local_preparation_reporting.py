"""Observational local contact evidence propagates without changing copper identity."""
from dataclasses import replace
import json
from pcbir import route_global, route_critical_nets
from pcbir.critical import critical_net_document, _fingerprint
from test_local_pair_branches import fixture

def test_blocked_local_work_is_reported_and_no_partial_copper_is_committed():
    board = fixture()
    board = replace(board, net_routing_rules=tuple(replace(r, max_vias=0) for r in board.net_routing_rules))
    guides = route_global(board)
    result = route_critical_nets(board, guides, pair_state_limit=1)
    item, = result.nets
    report = item.local_preparation
    assert not item.connected and report is not None
    assert report.status == 'blocked' and report.searches > 0
    assert report.expanded_states <= 16000
    assert result.board.tracks == board.tracks and result.board.vias == board.vias
    document = critical_net_document(item)
    assert document['local_preparation']['status'] == 'blocked'
    assert json.loads(result.to_json())['nets'][0]['local_preparation']['searches'] == report.searches
    assert all('requires two terminals per member' not in text for text in item.diagnostics)

def test_elapsed_reporting_does_not_change_routing_fingerprint_or_result_equality():
    board = fixture()
    board = replace(board, net_routing_rules=tuple(replace(r, max_vias=0) for r in board.net_routing_rules))
    guides = route_global(board)
    result = route_critical_nets(board, guides, pair_state_limit=1)
    item, = result.nets
    changed = replace(item, local_preparation=replace(item.local_preparation, elapsed_seconds=987.0))
    assert changed == item
    assert _fingerprint(guides.routing_fingerprint, list(result.locked_tracks), list(result.locked_vias), [changed]) == result.routing_fingerprint

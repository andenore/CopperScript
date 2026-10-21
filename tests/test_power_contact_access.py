"""Physical access regressions independent of any consumer/package identity."""
from collections import Counter
from dataclasses import replace

import pytest

from pcbir import (BoardSide, CopperLayer, CopperZone, PadReference, PhysicalNet,
                   PhysicalPowerDomain, Point, PolygonRing, PolygonWithHoles,
                   Stackup, TrackSegment, Via)
from pcbir.physical import CopperKeepout
from pcbir.drc import explicit_copper_connectivity, physical_board_digest
from pcbir.power_geometry import PowerAreaOptions, apply_power_area_plan, plan_power_areas as _plan
from test_power_feed import board as feed_board


def plan_power_areas(board, options):
    # Keep the backbone on the selected inner layer while retaining B.Cu as
    # an allowed local access layer, rather than choosing an all-back tree.
    return _plan(board, options, _seed_layer=CopperLayer.INTERNAL_2)


def fixture():
    b = feed_board()
    inner = CopperLayer.INTERNAL_2
    via = Via('RAIL', Point.mm(3, 6), 600000, 300000, CopperLayer.FRONT, CopperLayer.BACK)
    track = TrackSegment('RAIL', Point.mm(3, 5), via.position, 250000, CopperLayer.FRONT)
    blocked = CopperKeepout('no-load-drills', (CopperLayer.FRONT,),
        PolygonWithHoles(PolygonRing(tuple(Point.mm(x, y) for x, y in
            ((12, 2), (18, 2), (18, 8), (12, 8))))), block_tracks=False, block_zones=False)
    b = replace(b, placements=(b.placements[0], replace(b.placements[1], side=BoardSide.BACK)),
        stackup=Stackup((CopperLayer.FRONT, CopperLayer.INTERNAL_1, inner, CopperLayer.BACK)),
        net_routing_rules=(replace(b.net_routing_rules[0],
            allowed_layers=(CopperLayer.FRONT, inner, CopperLayer.BACK)),),
        tracks=(track,), vias=(via,), copper_keepouts=(blocked,),
        power_domains=(PhysicalPowerDomain('RAIL', b.nets[0].pads, b.nets[0].pads[:1]),))
    options = PowerAreaOptions(candidates_per_domain=1, maximum_layer_alternatives=1,
        allowed_layers=(inner, CopperLayer.BACK), maximum_branch_layers=1,
        search_states=100, branch_search_states=100)
    return b, options


def test_backside_bridge_joins_a_terminal_without_a_backbone_layer_contact():
    board, options = fixture()
    digest = physical_board_digest(board)
    plan = plan_power_areas(board, options)
    assert plan.selected, plan.report()
    routed, status = apply_power_area_plan(board, plan)
    assert status == 'contact_routed_native_pending'
    assert routed.vias == board.vias  # No redundant or invented transition.
    assert routed.tracks[:len(board.tracks)] == board.tracks
    assert {t.layer for t in routed.tracks[len(board.tracks):]} == {CopperLayer.BACK}
    graph = explicit_copper_connectivity(routed, include_internal_connections=False)
    roots = {graph.roots[graph.pad_nodes[p][0]] for p in board.nets[0].pads}
    assert len(roots) == 1
    assert physical_board_digest(board) == digest
    assert routed.placements == board.placements and routed.copper_keepouts == board.copper_keepouts
    assert not any(z.id.startswith('power-area-contact:') for z in routed.zones)
    observation = plan.report()['contact_observations'][0]
    assert observation['status'] == 'joined_existing_component'
    assert observation['terminals'] == [str(PadReference('LOAD', '1'))]
    assert observation['layer'] == CopperLayer.INTERNAL_2.value
    assert observation['bridge_layer'] == CopperLayer.BACK.value
    assert observation['expanded_states'] <= options.branch_search_states
    assert plan.report() == plan_power_areas(board, options).report()


@pytest.mark.parametrize('policy', ['disabled', 'forbidden_option_layer', 'forbidden_rule_layer'])
def test_mixed_layer_join_obeys_both_work_policy_and_source_layer_rules(policy):
    board, options = fixture()
    if policy == 'disabled':
        options = replace(options, maximum_branch_layers=0)
    elif policy == 'forbidden_option_layer':
        options = replace(options, allowed_layers=(CopperLayer.INTERNAL_2,))
    else:
        board = replace(board, net_routing_rules=(replace(board.net_routing_rules[0],
            allowed_layers=(CopperLayer.FRONT, CopperLayer.INTERNAL_2)),))
    plan = plan_power_areas(board, options)
    assert not plan.selected
    assert apply_power_area_plan(board, plan)[0] is board
    observations = plan.report()['contact_observations']
    assert any(str(PadReference('LOAD', '1')) in o['terminals']
               and o['status'] == 'contact_layer_unreachable' for o in observations)


def test_blocked_backside_bridge_reports_terminal_and_exact_obstacle():
    board, options = fixture()
    wall = TrackSegment('WALL', Point.mm(9, 0), Point.mm(9, 10), 250000, CopperLayer.BACK)
    board = replace(board, nets=(*board.nets, PhysicalNet('WALL', ())), tracks=(*board.tracks, wall))
    plan = plan_power_areas(board, options)
    assert not plan.selected
    assert apply_power_area_plan(board, plan)[0] is board
    observations = plan.contact_observations
    assert observations and observations[-1].status == 'contact_layer_unreachable'
    assert observations[-1].sampled_blockers
    assert sum(o.expanded_states or 0 for o in observations) <= options.branch_search_states
    assert board.tracks[-1] == wall


def test_zone_outline_cannot_supply_a_missing_transition():
    board, options = fixture()
    outline = PolygonWithHoles(PolygonRing(board.outline.vertices))
    board = replace(board, vias=(), tracks=(),
        copper_keepouts=(CopperKeepout('no-drills', (CopperLayer.FRONT,), outline,
                                      block_tracks=False, block_zones=False),),
        zones=(CopperZone('intent-only', 'RAIL', (CopperLayer.INTERNAL_2,), outline),))
    plan = plan_power_areas(board, options)
    assert not plan.selected and not plan.candidates
    assert apply_power_area_plan(board, plan)[0] is board


def test_missing_roots_share_one_maze_allowance_until_real_graph_growth(monkeypatch):
    import pcbir.power_geometry as geometry
    board, options = fixture()
    other = replace(board.placements[1], reference='OTHER', position=Point.mm(14, 4))
    pads = (*board.nets[0].pads, PadReference('OTHER', '1'))
    board = replace(board, placements=(*board.placements, other), nets=(replace(board.nets[0], pads=pads),),
        power_domains=(PhysicalPowerDomain('RAIL', pads, pads[:1]),))
    budgets = []
    def fail(*args, work, **kwargs):
        budget = args[7]
        budgets.append(budget)
        work.append(budget)
        return None
    monkeypatch.setattr(geometry, '_attachment_branch', fail)
    observations = []
    result = geometry._join_layer_contacts(board, geometry.power_obligations(board, options)[0],
        CopperLayer.INTERNAL_2, options, observations)
    assert result is board and len(observations) == 2
    assert sum(budgets) == options.branch_search_states
    assert Counter(board.vias) == Counter(result.vias)

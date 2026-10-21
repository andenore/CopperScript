from fractions import Fraction as F
from dataclasses import replace

import pytest

from pcbir.power_network import DCNetwork, ResistorEdge, solve_dc, strip_resistance


def test_parallel_paths_and_shared_feed_match_hand_solution():
    network = DCNetwork((ResistorEdge('feed', 's', 'j', F(1, 10)),
                         ResistorEdge('a', 'j', 'load', F(1, 5)),
                         ResistorEdge('b', 'j', 'load', F(1, 5)),
                         ResistorEdge('other', 'j', 'second', F(1, 2))),
                        {'s': F(33, 10)}, {'load': F(1), 'second': F(1, 2)})
    result = solve_dc(network)
    assert result.status == 'solved'
    assert result.voltages_v == pytest.approx({'s': 3.3, 'j': 3.15, 'load': 3.05, 'second': 2.9})
    assert result.edge_currents_a == pytest.approx({'feed': 1.5, 'a': .5, 'b': .5, 'other': .5})
    assert result.source_currents_a['s'] == pytest.approx(1.5)
    assert result.loss_w == pytest.approx(.45)
    assert result.maximum_kcl_residual_a <= 1e-9
    assert network.edges[0].resistance_ohms == F(1, 10)


def test_supply_and_return_resistance_both_reduce_load_voltage():
    network = DCNetwork((ResistorEdge('power', 's', 'p', F(1, 5)),
                         ResistorEdge('return', 'g', 'r', F(1, 10))),
                        {'s': F(3), 'g': F(0)}, {'p': F(2), 'r': F(-2)})
    result = solve_dc(network)
    assert result.status == 'solved'
    assert result.voltages_v['p'] - result.voltages_v['r'] == pytest.approx(2.4)
    assert sum(result.source_currents_a.values()) == pytest.approx(0)
    assert result.loss_w == pytest.approx(1.2)


def test_floating_zero_load_island_is_not_certified():
    result = solve_dc(DCNetwork((ResistorEdge('island', 'a', 'b', F(1)),), {'s': F(0)}, {}))
    assert result.status == 'missing_voltage_boundary'
    assert result.unresolved_nodes == ('a', 'b')
    assert not result.voltages_v and result.loss_w is None


def test_solver_budget_and_reordering():
    network = DCNetwork(tuple(ResistorEdge(str(i), str(i), str(i+1), F(i+1, 10)) for i in range(8)),
                        {'0': F(5)}, {'8': F(1)})
    assert solve_dc(network, maximum_nodes=4).status == 'node_budget_exhausted'
    limited = solve_dc(network, maximum_iterations=1)
    assert limited.status == 'iteration_budget_exhausted' and not limited.voltages_v
    a, b = solve_dc(network), solve_dc(replace(network, edges=tuple(reversed(network.edges))))
    assert a.status == b.status == 'solved'
    assert a.voltages_v == pytest.approx(b.voltages_v)
    assert a.voltages_v['8'] == pytest.approx(1.4)


def test_multiple_sources_and_unloaded_boundary():
    network = DCNetwork((ResistorEdge('a', 's1', 'load', F(1)), ResistorEdge('b', 's2', 'load', F(1))),
                        {'s1': F(3), 's2': F(4)}, {'load': F(1)})
    result = solve_dc(network)
    assert result.voltages_v['load'] == pytest.approx(3)
    assert result.source_currents_a == pytest.approx({'s1': 0, 's2': 1})
    assert solve_dc(DCNetwork((), {'source': F(1)}, {})).status == 'solved'


@pytest.mark.parametrize('bad', [0, -1, float('nan'), float('inf'), .1, True, F(1, 10**400)])
def test_invalid_resistance_rejected(bad):
    with pytest.raises(ValueError):
        ResistorEdge('e', 's', 'l', bad)


def test_exact_geometry_resistance_and_width_scaling():
    parameters = dict(length_nm=10_000_000, width_nm=1_000_000, thickness_nm=35_000,
                      resistivity_ohm_m=F(168, 10**10))
    resistance = strip_resistance(**parameters)
    assert resistance == F(3, 625)
    assert strip_resistance(**{**parameters, 'width_nm': 2_000_000}) == resistance/2


def test_duplicate_edges_rejected_but_parallel_ids_allowed():
    edge = ResistorEdge('e', 's', 'l', F(1))
    with pytest.raises(ValueError, match='unique'):
        DCNetwork((edge, edge), {'s': 1}, {'l': 1})
    result = solve_dc(DCNetwork((edge, replace(edge, id='parallel')), {'s': 1}, {'l': 1}))
    assert result.voltages_v['l'] == pytest.approx(.5)

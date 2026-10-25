from copy import deepcopy

import pytest

from pcbir.rf_layout_review import layout_inventory


def rectangle(x1, y1, x2, y2):
    return {'outer': [[x1, y1], [x2, y1], [x2, y2], [x1, y2]], 'holes': []}


def native():
    return {'layers': ['F.Cu', 'In1.Cu', 'B.Cu'],
        'tracks': [{'net': 'RF', 'start': [0, 0], 'end': [3000, 0], 'width_nm': 100, 'layer': 'F.Cu'},
                   {'net': 'CLK', 'start': [0, 500], 'end': [3000, 500], 'width_nm': 100, 'layer': 'F.Cu'}],
        'fills': [{'net': 'GND', 'layer': layer, **rectangle(-1000, 100, 4000, 3000)} for layer in ['F.Cu', 'In1.Cu']],
        'vias': [{'net': 'GND', 'position': [1000, 1000], 'size_nm': 200, 'drill_nm': 100,
                  'via_type': 'through', 'from_layer': 'F.Cu', 'to_layer': 'B.Cu'}],
        'lands': [{'component': 'J', 'pad': '2', 'net': 'GND', 'position': [0, 1000], 'size_nm': [400, 400],
                   'copper': {'F.Cu': [rectangle(-200, 800, 200, 1200)]}},
                  {'component': 'J', 'pad': '2', 'net': 'GND', 'position': [3000, 1000], 'size_nm': [400, 400],
                   'copper': {'F.Cu': [rectangle(2800, 800, 3200, 1200)]}}]}


def inventory(value):
    return layout_inventory(value, signal_net='RF', ground_net='GND', signal_layer='F.Cu',
                            reference_layer='In1.Cu', port_component='J', interference_nets=['CLK'])


def test_connected_returns_preserve_duplicate_physical_ground_lands_not_rf_pass():
    result = inventory(native())
    assert result['status'] == 'incomplete' and not result['qualified_release']
    assert len(result['port_ground_returns']) == 2
    assert all(p['modeled_spanning_return_via_count'] == 1 for p in result['port_ground_returns'])
    assert result['port_ground_returns'][0]['nearest_modeled_return_vias'][0]['pad_center_to_via_center_nm'] == 1000
    assert result['interference_projection_inventory'][0]['projected_edge_gap_nm'] == 400


def test_nearby_but_disconnected_ground_via_is_not_a_return_candidate():
    value = native()
    value['fills'][0].update(rectangle(-1000, 100, 300, 3000))
    assert inventory(value)['port_ground_returns'][0]['modeled_spanning_return_via_count'] == 0


def test_plane_hole_and_wrong_net_remove_candidate():
    value = native()
    value['fills'][1]['holes'] = [rectangle(800, 800, 1200, 1200)['outer']]
    # Do not let the via annulus alone masquerade as a reference-plane join:
    # the return-screen implementation must check real filled reference copper.
    assert inventory(value)['port_ground_returns'][0]['modeled_spanning_return_via_count'] == 0
    value['vias'][0]['net'] = 'VCC'
    assert inventory(value)['port_ground_returns'][0]['modeled_spanning_return_via_count'] == 0


def test_missing_land_geometry_and_nonadjacent_reference_rejected():
    value = native()
    value.pop('lands')
    with pytest.raises(ValueError, match='land polygons'):
        inventory(value)
    value = native()
    value['layers'].insert(1, 'In0.Cu')
    with pytest.raises(ValueError, match='adjacent'):
        inventory(value)


def test_no_mutation():
    value = native()
    before = deepcopy(value)
    inventory(value)
    assert value == before

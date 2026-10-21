import pytest
from pcbir.rf_cross_section import sample_coplanar_cross_sections


def native():
    return {'tracks':[{'net':'RF', 'layer':'F.Cu', 'width_nm':142_000,
                      'start':[1_000_000,0], 'end':[3_000_000,0]}],
            'vias':[], 'fills':[{'net':'GND', 'layer':'F.Cu', 'holes':[],
                'outer':[[0,272_000],[4_000_000,272_000],[4_000_000,2_000_000],[0,2_000_000]]},
                {'net':'GND', 'layer':'F.Cu', 'holes':[],
                'outer':[[0,-2_000_000],[4_000_000,-2_000_000],[4_000_000,-272_000],[0,-272_000]]}]}


def probe(value, **kw):
    result = sample_coplanar_cross_sections(value, identifier='RF', net='RF', ground_net='GND',
        signal_layer='F.Cu', expected_width_nm=142_000, expected_gap_nm=201_000,
        gap_tolerance_nm=1000, sample_pitch_nm=100_000, probe_length_nm=1_000_000, **kw)
    return {'status':result['status'], **result['metrics']}


def test_matching_samples_never_become_impedance_approval():
    result = probe(native())
    assert result['status'] == 'incomplete'
    assert result['geometry_matches_model_at_samples']
    assert len(result['samples']) == 20
    assert all(s['gaps_nm'] == [201_000,201_000] for s in result['samples'])


def test_missing_side_and_incorrect_width_are_visible():
    value = native()
    value['fills'].pop()
    result = probe(value)
    assert not result['geometry_matches_model_at_samples']
    assert len(result['missing_ground_sample_indices']) == 20
    value = native()
    value['tracks'][0]['width_nm'] = 200_000
    assert probe(value)['unsupported_track_indices'] == [0]


def test_wrong_gap_and_signal_vias_do_not_match_model():
    value = native()
    for p in value['fills'][0]['outer']:
        p[1] += 10_000
    assert len(probe(value)['out_of_bounds_sample_indices']) == 20
    value = native()
    value['vias'] = [{'net':'RF'}]
    assert not probe(value)['geometry_matches_model_at_samples']


def test_exclusions_are_explicit_and_no_samples_cannot_pass():
    exclusion = {'center_nm':[2_000_000,0], 'radius_nm':2_000_000, 'reason':'fixture launch area'}
    result = probe(native(), exclusions=[exclusion])
    assert result['excluded_sample_count'] == 20
    assert result['exclusions'] == [exclusion]
    assert not result['geometry_matches_model_at_samples']
    assert result['status'] == 'incomplete'


def test_invalid_native_polygon_is_not_healed():
    value = native()
    value['fills'][0]['outer'] = [[0,0],[100,100],[0,100],[100,0]]
    assert probe(value)['status'] == 'incomplete'
    assert 'geometry_matches_model_at_samples' not in probe(value)


def test_sampling_measures_normal_to_a_diagonal_track():
    value = native()
    value['tracks'][0].update(start=[1_000_000,1_000_000],end=[2_000_000,2_000_000])
    value['fills'][0]['outer'] = [[0,384_665],[3_000_000,3_384_665],
                                 [3_000_000,4_000_000],[0,4_000_000]]
    value['fills'][1]['outer'] = [[384_665,0],[3_384_665,3_000_000],
                                 [4_000_000,3_000_000],[4_000_000,0]]
    result = probe(value)
    assert result['geometry_matches_model_at_samples']
    assert result['status'] == 'incomplete'


def test_non_integer_geometry_parameters_are_rejected():
    with pytest.raises(ValueError):
        probe(native(), exclusions=[{'center_nm':[0,0], 'radius_nm':True, 'reason':'bad'}])

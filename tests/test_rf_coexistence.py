import pytest
from pcbir.rf_coexistence import isolation_budget


def budget(**changes):
    return isolation_budget(**(dict(transmitter_max_mdbm=25700,
        receiver_immunity_mdbm=-25000, guard_mdb=3000) | changes))


def test_unknown_isolation_is_not_qualified_by_listed_saw_rejection():
    result = budget(filter_rejection_min_mdb=34000)
    assert result['required_total_attenuation_mdb'] == 53700
    assert result['required_isolation_mdb'] == 19700
    assert result['margin_mdb'] is None
    assert result['status'] == 'incomplete'
    assert not result['qualified_release']


def test_absent_filter_and_insufficient_isolation():
    result = budget(isolation_min_mdb=20000, filter_rejection_min_mdb=0)
    assert result['predicted_receiver_input_mdbm'] == 5700
    assert result['margin_mdb'] == -33700
    assert result['status'] == 'insufficient_attenuation'


def test_boundary_and_favourable_screens_are_not_release_approval():
    result = budget(isolation_min_mdb=19700, filter_rejection_min_mdb=34000)
    assert result['margin_mdb'] == 0
    assert result['status'] == 'within_bound'
    assert not result['qualified_release']


@pytest.mark.parametrize('changes', [dict(guard_mdb=-1), dict(isolation_min_mdb=-1),
    dict(filter_rejection_min_mdb=-1), dict(transmitter_max_mdbm=True),
    dict(transmitter_max_mdbm=25.7), dict(receiver_immunity_mdbm=None)])
def test_invalid_or_ambiguous_bounds_rejected(changes):
    with pytest.raises(ValueError):
        budget(**changes)

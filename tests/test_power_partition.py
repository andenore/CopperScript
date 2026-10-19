from dataclasses import replace
import shutil
import pytest

from pcbir import PhysicalNet, PhysicalPowerDomain, PadReference, Point
from pcbir.power_geometry import (PowerAreaOptions, plan_power_areas, apply_power_area_plan,
                                  ExternalPowerBoundary)
from pcbir.power_extract import assess_power_plan_fill
from pcbir.plane_verify import verify_filled_planes
from test_power_geometry import board, zone


def fixture(*, split=False):
    b = board()
    supply = (PadReference('H1', '1'), PadReference('H2', '1'))
    poses = (replace(b.placements[0], reference='H1', position=Point.mm(3, 8 if not split else 5)),
             replace(b.placements[1], reference='H2', position=Point.mm(15, 8 if not split else 5)))
    if split:
        b = replace(b, placements=(replace(b.placements[0], position=Point.mm(9, '1.5')),
                                    replace(b.placements[1], position=Point.mm(9, '8.5'))))
    b = replace(b, placements=(*b.placements, *poses), nets=(*b.nets, PhysicalNet('HOST', supply)),
        power_domains=(*b.power_domains, PhysicalPowerDomain('HOST', supply, supply[:1])),
        zones=(zone('host-power', 'HOST', 1, 18),))
    options = PowerAreaOptions(nets=('RAIL',), candidates_per_domain=1, partitionable_zone_ids=('host-power',))
    return b, options


def test_partition_requires_explicit_permission_and_never_consumes_return_or_reserved_planes():
    b, options = fixture()
    assert not plan_power_areas(b, replace(options, partitionable_zone_ids=(), search_states=10)).selected
    with pytest.raises(ValueError, match='non-reference'):
        plan_power_areas(replace(b, zones=(replace(b.zones[0], reserve_routing=True),)), options)
    with pytest.raises(ValueError, match='non-reference'):
        plan_power_areas(b, replace(options, external_boundaries=(
            ExternalPowerBoundary('HOST', PadReference('H1', '1'), 'return', 'declared return'),)))


@pytest.mark.parametrize('split', (False, True))
def test_native_partition_checks_every_original_contact_group(tmp_path, split):
    cli = shutil.which('kicad-cli')
    if cli is None:
        pytest.skip('KiCad CLI unavailable')
    b, options = fixture(split=split)
    plan = plan_power_areas(b, options)
    assert plan.partitioned_zone_ids == ('host-power',)
    routed, status = apply_power_area_plan(b, plan)
    assert status == 'contact_routed_native_pending' and routed.zones[0] is b.zones[0]
    before = verify_filled_planes(b, kicad_cli=cli, artifact_directory=tmp_path/'before')
    after = verify_filled_planes(routed, kicad_cli=cli, artifact_directory=tmp_path/'after')
    assert before.matches(b) and after.matches(routed)
    unbound = assess_power_plan_fill(routed, plan, after, options)
    assert unbound['status'] == 'unresolved'
    assert unbound['partition_check']['status'] == 'fresh_baseline_required'
    checked = assess_power_plan_fill(routed, plan, after, options, baseline_evidence=before)
    if split:
        assert checked['status'] == 'unresolved'
        assert checked['partition_check']['lost_groups']
    else:
        assert checked['status'] == 'native_connected'
        assert checked['partition_check']['status'] == 'preserved'

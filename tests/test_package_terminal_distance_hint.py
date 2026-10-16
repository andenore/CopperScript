"""Source-position hints preserve ordinary source-legal move authority."""
from dataclasses import replace
from itertools import islice

import pytest

from pcbir import Point
from pcbir.package_access import _source_capacitor_pin_distance
from test_package_probe_fairness import family_scene, trials, assert_source_protected


@pytest.mark.parametrize('renamed', (False, True))
def test_tied_declared_targets_prefer_actual_terminal_positions_not_names(renamed):
    baseline, caps, other, package, _ = family_scene(renamed)
    baseline.pending_pads = (next(link.target for link in baseline.source.decoupling_links
                                  if link.capacitor.component == caps[0]),)
    # All four associations have the EXACT same target and existing ranking
    # tier. Move the lexically last cap's source pose closest to that target.
    xs = dict(zip(caps, (55, 25, 40, 10)))
    baseline.source = replace(baseline.source, placements=tuple(
        replace(pose, position=Point.mm(xs[pose.reference], 35))
        if pose.reference in xs else pose for pose in baseline.source.placements))
    result = list(islice(trials(baseline), 4))
    assert [ref for ref, _ in result] == [caps[3], caps[1], caps[2], caps[0]]
    assert other not in {ref for ref, _ in result}
    assert_source_protected(baseline, result)


def test_hint_is_exact_integer_transformed_terminal_center_distance():
    baseline, caps, *_ = family_scene()
    board = baseline.source
    # The cap at(10,35) has actual terminals(9.75,35)/(10.25,35).
    measured = _source_capacitor_pin_distance(board, caps[0], [Point.mm(10, 10)])
    assert measured == 250_000**2 + 25_000_000**2
    assert isinstance(measured, int)
    assert _source_capacitor_pin_distance(board, 'ABSENT_OWNER', [Point.mm(10, 10)]) is None
    assert _source_capacitor_pin_distance(board, caps[0], []) is None


def test_fixed_owner_remains_fixed_and_normal_legal_filter_still_controls():
    baseline, caps, *_ = family_scene()
    baseline.pending_pads = (next(link.target for link in baseline.source.decoupling_links
                                  if link.capacitor.component == caps[0]),)
    old = next(p for p in baseline.source.placements if p.reference == caps[0])
    baseline.source = replace(baseline.source, placement_rules=tuple(
        replace(rule, fixed_position=old.position, fixed_rotation_degrees=old.rotation_degrees)
        if rule.reference == caps[0] else rule for rule in baseline.source.placement_rules))
    result = list(islice(trials(baseline), 3))
    assert caps[0] not in {ref for ref, _ in result}
    assert all(next(p for p in candidate.placements if p.reference == caps[0]) == old
               for _, candidate in result)
    assert_source_protected(baseline, result)


def test_rotated_back_pose_uses_exact_source_terminals():
    baseline,caps,*_=family_scene()
    source=replace(baseline.source,placements=tuple(
        replace(p,rotation_degrees=90) if p.reference==caps[0] else p
        for p in baseline.source.placements))
    assert _source_capacitor_pin_distance(source,caps[0],[Point.mm(10,10)])==24_750_000**2


def test_unavailable_optional_hint_preserves_complete_old_tier(monkeypatch):
    import pcbir.package_access as access
    baseline,caps,*_=family_scene()
    baseline.pending_pads=(next(link.target for link in baseline.source.decoupling_links
        if link.capacitor.component==caps[0]),)
    xs=dict(zip(caps,(55,25,40,10)))
    baseline.source=replace(baseline.source,placements=tuple(
        replace(p,position=Point.mm(xs[p.reference],35)) if p.reference in xs else p
        for p in baseline.source.placements))
    real=access._source_capacitor_pin_distance
    # Adversarial unavailable optional measurement only. Physical source,
    # clearance, whole-unit legality and move generation remain real.
    monkeypatch.setattr(access,'_source_capacitor_pin_distance',
        lambda board,ref,points:None if ref==caps[-1] else real(board,ref,points))
    result=list(islice(trials(baseline),4))
    assert [ref for ref,_ in result]==list(caps)
    assert_source_protected(baseline,result)

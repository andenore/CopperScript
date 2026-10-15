"""Bounded probe fairness with real whole-unit legal geometry.

These tests exercise legal proposal generation without routing or placement solves.
"""
from dataclasses import replace
from itertools import islice
from types import SimpleNamespace

import pytest

from pcbir import (BoardOutline, ComponentPlacementRule, FootprintPad, PadReference,
                   PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement,
                   PlacementPlannerOptions, Point, Size, nm_from_mm)
from pcbir.physical import BoardSide, DecouplingLink, PlacementRegion
from pcbir.package_access import PackagePatternTrial, package_placement_trials
from pcbir.placement import placement_solution_is_legal


def family_scene(renamed=False):
    pkg_a, pkg_b = (('SENSOR_91', 'CONTROLLER_14') if renamed else ('PKG_A', 'PKG_B'))
    caps_a = tuple(('FILTER_' if renamed else 'CAP_A_') + str(i) for i in range(4))
    cap_b = 'BYPASS_701' if renamed else 'CAP_B'
    package = PhysicalFootprint('linear-four', tuple(
        FootprintPad(str(i+1), Point.mm(i, 0), Size.mm('0.2', '0.2'))
        for i in range(4)), Size.mm(8, 3))
    cap = PhysicalFootprint('two-land', (
        FootprintPad('1', Point.mm('-.25', 0), Size.mm('.3', '.3')),
        FootprintPad('2', Point.mm('.25', 0), Size.mm('.3', '.3'))), Size.mm(1, '.5'))
    poses = (Placement(pkg_a, package.name, Point.mm(10, 10)),
             Placement(pkg_b, package.name, Point.mm(80, 10)),
             *(Placement(ref, cap.name, Point.mm(x, 35), side=BoardSide.BACK)
               for ref, x in zip((*caps_a, cap_b), (10, 25, 40, 55, 75))))
    links = tuple(DecouplingLink(PadReference(ref, '1'), PadReference(pkg_a, '1'),
                PadReference(ref, '2'), 'RAIL', 'GND') for ref in caps_a)
    links += (DecouplingLink(PadReference(cap_b, '1'), PadReference(pkg_b, '4'),
                            PadReference(cap_b, '2'), 'RAIL', 'GND'),)
    board = PhysicalBoard('two-families', BoardOutline.rectangle(100, 60),
        {package.name: package, cap.name: cap}, poses,
        (PhysicalNet('RAIL', tuple(PadReference(ref, '1') for ref in (*caps_a, cap_b))
                     + (PadReference(pkg_a, '1'), PadReference(pkg_b, '1'), PadReference(pkg_b, '4'))),
         PhysicalNet('GND', tuple(PadReference(ref, '2') for ref in (*caps_a, cap_b)))),
        placement_rules=tuple(ComponentPlacementRule(p.reference,
            fixed_position=p.position, fixed_rotation_degrees=0, allowed_orientations=(0,))
            for p in poses[:2]) + tuple(ComponentPlacementRule(ref,
            allowed_orientations=(0, 90), side=BoardSide.BACK) for ref in (*caps_a, cap_b)),
        decoupling_links=links)
    # Only fields read by the production generator are provided. No fabricated
    # success flags, routing result or disposable copper is installed.
    baseline = SimpleNamespace(source=board,
        pending_pads=(PadReference(pkg_a, '1'), PadReference(pkg_b, '1')),
        failed_critical_nets=frozenset(), pattern_trials=())
    return baseline, caps_a, cap_b, pkg_a, pkg_b


def trials(baseline, options=None):
    return package_placement_trials(baseline, options or PlacementPlannerOptions(), nm_from_mm('.5'))


def assert_source_protected(baseline, generated):
    before = baseline.source
    for _, candidate in generated:
        assert placement_solution_is_legal(candidate,
            {p.reference: p for p in candidate.placements}, PlacementPlannerOptions())
        assert candidate.placements[:2] == before.placements[:2]
        assert replace(candidate, placements=before.placements) == before
        assert all(p.side is BoardSide.BACK and p.rotation_degrees in (0, 90)
                   for p in candidate.placements[2:])


@pytest.mark.parametrize('renamed', (False, True))
def test_distinct_source_package_gets_legal_slot_before_second_nearby_dependency(renamed):
    baseline, a, b, *_ = family_scene(renamed)
    result = list(islice(trials(baseline), 4))
    # Old distance/reference prefix is four A owners: all their target pins
    # coincide with a failed pad, while B's target is3mm from its failed pad.
    assert [ref for ref, _ in result] == [min(a), b, sorted(a)[1], b]
    assert_source_protected(baseline, result)


def test_fairness_counts_legal_candidates_not_just_first_displacement_attempt():
    baseline, a, b, *_ = family_scene()
    # B's east move is outside its unchanged region, west is legal. A's group
    # must not take another yielded slot merely because B's first move fails.
    region = PlacementRegion('west-bay', BoardOutline(tuple(Point.mm(x, y) for x, y in
        ((73.75, 34), (75.75, 34), (75.75, 36), (73.75, 36)))), side=BoardSide.BACK)
    board = replace(baseline.source, regions=(region,), placement_rules=tuple(
        replace(r, region=region.name) if r.reference == b else r
        for r in baseline.source.placement_rules))
    baseline = SimpleNamespace(**{**vars(baseline), 'source': board})
    result = list(islice(trials(baseline), 2))
    assert [ref for ref, _ in result] == [min(a), b]
    moved = next(p for p in result[1][1].placements if p.reference == b)
    assert moved.position == Point.mm(74.5, 35)
    assert_source_protected(baseline, result)


def test_single_source_target_keeps_existing_within_family_rank_and_move_rounds():
    baseline, a, b, pkg_a, _ = family_scene()
    baseline.pending_pads = (PadReference(pkg_a, '1'),)
    result = list(islice(trials(baseline), 5))
    assert [ref for ref, _ in result] == [*sorted(a), min(a)]
    old = {p.reference: p for p in baseline.source.placements}
    for ref, board in result[:4]:
        pose = next(p for p in board.placements if p.reference == ref)
        assert pose.position == Point(old[ref].position.x_nm + 500000, old[ref].position.y_nm)
    assert b not in {ref for ref, _ in result}


def test_exhausted_legal_cohort_does_not_consume_slots_or_stop_other_targets():
    baseline, a, b, *_ = family_scene()
    # The original body fits; every existing translation and 90-degree turn
    # exceeds this source region. Exhausting it cannot buy another move domain.
    region = PlacementRegion('closed-bay', BoardOutline(tuple(Point.mm(x, y) for x, y in
        ((74.45, 34.70), (75.55, 34.70), (75.55, 35.30), (74.45, 35.30)))),
        side=BoardSide.BACK)
    baseline.source = replace(baseline.source, regions=(region,), placement_rules=tuple(
        replace(rule, region=region.name) if rule.reference == b else rule
        for rule in baseline.source.placement_rules))
    assert placement_solution_is_legal(baseline.source,
        {pose.reference: pose for pose in baseline.source.placements}, PlacementPlannerOptions())
    generated = list(trials(baseline))
    assert generated and {ref for ref, _ in generated} == set(a)
    assert len(generated) <= 4 * 25
    assert_source_protected(baseline, generated)


def test_direct_failed_cap_owner_is_not_relabelled_as_its_dependency_target():
    baseline, a, b, *_ = family_scene()
    baseline.pending_pads += (PadReference(a[0], '1'),)
    result = list(islice(trials(baseline), 3))
    assert [ref for ref, _ in result] == [a[0], sorted(a)[1], b]
    assert_source_protected(baseline, result)


def test_measured_ordinary_hint_has_own_group_without_installing_discarded_geometry():
    baseline, a, b, *_ = family_scene()
    # An otherwise unrelated movable owner with a measured lost ordinary pad.
    ref = 'MEASURED_OWNER'
    board = baseline.source
    terminal = replace(board.placements[-1], reference=ref, position=Point.mm(90, 45))
    baseline.source = replace(board, placements=(*board.placements, terminal),
        placement_rules=(*board.placement_rules, ComponentPlacementRule(ref,
                         allowed_orientations=(0,), side=BoardSide.BACK)))
    baseline.pattern_trials = (PackagePatternTrial(1, 'critical_first',
        (PadReference(ref, '1'),), (), 'lost_ordinary_access'),)
    result = list(islice(trials(baseline), 3))
    assert {owner for owner, _ in result} == {min(a), b, ref}
    assert_source_protected(baseline, result)


def test_exact_bypass_hints_use_source_targets_and_return_pad_hint_is_ignored():
    baseline, a, b, *_ = family_scene()
    baseline.pending_pads = ()
    baseline.pattern_trials = (PackagePatternTrial(1, 'partial_bypass', (), (),
        'lost_bypass_connection', failed_bypass_capacitors=(
            PadReference(a[0], '1'), PadReference(a[1], '1'), PadReference(b, '1'))),)
    assert [ref for ref, _ in islice(trials(baseline), 2)] == [a[0], b]
    baseline.pattern_trials = (replace(baseline.pattern_trials[0],
        failed_bypass_capacitors=(PadReference(a[0], '2'),)),)
    assert list(trials(baseline)) == []


def test_fixed_cohort_cannot_unlock_and_unexhausted_cohort_continues_with_original_bound():
    baseline, a, b, *_ = family_scene()
    options = PlacementPlannerOptions(fixed_references={b})
    generated = list(trials(baseline, options))
    assert generated and {ref for ref, _ in generated} == set(a)
    assert len(generated) <= 4 * 25
    assert all(next(p for p in board.placements if p.reference == b)
               == baseline.source.placements[-1] for _, board in generated)
    assert_source_protected(baseline, generated)

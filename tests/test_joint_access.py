"""A coupled contact moves without losing its source connection or supply."""
from dataclasses import replace
from pathlib import Path
import shutil
import pytest

from pcbir.copper_ownership import CopperUse, begin_copper_ownership, append_copper_ownership, prepare_copper_trial
from pcbir.drc import physical_board_digest, explicit_copper_connectivity
from pcbir.escape_assignment import EscapeAssignmentOptions
from pcbir.joint_access import AccessBundle, AccessRequirement, JointAccessOptions, allocate_joint_access
from pcbir.physical import (BoardOutline, CopperLayer, DesignRules, FootprintPad, PadKind,
    PadReference, PhysicalBoard, PhysicalFootprint, PhysicalNet, Placement, Point, Size, TrackSegment, Via)
from pcbir.plane_verify import verify_filled_planes


def scene(renamed=False):
    names = ('sig-x', 'return-y', 'supply-z') if renamed else ('DATA', 'GND', 'V33')
    signal, ground, supply = names
    smd = PhysicalFootprint('smd', (FootprintPad('1', Point(0, 0), Size.mm('.3', '.3')),), Size.mm(1, 1))
    tht = PhysicalFootprint('tht', (FootprintPad('1', Point(0, 0), Size.mm('.8', '.8'),
        kind=PadKind.THROUGH_HOLE, drill=Size.mm('.3', '.3')),), Size.mm(1, 1))
    poses = tuple(p for y, role in ((5, 'S'), (7, 'G'), (9, 'V')) for p in (
        Placement(role, 'smd', Point.mm(5, y)), Placement('J'+role, 'tht', Point.mm(11, y))))
    source = PhysicalBoard('joint', BoardOutline.rectangle(16, 14), {'smd': smd, 'tht': tht}, poses,
        tuple(PhysicalNet(n, (PadReference(role, '1'), PadReference('J'+role, '1')))
              for n, role in zip(names, ('S', 'G', 'V'))),
        rules=DesignRules(minimum_track_width_nm=150000, default_track_width_nm=250000,
                         default_via_size_nm=600000, default_via_drill_nm=300000))
    def path(net, coordinates, layer):
        return tuple(TrackSegment(net, Point.mm(*a), Point.mm(*b), 250000, layer)
                     for a, b in zip(coordinates, coordinates[1:]))
    bad_tracks = (*path(ground, ((5, 7), (7, 5)), CopperLayer.FRONT),
                  *path(ground, ((7, 5), (9, 7), (11, 7)), CopperLayer.BACK))
    good_tracks = (*path(ground, ((5, 7), (7, 7)), CopperLayer.FRONT),
                   *path(ground, ((7, 7), (11, 7)), CopperLayer.BACK))
    supply_tracks = (*path(supply, ((5, 9), (7, 9)), CopperLayer.FRONT),
                     *path(supply, ((7, 9), (11, 9)), CopperLayer.BACK))
    supply_via = Via(supply, Point.mm(7, 9), 600000, 300000)
    old_via, good_via = (Via(ground, Point.mm(7, y), 600000, 300000) for y in (5, 7))
    original = replace(source, tracks=(*bad_tracks, *supply_tracks), vias=(old_via, supply_via))
    ground_pads = (PadReference('G', '1'), PadReference('JG', '1'))
    supply_pads = (PadReference('V', '1'), PadReference('JV', '1'))
    ledger = append_copper_ownership(begin_copper_ownership(source), source, original, 'contacts',
        uses=(CopperUse('return-connection', 'connection', ground_pads, bad_tracks, (old_via,)),
              CopperUse('supply-connection', 'connection', supply_pads, supply_tracks, (supply_via,))), trial_mutable=True)
    frame = prepare_copper_trial(original, ledger, ('via:0',))
    requirements = (AccessRequirement('return-connection', 'connection', ground, ground_pads),
        AccessRequirement('signal-port', 'signal', signal, (PadReference('S', '1'),), (CopperLayer.BACK,)),
        AccessRequirement('signal-connection', 'connection', signal, (PadReference('S', '1'), PadReference('JS', '1'))))
    digest = physical_board_digest(frame.held)
    sig_tracks = (*path(signal, ((5, 5), (9, 5)), CopperLayer.FRONT),
                  *path(signal, ((9, 5), (11, 5)), CopperLayer.BACK))
    signal_bundle = AccessBundle('signal', 'signal-family', digest, {'signal-port', 'signal-connection'},
        sig_tracks, (Via(signal, Point.mm(9, 5), 600000, 300000),), ((PadReference('S', '1'), Point.mm(9, 5)),))
    bundles = (AccessBundle('contact-0', 'return-family', digest, {'return-connection'}, bad_tracks, (old_via,)),
               AccessBundle('contact-1', 'return-family', digest, {'return-connection'}, good_tracks, (good_via,)), signal_bundle)
    return frame, requirements, bundles, names


@pytest.mark.parametrize('renamed', [False, True])
def test_joint_selection_moves_contact_and_preserves_return_supply_and_signal(renamed):
    frame, requirements, bundles, names = scene(renamed)
    blocked = allocate_joint_access(frame, requirements, (bundles[0], bundles[2]))
    assert blocked.status == 'no_compatible_assignment' and blocked.board is frame.original
    proposed = allocate_joint_access(frame, requirements, bundles)
    assert proposed.status == 'geometry_proposal'
    assert {c.id for c in proposed.selected} == {'contact-1', 'signal'}
    assert proposed == allocate_joint_access(frame, requirements, bundles)
    assert frame.original.vias[0] not in proposed.board.vias
    assert frame.held.tracks == proposed.board.tracks[:len(frame.held.tracks)]
    assert frame.original.vias[1] in proposed.board.vias  # same-net unrelated supply was never released
    graph = explicit_copper_connectivity(proposed.board, include_internal_connections=False)
    for net in proposed.board.nets:
        assert len({graph.roots[node] for p in net.pads for node in graph.pad_nodes[p]}) == 1


@pytest.mark.parametrize('bound', ['maximum_search_states', 'maximum_pair_checks', 'maximum_pair_queries'])
def test_exhausted_shared_work_returns_exact_original(bound):
    frame, requirements, bundles, _ = scene()
    # Both alternatives need exact checks, rather than allowing the good one
    # to pass the separated-bounds shortcut without spending an exact check.
    detour = (TrackSegment('GND', Point.mm(5, 7), Point.mm(6, 6), 250000, CopperLayer.FRONT),
              TrackSegment('GND', Point.mm(6, 6), Point.mm(7, 7), 250000, CopperLayer.FRONT), bundles[1].tracks[-1])
    bundles = (bundles[0], replace(bundles[1], tracks=detour), bundles[2])
    options = JointAccessOptions(assignment=EscapeAssignmentOptions(**{bound: 1}))
    result = allocate_joint_access(frame, requirements, bundles, options)
    assert result.status != 'geometry_proposal' and result.board is frame.original and not result.selected
    assert getattr(result, {'maximum_search_states':'search_states','maximum_pair_checks':'pair_checks','maximum_pair_queries':'pair_queries'}[bound]) <= 1


def test_missing_return_or_forged_source_requirement_is_rejected():
    frame, requirements, bundles, _ = scene()
    with pytest.raises(ValueError, match='omit'):
        allocate_joint_access(frame, requirements[1:], bundles[2:])
    with pytest.raises(ValueError, match='source identity'):
        allocate_joint_access(frame, (replace(requirements[0], terminals=(PadReference('S', '1'), PadReference('JS', '1'))), *requirements[1:]), bundles)
    invalid = replace(bundles[1], tracks=(), vias=())
    result = allocate_joint_access(frame, requirements, (invalid, bundles[2]))
    assert result.board is frame.original and ('contact-1', 'unverified_obligation') in result.assessments


def test_stale_frame_bundle_and_changed_inventory_reject_before_search():
    frame, requirements, bundles, _ = scene()
    with pytest.raises(ValueError, match='snapshot'):
        allocate_joint_access(frame, requirements, (replace(bundles[0], snapshot='old'), *bundles[1:]))
    with pytest.raises(ValueError, match='fresh'):
        allocate_joint_access(replace(frame, held=replace(frame.held, metadata={'x':'y'})), requirements, bundles)
    with pytest.raises(ValueError, match='fresh'):
        allocate_joint_access(replace(frame, ownership=replace(frame.ownership, frozen=True)), requirements, bundles)


def test_excluding_a_complete_assignment_does_not_accept_partial_geometry():
    frame, requirements, bundles, _ = scene()
    result = allocate_joint_access(frame, requirements, bundles, excluded_assignments=(('signal', 'contact-1'),))
    assert result.status == 'no_compatible_assignment' and result.board is frame.original
    with pytest.raises(ValueError, match='complete'):
        allocate_joint_access(frame, requirements, bundles, excluded_assignments=(('signal',),))


def test_candidate_and_family_limits_are_explicit():
    frame, requirements, bundles, _ = scene()
    limited = allocate_joint_access(frame, requirements, bundles, JointAccessOptions(maximum_candidates=2))
    assert limited.status == 'candidate_budget_exhausted' and limited.board is frame.original
    limited = allocate_joint_access(frame, requirements, bundles,
        JointAccessOptions(assignment=EscapeAssignmentOptions(maximum_cluster_pins=1)))
    assert limited.status == 'family_budget_exhausted' and limited.board is frame.original


def test_second_via_in_a_bundle_blocks_a_foreign_trace_on_its_other_layer():
    frame, requirements, bundles, _ = scene()
    extra = Via('GND', Point.mm(10, 5), 600000, 300000)
    contact = replace(bundles[1], vias=(*bundles[1].vias, extra))
    result = allocate_joint_access(frame, requirements, (contact, bundles[2]))
    assert ('contact-1', 'eligible_geometry') in result.assessments
    assert result.status == 'no_compatible_assignment' and result.board is frame.original


def test_bad_internal_drill_spacing_and_duplicate_vias_are_rejected():
    frame, requirements, bundles, _ = scene()
    bad = replace(bundles[1], vias=(*bundles[1].vias, Via('GND', Point.mm('7.1', 7), 600000, 300000)))
    result = allocate_joint_access(frame, requirements, (bad, bundles[2]))
    assert ('contact-1', 'physical_geometry_rejected') in result.assessments
    duplicate = replace(bundles[1], vias=(*bundles[1].vias, *bundles[1].vias))
    result = allocate_joint_access(frame, requirements, (duplicate, bundles[2]))
    assert ('contact-1', 'duplicate_copper_occurrences') in result.assessments


def test_one_physical_hole_can_be_shared_only_with_both_bundles_declaring_it():
    frame, requirements, bundles, _ = scene()
    ground, via = bundles[1], bundles[1].vias[0]
    alias = AccessRequirement('shared-port', 'signal', 'GND', (PadReference('JG', '1'),), (CopperLayer.BACK,))
    second = AccessBundle('alias', 'other-family', ground.snapshot, {'shared-port'},
        (ground.tracks[-1],), (via,), ((PadReference('JG', '1'), via.position),))
    rejected = allocate_joint_access(frame, (requirements[0], alias), (ground, second))
    assert rejected.status == 'no_compatible_assignment' and rejected.board is frame.original
    selected = allocate_joint_access(frame, (requirements[0], alias),
        (replace(ground, shared_vias={via}), replace(second, shared_vias={via})))
    assert selected.status == 'geometry_proposal'
    assert selected.board.vias.count(via) == 1
    assert frame.held.vias[0] in selected.board.vias


@pytest.mark.parametrize('defect', ['none', 'zone-hole', 'zone-keepout', 'wrong-layer'])
def test_contact_port_requires_real_prospective_zone_access(defect):
    from pcbir.physical import CopperZone, CopperKeepout, PolygonRing, PolygonWithHoles
    frame, requirements, bundles, _ = scene()
    outer = PolygonRing(tuple(Point.mm(x, y) for x, y in ((1, 1), (15, 1), (15, 13), (1, 13))))
    hole = PolygonRing(tuple(Point.mm(x, y) for x, y in ((6, 6), (8, 6), (8, 8), (6, 8))))
    zone = CopperZone('return-zone', 'GND', (CopperLayer.BACK,),
        PolygonWithHoles(outer, (hole,) if defect == 'zone-hole' else ()))
    keepouts = (CopperKeepout('no-fill', (CopperLayer.BACK,), PolygonWithHoles(hole),
        block_tracks=False, block_vias=False, block_zones=True),) if defect == 'zone-keepout' else ()
    source = replace(frame.original, tracks=(), vias=(), zones=(zone,), copper_keepouts=keepouts)
    original = replace(source, tracks=frame.original.tracks, vias=frame.original.vias)
    ledger = append_copper_ownership(begin_copper_ownership(source), source, original, 'contacts',
        uses=frame.ownership.uses, trial_mutable=True)
    fresh = prepare_copper_trial(original, ledger, ('via:0',))
    contact = AccessRequirement('return-port', 'contact', 'GND', (PadReference('G', '1'),),
        (CopperLayer.FRONT,) if defect == 'wrong-layer' else (CopperLayer.BACK,))
    updated = tuple(replace(b, snapshot=physical_board_digest(fresh.held)) for b in bundles[1:])
    good = replace(updated[0], covers=updated[0].covers | {contact.id},
        ports=((PadReference('G', '1'), updated[0].vias[0].position),))
    result = allocate_joint_access(fresh, (*requirements, contact), (good, updated[1]))
    if defect == 'none':
        assert result.status == 'geometry_proposal'
    else:
        assert result.board is original
        assert ('contact-1', 'unverified_obligation') in result.assessments


def test_native_joint_fixture_closes_all_three_nets(tmp_path):
    cli = shutil.which('kicad-cli')
    if not cli:
        pytest.skip('KiCad CLI unavailable')
    frame, requirements, bundles, _ = scene()
    result = allocate_joint_access(frame, requirements, bundles)
    assert result.status == 'geometry_proposal'
    native = verify_filled_planes(result.board, kicad_cli=Path(cli), artifact_directory=tmp_path/'native')
    assert native.matches(result.board) and native.passed, native.to_json()
    assert native.unconnected_count == native.other_violation_count == native.island_count == 0

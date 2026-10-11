"""Legalized rigid owners must not stale-lock later semantic bypass placement."""
from dataclasses import replace

import pytest

from pcbir import (ComponentPlacementRule, FootprintPad, PadReference, PhysicalFootprint,
                   PhysicalNet, Placement, Point, RigidPlacementCluster, RigidPlacementMember,
                   PlacementTarget, Size, nm_from_mm)
from pcbir.clusters import footprint_geometry_digest, cluster_placement_matches
from pcbir.decoupling import route_decouplers
from pcbir.placement import (PlacementPlannerOptions, PlacementAlgorithmError,
                             generate_placement_candidates, placement_solution_is_legal)
from test_routing_quality import bypass_board


def fixture(renamed=False, fixed_cap=False):
    board=bypass_board()
    neighbor=PhysicalFootprint('neighbor',(FootprintPad('1',Point(0,0),Size.mm('0.3','0.3')),),Size.mm(1,1))
    chip=board.footprints['chip']
    cluster=RigidPlacementCluster('synthetic-owner',PlacementTarget('U','1'),(
        RigidPlacementMember('U','chip',footprint_geometry_digest(chip),Point.mm(-1,0)),
        RigidPlacementMember('R','neighbor',footprint_geometry_digest(neighbor),Point.mm(-4,0))),
        'Synthetic algorithm fixture, not vendor reference layout',(0,))
    rules=(ComponentPlacementRule('U',fixed_position=Point.mm(15,8),fixed_rotation_degrees=0),)
    if fixed_cap:
        rules=(*rules,ComponentPlacementRule('C',fixed_position=board.placements[1].position,
                                             fixed_rotation_degrees=board.placements[1].rotation_degrees))
    board=replace(board,footprints={**board.footprints,'neighbor':neighbor},
        placements=(*board.placements,Placement('R','neighbor',Point.mm(5,8))),
        nets=tuple(replace(n,pads=(*n.pads,PadReference('R','1'))) if n.name=='G' else n for n in board.nets),
        placement_rules=rules,rigid_clusters=(cluster,))
    if renamed:
        refs={'U':'IC_X','C':'BYPASS_Y','R':'COMPANION_Z'}
        nets={'P':'SUPPLY_Q','G':'RETURN_R'}
        board=replace(board,
            placements=tuple(replace(p,reference=refs[p.reference]) for p in board.placements),
            nets=tuple(replace(n,name=nets[n.name],pads=tuple(PadReference(refs[p.component],p.pad) for p in n.pads)) for n in board.nets),
            placement_rules=tuple(replace(r,reference=refs[r.reference]) for r in board.placement_rules),
            rigid_clusters=(replace(cluster,anchor=PlacementTarget(refs['U'],'1'),
                members=tuple(replace(m,reference=refs[m.reference]) for m in cluster.members)),),
            decoupling_links=tuple(replace(link,capacitor=PadReference(refs[link.capacitor.component],link.capacitor.pad),
                target=PadReference(refs[link.target.component],link.target.pad),
                return_pad=PadReference(refs[link.return_pad.component],link.return_pad.pad),
                net=nets[link.net],return_net=nets[link.return_net]) for link in board.decoupling_links))
    return board


@pytest.mark.parametrize('renamed',(False,True))
def test_standard_planner_places_bypass_after_rigid_owner_moves_from_source_pose(renamed):
    board=fixture(renamed)
    options=PlacementPlannerOptions(candidate_count=1,analytical_iterations=0,
        refinement_passes=0,escape_spacing_passes=0)
    candidates=generate_placement_candidates(board,options)
    for candidate in candidates:
        poses={p.reference:p for p in candidate.placements}
        assert placement_solution_is_legal(board,poses,options)
        assert cluster_placement_matches(board,poses)
        capacitor=poses['BYPASS_Y' if renamed else 'C']
        assert capacitor.position != board.placements[1].position
        target=poses['IC_X' if renamed else 'U']
        assert target.position==Point.mm(15,8)
        assert abs(capacitor.position.x_nm-target.position.x_nm)+abs(capacitor.position.y_nm-target.position.y_nm)<nm_from_mm(5)
        bypass=route_decouplers(replace(board,placements=candidate.placements))
        assert bypass.created_tracks and not bypass.pending
        assert not bypass.board.vias


@pytest.mark.parametrize('renamed',(False,True))
def test_explicit_source_fixed_capacitor_is_not_rebased_or_moved(renamed):
    board=fixture(renamed,fixed_cap=True)
    candidates=generate_placement_candidates(board,PlacementPlannerOptions(candidate_count=1,
        analytical_iterations=0,refinement_passes=0,escape_spacing_passes=0))
    assert all(next(p for p in c.placements if p.reference==board.placements[1].reference)==board.placements[1]
               for c in candidates)


def test_session_locked_companion_cannot_be_rebased_to_waive_conflicting_source_anchor():
    board=fixture()
    with pytest.raises(PlacementAlgorithmError):
        generate_placement_candidates(board,PlacementPlannerOptions(candidate_count=1,
            analytical_iterations=0,refinement_passes=0,escape_spacing_passes=0,
            fixed_references=frozenset({'R'})))

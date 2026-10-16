from dataclasses import replace
from itertools import islice
import pytest
from pcbir import decoupling_paths as paths
from pcbir.fanout import FanoutOptions
from pcbir.physical import CopperKeepout,CopperLayer,PolygonWithHoles,PolygonRing,Point,Size
from pcbir.placement import transformed_local_point
from pcbir.drc import run_physical_drc
from test_decoupling_crosslayer_paths import fixture


@pytest.mark.parametrize('renamed',(False,True))
def test_nearby_sites_recover_bypass_when_outward_ray_is_blocked(monkeypatch,renamed):
    board,link=fixture(renamed=renamed)
    wall=PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in
        ((9.2,7.7),(14,7.7),(14,8.3),(9.2,8.3)))))
    board=replace(board,copper_keepouts=(CopperKeepout('blocked-outward-ray',
        (CopperLayer.FRONT,),wall,block_tracks=True,block_vias=True),))
    options=FanoutOptions(two_leg_escapes=False,refinement_step_nm=500000)
    with monkeypatch.context() as old:
        old.setattr(paths,'_local_launch_sites',lambda b,p,l,o,n:islice(paths._package_candidates(b,p,l,o),n))
        failed=paths.propose_crosslayer_decoupling(board,link,fanout_options=options,maximum_site_candidates=4)
        assert failed.witness is None and failed.board is board
    result=paths.propose_crosslayer_decoupling(board,link,fanout_options=options,maximum_site_candidates=4)
    assert result.witness is not None,result.reason
    assert paths.validate_crosslayer_witness(result.board,result.witness)==''
    assert not [f for f in run_physical_drc(result.board).findings if f.severity.value=='error'
        and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]


@pytest.mark.parametrize('step,limit',((500000,4),(125000,64),(100000,3)))
def test_ranked_prefix_matches_original_inventory_without_adding_sites(step,limit):
    b,_=fixture();pose=b.placements[1];pad=b.footprints[pose.footprint].pads[0]
    options=FanoutOptions(step_nm=step,refinement_step_nm=min(step,100000))
    start=transformed_local_point(pose,pad.position)
    old=tuple(paths._package_candidates(b,pose,pad,options))
    expected=sorted(old,key=lambda p:(p.x_nm-start.x_nm)**2+(p.y_nm-start.y_nm)**2)[:limit]
    assert tuple(paths._local_launch_sites(b,pose,pad,options,limit))==tuple(expected)


def test_fine_grid_inventory_materialization_stays_bounded(monkeypatch):
    b,_=fixture();pose=b.placements[0];pad=b.footprints[pose.footprint].pads[0]
    original=paths._package_candidates;count=0
    def counted(*args):
        nonlocal count
        for point in original(*args):
            count+=1;yield point
    monkeypatch.setattr(paths,'_package_candidates',counted)
    result=tuple(paths._local_launch_sites(b,pose,pad,
        FanoutOptions(step_nm=1,refinement_step_nm=1),64))
    assert len(result)==64 and count<=8*64+4


@pytest.mark.parametrize("renamed", (False, True))
def test_fresh_launch_slots_consider_near_refined_sites(renamed):
    board, link = fixture(renamed=renamed)
    chip = board.footprints["chip"]
    board = replace(board, footprints={**board.footprints, "chip": replace(chip,
        pads=tuple(replace(pad, size=Size.mm(.2, .2)) if pad.number == link.target.pad else pad
                   for pad in chip.pads))})
    options = FanoutOptions(two_leg_escapes=False, refinement_step_nm=125000)
    # A full coarse inventory must not hide the shorter legal 375 um launch.
    candidates = paths._connections(board, link.target, link.capacitor,
        link.net, options, 1, 64)
    candidate = next(candidates)
    assert len(candidate.vias) == 1
    start = paths._land(board, link.target)[2].position
    via = candidate.vias[0]
    assert abs(via.position.x_nm-start.x_nm)+abs(via.position.y_nm-start.y_nm) == 375000
    assert candidate.placements is board.placements
    assert not [f for f in run_physical_drc(candidate).findings
                if f.severity.value == "error" and f.code not in {"DRC-OPEN-NET", "DRC-ROUTE-INCOMPLETE"}]

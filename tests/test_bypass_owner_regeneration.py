from collections import Counter
from dataclasses import replace
import pytest
from pcbir import FootprintPad, PadReference, PhysicalNet, Point, Size
from pcbir.physical import DecouplingLink, PhysicalFootprint, TrackSegment, CopperLayer
from pcbir.fanout import route_fanout
from pcbir.package_access import _partial_ordinary_pattern, _verified_bypass_links
from pcbir.drc import run_physical_drc
import pcbir.package_access as access
from test_package_access import pattern_trap

def bypass_fixture(renamed=False, hits_feed=True):
    board, _, settings = pattern_trap()
    package = board.footprints["two"]
    package = replace(package, pads=(*package.pads,
        FootprintPad("3",Point.mm(0,"0.4"),Size.mm("0.3","0.3"))))
    capacitor = PhysicalFootprint("bypass", (
        FootprintPad("1",Point(0,0),Size.mm("0.3","0.3")),
        FootprintPad("2",Point.mm(0,1),Size.mm("0.3","0.3"))),Size.mm(1,2))
    names = {"A":"CLOCK_Q", "B":"SUPPLY_R", "GND":"RETURN_S"} if renamed else {n:n for n in ("A","B","GND")}
    board = replace(board, footprints={**board.footprints,"two":package,"bypass":capacitor},
        placements=tuple(replace(p,footprint="bypass") if p.reference=="JB" else p for p in board.placements),
        nets=(*tuple(replace(n,name=names[n.name],pads=(*n.pads,PadReference("U","3"))
                    if n.name=="B" else n.pads) for n in board.nets),
              PhysicalNet(names["GND"],(PadReference("JB","2"),))),
        net_routing_rules=tuple(replace(r,net=names[r.net]) for r in board.net_routing_rules),
        decoupling_links=(DecouplingLink(PadReference("JB","1"),PadReference("U","2"),
                           PadReference("JB","2"),names["B"],names["GND"]),))
    original = route_fanout(board, settings)
    assert original.decoupling_tracks and not original.decoupling_pending
    assert original.created_vias and all(v.net==names["B"] for v in original.created_vias)
    # A disposable clearance obstacle, not an asserted connected critical route.
    site = original.created_vias[0].position
    start,end = ((Point.mm("5.5","6.6"),Point.mm("7.5","6.6")) if hits_feed else
                 (Point(site.x_nm-100000,site.y_nm),Point(site.x_nm+100000,site.y_nm)))
    obstacle = replace(board, tracks=(TrackSegment(names["A"],start,end,100000,CopperLayer.FRONT),))
    obstacle=replace(obstacle,tracks=tuple(replace(t,width_nm=board.rules.minimum_track_width_nm) for t in obstacle.tracks))
    return board, original, obstacle, settings, names

@pytest.mark.parametrize("renamed", (False, True))
def test_regeneration_preserves_verified_source_link_and_all_package_exits(renamed):
    board,original,obstacle,settings,names=bypass_fixture(renamed)
    before=original.board
    required=_verified_bypass_links(before,{names['B']})
    assert required == frozenset(board.decoupling_links)
    protected=_partial_ordinary_pattern(original,board,obstacle,settings,8)
    assert protected[0] is None and protected[-1]=='protected_ordinary_owner'
    proposed,displaced,_,_,reason=_partial_ordinary_pattern(original,board,obstacle,settings,8,True)
    assert proposed is not None and reason=='proposed' and displaced==(names['B'],)
    assert required <= _verified_bypass_links(proposed.board,{names['B']})
    assert original.escaped_pads <= proposed.escaped_pads
    assert proposed.board.placements==board.placements and original.board==before
    assert proposed.decoupling_tracks != original.decoupling_tracks
    assert Counter(proposed.decoupling_tracks) <= Counter(proposed.created_tracks)
    combined=replace(proposed.board,tracks=(*obstacle.tracks,*proposed.created_tracks),vias=(*obstacle.vias,*proposed.created_vias))
    assert not [f for f in run_physical_drc(combined).findings if f.severity.value=='error'
                and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE'}]

def test_unverified_source_link_cannot_release_a_generated_feed():
    board,original,obstacle,settings,names=bypass_fixture()
    # Geometry alone is not a semantic dependency; removed association rejects.
    board=replace(board,decoupling_links=())
    original=replace(original,board=replace(original.board,decoupling_links=()))
    result=_partial_ordinary_pattern(original,board,obstacle,settings,8,True)
    assert result[0] is None and result[-1]=='protected_ordinary_owner'

def test_fresh_verification_rejects_loss_of_previously_connected_bypass(monkeypatch):
    board,original,obstacle,settings,names=bypass_fixture()
    before=original.board
    real=access.route_fanout
    def drop_feed(*args,**kwargs):
        result=real(*args,**kwargs)
        feeds=Counter(result.decoupling_tracks)
        held=[]
        for track in result.board.tracks:
            if feeds[track]: feeds[track]-=1
            else: held.append(track)
        return replace(result,board=replace(result.board,tracks=tuple(held)))
    monkeypatch.setattr(access,'route_fanout',drop_feed)
    failed=[]
    result=_partial_ordinary_pattern(original,board,obstacle,settings,8,True,failed)
    assert result[0] is None and result[-1]=='lost_bypass_connection'
    assert failed == [board.decoupling_links[0].capacitor]
    assert original.board==before and before.placements==board.placements


from test_partial_package_ownership import fixture
from pcbir.package_access import preflight_package_access
from pcbir.hard_macros import validate_hard_macros
from pcbir.pad_via_arrays import require_via_in_pad_arrays

def test_regeneration_dispatch_shares_budget_and_revalidates_immutable_owners(tmp_path,monkeypatch):
    board, guides, settings, baseline, _ = fixture(tmp_path, owners=True)
    real = access.route_fanout
    calls = []
    def exhausted_first(source, options, **kwargs):
        result = real(source, options, **kwargs)
        calls.append(kwargs)
        if len(calls) == 2:
            quiet = PadReference("QUIET_PACKAGE", "1")
            return replace(result, accesses={p:v for p,v in result.accesses.items() if p != quiet},
                           pending_pads=(*result.pending_pads, quiet))
        return result
    monkeypatch.setattr(access, "route_fanout", exhausted_first)
    real_access = access._access_result
    access_calls = []
    def initial_contact_exhaustion(*args, **kwargs):
        result = real_access(*args, **kwargs)
        access_calls.append(result)
        if len(access_calls) == 1:
            # Force an initial contact-search exhaustion so both normal owner
            # orders are eligible. Final contact geometry/acceptance stay real.
            return replace(result, plane_stitch=replace(result.plane_stitch,
                           pending_pads=(PadReference("ARRAY_OWNER", "EP"),)))
        return result
    monkeypatch.setattr(access, "_access_result", initial_contact_exhaustion)
    # Inject cheap eligibility rejection, not candidate geometry or route success.
    real_release=access._partial_ordinary_release
    def eligibility(*args):
        return real_release(*args) if len(args)==5 and args[4] else (("B",), "protected_ordinary_owner")
    monkeypatch.setattr(access,"_partial_ordinary_release",eligibility)
    result = preflight_package_access(board, guides, settings)
    assert result.ready and len(result.pattern_trials) == 2
    assert result.pattern_trials[0].strategy == "critical_and_plane_first"
    last = result.pattern_trials[1]
    assert last.strategy == "partial_bypass" and last.outcome == "accepted" and last.revalidated
    assert not last.partial_skipped_reason and len(calls)==3 and calls[-1]=={"only_nets":frozenset({"B"})}
    assert result.source == board and result.board.placements == board.placements
    validate_hard_macros(result.board)
    require_via_in_pad_arrays(result.board, "ineligible partial fallback")


from pcbir import route_global, ComponentPlacementRule
from pcbir.package_access import PackageAccessOptions, PackagePatternTrial, package_placement_trials
from pcbir.placement import PlacementPlannerOptions

def test_measured_bypass_hint_requires_exact_source_pad_and_respects_fixed_pose():
    board,original,obstacle,settings,names=bypass_fixture()
    board=replace(board,net_routing_rules=())
    baseline=preflight_package_access(board,route_global(board),settings,
        options=PackageAccessOptions(maximum_pattern_trials=0,reserve_plane_contacts=False))
    assert not baseline.pending_pads and not baseline.failed_critical_nets
    assert tuple(package_placement_trials(baseline,PlacementPlannerOptions(),500000))==()
    cap=board.decoupling_links[0].capacitor
    record=PackagePatternTrial(1,'partial_bypass',(),(),'lost_bypass_connection',failed_bypass_capacitors=(cap,))
    measured=replace(baseline,pattern_trials=(record,))
    proposals=tuple(package_placement_trials(measured,PlacementPlannerOptions(),500000))
    assert proposals and all(ref==cap.component for ref,_ in proposals)
    assert measured.pending_pads==baseline.pending_pads  # rejected dependency is not a current failed pin
    invalid=replace(record,failed_bypass_capacitors=(board.decoupling_links[0].return_pad,))
    assert tuple(package_placement_trials(replace(baseline,pattern_trials=(invalid,)),PlacementPlannerOptions(),500000))==()
    pose=next(p for p in board.placements if p.reference==cap.component)
    fixed=replace(board,placement_rules=(*tuple(r for r in board.placement_rules if r.reference!=cap.component),
        ComponentPlacementRule(cap.component,fixed_position=pose.position)))
    constraints=[]
    assert tuple(package_placement_trials(replace(measured,source=fixed),PlacementPlannerOptions(),500000,constraints=constraints))==()
    assert any(c.reference==cap.component and c.kind=='fixed_owner' for c in constraints)

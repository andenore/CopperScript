from collections import Counter
from dataclasses import replace
from pathlib import Path
import shutil

import pytest

from pcbir import CopperLayer, CopperZone, PadReference, Placement, Point, PolygonRing, PolygonWithHoles
from pcbir.plane_verify import verify_filled_planes, remove_native_dangling_tracks
from pcbir.native_stub_cleanup import native_stub_candidate
from pcbir.drc import explicit_copper_connectivity
from test_route_cleanup import board, track


@pytest.fixture(scope='module',params=(False,True))
def native_case(tmp_path_factory,request):
    cli=Path(shutil.which('kicad-cli') or 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file():
        pytest.skip('KiCad not installed')
    base=board()
    branch=track((10,4),(10,9))
    spur=track((13,6.15),(14,7.15))
    base=replace(base,placements=(*base.placements,Placement('J3','one-pad',Point.mm(10,4))),
        nets=(replace(base.nets[0],pads=(*base.nets[0].pads,PadReference('J3','1'))),),
        tracks=(track((3,6),(17,6)),branch,spur),
        zones=(CopperZone('local-thermal','S',(CopperLayer.FRONT,),
            PolygonWithHoles(PolygonRing(tuple(Point.mm(x,y) for x,y in ((1,2),(4,2),(4,7),(1,7)))))),))
    if request.param:
        # One terminal depends on actual plane fill. Cleaning an unrelated
        # remote stub must preserve its existing explicit partition, without
        # requiring or pretending that this terminal has an explicit feeder.
        base=replace(base,placements=(*base.placements,Placement('J4','one-pad',Point.mm(2,3))),
            nets=(replace(base.nets[0],pads=(*base.nets[0].pads,PadReference('J4','1'))),))
    root=tmp_path_factory.mktemp('native-stub-evidence')
    evidence=verify_filled_planes(base,kicad_cli=cli,artifact_directory=root/'before')
    assert evidence.unconnected_count==0
    assert len(evidence.dangling_track_uuids)==2
    return base,evidence,cli,branch,spur


def test_native_cleanup_trims_crossing_branch_and_remote_zone_spur(native_case,tmp_path):
    base,evidence,cli,branch,spur=native_case
    # Removing the entire native-flagged branch would sever J3. The successful
    # trial must retain its useful section and remove just the other spur.
    broken=replace(base,tracks=tuple(t for t in base.tracks if t!=branch))
    def joined(b):
        g=explicit_copper_connectivity(b)
        return g.roots[g.pad_nodes[PadReference('J1','1')][0]]==g.roots[g.pad_nodes[PadReference('J3','1')][0]]
    assert joined(base) and not joined(broken)
    result,checked=remove_native_dangling_tracks(base,evidence,Counter((base.tracks[0],)),
        kicad_cli=cli,artifact_directory=tmp_path/'after')
    assert checked.passed and checked.matches(result)
    assert set(result.tracks)=={base.tracks[0],track((10,4),(10,6))}
    assert result.zones==base.zones and result.vias==base.vias
    assert joined(result)
    assert explicit_copper_connectivity(result).net_connected(base.nets[0]) == (
        explicit_copper_connectivity(base).net_connected(base.nets[0]))


def test_native_cleanup_preserves_protected_tracks_and_rejects_stale_evidence(native_case):
    base,evidence,cli,branch,spur=native_case
    assert native_stub_candidate(base,evidence,Counter((branch,))) is None
    assert native_stub_candidate(replace(base,name='different-board'),evidence,Counter()) is None
    unknown=replace(evidence.native_findings[0].items[0],uuid='unbound-uuid')
    forged=replace(evidence,native_findings=(replace(evidence.native_findings[0],items=(unknown,)),
                                           *evidence.native_findings[1:]))
    assert native_stub_candidate(base,forged,Counter()) is None


def test_native_cleanup_rolls_back_if_fresh_verification_fails(native_case,monkeypatch):
    base,evidence,cli,branch,spur=native_case
    import pcbir.plane_verify as verification
    monkeypatch.setattr(verification,'verify_filled_planes',lambda *a,**kw:replace(evidence,passed=True))
    result,checked=remove_native_dangling_tracks(base,evidence,Counter(),kicad_cli=cli)
    assert result is base and checked is evidence


def test_critical_net_overhang_is_preserved(native_case,tmp_path):
    from pcbir.physical import NetRoutingRule, RouteKind
    base,evidence,cli,branch,spur=native_case
    critical=replace(base,net_routing_rules=(NetRoutingRule('S',kind=RouteKind.CRITICAL),))
    checked=verify_filled_planes(critical,kicad_cli=cli,artifact_directory=tmp_path/'critical')
    assert checked.dangling_track_uuids
    assert native_stub_candidate(critical,checked,Counter()) is None


@pytest.fixture(scope='module')
def mixed_native_case(tmp_path_factory):
    from pcbir import Via, nm_from_mm
    cli=Path(shutil.which('kicad-cli') or 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe')
    if not cli.is_file():
        pytest.skip('KiCad not installed')
    base=board()
    # The branch enters off centre, but its complete endpoint disk is in J3.
    # Removing the whole branch would disconnect J3; retain its useful section.
    branch=track((10,4),(10,9))
    via=Via('S',Point.mm(12,6),nm_from_mm('0.6'),nm_from_mm('0.3'))
    base=replace(base,placements=(*base.placements,Placement('J3','one-pad',Point.mm('10.1',4))),
        nets=(replace(base.nets[0],pads=(*base.nets[0].pads,PadReference('J3','1'))),),
        tracks=(track((3,6),(17,6)),branch),vias=(via,))
    evidence=verify_filled_planes(base,kicad_cli=cli,
        artifact_directory=tmp_path_factory.mktemp('mixed-native')/'before')
    assert {f.type for f in evidence.native_findings}=={'track_dangling','via_dangling'}
    assert evidence.unconnected_count==0
    return base,evidence,cli,via


def test_mixed_cleanup_preserves_offcentre_land_entry(mixed_native_case,tmp_path):
    base,evidence,cli,via=mixed_native_case
    cleaned,checked=remove_native_dangling_tracks(base,evidence,Counter((base.tracks[0],)),
        protected_vias=Counter(),kicad_cli=cli,artifact_directory=tmp_path/'after')
    assert checked.passed and checked.matches(cleaned)
    assert not cleaned.vias
    assert cleaned.tracks==(base.tracks[0],track((10,4),(10,6)))
    assert explicit_copper_connectivity(cleaned).net_connected(base.nets[0])
    assert cleaned.placements==base.placements


def test_mixed_cleanup_requires_explicit_via_ownership(mixed_native_case):
    base,evidence,cli,via=mixed_native_case
    assert native_stub_candidate(base,evidence,Counter()) is None
    assert native_stub_candidate(base,evidence,Counter(),protected_vias=Counter((via,))) is None
    assert native_stub_candidate(base,evidence,Counter((base.tracks[1],)),protected_vias=Counter()) is None


def test_mixed_cleanup_rolls_back_on_failed_fresh_native(mixed_native_case,monkeypatch):
    import pcbir.plane_verify as verification
    base,evidence,cli,via=mixed_native_case
    monkeypatch.setattr(verification,'verify_filled_planes',lambda *a,**kw:evidence)
    cleaned,checked=remove_native_dangling_tracks(base,evidence,Counter(),
        protected_vias=Counter(),kicad_cli=cli)
    assert cleaned is base and checked is evidence


def test_mixed_cleanup_rejects_grazing_pad_entry(mixed_native_case,tmp_path):
    base,_,cli,via=mixed_native_case
    # The existing trace overlaps this pad, but its endpoint disk is not
    # contained in it; no new off-centre anchor is inferred at a grazing edge.
    base=replace(base,placements=(*base.placements[:2],replace(base.placements[2],position=Point.mm('10.35',4))))
    evidence=verify_filled_planes(base,kicad_cli=cli,artifact_directory=tmp_path/'grazing')
    diagnostics=[]
    assert native_stub_candidate(base,evidence,Counter(),protected_vias=Counter(),diagnostics=diagnostics) is None
    assert diagnostics==[{'reason':'changed_explicit_partition','net':'S'}]


def test_mixed_cleanup_follows_via_launch_leg_in_one_transaction(mixed_native_case,tmp_path):
    base,_,cli,via=mixed_native_case
    base=replace(base,tracks=(*base.tracks,track((12,6),(12,8))),
                 vias=(replace(via,position=Point.mm(12,8)),))
    evidence=verify_filled_planes(base,kicad_cli=cli,artifact_directory=tmp_path/'before')
    cleaned,checked=remove_native_dangling_tracks(base,evidence,Counter((base.tracks[0],)),
        protected_vias=Counter(),kicad_cli=cli,artifact_directory=tmp_path/'after')
    assert checked.passed and checked.matches(cleaned)
    assert cleaned.tracks==(base.tracks[0],track((10,4),(10,6)))
    assert not cleaned.vias
    assert (tmp_path/'after'/'round-002'/'drc.json').is_file()


def test_mixed_cleanup_rolls_back_if_exposed_leg_is_owned(mixed_native_case,tmp_path):
    base,_,cli,via=mixed_native_case
    leg=track((12,6),(12,8))
    base=replace(base,tracks=(*base.tracks,leg),vias=(replace(via,position=Point.mm(12,8)),))
    evidence=verify_filled_planes(base,kicad_cli=cli,artifact_directory=tmp_path/'before')
    cleaned,checked=remove_native_dangling_tracks(base,evidence,Counter((leg,)),
        protected_vias=Counter(),kicad_cli=cli,artifact_directory=tmp_path/'after')
    assert cleaned is base and checked is evidence


def test_mixed_cleanup_removes_redundant_grazing_branch(mixed_native_case,tmp_path):
    base,_,cli,via=mixed_native_case
    retained=track(('10.35',4),('10.35',6))
    base=replace(base,placements=(*base.placements[:2],replace(base.placements[2],position=Point.mm('10.35',4))),
                 tracks=(*base.tracks,retained))
    evidence=verify_filled_planes(base,kicad_cli=cli,artifact_directory=tmp_path/'before')
    cleaned,checked=remove_native_dangling_tracks(base,evidence,Counter((base.tracks[0],retained)),
        protected_vias=Counter(),kicad_cli=cli,artifact_directory=tmp_path/'after')
    assert checked.passed and checked.matches(cleaned)
    assert cleaned.tracks==(base.tracks[0],retained)
    assert not cleaned.vias

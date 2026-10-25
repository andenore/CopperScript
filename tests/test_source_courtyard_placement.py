from dataclasses import replace
import pytest
from pcbir.physical import (
    BoardOutline, BoardSide, ComponentPlacementRule, PhysicalBoard,
    PhysicalFootprint, Placement, PlacementRegion, Point, Size,
)
from pcbir.placement import (
    PlacementPlannerOptions, _legal, _courtyards_too_close,
    _courtyard_contact_candidates, _pack_local_components,
    placement_solution_is_legal,
)


def scene(gaps=(100000, 100000), separation="1.1"):
    fp=PhysicalFootprint("unit", (), Size.mm(1, 1), courtyard=tuple(
        Point.mm(x,y) for x,y in ((-.5,-.5),(.5,-.5),(.5,.5),(-.5,.5))))
    a=Placement("left",fp.name,Point.mm(10,10),side=BoardSide.BACK)
    b=replace(a,reference="right",position=Point(a.position.x_nm+Point.mm(separation,0).x_nm,a.position.y_nm))
    rules=tuple(ComponentPlacementRule(q.reference,additional_courtyard_gap_nm=gap)
                for q,gap in zip((a,b),gaps))
    return PhysicalBoard("policy",BoardOutline.rectangle(30,30),{fp.name:fp},(a,b),(),placement_rules=rules)


def test_source_member_minima_legalize_only_annotated_pair_and_preserve_stricter_neighbor():
    b=scene()
    assert placement_solution_is_legal(b,{q.reference:q for q in b.placements})
    for gaps in ((100000,None),(None,100000),(100000,600000),(600000,100000)):
        strict=scene(gaps)
        assert not placement_solution_is_legal(strict,{q.reference:q for q in strict.placements})
    untouched=scene((None,None),"1.2")
    poses={q.reference:q for q in untouched.placements}
    assert not placement_solution_is_legal(untouched,poses)
    assert placement_solution_is_legal(untouched,poses,PlacementPlannerOptions(component_clearance_nm=200000))
    assert not placement_solution_is_legal(untouched,poses)


def test_source_minimum_is_preserved_when_caller_fallback_is_smaller():
    b=scene((300000,300000),"1.2")
    assert not placement_solution_is_legal(b,{q.reference:q for q in b.placements},
        PlacementPlannerOptions(component_clearance_nm=100000))


@pytest.mark.parametrize("gap,separation", [(100000, "1.1"), (0, "1")])
def test_unrelated_macro_preflight_honors_fixed_member_courtyard_policy(gap, separation):
    from pcbir.cluster_placement import place_rigid_clusters
    from pcbir.clusters import footprint_geometry_digest
    from pcbir.physical import RigidPlacementCluster, RigidPlacementMember, PlacementTarget
    from pcbir.placement import PlacementAlgorithmError
    b = scene((gap, gap), separation)
    fp = b.footprints["unit"]
    owner = Placement("owner", fp.name, Point.mm(20, 20))
    cluster = RigidPlacementCluster("unrelated", PlacementTarget("owner"), (
        RigidPlacementMember("owner", fp.name, footprint_geometry_digest(fp), Point(0, 0)),
    ), "Synthetic courtyard-policy regression", (0,))
    rules = tuple(replace(rule, fixed_position=pose.position, fixed_rotation_degrees=0)
                  for rule, pose in zip(b.placement_rules, b.placements))
    b = replace(b, placements=(*b.placements, owner), rigid_clusters=(cluster,),
                placement_rules=(*rules, ComponentPlacementRule("owner",
                    fixed_position=owner.position, fixed_rotation_degrees=0)))
    poses = {p.reference: p for p in b.placements}
    result, _, _ = place_rigid_clusters(b, poses, poses, PlacementPlannerOptions())
    assert result == poses
    strict = replace(b, placement_rules=tuple(
        replace(rule, additional_courtyard_gap_nm=None) for rule in b.placement_rules))
    with pytest.raises(PlacementAlgorithmError, match="conflicting fixed placements"):
        place_rigid_clusters(strict, poses, poses, PlacementPlannerOptions())


def test_contact_proposals_and_bounded_packer_use_source_pair_gap():
    b=scene()
    a,target=b.placements
    region=PlacementRegion("tight",BoardOutline.rectangle(2.1,1,origin=Point.mm(9.5,9.5)),BoardSide.BACK)
    target=replace(target,position=a.position)
    b=replace(b,placements=(a,target),regions=(region,),placement_rules=(
        replace(b.placement_rules[0],fixed_position=a.position,fixed_rotation_degrees=0,side=a.side),
        replace(b.placement_rules[1],region="tight",side=target.side)))
    options=PlacementPlannerOptions(legalization_candidates=1)
    base={a.reference:a}
    proposals=_courtyard_contact_candidates(b,target,(a,),options,excluded=(),limit=64)
    legal=[q for q in proposals if _legal(q,base,b,options)]
    assert legal and all(q.position.x_nm==11100000 for q in legal)
    before=dict(base)
    result=_pack_local_components(b,[target.reference],base,{q.reference:q for q in b.placements},options,allow_general=True)
    assert result is not None and base==before
    assert result[a.reference]==a
    assert placement_solution_is_legal(b,result,options)


def polygon(coords):
    return tuple(Point(x,y) for x,y in coords)


@pytest.mark.parametrize("reverse",(False,True))
def test_zero_extra_gap_allows_polygon_edge_and_corner_contact_but_rejects_area_overlap(reverse):
    # Diagonal, non-rectangle courts exercise the exact interior-overlap path.
    a=polygon(((0,0),(10,10),(0,20),(-10,10)))
    corner=polygon(((10,10),(20,20),(10,30),(0,20)))
    edge=polygon(((0,0),(10,10),(20,0),(10,-10)))
    shifted=tuple(Point(q.x_nm+1,q.y_nm) for q in a)
    if reverse:a=tuple(reversed(a))
    assert not _courtyards_too_close(a,corner,0)
    assert not _courtyards_too_close(a,edge,0)
    assert _courtyards_too_close(a,edge,1)
    assert _courtyards_too_close(a,shifted,0)
    assert _courtyards_too_close(a,a,0)


def test_zero_gap_concave_courts_containment_and_collinear_boundaries():
    concave=polygon(((0,0),(20,0),(20,10),(10,10),(10,20),(0,20)))
    in_notch=polygon(((10,10),(20,10),(20,20),(10,20)))
    overlapping=polygon(((9,9),(20,9),(20,20),(9,20)))
    contained=polygon(((1,1),(9,1),(9,9),(1,9)))
    assert not _courtyards_too_close(concave,in_notch,0)
    assert _courtyards_too_close(concave,overlapping,0)
    assert _courtyards_too_close(concave,contained,0)
    assert _courtyards_too_close(concave,concave,0)


def test_one_nanometre_extra_gap_never_waives_boundary_clearance():
    a=polygon(((0,0),(10,10),(0,20)))
    touching=polygon(((0,0),(10,10),(20,0)))
    assert not _courtyards_too_close(a,touching,0)
    assert _courtyards_too_close(a,touching,1)


def test_zero_extra_gap_does_not_admit_self_crossing_courtyard_as_simple_ring():
    invalid=polygon(((0,0),(24,14),(-4,14),(10,24),(20,0)))
    small=polygon(((7,7),(9,7),(9,9),(7,9)))
    assert _courtyards_too_close(invalid,small,0)
    assert _courtyards_too_close(small,invalid,0)


@pytest.mark.parametrize("gap",(0,100000,None))
def test_self_pose_support_anchor_uses_own_authored_gap_without_pair_authority(gap):
    b=scene((None,gap))
    a,target=b.placements
    target=replace(target,position=Point.mm(10,10))
    region=PlacementRegion("broad",BoardOutline.rectangle(10,10,origin=Point.mm(5,5)),BoardSide.BACK)
    b=replace(b,placements=(a,target),regions=(region,),placement_rules=(
        b.placement_rules[0],replace(b.placement_rules[1],region="broad",side=target.side)))
    proposals=_courtyard_contact_candidates(b,target,(target,),PlacementPlannerOptions(),excluded=(),limit=200)
    expected=1000000+(500000 if gap is None else gap)
    assert any(q.position==Point(target.position.x_nm+expected,target.position.y_nm) for q in proposals)
    # The source support pose creates no exception for the unannotated neighbor.
    assert not _legal(target,{a.reference:a},b,PlacementPlannerOptions())


def test_nonrectangular_protected_boundary_predicate_is_not_relaxed_by_pair_policy():
    from pcbir.placement import _polygons_too_close
    a=polygon(((0,0),(10,10),(0,20)))
    touching=polygon(((0,0),(10,10),(20,0)))
    assert _polygons_too_close(a,touching,0)
    assert not _courtyards_too_close(a,touching,0)

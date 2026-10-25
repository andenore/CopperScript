"""Couple mutable bypass placement to complete, prospective BGA via patterns.

Only an internal obstacle index defers mutable SMD lands. Every offered result
restores the complete inventory, source intent and exact copper checks. The
input, fixed poses, macros and existing copper are never edited by a trial.
"""
from dataclasses import dataclass, replace
from math import isqrt

from .drc import physical_board_digest, placed_pad_shape
from .geometry import RoundedConvexShape, shapes_clear
from .package_patterns import PackagePatternOptions, propose_package_patterns
from .physical import BoardSide, CopperLayer, PadKind, PadReference, Point
from .placement import (_allowed_orientations, _legal, placement_solution_is_legal,
                        transformed_local_point, transformed_pad_position)
from .routing_clearance import RoutingClearanceIndex
from .support_patterns import _protected_members
from .progress import emit


def package_contact_intents(board,component):
    """Active general-route lands; specialised macros and required arrays own theirs."""
    from .hard_macros import macro_owned_pads
    from .pad_via_arrays import array_rules
    from .physical import RouteKind
    membership={p:n.name for n in board.nets if len(n.pads)>1 for p in n.pads}
    rules={r.net:r for r in board.net_routing_rules}
    owned=macro_owned_pads(board)|{r.pad for r in array_rules(board)}
    supplies={d.net for d in board.power_domains}
    returns={l.return_net for l in board.decoupling_links}
    from .package_patterns import PackageContactIntent
    return tuple(PackageContactIntent(p,'return' if net in returns else 'supply' if net in supplies else 'signal')
        for p,net in sorted(membership.items()) if p.component==component and p not in owned
        and (net not in rules or rules[net].kind is RouteKind.GENERAL))


@dataclass(frozen=True, slots=True)
class PackageSupportOptions:
    maximum_patterns: int = 12
    maximum_moved_components: int = 32
    maximum_pose_checks: int = 32768
    candidates_per_component: int = 32
    maximum_search_states: int = 4096
    maximum_pair_queries: int = 400000
    maximum_complete_checks: int = 8
    radius_in_pitches: int = 5

    def __post_init__(self):
        if any(type(getattr(self,n)) is not int or getattr(self,n)<=0 for n in self.__dataclass_fields__):
            raise ValueError('package support work limits must be positive integers')


@dataclass(frozen=True)
class PackageSupportResult:
    board: object
    status: str
    source_digest: str
    pattern: object = None
    proposal: object = None
    moved: tuple = ()
    trials: tuple = ()
    work: tuple = ()

    def report(self):
        return dict(status=self.status,source_digest=self.source_digest,
            board_digest=physical_board_digest(self.board),moved=self.moved,
            pattern=None if self.pattern is None else dict(family=self.pattern.family,
                offset_nm=self.pattern.offset_nm,contacts=len(self.pattern.contacts)),
            trials=self.trials,work=dict(self.work),native_accepted=False,
            boundary_verified=False)


def _lands(board,pose):
    membership={p:n.name for n in board.nets for p in n.pads}
    layer=CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
    return tuple((membership.get(PadReference(pose.reference,p.number),'<unconnected>'),
        placed_pad_shape(transformed_local_point(pose,p.position),p,pose),layer)
        for p in board.footprints[pose.footprint].pads)


def _wired(board,pose):
    """Protect any existing copper contact, including a partial/foreign one."""
    layers=board.stackup.copper_layers
    for _,shape,layer in _lands(board,pose):
        if any(t.layer is layer and not shapes_clear(shape,
                RoundedConvexShape((t.start,t.end),t.width_nm//2),1) for t in board.tracks):return True
        if any(layer in layers[layers.index(v.from_layer):layers.index(v.to_layer)+1]
               and not shapes_clear(shape,RoundedConvexShape((v.position,),v.size_nm//2),1)
               for v in board.vias):return True
    return False


def _copper_clear(board,pose,index,vias):
    layers=board.stackup.copper_layers
    for net,shape,layer in _lands(board,pose):
        if not index.can_area(net,shape,layer):return False
        for via in vias:
            if layer not in layers[layers.index(via.from_layer):layers.index(via.to_layer)+1]:continue
            # No implicit via-in-pad, including a same-net annulus.
            if not shapes_clear(shape,RoundedConvexShape((via.position,),via.size_nm//2),1):return False
            if via.net!=net and not shapes_clear(shape,
                    RoundedConvexShape((via.position,),via.drill_nm//2),board.rules.minimum_hole_clearance_nm):return False
    return True


def _pose_pool(board,pose,links,pattern,array,options):
    """Via-relative supply anchors, inter-row pockets and local lattice steps."""
    contacts={c.intent.pad:c for c in pattern.contacts}
    supply=[contacts[l.target].via.position for l in links if l.target in contacts]
    package=next(p for p in board.placements if p.reference==array.component)
    if not supply:
        supply=[transformed_pad_position(board,package,l.target.pad) for l in links]
    returns=[c.via.position for c in pattern.contacts if c.net in {l.return_net for l in links}]
    anchors=tuple(dict.fromkeys((*supply,*sorted(returns,key=lambda p:min(
        (p.x_nm-s.x_nm)**2+(p.y_nm-s.y_nm)**2 for s in supply))[:2])))
    step=array.pitch_nm//2; radius=options.radius_in_pitches*2
    proposals={pose}
    for angle in _allowed_orientations(board,pose.reference):
        origin=replace(pose,position=Point(0,0),rotation_degrees=angle)
        land=transformed_pad_position(board,origin,links[0].capacitor.pad)
        for anchor in anchors:
            for x in range(-radius,radius+1):
                for y in range(-radius,radius+1):
                    if x*x+y*y>radius*radius:continue
                    offset=transformed_local_point(replace(package,position=Point(0,0)),Point(x*step,y*step))
                    proposals.add(replace(origin,position=Point(anchor.x_nm+offset.x_nm-land.x_nm,
                                                                anchor.y_nm+offset.y_nm-land.y_nm)))
    def rank(p):
        a=transformed_pad_position(board,p,links[0].capacitor.pad)
        r=transformed_pad_position(board,p,links[0].return_pad.pad)
        distance=lambda p,q:isqrt((p.x_nm-q.x_nm)**2+(p.y_nm-q.y_nm)**2)
        loop=min(distance(a,s) for s in supply)+(min(distance(r,t) for t in returns) if returns else 0)
        return (p!=pose,loop,distance(p.position,pose.position),p.rotation_degrees,p.position.x_nm,p.position.y_nm)
    return sorted(proposals,key=rank)


def plan_package_support(board,component,intents,placement_options,*,options=None,
                         pattern_options=None,accept=None,on_progress=None):
    """Return a complete placement + contact proposal or the exact input board.

    ``accept(proposal, pattern)`` is an optional additional acceptance gate.
    It may run the normal native/power transaction; rejection leaves no state.
    This stage does not claim filled continuity or boundary-route completion.
    """
    from .escape_assignment import _BudgetExceeded, _finite_assignment
    options=options or PackageSupportOptions();pattern_options=pattern_options or PackagePatternOptions()
    intents=tuple(intents);digest=physical_board_digest(board);poses={p.reference:p for p in board.placements}
    work=dict(poses=0,states=0,pair_queries=0,complete_checks=0,patterns=0);trials=[]
    def finish(status,**kwargs):
        return PackageSupportResult(board,status,digest,trials=tuple(trials),work=tuple(work.items()),**kwargs)
    if board.zone_fills:return finish('filled_source_requires_fresh_preparation')
    protected=_protected_members(board,poses,placement_options)
    links={}
    for link in board.decoupling_links:
        if link.target.component==component:links.setdefault(link.capacitor.component,[]).append(link)
    mutable=frozenset(ref for ref,ls in links.items() if ref not in protected
        and all(l.target.component==component for l in board.decoupling_links if l.capacitor.component==ref)
        and all(p.kind is PadKind.SMD for p in board.footprints[poses[ref].footprint].pads)
        and not _wired(board,poses[ref]))
    patterns=propose_package_patterns(board,component,intents,pattern_options,
        _deferred_components=mutable,on_progress=on_progress)
    if not patterns.candidates:return finish('pattern_pool_unresolved')
    # Cell/frontier offsets leave repeated inter-row pockets; try those before
    # near-tangent annuli when coordinating bottom-side component placement.
    ordered=sorted(patterns.candidates,key=lambda p:(p.family!='outward',-p.offset_nm,p.family))
    for pattern in ordered[:options.maximum_patterns]:
        work['patterns']+=1
        emit(on_progress,'package_support_pattern','started',family=pattern.family,offset_nm=pattern.offset_nm)
        index=RoutingClearanceIndex(board,_deferred_components=mutable)
        for t in pattern.tracks:index.add_track(t,locked=True)
        for v in pattern.new_vias:index.add_via(v,locked=True)
        vias=(*board.vias,*pattern.new_vias)
        moved=frozenset(ref for ref in mutable if not _copper_clear(board,poses[ref],index,vias))
        trial=dict(family=pattern.family,offset_nm=pattern.offset_nm,members=sorted(moved));trials.append(trial)
        if len(moved)>options.maximum_moved_components:
            trial['status']='mutable_component_limit';continue
        held={r:p for r,p in poses.items() if r not in moved}
        domains={}
        try:
            for ordinal,ref in enumerate(sorted(moved)):
                offered=[]; pockets={}
                allowance=(options.maximum_pose_checks-work['poses'])//(len(moved)-ordinal)
                local_limit=work['poses']+allowance
                for pose in _pose_pool(board,poses[ref],links[ref],pattern,patterns.array,options):
                    if work['poses']>=local_limit:break
                    work['poses']+=1
                    # Retain different physical pockets, rather than filling a
                    # domain with tiny shifts around one attractive return via.
                    # Two orientations per pitch-sized pocket remain useful.
                    pitch=patterns.array.pitch_nm
                    pocket=((pose.position.x_nm-poses[ref].position.x_nm)//pitch,
                            (pose.position.y_nm-poses[ref].position.y_nm)//pitch)
                    if pockets.get(pocket,0)>=2:continue
                    if not _copper_clear(board,pose,index,vias) or not _legal(pose,held,board,placement_options):continue
                    offered.append(pose);pockets[pocket]=pockets.get(pocket,0)+1
                    if len(offered)>=options.candidates_per_component:break
                domains[ref]=tuple(offered)
            trial['domain_sizes']={r:len(ps) for r,ps in domains.items()}
            if work['poses']>=options.maximum_pose_checks and any(not ps for ps in domains.values()):
                raise _BudgetExceeded('pose_budget_exhausted')
            if any(not ps for ps in domains.values()):trial['status']='placement_pool_unresolved';continue
            from .placement import _placement_polygon,_courtyards_too_close
            from .geometry import bounds
            from .physical import component_pair_courtyard_gap_nm
            polygons={(r,i):_placement_polygon(board,p) for r,ps in domains.items() for i,p in enumerate(ps)}
            boxes={k:bounds(v) for k,v in polygons.items()}
            gaps={(a,b):component_pair_courtyard_gap_nm(board,a,b,
                fallback_nm=placement_options.component_clearance_nm) for a in domains for b in domains if a<b}
            cache={};accepted=[]
            def compatible(a,i,b,j):
                if a>b:a,i,b,j=b,j,a,i
                key=(a,i,b,j)
                if key not in cache:
                    if work['pair_queries']>=options.maximum_pair_queries:raise _BudgetExceeded('pair_budget_exhausted')
                    work['pair_queries']+=1
                    cache[key]=(domains[a][i].side is not domains[b][j].side
                        or not boxes[a,i].intersects_expanded(boxes[b,j],gaps[a,b])
                        or not _courtyards_too_close(polygons[a,i],polygons[b,j],gaps[a,b]))
                return cache[key]
            def choice_key(a,i,remaining):
                # Least-constraining pockets prevent a locally attractive cap
                # pose from consuming the only slot for a neighbouring bank.
                return sum(sum(not compatible(a,i,b,j) for j in js)/len(js)
                           for b,js in remaining.items() if b!=a)
            def consume():
                if work['states']>=options.maximum_search_states:raise _BudgetExceeded('search_budget_exhausted')
                work['states']+=1
            def complete(selected):
                if work['complete_checks']>=options.maximum_complete_checks:raise _BudgetExceeded('complete_budget_exhausted')
                work['complete_checks']+=1
                placed={**held,**{r:domains[r][i] for r,i in selected.items()}}
                if not placement_solution_is_legal(board,placed,placement_options):return False
                candidate=replace(board,placements=tuple(placed[p.reference] for p in board.placements))
                fresh=RoutingClearanceIndex(candidate)
                for t in pattern.tracks:
                    if not fresh.can_track(t.net,t.start,t.end,t.width_nm,t.layer):return False
                if not fresh.candidate_vias_clear(pattern.new_vias):return False
                final_pattern=replace(pattern,source_digest=physical_board_digest(candidate),deferred_components=())
                from .copper_reservations import reserve_copper_scene
                proposal=reserve_copper_scene(candidate,pattern.tracks,pattern.new_vias).board
                if accept is not None and not accept(proposal,final_pattern):return False
                accepted.append((candidate,proposal,final_pattern));return True
            selected=_finite_assignment({r:tuple(range(len(ps))) for r,ps in domains.items()},{},preferred={},
                rank={r:i for i,r in enumerate(sorted(domains))},compatible=compatible,consume_state=consume,
                accept_complete=complete,choice_key=choice_key)
            if selected is not None:
                candidate,proposal,final_pattern=accepted[0];trial['status']='complete_geometry_proposal'
                emit(on_progress,'package_support_pattern','finished',status=trial['status'],moved=len(moved))
                return replace(finish('complete_geometry_proposal',pattern=final_pattern,proposal=proposal,
                    moved=tuple(sorted(moved))),board=candidate)
            trial['status']='joint_placement_unresolved'
        except _BudgetExceeded as error:
            trial['status']=str(error);return finish(str(error))
    return finish('finite_joint_pool_unresolved')


def improve_package_support_placement(board,placements,placement_options,*,on_progress=None):
    """Standard placement-stage adapter; retain poses, never provisional copper."""
    from .hard_macros import materialize_hard_macros
    from .package_patterns import circular_array
    source=replace(board,placements=tuple(placements[p.reference] for p in board.placements))
    scene=materialize_hard_macros(source);reports=[]
    components=sorted({l.target.component for l in source.decoupling_links
                       if circular_array(source,l.target.component)})
    if len(components)>4:return placements,({'status':'package_count_limit'},)
    for component in components:
        intents=package_contact_intents(scene,component)
        if not intents:continue
        result=plan_package_support(scene,component,intents,placement_options,
            options=PackageSupportOptions(maximum_complete_checks=placement_options.package_support_trials),
            on_progress=on_progress)
        reports.append(dict(component=component,**result.report()))
        if result.pattern is None:return placements,tuple(reports)
        # Earlier via patterns remain obstacles to later package placement.
        scene=result.proposal
    final={p.reference:p for p in scene.placements}
    if not placement_solution_is_legal(source,final,placement_options):return placements,tuple(reports)
    return final,tuple(reports)

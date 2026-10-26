"""Source-bound support-bank patterns during mutable, unrouted placement.

Patterns propose poses; complete local routing proves their connections. No
accepted copper is released, no source association is swapped, and no pattern
is a native/PDN certificate. All failed trials leave the input untouched.
"""
from collections import defaultdict
from dataclasses import dataclass, replace
from math import isqrt

from .drc import physical_board_digest, run_physical_drc
from .physical import BoardSide, CopperLayer, Point, RouteKind


@dataclass(frozen=True, slots=True)
class SupportPatternOptions:
    maximum_groups: int = 4
    maximum_members: int = 4
    patterns_per_group: int = 12
    maximum_pose_probes: int = 512
    maximum_search_states: int = 2048
    maximum_pair_checks: int = 8192
    maximum_complete_checks: int = 8
    maximum_return_targets: int = 8
    maximum_path_states: int = 10_000
    frontier_size: int = 3
    offsets_per_template: int = 8
    offset_axis_events: int = 8
    offset_sweep_events: int = 64
    offset_templates_per_family: int = 4

    def __post_init__(self):
        if any(type(getattr(self,n)) is not int or getattr(self,n)<=0 for n in self.__dataclass_fields__):
            raise ValueError('support pattern bounds must be positive integers')


@dataclass(frozen=True, slots=True)
class SupportPattern:
    source_digest: str
    family: str
    poses: tuple
    links: tuple
    parameters: tuple = ()

    def __post_init__(self):
        for name in ('poses','links','parameters'):
            object.__setattr__(self,name,tuple(getattr(self,name)))
        if not self.source_digest or not self.family or not self.poses or not self.links:
            raise ValueError('source-bound support poses and associations required')
        if len({p.reference for p in self.poses})!=len(self.poses):
            raise ValueError('unique source component poses required')


@dataclass(frozen=True)
class SupportPlacementResult:
    board: object
    status: str
    source_digest: str
    proposal: object = None
    signal_scene: object = None
    ports: tuple = ()
    selected: tuple = ()
    trials: tuple = ()
    protected_members: tuple = ()
    scope: tuple = ()
    baseline_verified: tuple = ()
    verified: tuple = ()
    pose_probes: int = 0
    search_states: int = 0
    pair_checks: int = 0
    complete_checks: int = 0
    routing_passes: int = 0
    frontier: tuple = ()
    reservation_report: object = None
    limits: object = None

    def report(self):
        return dict(status=self.status,source_digest=self.source_digest,
            board_digest=physical_board_digest(self.board),native_acceptance=False,
            protected_members=self.protected_members,scope=[str(l) for l in self.scope],
            baseline_verified=len(self.baseline_verified) if self.routing_passes else None,
            verified=len(self.verified) if self.routing_passes else None,
            reserved_ports=len(self.ports),pose_probes=self.pose_probes,
            search_states=self.search_states,pair_checks=self.pair_checks,
            complete_checks=self.complete_checks,routing_passes=self.routing_passes,
            selected=[dict(family=p.family,members=[q.reference for q in p.poses],
                           parameters=p.parameters) for p in self.selected],
            trials=self.trials,frontier=self.frontier,
            limits=self.limits,reservation_report=self.reservation_report,
            unobserved_associations=len(self.board.decoupling_links)-(len(self.scope) if self.routing_passes else 0),
            unobserved_phases=['distribution_area_acceptance','native_board_acceptance','electrical_loop_assessment'],
            electrical_assessment='route_length_via_count_and_envelope_proxies_only')


def _hard(board):
    return frozenset(f.fingerprint for f in run_physical_drc(board).findings
        if f.severity.value=='error' and f.code not in {'DRC-OPEN-NET','DRC-ROUTE-INCOMPLETE','DRC-PLACEMENT'})


def _protected_members(board,poses,placement_options):
    from .placement import _fixed_placements
    from .hard_macros import macro_owned_pads
    from .pad_via_arrays import array_rules
    fixed=set(_fixed_placements(board,poses,placement_options))
    fixed.update(m.reference for c in board.rigid_clusters for m in c.members)
    fixed.update(p.component for p in macro_owned_pads(board))
    fixed.update(r.pad.component for r in array_rules(board))
    return frozenset(fixed)


def _groups(board,scope,protected,options):
    groups=defaultdict(list)
    rules={r.net:r for r in board.net_routing_rules}
    for link in board.decoupling_links:
        if link not in scope or link.capacitor.component in protected:
            continue
        if link.return_pad.component!=link.capacitor.component or link.target.component==link.capacitor.component:
            continue
        if any(rules.get(n) and rules[n].kind is not RouteKind.GENERAL for n in (link.net,link.return_net)):
            continue
        # One rail can feed balls on different package edges. Keep explicit
        # pin-associated banks local; joint selection still couples their poses.
        groups[(link.target,link.net,link.return_net)].append(link)
    result=[];claimed=set()
    for key,links in groups.items():
        # Keep all source roles of one capacitor together, even for shared feeds.
        refs=tuple(dict.fromkeys(l.capacitor.component for l in links))
        for offset in range(0,len(refs),options.maximum_members):
            members=set(refs[offset:offset+options.maximum_members])-claimed
            if not members:continue
            group=tuple(l for l in board.decoupling_links if l.capacitor.component in members)
            if any(l not in scope or (l.target,l.net,l.return_net)!=key for l in group):
                continue  # Unsupported shared multi-domain roles stay explicit/unmoved.
            claimed.update(members);result.append(group)
    return tuple(result)


def _bounds(board,pose):
    from .placement import _placement_polygon
    from .geometry import bounds
    return bounds(_placement_polygon(board,pose))


def _oriented(board,pose,link,normal,side,placement_options):
    from .placement import _allowed_orientations,transformed_pad_position
    candidates=[]
    for angle in _allowed_orientations(board,pose.reference):
        origin=replace(pose,position=Point(0,0),side=side,rotation_degrees=angle)
        a=transformed_pad_position(board,origin,link.capacitor.pad)
        z=transformed_pad_position(board,origin,link.return_pad.pad)
        dx,dy=z.x_nm-a.x_nm,z.y_nm-a.y_nm
        dot=dx*normal[0]+dy*normal[1]
        candidates.append(((dot<=0,abs(dx*normal[1]-dy*normal[0]),-dot,angle),origin))
    return min(candidates,key=lambda c:c[0])[1] if candidates else None


def _bank_offsets(board,row,held,placement_options,options):
    """Translate a complete pattern at nearby courtyard/region support events.

    Exact original relative offsets/orientations stay intact. Bounding boxes
    only propose finite coordinates; ordinary polygon/hole/macro legality is
    still required. No held component or authored placement rule is changed.
    """
    from .physical import component_pair_courtyard_gap_nm
    from .geometry import bounds
    boxes={p.reference:_bounds(board,p) for p in row};rules={r.reference:r for r in board.placement_rules}
    regions={r.name:r for r in board.regions};xs={0};rectangles=[]
    lo_x=lo_y=None;hi_x=hi_y=None;blocked=0
    outline=bounds(board.outline.vertices)
    for p in row:
        b=boxes[p.reference];rule=rules.get(p.reference)
        region=regions.get(rule.region) if rule else None
        edge=rule.edge_clearance_nm if rule and rule.edge_clearance_nm is not None else placement_options.edge_clearance_nm
        low_x,high_x=outline.min_x+edge-b.min_x,outline.max_x-edge-b.max_x
        low_y,high_y=outline.min_y+edge-b.min_y,outline.max_y-edge-b.max_y
        if region is not None:
            rb=bounds(region.outline.vertices)
            low_x,high_x=max(low_x,rb.min_x-b.min_x),min(high_x,rb.max_x-b.max_x)
            low_y,high_y=max(low_y,rb.min_y-b.min_y),min(high_y,rb.max_y-b.max_y)
        lo_x=low_x if lo_x is None else max(lo_x,low_x)
        hi_x=high_x if hi_x is None else min(hi_x,high_x)
        lo_y=low_y if lo_y is None else max(lo_y,low_y)
        hi_y=high_y if hi_y is None else min(hi_y,high_y)
        for q in held.values():
            if q.side is not p.side:continue
            other=_bounds(board,q)
            gap=component_pair_courtyard_gap_nm(board,p.reference,q.reference,
                                               fallback_nm=placement_options.component_clearance_nm)
            ex=(other.min_x-gap-b.max_x,other.max_x+gap-b.min_x)
            ey=(other.min_y-gap-b.max_y,other.max_y+gap-b.min_y)
            rectangles.append((*ex,*ey));xs.update(ex)
            if b.intersects_expanded(other,gap):
                blocked+=1
    if lo_x>hi_x or lo_y>hi_y:return (),blocked
    rectangles=[(l,r,a,b) for l,r,a,b in rectangles if l<hi_x and r>lo_x and a<hi_y and b>lo_y]
    xs={0,lo_x,hi_x,*(x for l,r,a,b in rectangles for x in (l,r))}
    xs=sorted((x for x in xs if lo_x<=x<=hi_x),key=lambda x:(abs(x),x))[:options.offset_sweep_events]
    events=[]
    for x in xs:
        # Configuration-space sweep: derive free y intervals analytically from
        # the union of forbidden translations, rather than probing a Cartesian
        # grid of mostly colliding bank poses. Boundaries are legal contacts;
        # exact rotated polygons, holes and copper are checked by the caller.
        intervals=sorted((a,b) for l,r,a,b in rectangles if l<x<r)
        merged=[]
        for a,b in intervals:
            if merged and a<merged[-1][1]:merged[-1]=(merged[-1][0],max(b,merged[-1][1]))
            else:merged.append((a,b))
        ys={max(lo_y,min(0,hi_y)),lo_y,hi_y}
        ys.update(y for pair in merged for y in pair if lo_y<=y<=hi_y)
        free=sorted((y for y in ys if not any(a<y<b for a,b in merged)),key=lambda y:(abs(y),y))[:options.offset_axis_events]
        events.extend((x,y) for y in free if x or y)
    offsets=sorted(events,
        key=lambda d:(abs(d[0])+abs(d[1]),max(abs(d[0]),abs(d[1])),d))
    return tuple(offsets[:options.offsets_per_template]),blocked


def support_pattern_candidates(board,links,placement_options,*,options=None,work=None,mutable_members=None):
    """Finite group poses from actual courtyard/pin geometry, never part names.

    Complete bank instances include end-on/paired, staggered, split and underside
    pocket families. Geometry proposes; ordinary source legality decides.
    """
    from .placement import transformed_pad_position,transformed_local_point,_allowed_orientations,_legal
    from .physical import component_pair_courtyard_gap_nm
    options=options or SupportPatternOptions()
    work=work if work is not None else {'poses':0}
    if work['poses']>=options.maximum_pose_probes:return ()
    poses={p.reference:p for p in board.placements}
    refs=tuple(dict.fromkeys(l.capacitor.component for l in links))
    if not refs or len(refs)>options.maximum_members:return ()
    if set(refs)&_protected_members(board,poses,placement_options):return ()
    representatives={r:next(l for l in links if l.capacitor.component==r) for r in refs}
    target=poses[links[0].target.component]
    if any(l.target.component!=target.reference for l in links):return ()
    source=physical_board_digest(board);rules={r.reference:r for r in board.placement_rules}
    centre=Point(sum(transformed_pad_position(board,target,l.target.pad).x_nm for l in links)//len(links),
                 sum(transformed_pad_position(board,target,l.target.pad).y_nm for l in links)//len(links))
    normals=sorted(((1,0),(1,1),(0,1),(-1,1),(-1,0),(-1,-1),(0,-1),(1,-1)),key=lambda n:
        -(n[0]*(centre.x_nm-target.position.x_nm)+n[1]*(centre.y_nm-target.position.y_nm)))
    target_bounds=_bounds(board,target)
    # Opposite-face banks can lie inside the front package's courtyard. Its
    # outer land field is a useful extra rim, not a mechanical exclusion.
    from .drc import placed_pad_shape
    from .geometry import Bounds
    land_boxes=[placed_pad_shape(transformed_local_point(target,p.position),p,target).bounds
                for p in board.footprints[target.footprint].pads]
    land_bounds=(Bounds(min(b.min_x for b in land_boxes),min(b.min_y for b in land_boxes),
                        max(b.max_x for b in land_boxes),max(b.max_y for b in land_boxes))
                 if land_boxes else target_bounds)
    candidates=[];seen=set();failed=[]
    families=('paired' if len(refs)==2 else 'edge','staggered','pocket','split','land_rim','source_frame')
    sides=tuple(s for s in dict.fromkeys((poses[refs[0]].side,target.side,
                BoardSide.BACK if target.side is BoardSide.FRONT else BoardSide.FRONT))
                if all(not rules.get(r) or rules[r].side is None or rules[r].side is s for r in refs))
    def retained():
        pools=[[p for p in candidates if p.family==family] for family in families]
        return tuple(p for i in range(max(map(len,pools),default=0)) for pool in pools
                     for p in pool[i:i+1])[:options.patterns_per_group]
    held_source={r:p for r,p in poses.items() if r not in (mutable_members or refs)}
    def legal(row):
        held=dict(held_source)
        for p in row:
            if not _legal(p,held,board,placement_options):return False
            held[p.reference]=p
        return True
    start=work['poses'];nominal_limit=start+max(1,(options.maximum_pose_probes-start)//2)
    def translated():
        offsets_by_pattern={p:_bank_offsets(board,p.poses,held_source,placement_options,options) for p in failed}
        pools=[]
        for family in families:
            seeds=sorted((f for f in failed if f.family==family),key=lambda p:(
                not offsets_by_pattern[p][0],offsets_by_pattern[p][1],
                sum(abs(q.position.x_nm-centre.x_nm)+abs(q.position.y_nm-centre.y_nm) for q in p.poses),p.parameters))[:options.offset_templates_per_family]
            pools.append(seeds)
        seeds=[p for i in range(options.offset_templates_per_family) for pool in pools for p in pool[i:i+1]]
        domains=[offsets_by_pattern[p][0] for p in seeds]
        # Give different families/seeds an offset before trying their next one.
        for i in range(options.offsets_per_template):
            for p,offsets in zip(seeds,domains):
                if i>=len(offsets):continue
                if work['poses']>=options.maximum_pose_probes:return retained()
                dx,dy=offsets[i]
                row=tuple(replace(q,position=Point(q.position.x_nm+dx,q.position.y_nm+dy)) for q in p.poses)
                if row in seen:continue
                seen.add(row);work['poses']+=1
                if legal(row):candidates.append(replace(p,poses=row,parameters=(*p.parameters,('offset_nm',(dx,dy)))))
        return retained()
    # Retain a bank's existing split/pocket frame when no compact template fits.
    # Rotate/translate the complete source frame, preserving each physical role.
    # Legal frames also get offsets: another mutable bank may need their site.
    mx=sum(poses[r].position.x_nm for r in refs)//len(refs)
    my=sum(poses[r].position.y_nm for r in refs)//len(refs)
    for angle in (0,90,180,270):
        if work['poses']>=nominal_limit:return translated()
        row=[]
        for r in refs:
            p=poses[r];rotation=(p.rotation_degrees+angle)%360
            if rotation not in _allowed_orientations(board,r):break
            x,y=p.position.x_nm-mx,p.position.y_nm-my
            x,y=((x,y),(-y,x),(-x,-y),(y,-x))[angle//90]
            row.append(replace(p,position=Point(mx+x,my+y),rotation_degrees=rotation))
        else:
            row=tuple(sorted(row,key=lambda p:p.reference));work['poses']+=1;seen.add(row)
            pattern=SupportPattern(source,'source_frame',row,tuple(links),(('rotation',angle),))
            failed.append(pattern)
            if row!=tuple(sorted((poses[r] for r in refs),key=lambda p:p.reference)) and legal(row):candidates.append(pattern)
    # Interleave families/sides before deeper offsets so one easy family cannot
    # monopolize the retained cap. Incumbent is supplied by the caller.
    for depth,shift in ((0,0),(1,0),(0,-1),(0,1),(1,-1),(1,1)):
        for family in families[:-1]:
            for side in sides:
                if family=='land_rim' and side is target.side:continue
                for normal in normals:
                    if work['poses']>=nominal_limit:return translated()
                    work['poses']+=1
                    tangent=(-normal[1],normal[0]);oriented={};valid=True
                    for ref in refs:
                        rule=rules.get(ref)
                        if rule and rule.side is not None and rule.side is not side:valid=False;break
                        oriented[ref]=_oriented(board,poses[ref],representatives[ref],normal,side,placement_options)
                        if oriented[ref] is None:valid=False;break
                    if not valid:continue
                    boxes={r:_bounds(board,p) for r,p in oriented.items()}
                    span=lambda b,n:max(b.min_x*n[0],b.max_x*n[0])+max(b.min_y*n[1],b.max_y*n[1])
                    gap=max((component_pair_courtyard_gap_nm(board,a,b,fallback_nm=placement_options.component_clearance_nm)
                            for a in refs for b in refs if a!=b),default=0)
                    edge_gap=max(component_pair_courtyard_gap_nm(board,a,target.reference,
                        fallback_nm=placement_options.component_clearance_nm) for a in refs) if side is target.side else 0
                    norm2=normal[0]**2+normal[1]**2
                    projected_gap=isqrt(gap*gap*norm2)
                    if projected_gap*projected_gap<gap*gap*norm2:projected_gap+=1
                    projected_edge_gap=isqrt(edge_gap*edge_gap*norm2)
                    if projected_edge_gap*projected_edge_gap<edge_gap*edge_gap*norm2:projected_edge_gap+=1
                    pitch=max(span(b,tangent)+span(b,(-tangent[0],-tangent[1])) for b in boxes.values())+projected_gap
                    normal_pitch=max(span(b,normal)+span(b,(-normal[0],-normal[1])) for b in boxes.values())+projected_gap
                    ordered=sorted(refs,key=lambda r:(
                        tangent[0]*transformed_pad_position(board,target,representatives[r].target.pad).x_nm+
                        tangent[1]*transformed_pad_position(board,target,representatives[r].target.pad).y_nm,r))
                    edge=span(land_bounds if family=='land_rim' else target_bounds,normal)
                    ct=centre.x_nm*tangent[0]+centre.y_nm*tangent[1]
                    row=[]
                    for i,ref in enumerate(ordered):
                        origin=oriented[ref];n=normal;t=tangent
                        q=ct+((2*i-len(refs)+1)*pitch)//2+shift*pitch
                        nn=edge+span(boxes[ref],(-normal[0],-normal[1]))+projected_edge_gap+depth*normal_pitch
                        if family=='staggered':nn+=(i%2)*normal_pitch
                        elif family=='pocket':
                            if side is target.side:valid=False;break
                            nn=centre.x_nm*n[0]+centre.y_nm*n[1]+depth*normal_pitch
                        elif family=='split' and i>=len(refs)//2:
                            n=tangent;t=(-n[1],n[0])
                            origin=_oriented(board,poses[ref],representatives[ref],n,side,placement_options)
                            b=_bounds(board,origin)
                            nn=span(target_bounds,n)+span(b,(-n[0],-n[1]))+projected_edge_gap+depth*normal_pitch
                            q=centre.x_nm*t[0]+centre.y_nm*t[1]+shift*pitch
                        row.append(replace(origin,position=Point((n[0]*nn+t[0]*q)//norm2,(n[1]*nn+t[1]*q)//norm2)))
                    if not valid:continue
                    row=tuple(sorted(row,key=lambda p:p.reference))
                    if row in seen:continue
                    seen.add(row)
                    pattern=SupportPattern(source,family,row,tuple(links),
                        (('normal',normal),('side',side.value),('depth',depth),('shift',shift)))
                    if legal(row):candidates.append(pattern)
                    else:failed.append(pattern)  # Relative rules run on complete assignments.
    return translated()


def _paths(board,links,maximum_states):
    from .decoupling_paths import _find_path,_return_targets
    from .routing_clearance import RoutingClearanceIndex
    index=RoutingClearanceIndex(board);verified={}
    for link in links:
        supply=_find_path(board,link.target,(link.capacitor,),link.net,index,maximum_states)
        returned=_find_path(board,link.return_pad,_return_targets(board,link),link.return_net,index,maximum_states)
        if supply is not None and returned is not None:verified[link]=(supply,returned)
    return verified


def _route(board,links,fanout_options,options):
    """Normal source-associated routing plus checked same-face return paths."""
    from .decoupling import route_decouplers
    from .decoupling_paths import _land,_find_path,_return_targets,_budget_reason
    from .pin_escape import checked_access_paths
    from .routing_clearance import RoutingClearanceIndex
    routed=route_decouplers(board,only_links=frozenset(links),fanout_options=fanout_options).board
    for link in links:
        index=RoutingClearanceIndex(routed);targets=_return_targets(routed,link)
        if _find_path(routed,link.return_pad,targets,link.return_net,index,options.maximum_path_states):continue
        a=_land(routed,link.return_pad)
        if a is None:continue
        targets=sorted((t for t in targets if _land(routed,t)),key=lambda t:(
            abs(_land(routed,t)[2].position.x_nm-a[2].position.x_nm)+
            abs(_land(routed,t)[2].position.y_nm-a[2].position.y_nm),t))
        rule=next((r for r in board.net_routing_rules if r.net==link.return_net),None)
        if rule and rule.kind is not RouteKind.GENERAL:continue
        width=rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
        from .routing_layers import routing_layers
        if a[2].layer not in routing_layers(board,link.return_net,rule):continue
        for target in targets[:options.maximum_return_targets]:
            b=_land(routed,target)
            if a[2].layer is not b[2].layer:continue
            for path in checked_access_paths(routed,index,link.return_net,a[2].position,b[2].position,width,a[2].layer):
                candidate=replace(routed,tracks=(*routed.tracks,*(t for t in path if t not in routed.tracks)))
                if not _budget_reason(candidate,link.return_net):routed=candidate;break
            if _find_path(routed,link.return_pad,targets,link.return_net,RoutingClearanceIndex(routed),options.maximum_path_states):break
    return routed


def _port_paths(board,ports):
    from .pin_escape import verified_fanout_path,verified_routing_access
    from .routing_clearance import RoutingClearanceIndex
    membership={p:n.name for n in board.nets for p in n.pads};index=RoutingClearanceIndex(board)
    return all(p in membership and (verified_fanout_path if isinstance(a,Point) else verified_routing_access)(
        board,p,membership[p],a,index) is not None for p,a in ports)


def _score(board,links,verified):
    from .geometry import bounds
    from .placement import _placement_polygon
    refs={l.capacitor.component for l in links};area=0
    edges={edge for pair in verified.values() for path in pair for edge in path.tracks}
    vias={edge for pair in verified.values() for path in pair for edge in path.vias}
    for side in BoardSide:
        layer=CopperLayer.FRONT if side is BoardSide.FRONT else CopperLayer.BACK
        boxes=[bounds(_placement_polygon(board,p)) for p in board.placements if p.reference in refs and p.side is side]
        points=[(b.min_x,b.min_y) for b in boxes]+[(b.max_x,b.max_y) for b in boxes]
        points.extend((p.x_nm+d,p.y_nm+d) for t in edges if t.layer is layer for p in (t.start,t.end)
                      for d in (-(t.width_nm+1)//2,(t.width_nm+1)//2))
        points.extend((v.position.x_nm+d,v.position.y_nm+d) for v in vias
                      for d in (-(v.size_nm+1)//2,(v.size_nm+1)//2))
        if points:area+=(max(x for x,y in points)-min(x for x,y in points))*(max(y for x,y in points)-min(y for x,y in points))
    length=sum(isqrt((t.end.x_nm-t.start.x_nm)**2+(t.end.y_nm-t.start.y_nm)**2) for t in edges)
    return (len(links)-len(verified),length,len(vias),area)


def _ordinary_scope(board):
    """Nets that are neither power domains, zone nets nor specialised (non-general) routes."""
    from .physical import RouteKind
    protected = {d.net for d in board.power_domains} | {z.net for z in board.zones}
    protected.update(r.net for r in board.net_routing_rules if r.kind is not RouteKind.GENERAL)
    return frozenset(n.name for n in board.nets if n.name not in protected)


def improve_support_placement(board,placements,placement_options,*,options=None,fanout_options=None,
                              only_links=None,reserved_tracks=(),reserved_vias=(),ports=()):
    """Joint finite bank search; return unrouted poses and separate proof copper.

    Caller reservations are fixed actual copper, freshly checked with all ports.
    Otherwise reserve ordinary exits before routing local supply/return feeds.
    Scope filters never delete original declarations or inventory. Outside-scope
    associations are unobserved, never reported verified by this scoped probe.
    """
    from .hard_macros import materialize_hard_macros
    from .placement import placement_solution_is_legal,_legal
    from .fanout import FanoutOptions,route_fanout
    from .copper_reservations import reserve_copper_scene
    from .bypass_links import verified_bypass_links
    from .escape_assignment import _finite_assignment,_BudgetExceeded
    options=options or SupportPatternOptions();fanout_options=fanout_options or FanoutOptions()
    if board.tracks or board.vias or board.zone_fills:raise ValueError('support placement requires unrouted unfilled source')
    if set(placements)!={p.reference for p in board.placements}:raise ValueError('complete source pose inventory required')
    if any((placements[p.reference].reference,placements[p.reference].footprint)!=(p.reference,p.footprint)
           for p in board.placements):raise ValueError('source component/footprint identities required')
    if only_links is not None and (not isinstance(only_links,frozenset) or not only_links<=frozenset(board.decoupling_links)):
        raise ValueError('declared immutable bypass subset required')
    source=replace(board,placements=tuple(placements[p.reference] for p in board.placements))
    if not placement_solution_is_legal(board,placements,placement_options):raise ValueError('legal source poses required')
    digest=physical_board_digest(source);scope=tuple(l for l in source.decoupling_links if only_links is None or l in only_links)
    membership={p:n.name for n in source.nets for p in n.pads}
    if any(membership.get(l.target)!=l.net or membership.get(l.capacitor)!=l.net
           or membership.get(l.return_pad)!=l.return_net for l in scope):
        raise ValueError('source bypass pad/net associations required')
    protected=_protected_members(source,placements,placement_options)
    groups=_groups(source,frozenset(scope),protected,options);work={'poses':0,'states':0,'pairs':0,'checks':0,'routes':0}
    trials=[];baseline_verified=frozenset();ports=tuple(ports);best=None;frontier=[]
    reservation_report=dict(origin='provided' if reserved_tracks or reserved_vias or ports else 'not_run',
                            tracks=len(reserved_tracks),vias=len(reserved_vias))
    def finish(status):
        return SupportPlacementResult(source,status,digest,ports=ports,trials=tuple(trials),scope=scope,
            protected_members=tuple(sorted({l.capacitor.component for l in scope}&protected)),
            baseline_verified=tuple(l for l in scope if l in baseline_verified),
            verified=tuple(l for l in scope if l in baseline_verified),pose_probes=work['poses'],
            search_states=work['states'],pair_checks=work['pairs'],complete_checks=work['checks'],
            routing_passes=work['routes'],frontier=tuple(frontier),reservation_report=reservation_report,
            limits={**{n:getattr(options,n) for n in options.__dataclass_fields__},
                    'routing_passes':options.maximum_complete_checks+2,
                    'per_link_launches':8,'per_connection_sites':64,'per_link_complete_proposals':64})
    if not groups:return finish('fixed_or_unsupported' if scope else 'not_requested')
    base=materialize_hard_macros(source)
    if any(v.net in {n for l in scope for n in (l.net,l.return_net)} for v in (*reserved_tracks,*reserved_vias)):
        raise ValueError('ordinary reservation overlaps support producer net')
    if not reserved_tracks and not reserved_vias and not ports:
        excluded={n for l in source.decoupling_links for n in (l.net,l.return_net)}
        ordinary=_ordinary_scope(base)-excluded
        if ordinary:
            fanout=route_fanout(base,replace(fanout_options,prefer_surface_escapes=True,
                surface_preference_depth=1,surface_fallback=True),only_nets=ordinary)
            reserved_tracks=fanout.board.tracks[len(base.tracks):];reserved_vias=fanout.board.vias[len(base.vias):]
            ports=tuple(fanout.routing_accesses.items())
            assignment=fanout.assignment
            reservation_report=dict(origin='early_ordinary_fanout',tracks=len(reserved_tracks),vias=len(reserved_vias),
                pending=[str(p) for p in fanout.pending_pads],
                assignment=dict(pair_checks=assignment.pair_checks,pair_queries=assignment.pair_queries,
                    search_states=sum(t.search_states for t in assignment.trials),
                    expanded_pads=[str(p) for p in assignment.expanded_pads],
                    trials=[dict(pad=str(t.pad),search_states=t.search_states,diagnostic=t.diagnostic)
                            for t in assignment.trials]) if assignment else None,
                unobserved_phases=sorted({phase for p in fanout.pin_analysis if p.access_diagnostics
                                          for phase in p.access_diagnostics.unobserved_phases}))
    initial=reserve_copper_scene(base,reserved_tracks,reserved_vias)
    if not _port_paths(initial.board,ports):raise ValueError('fresh ordinary reservation witnesses required')
    hard=_hard(initial.board)
    def route_trial(scene):
        # Route the supports for this complete pose assignment afresh. No old
        # support copper or candidate plan is carried across a placement change.
        work['routes']+=1
        return _route(scene,scope,fanout_options,options)
    def completed(routed):
        local=_paths(routed,scope,options.maximum_path_states)
        return local,frozenset(local)
    baseline=route_trial(initial.board)
    baseline_verified=verified_bypass_links(baseline,{l.net for l in scope})&frozenset(scope)
    baseline_local,baseline_complete=completed(baseline)
    baseline_score=_score(baseline,scope,baseline_local)
    # Spend the finite group allowance on unresolved associations first, not
    # merely the earliest source declarations on an already working rail.
    groups=tuple(sorted(groups,key=lambda ls:(-sum(l not in baseline_complete for l in ls),
        -len({l.capacitor.component for l in ls}),source.decoupling_links.index(ls[0])))[:options.maximum_groups])
    domains={}
    mutable_members={l.capacitor.component for group in groups for l in group}
    for i,links in enumerate(groups):
        refs={l.capacitor.component for l in links}
        current=SupportPattern(digest,'incumbent',tuple(p for p in source.placements if p.reference in refs),links)
        allowance=(options.maximum_pose_probes-work['poses'])//(len(groups)-i)
        bounded=replace(options,maximum_pose_probes=max(1,work['poses']+allowance))
        generated=support_pattern_candidates(source,links,placement_options,options=bounded,
            work=work,mutable_members=mutable_members)
        valid=[]
        for pattern in generated[:options.patterns_per_group]:
            if (pattern.source_digest!=digest or pattern.links!=links
                    or {p.reference for p in pattern.poses}!=refs
                    or any(p.footprint!=placements[p.reference].footprint for p in pattern.poses)):
                trials.append(dict(status='stale_or_incomplete_pattern',family=pattern.family))
                continue
            valid.append(pattern)
        domains[i]=(current,*valid)
    pair_cache={}
    def compatible(a,i,b,j):
        key=(a,i,b,j)
        if key not in pair_cache:
            if work['pairs']>=options.maximum_pair_checks:raise _BudgetExceeded('pair_budget')
            work['pairs']+=1;ps=(*domains[a][i].poses,*domains[b][j].poses)
            held={p.reference:p for p in ps};valid=True
            for p in ps:
                if not _legal(p,{r:q for r,q in held.items() if r!=p.reference},source,placement_options):valid=False;break
            pair_cache[key]=valid
        return pair_cache[key]
    def consume():
        if work['states']>=options.maximum_search_states:raise _BudgetExceeded('search_budget')
        work['states']+=1
    def complete(assignment):
        nonlocal best,frontier
        selected=tuple(domains[g][i] for g,i in sorted(assignment.items()))
        if all(p.family=='incumbent' for p in selected):return False
        if work['checks']>=options.maximum_complete_checks:raise _BudgetExceeded('complete_check_budget')
        work['checks']+=1
        changed={q.reference:q for p in selected for q in p.poses}
        moved={r for r,p in changed.items() if p!=placements[r]}
        poses={**placements,**changed}
        trial=dict(families=[p.family for p in selected],members=sorted(moved),status='placement_rejected')
        trials.append(trial)
        if not moved or not placement_solution_is_legal(board,poses,placement_options):return False
        new=materialize_hard_macros(replace(source,placements=tuple(poses[p.reference] for p in source.placements)))
        scene=reserve_copper_scene(new,reserved_tracks,reserved_vias)
        if _hard(scene.board)-hard or not _port_paths(scene.board,ports):trial['status']='reserved_exit_or_copper_blocked';return False
        routed=route_trial(scene.board)
        verified=verified_bypass_links(routed,{l.net for l in scope})&frozenset(scope)
        local,complete_links=completed(routed)
        required={l for l in scope if l.capacitor.component in moved}
        if (not baseline_verified<=verified or not set(baseline_local)<=set(local)
                or not baseline_complete<=complete_links or not required<=complete_links):
            trial['status']='supply_or_return_unresolved';return False
        if _hard(routed)-hard or not _port_paths(routed,ports):trial['status']='combined_geometry_rejected';return False
        score=_score(routed,scope,local);trial.update(status='verified_geometry',score=score)
        if score>=baseline_score:return False
        if best is None or score<best[0]:best=(score,poses,selected,
            scene.extract(routed),routed,tuple(complete_links))
        if not any(all(x<=y for x,y in zip(row['score'],score)) for row in frontier):
            frontier=[row for row in frontier if not all(x<=y for x,y in zip(score,row['score']))]
            frontier.append(dict(score=score,families=trial['families']));frontier=sorted(frontier,key=lambda row:row['score'])[:options.frontier_size]
        return False  # Continue within the SAME global work budget to keep a frontier.
    try:
        _finite_assignment({g:tuple(range(len(values))) for g,values in domains.items()},{},
            preferred={g:0 for g in domains},rank={g:g for g in domains},compatible=compatible,
            consume_state=consume,accept_complete=complete)
        status='finite_pattern_pool_unresolved'
        if work['checks']>=options.maximum_complete_checks:status='complete_check_budget'
    except _BudgetExceeded as error:status=str(error)
    if best is None:return finish(status)
    score,poses,selected,proposal,signal_scene,complete_links=best
    # Rebuild the selected placement from clean source once more; no cached
    # paths or stored connectivity flags authorize the returned pose result.
    final=replace(source,placements=tuple(poses[p.reference] for p in source.placements))
    fresh=reserve_copper_scene(materialize_hard_macros(final),reserved_tracks,reserved_vias)
    regenerated=route_trial(fresh.board)
    local,proved=completed(regenerated)
    verified=verified_bypass_links(regenerated,{l.net for l in scope})&frozenset(scope)
    if (not baseline_verified<=verified
            or not set(baseline_local)<=set(local) or not set(complete_links)<=set(proved)
            or _hard(regenerated)-hard or not _port_paths(regenerated,ports)):
        trials.append(dict(status='final_regeneration_rejected'));return finish('final_regeneration_rejected')
    result=finish('geometry_improved')
    return replace(result,board=final,proposal=fresh.extract(regenerated),signal_scene=regenerated,
        selected=selected,verified=tuple(l for l in scope if l in proved),routing_passes=work['routes'])

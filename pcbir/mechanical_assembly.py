"""Component-owned mechanical exceptions, never electrical/copper DRC waivers."""
from decimal import Decimal
from functools import lru_cache

from .physical import (AssemblyAccess, AssemblyEnvelope, BodyOverhang, BoardOutline,
                       BoardSide, ComponentHeight, PadKind, Point)


@lru_cache(maxsize=128)
def expanded_body_outline(outline, edges, allowance):
    """Exact exterior rectangular allowance on a named axis-aligned straight edge.

    Only the body's containment query sees this expansion. Actual board material,
    internal voids, holes, copper and export always use the original outline.
    Diagonal/curved edge allowances fail until exact union predicates support them.
    """
    edge=next((e for e in edges if e.id==allowance.edge),None)
    if edge is None or outline.circular_boundary:
        raise ValueError('overhang requires an existing named straight boundary edge')
    dx,dy=edge.end.x_nm-edge.start.x_nm,edge.end.y_nm-edge.start.y_nm
    if dx and dy:
        raise ValueError('body overhang currently requires an axis-aligned edge')
    length=abs(dx)+abs(dy)
    if allowance.end_nm > length:
        raise ValueError('body overhang interval exceeds its edge')
    ring=outline.vertices
    pairs=tuple(zip(ring,(*ring[1:],ring[0])))
    if (edge.start,edge.end) not in pairs and (edge.end,edge.start) not in pairs:
        raise ValueError('body overhang edge was removed from the outline')
    forward=(edge.start,edge.end) in pairs
    winding=1 if sum(a.x_nm*b.y_nm-b.x_nm*a.y_nm for a,b in pairs)>0 else -1
    ux,uy=dx//length,dy//length
    outward=winding*(1 if forward else -1)
    normal=Point(uy*outward*allowance.distance_nm,-ux*outward*allowance.distance_nm)
    a=Point(edge.start.x_nm+ux*allowance.start_nm,edge.start.y_nm+uy*allowance.start_nm)
    b=Point(edge.start.x_nm+ux*allowance.end_nm,edge.start.y_nm+uy*allowance.end_nm)
    bump=(a,Point(a.x_nm+normal.x_nm,a.y_nm+normal.y_nm),Point(b.x_nm+normal.x_nm,b.y_nm+normal.y_nm),b)
    if not forward:bump=tuple(reversed(bump))
    expanded=[]
    for start,end in pairs:
        expanded.append(start)
        if {start,end}=={edge.start,edge.end}:expanded.extend(bump)
    # Collapse consecutive duplicate/collinear points without changing material.
    simple=[]
    for p in expanded:
        if not simple or simple[-1]!=p:simple.append(p)
    if simple[-1]==simple[0]:simple.pop()
    changed=True
    while changed:
        changed=False
        for i,p in enumerate(simple):
            a,b=simple[i-1],simple[(i+1)%len(simple)]
            if (p.x_nm-a.x_nm)*(b.y_nm-p.y_nm)==(p.y_nm-a.y_nm)*(b.x_nm-p.x_nm):
                simple.pop(i);changed=True;break
    return BoardOutline(tuple(simple),cutouts=outline.cutouts)


def lower_assembly(items, outline, edges, point, length):
    overhangs,heights,enclosures,access=[],[],[],[]
    for item in items:
        p=item.parameters
        if item.kind=='overhang':
            if set(p)!={'component','edge','start','end','distance','reason'}:
                raise ValueError('overhang requires exactly component, edge, start, end, distance and reason')
            policy=BodyOverhang(item.name,p['component'],p['edge'],length(p['start']),length(p['end']),length(p['distance']),p['reason'])
            expanded_body_outline(outline,edges,policy)
            overhangs.append(policy)
        elif item.kind=='component_height':
            if set(p)!={'component','height'}:raise ValueError('component_height requires component and height')
            heights.append(ComponentHeight(item.name,p['component'],length(p['height'])))
        else:
            keys={'side','maximum_height'} if item.kind=='enclosure' else {'component','side','purpose'}
            if item.shape=='rectangle':
                required=keys|{'width','height'};allowed=required|{'origin'}
                if set(p)-allowed or required-set(p):raise ValueError('assembly rectangle has missing or unknown properties')
                origin=point(p['origin']) if 'origin' in p else Point(0,0)
                region=BoardOutline.rectangle(Decimal(length(p['width']))/1000000,Decimal(length(p['height']))/1000000,origin=origin)
            elif item.shape=='polygon':
                if set(p)!=keys|{'vertices'}:raise ValueError('assembly polygon has missing or unknown properties')
                region=BoardOutline(tuple(point(v) for v in p['vertices']))
            else:raise ValueError('assembly regions require rectangle or polygon geometry')
            if item.kind=='enclosure':enclosures.append(AssemblyEnvelope(item.name,region,BoardSide(p['side']),length(p['maximum_height'])))
            else:access.append(AssemblyAccess(item.name,p['component'],region,p['side'],p['purpose']))
    for policies in (overhangs,heights,access):
        if len({p.reference for p in policies})!=len(policies):raise ValueError('conflicting assembly component ownership')
    return tuple(overhangs),tuple(heights),tuple(enclosures),tuple(access)


def component_height(board,pose):
    declared=next((h.height_nm for h in board.component_heights if h.reference==pose.reference),None)
    return declared if declared is not None else board.footprints[pose.footprint].height_nm


def body_in_material(board,pose,shape,clearance):
    from .mechanical import shape_in_board,shape_in_outline,hole_shape,slot_shape
    from .geometry import RoundedConvexShape,shapes_clear
    policy=next((a for a in board.body_overhangs if a.reference==pose.reference),None)
    if policy is None:return shape_in_board(board,shape,clearance)
    permitted=expanded_body_outline(board.outline,board.boundary_edges,policy)
    # No clearance at the audited outer body limit; all void/hole rules survive.
    if not shape_in_outline(shape,permitted,0):return False
    edge=next(e for e in board.boundary_edges if e.id==policy.edge)
    length=abs(edge.end.x_nm-edge.start.x_nm)+abs(edge.end.y_nm-edge.start.y_nm)
    ux,uy=(edge.end.x_nm-edge.start.x_nm)//length,(edge.end.y_nm-edge.start.y_nm)//length
    a=Point(edge.start.x_nm+ux*policy.start_nm,edge.start.y_nm+uy*policy.start_nm)
    b=Point(edge.start.x_nm+ux*policy.end_nm,edge.start.y_nm+uy*policy.end_nm)
    from .mechanical import ring_edges
    unaffected=[(s,e) for s,e in ring_edges(board.outline.vertices) if {s,e}!={edge.start,edge.end}]
    if a!=edge.start:unaffected.append((edge.start,a))
    if b!=edge.end:unaffected.append((b,edge.end))
    if any(not shapes_clear(shape,RoundedConvexShape((s,e)),clearance) for s,e in unaffected):return False
    if any(not shapes_clear(shape,RoundedConvexShape(c.vertices),max(1,clearance)) for c in board.outline.cutouts):return False
    return all(shapes_clear(shape,hole_shape(h),1) for h in board.mechanical_holes) and all(shapes_clear(shape,slot_shape(s),1) for s in board.mechanical_slots)


def assembly_pose_legal(board,pose,polygon,placed):
    from .geometry import RoundedConvexShape,shapes_clear
    from .placement import transformed_local_point,transformed_footprint_polygon,_polygons_too_close
    from .drc import placed_pad_shape
    from .mechanical import shape_in_board
    footprint=board.footprints[pose.footprint]
    if any(a.reference==pose.reference for a in board.body_overhangs):
        for pad in footprint.pads:
            if pad.kind is PadKind.APERTURE:continue
            position=transformed_local_point(pose,pad.position)
            if not shape_in_board(board,placed_pad_shape(position,pad,pose),board.rules.minimum_clearance_nm):return False
    height=component_height(board,pose)
    for enclosure in board.assembly_envelopes:
        if enclosure.side is pose.side and _polygons_too_close(polygon,enclosure.outline.vertices,0):
            if height is None or height>enclosure.maximum_height_nm:return False
    poses={**placed,pose.reference:pose}
    for region in board.assembly_access:
        owner=poses.get(region.reference)
        if owner is None:continue
        side=owner.side if region.side=='component' else (BoardSide.BACK if owner.side is BoardSide.FRONT else BoardSide.FRONT)
        reserved=RoundedConvexShape(tuple(transformed_local_point(owner,p) for p in region.outline.vertices))
        for other in poses.values():
            if other.reference!=owner.reference and other.side is side:
                if not shapes_clear(reserved,RoundedConvexShape(transformed_footprint_polygon(board,other)),1):return False
    return True

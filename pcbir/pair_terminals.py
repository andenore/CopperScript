"""Source terminal preparation for bounded exact differential searches.

Coarse access is a routing heuristic, not a certificate. These physical terminal
records have no global grid node and never make a failed global guide connected.
Short same-package branches are returned as disposable candidate copper; only
the complete pair's ordinary atomic geometry/connectivity gates may adopt them.
"""
from dataclasses import dataclass, replace
from itertools import permutations
from .physical import BoardSide, CopperLayer, PadReference, Point, TrackSegment, Via
from .placement import transformed_pad_position
from .routing import GlobalNetRoute
from .routing_clearance import RoutingClearanceIndex
from .pin_escape import checked_access_path
from .pin_access import local_access_path

@dataclass(frozen=True, slots=True)
class PairTerminal:
    pad: PadReference
    pad_position: Point
    access_position: Point
    layer: CopperLayer
    via: Via | None = None
    region_only: bool = True
    tracks: tuple[TrackSegment, ...] = ()

def terminal_partners(first, second):
    """Pair two ends geometrically while preserving every original pad identity.

    Shared packages take precedence, then closest physical endpoint pairing.
    This chooses a spine orientation, never changes the electrical net partition.
    """
    if len(first.accesses)!=2 or len(second.accesses)!=2:
        return None
    if any(len({a.pad.component for a in g.accesses})!=2 for g in (first, second)):
        return None
    choices=[]
    for order in permutations(second.accesses):
        score=(sum(a.pad.component!=b.pad.component for a,b in zip(first.accesses,order)),
            sum((a.pad_position.x_nm-b.pad_position.x_nm)**2+(a.pad_position.y_nm-b.pad_position.y_nm)**2 for a,b in zip(first.accesses,order)),
            tuple(str(b.pad) for b in order))
        choices.append((score,order))
    _,order=min(choices,key=lambda item:item[0])
    return {a.pad.component:b for a,b in zip(first.accesses,order)}

def source_terminal_guide(board, name, existing):
    net=next((n for n in board.nets if n.name==name),None)
    if net is None: return existing
    poses={p.reference:p for p in board.placements}
    accesses=[]
    for ref in net.pads:
        pose=poses.get(ref.component)
        if pose is None: return existing
        lands=[p for p in board.footprints[pose.footprint].pads if p.number==ref.pad]
        if len(lands)!=1: return existing  # same-number multi-land inventory is not guessed
        pos=transformed_pad_position(board,pose,ref.pad)
        layer=CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
        accesses.append(PairTerminal(ref,pos,pos,layer))
    if existing is not None and existing.connected and {a.pad for a in existing.accesses}==set(net.pads):
        return existing
    return GlobalNetRoute(name,False,tuple(accesses),(),(),0,
        ('source terminals for exact search; no connected coarse guide',))

def prepare_pair_terminals(board, first, second, routes):
    from .hard_macros import macro_owned_pads
    owned = macro_owned_pads(board)
    if any(p in owned for n in board.nets if n.name in (first.net, second.net) for p in n.pads):
        return routes, (), ()  # keep prescribed macro ports and their existing access contract
    guides=[source_terminal_guide(board,r.net,routes.get(r.net)) for r in (first,second)]
    if any(g is None for g in guides): return routes,(),()
    all_branches=[]
    normalized=[]
    from .pair_search import _PairAccessIndex
    index=_PairAccessIndex(board)
    for rule,guide in zip((first,second),guides):
        groups={}
        for a in guide.accesses: groups.setdefault(a.pad.component,[]).append(a)
        if len(groups)!=2 or sum(len(v)-1 for v in groups.values())>8:
            return routes,(),()  # fixed bound on disposable local branches
        anchors=[]
        branches=[]
        for component,terminals in sorted(groups.items()):
            anchor=min(terminals,key=lambda a:str(a.pad))
            anchors.append(anchor)
            for other in sorted(terminals,key=lambda a:str(a.pad)):
                if other is anchor: continue
                # A local contact tie is allowed only inside a source-declared
                # breakout radius. No remote branch is inferred or silently omitted.
                limit=rule.breakout_length_nm
                if (limit is None or anchor.layer is not other.layer or
                    (anchor.pad_position.x_nm-other.pad_position.x_nm)**2+
                    (anchor.pad_position.y_nm-other.pad_position.y_nm)**2>limit**2):
                    return routes,(),()
                width=rule.width_nm or board.rules.default_track_width_nm
                path=checked_access_path(board,index,rule.net,anchor.pad_position,other.pad_position,width,anchor.layer)
                if path is None:
                    path=local_access_path(board,index,rule.net,anchor.pad_position,other.pad_position,anchor.layer,width,
                        step_nm=125000,detour_nm=limit,maximum_states=600)
                if path is None: return routes,(),()
                for track in path: index.add_track(track,locked=True)
                branches.extend(path)
        normalized.append(replace(guide,accesses=tuple(anchors),connected=guide.connected and len(guide.accesses)==2))
        all_branches.append(tuple(branches))
    if terminal_partners(*normalized) is None: return routes,(),()
    return {**routes,first.net:normalized[0],second.net:normalized[1]},*all_branches

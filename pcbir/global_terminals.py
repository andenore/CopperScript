"""Fresh exact terminal launch witnesses for the coarse capacity graph."""
from dataclasses import replace
from types import MappingProxyType
from .physical import Via
from .routing_clearance import RoutingClearanceIndex

MAX_TERMINAL_CAPACITY_PROPOSALS = 4096

def register_terminal_capacity(board, graph, access_options):
    from .routing import _via_resource_id, _Resource, GridNode, _edge_key
    sites = dict(graph.via_sites)
    index = RoutingClearanceIndex(board)
    # Existing sampled slots remain in their original order. They are capacity
    # witnesses, never committed board copper; all new sites must coexist.
    seen = set()
    for identifier, positions in sorted(sites.items()):
        _,low,high,_,_ = identifier.split(':')
        span = (graph.layers[int(low)],graph.layers[int(high)])
        for point in positions:
            key=(span,point)
            if key not in seen:
                seen.add(key)
                index.add_via(Via('<capacity-'+str(len(seen))+'>',point,board.rules.default_via_size_nm,
                    board.rules.default_via_drill_nm,*span),locked=True)
    accepted = {}
    proposals = 0
    changed = set()
    for key, choices in sorted(access_options.items(),key=lambda item:(item[0][0],str(item[0][1]))):
        held=[]
        unregistered=[]
        for access in choices:
            via=access.via
            if via is None:
                held.append(access)
                continue
            identifier=_via_resource_id(graph,access.node,via.from_layer,via.to_layer)
            _,low,high,x,y=identifier.split(':')
            nodes=(GridNode(int(low),int(x),int(y)),GridNode(int(high),int(x),int(y)))
            # A choice without registered capacity is dropped, unless that
            # would leave the pad with no choice at all: a pad without choices
            # makes the whole net unreachable (CopperAssetTracker regressed
            # this way). Detailed routing checks exact copper either way.
            if any(node not in graph.legal_nodes for node in nodes):
                unregistered.append(access)
                continue
            positions=sites.get(identifier,())
            if via.position not in positions:
                if proposals >= MAX_TERMINAL_CAPACITY_PROPOSALS:
                    unregistered.append(access)
                    continue
                proposals+=1
                capacity_net='<capacity-proposal-'+str(proposals)+'>'
                if not index.can_via(capacity_net,via.position,via.size_nm,
                        via.from_layer,via.to_layer,via.drill_nm,check_hole_copper=True):
                    unregistered.append(access)
                    continue
                sites[identifier]=(*positions,via.position)
                index.add_via(replace(via,net=capacity_net),locked=True)
                changed.add(identifier)
            held.append(access)
        accepted[key]=tuple(held or unregistered)
    resources=dict(graph.resources)
    for identifier in sorted(changed):
        _,low,high,x,y=identifier.split(':')
        low,high,x,y=map(int,(low,high,x,y))
        capacity=len(sites[identifier])
        # A through launch contacts all declared copper layers in its span.
        # Only already-existing legal graph nodes receive alias edges.
        for first in range(low,high):
            for second in range(first+1,high+1):
                a,b=GridNode(first,x,y),GridNode(second,x,y)
                if a in graph.legal_nodes and b in graph.legal_nodes:
                    resources[_edge_key(a,b)]=_Resource(identifier,a,b,capacity,None)
    return replace(graph,via_sites=MappingProxyType(sites),resources=MappingProxyType(resources)),accepted

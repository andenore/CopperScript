"""Fresh exact terminal launch witnesses for the coarse capacity graph."""
from collections import defaultdict
from dataclasses import replace
from types import MappingProxyType
from .physical import Via
from .routing_clearance import RoutingClearanceIndex

MAX_TERMINAL_CAPACITY_PROPOSALS = 4096
# Broad-phase reach for sampled witnesses that may collide with a terminal
# launch, beyond both via radii. The exact pairwise check decides.
_WITNESS_REACH_NM = 2_000_000


def register_terminal_capacity(board, graph, access_options, expand=None):
    """Register exact terminal launches as capacity sites of the coarse graph.

    Sampled via sites are capacity witnesses, never copper a route will place.
    A terminal launch is real copper at its pad. When it collides only with
    sampled witnesses, it is registered and displaces them: those tiles lose
    that capacity, so every remaining site still coexists with every other.
    Board copper, an accepted terminal slot or a witness an accepted launch
    already sits on still rejects it. Without displacement a pad whose only
    launches lie near sampled sites would lose every access.

    ``expand(key)`` may return a wider, still bounded candidate set for a pad
    left with no admissible access; its new launches pass the same checks.
    """
    from .routing import _via_resource_id, _Resource, GridNode, _edge_key
    sites = dict(graph.via_sites)
    # Board copper, accepted terminal slots and witnesses in use: never displaced.
    exact = RoutingClearanceIndex(board)
    witnesses = {}
    owners = defaultdict(list)
    buckets = defaultdict(list)
    for identifier, positions in sorted(sites.items()):
        _,low,high,_,_ = identifier.split(':')
        span = (graph.layers[int(low)],graph.layers[int(high)])
        for point in positions:
            key=(span,point)
            owners[key].append(identifier)
            if key not in witnesses:
                witnesses[key]=Via('<capacity-'+str(len(witnesses)+1)+'>',point,
                    board.rules.default_via_size_nm,board.rules.default_via_drill_nm,*span)
                buckets[point.x_nm//_WITNESS_REACH_NM,point.y_nm//_WITNESS_REACH_NM].append(key)
    protected = set()

    def protect(key):
        if key in witnesses and key not in protected:
            protected.add(key)
            exact.add_via(witnesses[key],locked=True)

    def colliding_witnesses(via):
        reach = _WITNESS_REACH_NM
        cx, cy = via.position.x_nm//reach, via.position.y_nm//reach
        found = []
        for bx in (cx-1,cx,cx+1):
            for by in (cy-1,cy,cy+1):
                for key in buckets.get((bx,by),()):
                    if key not in witnesses:
                        continue
                    point = key[1]
                    if (abs(point.x_nm-via.position.x_nm) > reach
                            or abs(point.y_nm-via.position.y_nm) > reach):
                        continue
                    if not exact.candidate_vias_clear((via,witnesses[key])):
                        found.append(key)
        return sorted(found, key=lambda key: (key[1].x_nm,key[1].y_nm,str(key[0])))

    proposals = 0
    changed = set()

    def admit(access):
        nonlocal proposals
        via=access.via
        identifier=_via_resource_id(graph,access.node,via.from_layer,via.to_layer)
        _,low,high,x,y=identifier.split(':')
        nodes=(GridNode(int(low),int(x),int(y)),GridNode(int(high),int(x),int(y)))
        if any(node not in graph.legal_nodes for node in nodes):
            return False
        positions=sites.get(identifier,())
        if via.position in positions:
            # An existing sampled site is now real terminal copper.
            protect(((graph.layers[int(low)],graph.layers[int(high)]),via.position))
            return True
        if proposals >= MAX_TERMINAL_CAPACITY_PROPOSALS:
            return False
        proposals+=1
        capacity_via=replace(via,net='<capacity-proposal-'+str(proposals)+'>')
        if not exact.can_via(capacity_via.net,via.position,via.size_nm,
                via.from_layer,via.to_layer,via.drill_nm,check_hole_copper=True):
            return False
        for displaced in colliding_witnesses(capacity_via):
            del witnesses[displaced]
            for owner in owners.pop(displaced):
                sites[owner]=tuple(point for point in sites[owner] if point != displaced[1])
                changed.add(owner)
        sites[identifier]=(*sites.get(identifier,()),via.position)
        exact.add_via(capacity_via,locked=True)
        changed.add(identifier)
        return True

    # Two passes in the same deterministic pad order: first each pad's first
    # admissible launch, then the remaining alternatives. Otherwise a pad
    # earlier in the order can take every launch site near a neighbour.
    ordered = sorted(access_options.items(),key=lambda item:(item[0][0],str(item[0][1])))
    decided = {key: [access.via is None or None for access in choices] for key, choices in ordered}
    for key, choices in ordered:
        for slot, access in enumerate(choices):
            if decided[key][slot] is None:
                decided[key][slot] = admit(access)
                if decided[key][slot]:
                    break
    for key, choices in ordered:
        for slot, access in enumerate(choices):
            if decided[key][slot] is None:
                decided[key][slot] = admit(access)
    accepted = {key: tuple(access for slot, access in enumerate(choices) if decided[key][slot])
                for key, choices in ordered}
    if expand is not None:
        # Fine-pitch neighbours can claim every nearby launch of a pad first.
        # Only a pad left with nothing tries its wider set, one launch each.
        for key, choices in ordered:
            if accepted[key] or not choices:
                continue
            tried = {(access.node, access.via) for access in choices}
            for access in expand(key):
                if (access.node, access.via) in tried:
                    continue
                if access.via is None or admit(access):
                    accepted[key] = (access,)
                    break
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
                    if capacity:
                        resources[_edge_key(a,b)]=_Resource(identifier,a,b,capacity,None)
                    elif getattr(resources.get(_edge_key(a,b)),'identifier',None)==identifier:
                        del resources[_edge_key(a,b)]
    return replace(graph,via_sites=MappingProxyType(sites),resources=MappingProxyType(resources)),accepted

def terminal_access_failures(generated, registered):
    """Explain empty terminals without altering candidate admission or routing.

    A missing local escape and a legal escape rejected by shared transition
    capacity need different repairs. Preserve both counts instead of reporting
    every failure as a geometric impossibility.
    """
    return {
        key: (len(choices), len(registered.get(key, ())))
        for key, choices in generated.items()
        if not registered.get(key)
    }

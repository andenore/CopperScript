"""Fresh exact terminal launch witnesses for the coarse capacity graph."""
from dataclasses import replace
from types import MappingProxyType
from .physical import Via
from .routing_clearance import RoutingClearanceIndex

MAX_TERMINAL_CAPACITY_PROPOSALS = 4096

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


def register_terminal_capacity(board, graph, access_options):
    from .routing import _via_resource_id, _Resource, GridNode, _edge_key
    # Sampled sites are provisional capacity witnesses, never input copper.
    # Admit checked terminal launches first, then refill the remaining sample
    # pool. Rebuilding the graph counts the surviving coexisting sites; it
    # does not add a terminal alias on top of an overlapping sampled slot.
    sites = {identifier: () for identifier in graph.via_sites}
    index = RoutingClearanceIndex(board)
    selected = set()
    ordered = sorted(access_options.items(), key=lambda item: (item[0][0], str(item[0][1])))
    accepted_indexes = {key: set() for key, _ in ordered}
    proposals = 0
    # One candidate per terminal per round prevents an early terminal's
    # alternatives from filling every free site before its peers are seen.
    for depth in range(max((len(choices) for _, choices in ordered), default=0)):
        for key, choices in ordered:
            if depth >= len(choices):
                continue
            access = choices[depth]
            via=access.via
            if via is None:
                accepted_indexes[key].add(depth)
                continue
            identifier=_via_resource_id(graph,access.node,via.from_layer,via.to_layer)
            _,low,high,x,y=identifier.split(':')
            nodes=(GridNode(int(low),int(x),int(y)),GridNode(int(high),int(x),int(y)))
            if any(node not in graph.legal_nodes for node in nodes):
                continue
            positions=sites.get(identifier,())
            if via.position not in positions:
                sampled = via.position in graph.via_sites.get(identifier, ())
                if not sampled and proposals >= MAX_TERMINAL_CAPACITY_PROPOSALS:
                    continue
                if not sampled:
                    proposals += 1
                identity = (via.from_layer, via.to_layer, via.position)
                if identity not in selected:
                    capacity_net = '<capacity-terminal-' + str(len(selected)) + '>'
                    if not index.can_via(capacity_net,via.position,via.size_nm,
                            via.from_layer,via.to_layer,via.drill_nm,check_hole_copper=True):
                        continue
                    selected.add(identity)
                    index.add_via(replace(via,net=capacity_net),locked=True)
                sites[identifier]=(*positions,via.position)
            accepted_indexes[key].add(depth)
    accepted = {key: tuple(a for i, a in enumerate(choices) if i in accepted_indexes[key])
                for key, choices in ordered}
    if not selected:
        return graph, accepted
    for identifier, positions in sorted(graph.via_sites.items()):
        _, low, high, _, _ = identifier.split(':')
        span = graph.layers[int(low)], graph.layers[int(high)]
        for point in positions:
            identity = (*span, point)
            if identity not in selected:
                via = Via('<capacity-sample-' + str(len(selected)) + '>', point,
                          board.rules.default_via_size_nm, board.rules.default_via_drill_nm, *span)
                if not index.can_via(via.net, point, via.size_nm, *span, via.drill_nm,
                                     check_hole_copper=True):
                    continue
                selected.add(identity)
                index.add_via(via, locked=True)
            if point not in sites[identifier]:
                sites[identifier] = (*sites[identifier], point)
    resources={key: resource for key, resource in graph.resources.items() if resource.layer is not None}
    for identifier, positions in sorted(sites.items()):
        if not positions:
            continue
        _,low,high,x,y=identifier.split(':')
        low,high,x,y=map(int,(low,high,x,y))
        capacity=len(positions)
        # A through launch contacts all declared copper layers in its span.
        # Only already-existing legal graph nodes receive alias edges.
        for first in range(low,high):
            for second in range(first+1,high+1):
                a,b=GridNode(first,x,y),GridNode(second,x,y)
                if a in graph.legal_nodes and b in graph.legal_nodes:
                    resources[_edge_key(a,b)]=_Resource(identifier,a,b,capacity,None)
    return replace(graph,via_sites=MappingProxyType(sites),resources=MappingProxyType(resources)),accepted

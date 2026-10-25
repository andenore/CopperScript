"""Project freshly verified, retained package ports into the capacity graph."""
from .physical import Point
from .pin_escape import RoutingAccess, verified_routing_access, verified_fanout_path, checked_access_path
from .pin_access import local_access_path
from .placement import transformed_pad_position


def admit_fixed_prefix(source, held):
    """Admit immutable extra copper, never a placement/contract replacement."""
    from dataclasses import replace
    from .drc import run_physical_drc, physical_board_digest
    if (replace(held, tracks=source.tracks, vias=source.vias, metadata=source.metadata) != source
            or any(held.metadata.get(k) != v for k, v in source.metadata.items())
            or held.tracks[:len(source.tracks)] != source.tracks
            or held.vias[:len(source.vias)] != source.vias):
        raise ValueError("fixed global prefix changes source contracts, immutable copper or required arrays")
    hard = [f for f in run_physical_drc(held).findings if f.severity.value == 'error'
            and f.code not in {'DRC-OPEN-NET', 'DRC-ROUTE-INCOMPLETE'}]
    if hard:
        raise ValueError(f"fixed global prefix fails fresh physical DRC: {hard[0].code}")
    return physical_board_digest(held)


def reserved_pin_access_candidates(board, graph, pad, net, allowed, rule, clearance, options, anchor):
    from .routing import PinAccess, _neighbors, _node_distance_nm
    pose = next(p for p in board.placements if p.reference == pad.component)
    retained_via = None
    if isinstance(anchor, RoutingAccess):
        chain = verified_routing_access(board, pad, net, anchor, clearance)
        position, layers = anchor.position, (anchor.layer,)
        retained_via = next((v for v in board.vias if v.net == net and v.position == anchor.launch_position), None)
    elif isinstance(anchor, Point):
        chain = verified_fanout_path(board, pad, net, anchor, clearance)
        retained_via = next((v for v in board.vias if v.net == net and v.position == anchor), None)
        if retained_via is None:
            return ()
        low, high = sorted((board.stackup.copper_layers.index(retained_via.from_layer),
                            board.stackup.copper_layers.index(retained_via.to_layer)))
        position, layers = anchor, board.stackup.copper_layers[low:high+1]
    else:
        return ()
    if chain is None:
        return ()  # An invalid owned port cannot be replaced by a new exit.
    width = rule.width_nm if rule and rule.width_nm else board.rules.default_track_width_nm
    allowed_indexes = {graph.layers.index(layer) for layer in allowed}
    pad_position = transformed_pad_position(board, pose, pad.pad)
    cohorts = []
    for layer in allowed:
        if layer not in layers:
            continue
        nearby = sorted((n for n in graph.legal_nodes
            if graph.layers[n.layer_index] is layer
            and _node_distance_nm(graph.point(n), position) <= max(options.tile_size_nm*2, options.escape_radius_nm)
            and _neighbors(graph, n, allowed_indexes)),
            key=lambda n: (_node_distance_nm(graph.point(n), position), n))
        choices = []
        for node in nearby[:max(4, options.pin_access_candidates)]:
            endpoint = graph.point(node)
            # Most owned launches need only an exact octilinear corner to a
            # nearby guide. Do not spend a maze budget before trying it.
            path = checked_access_path(board, clearance, net, position, endpoint, width, layer)
            if path is None:
                path = local_access_path(board, clearance, net, position, endpoint, layer, width,
                    step_nm=min(options.escape_step_nm, 250_000),
                    detour_nm=options.escape_radius_nm, maximum_states=600)
            regional = path is None
            if regional and (abs(endpoint.x_nm-position.x_nm)>options.tile_size_nm//2
                             or abs(endpoint.y_nm-position.y_nm)>options.tile_size_nm//2):
                continue
            # The regional form is the established checked physical exit into
            # a coarse cell, not permission for a wire to its centre. Detailed
            # routing must still join the exact owned port. A retained via is
            # fixed input copper, never new shared transition capacity.
            choices.append(PinAccess(pad, pad_position, node, position if regional else endpoint,
                layer, tuple(dict.fromkeys((*chain, *(path or ())))), region_only=regional,
                retained_via=retained_via))
        if choices:
            cohorts.append(choices)
    result = []
    for depth in range(max((len(c) for c in cohorts), default=0)):
        for choices in cohorts:
            if depth < len(choices):
                result.append(choices[depth])
            if len(result) >= options.pin_access_candidates:
                return tuple(result)
    return tuple(result)

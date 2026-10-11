"""Read-only final-copper measurements; no inferred electrical acceptance limits."""
from __future__ import annotations

from math import isqrt

from .drc import explicit_copper_connectivity, physical_board_digest
from .physical import BoardSide, CopperLayer, PhysicalNet
from .placement import transformed_pad_position


def track_length_nm(track):
    """Integer-nanometre floor of segment length, consistent across reports."""
    dx, dy = track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm
    return isqrt(dx * dx + dy * dy)


def routing_quality(board, native=None):
    digest = physical_board_digest(board)
    graph = explicit_copper_connectivity(board)
    angles = dict(straight_nm=0, diagonal_45_nm=0, other_angle_nm=0)
    layers = {layer.value: 0 for layer in board.stackup.copper_layers}
    nets = {}
    for net in sorted(board.nets, key=lambda item: item.name):
        tracks = [t for t in board.tracks if t.net == net.name]
        nets[net.name] = {
            "track_length_nm": sum(track_length_nm(t) for t in tracks),
            "track_count": len(tracks),
            "via_count": sum(v.net == net.name for v in board.vias),
            "explicit_copper_connected": graph.net_connected(net),
        }
    for track in board.tracks:
        dx, dy = abs(track.end.x_nm - track.start.x_nm), abs(track.end.y_nm - track.start.y_nm)
        length = track_length_nm(track)
        angles["straight_nm" if not dx or not dy else
               "diagonal_45_nm" if dx == dy else "other_angle_nm"] += length
        layers[track.layer.value] += length

    poses = {pose.reference: pose for pose in board.placements}
    surface_graphs = {}
    bypasses = []
    for link in sorted(board.decoupling_links, key=lambda item: item.capacitor):
        cap, target = poses[link.capacitor.component], poses[link.target.component]
        layer = CopperLayer.FRONT if cap.side is BoardSide.FRONT else CopperLayer.BACK
        key = (link.net, layer)
        if key not in surface_graphs:
            surface_graphs[key] = explicit_copper_connectivity(
                board, only_nets=frozenset((link.net,)), include_internal_connections=False,
                tracks=tuple(t for t in board.tracks if t.net == link.net and t.layer is layer),
                vias=(), polygons=())
        surface = surface_graphs[key]
        connected = cap.side is target.side and surface.net_connected(
            PhysicalNet(link.net, (link.capacitor, link.target)))
        a = transformed_pad_position(board, cap, link.capacitor.pad)
        b = transformed_pad_position(board, target, link.target.pad)
        roots = {surface.roots[node] for node in surface.pad_nodes.get(link.capacitor, ())}
        # The graph indices refer to the filtered track inventory, not board indices.
        filtered = [t for t in board.tracks if t.net == link.net and t.layer is layer]
        component_tracks = [t for index, t in enumerate(filtered)
                            if surface.roots.get(f"track:{index}") in roots]
        bypasses.append({
            "capacitor": f"{link.capacitor.component}.{link.capacitor.pad}",
            "target": f"{link.target.component}.{link.target.pad}", "net": link.net,
            "layer": layer.value, "surface_connected": connected,
            "pad_distance_nm": isqrt((a.x_nm-b.x_nm)**2 + (a.y_nm-b.y_nm)**2),
            "surface_component_track_length_nm": sum(track_length_nm(t) for t in component_tracks),
            "length_scope": "entire connected surface track component, including branches; not shortest path",
        })

    references = []
    native_current = native is not None and native.matches(board)
    for rule in sorted(board.net_routing_rules, key=lambda item: item.net):
        if rule.kind.value not in {"rf_feed", "differential", "clock", "can_bus"} and rule.shared_reference_layer is None:
            continue
        if not native_current or rule.shared_reference_layer is None or not rule.return_via_net:
            references.append({"net": rule.net, "status": "incomplete",
                "reason": "requires current native fill and an explicit reference net/layer"})
            continue
        from .rf_geometry import reference_coverage
        result = reference_coverage({
            "layers": [layer.value for layer in board.stackup.copper_layers],
            "tracks": [{"net": t.net, "layer": t.layer.value, "width_nm": t.width_nm,
                        "start": [t.start.x_nm, t.start.y_nm], "end": [t.end.x_nm, t.end.y_nm]}
                       for t in board.tracks],
            "fills": [{"net": f.net, "layer": f.layer, "outer": f.vertices_nm, "holes": []}
                      for f in native.filled_regions],
        }, identifier=f"reference:{rule.net}", net=rule.net, reference_net=rule.return_via_net,
           reference_layer=rule.shared_reference_layer.value, margin_nm=0)
        references.append({"net": rule.net, **result})

    return {"schema": "copperscript-routing-quality/v0.1", "board_digest": digest,
        "scope": "explicit track geometry and surface connectivity; no impedance, thermal or production qualification",
        "length_method": "sum of integer-nanometre segment lengths rounded down",
        "track_count": len(board.tracks), "via_count": len(board.vias),
        "track_length_nm": sum(layers.values()), "layer_length_nm": layers,
        "angle_length_nm": angles, "nets": nets, "decoupling": bypasses,
        "reference_coverage": references,
        "native_fill_current": native_current,
        "native_report_digest": native.report_digest if native_current else None}

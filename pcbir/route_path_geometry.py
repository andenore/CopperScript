"""Bounded pad-centre/straight-track graph extraction, never electrical signoff.

Exact centreline coincidences only. Copper-edge contacts, pad-area traversal,
arcs, filled zones and component internals are deliberately not inferred.
"""
from collections import defaultdict, deque
from math import hypot

from .qualification import check


def _point(value):
    if len(value) != 2 or any(type(n) is not int for n in value):
        raise ValueError("native positions require integer nanometres")
    return tuple(value)


def _on(point, a, b):
    return ((point[0]-a[0])*(b[1]-a[1]) == (point[1]-a[1])*(b[0]-a[0]) and
            min(a[0], b[0]) <= point[0] <= max(a[0], b[0]) and
            min(a[1], b[1]) <= point[1] <= max(a[1], b[1]))


def _proper_cross(a, b, c, d):
    def side(p, q, r):
        return (q[0]-p[0])*(r[1]-p[1]) - (q[1]-p[1])*(r[0]-p[0])
    return side(a,b,c)*side(a,b,d) < 0 and side(c,d,a)*side(c,d,b) < 0


def _construction_centres(context, layers):
    physical = context.get("physical_layers")
    if not physical:
        return {}
    if context.get("copper_layers") != layers:
        raise ValueError("path stackup order differs from native board")
    centres, position = {}, 0
    for layer in physical:
        thickness = layer["thickness_nm"]
        if type(thickness) is not int or thickness <= 0:
            raise ValueError("invalid physical thickness")
        if layer["kind"] == "copper":
            centres[layer["name"]] = position + thickness / 2
        position += thickness
    if list(centres) != layers:
        raise ValueError("physical copper order differs from native board")
    return centres


def routed_path_inventory(native, context, *, identifier, net, terminals, return_net):
    if not isinstance(net, str) or not net or not isinstance(return_net, str) or not return_net or net == return_net:
        raise ValueError("path and return nets must be distinct nonempty strings")
    if (not isinstance(terminals, list) or len(terminals) != 2 or
            any(not isinstance(t, dict) or set(t) != {"component", "pad", "layer"} or
                any(not isinstance(v, str) or not v for v in t.values()) for t in terminals) or terminals[0] == terminals[1]):
        raise ValueError("path requires two distinct explicit component/pad/layer selectors")
    if native is None:
        return check(identifier, "incomplete", "Native contact/track geometry unavailable")
    layers = native["layers"]
    if not layers or len(set(layers)) != len(layers):
        raise ValueError("invalid native copper order")
    tracks = [t for t in native["tracks"] if t["net"] == net]
    if not tracks:
        return check(identifier, "incomplete", "No supported straight segments for selected net")
    points = defaultdict(set)
    for t in tracks:
        a, b = _point(t["start"]), _point(t["end"])
        if t["layer"] not in layers or a == b or type(t["width_nm"]) is not int or t["width_nm"] <= 0:
            raise ValueError("invalid native straight segment")
        points[t["layer"]].update((a, b))
    for i, t in enumerate(tracks):
        for other in tracks[:i]:
            if t["layer"] == other["layer"] and _proper_cross(t["start"], t["end"], other["start"], other["end"]):
                return check(identifier, "incomplete", "Interior centreline crossing unsupported; no shortest-path guess")
    nodes = []
    for selector in terminals:
        matches = [c for c in native.get("contacts", []) if c["component"] == selector["component"] and c["pad"] == selector["pad"]]
        if len(matches) != 1 or matches[0]["net"] != net or selector["layer"] not in matches[0].get("layers", []):
            return check(identifier, "incomplete", "Terminal net/layer/unique land identity unestablished", terminal=selector)
        xy = _point(matches[0]["position"])
        points[selector["layer"]].add(xy)
        nodes.append((*xy, selector["layer"]))
    if nodes[0] == nodes[1]:
        return check(identifier, "incomplete", "Coincident terminal centres do not establish a routed path")
    centres = _construction_centres(context, layers)
    vias = [v for v in native.get("vias", []) if v["net"] == net]
    via_info = []
    for v in vias:
        # Only through spans are supported here; no blind/microvia assumptions.
        if v.get("via_type") != "through" or v["from_layer"] != layers[0] or v["to_layer"] != layers[-1]:
            return check(identifier, "incomplete", "Non-through signal via requires explicit barrel geometry")
        xy = _point(v["position"])
        used = [layer for layer in layers if any(t["layer"] == layer and _on(xy, _point(t["start"]), _point(t["end"])) for t in tracks)]
        if not used:
            return check(identifier, "incomplete", "Signal via has no exact centreline attachment", position=list(xy))
        for layer in used:
            points[layer].add(xy)
        returns = [r for r in native.get("vias", []) if r["net"] == return_net and
                   r["from_layer"] in layers and r["to_layer"] in layers and
                   layers.index(r["from_layer"]) <= layers.index(used[0]) and
                   layers.index(r["to_layer"]) >= layers.index(used[-1])]
        nearest = min(returns, key=lambda r: hypot(r["position"][0]-xy[0], r["position"][1]-xy[1]), default=None)
        via_info.append(dict(position=list(xy), attached_track_layers=used,
            construction_span_between_used_layer_centres_nm=centres[used[-1]]-centres[used[0]] if centres else None,
            construction_unused_barrel_above_nm=centres[used[0]]-centres[layers[0]] if centres else None,
            construction_unused_barrel_below_nm=centres[layers[-1]]-centres[used[-1]] if centres else None,
            nearest_return_via=None if nearest is None else dict(net=return_net, position=nearest["position"],
                from_layer=nearest["from_layer"], to_layer=nearest["to_layer"],
                centre_distance_nm=hypot(nearest["position"][0]-xy[0], nearest["position"][1]-xy[1]))))
    edges, graph, seen = [], defaultdict(list), set()
    def edge(a, b, metadata):
        key = tuple(sorted((a, b)))
        if key in seen:
            raise ValueError("overlapping/duplicate centreline edges; no automatic normalization")
        seen.add(key)
        index = len(edges)
        edges.append(dict(start=list(a), end=list(b), **metadata))
        graph[a].append((b, index))
        graph[b].append((a, index))
    for i, t in enumerate(tracks):
        a, b = _point(t["start"]), _point(t["end"])
        selected = sorted((p for p in points[t["layer"]] if _on(p, a, b)),
                          key=lambda p: (p[0]-a[0])*(b[0]-a[0])+(p[1]-a[1])*(b[1]-a[1]))
        for p, q in zip(selected, selected[1:]):
            edge((*p, t["layer"]), (*q, t["layer"]), dict(kind="track", segment=i, layer=t["layer"],
                 width_nm=t["width_nm"], length_nm=hypot(q[0]-p[0], q[1]-p[1])))
    for i, v in enumerate(via_info):
        used, xy = v["attached_track_layers"], v["position"]
        for a, b in zip(used, used[1:]):
            edge((*xy, a), (*xy, b), dict(kind="via", via=i,
                construction_length_nm=centres[b]-centres[a] if centres else None))
    parent, queue = {nodes[0]: None}, deque([nodes[0]])
    while queue:
        for node, index in graph[queue.popleft()]:
            if node not in parent:
                parent[node] = (edges[index], index)
                queue.append(node)
    connected_edges = {index for node in parent for _, index in graph[node]}
    metrics = dict(net=net, terminals=terminals, vias=via_info,
        connected_node_count=len(parent), connected_edge_count=len(connected_edges),
        off_component_edge_count=len(edges)-len(connected_edges),
        branch_nodes=[list(node) for node in sorted(parent) if len(graph[node]) > 2],
        centreline_path_found=False, path_track_length_nm=None)
    if nodes[1] not in parent:
        return check(identifier, "incomplete", "No exact pad-centre graph path; not proof of a physical open", **metrics)
    if len(connected_edges) != len(parent)-1:
        return check(identifier, "incomplete", "Centreline cycle/alternate paths; no shortest-path length selected", **metrics)
    path, cursor = [], nodes[1]
    while cursor != nodes[0]:
        item, index = parent[cursor]
        a, b = tuple(item["start"]), tuple(item["end"])
        previous = b if cursor == a else a
        path.append({**item, "start": list(previous), "end": list(cursor)})
        cursor = previous
    path.reverse()
    by_layer = defaultdict(float)
    for item in path:
        if item["kind"] == "track":
            by_layer[item["layer"]] += item["length_nm"]
    metrics.update(centreline_path_found=True, path=path, track_length_by_layer_nm=dict(by_layer),
        path_track_length_nm=sum(by_layer.values()), path_via_transition_count=sum(e["kind"] == "via" for e in path),
        off_path_connected_edge_count=len(connected_edges)-len(path))
    return check(identifier, "incomplete", "Unique bounded centreline graph path, not electrical delay or copper-connectivity approval",
                 evidence_grade="screening", **metrics,
                 limitations=["Exact pad centres/straight centrelines only; pad fields, copper-edge contacts and component internals excluded",
                              "Via spans/stubs use published construction layer centres, not finished-board measurements or EM models",
                              "Nearby return via distance does not prove reference connectivity or GND-to-power-plane return transfer",
                              "No propagation delay, impedance or functional qualification inferred"])

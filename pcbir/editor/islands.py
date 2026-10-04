"""Exact deterministic island MST using spatial Boruvka nearest-cut edges.

Bounding boxes and uniform-component subtree labels prune distance tests, not
candidate connectivity. Unique edge ordering preserves the exhaustive Kruskal
result even for coincident terminals and tied Euclidean lengths. No third-party
geometry dependency, approximate neighbours, or zone-outline connectivity.
"""
from __future__ import annotations

from dataclasses import dataclass


def edge(a, b):
    a, b = sorted((a, b), key=lambda land: (land[0], land[1]))
    distance = (a[4].x_nm - b[4].x_nm) ** 2 + (a[4].y_nm - b[4].y_nm) ** 2
    return (distance, a[0], b[0], a[1], b[1], a, b)


@dataclass
class _Node:
    box: tuple
    lands: tuple = ()
    children: tuple = ()
    component: str | None = None


def _tree(lands):
    xs, ys = [l[4].x_nm for l in lands], [l[4].y_nm for l in lands]
    box = min(xs), min(ys), max(xs), max(ys)
    if len(lands) <= 8:
        return _Node(box, tuple(lands))
    axis = 0 if box[2] - box[0] >= box[3] - box[1] else 1
    ordered = sorted(lands, key=lambda l: ((l[4].x_nm, l[4].y_nm)[axis], l[0], l[1]))
    middle = len(ordered) // 2
    return _Node(box, children=(_tree(ordered[:middle]), _tree(ordered[middle:])))


def nearest_island_tree(lands, *, statistics=None):
    """Return the exhaustive complete-island-graph MST, in stable edge order.

    Each land is (explicit_root, identity, reference, pad, Point). Returns the
    same tuples as ``edge``. Worst-case geometry may still require quadratic
    distance work; memory stays linear and all pruning is exact.
    """
    if statistics is None:
        statistics = {}
    statistics.update(distance_tests=0, rounds=0)
    parent = {l[0]: l[0] for l in lands}
    if len(parent) <= 1:
        return []

    def find(root):
        while root != parent[root]:
            parent[root] = parent[parent[root]]
            root = parent[root]
        return root

    root = _tree(lands)

    def label(node):
        if node.children:
            groups = {label(child) for child in node.children}
        else:
            groups = {find(l[0]) for l in node.lands}
        node.component = next(iter(groups)) if len(groups) == 1 and None not in groups else None
        return node.component

    def lower_bound(node, point):
        x0, y0, x1, y1 = node.box
        dx, dy = max(x0 - point.x_nm, 0, point.x_nm - x1), max(y0 - point.y_nm, 0, point.y_nm - y1)
        return dx*dx + dy*dy

    def nearest(land, group):
        best = None

        def visit(node):
            nonlocal best
            if node.component == group or (best is not None and lower_bound(node, land[4]) > best[0]):
                return
            if node.children:
                for child in sorted(node.children, key=lambda n: lower_bound(n, land[4])):
                    visit(child)
            else:
                for other in node.lands:
                    if find(other[0]) == group:
                        continue
                    statistics["distance_tests"] += 1
                    candidate = edge(land, other)
                    if best is None or candidate[:5] < best[:5]:
                        best = candidate
        visit(root)
        return best

    result = []
    while len({find(r) for r in parent}) > 1:
        statistics["rounds"] += 1
        label(root)
        choices = {}
        for land in lands:
            group = find(land[0])
            candidate = nearest(land, group)
            if candidate is not None and (group not in choices or candidate[:5] < choices[group][:5]):
                choices[group] = candidate
        added = 0
        for candidate in sorted(choices.values(), key=lambda e: e[:5]):
            a, b = find(candidate[1]), find(candidate[2])
            if a != b:
                parent[max(a, b)] = min(a, b)
                result.append(candidate)
                added += 1
        if not added:
            raise ValueError("island MST could not connect the complete terminal graph")
    return sorted(result, key=lambda e: e[:5])

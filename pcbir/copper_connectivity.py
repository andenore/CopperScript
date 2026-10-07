"""Layer-aware connectivity of explicit copper; zone outlines are not copper.

Each plated object is one node spanning only its physical copper layers.
Broad-phase bounds select pairs; positive-area copper overlap joins nodes.
No grid, clearance tolerance or implicit pad-number join is used. Explicit
component-internal edges are separate facts, never fabricated copper.
Filled-zone connectivity still requires independent fill verification.
"""
from dataclasses import dataclass
from collections import defaultdict
from types import MappingProxyType
from typing import Mapping

from .geometry import (RoundedConvexShape, SpatialIndex, SpatialItem,
                       point_segment_distance_squared, shape_distance_squared)
from .physical import CopperLayer, PadReference, PhysicalNet, Via


@dataclass(frozen=True, slots=True)
class PhysicalCopperConnectivity:
    """Read-only land roots including only explicitly declared internal edges."""

    roots: Mapping[str, str]
    pad_nodes: Mapping[PadReference, tuple[str, ...]]
    internal_connections: tuple[tuple[str, ...], ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "roots", MappingProxyType(dict(self.roots)))
        object.__setattr__(self, "pad_nodes", MappingProxyType(dict(self.pad_nodes)))

    def pad_connected(self, pad: PadReference) -> bool:
        nodes = self.pad_nodes.get(pad, ())
        return bool(nodes) and len({self.roots[node] for node in nodes}) == 1

    def net_connected(self, net: PhysicalNet) -> bool:
        if not net.pads:
            return True
        if any(not self.pad_nodes.get(pad) for pad in net.pads):
            return False
        return len({self.roots[node] for pad in net.pads
                    for node in self.pad_nodes[pad]}) == 1


@dataclass(frozen=True, slots=True)
class CopperContact:
    identity: str
    net: str
    layers: tuple[CopperLayer, ...]
    shape: RoundedConvexShape
    drill: RoundedConvexShape | None = None
    capped_layers: tuple[CopperLayer, ...] = ()


def via_copper_contact(via: Via, copper_layers: tuple[CopperLayer, ...], *, identity: str = "via") -> CopperContact:
    """The plated span, open drill and any exterior copper caps of a via."""
    low, high = sorted((copper_layers.index(via.from_layer), copper_layers.index(via.to_layer)))
    capped = (copper_layers[0], copper_layers[-1]) if via.finish == "filled-capped" else ()
    return CopperContact(identity, via.net, copper_layers[low:high + 1],
                         RoundedConvexShape((via.position,), via.size_nm // 2),
                         RoundedConvexShape((via.position,), via.drill_nm // 2), capped)


def _inside_drill(shape: RoundedConvexShape, drill: RoundedConvexShape | None) -> bool:
    """Copper inside a drilled void, including a zero-area touch of its rim."""
    if drill is None or shape.radius_nm > drill.radius_nm:
        return False
    start, end = drill.spine[0], drill.spine[-1]
    margin = drill.radius_nm - shape.radius_nm
    return all(point_segment_distance_squared(point, start, end) <= margin * margin
               for point in shape.spine)


def _positive_area_overlap(first: RoundedConvexShape, second: RoundedConvexShape) -> bool:
    """A shared boundary alone is not a reliable electrical connection."""
    radius = first.radius_nm + second.radius_nm
    if radius:
        return shape_distance_squared(first, second) < radius * radius
    # Filled convex polygon pads have zero rounding radius. Spine distance is
    # zero for both an area overlap and a mere edge/vertex touch; strict
    # separating-axis overlap distinguishes them without a clearance tolerance.
    for shape in (first, second):
        points = shape.spine
        edges = tuple(zip(points, (*points[1:], points[0])))
        if not sum(a.x_nm * b.y_nm - a.y_nm * b.x_nm for a, b in edges):
            return False
        for a, b in edges:
            dx, dy = b.x_nm - a.x_nm, b.y_nm - a.y_nm
            if dx == dy == 0:
                continue
            left = [p.x_nm * dy - p.y_nm * dx for p in first.spine]
            right = [p.x_nm * dy - p.y_nm * dx for p in second.spine]
            if max(left) <= min(right) or max(right) <= min(left):
                return False
    return True


def copper_contacts_overlap(first: CopperContact, second: CopperContact) -> bool:
    """Positive-area electrical contact on at least one common copper layer."""
    if first.net != second.net or not _positive_area_overlap(first.shape, second.shape):
        return False
    # A cap provides copper at the drill centre on its outer layer;
    # an open plated drill is not a solid disk.
    return any(
        not _inside_drill(second.shape, None if layer in first.capped_layers else first.drill)
        and not _inside_drill(first.shape, None if layer in second.capped_layers else second.drill)
        for layer in set(first.layers).intersection(second.layers)
    )


def copper_contact_roots(
    objects: tuple[CopperContact, ...],
    internal_connections: tuple[tuple[str, ...], ...] = (),
) -> dict[str, str]:
    """Return deterministic component roots for explicit track/via/pad copper."""
    by_id = {item.identity: item for item in objects}
    if len(by_id) != len(objects):
        raise ValueError("copper contact identities must be unique")
    parent = {identity: identity for identity in by_id}

    def find(identity: str) -> str:
        root = identity
        while parent[root] != root:
            root = parent[root]
        while parent[identity] != identity:
            next_identity = parent[identity]
            parent[identity] = root
            identity = next_identity
        return root

    by_layer_net = defaultdict(list)
    for item in objects:
        for layer in item.layers:
            by_layer_net[layer, item.net].append(item)
    for items in by_layer_net.values():
        index = SpatialIndex(SpatialItem(item.identity, item.shape.bounds) for item in items)
        for first in items:
            for identity in index.query(first.shape.bounds):
                if identity <= first.identity or find(identity) == find(first.identity):
                    continue
                second = by_id[identity]
                if not copper_contacts_overlap(first, second):
                    continue
                left, right = find(first.identity), find(identity)
                parent[max(left, right)] = min(left, right)
    for group in internal_connections:
        if len(group) < 2 or any(node not in by_id for node in group):
            raise ValueError("internal copper graph edge has missing contacts")
        if len({by_id[node].net for node in group}) != 1:
            raise ValueError("internal connection cannot bridge different nets")
        for node in group[1:]:
            left, right = find(group[0]), find(node)
            parent[max(left, right)] = min(left, right)
    return {identity: find(identity) for identity in sorted(by_id)}

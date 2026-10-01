"""Layer-aware connectivity of explicit copper; zone outlines are not copper.

Each plated object is one node spanning only its physical copper layers.
Broad-phase bounds select pairs; exact rounded-shape contact joins nodes.
No grid, clearance tolerance, net-name shortcut or virtual pad-number join is
used. Filled-zone connectivity still requires independent fill verification.
"""
from dataclasses import dataclass
from collections import defaultdict

from .geometry import (RoundedConvexShape, SpatialIndex, SpatialItem,
                       point_segment_distance_squared, shape_distance_squared)
from .physical import CopperLayer


@dataclass(frozen=True, slots=True)
class CopperContact:
    identity: str
    net: str
    layers: tuple[CopperLayer, ...]
    shape: RoundedConvexShape
    drill: RoundedConvexShape | None = None
    capped_layers: tuple[CopperLayer, ...] = ()


def _inside_drill(shape: RoundedConvexShape, drill: RoundedConvexShape | None) -> bool:
    """Reject copper wholly inside a round/oval drilled void, not its rim."""
    if drill is None or shape.radius_nm >= drill.radius_nm:
        return False
    start, end = drill.spine[0], drill.spine[-1]
    margin = drill.radius_nm - shape.radius_nm
    return all(point_segment_distance_squared(point, start, end) < margin * margin
               for point in shape.spine)


def copper_contact_roots(objects: tuple[CopperContact, ...]) -> dict[str, str]:
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
                radius = first.shape.radius_nm + second.shape.radius_nm
                if shape_distance_squared(first.shape, second.shape) > radius * radius:
                    continue
                # A cap provides copper at the drill centre on its outer layer;
                # an open plated drill is not a solid disk.
                common = set(first.layers).intersection(second.layers)
                if not any(
                    not _inside_drill(second.shape, None if layer in first.capped_layers else first.drill)
                    and not _inside_drill(first.shape, None if layer in second.capped_layers else second.drill)
                    for layer in common
                ):
                    continue
                left, right = find(first.identity), find(identity)
                parent[max(left, right)] = min(left, right)
    return {identity: find(identity) for identity in sorted(by_id)}

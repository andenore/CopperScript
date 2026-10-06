"""Conservative declared shared-reference topology, not filled-copper proof."""
from __future__ import annotations

from .geometry import (RoundedConvexShape, point_in_polygon, point_on_segment,
                       segment_in_polygon, shapes_clear)
from .physical import (CopperLayer, NetRoutingRule, PhysicalBoard, Point,
                       ReturnViaPolicy, TrackSegment, Via)
from .placement import resolved_copper_keepouts
from .routing_layers import dedicated_plane_layers


def shared_reference_plane(
    board: PhysicalBoard, first: NetRoutingRule, second: NetRoutingRule,
    signal_layers: tuple[CopperLayer, ...], points: tuple[Point, ...] = (),
) -> CopperLayer | None:
    """Only explicit, matching adjacent-layer declarations can omit a via.

    Default/unknown/mixed policies retain the ordinary required-via behavior.
    A barrel crossing a plane is not itself a signal-reference declaration.
    """
    if not all(rule.require_return_vias and
               rule.return_via_policy is ReturnViaPolicy.REFERENCE_CHANGE
               for rule in (first, second)):
        return None
    layer = first.shared_reference_layer
    if (layer is None or layer != second.shared_reference_layer
            or first.return_via_net != second.return_via_net
            or len(set(signal_layers)) != 2):
        return None
    layers = board.stackup.copper_layers
    if layer not in layers or any(value not in layers for value in signal_layers):
        return None
    if any(abs(layers.index(value) - layers.index(layer)) != 1 for value in signal_layers):
        return None
    if dedicated_plane_layers(board).get(layer) != first.return_via_net:
        return None
    zones = tuple(zone for zone in board.zones
                  if layer in zone.layers and zone.net == first.return_via_net)
    if not any(not zone.outline.holes and
               all(point_in_polygon(point, zone.outline.outer.vertices) for point in points)
               for zone in zones):
        return None
    if any(keepout.block_zones and layer in keepout.layers and
           any(point_in_polygon(point, keepout.outline.outer.vertices) for point in points)
           for keepout in resolved_copper_keepouts(board)):
        return None
    return layer


def transition_contact_layers(tracks: tuple[TrackSegment, ...], via: Via) -> tuple[CopperLayer, ...]:
    """Read actual centered signal contacts, not through-barrel endpoints."""
    return tuple(sorted({track.layer for track in tracks if track.net == via.net
                         and point_on_segment(via.position, track.start, track.end)},
                        key=lambda layer: layer.value))


def pair_reference_intent_covers(
    board: PhysicalBoard, reference: CopperLayer, net: str,
    tracks: tuple[TrackSegment, ...],
) -> bool:
    """Screen every projected track envelope and footprint-local zone keepout.

    Intentionally require one unperforated declared zone to cover the complete
    pair; overlapping/split pours are not guessed to be a continuous plane.
    Actual antipads/refill continuity and electrical SI still need verification.
    """
    if not tracks:
        return False
    layers = board.stackup.copper_layers
    if reference not in layers or any(track.layer not in layers or
            abs(layers.index(track.layer) - layers.index(reference)) != 1 for track in tracks):
        return False
    def within_zone(track, zone):
        vertices = zone.outline.outer.vertices
        envelope = RoundedConvexShape((track.start, track.end), (track.width_nm + 1) // 2)
        return (segment_in_polygon(track.start, track.end, vertices) and
                all(shapes_clear(envelope, RoundedConvexShape((start, end)), 0)
                    for start, end in zip(vertices, (*vertices[1:], vertices[0]))))
    if not any(zone.net == net and reference in zone.layers and not zone.outline.holes
               and all(within_zone(track, zone) for track in tracks) for zone in board.zones):
        return False
    keepouts = tuple(keepout for keepout in resolved_copper_keepouts(board)
                     if keepout.block_zones and reference in keepout.layers)
    return all(shapes_clear(RoundedConvexShape((track.start, track.end), (track.width_nm + 1) // 2),
                            RoundedConvexShape(keepout.outline.outer.vertices), 0)
               for track in tracks for keepout in keepouts)

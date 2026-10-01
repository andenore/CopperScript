"""Bounded terminal-tree alternatives to expensive single-net global guides.

This is a small legal-path graph, not a Steiner solver or RF reference-copper
transplant. Native critical acceptance must still check the complete candidate.
"""
from itertools import combinations
from math import hypot
from typing import Iterator

from .physical import BoardSide, CopperLayer, NetRoutingRule, PadKind, PhysicalBoard, Point, RouteKind, TrackSegment
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .surface_path import surface_path


def local_surface_candidates(
    board: PhysicalBoard, rule: NetRoutingRule,
) -> Iterator[tuple[TrackSegment, ...]]:
    """Yield deterministic, via-free octilinear trees for two to eight lands.

    Enumerate cheap direct/one-corner legal paths between actual physical lands,
    then Kruskal's minimum spanning tree on that bounded weighted graph. Every
    terminal remains a graph vertex, including repeated physical pad numbers.
    A missing edge is not a reason to drop a terminal. No maze search, long
    arbitrary-angle shortcuts, component movement or cross-layer guesses occur.
    """
    if rule.kind not in {RouteKind.CRITICAL, RouteKind.CLOCK, RouteKind.RF_FEED}:
        return
    net = next((net for net in board.nets if net.name == rule.net), None)
    if net is None or not 2 <= len(net.pads) <= 8:
        return
    if rule.kind in {RouteKind.CLOCK, RouteKind.RF_FEED} and len(net.pads) != 2:
        return
    if len(net.pads) > 2 and rule.topology != "tree":
        return
    poses = {pose.reference: pose for pose in board.placements}
    layers = set(routing_layers(board, rule.net, rule))
    terminals: list[Point] = []
    for reference in net.pads:
        pose = poses[reference.component]
        pads = [pad for pad in board.footprints[pose.footprint].pads if pad.number == reference.pad]
        if not pads or any(pad.kind not in {PadKind.SMD, PadKind.THROUGH_HOLE} for pad in pads):
            return
        for pad in pads:
            if pad.kind is PadKind.SMD:
                layers.intersection_update({CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK})
            terminals.append(transformed_local_point(pose, pad.position))
            if len(terminals) > 8:
                return
    # Count physical lands before deduplication to bound work on repeated pads.
    if not 2 <= len(terminals) <= 8:
        return
    if rule.kind in {RouteKind.CLOCK, RouteKind.RF_FEED} and len(terminals) != 2:
        return  # Preserve the existing physical point-to-point profile limit.
    terminals = sorted(set(terminals), key=lambda point: (point.x_nm, point.y_nm))
    if len(terminals) < 2:
        return
    width = rule.width_nm or board.rules.default_track_width_nm
    clearance = RoutingClearanceIndex(board)
    for layer in routing_layers(board, rule.net, rule):
        if layer not in layers:
            continue
        edges = []
        for first, second in combinations(range(len(terminals)), 2):
            path = surface_path(board, clearance, rule.net, terminals[first], terminals[second],
                                width, layer, board.tracks)
            if not path or any(
                track.start.x_nm != track.end.x_nm and track.start.y_nm != track.end.y_nm
                and abs(track.start.x_nm - track.end.x_nm) != abs(track.start.y_nm - track.end.y_nm)
                for track in path
            ):
                continue
            length = sum(round(hypot(track.end.x_nm - track.start.x_nm,
                                     track.end.y_nm - track.start.y_nm)) for track in path)
            edges.append((length, first, second, path))
        parent = list(range(len(terminals)))

        def root(node):
            while parent[node] != node:
                node = parent[node]
            return node

        tracks = []
        joined = 0
        for _, first, second, path in sorted(edges, key=lambda edge: edge[:3]):
            a, b = root(first), root(second)
            if a == b:
                continue
            parent[b] = a
            tracks.extend(path)
            joined += 1
            if joined == len(terminals) - 1:
                yield tuple(dict.fromkeys(tracks))
                break

"""Bounded terminal-tree alternatives to expensive single-net global guides.

This is a small legal-path graph, not a Steiner solver or RF reference-copper
transplant. Native critical acceptance must still check the complete candidate.
"""
from itertools import combinations
from dataclasses import replace
from math import hypot
from typing import Iterator

from .physical import BoardSide, CopperLayer, NetRoutingRule, PadKind, PhysicalBoard, Point, RouteKind, TrackSegment
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers
from .surface_path import surface_path, surface_path_between
from .physical import nm_from_mm


def _bounded_surface_maze(board, clearance, net, start, end, width, layer, *,
                          budget=20_000, detour_nm=nm_from_mm(2), phase_budget=None):
    """Use either terminal's grid phase within one shared local allowance."""
    step = nm_from_mm("0.1")
    if budget <= 0 or detour_nm < 0 or phase_budget is not None and phase_budget <= 0:
        raise ValueError("local maze computational bounds must be positive")
    spent = 0

    def observe(phase, event, details):
        nonlocal spent
        if phase == "surface_bridge_search" and event == "finished":
            spent += int(details["expanded_states"])

    path = surface_path_between(board, clearance, net, start, end, width, layer,
        step_nm=step, maximum_detour_nm=detour_nm,
        state_budget=min(budget, phase_budget or budget), on_progress=observe)
    if (path is None and 0 < spent < budget
            and (phase_budget is not None or (end.x_nm-start.x_nm) % step or (end.y_nm-start.y_nm) % step)):
        # A legal narrow channel may lie between every point of the first
        # terminal's grid. The other actual terminal supplies a second phase;
        # only unused states are available, and every edge keeps exact checks.
        reverse = surface_path_between(board, clearance, net, end, start, width, layer,
            step_nm=step, maximum_detour_nm=detour_nm,
            state_budget=min(budget-spent, phase_budget or budget), on_progress=observe)
        if reverse is not None:
            path = tuple(replace(t, start=t.end, end=t.start) for t in reversed(reverse))
    return path


def local_surface_candidates(
    board: PhysicalBoard, rule: NetRoutingRule,
    *, maze: bool = False, maze_state_budget: int = 20_000,
    maze_detour_nm: int = nm_from_mm(2), maze_phase_budget: int | None = None,
) -> Iterator[tuple[TrackSegment, ...]]:
    """Yield deterministic, via-free octilinear trees for two to eight lands.

    Enumerate cheap direct/one-corner legal paths between actual physical lands,
    then Kruskal's minimum spanning tree on that bounded weighted graph. Every
    terminal remains a graph vertex, including repeated physical pad numbers.
    A missing edge is not a reason to drop a terminal. No maze search, long
    arbitrary-angle shortcuts, component movement or cross-layer guesses occur
    by default. The optional two-terminal, explicitly via-free fallback uses
    the shared exact surface maze with a 20,000-state bound per terminal pair
    and permitted layer by default. Explicit computational bounds may reserve
    part of the allowance for each terminal's grid phase. They never alter
    copper, via, length or clearance contracts. Multi-terminal critical/RF
    trees require explicit tree topology and zero vias. Geometric connectivity
    does not approve RF stubs or performance.
    """
    if rule.kind not in {RouteKind.CRITICAL, RouteKind.CLOCK, RouteKind.RF_FEED}:
        return
    net = next((net for net in board.nets if net.name == rule.net), None)
    if net is None or not 2 <= len(net.pads) <= 8:
        return
    rf_tree = rule.kind is RouteKind.RF_FEED and rule.topology == "tree" and rule.max_vias == 0
    critical_tree = rule.kind is RouteKind.CRITICAL and rule.topology == "tree" and rule.max_vias == 0
    if rule.kind in {RouteKind.CLOCK, RouteKind.RF_FEED} and len(net.pads) != 2 and not rf_tree:
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
    if rule.kind in {RouteKind.CLOCK, RouteKind.RF_FEED} and len(terminals) != 2 and not rf_tree:
        return  # Preserve the existing physical point-to-point profile limit.
    terminals = sorted(set(terminals), key=lambda point: (point.x_nm, point.y_nm))
    if len(terminals) < 2:
        return
    width = rule.width_nm or board.rules.default_track_width_nm
    clearance = RoutingClearanceIndex(board)

    def octilinear(path):
        return path and all(
            t.start.x_nm == t.end.x_nm or t.start.y_nm == t.end.y_nm
            or abs(t.start.x_nm-t.end.x_nm) == abs(t.start.y_nm-t.end.y_nm)
            for t in path)

    for layer in routing_layers(board, rule.net, rule):
        if layer not in layers:
            continue
        edges = []
        for first, second in combinations(range(len(terminals)), 2):
            path = surface_path(board, clearance, rule.net, terminals[first], terminals[second],
                                width, layer, board.tracks)
            if not octilinear(path) and maze and (len(terminals) == 2 or rf_tree or critical_tree) and rule.max_vias == 0:
                path = _bounded_surface_maze(
                    board, clearance, rule.net, terminals[first], terminals[second], width, layer,
                    budget=maze_state_budget, detour_nm=maze_detour_nm, phase_budget=maze_phase_budget)
            if not octilinear(path):
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

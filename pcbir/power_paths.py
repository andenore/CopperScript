"""Explicit-copper track-width continuity screening for declared power paths.

This is deliberately separate from net-wide DRC. A legal narrow sense branch
must not become the only connection between two load terminals. The screen
removes subminimum tracks from a read-only connectivity query; it never changes
the board, repairs copper, or grants a width exception.
"""
from __future__ import annotations

from dataclasses import dataclass

from .drc import explicit_copper_connectivity
from .physical import PadReference, PhysicalBoard


@dataclass(frozen=True)
class PowerPathWidth:
    net: str
    terminals: tuple[PadReference, ...]
    minimum_width_nm: int

    def __post_init__(self):
        if not self.net or len(set(self.terminals)) < 2:
            raise ValueError("a power path needs a net and at least two distinct terminals")
        if type(self.minimum_width_nm) is not int or self.minimum_width_nm <= 0:
            raise ValueError("minimum width must be positive integer nanometres")


@dataclass(frozen=True)
class PowerPathWidthResult:
    requirement: PowerPathWidth
    explicit_connected: bool
    width_connected: bool
    excluded_track_indexes: tuple[int, ...]


def check_power_path_widths(board: PhysicalBoard, requirements: tuple[PowerPathWidth, ...]
                           ) -> tuple[PowerPathWidthResult, ...]:
    """Check terminal continuity using pads, vias and width-qualified tracks.

    All physical lands of every selected terminal must join one component.
    Installed-component internal links, polygons and zone outlines/fills are
    excluded: their current paths/minimum webs are not assessed by this screen.
    Pad necks, via capacity, copper thickness, parallel narrow conductors,
    temperature and current distribution still require separate qualification.
    A positive result proves an explicit track-width path, not an ampacity.
    """
    nets = {net.name: set(net.pads) for net in board.nets}
    results = []
    cache = {}
    for requirement in requirements:
        if requirement.net not in nets or not set(requirement.terminals) <= nets[requirement.net]:
            raise ValueError(f"power path terminals do not belong to net {requirement.net!r}")
        key = requirement.net, requirement.minimum_width_nm
        if key not in cache:
            selected = frozenset({requirement.net})
            excluded = tuple(i for i, track in enumerate(board.tracks)
                             if track.net == requirement.net and track.width_nm < requirement.minimum_width_nm)
            tracks = tuple(track for track in board.tracks
                           if track.net == requirement.net and track.width_nm >= requirement.minimum_width_nm)
            full = explicit_copper_connectivity(board, only_nets=selected,
                                               include_internal_connections=False, polygons=())
            wide = explicit_copper_connectivity(board, only_nets=selected,
                                               include_internal_connections=False, tracks=tracks, polygons=())
            cache[key] = full, wide, excluded
        full, wide, excluded = cache[key]

        def connected(graph):
            nodes = [graph.pad_nodes.get(pad, ()) for pad in requirement.terminals]
            return all(nodes) and len({graph.roots[node] for group in nodes for node in group}) == 1

        results.append(PowerPathWidthResult(requirement, bool(connected(full)),
                                           bool(connected(wide)), excluded))
    return tuple(results)

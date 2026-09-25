"""Incremental exact-copper clearance queries for the detailed router.

The grid guides a search; it is not a substitute for checking the copper that
will actually be written.  This index uses the same placed-pad shapes and
integer geometry predicates as physical DRC, with a coarse spatial hash only
as a broad phase.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable

from .drc import non_plated_holes, placed_pad_shape
from .geometry import RoundedConvexShape, shapes_clear
from .physical import (
    BoardSide,
    CopperLayer,
    NetRoutingRule,
    PadKind,
    PadReference,
    PhysicalBoard,
    Point,
    TrackSegment,
    Via,
    nm_from_mm,
)
from .placement import resolved_copper_keepouts, transformed_local_point


@dataclass(frozen=True, slots=True)
class _CopperObject:
    net: str
    layers: tuple[CopperLayer, ...]
    shape: RoundedConvexShape
    clearance_nm: int = 0
    locked: bool = True


@dataclass(frozen=True, slots=True)
class _KeepoutObject:
    layers: frozenset[CopperLayer]
    shape: RoundedConvexShape
    block_tracks: bool
    block_vias: bool
    has_holes: bool


class RoutingClearanceIndex:
    """Query and incrementally reserve physical copper, excluding the same net."""

    def __init__(self, board: PhysicalBoard, bin_size_nm: int = nm_from_mm(2)) -> None:
        self.board = board
        self.bin_size_nm = bin_size_nm
        self.rules: dict[str, NetRoutingRule] = {
            rule.net: rule for rule in board.net_routing_rules
        }
        self._objects: list[_CopperObject] = []
        self._bins: dict[tuple[CopperLayer, int, int], list[int]] = {}
        self._keepouts = tuple(
            _KeepoutObject(
                frozenset(item.layers),
                RoundedConvexShape(item.outline.outer.vertices),
                item.block_tracks,
                item.block_vias,
                bool(item.outline.holes),
            )
            for item in resolved_copper_keepouts(board)
        )
        self._max_clearance_nm = max(
            board.rules.minimum_clearance_nm,
            board.rules.minimum_hole_clearance_nm,
            *(rule.clearance_nm or 0 for rule in self.rules.values()),
            *(footprint.clearance_nm or 0 for footprint in board.footprints.values()),
        )
        assigned = {pad: net.name for net in board.nets for pad in net.pads}
        for placement in sorted(board.placements, key=lambda item: item.reference):
            footprint = board.footprints[placement.footprint]
            for pad_index, pad in enumerate(footprint.pads):
                if pad.kind in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}:
                    continue
                net = assigned.get(
                    PadReference(placement.reference, pad.number),
                    f"<unconnected:{placement.reference}.{pad.number}:{pad_index}>",
                )
                layers = (
                    (CopperLayer.FRONT if placement.side is BoardSide.FRONT else CopperLayer.BACK,)
                    if pad.kind is PadKind.SMD else board.stackup.copper_layers
                )
                position = transformed_local_point(placement, pad.position)
                self._add(_CopperObject(
                    net, tuple(layers), placed_pad_shape(position, pad, placement),
                    footprint.clearance_nm or 0,
                ))
        for _, hole in non_plated_holes(board):
            self._add(_CopperObject(
                "<non-plated-hole>", tuple(board.stackup.copper_layers), hole,
                board.rules.minimum_hole_clearance_nm,
            ))
        for track in board.tracks:
            self.add_track(track, locked=True)
        for via in board.vias:
            self.add_via(via, locked=True)

    def can_track(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer
    ) -> bool:
        shape = RoundedConvexShape((start, end), width_nm // 2)
        return self._keepout_clear(shape, (layer,), for_via=False) and self._clear(net, shape, (layer,))

    def can_via(
        self, net: str, position: Point, size_nm: int,
        from_layer: CopperLayer, to_layer: CopperLayer,
    ) -> bool:
        layers = self._via_layers(from_layer, to_layer)
        shape = RoundedConvexShape((position,), size_nm // 2)
        return self._keepout_clear(shape, layers, for_via=True) and self._clear(net, shape, layers)

    def _keepout_clear(
        self, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...], *, for_via: bool
    ) -> bool:
        layer_set = set(layers)
        for keepout in self._keepouts:
            if not (keepout.block_vias if for_via else keepout.block_tracks):
                continue
            if not layer_set.intersection(keepout.layers):
                continue
            if keepout.has_holes:
                # Match DRC's fail-closed treatment of unsupported holes.
                return False
            if not shape.bounds.intersects(keepout.shape.bounds):
                continue
            if not shapes_clear(shape, keepout.shape):
                return False
        return True

    def add_track(self, track: TrackSegment, *, locked: bool = False) -> None:
        self._add(_CopperObject(
            track.net, (track.layer,),
            RoundedConvexShape((track.start, track.end), track.width_nm // 2),
            locked=locked,
        ))

    def add_via(self, via: Via, *, locked: bool = False) -> None:
        self._add(_CopperObject(
            via.net, self._via_layers(via.from_layer, via.to_layer),
            RoundedConvexShape((via.position,), via.size_nm // 2),
            locked=locked,
        ))

    def blocking_track_nets(self, track: TrackSegment) -> tuple[frozenset[str], bool]:
        """Return movable blocker nets and whether immutable geometry blocks a track."""
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        return self._blockers(track.net, shape, (track.layer,), for_via=False)

    def blocking_via_nets(self, via: Via) -> tuple[frozenset[str], bool]:
        """Return movable blocker nets and whether immutable geometry blocks a via."""
        layers = self._via_layers(via.from_layer, via.to_layer)
        shape = RoundedConvexShape((via.position,), via.size_nm // 2)
        return self._blockers(via.net, shape, layers, for_via=True)

    def _blockers(
        self, net: str, shape: RoundedConvexShape,
        layers: tuple[CopperLayer, ...], *, for_via: bool,
    ) -> tuple[frozenset[str], bool]:
        if not self._keepout_clear(shape, layers, for_via=for_via):
            return frozenset(), True
        movable: set[str] = set()
        locked = False
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            own_rule = self.rules.get(net)
            other_rule = self.rules.get(other.net)
            clearance = max(
                self.board.rules.minimum_clearance_nm,
                own_rule.clearance_nm or 0 if own_rule else 0,
                other_rule.clearance_nm or 0 if other_rule else 0,
                other.clearance_nm,
            )
            if shapes_clear(shape, other.shape, clearance):
                continue
            if other.locked:
                locked = True
            else:
                movable.add(other.net)
        return frozenset(movable), locked

    def _via_layers(
        self, from_layer: CopperLayer, to_layer: CopperLayer
    ) -> tuple[CopperLayer, ...]:
        layers = self.board.stackup.copper_layers
        first, last = sorted((layers.index(from_layer), layers.index(to_layer)))
        return layers[first:last + 1]

    def _add(self, item: _CopperObject) -> None:
        identity = len(self._objects)
        self._objects.append(item)
        bounds = item.shape.bounds
        for layer in item.layers:
            for x in range(bounds.min_x // self.bin_size_nm, bounds.max_x // self.bin_size_nm + 1):
                for y in range(bounds.min_y // self.bin_size_nm, bounds.max_y // self.bin_size_nm + 1):
                    self._bins.setdefault((layer, x, y), []).append(identity)

    def _clear(
        self, net: str, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...]
    ) -> bool:
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            own_rule = self.rules.get(net)
            other_rule = self.rules.get(other.net)
            clearance = max(
                self.board.rules.minimum_clearance_nm,
                own_rule.clearance_nm or 0 if own_rule else 0,
                other_rule.clearance_nm or 0 if other_rule else 0,
                other.clearance_nm,
            )
            if not shapes_clear(shape, other.shape, clearance):
                return False
        return True

    def _overlapping_objects(
        self, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...]
    ) -> Iterable[_CopperObject]:
        area = shape.bounds.expanded(self._max_clearance_nm)
        seen: set[int] = set()
        for layer in layers:
            for x in range(area.min_x // self.bin_size_nm, area.max_x // self.bin_size_nm + 1):
                for y in range(area.min_y // self.bin_size_nm, area.max_y // self.bin_size_nm + 1):
                    for identity in self._bins.get((layer, x, y), ()):
                        if identity in seen:
                            continue
                        seen.add(identity)
                        yield self._objects[identity]

"""Incremental exact-copper clearance queries for the detailed router.

The grid guides a search; it is not a substitute for checking the copper that
will actually be written.  This index uses the same placed-pad shapes and
integer geometry predicates as physical DRC, with a coarse spatial hash only
as a broad phase.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Iterable, Iterator

from .breakout import BreakoutRegions
from .drc import non_plated_holes, placed_pad_shape
from .geometry import Bounds, RoundedConvexShape, shapes_clear
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
    is_pad: bool = False
    pad_reference: PadReference | None = None
    # Terminal land whose breakout region holds this copper (plan R1).
    breakout_land: str | None = None


@dataclass(frozen=True, slots=True)
class _KeepoutObject:
    layers: frozenset[CopperLayer]
    shape: RoundedConvexShape
    block_tracks: bool
    block_vias: bool
    has_holes: bool
    clearance_nm: int = 0


@dataclass(frozen=True, slots=True)
class _DrilledHole:
    net: str
    position: Point
    radius_nm: int
    locked: bool
    reusable: bool = False
    span: tuple[CopperLayer, CopperLayer] | None = None


class RoutingClearanceIndex:
    """Reserve copper; same-net tracks may join, but vias must avoid pads."""

    def __init__(self, board: PhysicalBoard, bin_size_nm: int = nm_from_mm(2)) -> None:
        if bin_size_nm <= 0:
            raise ValueError("clearance bin size must be positive")
        self.board = board
        self.bin_size_nm = bin_size_nm
        self.rules: dict[str, NetRoutingRule] = {
            rule.net: rule for rule in board.net_routing_rules
        }
        # Breakout regions relax width, gap and clearance next to terminal
        # lands. Without breakout rules every query is exactly as before.
        self.breakout = BreakoutRegions(board)
        from .hard_macros import macro_reservations
        self._macro_regions = tuple(
            (frozenset(r.layers), RoundedConvexShape(r.outline.outer.vertices))
            for r in macro_reservations(board)
        )
        self._objects: list[_CopperObject] = []
        self._holes: list[_DrilledHole] = []
        # Ordered insertions; with ``board`` they determine the whole index.
        self._additions: list[tuple[TrackSegment | Via, bool]] = []
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
        self._index_static_regions()
        self._max_clearance_nm = max(
            board.rules.minimum_clearance_nm,
            board.rules.minimum_hole_clearance_nm,
            *(rule.clearance_nm or 0 for rule in self.rules.values()),
            *(footprint.clearance_nm or 0 for footprint in board.footprints.values()),
            # A pair with breakout properties is spaced by its gap, not clearance.
            *(rule.pair_gap_nm or 0 for rule in self.rules.values()
              if rule.breakout_length_nm is not None),
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
                if pad.kind is PadKind.THROUGH_HOLE and pad.drill is not None:
                    self._holes.append(_DrilledHole(
                        net, position, max(pad.drill.width_nm, pad.drill.height_nm) // 2,
                        True,
                    ))
                self._add(_CopperObject(
                    net, tuple(layers), placed_pad_shape(position, pad, placement),
                    footprint.clearance_nm or 0,
                    is_pad=True,
                    pad_reference=PadReference(placement.reference,pad.number),
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

    def route_pieces(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer,
    ) -> tuple[TrackSegment, ...]:
        """Ordinary copper for one centreline, necked down inside breakout regions."""
        return self.breakout.ordinary_pieces(net, start, end, width_nm, layer)

    def can_route(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer,
    ) -> bool:
        """``can_track`` for each piece ordinary routing emits (``route_pieces``).

        Nets without breakout properties check the one ``width_nm`` track.
        """
        if net not in self.breakout.ordinary:
            return self.can_track(net, start, end, width_nm, layer)
        return all(self.can_track(net, piece.start, piece.end, piece.width_nm, layer)
                   for piece in self.route_pieces(net, start, end, width_nm, layer))

    def can_area(self, net: str, shape: RoundedConvexShape, layer: CopperLayer) -> bool:
        """Like ``can_track`` for any swept shape, such as a whole tuning bump."""
        return self._keepout_clear(shape, (layer,), for_via=False) and self._clear(net, shape, (layer,))

    def can_via(
        self, net: str, position: Point, size_nm: int,
        from_layer: CopperLayer, to_layer: CopperLayer,
        drill_nm: int | None = None,
        *, check_hole_copper: bool = False, allow_pad_overlap: bool = False,
        allowed_pad: PadReference | None = None,
    ) -> bool:
        layers = self._via_layers(from_layer, to_layer)
        shape = RoundedConvexShape((position,), size_nm // 2)
        return (self._keepout_clear(shape, layers, for_via=True)
                and (self.pad_copper_clear(shape,layers,allowed_pad=allowed_pad)
                     if allowed_pad is not None else
                     allow_pad_overlap or self.pad_copper_clear(shape, layers))
                and self._clear(net, shape, layers)
                and (not check_hole_copper or self._clear(
                    net, RoundedConvexShape(
                        (position,), (drill_nm or self.board.rules.default_via_drill_nm) // 2
                    ), layers,
                    clearance_floor_nm=self.board.rules.minimum_hole_clearance_nm,
                ))
                and self._hole_clear(
                    net, position, (drill_nm or self.board.rules.default_via_drill_nm) // 2,
                    (from_layer, to_layer),
                 ))

    def candidate_vias_clear(self, vias: Iterable[Via]) -> bool:
        """Validate a net's whole tentative via set, not just each via alone.

        Search checks against committed copper incrementally, but several
        branches of one net are materialized together. Their drill envelopes
        must also be compared with one another before accepting the route.
        """

        return self.candidate_via_conflict(vias) is None

    def candidate_via_conflict(
        self, vias: Iterable[Via], *, allow_movable_conflicts: bool = False,
    ) -> Via | None:
        """Return the first tentative via that cannot coexist with its peers."""

        proposed = tuple(vias)
        for index, via in enumerate(proposed):
            if allow_movable_conflicts:
                _, locked = self.blocking_via_nets(via)
                legal = not locked
            else:
                legal = self.can_via(
                    via.net, via.position, via.size_nm, via.from_layer, via.to_layer,
                    drill_nm=via.drill_nm,
                )
            if not legal:
                return via
            for earlier in proposed[:index]:
                distance_squared = (
                    (via.position.x_nm - earlier.position.x_nm) ** 2
                    + (via.position.y_nm - earlier.position.y_nm) ** 2
                )
                required = (
                    via.drill_nm // 2 + earlier.drill_nm // 2
                    + self.board.rules.minimum_hole_clearance_nm
                )
                if distance_squared < required * required:
                    return via
        return None

    def pad_copper_clear(self, shape: RoundedConvexShape,
                         layers: tuple[CopperLayer, ...], *,
                         allowed_pad: PadReference | None = None) -> bool:
        """No pad contact, independent of net (including the full annulus)."""
        # Include same-net and unassigned pads, and the complete annulus rather
        # than only the drill/center. A one-nanometre separation rejects contact
        # too; foreign copper still observes the ordinary clearance rule.
        return all(not other.is_pad or (allowed_pad is not None and other.pad_reference == allowed_pad)
                   or shapes_clear(shape, other.shape, 1)
                   for other in self._overlapping_objects(shape, layers))

    def _hole_clear(
        self, net: str, position: Point, radius_nm: int,
        span: tuple[CopperLayer, CopperLayer],
    ) -> bool:
        for hole in self._holes:
            distance_squared = ((position.x_nm - hole.position.x_nm) ** 2
                                + (position.y_nm - hole.position.y_nm) ** 2)
            if (distance_squared == 0 and hole.net == net and hole.reusable
                    and hole.span == span):
                continue  # Reuse an existing same-net through via, not a new drill.
            required = radius_nm + hole.radius_nm + self.board.rules.minimum_hole_clearance_nm
            if distance_squared < required * required:
                return False
        return True

    def _keepout_clear(
        self, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...], *, for_via: bool
    ) -> bool:
        if self.board.outline.circular_boundary or self.board.outline.boundary_path or self.board.outline.cutouts or self.board.mechanical_holes or self.board.mechanical_slots:
            from .mechanical import shape_in_board
            if not shape_in_board(self.board, shape, self.board.rules.minimum_clearance_nm,
                                  self.board.rules.minimum_hole_clearance_nm):
                return False  # Always locked, including speculative soft rip-up.
        if not self._static_regions:
            return True
        # Unsupported keepout holes fail closed even far from the query, as in
        # native DRC and the original linear implementation.
        blocked_layers = self._static_hole_via_layers if for_via else self._static_hole_track_layers
        if any(layer in blocked_layers for layer in layers):
            return False
        for keepout in self._overlapping_static_regions(shape, layers):
            if not (keepout.block_vias if for_via else keepout.block_tracks):
                continue
            area = shape.bounds.expanded(keepout.clearance_nm) if keepout.clearance_nm else shape.bounds
            if not area.intersects(keepout.shape.bounds):
                continue
            if not shapes_clear(shape, keepout.shape, keepout.clearance_nm):
                return False
        return True

    _STATIC_BIN_LIMIT = 512

    def _index_static_regions(self) -> None:
        """Build a snapshot-local broad phase, separate from mutable copper.

        Large regions stay in per-layer fallback lists rather than allocating
        board-sized bin arrays. Queries with huge envelopes scan layer lists.
        Original indices preserve deterministic deduplication/order.
        """
        self._static_regions = (*(_KeepoutObject(layers, shape, True, True, False, 1)
                                  for layers, shape in self._macro_regions),
                                *(region for region in self._keepouts
                                  if region.block_tracks or region.block_vias))
        bins, large, by_layer = {}, {}, {}
        hole_tracks, hole_vias = set(), set()
        for identity, region in enumerate(self._static_regions):
            area = region.shape.bounds
            xs, ys = self._static_cells(area)
            for layer in region.layers:
                by_layer.setdefault(layer, []).append(identity)
                if region.has_holes:
                    if region.block_tracks:
                        hole_tracks.add(layer)
                    if region.block_vias:
                        hole_vias.add(layer)
                if len(xs) * len(ys) > self._STATIC_BIN_LIMIT:
                    large.setdefault(layer, []).append(identity)
                else:
                    for x in xs:
                        for y in ys:
                            bins.setdefault((layer, x, y), []).append(identity)
        self._static_bins = {key: tuple(ids) for key, ids in bins.items()}
        self._static_large = {key: tuple(ids) for key, ids in large.items()}
        self._static_by_layer = {key: tuple(ids) for key, ids in by_layer.items()}
        self._static_hole_track_layers = frozenset(hole_tracks)
        self._static_hole_via_layers = frozenset(hole_vias)

    def _static_cells(self, area: Bounds) -> tuple[range, range]:
        return (range(area.min_x // self.bin_size_nm, area.max_x // self.bin_size_nm + 1),
                range(area.min_y // self.bin_size_nm, area.max_y // self.bin_size_nm + 1))

    def _overlapping_static_regions(self, shape: RoundedConvexShape,
                                     layers: tuple[CopperLayer, ...]) -> Iterator[_KeepoutObject]:
        # Macro access reservations reject even exact contact (one nm margin).
        # Expand the QUERY so an adjacent-bin contact cannot be lost.
        if not any(layer in self._static_by_layer for layer in layers):
            return
        xs, ys = self._static_cells(shape.bounds.expanded(1))
        identities = set()
        for layer in layers:
            if layer not in self._static_by_layer:
                continue
            if len(xs) * len(ys) > self._STATIC_BIN_LIMIT:
                identities.update(self._static_by_layer.get(layer, ()))
            else:
                identities.update(self._static_large.get(layer, ()))
                for x in xs:
                    for y in ys:
                        identities.update(self._static_bins.get((layer, x, y), ()))
        for identity in sorted(identities):
            yield self._static_regions[identity]

    def additions(self) -> tuple[tuple[TrackSegment | Via, bool], ...]:
        """Every added track/via with its lock flag, in insertion order.

        All other index state derives from ``board`` and ``bin_size_nm``, so
        these three values identify an index exactly, e.g. for search reuse.
        """
        return tuple(self._additions)

    def add_track(self, track: TrackSegment, *, locked: bool = False) -> None:
        self._additions.append((track, locked))
        self._add(_CopperObject(
            track.net, (track.layer,),
            RoundedConvexShape((track.start, track.end), track.width_nm // 2),
            locked=locked,
        ))

    def add_via(self, via: Via, *, locked: bool = False) -> None:
        self._additions.append((via, locked))
        self._holes.append(_DrilledHole(
            via.net, via.position, via.drill_nm // 2, locked, True,
            (via.from_layer, via.to_layer),
        ))
        self._add(_CopperObject(
            via.net, self._via_layers(via.from_layer, via.to_layer),
            RoundedConvexShape((via.position,), via.size_nm // 2),
            locked=locked,
        ))

    def blocking_track_nets(self, track: TrackSegment) -> tuple[frozenset[str], bool]:
        """Return movable blocker nets and whether immutable geometry blocks a track."""
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        return self._blockers(track.net, shape, (track.layer,), for_via=False)

    def route_blockers(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer,
    ) -> tuple[frozenset[str], bool]:
        """``blocking_track_nets`` over the pieces of ``route_pieces``."""
        if net not in self.breakout.ordinary:
            return self.blocking_track_nets(TrackSegment(net, start, end, width_nm, layer))
        movable: set[str] = set()
        locked = False
        for piece in self.route_pieces(net, start, end, width_nm, layer):
            names, fixed = self.blocking_track_nets(piece)
            movable.update(names)
            locked |= fixed
        return frozenset(movable), locked

    def blocking_via_nets(self, via: Via) -> tuple[frozenset[str], bool]:
        """Return movable blocker nets and whether immutable geometry blocks a via."""
        layers = self._via_layers(via.from_layer, via.to_layer)
        shape = RoundedConvexShape((via.position,), via.size_nm // 2)
        movable, locked = self._blockers(via.net, shape, layers, for_via=True)
        locked = locked or not self.pad_copper_clear(shape, layers)
        movable = set(movable)
        for hole in self._holes:
            distance_squared = ((via.position.x_nm - hole.position.x_nm) ** 2
                                + (via.position.y_nm - hole.position.y_nm) ** 2)
            if (distance_squared == 0 and hole.net == via.net and hole.reusable
                    and hole.span == (via.from_layer, via.to_layer)):
                continue
            required = (via.drill_nm // 2 + hole.radius_nm
                        + self.board.rules.minimum_hole_clearance_nm)
            if distance_squared < required * required:
                if hole.locked or hole.net == via.net:
                    locked = True
                else:
                    movable.add(hole.net)
        return frozenset(movable), locked

    def _blockers(
        self, net: str, shape: RoundedConvexShape,
        layers: tuple[CopperLayer, ...], *, for_via: bool,
    ) -> tuple[frozenset[str], bool]:
        if not self._keepout_clear(shape, layers, for_via=for_via):
            return frozenset(), True
        movable: set[str] = set()
        locked = False
        land = self._breakout_land(net, shape)
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            clearance = max(self._rule_clearance(net, land, other), other.clearance_nm)
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
        if item.net in self.breakout.declared:
            item = replace(item, breakout_land=self.breakout.region(item.net, item.shape.spine))
        identity = len(self._objects)
        self._objects.append(item)
        bounds = item.shape.bounds
        for layer in item.layers:
            for x in range(bounds.min_x // self.bin_size_nm, bounds.max_x // self.bin_size_nm + 1):
                for y in range(bounds.min_y // self.bin_size_nm, bounds.max_y // self.bin_size_nm + 1):
                    self._bins.setdefault((layer, x, y), []).append(identity)

    def _clear(
        self, net: str, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...],
        *, clearance_floor_nm: int = 0,
    ) -> bool:
        land = self._breakout_land(net, shape)
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            clearance = max(
                self._rule_clearance(net, land, other),
                clearance_floor_nm,
                other.clearance_nm,
            )
            if not shapes_clear(shape, other.shape, clearance):
                return False
        return True

    def _breakout_land(self, net: str, shape: RoundedConvexShape) -> str | None:
        """The breakout region holding a queried shape's centreline, if any."""
        if net not in self.breakout.declared:
            return None
        return self.breakout.region(net, shape.spine)

    def _rule_clearance(self, net: str, land: str | None, other: _CopperObject) -> int:
        """Net-rule spacing between queried copper and ``other``, region-aware."""
        if self.breakout:
            return self.breakout.spacing_nm(net, land, other.net, other.breakout_land)
        own_rule = self.rules.get(net)
        other_rule = self.rules.get(other.net)
        return max(
            self.board.rules.minimum_clearance_nm,
            own_rule.clearance_nm or 0 if own_rule else 0,
            other_rule.clearance_nm or 0 if other_rule else 0,
        )

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

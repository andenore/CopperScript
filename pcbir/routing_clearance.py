"""Incremental exact-copper clearance queries for the detailed router.

The grid guides a search; it is not a substitute for checking the copper that
will actually be written.  This index uses the same placed-pad shapes and
integer geometry predicates as physical DRC, with a coarse spatial hash only
as a broad phase.
"""

from __future__ import annotations

from dataclasses import dataclass, fields, is_dataclass, replace
from typing import Iterable, Iterator
from hashlib import sha256
from enum import Enum
from decimal import Decimal
from functools import lru_cache
import json

from ._cache import IdentityKey
from .breakout import BreakoutLand, BreakoutRegions, _nearest_region
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
from .zone_geometry import ZoneRoutingReservations


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
    occurrence: str = ""


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
    occurrence: str = ""


@dataclass(frozen=True, slots=True)
class _PadIndexTemplate:
    objects: tuple[_CopperObject, ...]
    holes: tuple[_DrilledHole, ...]
    bins: tuple[tuple[tuple[CopperLayer, int, int], tuple[int, ...]], ...]


@lru_cache(maxsize=512)
def _posed_pad_geometry(footprint_key: IdentityKey, pose_key: IdentityKey):
    """Only immutable original land geometry, with no rules or clearance result."""
    footprint, pose = footprint_key.value, pose_key.value
    return tuple((index, pad, position, placed_pad_shape(position, pad, pose))
                 for index, pad in enumerate(footprint.pads)
                 if pad.kind not in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}
                 for position in (transformed_local_point(pose, pad.position),))


@lru_cache(maxsize=8)
def _pad_index_template(poses_key: IdentityKey, footprints, nets_key: IdentityKey,
                        stackup_key: IdentityKey, rules_key: IdentityKey,
                        routing_rules_key: IdentityKey, bin_size_nm: int):
    """Immutable pad broad phase shared by copper-only board replacements.

    Identity keys hold their immutable inputs strongly, including complete
    source policy epochs. No board, mutable index, clearance answer, generated
    copper, zone or macro reservation is cached here. Breakout-owner strings
    are immutable source geometry under this complete source epoch, never
    answers to dynamic copper-clearance queries.
    """
    assigned = {pad: net.name for net in nets_key.value for pad in net.pads}
    footprint_map = dict(footprints)
    objects, holes, bins = [], [], {}
    lengths = {rule.net: rule.breakout_length_nm for rule in routing_rules_key.value
               if rule.breakout_length_nm is not None}
    lands = {}
    for pose in sorted(poses_key.value, key=lambda item: item.reference):
        footprint_key = footprint_map[pose.footprint]
        footprint = footprint_key.value
        for pad_index, pad, position, shape in _posed_pad_geometry(footprint_key, IdentityKey(pose)):
            reference = PadReference(pose.reference, pad.number)
            net = assigned.get(reference, f"<unconnected:{pose.reference}.{pad.number}:{pad_index}>")
            layers = ((CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK,)
                      if pad.kind is PadKind.SMD else stackup_key.value.copper_layers)
            if pad.kind is PadKind.THROUGH_HOLE and pad.drill is not None:
                holes.append(_DrilledHole(net, position,
                    max(pad.drill.width_nm, pad.drill.height_nm) // 2, True,
                    occurrence=f"pad:{pose.reference}.{pad.number}:{pad_index}"))
            identity = len(objects)
            objects.append(_CopperObject(net, tuple(layers), shape, footprint.clearance_nm or 0,
                                         is_pad=True, pad_reference=reference,
                                         occurrence=f"pad:{pose.reference}.{pad.number}:{pad_index}"))
            if reference in assigned and net in lengths:
                length = lengths[net]
                lands.setdefault(net, []).append(BreakoutLand(
                    f"pad:{pose.reference}.{pad.number}:{pad_index}", net, shape,
                    length, shape.bounds.expanded(length)))
            bounds = shape.bounds
            for layer in layers:
                for x in range(bounds.min_x // bin_size_nm, bounds.max_x // bin_size_nm + 1):
                    for y in range(bounds.min_y // bin_size_nm, bounds.max_y // bin_size_nm + 1):
                        bins.setdefault((layer, x, y), []).append(identity)
    owned = tuple(replace(item, breakout_land=_nearest_region(lands.get(item.net, ()), item.shape.spine))
                  if item.net in lengths else item for item in objects)
    return _PadIndexTemplate(owned, tuple(holes),
                             tuple((key, tuple(ids)) for key, ids in bins.items()))


def _pad_template_for(board: PhysicalBoard, bin_size_nm: int) -> _PadIndexTemplate:
    return _pad_index_template(IdentityKey(board.placements),
        tuple((name, IdentityKey(footprint)) for name, footprint in board.footprints.items()),
        IdentityKey(board.nets), IdentityKey(board.stackup), IdentityKey(board.rules),
        IdentityKey(board.net_routing_rules), bin_size_nm)


@dataclass(frozen=True, slots=True)
class RoutingBlocker:
    identity: str
    reason: str
    net: str | None
    immutable: bool
    layers: tuple[str, ...]
    positions_nm: tuple[tuple[int, int], ...]
    # Snapshot-local occurrence, distinct even for geometrically equal objects.
    # A lock flag is authority; this diagnostic label never grants mutability.
    occurrence: str = ""


def _geometry_identity(value) -> str:
    def canonical(item):
        if isinstance(item, Enum):
            return item.value
        if isinstance(item, Decimal):
            return format(item, "f")
        if is_dataclass(item):
            return {field.name: canonical(getattr(item, field.name)) for field in fields(item)}
        if isinstance(item, (set, frozenset)):
            return sorted((canonical(part) for part in item), key=lambda part: json.dumps(part, sort_keys=True))
        if isinstance(item, (tuple, list)):
            return [canonical(part) for part in item]
        return item
    return sha256(json.dumps(canonical(value), sort_keys=True, separators=(",", ":")).encode()).hexdigest()


class RoutingClearanceIndex:
    """Reserve copper; same-net tracks may join, but vias must avoid pads."""

    def __init__(self, board: PhysicalBoard, bin_size_nm: int = nm_from_mm(2), *,
                 _deferred_components: frozenset[str] = frozenset()) -> None:
        if bin_size_nm <= 0:
            raise ValueError("clearance bin size must be positive")
        self.board = board
        from .terminal_preferences import TerminalPreferences
        self.terminal_preferences = TerminalPreferences(board)
        self.zone_reservations = ZoneRoutingReservations(board)
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
        template = _pad_template_for(board, bin_size_nm)
        # The cached template is immutable. Each index owns fresh containers;
        # Source breakout-owner strings belong to that complete source epoch;
        # dynamic breakout queries and all mutable reservations remain local.
        self._objects = list(template.objects)
        self._holes = list(template.holes)
        self._bins = {key: list(ids) for key, ids in template.bins}
        if _deferred_components:
            # Private support-placement proposal view. The caller must restore
            # every land and validate the complete new pose before adoption.
            # Never mutate the shared immutable pad template or its cache.
            poses = {p.reference:p for p in board.placements}
            if not _deferred_components <= poses.keys() or any(
                    pad.kind is not PadKind.SMD
                    for ref in _deferred_components for pad in board.footprints[poses[ref].footprint].pads):
                raise ValueError('deferred placement view requires known SMD-only components')
            self._objects = []; self._bins = {}
            for obj in template.objects:
                if obj.pad_reference.component not in _deferred_components:
                    self._add(obj)
        for _, hole in non_plated_holes(board):
            self._add(_CopperObject(
                "<non-plated-hole>", tuple(board.stackup.copper_layers), hole,
                board.rules.minimum_hole_clearance_nm,
            ))
        for ordinal, track in enumerate(board.tracks):
            self.add_track(track, locked=True, occurrence=f"track:{ordinal}")
        for ordinal, via in enumerate(board.vias):
            self.add_via(via, locked=True, occurrence=f"via:{ordinal}")

    def can_track(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer
    ) -> bool:
        shape = RoundedConvexShape((start, end), width_nm // 2)
        return (self.zone_reservations.can_area(net, shape, layer)
                and self._keepout_clear(shape, (layer,), for_via=False)
                and self._clear(net, shape, (layer,)))

    def route_pieces(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer,
    ) -> tuple[TrackSegment, ...]:
        """Prefer the requested width; use permitted neckdowns only if needed.

        This selects copper, not permission to commit it: callers still check
        every returned piece. Critical routing owns its coupled width profile.
        """
        if net not in self.breakout.ordinary:
            tracks = (TrackSegment(net, start, end, width_nm, layer),)
            # Exact local/global terminal access uses the authored critical
            # breakout widths too. Ordinary nets retain preferred-width trials.
            return self.breakout.split_tracks(tracks, width_nm) if net in self.breakout.declared else tracks
        from .surface_path import _track_inside_board

        def fits(first: Point, second: Point) -> bool:
            return (self.can_track(net, first, second, width_nm, layer)
                    and _track_inside_board(self.board, first, second, width_nm))

        if fits(start, end):
            return (TrackSegment(net, start, end, width_nm, layer),)
        minimum = self.breakout.ordinary_pieces(net, start, end, width_nm, layer)
        return tuple(replace(piece, width_nm=width_nm)
                     if piece.width_nm < width_nm and fits(piece.start, piece.end)
                     else piece for piece in minimum)

    def can_route(
        self, net: str, start: Point, end: Point, width_nm: int, layer: CopperLayer,
    ) -> bool:
        """``can_track`` for each piece ordinary routing emits (``route_pieces``).

        Nets without breakout properties check the one ``width_nm`` track.
        """
        if net not in self.breakout.declared:
            return self.can_track(net, start, end, width_nm, layer)
        return all(self.can_track(net, piece.start, piece.end, piece.width_nm, layer)
                   for piece in self.route_pieces(net, start, end, width_nm, layer))

    def can_area(self, net: str, shape: RoundedConvexShape, layer: CopperLayer) -> bool:
        """Like ``can_track`` for any swept shape, such as a whole tuning bump."""
        return (self.zone_reservations.can_area(net, shape, layer)
                and self._keepout_clear(shape, (layer,), for_via=False)
                and self._clear(net, shape, (layer,)))

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
                # All plated drills must clear foreign copper, not only
                # filled/capped contacts. Keep the keyword for old callers.
                and self._clear(
                    net, RoundedConvexShape(
                        (position,), (drill_nm or self.board.rules.default_via_drill_nm) // 2
                    ), layers,
                    clearance_floor_nm=self.board.rules.minimum_hole_clearance_nm,
                )
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
                if via.net != earlier.net and set(self._via_layers(via.from_layer, via.to_layer)).intersection(
                        self._via_layers(earlier.from_layer, earlier.to_layer)):
                    earlier_shape = RoundedConvexShape((earlier.position,), earlier.size_nm // 2)
                    earlier_copper = _CopperObject(earlier.net,
                        self._via_layers(earlier.from_layer, earlier.to_layer), earlier_shape,
                        breakout_land=self._breakout_land(earlier.net, earlier_shape))
                    clearance = self._rule_clearance(via.net,
                        self._breakout_land(via.net, RoundedConvexShape((via.position,), via.size_nm // 2)),
                        earlier_copper)
                    required = max(required,
                        via.drill_nm // 2 + earlier.size_nm // 2 + self.board.rules.minimum_hole_clearance_nm,
                        earlier.drill_nm // 2 + via.size_nm // 2 + self.board.rules.minimum_hole_clearance_nm,
                        via.size_nm // 2 + earlier.size_nm // 2 + clearance)
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
        # Every outline, plain polygons included: the detailed grid spans the
        # outline's bounds inclusively, so edge nodes would otherwise admit
        # copper the exported copper-to-edge clearance rejects.
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

    def add_track(self, track: TrackSegment, *, locked: bool = False,
                  occurrence: str | None = None) -> None:
        occurrence = occurrence or f"addition:{len(self._additions)}:track"
        self._additions.append((track, locked))
        self._add(_CopperObject(
            track.net, (track.layer,),
            RoundedConvexShape((track.start, track.end), track.width_nm // 2),
            locked=locked,
            occurrence=occurrence,
        ))

    def add_via(self, via: Via, *, locked: bool = False,
                occurrence: str | None = None) -> None:
        occurrence = occurrence or f"addition:{len(self._additions)}:via"
        self._additions.append((via, locked))
        self._holes.append(_DrilledHole(
            via.net, via.position, via.drill_nm // 2, locked, True,
            (via.from_layer, via.to_layer),
            occurrence,
        ))
        self._add(_CopperObject(
            via.net, self._via_layers(via.from_layer, via.to_layer),
            RoundedConvexShape((via.position,), via.size_nm // 2),
            locked=locked,
            occurrence=occurrence,
        ))
        self._add(_CopperObject(
            via.net, self._via_layers(via.from_layer, via.to_layer),
            RoundedConvexShape((via.position,), via.drill_nm // 2),
            clearance_nm=self.board.rules.minimum_hole_clearance_nm,
            locked=locked,
            occurrence=occurrence,
        ))

    def blocking_track_nets(self, track: TrackSegment) -> tuple[frozenset[str], bool]:
        """Return movable blocker nets and whether immutable geometry blocks a track."""
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        return self._blockers(track.net, shape, (track.layer,), for_via=False)

    def track_blocker_records(self, track: TrackSegment) -> tuple[RoutingBlocker, ...]:
        """Snapshot-bound exact blocker evidence; does not relax the query."""
        shape = RoundedConvexShape((track.start, track.end), track.width_nm // 2)
        return self._blocker_records(track.net, shape, (track.layer,), for_via=False)

    def _blocker_records(self, net, shape, layers, *, for_via, clearance_floor_nm=0):
        records = []
        def add(identity, reason, other_net, locked, other_layers, points, occurrence=""):
            records.append(RoutingBlocker(identity, reason, other_net, locked,
                tuple(layer.value for layer in other_layers),
                tuple((p.x_nm, p.y_nm) for p in points), occurrence))
        if not for_via:
            for layer in layers:
                for zone in self.zone_reservations.blocking_zones(net, shape, layer):
                    add("zone:" + zone.id, "source routing reservation", zone.net, True,
                        (layer,), zone.outline.outer.vertices)
        for region in self._static_regions:
            if (set(layers).intersection(region.layers)
                    and (region.block_vias if for_via else region.block_tracks)
                    and (region.has_holes or not shapes_clear(shape, region.shape, region.clearance_nm))):
                add("region:" + _geometry_identity(region), "keepout or immutable macro region",
                    None, True, sorted(region.layers, key=lambda layer: layer.value), region.shape.spine)
        land = self._breakout_land(net, shape)
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            required = max(self._rule_clearance(net, land, other), other.clearance_nm, clearance_floor_nm)
            if not shapes_clear(shape, other.shape, required):
                identity = (f"pad:{other.pad_reference.component}.{other.pad_reference.pad}:"
                            if other.pad_reference else "copper:") + _geometry_identity(other)
                add(identity, "pad clearance" if other.is_pad else "copper clearance",
                    other.net, other.locked, other.layers, other.shape.spine, other.occurrence)
        if (not self._keepout_clear(shape, layers, for_via=for_via)
                and not any(item.net is None for item in records)):
            add("board-material:" + _geometry_identity(self.board.outline),
                "board material, hole or unsupported keepout geometry", None, True, layers, shape.spine)
        return tuple(sorted(set(records), key=lambda item: (item.identity, item.occurrence, item.reason)))

    def via_blocker_records(self, via: Via) -> tuple[RoutingBlocker, ...]:
        layers = self._via_layers(via.from_layer, via.to_layer)
        shape = RoundedConvexShape((via.position,), via.size_nm // 2)
        records = list(self._blocker_records(via.net, shape, layers, for_via=True))
        records.extend(self._blocker_records(via.net,
            RoundedConvexShape((via.position,), via.drill_nm // 2), layers, for_via=True,
            clearance_floor_nm=self.board.rules.minimum_hole_clearance_nm))
        for other in self._overlapping_objects(shape, layers):
            if other.is_pad and not shapes_clear(shape, other.shape, 1):
                records.append(RoutingBlocker("pad-contact:" + _geometry_identity(other),
                    "via annulus must avoid pad copper", other.net, True,
                    tuple(layer.value for layer in other.layers),
                    tuple((p.x_nm, p.y_nm) for p in other.shape.spine), other.occurrence))
        for hole in self._holes:
            distance = (via.position.x_nm-hole.position.x_nm)**2 + (via.position.y_nm-hole.position.y_nm)**2
            if (distance == 0 and hole.net == via.net and hole.reusable
                    and hole.span == (via.from_layer, via.to_layer)):
                continue
            required = via.drill_nm//2 + hole.radius_nm + self.board.rules.minimum_hole_clearance_nm
            if distance < required**2:
                records.append(RoutingBlocker("drill:" + _geometry_identity(hole), "drill spacing",
                    hole.net, hole.locked or hole.net == via.net,
                    tuple(layer.value for layer in layers), ((hole.position.x_nm, hole.position.y_nm),),
                    hole.occurrence))
        return tuple(sorted(set(records), key=lambda item: (item.identity, item.occurrence, item.reason)))

    def via_obstacle_bounds(self, net, position, size_nm, drill_nm, from_layer, to_layer,
                            radius_nm, maximum_obstacles):
        """Bounded nearby frontier proposals, not a clearance approximation.

        All spanned layers participate, including opposite-face same-net lands.
        Rounded shapes keep their exact legality predicates elsewhere.
        """
        layers = self._via_layers(from_layer, to_layer)
        shape = RoundedConvexShape((position,), size_nm // 2)
        land = self._breakout_land(net, shape)
        boundaries = set()
        for other in self.nearby_copper(shape, layers, radius_nm):
            if other.net == net and not other.is_pad:
                continue
            margin = (size_nm//2 + 1 if other.net == net else max(
                size_nm//2 + max(self._rule_clearance(net, land, other), other.clearance_nm),
                drill_nm//2 + self.board.rules.minimum_hole_clearance_nm))
            boundaries.add(other.shape.bounds.expanded(margin))
        for hole in self._holes:
            if (abs(hole.position.x_nm-position.x_nm) <= radius_nm+size_nm
                    and abs(hole.position.y_nm-position.y_nm) <= radius_nm+size_nm):
                margin = hole.radius_nm + drill_nm//2 + self.board.rules.minimum_hole_clearance_nm
                boundaries.add(RoundedConvexShape((hole.position,), margin).bounds)
        for region in self._overlapping_static_regions(
                RoundedConvexShape((position,), radius_nm+size_nm//2), layers):
            if region.block_vias:
                boundaries.add(region.shape.bounds.expanded(size_nm//2+region.clearance_nm+1))
        return tuple(sorted(boundaries, key=lambda b: (
            max(b.min_x-position.x_nm, 0, position.x_nm-b.max_x)**2
            + max(b.min_y-position.y_nm, 0, position.y_nm-b.max_y)**2, b))[:maximum_obstacles])

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
        drill_movable, drill_locked = self._blockers(via.net,
            RoundedConvexShape((via.position,), via.drill_nm // 2), layers,
            for_via=True, clearance_floor_nm=self.board.rules.minimum_hole_clearance_nm)
        movable = movable | drill_movable
        locked |= drill_locked
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
        layers: tuple[CopperLayer, ...], *, for_via: bool, clearance_floor_nm: int = 0,
    ) -> tuple[frozenset[str], bool]:
        if not for_via and any(not self.zone_reservations.can_area(net, shape, layer) for layer in layers):
            return frozenset(), True
        if not self._keepout_clear(shape, layers, for_via=for_via):
            return frozenset(), True
        movable: set[str] = set()
        locked = False
        land = self._breakout_land(net, shape)
        for other in self._overlapping_objects(shape, layers):
            if other.net == net:
                continue
            clearance = max(self._rule_clearance(net, land, other), other.clearance_nm,
                            clearance_floor_nm)
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
        return self.nearby_copper(shape, layers, self._max_clearance_nm)

    def nearby_copper(
        self, shape: RoundedConvexShape, layers: tuple[CopperLayer, ...], distance_nm: int,
    ) -> Iterator[_CopperObject]:
        """Indexed copper on ``layers`` in the bins within ``distance_nm`` of ``shape``.

        The broad phase of every clearance query, also used by distance
        reports: pads, tracks, vias, via drills and non-plated holes, each
        once, in a deterministic order.
        """
        area = shape.bounds.expanded(distance_nm)
        seen: set[int] = set()
        for layer in layers:
            for x in range(area.min_x // self.bin_size_nm, area.max_x // self.bin_size_nm + 1):
                for y in range(area.min_y // self.bin_size_nm, area.max_y // self.bin_size_nm + 1):
                    for identity in self._bins.get((layer, x, y), ()):
                        if identity in seen:
                            continue
                        seen.add(identity)
                        yield self._objects[identity]

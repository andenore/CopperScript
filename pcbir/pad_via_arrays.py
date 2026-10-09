"""Required filled/capped via arrays in exposed pads, placed before routing.

A ``via_in_pad`` permission with ``rows`` and ``columns`` requires a centred
grid of 0.30/0.20 mm filled/capped through-vias inside its single SMD land.
The array is fixed owner copper: ``materialize_hard_macros`` commits it with
any macro copper, before package access, critical and ordinary routing, and
plane stitching accepts it as that pad's contact. Every site passes the same
exact checks as the single fallback via; one failed site rejects the array.
"""

from __future__ import annotations

from collections import Counter
from dataclasses import replace
from decimal import Decimal
from types import SimpleNamespace

from .drc import _rotate_offset, placed_pad_shape
from .geometry import RoundedConvexShape, circle_inside_shape, point_in_polygon
from .physical import (
    FootprintPad, PadKind, PadReference, PadViaInPadRule, PhysicalBoard, Placement,
    Point, PolygonWithHoles, Via, nm_from_mm,
)
from .placement import resolved_copper_keepouts, transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .surface_path import via_inside_board

# Today's single fallback via size; the process qualification is unchanged.
ARRAY_VIA_SIZE_NM = nm_from_mm("0.30")
ARRAY_VIA_DRILL_NM = nm_from_mm("0.20")
# Default pitches are rounded down to this grid for readable coordinates.
_PITCH_GRID_NM = nm_from_mm("0.01")
# ``placed_pad_shape`` reads only its placement's rotation.
_UNROTATED = SimpleNamespace(rotation_degrees=Decimal(0))


def array_rules(board: PhysicalBoard) -> tuple[PadViaInPadRule, ...]:
    """Permissions that require an array, in deterministic pad order."""
    return tuple(sorted((rule for rule in board.via_in_pad_rules if rule.rows is not None),
                        key=lambda rule: rule.pad))


def array_pitch_nm(pad: FootprintPad, rule: PadViaInPadRule) -> int:
    """Explicit pitch, else the square pitch centring the vias in equal cells."""
    if rule.pitch_nm is not None:
        return rule.pitch_nm
    cells = [extent // count for extent, count in (
        (pad.size.width_nm, rule.columns), (pad.size.height_nm, rule.rows)) if count > 1]
    return min(cells) // _PITCH_GRID_NM * _PITCH_GRID_NM if cells else 0


def _offsets(pad: FootprintPad, rule: PadViaInPadRule) -> tuple[tuple[int, int, int, int], ...]:
    """(row, column, dx, dy) in the land's own axes, row-major from -y, -x."""
    pitch = array_pitch_nm(pad, rule)
    return tuple(
        (row + 1, column + 1,
         (2 * column - rule.columns + 1) * pitch // 2,
         (2 * row - rule.rows + 1) * pitch // 2)
        for row in range(rule.rows) for column in range(rule.columns)
    )


def validate_array_fit(board: PhysicalBoard, rule: PadViaInPadRule,
                       lands: list[FootprintPad]) -> None:
    """Placement-independent checks: one land, drill spacing and containment."""
    name = f"{rule.pad.component}.{rule.pad.pad}"
    if len(lands) != 1:
        raise ValueError(f"via_in_pad array {name} requires exactly one SMD land")
    pad = lands[0]
    pitch = array_pitch_nm(pad, rule)
    spacing = ARRAY_VIA_DRILL_NM + board.rules.minimum_hole_clearance_nm
    if rule.rows * rule.columns > 1 and pitch < spacing:
        raise ValueError(f"via_in_pad array {name} pitch {_mm(pitch)} mm is below the "
                         f"{_mm(spacing)} mm drill spacing")
    land = placed_pad_shape(Point(0, 0), replace(pad, rotation_degrees=Decimal(0)), _UNROTATED)
    for row, column, dx, dy in _offsets(pad, rule):
        if not circle_inside_shape(Point(dx, dy), ARRAY_VIA_SIZE_NM // 2, land):
            raise ValueError(f"via_in_pad array {name} site row {row} column {column} "
                             f"does not fit inside the land at pitch {_mm(pitch)} mm")


def _land(board: PhysicalBoard, reference: PadReference) -> tuple[Placement, FootprintPad]:
    placement = next(item for item in board.placements if item.reference == reference.component)
    pad = next(item for item in board.footprints[placement.footprint].pads
               if item.number == reference.pad and item.kind is PadKind.SMD)
    return placement, pad


def _sites(board: PhysicalBoard, rule: PadViaInPadRule) -> tuple[tuple[int, int, Point], ...]:
    """Board positions, rotated with the land exactly as DRC shapes it."""
    placement, pad = _land(board, rule.pad)
    centre = transformed_local_point(placement, pad.position)
    angle = -float(placement.rotation_degrees + pad.rotation_degrees)
    return tuple((row, column, _rotate_offset(centre, dx, dy, angle))
                 for row, column, dx, dy in _offsets(pad, rule))


def via_in_pad_array_vias(board: PhysicalBoard) -> tuple[Via, ...]:
    """The vias every required array consists of; pure geometry, unchecked."""
    rules = array_rules(board)
    if not rules:
        return ()
    outer = (board.stackup.copper_layers[0], board.stackup.copper_layers[-1])
    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads}
    return tuple(
        Via(net_by_pad[rule.pad], site, ARRAY_VIA_SIZE_NM, ARRAY_VIA_DRILL_NM,
            outer[0], outer[1], finish="filled-capped")
        for rule in rules for _, _, site in _sites(board, rule)
    )


def via_in_pad_arrays_present(board: PhysicalBoard) -> bool:
    return not Counter(via_in_pad_array_vias(board)) - Counter(board.vias)


def require_via_in_pad_arrays(board: PhysicalBoard, stage: str) -> None:
    """Reject routing that would run before a declared array is fixed copper."""
    if array_rules(board) and not via_in_pad_arrays_present(board):
        raise ValueError(f"place required via-in-pad arrays before {stage}")


def materialize_via_in_pad_arrays(board: PhysicalBoard) -> PhysicalBoard:
    """Commit every required array, or reject without changing the board."""
    rules = array_rules(board)
    if not rules:
        return board
    if board.hard_macros and not board.materialized_macros:
        raise ValueError("materialize hard macros before via-in-pad arrays")
    expected = Counter(via_in_pad_array_vias(board))
    present = expected & Counter(board.vias)
    if present == expected:
        return board  # idempotent; never duplicate committed array copper
    if present:
        raise ValueError("a via-in-pad array is only partly present; rebuild it from an unrouted source")
    if board.zone_fills:
        raise ValueError("via-in-pad arrays must precede zone fill")
    clearance = RoutingClearanceIndex(board)
    layers = tuple(board.stackup.copper_layers)
    outer = (layers[0], layers[-1])
    keepouts = resolved_copper_keepouts(board)
    occupied = {via.position for via in board.vias}
    net_by_pad = {pad: net.name for net in board.nets for pad in net.pads}
    added: list[Via] = []
    for rule in rules:
        net = net_by_pad[rule.pad]
        planes = tuple(zone for zone in board.zones if zone.net == net
                       and any(layer not in outer for layer in zone.layers))
        placement, pad = _land(board, rule.pad)
        land = placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement)
        for row, column, site in _sites(board, rule):
            shape = RoundedConvexShape((site,), ARRAY_VIA_SIZE_NM // 2)
            reason = (
                "leaves the land" if not circle_inside_shape(site, ARRAY_VIA_SIZE_NM // 2, land)
                else "violates the board edge clearance"
                if not via_inside_board(board, site, ARRAY_VIA_SIZE_NM)
                else "lies outside the inner GND zone" if not any(
                    _in_outline(site, zone.outline) and not any(
                        k.block_zones and layer in k.layers
                        and _in_outline(site, k.outline) for k in keepouts)
                    for zone in planes for layer in zone.layers if layer not in outer)
                else "coincides with an existing via" if site in occupied
                else "touches another land"
                if not clearance.pad_copper_clear(shape, layers, allowed_pad=rule.pad)
                else "violates a keepout, copper or drill clearance"
                if not clearance.can_via(net, site, ARRAY_VIA_SIZE_NM, outer[0], outer[1],
                                         ARRAY_VIA_DRILL_NM, check_hole_copper=True,
                                         allowed_pad=rule.pad)
                else None
            )
            if reason is not None:
                raise ValueError(
                    f"via_in_pad array {rule.pad.component}.{rule.pad.pad}: site row {row} "
                    f"column {column} at ({_mm(site.x_nm)}, {_mm(site.y_nm)}) mm {reason}")
            via = Via(net, site, ARRAY_VIA_SIZE_NM, ARRAY_VIA_DRILL_NM, outer[0], outer[1],
                      finish="filled-capped")
            # Later sites, including this array's own, check against it.
            clearance.add_via(via, locked=True)
            occupied.add(site)
            added.append(via)
    # An absolute count: sources keep metadata when their copper is stripped.
    metadata = {**board.metadata, "fabrication_ready": "false",
                "via_in_pad_process": "filled-capped", "via_in_pad_count": str(len(added))}
    return replace(board, vias=(*board.vias, *added), metadata=metadata)


def via_in_pad_array_report(board: PhysicalBoard) -> list[dict[str, object]]:
    """Per-pad array records for the route and preflight reports."""
    present = Counter(board.vias)
    vias = iter(via_in_pad_array_vias(board))
    records: list[dict[str, object]] = []
    for rule in array_rules(board):
        _, pad = _land(board, rule.pad)
        count = rule.rows * rule.columns
        array = [next(vias) for _ in range(count)]
        records.append({
            "pad": f"{rule.pad.component}.{rule.pad.pad}",
            "rows": rule.rows,
            "columns": rule.columns,
            "count": count,
            "pitch_nm": array_pitch_nm(pad, rule) if count > 1 else None,
            "explicit_pitch": rule.pitch_nm is not None,
            "diameter_nm": ARRAY_VIA_SIZE_NM,
            "drill_nm": ARRAY_VIA_DRILL_NM,
            "finish": "filled-capped",
            "placed": all(present[via] for via in array),
            "positions_nm": [[via.position.x_nm, via.position.y_nm] for via in array],
        })
    return records


def _in_outline(point: Point, outline: PolygonWithHoles) -> bool:
    return (point_in_polygon(point, outline.outer.vertices)
            and not any(point_in_polygon(point, hole.vertices) for hole in outline.holes))


def _mm(value_nm: int) -> str:
    return str(Decimal(value_nm) / Decimal(1_000_000))

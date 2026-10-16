"""Cross-component footprint drill obstacles for incremental placement.

Through drills occupy both faces. A courtyard policy, solder mask choice, net
label or a component's own-NPTH exception never permits a foreign component to
penetrate them. This checks pads/courtyards, not completed routed copper or CAM.
"""
from dataclasses import dataclass, replace
from functools import lru_cache
from typing import Mapping

from ._cache import IdentityKey
from .geometry import RoundedConvexShape, shapes_clear
from .physical import BoardSide, PadKind, PadShape, PhysicalBoard, Placement, Point


@dataclass(frozen=True, slots=True)
class _PlacedDrill:
    shape: RoundedConvexShape
    kind: PadKind


@dataclass(frozen=True, slots=True)
class _PlacedFootprintGeometry:
    holes: tuple[_PlacedDrill, ...]
    copper: tuple[RoundedConvexShape, ...]


@lru_cache(maxsize=256)
def _has_drills(footprint_key: IdentityKey) -> bool:
    return any(pad.kind in {PadKind.THROUGH_HOLE, PadKind.NON_PLATED_THROUGH_HOLE}
               for pad in footprint_key.value.pads)


@lru_cache(maxsize=8192)
def _transformed_geometry(footprint_key: IdentityKey, position: Point, rotation, side: BoardSide):
    """Cache exact immutable footprint/pose geometry, never acceptance or rules."""
    from .drc import placed_pad_shape
    from .placement import transformed_local_point
    footprint = footprint_key.value
    pose = Placement("geometry", footprint.name, position, rotation, side)
    holes, copper = [], []
    for pad in footprint.pads:
        center = transformed_local_point(pose, pad.position)
        if pad.kind in {PadKind.THROUGH_HOLE, PadKind.NON_PLATED_THROUGH_HOLE}:
            # Identical drill-envelope construction to non_plated_holes and
            # physical DRC plated-pad drill-spacing checks; includes oval drills.
            assert pad.drill is not None
            drill_pad = replace(pad, size=pad.drill,
                                kind=pad.kind if pad.kind is PadKind.NON_PLATED_THROUGH_HOLE else PadKind.SMD,
                                drill=pad.drill if pad.kind is PadKind.NON_PLATED_THROUGH_HOLE else None,
                                shape=PadShape.CIRCLE if pad.drill.width_nm == pad.drill.height_nm
                                else PadShape.OVAL)
            holes.append(_PlacedDrill(placed_pad_shape(center, drill_pad, pose), pad.kind))
        if pad.kind not in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}:
            copper.append(placed_pad_shape(center, pad, pose))
    return _PlacedFootprintGeometry(tuple(holes), tuple(copper))


def _geometry(board: PhysicalBoard, pose: Placement) -> _PlacedFootprintGeometry:
    return _transformed_geometry(IdentityKey(board.footprints[pose.footprint]),
                                 pose.position, pose.rotation_degrees, pose.side)


def _drills_clear_courtyard(holes, board: PhysicalBoard, pose: Placement) -> bool:
    from .placement import _placement_polygon, _courtyard_triangles
    try:
        pieces = tuple(RoundedConvexShape(triangle)
                       for triangle in _courtyard_triangles(_placement_polygon(board, pose)))
    except ValueError:
        return False
    # Same positive geometric nonpenetration guard used for board mechanical
    # holes: this is not an invented component spacing or electrical distance.
    return all(shapes_clear(hole.shape, piece, 1) for hole in holes for piece in pieces)


def footprint_holes_clear(board: PhysicalBoard, candidate: Placement,
                          held: Mapping[str, Placement]) -> bool:
    """Check candidate/accepted peers symmetrically on both board faces.

    Never read peer poses from board.placements: those may be unaccepted seeds.
    A drilled component's own courtyard and PTH annular land are not foreign
    obstacles. Own-NPTH copper checks use exactly the component-scoped physical
    DRC rule. Other own-PTH land checks remain unchanged and require separate
    DRC/native coverage; routed copper and CAM gates remain separate.
    """
    candidate_key = IdentityKey(board.footprints[candidate.footprint])
    candidate_has_holes = _has_drills(candidate_key)
    candidate_geometry = None
    clearance = board.rules.minimum_hole_clearance_nm
    if candidate_has_holes:
        candidate_geometry = _geometry(board, candidate)
        own_clearance = next((rule.clearance_nm for rule in board.component_hole_clearances
                              if rule.reference == candidate.reference), clearance)
        if any(not shapes_clear(hole.shape, pad, own_clearance)
               for hole in candidate_geometry.holes if hole.kind is PadKind.NON_PLATED_THROUGH_HOLE
               for pad in candidate_geometry.copper):
            return False
    for reference, other in held.items():
        if reference != other.reference:
            return False
        if reference == candidate.reference:
            continue
        other_key = IdentityKey(board.footprints[other.footprint])
        if not candidate_has_holes and not _has_drills(other_key):
            continue
        if candidate_geometry is None:
            candidate_geometry = _geometry(board, candidate)
        other_geometry = _geometry(board, other)
        if candidate_geometry.holes and (
            not _drills_clear_courtyard(candidate_geometry.holes, board, other)
            or any(not shapes_clear(hole.shape, pad, clearance)
                   for hole in candidate_geometry.holes for pad in other_geometry.copper)
        ):
            return False
        if other_geometry.holes and (
            not _drills_clear_courtyard(other_geometry.holes, board, candidate)
            or any(not shapes_clear(hole.shape, pad, clearance)
                   for hole in other_geometry.holes for pad in candidate_geometry.copper)
        ):
            return False
    return True

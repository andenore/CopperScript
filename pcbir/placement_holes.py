"""Cross-component footprint drill obstacles for incremental placement.

Through drills occupy both faces. A courtyard policy, solder mask choice, net
label or a component's own-NPTH exception never permits a foreign component to
penetrate them. This checks pads/courtyards, not completed routed copper or CAM.
"""
from dataclasses import dataclass, replace
from functools import lru_cache
from typing import Mapping

from ._cache import IdentityKey
from .geometry import Bounds, RoundedConvexShape, shapes_clear
from .physical import BoardSide, PadKind, PadShape, PhysicalBoard, Placement, Point


@dataclass(frozen=True, slots=True)
class _PlacedDrill:
    shape: RoundedConvexShape
    kind: PadKind


@dataclass(frozen=True, slots=True)
class _PlacedFootprintGeometry:
    holes: tuple[_PlacedDrill, ...]
    copper: tuple[RoundedConvexShape, ...]
    envelope: Bounds
    lands: tuple[tuple[str, PadKind, RoundedConvexShape], ...] = ()


@lru_cache(maxsize=256)
def _has_drills(footprint_key: IdentityKey) -> bool:
    return any(pad.kind in {PadKind.THROUGH_HOLE, PadKind.NON_PLATED_THROUGH_HOLE}
               for pad in footprint_key.value.pads)


@lru_cache(maxsize=8192)
def _transformed_geometry(footprint_key: IdentityKey, position: Point, rotation, side: BoardSide):
    """Cache exact immutable footprint/pose geometry, never acceptance or rules."""
    from .drc import placed_pad_shape
    from .placement import transformed_local_point, _placed_courtyard
    footprint = footprint_key.value
    pose = Placement("geometry", footprint.name, position, rotation, side)
    holes, copper, lands = [], [], []
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
            shape = placed_pad_shape(center, pad, pose)
            copper.append(shape)
            lands.append((pad.number,pad.kind,shape))
    # Include copper, drill and courtyard extents: pads or plated slots can
    # extend beyond a package's nominal body or authored courtyard.
    boxes = [shape.bounds for shape in copper]
    boxes.extend(hole.shape.bounds for hole in holes)
    courtyard = _placed_courtyard(footprint_key, position, rotation, side)
    boxes.append(RoundedConvexShape(courtyard).bounds)
    envelope = Bounds(min(b.min_x for b in boxes), min(b.min_y for b in boxes),
                      max(b.max_x for b in boxes), max(b.max_y for b in boxes))
    return _PlacedFootprintGeometry(tuple(holes), tuple(copper), envelope,tuple(lands))


@lru_cache(maxsize=16)
def _assigned_nets(nets_key: IdentityKey):
    from .physical import PadReference
    return {pad:net.name for net in nets_key.value for pad in net.pads}


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


@lru_cache(maxsize=256)
def _array_owners(board_key: IdentityKey):
    return frozenset(rule.pad.component for rule in board_key.value.via_in_pad_rules
                     if rule.rows is not None)


@lru_cache(maxsize=2048)
def _array_vias(board_key: IdentityKey, pose: Placement):
    from .pad_via_arrays import via_in_pad_array_vias
    if pose.reference not in _array_owners(board_key):
        return ()
    return via_in_pad_array_vias(board_key.value, placements={pose.reference: pose})


@lru_cache(maxsize=256)
def _pad_nets(board_key: IdentityKey):
    return {pad: net.name for net in board_key.value.nets for pad in net.pads}


def _array_peer_clear(board, vias, peer, board_key):
    from .physical import PadReference
    geometry = _geometry(board, peer)
    footprint = board.footprints[peer.footprint]
    pads = [p for p in footprint.pads if p.kind not in {PadKind.NON_PLATED_THROUGH_HOLE, PadKind.APERTURE}]
    nets = _pad_nets(board_key)
    clearances = {rule.net: rule.clearance_nm or 0 for rule in board.net_routing_rules}
    for via in vias:
        annulus = RoundedConvexShape((via.position,), via.size_nm // 2)
        drill = RoundedConvexShape((via.position,), via.drill_nm // 2)
        for pad, copper in zip(pads, geometry.copper):
            # Array materialization forbids contact with every foreign land,
            # including same-net lands, independently of the drilled clearance.
            if not shapes_clear(annulus, copper, 1):
                return False
            net = nets.get(PadReference(peer.reference, pad.number))
            if net != via.net:
                gap = max(board.rules.minimum_clearance_nm, footprint.clearance_nm or 0,
                          clearances.get(net, 0), clearances.get(via.net, 0))
                if (not shapes_clear(annulus, copper, gap)
                        or not shapes_clear(drill, copper, max(gap, board.rules.minimum_hole_clearance_nm))):
                    return False
        if any(not shapes_clear(drill, hole.shape, board.rules.minimum_hole_clearance_nm)
               for hole in geometry.holes):
            return False
        for other in _array_vias(board_key, peer):
            other_drill = RoundedConvexShape((other.position,), other.drill_nm // 2)
            if not shapes_clear(drill, other_drill, board.rules.minimum_hole_clearance_nm):
                return False
            if other.net != via.net:
                other_copper = RoundedConvexShape((other.position,), other.size_nm // 2)
                gap = max(board.rules.minimum_clearance_nm, clearances.get(other.net, 0),
                          clearances.get(via.net, 0))
                if (not shapes_clear(annulus, other_copper, gap)
                        or not shapes_clear(drill, other_copper, max(gap, board.rules.minimum_hole_clearance_nm))
                        or not shapes_clear(annulus, other_drill, max(gap, board.rules.minimum_hole_clearance_nm))):
                    return False
    return True


def _required_arrays_clear(board, candidate, held):
    if any(ref != pose.reference for ref, pose in held.items()):
        return False
    board_key = IdentityKey(board)
    owners = _array_owners(board_key)
    if not owners:
        return True
    if candidate.reference in owners:
        vias = _array_vias(board_key, candidate)
        if any(not _array_peer_clear(board, vias, other, board_key)
               for ref, other in held.items() if ref != candidate.reference):
            return False
    return all(_array_peer_clear(board, _array_vias(board_key, pose), candidate, board_key)
               for ref, pose in held.items() if ref in owners and ref != candidate.reference)


def footprint_holes_clear(board: PhysicalBoard, candidate: Placement,
                          held: Mapping[str, Placement]) -> bool:
    """Check candidate/accepted peers symmetrically on both board faces.

    Never read peer poses from board.placements: those may be unaccepted seeds.
    A drilled component's own courtyard and PTH annular land are not foreign
    obstacles. Required filled/capped arrays reserve their annuli and drills
    against accepted peer copper and holes on both faces, at the actual held
    owner pose. Array sites do not forbid covering body-only space. This is a
    placement obstacle check, not array materialization or source admission.
    Own-NPTH copper checks use exactly the component-scoped physical
    DRC rule. Other own-PTH land checks remain unchanged and require separate
    DRC/native coverage; routed copper and CAM gates remain separate.
    """
    if not _required_arrays_clear(board, candidate, held):
        return False
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
        if not candidate_geometry.envelope.intersects_expanded(
                other_geometry.envelope, max(clearance, board.rules.minimum_clearance_nm, 1)):
            continue
        if candidate.side is not other.side:
            # A plated annulus exists on the opposite face even when its drill
            # misses the SMD land/courtyard. Reject foreign-net copper closer
            # than the board minimum now; later DRC still owns any larger
            # net/profile-specific spacing and routed-copper checks.
            from .physical import PadReference
            assigned = _assigned_nets(IdentityKey(board.nets))
            for a_number,a_kind,a_shape in candidate_geometry.lands:
                for b_number,b_kind,b_shape in other_geometry.lands:
                    if PadKind.THROUGH_HOLE not in (a_kind,b_kind):
                        continue
                    a_net = assigned.get(PadReference(candidate.reference,a_number))
                    b_net = assigned.get(PadReference(other.reference,b_number))
                    if a_net is not None and a_net==b_net:
                        continue
                    if not shapes_clear(a_shape,b_shape,board.rules.minimum_clearance_nm):
                        return False
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

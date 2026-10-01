"""Identity-bound rigid physical templates and deterministic pose transforms.

No electrical coordinates, inferred RF dimensions, connectivity changes or
reference copper are introduced here. Routing always rebuilds copper after a
placement move; a connected template alone is not manufacturing qualification.
"""

from __future__ import annotations

from dataclasses import fields, is_dataclass, replace
from decimal import Decimal
from enum import Enum
from hashlib import sha256
import json
from typing import Mapping

from .physical import (
    BoardSide, CopperKeepout, PhysicalBoard, PhysicalFootprint, Placement,
    Point, PolygonRing, PolygonWithHoles, RigidPlacementCluster,
)


def footprint_geometry_digest(footprint: PhysicalFootprint) -> str:
    """Bind the complete resolved footprint, including source ID and pad geometry."""

    def document(value):
        if is_dataclass(value):
            return {item.name: document(getattr(value, item.name)) for item in fields(value)}
        if isinstance(value, Mapping):
            return {key: document(item) for key, item in sorted(value.items())}
        if isinstance(value, (tuple, list)):
            return [document(item) for item in value]
        if isinstance(value, Enum):
            return value.value
        if isinstance(value, Decimal):
            return str(value)
        return value

    return sha256(json.dumps(document(footprint), sort_keys=True,
                             separators=(",", ":")).encode()).hexdigest()


def validate_cluster_bindings(board: PhysicalBoard) -> None:
    placements = {item.reference: item for item in board.placements}
    names: set[str] = set()
    members: set[str] = set()
    keepout_ids = {item.id for item in board.copper_keepouts}
    for cluster in board.rigid_clusters:
        if cluster.name in names:
            raise ValueError("rigid cluster names must be unique")
        names.add(cluster.name)
        for member in cluster.members:
            if member.reference in members:
                raise ValueError("overlapping rigid clusters are unsupported")
            members.add(member.reference)
            placed = placements.get(member.reference)
            if placed is None or placed.footprint != member.footprint:
                raise ValueError(f"rigid cluster {cluster.name!r} footprint binding mismatch: {member.reference}")
            if footprint_geometry_digest(board.footprints[member.footprint]) != member.footprint_digest:
                raise ValueError(f"rigid cluster {cluster.name!r} footprint digest mismatch: {member.reference}")
            if placed.side is not BoardSide.FRONT:
                raise ValueError("rigid clusters currently require front-side members; mirroring is unsupported")
        anchor = next(item for item in cluster.members if item.reference == cluster.anchor.reference)
        if anchor.rotation_degrees != 0:
            raise ValueError("rigid template anchor must have local rotation zero")
        origin = Point(0, 0)
        if cluster.anchor.pad is not None:
            pads = [pad for pad in board.footprints[anchor.footprint].pads
                    if pad.number == cluster.anchor.pad]
            if len(pads) != 1:
                raise ValueError("rigid anchor requires a unique physical pad")
            origin = pads[0].position
        if anchor.position != Point(-origin.x_nm, -origin.y_nm):
            raise ValueError("rigid template origin must coincide with the anchor pad")
        for keepout in cluster.keepouts:
            resolved_id = f"cluster/{cluster.name}/{keepout.id}"
            if resolved_id in keepout_ids:
                raise ValueError("duplicate resolved cluster keepout id")
            keepout_ids.add(resolved_id)
            if not set(keepout.layers) <= set(board.stackup.copper_layers):
                raise ValueError("rigid cluster keepout uses a layer outside the stackup")


def cluster_placements(
    board: PhysicalBoard, cluster: RigidPlacementCluster, anchor: Placement,
    placements: Mapping[str, Placement] | None = None,
) -> dict[str, Placement]:
    """Materialize the entire macro; preserve each instance's non-pose metadata."""

    from .placement import transformed_local_point

    if anchor.reference != cluster.anchor.reference or anchor.side is not BoardSide.FRONT:
        raise ValueError("cluster transform requires its front-side anchor")
    local_anchor = next(item for item in cluster.members if item.reference == anchor.reference)
    if anchor.footprint != local_anchor.footprint:
        raise ValueError("cluster anchor footprint binding mismatch")
    if anchor.rotation_degrees not in cluster.allowed_rotations:
        raise ValueError("cluster transform rotation is not allowed")
    source = placements or {item.reference: item for item in board.placements}
    origin = transformed_local_point(anchor, Point(-local_anchor.position.x_nm,
                                                   -local_anchor.position.y_nm))
    frame = replace(anchor, position=origin)
    return {
        member.reference: replace(
            source[member.reference],
            position=transformed_local_point(frame, member.position),
            rotation_degrees=(anchor.rotation_degrees + member.rotation_degrees) % Decimal(360),
            side=BoardSide.FRONT,
        )
        for member in cluster.members
    }


def cluster_placement_matches(
    board: PhysicalBoard, placements: Mapping[str, Placement],
) -> bool:
    for cluster in board.rigid_clusters:
        try:
            expected = cluster_placements(board, cluster, placements[cluster.anchor.reference], placements)
        except (ValueError, KeyError):
            return False
        if any(placements[reference] != item for reference, item in expected.items()):
            return False
    return True


def move_placement_unit(
    board: PhysicalBoard, placements: Mapping[str, Placement], reference: str,
    proposed: Placement,
) -> dict[str, Placement]:
    """Expand a member's proposed pose; callers still apply all acceptance gates."""

    from .placement import transformed_local_point

    cluster = next((item for item in board.rigid_clusters
                    if any(member.reference == reference for member in item.members)), None)
    if cluster is None:
        return {**placements, reference: proposed}
    local = next(item for item in cluster.members if item.reference == reference)
    local_anchor = next(item for item in cluster.members if item.reference == cluster.anchor.reference)
    rotation = ((proposed.rotation_degrees - local.rotation_degrees) % Decimal(360)
                + Decimal(360)) % Decimal(360)
    frame = replace(proposed, rotation_degrees=rotation)
    anchor = replace(placements[cluster.anchor.reference],
                     side=proposed.side, rotation_degrees=rotation,
                     position=transformed_local_point(frame, Point(
                         local_anchor.position.x_nm - local.position.x_nm,
                         local_anchor.position.y_nm - local.position.y_nm)))
    return {**placements, **cluster_placements(board, cluster, anchor, placements)}


def resolved_cluster_keepouts(
    board: PhysicalBoard, placements: Mapping[str, Placement],
) -> tuple[CopperKeepout, ...]:
    """Resolve local polygons/holes on explicitly declared physical layers."""

    from .placement import transformed_local_point

    result = []
    for cluster in board.rigid_clusters:
        anchor = placements.get(cluster.anchor.reference)
        if anchor is None:
            continue  # partial legalization; full acceptance checks every anchor
        local_anchor = next(item for item in cluster.members if item.reference == anchor.reference)
        frame = replace(anchor, position=transformed_local_point(
            anchor, Point(-local_anchor.position.x_nm, -local_anchor.position.y_nm)))
        for keepout in cluster.keepouts:
            def ring(local):
                return PolygonRing(tuple(transformed_local_point(frame, point) for point in local.vertices))
            result.append(replace(
                keepout, id=f"cluster/{cluster.name}/{keepout.id}",
                outline=PolygonWithHoles(ring(keepout.outline.outer),
                                        tuple(ring(hole) for hole in keepout.outline.holes)),
            ))
    return tuple(result)

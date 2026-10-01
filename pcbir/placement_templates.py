"""Explicit, data-only scene bindings for source-backed physical macros.

Files are content-addressed; electrical pad/net bindings and resolved footprint
digests are checked before any placement mutation. No Python template execution,
network fetching, implicit discovery, rewiring or qualification is performed.
"""
from dataclasses import replace
from decimal import Decimal, InvalidOperation
from hashlib import sha256
import json
from pathlib import Path

from .clusters import footprint_geometry_digest
from .physical import (
    ComponentPlacementRule, PhysicalBoard, PlacementTarget, Point,
    RigidPlacementCluster, RigidPlacementMember,
)


def apply_placement_templates(board: PhysicalBoard, scene_path: Path) -> PhysicalBoard:
    if board.tracks or board.vias or board.zone_fills:
        raise ValueError("placement templates require an unrouted, unfilled board")
    try:
        raw = Path(scene_path).read_bytes()
        scene = json.loads(raw)
        if set(scene) != {"schema", "clusters"} or scene["schema"] != "copperscript-placement-templates/v0.1":
            raise ValueError("unsupported placement-template scene schema or fields")
        if not isinstance(scene["clusters"], list) or not scene["clusters"]:
            raise ValueError("placement-template scene requires clusters")
        placements = {item.reference: item for item in board.placements}
        assigned = {(pad.component, pad.pad): net.name for net in board.nets for pad in net.pads}
        rules = {item.reference: item for item in board.placement_rules}
        clusters = list(board.rigid_clusters)
        for binding in scene["clusters"]:
            if set(binding) != {"name", "reference", "reference_sha256", "bindings", "net_bindings",
                                "footprint_digests", "allowed_rotations", "internal_clearance_nm"}:
                raise ValueError("unsupported placement-template binding fields")
            reference_raw = (Path(scene_path).resolve().parent / binding["reference"]).read_bytes()
            if sha256(reference_raw).hexdigest() != binding["reference_sha256"]:
                raise ValueError("placement reference identity changed")
            reference = json.loads(reference_raw)
            if reference["schema"] != "copperlib-rigid-reference/v0.1" or reference["production_publishable"] is not False:
                raise ValueError("unsupported placement reference; qualification is not inferred")
            members = reference["members"]
            refs = {item["reference"] for item in members}
            if set(binding["bindings"]) != refs or set(binding["footprint_digests"]) != refs:
                raise ValueError("template bindings must exactly cover the reference members")
            if len(set(binding["bindings"].values())) != len(refs):
                raise ValueError("template members must bind distinct instances")
            for member in members:
                pose = placements[binding["bindings"][member["reference"]]]
                footprint = board.footprints[pose.footprint]
                if footprint_geometry_digest(footprint) != binding["footprint_digests"][member["reference"]]:
                    raise ValueError(f"template footprint identity changed: {pose.reference}")
                if len(member["center_nm"]) != 2 or any(type(value) is not int for value in member["center_nm"]):
                    raise ValueError("template centers require integer nanometres")
            logical_nets = {row[2] for row in reference["pad_nets"]}
            if set(binding["net_bindings"]) != logical_nets:
                raise ValueError("template net bindings must exactly cover electrical roles")
            for source_ref, pad, role in reference["pad_nets"]:
                ref = binding["bindings"][source_ref]
                if assigned.get((ref, pad)) != binding["net_bindings"][role]:
                    raise ValueError(f"template pad/net binding mismatch: {ref}.{pad} ({role})")
            anchor_ref = reference["anchor"]["reference"]
            anchor_pose = placements[binding["bindings"][anchor_ref]]
            anchor_member = next(item for item in members if item["reference"] == anchor_ref)
            if Decimal(anchor_member["rotation_degrees"]) != 0:
                raise ValueError("reference anchor rotation must be zero")
            anchor_pads = [pad for pad in board.footprints[anchor_pose.footprint].pads
                           if pad.number == reference["anchor"]["pad"]]
            if len(anchor_pads) != 1:
                raise ValueError("template anchor pad must be unique")
            origin_x = anchor_member["center_nm"][0] + anchor_pads[0].position.x_nm
            origin_y = anchor_member["center_nm"][1] + anchor_pads[0].position.y_nm
            allowed = tuple(binding["allowed_rotations"])
            local_members = []
            for member in members:
                ref = binding["bindings"][member["reference"]]
                pose = placements[ref]
                local_rotation = Decimal(member["rotation_degrees"])
                local_members.append(RigidPlacementMember(
                    ref, pose.footprint, binding["footprint_digests"][member["reference"]],
                    Point(member["center_nm"][0] - origin_x, member["center_nm"][1] - origin_y),
                    local_rotation,
                ))
                orientations = tuple((Decimal(str(angle)) + local_rotation) % 360 for angle in allowed)
                existing = rules.get(ref)
                # Existing explicit orientation restrictions are never broadened.
                rules[ref] = (ComponentPlacementRule(ref, allowed_orientations=orientations)
                              if existing is None else existing)
            clearance = binding["internal_clearance_nm"]
            if type(clearance) is not int or clearance < 0:
                raise ValueError("template internal courtyard clearance requires nonnegative integer nm")
            clusters.append(RigidPlacementCluster(
                binding["name"], PlacementTarget(anchor_pose.reference, reference["anchor"]["pad"]),
                tuple(local_members),
                f'{reference["source"]["url"]}#{reference["source"]["entry"]};sha256={binding["reference_sha256"]}',
                allowed_rotations=allowed, internal_clearance_nm=clearance,
            ))
        metadata = {**board.metadata, "placement_template_scene_sha256": sha256(raw).hexdigest(),
                    "placement_template_status": "provisional-footprint-adaptation", "fabrication_ready": "false"}
        return replace(board, rigid_clusters=tuple(clusters), placement_rules=tuple(rules.values()), metadata=metadata)
    except (KeyError, TypeError, IndexError, StopIteration, OSError, InvalidOperation, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid placement template {scene_path}: {exc}") from exc

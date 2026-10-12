"""Artifact-bound verification of individual semantic declarations.

Routing closure, declaration coverage and production qualification are separate.
Unsupported verifiers and parameters cannot acquire authority from source labels.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import replace
from hashlib import sha256
from math import isqrt

from .constraint_coverage import ConstraintMode
from .drc import physical_board_digest
from .elaborate import elaborate
from .model import Board, ConstraintKind
from .serializer import board_to_json


_ROUTE_PARAMETERS = {
    "kind", "priority", "width", "clearance", "allowed_layers", "max_vias",
    "max_length", "partner", "max_skew", "breakout_length", "breakout_width",
    "breakout_gap", "breakout_clearance",
}
_PLACEMENT_PARAMETERS = {
    "max_distance": {"distance", "weight", "priority"},
    "min_distance": {"distance", "weight", "priority"},
    "align": {"axis", "tolerance", "weight"},
    "fixed_placement": {"x", "y", "rotation", "side", "edge_clearance", "priority"},
    "allowed_orientations": {"values"},
    "placement_region": {"name", "x", "y", "width", "height", "side"},
    "keepout": {"name", "x", "y", "width", "height", "side", "maximum_height"},
}


def _lower(flat, constraint, board):
    # Preserve explicitly referenced region declarations for zone lowering.
    from .physicalize import _lower_physical_constraints
    prerequisites = tuple(c for c in flat.constraints
                          if c.kind is ConstraintKind.PLACEMENT_REGION
                          and constraint.kind is ConstraintKind.COPPER_ZONE)
    selected = replace(flat, constraints=(*prerequisites, constraint))
    metadata = {}
    lowered = _lower_physical_constraints(selected,
        {c.ref: c for c in flat.components}, metadata, board.outline)
    if metadata.get("omitted_constraint_targets"):
        raise ValueError("constraint target did not resolve to a physical component")
    return selected, lowered


def _placement(constraint, lowered, board):
    from .geometry import RoundedConvexShape
    from .mechanical import shape_in_outline, shape_in_board
    from .placement import _placement_polygon, _keepout_blocks, transformed_pad_position
    from .mechanical_assembly import body_in_material
    regions, keepouts, rules, relative = lowered[:4]
    poses = {p.reference: p for p in board.placements}
    measurements, violations = [], []
    for rule in relative:
        points = []
        for target in rule.targets:
            pose = poses[target.reference]
            points.append(transformed_pad_position(board, pose, target.pad)
                          if target.pad else pose.position)
        if rule.kind.value in {"max_distance", "min_distance"}:
            for a, b in zip(points, points[1:]):
                squared = (a.x_nm-b.x_nm)**2 + (a.y_nm-b.y_nm)**2
                limit = rule.distance_nm
                valid = squared <= limit**2 if rule.kind.value == "max_distance" else squared >= limit**2
                measurements.append({"property": rule.kind.value, "actual_nm": isqrt(squared),
                    "limit_nm": limit, "comparison": "exact squared distance", "passed": valid})
                if not valid:
                    violations.append("declared pad/component distance is violated")
        else:
            values = [p.x_nm if rule.axis.value == "x" else p.y_nm for p in points]
            span = max(values)-min(values)
            measurements.append({"property": "alignment", "actual_nm": span,
                "limit_nm": rule.tolerance_nm, "passed": span <= rule.tolerance_nm})
            if span > rule.tolerance_nm:
                violations.append("declared alignment tolerance is violated")
    for rule in rules:
        pose = poses[rule.reference]
        if constraint.kind is ConstraintKind.FIXED_PLACEMENT:
            if rule.fixed_position is not None and pose.position != rule.fixed_position:
                violations.append(f"{rule.reference}: fixed position differs")
            if rule.fixed_rotation_degrees is not None and pose.rotation_degrees != rule.fixed_rotation_degrees:
                violations.append(f"{rule.reference}: fixed rotation differs")
            if rule.side is not None and pose.side is not rule.side:
                violations.append(f"{rule.reference}: fixed side differs")
            # Use the legalizer's own predicate. A declared body overhang is
            # part of the placement rule, so re-checking the pose against the
            # bare outline would fail a placement the legalizer accepted.
            # body_in_material falls back to shape_in_board when the reference
            # declares no overhang, so unaffected components are unchanged.
            if rule.edge_clearance_nm is not None and not body_in_material(board, pose,
                    RoundedConvexShape(_placement_polygon(board, pose)), rule.edge_clearance_nm):
                violations.append(f"{rule.reference}: declared edge clearance is violated")
        elif constraint.kind is ConstraintKind.ALLOWED_ORIENTATIONS:
            if pose.rotation_degrees not in rule.allowed_orientations:
                violations.append(f"{rule.reference}: rotation is outside allowed orientations")
        elif constraint.kind is ConstraintKind.PLACEMENT_REGION:
            region = next(r for r in regions if r.name == rule.region)
            if (region.side is not None and pose.side is not region.side) or not shape_in_outline(
                    RoundedConvexShape(_placement_polygon(board, pose)), region.outline):
                violations.append(f"{rule.reference}: footprint leaves declared placement region/side")
    for keepout in keepouts:
        for pose in board.placements:
            if _keepout_blocks(board, keepout, pose, _placement_polygon(board, pose)):
                violations.append(f"{pose.reference}: footprint enters declared placement keepout")
    return ("fail" if violations else "pass", "; ".join(violations) or
            "checked against actual final placement geometry", measurements)


def _routing(constraint, lowered, board, drc):
    from .drc import (_check_track_rules, _check_vias, _check_differential_rules,
                      explicit_copper_connectivity)
    expected = lowered[5][0]
    current = next((r for r in board.net_routing_rules if r.net == expected.net), None)
    if current is None:
        return "blocked", "routing intent is absent from the final physical board", []
    view = replace(board, net_routing_rules=tuple(r for r in board.net_routing_rules
                                                 if r.net != expected.net) + (expected,))
    findings = []
    _check_track_rules(view, findings)
    _check_vias(view, findings)
    _check_differential_rules(view, findings)
    related = {expected.net, expected.differential_partner} - {None}
    errors = [f for f in findings if f.severity.value == "error" and set(f.nets) & related]
    measurements = [{"property": f.code, "actual_nm": f.measured_nm,
                     "limit_nm": f.required_nm} for f in errors]
    if errors:
        return "fail", "; ".join(f.message for f in errors), measurements
    nets = [n for n in board.nets if n.name in related]
    graph = explicit_copper_connectivity(board, only_nets=frozenset(related))
    if len(nets) != len(related) or not all(graph.net_connected(n) for n in nets):
        return "blocked", "routing limits cannot pass while an affected net lacks explicit connectivity", measurements
    if "clearance" in constraint.parameters or "breakout_clearance" in constraint.parameters:
        # Reuse exact bound DRC only when its effective limits are at least as
        # strict as this original declaration. Never trust last-rule-wins alone.
        required = expected.clearance_nm or board.rules.minimum_clearance_nm
        actual = current.clearance_nm or board.rules.minimum_clearance_nm
        if actual < required or ("breakout_clearance" in constraint.parameters
                and current.breakout_clearance_nm != expected.breakout_clearance_nm):
            return "blocked", "final clearance rule differs; a scoped exact-clearance verification is required", measurements
        if not drc or not any(c.check == "shorts_and_clearance" and c.status.value == "executed"
                              for c in drc.coverage):
            return "blocked", "no current exact-clearance DRC coverage", measurements
        errors = [f for f in drc.findings if f.severity.value == "error"
                  and set(f.nets) & related and f.code in {"DRC-CLEARANCE", "DRC-SHORT", "DRC-ZONE-CLEARANCE"}]
        if errors:
            return "fail", "; ".join(f.message for f in errors), measurements
    unknown = sorted(set(constraint.parameters) - _ROUTE_PARAMETERS)
    if unknown:
        return "blocked", "no complete final-geometry/electrical verifier for parameters: " + ", ".join(unknown), measurements
    if "breakout_gap" in constraint.parameters:
        return "blocked", "pair-gap enforcement needs a complete final pair-geometry verifier", measurements
    if "max_skew" in constraint.parameters and not expected.differential_partner:
        return "blocked", "pair-skew limit has no declared differential partner", measurements
    return "pass", "original declaration's implemented geometric limits checked; no electrical performance claim", measurements


def _evaluate(flat, constraint, board, drc, native, stage):
    if constraint.mode is ConstraintMode.ASSUME:
        return "assumed", "source assumption; not verified evidence", []
    if constraint.mode is ConstraintMode.EXTERNAL:
        return "blocked", "external declaration needs an applicable reviewed evidence adapter", []
    if constraint.kind in {ConstraintKind.NOTE, ConstraintKind.PLACEMENT_GROUP}:
        return "advisory", "documentary/organizational intent has no numerical acceptance criterion", []
    if constraint.consumers or constraint.verifier:
        return "blocked", "custom consumer/verifier labels have no registered verification authority", []
    _, lowered = _lower(flat, constraint, board)
    if constraint.kind.value in _PLACEMENT_PARAMETERS:
        unknown = sorted(set(constraint.parameters) - _PLACEMENT_PARAMETERS[constraint.kind.value])
        if unknown:
            return "blocked", "unsupported placement parameters: " + ", ".join(unknown), []
        return _placement(constraint, lowered, board)
    if stage != "route":
        return "blocked", "requires final routing-stage evidence; placement alone is insufficient", []
    if constraint.kind is ConstraintKind.ROUTING:
        return _routing(constraint, lowered, board, drc)
    if constraint.kind is ConstraintKind.LENGTH_MATCH:
        from .signal_integrity import verify_match_groups
        from .physicalize import _lower_match_groups
        selected = replace(flat, constraints=(constraint,))
        expected = _lower_match_groups(selected)
        results = verify_match_groups(replace(board, match_groups=expected))
        if not results:
            return "blocked", "no match-group measurement", []
        r = results[0]
        return (r.status if r.status in {"pass", "fail"} else "blocked",
                "explicit connected track-length skew; no delay/impedance qualification",
                [{"property": "skew", "actual_nm": r.skew_nm, "limit_nm": r.max_skew_nm}])
    if constraint.kind is ConstraintKind.COPPER_ZONE:
        expected = lowered[6][0]
        if expected not in board.zones:
            return "fail", "declared zone geometry/settings absent or changed", []
        if native is None or not native.matches(board):
            return "blocked", "current artifact-bound native refill evidence is missing", []
        if not native.passed:
            return "fail", "native filled-board connectivity/DRC did not pass", []
        return "pass", "preserved zone declaration and current native filled-board checks; no impedance proof", []
    if constraint.kind is ConstraintKind.VIA_IN_PAD:
        from .pad_via_arrays import via_in_pad_arrays_present
        expected = lowered[7][0]
        if expected not in board.via_in_pad_rules:
            return "fail", "via-in-pad permission/array differs from source", []
        if expected.rows is not None:
            if not via_in_pad_arrays_present(replace(board, via_in_pad_rules=(expected,))):
                return "fail", "required via-array geometry or multiplicity is absent", []
            return "blocked", "required array is present; fabrication process confirmation remains external", []
        return "pass", "geometric permission preserved; no claim that an ordered via process is qualified", []
    if constraint.kind is ConstraintKind.HOLE_CLEARANCE:
        from .physicalize import _lower_hole_clearances
        expected = _lower_hole_clearances(replace(flat, constraints=(constraint,)),
            board.footprints, list(board.placements), board.rules, board.metadata)
        if not expected or not all(r in board.component_hole_clearances for r in expected):
            return "blocked", "scoped hole-clearance rule absent or unresolved", []
        if drc is None or not any(c.check == "component_hole_clearance" and c.status.value == "executed"
                                  for c in drc.coverage):
            return "blocked", "current scoped hole-clearance DRC coverage is missing", []
        errors = [f for f in drc.findings if f.code == "DRC-HOLE-CLEARANCE" and f.severity.value == "error"]
        return ("fail" if errors else "pass", "current exact hole-clearance DRC (board-wide conservative scope)", [])
    return "blocked", "no supported verifier for this declaration", []


def verify_requirements(source, physical, *, drc=None, native=None, stage="route"):
    if stage not in {"placement", "route"}:
        raise ValueError("requirement stage must be placement or route")
    flat = elaborate(source) if isinstance(source, Board) else source
    digest = physical_board_digest(physical)
    stale_drc = drc is not None and drc.token.board_digest != digest
    identifiers = [c.constraint_id or f"{c.kind.value}:{i}" for i, c in enumerate(flat.constraints)]
    duplicates = {key for key, count in Counter(identifiers).items() if count > 1}
    rows = []
    for index, original in enumerate(flat.constraints):
        identifier = identifiers[index]
        # IDs generated by physical lowering include the original declaration index.
        physical_id = (f"zone:{original.targets[0]}:{index}" if original.kind is ConstraintKind.COPPER_ZONE
                       else identifier)
        constraint = replace(original, constraint_id=original.constraint_id or physical_id)
        try:
            if identifier in duplicates:
                status, detail, measurements = "blocked", "duplicate declaration ID", []
            elif stale_drc:
                status, detail, measurements = "blocked", "DRC evidence belongs to different physical geometry", []
            else:
                status, detail, measurements = _evaluate(flat, constraint, physical, drc, native, stage)
        except (ValueError, KeyError, StopIteration) as exc:
            status, detail, measurements = "blocked", f"unresolved physical verification: {exc}", []
        required = original.mode is ConstraintMode.EXTERNAL or (
            original.mode is ConstraintMode.REQUIRE and original.kind not in {
                ConstraintKind.NOTE, ConstraintKind.PLACEMENT_GROUP})
        rows.append({"id": identifier, "kind": original.kind.value, "targets": list(original.targets),
            "mode": original.mode.value, "origins": list(original.origins),
            "parameters": {k: str(v) for k, v in sorted(original.parameters.items())},
            "required": required, "status": status, "detail": detail, "measurements": measurements,
            "verifier": "copperscript-final-geometry/v0.1",
            "declared_consumers": list(original.consumers), "declared_verifier": original.verifier})
    hard = [r for r in rows if r["required"]]
    overall = "fail" if any(r["status"] == "fail" for r in hard) else (
        "blocked" if any(r["status"] != "pass" for r in hard) else "pass")
    native_current = native is not None and native.matches(physical)
    return {"schema": "copperscript-requirement-verification/v0.1", "stage": stage,
        "status": overall, "complete": overall == "pass", "board_digest": digest,
        "source_digest": sha256(board_to_json(source).encode("utf-8")).hexdigest(),
        "native_report_digest": native.report_digest if native_current else None,
        "counts": dict(sorted(Counter(r["status"] for r in rows).items())), "requirements": rows,
        "scope": "explicit semantic declarations; ERC, mechanical, package, CAM and production gates remain separate"}

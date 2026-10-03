"""Normalized constraint ownership and fail-closed coverage reporting."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class ConstraintMode(str, Enum):
    REQUIRE = "require"
    TARGET = "target"
    PREFER = "prefer"
    ASSUME = "assume"
    EXTERNAL = "external"


class ConstraintCheckStatus(str, Enum):
    PASS = "pass"
    WARN = "warn"
    FAIL = "fail"
    BLOCKED = "blocked"
    WAIVED = "waived"


@dataclass(frozen=True, slots=True)
class NormalizedConstraint:
    id: str
    subject: str
    property: str
    mode: ConstraintMode
    domain: str
    origins: tuple[str, ...]
    consumers: tuple[str, ...] = ()
    verifier: str | None = None


@dataclass(frozen=True, slots=True)
class ConstraintCoverage:
    constraint_id: str
    status: ConstraintCheckStatus
    consumers: tuple[str, ...]
    verifier: str | None
    detail: str = ""


def constraint_coverage(constraints: tuple[NormalizedConstraint, ...],
                        results: dict[str, ConstraintCheckStatus]) -> tuple[ConstraintCoverage, ...]:
    coverage: list[ConstraintCoverage] = []
    for constraint in sorted(constraints, key=lambda item: item.id):
        hard = constraint.mode in {ConstraintMode.REQUIRE, ConstraintMode.EXTERNAL}
        if hard and (not constraint.consumers or constraint.verifier is None):
            status, detail = ConstraintCheckStatus.BLOCKED, "hard constraint has no complete consumer/verifier chain"
        elif constraint.id not in results:
            status = ConstraintCheckStatus.BLOCKED if hard else ConstraintCheckStatus.WARN
            detail = "no verification result"
        else:
            status, detail = results[constraint.id], ""
        coverage.append(ConstraintCoverage(constraint.id, status, constraint.consumers,
                                           constraint.verifier, detail))
    return tuple(coverage)


def normalize_constraints(constraints: tuple[object, ...]) -> tuple[NormalizedConstraint, ...]:
    """Convert semantic IR constraints into the common ownership contract."""

    normalized: list[NormalizedConstraint] = []
    for index, constraint in enumerate(constraints):
        kind = constraint.kind.value
        routing = kind == "routing"
        zone = kind == "copper_zone"
        via_in_pad = kind == "via_in_pad"
        default_consumers = (
            ("physicalizer", "plane_stitch", "physical_drc") if via_in_pad
            else ("physicalizer", "kicad_zone_refill", "physical_drc") if zone
            else ("critical_router", "physical_drc") if routing
            else ("placement", "physical_drc")
        )
        normalized.append(
            NormalizedConstraint(
                constraint.constraint_id or f"{kind}:{index}",
                ",".join(constraint.targets),
                f"copper.{kind}" if zone or via_in_pad else f"route.{kind}" if routing else f"placement.{kind}",
                constraint.mode,
                "physical" if zone or via_in_pad else "routing" if routing else "placement",
                constraint.origins,
                constraint.consumers or default_consumers,
                constraint.verifier or (
                    "DRC-VIA-PAD-OVERLAP" if via_in_pad else "KICAD-ZONE-FILL" if zone else "DRC-ROUTING" if routing else "DRC-PLACEMENT"
                ),
            )
        )
    return tuple(normalized)

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

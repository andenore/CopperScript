"""Reuse unused joint-search states without adding portals or relaxing gates."""
from __future__ import annotations

from dataclasses import dataclass, fields, is_dataclass, replace
from decimal import Decimal
from enum import Enum
from fractions import Fraction
from hashlib import sha256
import json
from typing import Iterator, Mapping, TYPE_CHECKING

from .physical import CopperLayer, PhysicalBoard, Via

if TYPE_CHECKING:
    from .pair_search import PairSearchCandidate, PairSearchStats, _Port, _SpineSearchResult


def _board_fingerprint(board: PhysicalBoard) -> str:
    def freeze(value):
        if isinstance(value, Enum):
            return value.value
        if isinstance(value, (Decimal, Fraction)):
            return str(value)
        if is_dataclass(value):
            return {f.name: freeze(getattr(value, f.name)) for f in fields(value)}
        if isinstance(value, Mapping):
            return {str(k): freeze(v) for k, v in value.items()}
        if isinstance(value, (tuple, list)):
            return [freeze(v) for v in value]
        if isinstance(value, (set, frozenset)):
            return sorted((freeze(v) for v in value), key=lambda v: json.dumps(v, sort_keys=True))
        return value
    return sha256(json.dumps(freeze(board), sort_keys=True, separators=(",", ":")).encode()).hexdigest()


@dataclass(frozen=True, slots=True)
class _Ownership:
    source_fingerprint: str
    escaped_fingerprint: str
    via_pairs: tuple[tuple[Via, Via], ...] = ()
    return_vias: tuple[Via, ...] = ()


@dataclass(slots=True)
class _Continuation:
    board: PhysicalBoard
    first_name: str
    second_name: str
    start: _Port
    end: _Port
    layer: CopperLayer
    pitch: int
    search_index: int
    steps: Iterator[_SpineSearchResult | None] | None = None
    ownership: _Ownership | None = None
    active: bool = True
    resumed: bool = False
    expanded_states: int = 0
    legal_advances: int = 0
    minimum_goal_distance_nm: float = float("inf")


def _advance(continuation: _Continuation, budget: int) -> _SpineSearchResult | None:
    assert continuation.steps is not None
    for _ in range(budget):
        try:
            candidate = next(continuation.steps)
        except StopIteration:
            continuation.active = False
            return None
        if candidate is not None:
            continuation.steps.close()
            return candidate
    return None


def register_pair_search(stats: PairSearchStats, board: PhysicalBoard, *,
                         via_pairs: tuple[tuple[Via, Via], ...] = (),
                         return_vias: tuple[Via, ...] = (), retain_frontier: bool = True) -> None:
    """Freeze the original proposal ownership immediately after a portal trial."""
    if not stats._continuations or stats._continuations[-1].ownership is not None:
        return
    continuation = stats._continuations[-1]
    if not retain_frontier:
        continuation.steps.close()
        stats._continuations.pop()
        return
    continuation.ownership = _Ownership(_board_fingerprint(board),
        _board_fingerprint(continuation.board), tuple(tuple(p) for p in via_pairs), tuple(return_vias))


def resume_pair_candidates(stats: PairSearchStats, *, board: PhysicalBoard,
                           maximum_total_states: int) -> Iterator[PairSearchCandidate]:
    """Continue measured viable frontiers under the original aggregate cap.

    No new portal or search slot is created. The caller must retain the exact
    source/support board and run every ordinary candidate acceptance gate.
    Closest observed approach ranks eligible frontiers, with stable trial order
    breaking ties. Empty or blocked frontiers cannot consume leftover states.
    """
    if maximum_total_states <= 0:
        raise ValueError("joint pair aggregate state bound must be positive")
    from .pair_search import PairSearchCandidate
    fingerprint = _board_fingerprint(board)
    eligible = sorted((c for c in stats._continuations
        if c.active and c.legal_advances and c.ownership is not None
        and c.ownership.source_fingerprint == fingerprint),
        key=lambda c: c.minimum_goal_distance_nm)
    for c in eligible:
        remaining = maximum_total_states - stats.expanded_states
        if remaining <= 0:
            break
        if c.ownership.escaped_fingerprint != _board_fingerprint(c.board):
            continue
        if not c.resumed:
            stats.resumed_portals += 1
            c.resumed = True
        before = stats.expanded_states
        candidate = _advance(c, remaining)
        stats.resumed_states += stats.expanded_states-before
        if candidate is None:
            continue
        first, second, expanded, spine = candidate
        stats.candidates += 1
        yield PairSearchCandidate(
            (*c.start.first, *first, *(replace(t, start=t.end, end=t.start) for t in reversed(c.end.first))),
            (*c.start.second, *second, *(replace(t, start=t.end, end=t.start) for t in reversed(c.end.second))),
            expanded, c.search_index, spine, c.start, c.end,
            c.ownership.via_pairs, c.ownership.return_vias)

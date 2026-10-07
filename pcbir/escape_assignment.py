"""Bounded local CSP for provisional ordinary package escapes, not area routing.

Domains are legal against immutable input. Lazy binary conflicts use the same
track/via/drill predicates as routing; only newly selected alternatives may move.
Every accepted local solution preserves all prior escaped identities.
"""
from collections import Counter
from dataclasses import dataclass
from typing import Callable, Mapping

from .physical import PadReference, PhysicalBoard, PhysicalNet, TrackSegment, Via
from .routing_clearance import RoutingClearanceIndex


EscapeCandidate = tuple[tuple[TrackSegment, ...], Via | None]


@dataclass(frozen=True, slots=True)
class EscapeAssignmentOptions:
    # None tries each missing root once, within the shared pair/query budgets.
    maximum_trials: int | None = None
    maximum_cluster_pins: int = 12
    maximum_search_states: int = 20_000
    maximum_pair_checks: int = 200_000
    maximum_pair_queries: int = 2_000_000

    def __post_init__(self):
        if self.maximum_trials is not None and (
                not isinstance(self.maximum_trials, int) or self.maximum_trials <= 0):
            raise ValueError("escape assignment trial budget must be a positive integer or None")
        if min(self.maximum_cluster_pins,
               self.maximum_search_states, self.maximum_pair_checks, self.maximum_pair_queries) <= 0:
            raise ValueError("escape assignment budgets must be positive")


@dataclass(frozen=True, slots=True)
class EscapeAssignmentTrial:
    pad: PadReference
    cluster: tuple[PadReference, ...]
    search_states: int
    solution_found: bool
    diagnostic: str


@dataclass(frozen=True, slots=True)
class EscapeAssignmentReport:
    trials: tuple[EscapeAssignmentTrial, ...] = ()
    pair_checks: int = 0
    expanded_pads: tuple[PadReference, ...] = ()
    native_accepted: bool = True
    pair_queries: int = 0
    broad_phase_accepts: int = 0


class _BudgetExceeded(Exception):
    pass


class EscapeConflicts:
    """A lazy exact candidate-conflict graph scoped to one immutable board."""

    def __init__(self, board: PhysicalBoard, maximum_checks: int, maximum_queries: int = 2_000_000):
        # Input pads/keepouts were checked by domain generation. This tiny
        # board checks only additional candidate-candidate copper and drills.
        # Original rule/stackup values are retained, not approximated.
        self.board = PhysicalBoard("escape-conflicts", board.outline, {}, (),
            tuple(PhysicalNet(n.name, ()) for n in board.nets),
            stackup=board.stackup, rules=board.rules, net_routing_rules=board.net_routing_rules)
        self.maximum_checks = maximum_checks
        self.checks = 0
        self.maximum_queries = maximum_queries
        self.queries = 0
        self.broad_phase_accepts = 0
        self._identities: dict[EscapeCandidate, int] = {}
        self._indices: dict[int, RoutingClearanceIndex] = {}
        self._bounds: dict[int, tuple[int, int, int, int]] = {}
        self._cache: dict[tuple[int, int], bool] = {}
        self.margin = max(board.rules.minimum_clearance_nm,
                          board.rules.minimum_hole_clearance_nm,
                          *(r.clearance_nm or 0 for r in board.net_routing_rules))

    def _identity(self, candidate: EscapeCandidate) -> int:
        if candidate not in self._identities:
            identity = len(self._identities)
            self._identities[candidate] = identity
            tracks, via = candidate
            points = [p for t in tracks for p in (t.start, t.end)]
            radius = max(t.width_nm // 2 for t in tracks)
            if via is not None:
                points.append(via.position)
                radius = max(radius, via.size_nm // 2, via.drill_nm // 2)
            margin = radius + self.margin
            self._bounds[identity] = (min(p.x_nm for p in points)-margin,
                min(p.y_nm for p in points)-margin, max(p.x_nm for p in points)+margin,
                max(p.y_nm for p in points)+margin)
        return self._identities[candidate]

    def compatible(self, first: EscapeCandidate, second: EscapeCandidate) -> bool:
        if self.queries >= self.maximum_queries:
            raise _BudgetExceeded("candidate compatibility query budget exhausted")
        self.queries += 1
        a, b = self._identity(first), self._identity(second)
        key = min(a, b), max(a, b)
        if key in self._cache:
            return self._cache[key]
        aa, bb = self._bounds[a], self._bounds[b]
        if aa[2] < bb[0] or bb[2] < aa[0] or aa[3] < bb[1] or bb[3] < aa[1]:
            # Conservative separated bounds prove compatibility without any
            # exact geometry query. Do not charge its scarce budget or fill
            # the exact-pair cache with distant packages. Total queries retain
            # an independent bound, including cached/broad-phase requests.
            self.broad_phase_accepts += 1
            return True
        if self.checks >= self.maximum_checks:
            raise _BudgetExceeded("candidate compatibility budget exhausted")
        self.checks += 1
        if a not in self._indices:
            index = RoutingClearanceIndex(self.board)
            for track in first[0]:
                index.add_track(track, locked=True)
            if first[1] is not None:
                index.add_via(first[1], locked=True)
            self._indices[a] = index
        index = self._indices[a]
        tracks, via = second
        answer = (all(index.can_track(t.net, t.start, t.end, t.width_nm, t.layer) for t in tracks)
                  and (via is None or index.can_via(via.net, via.position, via.size_nm,
                       via.from_layer, via.to_layer, drill_nm=via.drill_nm)))
        self._cache[key] = answer
        return answer


def improve_escape_assignment(
    board: PhysicalBoard, ordered: tuple[PadReference, ...],
    domains: dict[PadReference, tuple[EscapeCandidate, ...]],
    incumbent: Mapping[PadReference, int],
    expand: Callable[[PadReference], tuple[EscapeCandidate, ...]],
    options: EscapeAssignmentOptions,
) -> tuple[dict[PadReference, int], EscapeAssignmentReport]:
    """Try missing pins with current blockers; outside selections stay fixed.

    Expansion is once per affected pin. An unsolved/over-budget cluster changes
    no selection. Search uses minimum remaining values and forward checking;
    incumbent alternatives are tried before appended alternatives when legal.
    """
    selected = dict(incumbent)
    conflicts = EscapeConflicts(board, options.maximum_pair_checks, options.maximum_pair_queries)
    expanded: set[PadReference] = set()
    trials = []
    rank = {pad: index for index, pad in enumerate(ordered)}
    for root in ordered:
        if root in selected or not domains[root]:
            continue
        if options.maximum_trials is not None and len(trials) >= options.maximum_trials:
            break
        cluster = {root}
        states = 0
        try:
            # Include the selected owners that actually block root choices,
            # not every pin in a package or any immutable input-copper owner.
            for pad, choice in selected.items():
                if any(not conflicts.compatible(candidate, domains[pad][choice])
                       for candidate in domains[root]):
                    cluster.add(pad)
            if len(cluster) > options.maximum_cluster_pins:
                trials.append(EscapeAssignmentTrial(root, tuple(sorted(cluster)), 0, False,
                                                    "local cluster pin budget exceeded"))
                continue
            def search(remaining, assigned):
                nonlocal states
                if not remaining:
                    return assigned
                pad = min(remaining, key=lambda p: (len(remaining[p]), rank[p]))
                choices = sorted(remaining[pad], key=lambda i: (i != selected.get(pad), i))
                for choice in choices:
                    if states >= options.maximum_search_states:
                        raise _BudgetExceeded("local assignment search budget exhausted")
                    states += 1
                    filtered = {other: tuple(i for i in values if conflicts.compatible(
                        domains[pad][choice], domains[other][i]))
                        for other, values in remaining.items() if other != pad}
                    if any(not values for values in filtered.values()):
                        continue
                    answer = search(filtered, {**assigned, pad: choice})
                    if answer is not None:
                        return answer
                return None

            while True:
                for pad in sorted(cluster):
                    if pad not in expanded:
                        additions = expand(pad)
                        domains[pad] = tuple(dict.fromkeys((*domains[pad], *additions)))
                        expanded.add(pad)
                outside = {p: domains[p][choice] for p, choice in selected.items() if p not in cluster}
                pressure: Counter[PadReference] = Counter()
                available = {}
                for pad in sorted(cluster):
                    values = []
                    for i, candidate in enumerate(domains[pad]):
                        blockers = [p for p, fixed in outside.items()
                                    if not conflicts.compatible(candidate, fixed)]
                        if blockers:
                            pressure.update(blockers)
                        else:
                            values.append(i)
                    available[pad] = tuple(values)
                answer = search(available, {})
                if answer is not None:
                    selected.update(answer)
                    diagnostic = "compatible local assignment found"
                    break
                if not pressure:
                    diagnostic = "no compatible assignment in local domains"
                    break
                if len(cluster) >= options.maximum_cluster_pins:
                    diagnostic = "local cluster pin budget exceeded"
                    break
                # Negotiate only a proven newly-selected blocker. Grow one pin
                # at a time, preserving the same root search/pair budgets and
                # every outside reservation until a complete solution exists.
                blocker = min(pressure, key=lambda p: (-pressure[p], rank[p]))
                cluster.add(blocker)
            trials.append(EscapeAssignmentTrial(root, tuple(sorted(cluster)), states,
                                                answer is not None, diagnostic))
        except _BudgetExceeded as exc:
            trials.append(EscapeAssignmentTrial(root, tuple(sorted(cluster)), states, False, str(exc)))
            if (conflicts.checks >= options.maximum_pair_checks
                    or conflicts.queries >= options.maximum_pair_queries):
                break
    return selected, EscapeAssignmentReport(tuple(trials), conflicts.checks, tuple(sorted(expanded)),
                                           pair_queries=conflicts.queries,
                                           broad_phase_accepts=conflicts.broad_phase_accepts)

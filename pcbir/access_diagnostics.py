"""Bounded observations of access proposals, never routing permissions."""
from collections import Counter
from dataclasses import dataclass
from typing import Callable

from .physical import Point, TrackSegment, Via
from .routing_clearance import RoutingBlocker


@dataclass(frozen=True, slots=True)
class AccessAttempt:
    site: Point
    phase: str
    outcome: str
    tracks: tuple[TrackSegment, ...] = ()
    via: Via | None = None
    blockers: tuple[RoutingBlocker, ...] = ()
    blockers_truncated: bool = False
    context_digest: str = ""


@dataclass(frozen=True, slots=True)
class AccessDiagnostics:
    source_digest: str
    observed_site_queries: int
    # Counts include unsampled events and repeated sites in different phases.
    outcome_counts: tuple[tuple[str, str, int], ...]
    samples: tuple[AccessAttempt, ...]
    samples_truncated: bool
    stop_reason: str
    # The geometric proposer observes sites/paths, not maze expansions.
    expanded_states: int | None = None
    unobserved_phases: tuple[str, ...] = ()


class AccessRecorder:
    """Bound storage AND extra blocker queries without limiting the search.

    One third of sample slots are reserved for assignment conflicts; per-key
    caps retain refinement evidence. Zero disables blocker collection. The
    The number of blocker-collection callbacks never exceeds the sample limit;
    one callback may inspect several exact emitted path pieces and a via.
    """

    def __init__(self, maximum_samples: int = 24, maximum_blockers: int = 8):
        if (type(maximum_samples) is not int or maximum_samples < 0
                or type(maximum_blockers) is not int or maximum_blockers <= 0):
            raise ValueError("access diagnostic sample bounds are invalid")
        self.maximum_samples = maximum_samples
        self.maximum_blockers = maximum_blockers
        self.site_queries = 0
        self.counts = Counter()
        self.samples = []
        self.sample_counts = Counter()
        self.generation_samples = 0
        self.assignment_samples = 0
        self.truncated = False
        self.unobserved_phases = set()

    def record(self, site: Point, phase: str, outcome: str, *,
               tracks: tuple[TrackSegment, ...] = (), via: Via | None = None,
               blockers: Callable[[], tuple[RoutingBlocker, ...]] | None = None,
               context_digest: str = ""):
        key = phase, outcome
        self.counts[key] += 1
        reserved = self.maximum_samples // 3
        if (len(self.samples) >= self.maximum_samples
                or self.sample_counts[key] >= max(1, self.maximum_samples // 8)
                or phase != 'assignment' and self.generation_samples >= self.maximum_samples-reserved
                or phase == 'assignment' and self.assignment_samples >= reserved and reserved > 0):
            self.truncated = True
            return
        observations = blockers() if blockers else ()
        self.samples.append(AccessAttempt(site, phase, outcome, tracks, via,
            observations[:self.maximum_blockers], len(observations) > self.maximum_blockers,
            context_digest))
        self.sample_counts[key] += 1
        if phase == 'assignment':
            self.assignment_samples += 1
        else:
            self.generation_samples += 1

    def finish(self, source_digest: str, stop_reason: str) -> AccessDiagnostics:
        return AccessDiagnostics(source_digest, self.site_queries,
            tuple((phase, outcome, count) for (phase, outcome), count in sorted(self.counts.items())),
            tuple(self.samples), self.truncated, stop_reason,
            unobserved_phases=tuple(sorted(self.unobserved_phases)))

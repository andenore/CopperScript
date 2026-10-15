"""Exact geometric containment for redundant straight-track construction.

This proves only equal-width copper coverage by one existing segment. It grants
no ownership, routing exemption, connectivity or source-validation authority.
"""
from __future__ import annotations

from collections.abc import Iterable

from .geometry import point_on_segment
from .physical import TrackSegment


def track_is_contained(candidate: TrackSegment, existing: Iterable[TrackSegment]) -> bool:
    """Whether one original equal-net/layer/width segment contains both ends.

    Integer centerline containment at equal width also contains the complete
    rounded track shape. Partial overlap, a union of segments and wider copper
    deliberately do not establish this narrow producer deduplication predicate.
    """
    return any(held.net == candidate.net and held.layer is candidate.layer
               and held.width_nm == candidate.width_nm
               and point_on_segment(candidate.start, held.start, held.end)
               and point_on_segment(candidate.end, held.start, held.end)
               for held in existing)

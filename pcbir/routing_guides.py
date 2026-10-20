"""Physical guide exposure for detailed-search costs, not a clearance gate.

Guides are unions of segment capsules and square access regions, matching
the detailed router's point-membership predicate. Clip an entire edge to the
union so coordinate insertion does not charge another deviation event. Float
intersection parameters are rounded only once to integer nanometres per edge;
subdivision can therefore differ by at most one nanometre per extra edge.
"""
from math import hypot, sqrt

from .physical import CopperLayer, Point
from .routing import GlobalNetRoute


def _slab(origin: float, delta: float, low: float, high: float,
          interval: tuple[float, float]) -> tuple[float, float] | None:
    if delta == 0:
        return interval if low <= origin <= high else None
    first, last = sorted(((low - origin) / delta, (high - origin) / delta))
    first, last = max(first, interval[0]), min(last, interval[1])
    return (first, last) if first <= last else None


def _disk_interval(x: float, y: float, dx: float, dy: float,
                   radius: int) -> tuple[float, float] | None:
    # Work in coordinates relative to the disk, avoiding large board offsets.
    squared = dx * dx + dy * dy
    projection = -(x * dx + y * dy) / squared
    perpendicular_squared = (x * dy - y * dx) ** 2 / squared
    remaining = radius * radius - perpendicular_squared
    if remaining < 0:
        return None
    half = sqrt(remaining / squared)
    first, last = max(0.0, projection - half), min(1.0, projection + half)
    return (first, last) if first <= last else None


class GuideExposure:
    """Preclassified regions; reused within one net's bounded search."""

    def __init__(self, guide: GlobalNetRoute, margin_nm: int, *,
                 ignore_layer: bool = False):
        self.ignore_layer = ignore_layer
        self.capsules = tuple((
            segment.layer, segment.start, segment.end,
            segment.guide_half_width_nm + margin_nm,
        ) for segment in guide.segments)
        self.squares = tuple((access.layer, access.access_position, margin_nm)
                             for access in guide.accesses)

    def outside_length_nm(self, first: Point, second: Point,
                          layer: CopperLayer) -> int:
        dx, dy = second.x_nm - first.x_nm, second.y_nm - first.y_nm
        length = hypot(dx, dy)
        if length == 0:
            return 0
        intervals: list[tuple[float, float]] = []
        xmin, xmax = sorted((first.x_nm, second.x_nm))
        ymin, ymax = sorted((first.y_nm, second.y_nm))
        for owner, start, end, radius in self.capsules:
            if not self.ignore_layer and owner is not layer:
                continue
            if (xmax < min(start.x_nm, end.x_nm) - radius
                    or xmin > max(start.x_nm, end.x_nm) + radius
                    or ymax < min(start.y_nm, end.y_nm) - radius
                    or ymin > max(start.y_nm, end.y_nm) + radius):
                continue
            sx, sy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
            span = hypot(sx, sy)
            if span:
                # Rectangle in the capsule's local (longitudinal, normal) frame.
                ox, oy = first.x_nm - start.x_nm, first.y_nm - start.y_nm
                interval = _slab((ox * sx + oy * sy) / span,
                                 (dx * sx + dy * sy) / span, 0, span, (0, 1))
                if interval is not None:
                    interval = _slab((ox * -sy + oy * sx) / span,
                                     (dx * -sy + dy * sx) / span,
                                     -radius, radius, interval)
                    if interval is not None:
                        intervals.append(interval)
            for center in (start, end) if span else (start,):
                interval = _disk_interval(first.x_nm - center.x_nm,
                                          first.y_nm - center.y_nm, dx, dy, radius)
                if interval is not None:
                    intervals.append(interval)
        for owner, center, radius in self.squares:
            if not self.ignore_layer and owner is not layer:
                continue
            interval = _slab(first.x_nm - center.x_nm, dx, -radius, radius, (0, 1))
            if interval is not None:
                interval = _slab(first.y_nm - center.y_nm, dy, -radius, radius, interval)
                if interval is not None:
                    intervals.append(interval)
        covered = 0.0
        previous_end = 0.0
        for begin, end in sorted(intervals):
            covered += max(0.0, end - max(begin, previous_end))
            previous_end = max(previous_end, end)
        return round(length * max(0.0, min(1.0, 1.0 - covered)))


class GuideDistance:
    """Lower bounds on guide-exposed length, for admissible search heuristics.

    The region is the union of every layer's capsules and access squares, so
    a distance to it never exceeds the distance to one layer's region that
    ``GuideExposure`` charges. A via changes layer at one planar point.
    """

    def __init__(self, guide: GlobalNetRoute, margin_nm: int):
        self.capsules = tuple((segment.start, segment.end, segment.guide_half_width_nm + margin_nm)
                              for segment in guide.segments)
        self.squares = tuple((access.access_position, margin_nm) for access in guide.accesses)

    def distance_nm(self, point: Point) -> float:
        """Euclidean distance to the guide union; ``inf`` for an empty guide."""
        best = float("inf")
        for start, end, radius in self.capsules:
            dx, dy = end.x_nm - start.x_nm, end.y_nm - start.y_nm
            ox, oy = point.x_nm - start.x_nm, point.y_nm - start.y_nm
            span = dx * dx + dy * dy
            fraction = max(0.0, min(1.0, (ox * dx + oy * dy) / span)) if span else 0.0
            distance = hypot(ox - fraction * dx, oy - fraction * dy) - radius
            if distance < best:
                best = distance
                if best <= 0:
                    return 0.0
        for center, radius in self.squares:
            dx = max(0, abs(point.x_nm - center.x_nm) - radius)
            dy = max(0, abs(point.y_nm - center.y_nm) - radius)
            distance = hypot(dx, dy)
            if distance < best:
                best = distance
                if best <= 0:
                    return 0.0
        return best

    def exposed_length_bound_nm(self, first: Point, first_distance: float,
                                second: Point, second_distance: float) -> int:
        """Guide-exposed length that any planar path between two points must have.

        A path either stays outside the union, exposing all of its length (at
        least the straight distance), or enters it, exposing at least each
        endpoint's distance to it. ``outside_length_nm`` rounds each edge to a
        nanometre, so the bound keeps 1 % slack: it stays below the charged
        length for every edge longer than 50 nm.
        """
        straight = hypot(second.x_nm - first.x_nm, second.y_nm - first.y_nm)
        bound = min(straight, first_distance + second_distance)
        return max(0, int(0.99 * bound))


def guide_transition_cost(inside_from: bool, inside_to: bool, event_cost: int) -> int:
    """A via entering an unguided layer is one event, not planar wirelength."""
    return event_cost if inside_from and not inside_to else 0

"""Polygon-aware coarse keepout geometry, without changing exact copper gates."""
from dataclasses import dataclass
from fractions import Fraction
from math import ceil, floor

from .geometry import Bounds, bounds, point_in_polygon, polygon_triangles
from .physical import CopperLayer, Point


@dataclass(frozen=True, slots=True)
class GlobalTrackKeepout:
    layers: tuple[CopperLayer, ...]
    vertices: tuple[Point, ...]
    area: Bounds
    triangles: tuple[tuple[Point, Point, Point], ...]

    @classmethod
    def from_keepout(cls, keepout):
        # Holes retain the conservative filled outer ring. Exact routing
        # already reports keepout holes as unsupported and fails closed.
        vertices = keepout.outline.outer.vertices
        return cls(keepout.layers, vertices, bounds(vertices), polygon_triangles(vertices))

    def blocks_node(self, point, layer):
        return (layer in self.layers
                and self.area.min_x <= point.x_nm <= self.area.max_x
                and self.area.min_y <= point.y_nm <= self.area.max_y
                and point_in_polygon(point, self.vertices))

    def blocked_intervals(self, first, second, half_width_nm, clearance_nm, layer):
        """Conservative transverse projection over the *whole* coarse edge.

        Clipping each convex triangle to the longitudinal strip preserves a
        concave region's empty bays. Expanding in both axes is conservative
        for Euclidean clearance. Rational cuts round outward to integer nm.
        A narrow barrier between grid centers must still cut the resource.
        """
        if layer not in self.layers:
            return ()
        horizontal = first.y_nm == second.y_nm
        along = (lambda p: p.x_nm) if horizontal else (lambda p: p.y_nm)
        across = (lambda p: p.y_nm) if horizontal else (lambda p: p.x_nm)
        low, high = sorted((along(first), along(second)))
        low -= clearance_nm
        high += clearance_nm
        center = across(first)
        box_low, box_high = ((self.area.min_x, self.area.max_x) if horizontal
                             else (self.area.min_y, self.area.max_y))
        side_low, side_high = ((self.area.min_y, self.area.max_y) if horizontal
                               else (self.area.min_x, self.area.max_x))
        if (box_high < low or box_low > high
                or side_high + clearance_nm < center - half_width_nm
                or side_low - clearance_nm > center + half_width_nm):
            return ()
        result = []
        for triangle in self.triangles:
            values = [Fraction(across(p)) for p in triangle if low <= along(p) <= high]
            for a, b in zip(triangle, (*triangle[1:], triangle[0])):
                av, bv = along(a), along(b)
                if av == bv:
                    continue
                for boundary in (low, high):
                    if min(av, bv) <= boundary <= max(av, bv):
                        values.append(Fraction(across(a)) +
                                      Fraction(boundary - av, bv - av) * (across(b) - across(a)))
            if values:
                result.append((floor(min(values)) - clearance_nm,
                               ceil(max(values)) + clearance_nm))
        return tuple(result)

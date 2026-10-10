"""Routing preferences separate from electrical clearance and connectivity."""

from .physical import TrackSegment


def proper_same_net_crossing(first: TrackSegment, second: TrackSegment) -> bool:
    """True only for an interior/interior X on one copper layer.

    Shared endpoints, T junctions and collinear trunks are intentional connection
    shapes, not gratuitous crosses. Use exact integer orientation predicates;
    different-layer crossings remain legal. Never rewrite immutable input copper.
    """
    if first.net != second.net or first.layer is not second.layer:
        return False
    def side(a, b, p):
        return (b.x_nm-a.x_nm)*(p.y_nm-a.y_nm) - (b.y_nm-a.y_nm)*(p.x_nm-a.x_nm)
    a, b, c, d = first.start, first.end, second.start, second.end
    return side(a, b, c)*side(a, b, d) < 0 and side(c, d, a)*side(c, d, b) < 0


def escape_paths_cross(first, second) -> bool:
    return any(proper_same_net_crossing(a, b) for a in first for b in second)

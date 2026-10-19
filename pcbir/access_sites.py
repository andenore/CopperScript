"""Finite geometry-derived via proposals; callers own all legality checks."""
from .physical import PadKind, PadShape, Point
from .placement import transformed_local_point


def circular_array_depth(board, placement, pad, maximum_neighbor_distance_nm):
    """Ring depth in a complete regular circular-land array, else unknown.

    This is a proposal-ordering hint derived from lands, not a package-name
    selector or an electrical escape rule. Missing/irregular grids stay unknown.
    """
    if pad.kind is not PadKind.SMD or pad.shape is not PadShape.CIRCLE:
        return None
    footprint = board.footprints[placement.footprint]
    lands = tuple(p for p in footprint.pads if p.kind is PadKind.SMD)
    if any(p.shape is not PadShape.CIRCLE for p in lands):
        return None
    points = {p.position for p in lands}
    xs = sorted({p.x_nm for p in points})
    ys = sorted({p.y_nm for p in points})
    if min(len(xs), len(ys)) < 3 or len(points) != len(xs)*len(ys):
        return None
    pitches = {b-a for axis in (xs,ys) for a,b in zip(axis,axis[1:])}
    if len(pitches) != 1 or next(iter(pitches)) > maximum_neighbor_distance_nm:
        return None
    x,y = xs.index(pad.position.x_nm), ys.index(pad.position.y_nm)
    return min(x,y,len(xs)-1-x,len(ys)-1-y)


def circular_cell_sites(board, placement, pad, maximum_neighbor_distance_nm, maximum_radius_nm):
    """At most four actual square-cell centres, with no package/pitch selector."""
    if pad.kind is not PadKind.SMD or pad.shape is not PadShape.CIRCLE:
        return ()
    points = {p.position for p in board.footprints[placement.footprint].pads
              if p.kind is PadKind.SMD and p.shape is PadShape.CIRCLE}
    x, y = pad.position.x_nm, pad.position.y_nm
    sites = []
    for sx, sy in ((1, 1), (1, -1), (-1, 1), (-1, -1)):
        dx = min((sx*(p.x_nm-x) for p in points if p.y_nm == y
                  and sx*(p.x_nm-x) > 0), default=0)
        dy = min((sy*(p.y_nm-y) for p in points if p.x_nm == x
                  and sy*(p.y_nm-y) > 0), default=0)
        if (not dx or dx != dy or dx > maximum_neighbor_distance_nm
                or dx*dx > 2*maximum_radius_nm**2
                or Point(x+sx*dx, y+sy*dy) not in points):
            continue
        sites.append(transformed_local_point(placement, Point(x+sx*(dx//2), y+sy*(dy//2))))
    start = transformed_local_point(placement, pad.position)
    return tuple(sorted(set(sites), key=lambda p: (
        -((p.x_nm-start.x_nm)*(start.x_nm-placement.position.x_nm)
          +(p.y_nm-start.y_nm)*(start.y_nm-placement.position.y_nm)), p.x_nm, p.y_nm)))


def obstacle_boundary_sites(clearance, net, start, size_nm, drill_nm, span,
                            maximum_radius_nm, maximum_sites, *, seeds=(), maximum_obstacles=16):
    """Project local obstacle frontiers around a finite set of launch seeds.

    Project frontiers onto the actual launch rays too. This retains a compact
    straight dogbone when a cell centre is blocked, instead of necessarily
    turning an offset launch into an elbow. No reference/package dimensions
    are copied: the seed direction and obstacle boundaries supply the geometry.
    Expanded boxes only propose sites; they neither remove rounded free space
    nor establish that a via/path fits. The exact caller checks every span.
    There is no adaptive grid, search restart, or hidden allowance growth.
    """
    if type(maximum_sites) is not int or maximum_sites < 0:
        raise ValueError('geometry site limit must be a nonnegative integer')
    if not maximum_sites:
        return ()
    if type(maximum_obstacles) is not int or maximum_obstacles <= 0:
        raise ValueError('geometry obstacle limit must be a positive integer')
    boundaries = clearance.via_obstacle_bounds(net, start, size_nm, drill_nm, *span,
                                              maximum_radius_nm, maximum_obstacles)
    seeds = tuple(dict.fromkeys((*seeds, start)))
    candidates, ray_candidates = set(), set()
    for seed in seeds:
        xs = sorted({x for b in boundaries for x in (b.min_x, b.max_x)},
                    key=lambda x: (abs(x-seed.x_nm), x))[:4]
        ys = sorted({y for b in boundaries for y in (b.min_y, b.max_y)},
                    key=lambda y: (abs(y-seed.y_nm), y))[:4]
        dx, dy = seed.x_nm-start.x_nm, seed.y_nm-start.y_nm
        # Existing octilinear directions only, between this terminal and its
        # supplied seed. One-nanometre neighbors bracket strict boundaries;
        # they do not grant clearance or via-in-pad permission.
        if (dx or dy) and (not dx or not dy or abs(dx) == abs(dy)):
            for x in (v+d for v in xs for d in (-1, 0, 1)):
                if dx and 0 < (x-start.x_nm)*dx < dx*dx:
                    ray_candidates.add(Point(x, start.y_nm+(x-start.x_nm)*dy//dx))
            for y in (v+d for v in ys for d in (-1, 0, 1)):
                if dy and 0 < (y-start.y_nm)*dy < dy*dy:
                    ray_candidates.add(Point(start.x_nm+(y-start.y_nm)*dx//dy, y))
        for x in (*xs, seed.x_nm):
            for y in (*ys, seed.y_nm):
                point = Point(x, y)
                if (point not in seeds and (x-start.x_nm)**2 + (y-start.y_nm)**2
                        <= maximum_radius_nm**2):
                    candidates.add(point)
    ray_candidates = {p for p in ray_candidates if p not in seeds
                      and (p.x_nm-start.x_nm)**2+(p.y_nm-start.y_nm)**2 <= maximum_radius_nm**2}
    return tuple(sorted(candidates | ray_candidates, key=lambda p: (
        p not in ray_candidates,
        min((p.x_nm-s.x_nm)**2+(p.y_nm-s.y_nm)**2 for s in seeds),
        (p.x_nm-start.x_nm)**2+(p.y_nm-start.y_nm)**2, p.x_nm, p.y_nm))[:maximum_sites])

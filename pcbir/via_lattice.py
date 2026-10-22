"""Package via-site lattices, reserved before anything else is routed (escape plan E7).

Reference ball-grid layouts (the Infineon FX10 kit, for example) give every
connected ball that leaves its surface layer one dogbone via of its own, in a
regular quadrant-to-corner pattern, before decoupling, critical or general
routing: the via rows then leave straight channels on the inner layers, and
no later stage can take a ball's only site.

This module chooses those sites jointly. Each ball that needs a via may use
one of the cell centres around it (its four diagonal neighbours; an edge ball
also the cells just outside the array); a cell holds one via. An integer
min-cost assignment (escape_plan._MinCostFlow) gives the most balls a site,
preferring the outward diagonal and keeping signal vias out of the array's
outer cell rings, which carry the surface escapes of the edge balls. Every
site and stub is checked exactly against the board, so the lattice is legal
by construction; balls left without a site are reported with the reason.

Which balls need a via: plane (zone) nets, the supply and return nets of
decoupling links, and signal balls that do not escape on the surface (deeper
than ``surface_rings``, or not in the array plan's surface set when one is
given). Critical nets, macro-owned balls and nets confined to the surface
layer keep their own escapes.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace

from .physical import BoardSide, CopperLayer, PadReference, PhysicalBoard, Point, RouteKind, TrackSegment, Via


@dataclass(frozen=True, slots=True)
class LatticeSite:
    pad: PadReference
    net: str
    position: Point
    cell: tuple[int, int]      # array cell; -1 or n-1 lies just outside the array
    preferred: bool            # the outward (quadrant-to-corner) diagonal


@dataclass(frozen=True, slots=True)
class PackageLattice:
    component: str
    pitch_nm: int
    sites: tuple[LatticeSite, ...]
    # (ball, reason): "no legal site" when every candidate fails the exact
    # check, "sites taken" when its legal cells serve other balls.
    unsited: tuple[tuple[PadReference, str], ...] = ()

    def report(self) -> dict:
        return {"component": self.component, "sites": len(self.sites),
                "preferred": sum(site.preferred for site in self.sites),
                "unsited": [f"{pad.component}.{pad.pad}: {reason}" for pad, reason in self.unsited]}


@dataclass(frozen=True, slots=True)
class LatticeResult:
    board: PhysicalBoard
    lattices: tuple[PackageLattice, ...]
    tracks: tuple[TrackSegment, ...] = field(default=())
    vias: tuple[Via, ...] = field(default=())

    def report(self) -> dict:
        return {"packages": [lattice.report() for lattice in self.lattices],
                "new_vias": len(self.vias), "new_tracks": len(self.tracks)}


# Assignment costs: the outward diagonal is free; another diagonal, a cell in
# the outer band (for a signal ball) or outside the array cost extra.
_OTHER_DIAGONAL = 2
_BAND = 4
_OUTSIDE = 1


def plan_via_lattices(board: PhysicalBoard, *, surface: frozenset[PadReference] | None = None,
                      surface_rings: int = 2, band_rings: int = 1) -> LatticeResult:
    """Reserve one via site per ball that needs one, for every full ball grid on ``board``.

    Returns the board with the dogbone stubs and vias added. ``surface`` (from
    ``plan_array_escapes``) names the signal balls that escape on the surface;
    without it, signal balls within ``surface_rings`` of the edge do.
    """
    from .escape_plan import _MinCostFlow
    from .hard_macros import macro_owned_pads
    from .package_patterns import circular_array
    from .placement import transformed_local_point
    from .routing_clearance import RoutingClearanceIndex
    from .routing_layers import routing_layers
    from .zone_geometry import deferred_zone_nets

    rules = {rule.net: rule for rule in board.net_routing_rules}
    net_of = {pad: net.name for net in board.nets for pad in net.pads}
    planes = set(deferred_zone_nets(board))
    supplies = planes | {link.net for link in board.decoupling_links} | {
        link.return_net for link in board.decoupling_links}
    owned = set(macro_owned_pads(board))
    index = RoutingClearanceIndex(board)
    layers = board.stackup.copper_layers
    size, drill = board.rules.default_via_size_nm, board.rules.default_via_drill_nm
    width = board.rules.minimum_track_width_nm
    lattices, tracks, vias = [], [], []
    for pose in sorted(board.placements, key=lambda p: p.reference):
        try:
            array = circular_array(board, pose.reference)
        except ValueError:
            array = None
        if array is None:
            continue
        side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
        nx, ny, half = len(array.xs_nm), len(array.ys_nm), array.pitch_nm // 2
        balls = []  # (reference, net, ix, iy, land position, needs-band-free)
        for land in sorted(board.footprints[pose.footprint].pads, key=lambda p: p.number):
            reference = PadReference(pose.reference, land.number)
            net = net_of.get(reference)
            if net is None or reference in owned:
                continue
            rule = rules.get(net)
            supply = net in supplies
            if rule is not None and rule.kind is not RouteKind.GENERAL and not supply:
                continue  # critical routers own their escapes
            if not supply and not any(layer != side for layer in routing_layers(board, net, rule)):
                continue
            ix, iy = array.xs_nm.index(land.position.x_nm), array.ys_nm.index(land.position.y_nm)
            ring = min(ix, iy, nx - 1 - ix, ny - 1 - iy)
            if not supply and (reference in surface if surface is not None else ring < surface_rings):
                continue
            balls.append((reference, net, ix, iy, land.position, not supply))
        if not balls:
            continue
        # Candidate cells per ball, each checked exactly (via and stub).
        cells: dict[tuple[int, int], int] = {}
        candidates = []  # per ball: [(cell, site, cost, preferred)]
        for reference, net, ix, iy, position, signal in balls:
            sx = -1 if 2 * ix < nx - 1 else 1
            sy = -1 if 2 * iy < ny - 1 else 1
            start = transformed_local_point(pose, position)
            options = []
            for dx, dy in ((sx, sy), (sx, -sy), (-sx, sy), (-sx, -sy)):
                cell = (ix - (dx < 0), iy - (dy < 0))
                outside = not (0 <= cell[0] <= nx - 2 and 0 <= cell[1] <= ny - 2)
                if outside and signal:
                    continue  # a signal ball that leaves the array does so on the surface
                band = not outside and min(cell[0], cell[1], nx - 2 - cell[0], ny - 2 - cell[1]) < band_rings
                site = transformed_local_point(pose, Point(position.x_nm + dx * half, position.y_nm + dy * half))
                if not index.can_via(net, site, size, layers[0], layers[-1], drill_nm=drill):
                    continue
                if not index.can_track(net, start, site, width, side):
                    continue
                cost = ((0 if (dx, dy) == (sx, sy) else _OTHER_DIAGONAL) + (_BAND if band and signal else 0)
                        + (_OUTSIDE if outside else 0))
                options.append((cell, site, cost, (dx, dy) == (sx, sy)))
                cells.setdefault(cell, len(cells))
            candidates.append(options)
        flow = _MinCostFlow(2 + len(balls) + len(cells))
        source, sink = 0, 1
        edges = []
        for number, options in enumerate(candidates):
            flow.add(source, 2 + number, 1, 0)
            edges.append([(flow.add(2 + number, 2 + len(balls) + cells[cell], 1, cost), cell, site, preferred)
                          for cell, site, cost, preferred in options])
        for cell, number in cells.items():
            flow.add(2 + len(balls) + number, sink, 1, 0)
        flow.run(source, sink)
        sites, unsited = [], []
        for number, (reference, net, ix, iy, position, _) in enumerate(balls):
            chosen = next(((cell, site, preferred) for edge, cell, site, preferred in edges[number]
                           if flow.edge_flow(2 + number, edge)), None)
            if chosen is None:
                unsited.append((reference, "sites taken" if candidates[number] else "no legal site"))
                continue
            cell, site, preferred = chosen
            stub = TrackSegment(net, transformed_local_point(pose, position), site, width, side)
            via = Via(net, site, size, drill, layers[0], layers[-1])
            # Sites were checked against the board alone; check them against
            # the sites already accepted too (one via per cell is not a proof).
            if not (index.can_via(net, site, size, layers[0], layers[-1], drill_nm=drill)
                    and index.can_track(net, stub.start, site, width, side)):
                unsited.append((reference, "conflicts with another site"))
                continue
            sites.append(LatticeSite(reference, net, site, cell, preferred))
            index.add_track(stub)
            index.add_via(via)
            tracks.append(stub)
            vias.append(via)
        lattices.append(PackageLattice(pose.reference, array.pitch_nm, tuple(sites), tuple(unsited)))
    if not vias:
        return LatticeResult(board, tuple(lattices))
    result = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))
    return LatticeResult(result, tuple(lattices), tuple(tracks), tuple(vias))

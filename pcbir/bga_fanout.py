"""Deterministic dogbone fanout for full-grid ball arrays (routing refactor P4).

For every complete, square-pitch array of round SMD lands (a BGA), each ball
that needs a layer change gets a short stub to a through via at the centre of
one of its four neighbouring cells. The preferred cell points away from the
package centre (the classic quadrant pattern); the other three diagonals are
fallbacks. Every stub and via is checked exactly against the board and the
copper accepted so far, so the pattern is legal by construction.

Which balls get a via:

* balls of plane (zone) nets, so they reach their planes;
* signal balls deeper than ``surface_rings`` from the array edge, which cannot
  escape on the surface layer;
* never balls without a net, balls already joined by existing copper, or
  balls whose net may only use the surface layer.

There is no search over alternatives and no gate: a ball whose four sites are
all blocked is reported and left to the router's own terminal access.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .package_patterns import circular_array
from .physical import (BoardSide, CopperKeepout, CopperLayer, PadReference, PhysicalBoard, Point,
                       PolygonRing, PolygonWithHoles, RouteKind, TrackSegment, Via)
from .pin_escape import checked_access_path
from .placement import transformed_local_point
from .routing_clearance import RoutingClearanceIndex
from .routing_layers import routing_layers


@dataclass(frozen=True, slots=True)
class BgaFanoutResult:
    board: PhysicalBoard
    tracks: tuple[TrackSegment, ...]
    vias: tuple[Via, ...]
    packages: tuple[str, ...]
    fanned: tuple[PadReference, ...]
    blocked: tuple[PadReference, ...]
    surface: tuple[PadReference, ...] = field(default=())

    def report(self) -> dict:
        return {"packages": list(self.packages), "fanned": len(self.fanned),
                "surface": len(self.surface), "blocked": [str(p) for p in self.blocked],
                "new_vias": len(self.vias), "new_tracks": len(self.tracks)}


def plan_bga_fanouts(board: PhysicalBoard, *, surface_rings: int = 2,
                     stub_width_nm: int | None = None) -> BgaFanoutResult:
    from .drc import explicit_copper_connectivity
    from .hard_macros import macro_owned_pads
    from .zone_geometry import deferred_zone_nets

    rules = {rule.net: rule for rule in board.net_routing_rules}
    net_of = {pad: net.name for net in board.nets for pad in net.pads}
    plane_nets = deferred_zone_nets(board)
    owned = set(macro_owned_pads(board))
    connectivity = explicit_copper_connectivity(board)
    copper_roots = {root for key, root in connectivity.roots.items() if not key.startswith("pad:")}
    index = RoutingClearanceIndex(board)
    layers = board.stackup.copper_layers
    via_size = board.rules.default_via_size_nm
    via_drill = board.rules.default_via_drill_nm
    width = stub_width_nm or board.rules.minimum_track_width_nm
    tracks: list[TrackSegment] = []
    vias: list[Via] = []
    packages, fanned, blocked, surface = [], [], [], []
    for pose in sorted(board.placements, key=lambda p: p.reference):
        try:
            array = circular_array(board, pose.reference)
        except ValueError:
            array = None
        if array is None:
            continue
        packages.append(pose.reference)
        side = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
        footprint = board.footprints[pose.footprint]
        nx, ny = len(array.xs_nm), len(array.ys_nm)
        half = array.pitch_nm // 2
        # Inner balls first: they have the fewest alternatives.
        def depth(pad):
            ix, iy = array.xs_nm.index(pad.position.x_nm), array.ys_nm.index(pad.position.y_nm)
            return min(ix, iy, nx - 1 - ix, ny - 1 - iy)
        for land in sorted(footprint.pads, key=lambda p: (-depth(p), p.number)):
            reference = PadReference(pose.reference, land.number)
            net = net_of.get(reference)
            if net is None or reference in owned:
                continue
            rule = rules.get(net)
            if rule is not None and rule.kind is not RouteKind.GENERAL and net not in plane_nets:
                continue  # Critical routers own their own escapes.
            allowed = routing_layers(board, net, rule)
            if not any(layer != side for layer in allowed) and net not in plane_nets:
                continue
            ring = depth(land)
            if net not in plane_nets and ring < surface_rings:
                surface.append(reference)
                continue
            if any(connectivity.roots[node] in copper_roots
                   for node in connectivity.pad_nodes.get(reference, ())):
                continue  # Already has a launch (critical route, macro, earlier fanout).
            ix = array.xs_nm.index(land.position.x_nm)
            iy = array.ys_nm.index(land.position.y_nm)
            sx = -1 if 2 * ix < nx - 1 else 1
            sy = -1 if 2 * iy < ny - 1 else 1
            # The four adjacent cell centres (outward first), then the next
            # ring of cell centres reached by a checked octilinear stub.
            options = [(sx, sy), (sx, -sy), (-sx, sy), (-sx, -sy)]
            options += sorted(((dx, dy) for dx in (-3, -1, 1, 3) for dy in (-3, -1, 1, 3)
                               if max(abs(dx), abs(dy)) == 3),
                              key=lambda d: (-(d[0] * sx + d[1] * sy), d))
            start = transformed_local_point(pose, land.position)
            accepted = False
            for dx, dy in options:
                local = Point(land.position.x_nm + dx * half, land.position.y_nm + dy * half)
                site = transformed_local_point(pose, local)
                via = Via(net, site, via_size, via_drill, layers[0], layers[-1])
                if not index.can_via(net, site, via_size, via.from_layer, via.to_layer, drill_nm=via_drill):
                    continue
                if abs(dx) == 1:
                    stubs = ((TrackSegment(net, start, site, width, side),)
                             if index.can_track(net, start, site, width, side) else None)
                else:
                    path = checked_access_path(board, index, net, start, site, width, side)
                    stubs = tuple(path) if path else None
                if not stubs:
                    continue
                for stub in stubs:
                    index.add_track(stub)
                    tracks.append(stub)
                index.add_via(via)
                vias.append(via)
                fanned.append(reference)
                accepted = True
                break
            if not accepted:
                blocked.append(reference)
    if not tracks:
        return BgaFanoutResult(board, (), (), tuple(packages), (), tuple(blocked), tuple(surface))
    from dataclasses import replace
    result = replace(board, tracks=(*board.tracks, *tracks), vias=(*board.vias, *vias))
    return BgaFanoutResult(result, tuple(tracks), tuple(vias), tuple(packages), tuple(fanned),
                           tuple(blocked), tuple(surface))


ESCAPE_STRIP_PREFIX = "bga-escape-strip"


def escape_strips(board: PhysicalBoard, *, rings: int = 2,
                  depth_nm: int = 2_000_000) -> tuple[CopperKeepout, ...]:
    """Via keepouts that hold a ball array's surface escape channels open.

    For each edge of a full-grid ball array whose outer ``rings`` carry signal
    balls, the strip just outside the outer lands, ``depth_nm`` deep and as
    wide as those balls' span, is closed to vias. Signal balls there leave on
    the surface layer between the outer lands (as manufacturer layouts route
    a bus out of a BGA edge); supply and return vias placed meanwhile then
    take sites inside the ball field instead. Only unrouted ordinary signal
    balls count: supply and return nets (plane nets, decoupling targets and
    returns), critical nets and balls already launched by copper do not.
    The strips are temporary: callers hold them only while placing supply
    and return copper.
    """
    from .drc import explicit_copper_connectivity
    supply = {zone.net for zone in board.zones}
    supply |= {link.net for link in board.decoupling_links}
    supply |= {link.return_net for link in board.decoupling_links}
    rules = {rule.net: rule for rule in board.net_routing_rules}
    # Critical nets own their escapes (they route before this), and a ball
    # whose escape copper already exists needs no channel.
    supply |= {net for net, rule in rules.items() if rule.kind is not RouteKind.GENERAL}
    connectivity = explicit_copper_connectivity(board)
    copper_roots = {root for key, root in connectivity.roots.items() if not key.startswith("pad:")}
    launched = {pad for pad, nodes in connectivity.pad_nodes.items()
                if any(connectivity.roots[node] in copper_roots for node in nodes)}
    net_of = {pad: net.name for net in board.nets for pad in net.pads
              if len(net.pads) >= 2 and pad not in launched}
    layers = board.stackup.copper_layers
    strips: list[CopperKeepout] = []
    for pose in sorted(board.placements, key=lambda p: p.reference):
        try:
            array = circular_array(board, pose.reference)
        except ValueError:
            array = None
        if array is None:
            continue
        xs, ys = array.xs_nm, array.ys_nm
        half, radius = array.pitch_nm // 2, array.land_diameter_nm // 2
        signal = []
        for land in board.footprints[pose.footprint].pads:
            net = net_of.get(PadReference(pose.reference, land.number))
            if net is not None and net not in supply:
                signal.append((xs.index(land.position.x_nm), ys.index(land.position.y_nm)))
        nx, ny = len(xs), len(ys)
        # (edge, balls in its outer rings as (along-axis index), outward local box)
        edges = (
            ("left", [iy for ix, iy in signal if ix < rings], lambda a, b: (
                xs[0] - radius - depth_nm, ys[a] - half, xs[0] - radius, ys[b] + half)),
            ("right", [iy for ix, iy in signal if ix >= nx - rings], lambda a, b: (
                xs[-1] + radius, ys[a] - half, xs[-1] + radius + depth_nm, ys[b] + half)),
            ("top", [ix for ix, iy in signal if iy < rings], lambda a, b: (
                xs[a] - half, ys[0] - radius - depth_nm, xs[b] + half, ys[0] - radius)),
            ("bottom", [ix for ix, iy in signal if iy >= ny - rings], lambda a, b: (
                xs[a] - half, ys[-1] + radius, xs[b] + half, ys[-1] + radius + depth_nm)),
        )
        for name, along, box in edges:
            if not along:
                continue
            x0, y0, x1, y1 = box(min(along), max(along))
            corners = tuple(transformed_local_point(pose, Point(x, y))
                            for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)))
            strips.append(CopperKeepout(f"{ESCAPE_STRIP_PREFIX}-{pose.reference}-{name}", layers,
                                        PolygonWithHoles(PolygonRing(corners)),
                                        block_tracks=False, block_vias=True, block_zones=False))
    return tuple(strips)

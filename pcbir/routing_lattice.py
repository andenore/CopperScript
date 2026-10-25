"""Search lattice of the unified router (multi-resolution lattice plan, S0).

Every node id, planar offset, point/node conversion, search window and
neighbour step of :mod:`pcbir.unified_router` goes through one
:class:`Lattice` object. :class:`UniformLattice` is today's single pitch over
the whole board (formerly ``unified_router._Grid``); a nested lattice with
fine rectangular patches at base pitch / k implements the same protocol.

Node ids are dense: ``layer * plane + column``, where a *column* is the
planar (xy) id shared by every layer at one lattice point (a via site). The
uniform lattice keeps its dense ``(nl, ny, nx)`` shape (``nx``, ``ny``,
``plane``, ``xs``, ``ys``, ``window``) for the legality rasters and the C
kernel.
"""

from __future__ import annotations

from math import ceil, floor, hypot, sqrt
from typing import Iterable, Protocol, Sequence, runtime_checkable

import numpy as np

from .physical import CopperLayer, PhysicalBoard, Point

_SQRT2 = sqrt(2.0)
# 8 planar moves: (dx, dy, length in pitches). The order is part of the
# search contract (move tables, turn table, native kernel).
MOVES = ((1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0),
         (1, 1, _SQRT2), (1, -1, _SQRT2), (-1, 1, _SQRT2), (-1, -1, _SQRT2))


@runtime_checkable
class Lattice(Protocol):
    """What the router needs from a search lattice (S1 and S2 build on it)."""

    pitch_nm: int                      # finest pitch present (uniform: the pitch)
    size: int                          # number of nodes (all layers)
    layers: tuple[CopperLayer, ...]
    nl: int

    def point(self, node: int) -> Point: ...
    def layer_of(self, node: int) -> int: ...
    def node_at(self, layer: int, point: Point) -> int | None: ...   # exact lattice point only
    def nearest(self, layer: int, point: Point) -> int: ...          # nearest node on that layer
    def nodes_in(self, layer: int, x0, y0, x1, y1) -> np.ndarray: ...  # node ids inside a box
    def neighbours(self, node: int) -> tuple[tuple[int, float], ...]: ...  # (node, length in pitches)
    def column(self, node: int) -> int: ...                          # xy id shared by all layers
    def column_nodes(self, column: int) -> tuple[int, ...]: ...
    def step_nm(self, node: int) -> int: ...                         # local pitch at the node


class UniformLattice:
    """One pitch over the board outline's bounding box (or a search window).

    Index boxes ``(ix0, iy0, ix1, iy1)`` are inclusive lattice coordinates
    (``window``, ``full_box``, ``box_of``, ``grow_box``); the search and the
    kernel use them as their bounding windows.
    """

    def __init__(self, board: PhysicalBoard, pitch_nm: int, window=None) -> None:
        pitch = pitch_nm
        vertices = board.outline.vertices
        min_x = min(p.x_nm for p in vertices)
        min_y = min(p.y_nm for p in vertices)
        max_x = max(p.x_nm for p in vertices)
        max_y = max(p.y_nm for p in vertices)
        self.pitch = pitch
        self.pitch_nm = pitch
        self.x0 = floor(min_x / pitch) * pitch
        self.y0 = floor(min_y / pitch) * pitch
        self.nx = int((max_x - self.x0) // pitch) + 2
        self.ny = int((max_y - self.y0) // pitch) + 2
        if window is not None:
            # Retain the full-board lattice phase. A window only discards
            # search nodes; it never translates or clips any source object.
            x0, y0, x1, y1 = window
            ix0 = -((self.x0 - max(min_x, x0)) // pitch)
            iy0 = -((self.y0 - max(min_y, y0)) // pitch)
            ix1 = (min(max_x, x1) - self.x0) // pitch
            iy1 = (min(max_y, y1) - self.y0) // pitch
            self.x0 += ix0 * pitch
            self.y0 += iy0 * pitch
            self.nx, self.ny = ix1 - ix0 + 1, iy1 - iy0 + 1
            if self.nx < 2 or self.ny < 2:
                raise ValueError("grid_window_nm must contain at least two board grid nodes per axis")
        self.layers = tuple(board.stackup.copper_layers)
        self.nl = len(self.layers)
        self.plane = self.nx * self.ny
        self.size = self.plane * self.nl
        self.xs = self.x0 + pitch * np.arange(self.nx, dtype=np.float64)
        self.ys = self.y0 + pitch * np.arange(self.ny, dtype=np.float64)
        # Offsets tuple id -> (offsets, ox, oy, node deltas, bounds); the
        # tuple is kept so its id stays unique while cached.
        self._offsets: dict[int, tuple] = {}
        nx = self.nx
        self._moves = tuple((dx, dy, dy * nx + dx, (0 if dy > 0 else -1) * nx + (0 if dx > 0 else -1), step)
                            for dx, dy, step in MOVES)

    # -- identity -----------------------------------------------------------

    def signature(self) -> tuple[int, int, int, int, int, int]:
        """(pitch, x0, y0, nx, ny, nl): equal signatures share node ids."""
        return (self.pitch, self.x0, self.y0, self.nx, self.ny, self.nl)

    def step_nm(self, node: int) -> int:
        return self.pitch

    # -- dense coordinates (uniform lattice and rasters) -------------------

    def index(self, layer: int, ix: int, iy: int) -> int:
        return layer * self.plane + iy * self.nx + ix

    def unpack(self, node: int) -> tuple[int, int, int]:
        layer, rest = divmod(node, self.plane)
        iy, ix = divmod(rest, self.nx)
        return layer, ix, iy

    def decode(self, node: int) -> tuple[int, int, int, int]:
        """(layer, column, ix, iy) of a node in one call."""
        layer, rest = divmod(node, self.plane)
        iy, ix = divmod(rest, self.nx)
        return layer, rest, ix, iy

    def decoder(self):
        """``decode`` as a closure over the lattice shape (search inner loop)."""
        plane, nx = self.plane, self.nx

        def decode(node: int) -> tuple[int, int, int, int]:
            layer, rest = divmod(node, plane)
            iy, ix = divmod(rest, nx)
            return layer, rest, ix, iy
        return decode

    def point(self, node: int) -> Point:
        _, ix, iy = self.unpack(node)
        return Point(self.x0 + ix * self.pitch, self.y0 + iy * self.pitch)

    def window(self, min_x: float, min_y: float, max_x: float, max_y: float):
        """Index box covering a nanometre box (clipped to the grid)."""
        ix0 = max(0, int(floor((min_x - self.x0) / self.pitch)))
        iy0 = max(0, int(floor((min_y - self.y0) / self.pitch)))
        ix1 = min(self.nx - 1, int(ceil((max_x - self.x0) / self.pitch)))
        iy1 = min(self.ny - 1, int(ceil((max_y - self.y0) / self.pitch)))
        return ix0, iy0, ix1, iy1

    # -- layers and columns ------------------------------------------------

    def layer_of(self, node: int) -> int:
        return node // self.plane

    def column(self, node: int) -> int:
        return node % self.plane

    def column_nodes(self, column: int) -> tuple[int, ...]:
        plane = self.plane
        return tuple(layer * plane + column for layer in range(self.nl))

    def layer_bases(self) -> tuple[int, ...]:
        """Per layer, the id offset of its nodes: ``layer_node(c, l) == layer_bases()[l] + c``."""
        return tuple(layer * self.plane for layer in range(self.nl))

    def layer_node(self, column: int, layer: int) -> int:
        """The node of ``column`` on ``layer``."""
        return layer * self.plane + column

    def column_point(self, column: int) -> Point:
        iy, ix = divmod(column, self.nx)
        return Point(self.x0 + ix * self.pitch, self.y0 + iy * self.pitch)

    # -- points to nodes ---------------------------------------------------

    def _rounded(self, point: Point) -> tuple[int, int]:
        # Python round() of the float ratio: half-way points go to the even index.
        return (int(round((point.x_nm - self.x0) / self.pitch)),
                int(round((point.y_nm - self.y0) / self.pitch)))

    def node_at(self, layer: int, point: Point) -> int | None:
        """The node exactly at ``point`` on ``layer``, or None off the lattice."""
        dx, dy = point.x_nm - self.x0, point.y_nm - self.y0
        if dx % self.pitch or dy % self.pitch:
            return None
        ix, iy = int(dx // self.pitch), int(dy // self.pitch)
        if not (0 <= ix < self.nx and 0 <= iy < self.ny and 0 <= layer < self.nl):
            return None
        return self.index(layer, ix, iy)

    def snap(self, layer: int, point: Point) -> int | None:
        """The node at the rounded lattice position of ``point``; None outside the lattice."""
        ix, iy = self._rounded(point)
        if not (0 <= ix < self.nx and 0 <= iy < self.ny):
            return None
        return self.index(layer, ix, iy)

    def nearest(self, layer: int, point: Point) -> int:
        """The node nearest ``point`` on ``layer`` (rounded as ``snap``, clamped)."""
        ix, iy = self._rounded(point)
        return self.index(layer, min(max(ix, 0), self.nx - 1), min(max(iy, 0), self.ny - 1))

    def nodes_in(self, layer: int, x0, y0, x1, y1) -> np.ndarray:
        """Ids of the nodes on ``layer`` with x0 <= x <= x1 and y0 <= y <= y1 (row-major)."""
        p = self.pitch
        ix0 = max(0, int(ceil((x0 - self.x0) / p)))
        iy0 = max(0, int(ceil((y0 - self.y0) / p)))
        ix1 = min(self.nx - 1, int(floor((x1 - self.x0) / p)))
        iy1 = min(self.ny - 1, int(floor((y1 - self.y0) / p)))
        return self.window_nodes(layer, (ix0, iy0, ix1, iy1))

    def cover(self, min_x: float, min_y: float, max_x: float, max_y: float) -> list[tuple[int, int, int]]:
        """(column, x_nm, y_nm) of every node of ``window``'s covering box, x-major
        (ascending x, then y). Stable sorts by distance keep this order on ties."""
        ix0, iy0, ix1, iy1 = self.window(min_x, min_y, max_x, max_y)
        nx, x0, y0, p = self.nx, self.x0, self.y0, self.pitch
        return [(iy * nx + ix, x0 + ix * p, y0 + iy * p)
                for ix in range(ix0, ix1 + 1) for iy in range(iy0, iy1 + 1)]

    def window_columns(self, window, mask=None) -> np.ndarray:
        """Columns of an index box (row-major), optionally where ``mask``
        (shaped like the box, rows = y) is True."""
        ix0, iy0, ix1, iy1 = window
        if ix0 > ix1 or iy0 > iy1:
            return np.zeros(0, dtype=np.int64)
        if mask is None:
            iy, ix = np.mgrid[iy0:iy1 + 1, ix0:ix1 + 1]
            iy, ix = iy.ravel(), ix.ravel()
        else:
            iy, ix = np.nonzero(mask)
            iy, ix = iy + iy0, ix + ix0
        return (iy * self.nx + ix).astype(np.int64)

    def window_nodes(self, layer: int, window, mask=None) -> np.ndarray:
        """``window_columns`` on ``layer`` as node ids."""
        return self.window_columns(window, mask) + layer * self.plane

    def segment_nodes(self, layer: int, start: Point, end: Point, radius_nm: float) -> list[tuple[int, float]]:
        """(node, distance) for nodes on ``layer`` strictly within ``radius_nm`` of a segment (row-major)."""
        ix0, iy0, ix1, iy1 = self.window(min(start.x_nm, end.x_nm) - radius_nm, min(start.y_nm, end.y_nm) - radius_nm,
                                         max(start.x_nm, end.x_nm) + radius_nm, max(start.y_nm, end.y_nm) + radius_nm)
        ax, ay, bx, by = start.x_nm, start.y_nm, end.x_nm, end.y_nm
        dx, dy = bx - ax, by - ay
        length2 = dx * dx + dy * dy
        base, nx, x0, y0, p = layer * self.plane, self.nx, self.x0, self.y0, self.pitch
        result = []
        for iy in range(iy0, iy1 + 1):
            py = y0 + iy * p
            for ix in range(ix0, ix1 + 1):
                px = x0 + ix * p
                t = 0.0 if not length2 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / length2))
                distance = hypot(px - (ax + t * dx), py - (ay + t * dy))
                if distance < radius_nm:
                    result.append((base + iy * nx + ix, distance))
        return result

    # -- planar steps ------------------------------------------------------

    def move_deltas(self) -> tuple[tuple[int, int, int, int, float], ...]:
        """Per move of ``MOVES``: (dx, dy, node-id delta, diagonal cell delta, length).

        The cell delta leads from a node to the lower-left node of the cell
        a diagonal move crosses (the mid-cell raster's index)."""
        return self._moves

    def neighbours(self, node: int, diagonals: bool = True) -> tuple[tuple[int, float], ...]:
        """In-bounds planar moves from ``node`` in ``MOVES`` order (layer changes are not neighbours)."""
        _, ix, iy = self.unpack(node)
        nx, ny = self.nx, self.ny
        return tuple((node + dn, step) for dx, dy, dn, _, step in (self._moves if diagonals else self._moves[:4])
                     if 0 <= ix + dx < nx and 0 <= iy + dy < ny)

    def delta(self, a: int, b: int) -> tuple[int, int]:
        """Planar lattice step (dx, dy) from node ``a`` to node ``b``."""
        ya, xa = divmod(a % self.plane, self.nx)
        yb, xb = divmod(b % self.plane, self.nx)
        return xb - xa, yb - ya

    def diagonal_cell(self, a: int, b: int) -> int:
        """Cell id (its lower-left node) crossed by the diagonal step a-b, or -1."""
        la, xa, ya = self.unpack(a)
        _, xb, yb = self.unpack(b)
        if xa == xb or ya == yb:
            return -1
        return la * self.plane + min(ya, yb) * self.nx + min(xa, xb)

    def _offset_table(self, offsets: Sequence[tuple[int, int]]):
        entry = self._offsets.get(id(offsets))
        if entry is None or entry[0] is not offsets:
            ox = np.fromiter((o[0] for o in offsets), dtype=np.int64, count=len(offsets))
            oy = np.fromiter((o[1] for o in offsets), dtype=np.int64, count=len(offsets))
            bounds = ((int(ox.min()), int(ox.max()), int(oy.min()), int(oy.max())) if len(offsets)
                      else (0, 0, 0, 0))
            entry = self._offsets[id(offsets)] = (offsets, ox, oy, oy * self.nx + ox, bounds)
        return entry

    def _offset_ids(self, ix: int, iy: int, base: int, offsets) -> np.ndarray:
        _, ox, oy, dn, (lo_x, hi_x, lo_y, hi_y) = self._offset_table(offsets)
        nx, ny = self.nx, self.ny
        if 0 <= ix + lo_x and ix + hi_x < nx and 0 <= iy + lo_y and iy + hi_y < ny:
            return dn + (base + iy * nx + ix)
        x, y = ox + ix, oy + iy
        keep = (x >= 0) & (x < nx) & (y >= 0) & (y < ny)
        return (y[keep] * nx + x[keep]) + base

    def offset_nodes(self, node: int, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """Ids of the in-bounds planar offsets (dx, dy) of ``node`` on its layer, in ``offsets`` order."""
        layer, rest = divmod(node, self.plane)
        iy, ix = divmod(rest, self.nx)
        return self._offset_ids(ix, iy, layer * self.plane, offsets)

    def offset_columns(self, column: int, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """Columns of the in-bounds planar offsets of ``column``, in ``offsets`` order."""
        iy, ix = divmod(column, self.nx)
        return self._offset_ids(ix, iy, 0, offsets)

    def columns_around(self, point: Point, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """Columns of the in-bounds offsets around ``point``'s rounded lattice
        position (which may itself lie outside the lattice)."""
        ix, iy = self._rounded(point)
        return self._offset_ids(ix, iy, 0, offsets)

    # -- search windows (inclusive index boxes) -----------------------------

    def full_box(self) -> tuple[int, int, int, int]:
        return 0, 0, self.nx - 1, self.ny - 1

    def box_of(self, nodes: Iterable[int]) -> tuple[int, int, int, int]:
        """Bounding index box of nodes (any layer); nodes must not be empty."""
        if isinstance(nodes, np.ndarray):
            rest = nodes % self.plane
            xs, ys = rest % self.nx, rest // self.nx
            return int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())
        plane, nx = self.plane, self.nx
        x0 = y0 = None
        for node in nodes:
            y, x = divmod(node % plane, nx)
            if x0 is None:
                x0 = x1 = x
                y0 = y1 = y
                continue
            if x < x0:
                x0 = x
            elif x > x1:
                x1 = x
            if y < y0:
                y0 = y
            elif y > y1:
                y1 = y
        if x0 is None:
            raise ValueError("box_of needs at least one node")
        return x0, y0, x1, y1

    def grow_box(self, box, margin: int) -> tuple[int, int, int, int]:
        """``box`` widened by ``margin`` lattice steps per side, clipped to the lattice."""
        x0, y0, x1, y1 = box
        return (max(0, x0 - margin), max(0, y0 - margin),
                min(self.nx - 1, x1 + margin), min(self.ny - 1, y1 + margin))

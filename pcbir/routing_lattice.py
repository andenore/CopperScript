"""Search lattice of the unified router (multi-resolution lattice plan, S0, S2).

Every node id, planar offset, point/node conversion, search window and
neighbour step of :mod:`pcbir.unified_router` goes through one
:class:`Lattice` object. :class:`UniformLattice` is today's single pitch over
the whole board (formerly ``unified_router._Grid``); :class:`NestedLattice`
adds fine rectangular patches at base pitch / k (planned by
:mod:`pcbir.fine_regions`) and implements the same protocol.

Node ids are dense: ``layer * plane + column``, where a *column* is the
planar (xy) id shared by every layer at one lattice point (a via site). A
lattice is a sequence of :class:`LatticeBlock` objects, each a dense regular
grid with its own origin and pitch; a block's columns are one contiguous run
of the column space (block 0 is the base lattice). The uniform lattice is one
block and keeps its dense ``(nl, ny, nx)`` shape (``nx``, ``ny``, ``plane``,
``xs``, ``ys``, ``window``) for the legality rasters and the C kernel.

Nested lattice rules (S2):

* **Shadowing.** Base nodes strictly inside a patch rectangle, and base cells
  whose centre is, are disabled (``live`` / ``live_cells``): they keep their
  ids but are never returned by ``nodes_in``, ``nearest``, ``cover`` or the
  disc and segment queries, and ``neighbours`` never leads to them.
* **Portals.** Every fine node on a patch boundary that lies on a base lattice
  point coincides with a live base node on the patch's boundary ring; the two
  are *twins* (``twins``). ``neighbours`` of either includes the other as a
  zero-length move, on every layer.
* **Units.** ``neighbours`` and ``block_moves`` give step lengths in base
  pitches (a fine orthogonal step is 1/k); index boxes (``full_box``,
  ``box_of``, ``grow_box``) are base-lattice boxes.
"""

from __future__ import annotations

from array import array
from bisect import bisect_right
from math import ceil, floor, hypot, sqrt
from typing import TYPE_CHECKING, Iterable, Protocol, Sequence, runtime_checkable

import numpy as np

from .physical import CopperLayer, PhysicalBoard, Point

if TYPE_CHECKING:
    from .fine_regions import FineRegionPlan

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


class LatticeBlock:
    """One dense regular block of a lattice: origin, pitch and shape.

    Block-local index boxes ``(ix0, iy0, ix1, iy1)`` are inclusive; a block's
    columns are ``column_offset + iy * nx + ix``. ``live`` (shared with the
    lattice, indexed by column) is set on a block whose nodes may be shadowed
    (a nested lattice's base); column queries then drop disabled columns.
    """

    __slots__ = ("index", "x0", "y0", "pitch", "factor", "nx", "ny", "plane", "column_offset",
                 "xs", "ys", "x1", "y1", "live")

    def __init__(self, index: int, x0: int, y0: int, pitch: int, factor: int, nx: int, ny: int,
                 column_offset: int, xs=None, ys=None) -> None:
        self.index, self.x0, self.y0, self.pitch, self.factor = index, x0, y0, pitch, factor
        self.nx, self.ny, self.plane, self.column_offset = nx, ny, nx * ny, column_offset
        self.xs = xs if xs is not None else x0 + pitch * np.arange(nx, dtype=np.float64)
        self.ys = ys if ys is not None else y0 + pitch * np.arange(ny, dtype=np.float64)
        self.x1 = x0 + (nx - 1) * pitch
        self.y1 = y0 + (ny - 1) * pitch
        self.live: np.ndarray | None = None

    def window(self, min_x: float, min_y: float, max_x: float, max_y: float):
        """Block index box covering a nanometre box (clipped; may be empty)."""
        p = self.pitch
        ix0 = max(0, int(floor((min_x - self.x0) / p)))
        iy0 = max(0, int(floor((min_y - self.y0) / p)))
        ix1 = min(self.nx - 1, int(ceil((max_x - self.x0) / p)))
        iy1 = min(self.ny - 1, int(ceil((max_y - self.y0) / p)))
        return ix0, iy0, ix1, iy1

    def inner_window(self, min_x: float, min_y: float, max_x: float, max_y: float):
        """Block index box of the nodes inside a nanometre box (clipped; may be empty)."""
        p = self.pitch
        ix0 = max(0, int(ceil((min_x - self.x0) / p)))
        iy0 = max(0, int(ceil((min_y - self.y0) / p)))
        ix1 = min(self.nx - 1, int(floor((max_x - self.x0) / p)))
        iy1 = min(self.ny - 1, int(floor((max_y - self.y0) / p)))
        return ix0, iy0, ix1, iy1

    def window_columns(self, window, mask=None) -> np.ndarray:
        """Live columns of a block index box (row-major), optionally where ``mask``
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
        columns = (iy * self.nx + ix).astype(np.int64) + self.column_offset
        if self.live is not None:
            columns = columns[self.live[columns]]
        return columns

    def contains(self, x: float, y: float) -> bool:
        """Whether a point lies in the block's closed rectangle."""
        return self.x0 <= x <= self.x1 and self.y0 <= y <= self.y1

    def moves(self) -> tuple[tuple[int, int, int, int, float], ...]:
        """Per move of ``MOVES``: (dx, dy, column delta, diagonal cell delta,
        length in base pitches)."""
        nx, k = self.nx, self.factor
        return tuple((dx, dy, dy * nx + dx, (0 if dy > 0 else -1) * nx + (0 if dx > 0 else -1),
                      step if k == 1 else step / k) for dx, dy, step in MOVES)


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
        # One block: the whole lattice (the nested lattice's interface).
        self.base_pitch_nm = pitch
        self.patches = ()
        self.blocks = (LatticeBlock(0, self.x0, self.y0, pitch, 1, self.nx, self.ny, 0, self.xs, self.ys),)
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


class NestedLattice:
    """A base lattice plus dense fine blocks over the patches of a fine-region plan.

    Block 0 is the :class:`UniformLattice` base over the board; each
    :class:`pcbir.fine_regions.FinePatch` adds one block over its rectangle at
    ``base_pitch // factor`` with ``(cx*k + 1) * (cy*k + 1)`` nodes per layer,
    boundary rows included. Node, column and diagonal-cell ids are the blocks'
    ids concatenated (``layer * plane + column``; ``plane`` is the total of
    all blocks' columns). See the module docstring for shadowing, portals and
    units. ``pitch`` is the base pitch; ``pitch_nm`` the finest pitch present.
    """

    def __init__(self, board: PhysicalBoard, base_pitch_nm: int, plan: "FineRegionPlan", window=None) -> None:
        if window is not None:
            raise ValueError("grid_window_nm is not supported with fine_regions")
        if plan.base_pitch_nm != base_pitch_nm:
            raise ValueError("fine-region plan was made for another base pitch")
        base = UniformLattice(board, base_pitch_nm)
        bp = base_pitch_nm
        self.base = base
        self.plan = plan
        self.patches = tuple(plan.patches)
        self.pitch = self.base_pitch_nm = bp
        self.x0, self.y0, self.nx, self.ny = base.x0, base.y0, base.nx, base.ny
        self.layers, self.nl = base.layers, base.nl
        blocks = [LatticeBlock(0, base.x0, base.y0, bp, 1, base.nx, base.ny, 0, base.xs, base.ys)]
        offset = base.plane
        rects = []
        for patch in self.patches:
            k = patch.factor
            if k < 2 or bp % k:
                raise ValueError(f"fine patch factor {k} does not divide the base pitch {bp}")
            for value, origin in ((patch.x0_nm, base.x0), (patch.x1_nm, base.x0),
                                  (patch.y0_nm, base.y0), (patch.y1_nm, base.y0)):
                if (value - origin) % bp:
                    raise ValueError("fine patch corners must lie on the base lattice")
            i0, j0 = (patch.x0_nm - base.x0) // bp, (patch.y0_nm - base.y0) // bp
            i1, j1 = (patch.x1_nm - base.x0) // bp, (patch.y1_nm - base.y0) // bp
            if not (0 <= i0 and 0 <= j0 and i1 < base.nx and j1 < base.ny):
                raise ValueError("fine patch lies outside the base lattice")
            if i1 - i0 < 2 or j1 - j0 < 2:
                raise ValueError("fine patch must span at least two base cells per axis")
            for other in rects:
                if not (i1 < other[0] or other[2] < i0 or j1 < other[1] or other[3] < j0):
                    raise ValueError("fine patches must not overlap or touch")
            rects.append((i0, j0, i1, j1))
            cx, cy = i1 - i0, j1 - j0
            block = LatticeBlock(len(blocks), patch.x0_nm, patch.y0_nm, bp // k, k, cx * k + 1, cy * k + 1, offset)
            blocks.append(block)
            offset += block.plane
        self.blocks = tuple(blocks)
        self._rects = tuple(rects)                    # base index rectangles of the patches
        self.plane = offset
        self.size = offset * self.nl
        self.pitch_nm = min(block.pitch for block in blocks)
        self._offsets_list = [block.column_offset for block in blocks]
        self._first_fine = self._offsets_list[1] if len(blocks) > 1 else offset
        # Per column: block index, board coordinates, liveness, twin column.
        self.block_of = np.zeros(offset, dtype=np.uint8)
        xs, ys = [], []
        for block in blocks:
            self.block_of[block.column_offset:block.column_offset + block.plane] = block.index
            gy, gx = np.mgrid[0:block.ny, 0:block.nx]
            xs.append(block.x0 + gx.ravel().astype(np.int64) * block.pitch)
            ys.append(block.y0 + gy.ravel().astype(np.int64) * block.pitch)
        self.column_x = np.concatenate(xs)
        self.column_y = np.concatenate(ys)
        self._px = array("q", self.column_x.tolist())
        self._py = array("q", self.column_y.tolist())
        self.live = np.ones(offset, dtype=bool)
        self.live_cells = np.ones(offset, dtype=bool)
        self.twins = np.full(offset, -1, dtype=np.int64)
        nx = base.nx
        for (i0, j0, i1, j1), block in zip(rects, blocks[1:]):
            iy, ix = np.mgrid[j0 + 1:j1, i0 + 1:i1]
            self.live[(iy * nx + ix).ravel()] = False
            iy, ix = np.mgrid[j0:j1, i0:i1]
            self.live_cells[(iy * nx + ix).ravel()] = False
            k = block.factor
            fy, fx = np.mgrid[0:block.ny:k, 0:block.nx:k]
            ring = (fx == 0) | (fx == block.nx - 1) | (fy == 0) | (fy == block.ny - 1)
            fine = block.column_offset + (fy * block.nx + fx)[ring]
            coarse = ((fy // k + j0) * nx + (fx // k + i0))[ring]
            self.twins[fine] = coarse
            self.twins[coarse] = fine
        blocks[0].live = self.live
        self.shadowed = np.flatnonzero(~self.live)
        self.shadowed_cells = np.flatnonzero(~self.live_cells)
        pairs = np.flatnonzero(self.twins[:base.plane] >= 0)
        self.portal_pairs = (pairs, self.twins[pairs])     # (base columns, fine columns)
        self._twins_list = array("q", self.twins.tolist())
        self._offsets: dict[tuple[int, int], tuple] = {}
        self._moves = tuple(block.moves() for block in blocks)
        self._search_tables = None

    # -- identity -----------------------------------------------------------

    def signature(self) -> tuple:
        """Base signature plus every patch: equal signatures share node ids."""
        return (*self.base.signature(),
                tuple((p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm, p.factor) for p in self.patches))

    @property
    def node_count(self) -> int:
        """Distinct live lattice sites, all layers (twins once, shadowed nodes not at all)."""
        return int((int(self.live.sum()) - len(self.portal_pairs[0])) * self.nl)

    def block_index(self, column: int) -> int:
        """Index of the block holding ``column``."""
        if column < self._first_fine:
            return 0
        return bisect_right(self._offsets_list, column) - 1

    def block_at(self, point: Point) -> LatticeBlock:
        """The fine block whose closed rectangle holds ``point``, else the base."""
        for block in self.blocks[1:]:
            if block.contains(point.x_nm, point.y_nm):
                return block
        return self.blocks[0]

    def step_nm(self, node: int) -> int:
        return self.blocks[self.block_index(node % self.plane)].pitch

    def column_weight(self) -> np.ndarray:
        """Per column, its area in base cells: ``(step / base pitch) ** 2``."""
        weights = np.ones(self.plane, dtype=np.float64)
        for block in self.blocks[1:]:
            weights[block.column_offset:block.column_offset + block.plane] = 1.0 / (block.factor ** 2)
        return weights

    # -- layers, columns and points ----------------------------------------

    def layer_of(self, node: int) -> int:
        return node // self.plane

    def column(self, node: int) -> int:
        return node % self.plane

    def column_nodes(self, column: int) -> tuple[int, ...]:
        plane = self.plane
        return tuple(layer * plane + column for layer in range(self.nl))

    def layer_bases(self) -> tuple[int, ...]:
        return tuple(layer * self.plane for layer in range(self.nl))

    def layer_node(self, column: int, layer: int) -> int:
        return layer * self.plane + column

    def column_point(self, column: int) -> Point:
        return Point(self._px[column], self._py[column])

    def point(self, node: int) -> Point:
        column = node % self.plane
        return Point(self._px[column], self._py[column])

    def local(self, column: int) -> tuple[LatticeBlock, int, int]:
        """(block, ix, iy) of a column."""
        block = self.blocks[self.block_index(column)]
        iy, ix = divmod(column - block.column_offset, block.nx)
        return block, ix, iy

    def decode(self, node: int) -> tuple[int, int, int, int]:
        """(layer, column, block-local ix, block-local iy)."""
        layer, column = divmod(node, self.plane)
        _, ix, iy = self.local(column)
        return layer, column, ix, iy

    def decoder(self):
        return self.decode

    def twin(self, node: int) -> int:
        """The portal twin of ``node`` (same layer), or -1."""
        layer, column = divmod(node, self.plane)
        other = self._twins_list[column]
        return -1 if other < 0 else layer * self.plane + other

    def with_twins(self, nodes) -> np.ndarray:
        """Node ids of ``nodes`` plus the twins of those that have one."""
        nodes = np.asarray(nodes, dtype=np.int64)
        if not nodes.size:
            return nodes
        columns = nodes % self.plane
        other = self.twins[columns]
        keep = other >= 0
        return np.concatenate((nodes, nodes[keep] - columns[keep] + other[keep]))

    def columns_with_twins(self, columns) -> np.ndarray:
        """Columns plus the twins of those that have one."""
        columns = np.asarray(columns, dtype=np.int64)
        if not columns.size:
            return columns
        other = self.twins[columns]
        return np.concatenate((columns, other[other >= 0]))

    # -- points to nodes ---------------------------------------------------

    def node_at(self, layer: int, point: Point) -> int | None:
        """The live node exactly at ``point`` (the fine one of a twin pair), or None."""
        if not 0 <= layer < self.nl:
            return None
        block = self.block_at(point)
        dx, dy = point.x_nm - block.x0, point.y_nm - block.y0
        if dx % block.pitch or dy % block.pitch:
            return None
        ix, iy = dx // block.pitch, dy // block.pitch
        if not (0 <= ix < block.nx and 0 <= iy < block.ny):
            return None
        return layer * self.plane + block.column_offset + iy * block.nx + ix

    @staticmethod
    def _rounded(block: LatticeBlock, point: Point) -> tuple[int, int]:
        return (int(round((point.x_nm - block.x0) / block.pitch)),
                int(round((point.y_nm - block.y0) / block.pitch)))

    def snap(self, layer: int, point: Point) -> int | None:
        """The node at the rounded position of ``point`` in its block; None off the lattice."""
        block = self.block_at(point)
        ix, iy = self._rounded(block, point)
        if not (0 <= ix < block.nx and 0 <= iy < block.ny):
            return None
        return layer * self.plane + block.column_offset + iy * block.nx + ix

    def nearest(self, layer: int, point: Point) -> int:
        """The live node nearest ``point`` on ``layer`` (rounded in its block, clamped)."""
        block = self.block_at(point)
        ix, iy = self._rounded(block, point)
        ix, iy = min(max(ix, 0), block.nx - 1), min(max(iy, 0), block.ny - 1)
        return layer * self.plane + block.column_offset + iy * block.nx + ix

    def _overlapping(self, x0, y0, x1, y1):
        return [block for block in self.blocks
                if not (x1 < block.x0 or block.x1 < x0 or y1 < block.y0 or block.y1 < y0)]

    def nodes_in(self, layer: int, x0, y0, x1, y1) -> np.ndarray:
        """Ids of the live nodes on ``layer`` with x0 <= x <= x1 and y0 <= y <= y1
        (block by block, row-major; both nodes of a twin pair)."""
        parts = [block.window_columns(block.inner_window(x0, y0, x1, y1))
                 for block in self._overlapping(x0, y0, x1, y1)]
        parts = [part for part in parts if part.size]
        if not parts:
            return np.zeros(0, dtype=np.int64)
        return np.concatenate(parts) + layer * self.plane

    def cover(self, min_x: float, min_y: float, max_x: float, max_y: float) -> list[tuple[int, int, int]]:
        """(column, x_nm, y_nm) of the live columns of every block's covering box,
        x-major. Of a twin pair only the fine column is listed."""
        blocks = self._overlapping(min_x, min_y, max_x, max_y)
        parts = [block.window_columns(block.window(min_x, min_y, max_x, max_y)) for block in blocks]
        if not parts:
            return []
        columns = np.concatenate(parts)
        if not columns.size:
            return []
        if blocks[0].index == 0 and len(parts) > 1 and parts[0].size:
            base, fine = columns[:parts[0].size], columns[parts[0].size:]
            other = self.twins[base]
            drop = (other >= 0) & np.isin(other, fine)
            columns = np.concatenate((base[~drop], fine))
        xs, ys = self.column_x[columns], self.column_y[columns]
        order = np.lexsort((ys, xs))
        return list(zip(columns[order].tolist(), xs[order].tolist(), ys[order].tolist()))

    def columns_within(self, point: Point, radius_nm: float) -> np.ndarray:
        """Live columns strictly within ``radius_nm`` of ``point`` (block by block)."""
        x, y = point.x_nm, point.y_nm
        parts = []
        for block in self._overlapping(x - radius_nm, y - radius_nm, x + radius_nm, y + radius_nm):
            columns = block.window_columns(block.inner_window(x - radius_nm, y - radius_nm,
                                                              x + radius_nm, y + radius_nm))
            if columns.size:
                d = np.hypot(self.column_x[columns] - x, self.column_y[columns] - y)
                parts.append(columns[d < radius_nm])
        return np.concatenate(parts) if parts else np.zeros(0, dtype=np.int64)

    def nodes_within(self, layer: int, point: Point, radius_nm: float) -> np.ndarray:
        """Live nodes on ``layer`` strictly within ``radius_nm`` of ``point``."""
        return self.columns_within(point, radius_nm) + layer * self.plane

    def _segment_columns(self, start: Point, end: Point, radius_of):
        ax, ay, bx, by = start.x_nm, start.y_nm, end.x_nm, end.y_nm
        reach = max(radius_of(block) for block in self.blocks)
        lo_x, lo_y, hi_x, hi_y = min(ax, bx), min(ay, by), max(ax, bx), max(ay, by)
        dx, dy = float(bx - ax), float(by - ay)
        length2 = dx * dx + dy * dy
        result = []
        for block in self._overlapping(lo_x - reach, lo_y - reach, hi_x + reach, hi_y + reach):
            r = radius_of(block)
            columns = block.window_columns(block.window(lo_x - r, lo_y - r, hi_x + r, hi_y + r))
            if not columns.size:
                continue
            px = self.column_x[columns].astype(np.float64)
            py = self.column_y[columns].astype(np.float64)
            if length2:
                t = np.clip(((px - ax) * dx + (py - ay) * dy) / length2, 0.0, 1.0)
            else:
                t = np.zeros(px.shape)
            d = np.hypot(px - (ax + t * dx), py - (ay + t * dy))
            keep = d < r
            result.append((block, columns[keep], d[keep]))
        return result

    def segment_nodes(self, layer: int, start: Point, end: Point, radius_nm: float) -> list[tuple[int, float]]:
        """(node, distance) for live nodes on ``layer`` strictly within ``radius_nm`` of a segment."""
        base = layer * self.plane
        return [(column + base, distance)
                for _, columns, distances in self._segment_columns(start, end, lambda block: radius_nm)
                for column, distance in zip(columns.tolist(), distances.tolist())]

    def segment_nodes_local(self, layer: int, start: Point, end: Point, half_steps: float = 0.5,
                            extra_nm: float = 0.0) -> list[tuple[int, float, float]]:
        """(node, distance, weight) for live nodes on ``layer`` strictly within
        ``half_steps`` of their own block's pitch (plus ``extra_nm``) of a
        segment; ``weight`` is that pitch in base pitches."""
        base = layer * self.plane
        return [(column + base, distance, block.pitch / self.pitch)
                for block, columns, distances in self._segment_columns(
                    start, end, lambda block: half_steps * block.pitch + extra_nm)
                for column, distance in zip(columns.tolist(), distances.tolist())]

    def owns(self, block: LatticeBlock, x0: float, y0: float, x1: float, y1: float) -> bool:
        """Whether a nanometre box lies wholly in ``block``'s own region: a fine
        block's closed rectangle, or (base) clear of every patch rectangle."""
        if block.index:
            return block.x0 <= x0 and x1 <= block.x1 and block.y0 <= y0 and y1 <= block.y1
        return not any(not (x1 < b.x0 or b.x1 < x0 or y1 < b.y0 or b.y1 < y0) for b in self.blocks[1:])

    # -- planar steps ------------------------------------------------------

    def block_moves(self, index: int) -> tuple[tuple[int, int, int, int, float], ...]:
        """``LatticeBlock.moves`` of block ``index`` (lengths in base pitches)."""
        return self._moves[index]

    def move_deltas(self) -> tuple[tuple[int, int, int, int, float], ...]:
        """The base block's moves (``block_moves`` gives every block's)."""
        return self._moves[0]

    def neighbours(self, node: int, diagonals: bool = True) -> tuple[tuple[int, float], ...]:
        """In-block planar moves to live nodes in ``MOVES`` order (lengths in base
        pitches), then the portal twin at length 0."""
        layer, column = divmod(node, self.plane)
        block, ix, iy = self.local(column)
        live, cells = self.live, self.live_cells
        moves = self._moves[block.index]
        result = []
        for dx, dy, dn, dcell, step in (moves if diagonals else moves[:4]):
            if not (0 <= ix + dx < block.nx and 0 <= iy + dy < block.ny):
                continue
            if not live[column + dn] or (dx and dy and not cells[column + dcell]):
                continue
            result.append((node + dn, step))
        twin = self._twins_list[column]
        if twin >= 0:
            result.append((layer * self.plane + twin, 0.0))
        return tuple(result)

    def delta(self, a: int, b: int) -> tuple[int, int]:
        """Direction (sign x, sign y) from node ``a`` to ``b``: the lattice step
        within a block, (0, 0) across a portal."""
        ca, cb = a % self.plane, b % self.plane
        dx, dy = self._px[cb] - self._px[ca], self._py[cb] - self._py[ca]
        return (dx > 0) - (dx < 0), (dy > 0) - (dy < 0)

    def diagonal_cell(self, a: int, b: int) -> int:
        """Cell id (its lower-left node) crossed by the in-block diagonal step a-b, or -1."""
        layer, ca = divmod(a, self.plane)
        block, xa, ya = self.local(ca)
        other, xb, yb = self.local(b % self.plane)
        if other is not block or xa == xb or ya == yb:
            return -1
        return layer * self.plane + block.column_offset + min(ya, yb) * block.nx + min(xa, xb)

    def _offset_table(self, block: LatticeBlock, offsets: Sequence[tuple[int, int]]):
        key = (block.index, id(offsets))
        entry = self._offsets.get(key)
        if entry is None or entry[0] is not offsets:
            ox = np.fromiter((o[0] for o in offsets), dtype=np.int64, count=len(offsets))
            oy = np.fromiter((o[1] for o in offsets), dtype=np.int64, count=len(offsets))
            bounds = ((int(ox.min()), int(ox.max()), int(oy.min()), int(oy.max())) if len(offsets)
                      else (0, 0, 0, 0))
            entry = self._offsets[key] = (offsets, ox, oy, oy * block.nx + ox, bounds)
        return entry

    def _offset_ids(self, block: LatticeBlock, ix: int, iy: int, offsets) -> np.ndarray:
        _, ox, oy, dn, (lo_x, hi_x, lo_y, hi_y) = self._offset_table(block, offsets)
        nx, ny = block.nx, block.ny
        if 0 <= ix + lo_x and ix + hi_x < nx and 0 <= iy + lo_y and iy + hi_y < ny:
            ids = dn + (block.column_offset + iy * nx + ix)
        else:
            x, y = ox + ix, oy + iy
            keep = (x >= 0) & (x < nx) & (y >= 0) & (y < ny)
            ids = (y[keep] * nx + x[keep]) + block.column_offset
        if block.live is not None:
            ids = ids[block.live[ids]]
        return ids

    def offset_columns(self, column: int, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """Live columns of the in-block offsets (in the block's own pitch) of ``column``."""
        block, ix, iy = self.local(column)
        return self._offset_ids(block, ix, iy, offsets)

    def offset_nodes(self, node: int, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """``offset_columns`` of ``node``'s column, on its layer."""
        layer, column = divmod(node, self.plane)
        return self.offset_columns(column, offsets) + layer * self.plane

    def columns_around(self, point: Point, offsets: Sequence[tuple[int, int]]) -> np.ndarray:
        """Live in-block offsets around ``point``'s rounded position in its block."""
        block = self.block_at(point)
        ix, iy = self._rounded(block, point)
        return self._offset_ids(block, ix, iy, offsets)

    # -- search windows (inclusive base-lattice index boxes) ----------------

    def full_box(self) -> tuple[int, int, int, int]:
        return 0, 0, self.nx - 1, self.ny - 1

    def box_of(self, nodes: Iterable[int]) -> tuple[int, int, int, int]:
        """Base-lattice index box covering nodes (any layer, any block); not empty."""
        if not isinstance(nodes, np.ndarray):
            nodes = np.fromiter(nodes, dtype=np.int64)
        if not nodes.size:
            raise ValueError("box_of needs at least one node")
        columns = nodes % self.plane
        xs, ys = self.column_x[columns], self.column_y[columns]
        p = self.pitch
        return (int((int(xs.min()) - self.x0) // p), int((int(ys.min()) - self.y0) // p),
                int(-((self.x0 - int(xs.max())) // p)), int(-((self.y0 - int(ys.max())) // p)))

    def grow_box(self, box, margin: int) -> tuple[int, int, int, int]:
        """``box`` widened by ``margin`` base steps per side, clipped to the lattice."""
        x0, y0, x1, y1 = box
        return (max(0, x0 - margin), max(0, y0 - margin),
                min(self.nx - 1, x1 + margin), min(self.ny - 1, y1 + margin))

    def search_tables(self):
        """(x, y) of every column in base pitches from the base origin
        (``array('d')``), the block of every column (``array('B')``) and the
        twin column of every column (``array('q')``, -1 for none)."""
        if self._search_tables is None:
            p = float(self.pitch)
            ux = array("d", ((self.column_x - self.x0) / p).tolist())
            uy = array("d", ((self.column_y - self.y0) / p).tolist())
            self._search_tables = (ux, uy, array("B", self.block_of.tobytes()), self._twins_list)
        return self._search_tables

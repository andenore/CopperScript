"""Fine-patch planning for the multi-resolution search lattice (stage S1).

The unified router searches one uniform lattice today, and ``choose_pitch``
drops the pitch for the whole board when any ball array needs nodes on its
balls. This module plans the alternative: a coarse base lattice over the board
plus rectangular fine patches at ``base_pitch // k`` around the packages that
need them. It is pure planning; nothing routes through it yet (stage S2 builds
a nested lattice from the plan).

Which packages get a patch
    Every placement whose footprint is a full ball array
    (``package_patterns.circular_array``), and every placement with at least
    ``minimum_pads`` copper pins (SMD or plated through hole; pads sharing a
    number are one pin) that each have another pin closer than
    ``pad_pitch_threshold_nm``, centre to centre.

Refinement factor (the ``choose_pitch`` rule, applied per package)
    Candidate factors are the integers ``2 <= k <= maximum_factor`` that divide
    the base pitch exactly, so every coarse node coincides with a fine node
    (for 0.25 mm: 2, 4, 5 and 8, i.e. 0.125, 0.0625, 0.05 and 0.03125 mm).
    The coarsest factor whose fine pitch ``f`` puts nodes on every ball or pad
    centre and on the channel centre lines between neighbouring pins wins:

    * ball array: ``(array_pitch // 2) % f == 0`` and the first ball's board
      coordinates are multiples of ``f``, exactly as ``choose_pitch`` tests;
    * dense package: every dense pin's pad centre (both axes) and the midpoint
      of every pad pair closer than the threshold are multiples of ``f``.

    ``k = 1`` passing (already aligned at the base pitch) means no patch. When
    no candidate aligns exactly (off-lattice or rotated placement, a pitch such
    as 0.65 mm) the fallback is the coarsest candidate ``k >= 2`` whose fine
    pitch is at most half the package's pin spacing, or ``maximum_factor``'s
    candidate when none is that fine. The fallback never yields ``k = 1``: a
    package that qualified and is not aligned always gets some refinement.

    ``choose_pitch`` tries ``base, base // 2, base // 4`` and 0.05 mm; at the
    0.25 mm base those are k = 1, 2, 4, 5, which is why 5 is a candidate here
    (a 0.4 mm array needs 0.05 mm). The two rules agree whenever
    ``choose_pitch`` finds an aligned pitch for a board with one package.

Node accounting
    ``base_nodes`` is the uniform base lattice (``_Grid`` formula) times the
    copper layers. A patch spanning ``cx`` by ``cy`` base cells with factor
    ``k`` holds ``(cx*k + 1) * (cy*k + 1)`` fine nodes per layer, its boundary
    included; ``(cx + 1) * (cy + 1)`` of them coincide with base nodes, so the
    patch adds the difference. ``fine_nodes`` is the sum over patches (patches
    never overlap or touch after merging), times the layers. Patches cover all
    copper layers for now (``layers is None``); per-layer patches are a later
    refinement.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from math import cos, gcd, radians, sin

from .physical import CopperLayer, PadKind, PadShape, PhysicalBoard, Placement, Point, nm_from_mm
from .placement import transformed_local_point
from .quantities import decimal_text

_COPPER_KINDS = (PadKind.SMD, PadKind.THROUGH_HOLE)


def _mm_text(value_nm: int) -> str:
    return decimal_text(Decimal(value_nm) / Decimal(1_000_000)) + " mm"


def _mm(value_nm: int) -> float:
    return value_nm / 1_000_000


# ---------------------------------------------------------------------------
# Public data


@dataclass(frozen=True, slots=True)
class LatticeBounds:
    """Origin and node counts of a uniform lattice, as ``_Grid`` derives them."""

    pitch_nm: int
    x0_nm: int
    y0_nm: int
    nx: int
    ny: int

    @property
    def x1_nm(self) -> int:
        """Board coordinate of the last lattice column."""
        return self.x0_nm + (self.nx - 1) * self.pitch_nm

    @property
    def y1_nm(self) -> int:
        """Board coordinate of the last lattice row."""
        return self.y0_nm + (self.ny - 1) * self.pitch_nm

    @property
    def plane(self) -> int:
        """Nodes per copper layer."""
        return self.nx * self.ny


@dataclass(frozen=True, slots=True)
class FinePatch:
    """One rectangle of the lattice refined to ``base_pitch // factor``.

    Corners lie on the base lattice, so the patch spans a whole number of base
    cells and every coarse node on its boundary is also a fine node.
    """

    x0_nm: int
    y0_nm: int
    x1_nm: int
    y1_nm: int
    factor: int
    references: tuple[str, ...]
    reason: str
    # None means every copper layer; per-layer patches are a later refinement.
    layers: tuple[CopperLayer, ...] | None = None
    # Fine nodes (all layers) inside this rectangle but outside the union of
    # the package rectangles merged into it: the price of one bounding box.
    waste_nodes: int = 0

    def pitch_nm(self, base_pitch_nm: int) -> int:
        """Fine pitch of this patch."""
        return base_pitch_nm // self.factor

    def cells(self, base_pitch_nm: int) -> tuple[int, int]:
        """Base cells spanned along x and y."""
        return ((self.x1_nm - self.x0_nm) // base_pitch_nm, (self.y1_nm - self.y0_nm) // base_pitch_nm)

    def extra_nodes_per_layer(self, base_pitch_nm: int) -> int:
        """Fine nodes minus the coincident base nodes they replace, one layer."""
        cx, cy = self.cells(base_pitch_nm)
        k = self.factor
        return (cx * k + 1) * (cy * k + 1) - (cx + 1) * (cy + 1)


@dataclass(frozen=True, slots=True)
class FineRegionOptions:
    """Knobs of the fine-patch planner."""

    pad_pitch_threshold_nm: int = nm_from_mm("0.65")
    margin_nm: int = nm_from_mm("1.5")
    maximum_factor: int = 8
    merge_gap_nm: int = nm_from_mm("0.5")
    # Pad-field area (bounding box of the pads' copper, before the margin)
    # below which a package never gets a patch: an 0402 pair is about 0.8 mm2.
    minimum_area_nm2: int = nm_from_mm(1) * nm_from_mm(1)
    minimum_pads: int = 8
    enabled: bool = True

    def __post_init__(self) -> None:
        if self.pad_pitch_threshold_nm <= 0:
            raise ValueError("fine-region pad pitch threshold must be positive")
        if self.margin_nm < 0 or self.merge_gap_nm < 0 or self.minimum_area_nm2 < 0:
            raise ValueError("fine-region margin, merge gap and minimum area cannot be negative")
        if self.maximum_factor < 2:
            raise ValueError("fine-region maximum factor must be at least 2")
        if self.minimum_pads < 2:
            raise ValueError("fine-region minimum pad count must be at least 2")


@dataclass(frozen=True, slots=True)
class FineRegionPlan:
    """Fine patches for one board and base pitch, with their node cost."""

    base_pitch_nm: int
    patches: tuple[FinePatch, ...]
    base_nodes: int
    fine_nodes: int
    uniform_nodes_at_finest: int
    layer_count: int = 0
    # (reference, reason) for candidate packages that got no patch.
    skipped: tuple[tuple[str, str], ...] = ()

    @property
    def finest_pitch_nm(self) -> int:
        """Finest pitch present (the base pitch when there are no patches)."""
        return self.base_pitch_nm // max((p.factor for p in self.patches), default=1)

    @property
    def total_nodes(self) -> int:
        """Nodes of the nested lattice: base plus what the patches add."""
        return self.base_nodes + self.fine_nodes

    def report(self) -> dict:
        """JSON-ready summary: node counts and one row per patch, in mm."""
        total = self.total_nodes
        return {
            "base_pitch_mm": _mm(self.base_pitch_nm),
            "finest_pitch_mm": _mm(self.finest_pitch_nm),
            "layers": int(self.layer_count),
            "base_nodes": int(self.base_nodes),
            "fine_nodes": int(self.fine_nodes),
            "total_nodes": int(total),
            "uniform_nodes_at_finest": int(self.uniform_nodes_at_finest),
            "uniform_to_nested_ratio": round(self.uniform_nodes_at_finest / total, 3) if total else 0.0,
            "patches": [{
                "references": list(p.references),
                "reason": p.reason,
                "factor": int(p.factor),
                "pitch_mm": _mm(p.pitch_nm(self.base_pitch_nm)),
                "x0_mm": _mm(p.x0_nm), "y0_mm": _mm(p.y0_nm),
                "x1_mm": _mm(p.x1_nm), "y1_mm": _mm(p.y1_nm),
                "width_mm": _mm(p.x1_nm - p.x0_nm), "height_mm": _mm(p.y1_nm - p.y0_nm),
                "layers": "all" if p.layers is None else [layer.value for layer in p.layers],
                "extra_nodes": int(p.extra_nodes_per_layer(self.base_pitch_nm) * self.layer_count),
                "waste_nodes": int(p.waste_nodes),
            } for p in self.patches],
            "skipped": [{"reference": ref, "reason": why} for ref, why in self.skipped],
        }


# ---------------------------------------------------------------------------
# Lattice geometry


def base_lattice_bounds(board: PhysicalBoard, base_pitch_nm: int) -> LatticeBounds:
    """Uniform lattice over the outline: ``_Grid``'s origin and node counts."""
    if base_pitch_nm <= 0:
        raise ValueError("lattice pitch must be positive")
    vertices = board.outline.vertices
    min_x = min(p.x_nm for p in vertices)
    min_y = min(p.y_nm for p in vertices)
    max_x = max(p.x_nm for p in vertices)
    max_y = max(p.y_nm for p in vertices)
    x0 = (min_x // base_pitch_nm) * base_pitch_nm
    y0 = (min_y // base_pitch_nm) * base_pitch_nm
    return LatticeBounds(base_pitch_nm, x0, y0,
                         (max_x - x0) // base_pitch_nm + 2, (max_y - y0) // base_pitch_nm + 2)


def candidate_factors(base_pitch_nm: int, maximum_factor: int) -> tuple[int, ...]:
    """Refinement factors k >= 2 that divide the base pitch, coarsest first."""
    return tuple(k for k in range(2, maximum_factor + 1) if base_pitch_nm % k == 0)


# ---------------------------------------------------------------------------
# Package analysis


@dataclass(frozen=True, slots=True)
class _Spacing:
    """Footprint-local pin spacing (rotation and mirror invariant)."""

    spacing_nm: int                         # minimum centre distance between dense pins
    dense_pads: tuple[int, ...]             # pad indices of pins with a close neighbour
    pairs: tuple[tuple[int, int], ...]      # pad index pairs closer than the threshold
    pin_count: int                          # dense pins


def _local_spacing(footprint, threshold_nm: int) -> _Spacing | None:
    pads = [(i, pad) for i, pad in enumerate(footprint.pads) if pad.kind in _COPPER_KINDS]
    if len(pads) < 2:
        return None
    # Bucket pads by threshold-sized cells; only neighbouring buckets can hold
    # a pair closer than the threshold.
    buckets: dict[tuple[int, int], list[int]] = {}
    for i, pad in pads:
        key = (pad.position.x_nm // threshold_nm, pad.position.y_nm // threshold_nm)
        buckets.setdefault(key, []).append(i)
    limit = threshold_nm * threshold_nm
    pairs, dense, best = [], set(), None
    for (bx, by), members in sorted(buckets.items()):
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for j in buckets.get((bx + dx, by + dy), ()):
                    for i in members:
                        if j <= i:
                            continue
                        a, b = footprint.pads[i], footprint.pads[j]
                        if a.number == b.number:
                            continue
                        d2 = (a.position.x_nm - b.position.x_nm) ** 2 + (a.position.y_nm - b.position.y_nm) ** 2
                        if d2 >= limit:
                            continue
                        pairs.append((i, j))
                        dense.update((i, j))
                        best = d2 if best is None else min(best, d2)
    if best is None:
        return None
    pins = {footprint.pads[i].number for i in dense}
    return _Spacing(round(best ** 0.5), tuple(sorted(dense)), tuple(sorted(pairs)), len(pins))


def _pad_extent(pose: Placement, pad) -> tuple[int, int, int, int]:
    """Board-frame bounding box of one pad's copper."""
    if pad.shape is PadShape.CIRCLE:
        centre = transformed_local_point(pose, pad.position)
        r = max(pad.size.width_nm, pad.size.height_nm) // 2
        return centre.x_nm - r, centre.y_nm - r, centre.x_nm + r, centre.y_nm + r
    hw, hh = pad.size.width_nm / 2, pad.size.height_nm / 2
    angle = radians(-float(pad.rotation_degrees))
    c, s = cos(angle), sin(angle)
    xs, ys = [], []
    for ox, oy in ((-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)):
        local = Point(pad.position.x_nm + round(ox * c - oy * s), pad.position.y_nm + round(ox * s + oy * c))
        point = transformed_local_point(pose, local)
        xs.append(point.x_nm)
        ys.append(point.y_nm)
    return min(xs), min(ys), max(xs), max(ys)


def _array_points(array, pose: Placement) -> tuple[dict[int, Point], tuple[tuple[int, int], ...]]:
    """Every ball of a full array in board coordinates, and its adjacent pairs.

    The alignment test must see the placed array, not its first pad and its
    unrotated pitch: a rotated array can land one ball on the lattice while
    every other ball and every channel centre line is off it.
    """
    points: dict[int, Point] = {}
    pairs: list[tuple[int, int]] = []
    columns, rows = len(array.xs_nm), len(array.ys_nm)
    for row, y in enumerate(array.ys_nm):
        for column, x in enumerate(array.xs_nm):
            index = row * columns + column
            points[index] = transformed_local_point(pose, Point(x, y))
            if column:
                pairs.append((index - 1, index))
            if row:
                pairs.append((index - columns, index))
    return points, tuple(pairs)


def _dense_aligned(points: dict[int, Point], pairs, fine: int) -> bool:
    if any(p.x_nm % fine or p.y_nm % fine for p in points.values()):
        return False
    double = 2 * fine
    for i, j in pairs:
        a, b = points[i], points[j]
        if (a.x_nm + b.x_nm) % double or (a.y_nm + b.y_nm) % double:
            return False
    return True


@dataclass(frozen=True, slots=True)
class _Need:
    reference: str
    factor: int
    reason: str
    box: tuple[int, int, int, int]      # pad-field copper extents, board frame


def _package_need(board: PhysicalBoard, pose: Placement, base: int, options: FineRegionOptions,
                  spacing_cache: dict) -> tuple[_Need | None, str | None]:
    """(need, None) for a package needing a patch, (None, why) for a candidate
    that does not, (None, None) for a non-candidate."""
    from .package_patterns import circular_array
    footprint = board.footprints[pose.footprint]
    try:
        array = circular_array(board, pose.reference)
    except ValueError:
        array = None
    if array is not None:
        spacing = array.pitch_nm
        balls, adjacent = _array_points(array, pose)
        aligned = lambda fine: _dense_aligned(balls, adjacent, fine)  # noqa: E731
        kind = f"{len(array.xs_nm)}x{len(array.ys_nm)} ball array at {_mm_text(spacing)}"
    else:
        key = (pose.footprint, options.pad_pitch_threshold_nm)
        if key not in spacing_cache:
            spacing_cache[key] = _local_spacing(footprint, options.pad_pitch_threshold_nm)
        local = spacing_cache[key]
        if local is None or local.pin_count < options.minimum_pads:
            return None, None
        spacing = local.spacing_nm
        points = {i: transformed_local_point(pose, footprint.pads[i].position) for i in local.dense_pads}
        aligned = lambda fine: _dense_aligned(points, local.pairs, fine)  # noqa: E731
        kind = f"{local.pin_count} pins at {_mm_text(spacing)}"
    extents = [_pad_extent(pose, pad) for pad in footprint.pads if pad.kind in _COPPER_KINDS]
    box = (min(e[0] for e in extents), min(e[1] for e in extents),
           max(e[2] for e in extents), max(e[3] for e in extents))
    if (box[2] - box[0]) * (box[3] - box[1]) < options.minimum_area_nm2:
        return None, f"{kind}: pad field below the minimum area"
    if aligned(base):
        return None, f"{kind}: aligned at the base pitch"
    factors = candidate_factors(base, options.maximum_factor)
    if not factors:
        return None, f"{kind}: no factor up to {options.maximum_factor} divides the base pitch"
    for k in factors:
        if aligned(base // k):
            return _Need(pose.reference, k, f"{kind}: k={k} puts nodes on pin centres and channel centre lines",
                         box), None
    k = next((k for k in factors if 2 * (base // k) <= spacing), factors[-1])
    return _Need(pose.reference, k, f"{kind}: no exact k up to {options.maximum_factor}; "
                 f"fallback k={k}, pitch {_mm_text(base // k)} at most half the spacing"
                 if 2 * (base // k) <= spacing else
                 f"{kind}: no exact k up to {options.maximum_factor}; fallback to the finest k={k}", box), None


# ---------------------------------------------------------------------------
# Rectangles


def _snap_out(box, lattice: LatticeBounds, clip) -> tuple[int, int, int, int] | None:
    p = lattice.pitch_nm
    x0 = lattice.x0_nm + ((box[0] - lattice.x0_nm) // p) * p
    y0 = lattice.y0_nm + ((box[1] - lattice.y0_nm) // p) * p
    x1 = lattice.x0_nm - ((lattice.x0_nm - box[2]) // p) * p
    y1 = lattice.y0_nm - ((lattice.y0_nm - box[3]) // p) * p
    x0, y0, x1, y1 = max(x0, clip[0]), max(y0, clip[1]), min(x1, clip[2]), min(y1, clip[3])
    if x1 <= x0 or y1 <= y0:
        return None
    return x0, y0, x1, y1


def _gap(a, b) -> int:
    """Chebyshev distance between two closed rectangles (0 when they touch or overlap)."""
    return max(0, b[0] - a[2], a[0] - b[2], b[1] - a[3], a[1] - b[3])


def _union_cells(rects, pitch: int) -> int:
    """Base cells covered by the union of lattice-aligned rectangles."""
    xs = sorted({v for r in rects for v in (r[0], r[2])})
    ys = sorted({v for r in rects for v in (r[1], r[3])})
    cells = 0
    for xa, xb in zip(xs, xs[1:]):
        for ya, yb in zip(ys, ys[1:]):
            if any(r[0] <= xa and xb <= r[2] and r[1] <= ya and yb <= r[3] for r in rects):
                cells += ((xb - xa) // pitch) * ((yb - ya) // pitch)
    return cells


@dataclass(slots=True)
class _Group:
    rect: tuple[int, int, int, int]
    needs: list[tuple[_Need, tuple[int, int, int, int]]]


def _merge(groups: list[_Group], gap_nm: int) -> list[_Group]:
    groups = list(groups)
    changed = True
    while changed:
        changed = False
        for i in range(len(groups)):
            for j in range(i + 1, len(groups)):
                a, b = groups[i].rect, groups[j].rect
                if _gap(a, b) <= gap_nm:
                    groups[i] = _Group((min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3])),
                                       groups[i].needs + groups[j].needs)
                    del groups[j]
                    changed = True
                    break
            if changed:
                break
    return groups


def _group_factor(needs, base: int, maximum: int) -> tuple[int, str]:
    factors = sorted({need.factor for need, _ in needs})
    lcm = 1
    for k in factors:
        lcm = lcm * k // gcd(lcm, k)
    if lcm <= maximum and base % lcm == 0:
        return lcm, ""
    k = factors[-1]
    lost = sorted(need.reference for need, _ in needs if k % need.factor)
    return k, (f"; lcm {lcm} exceeds the maximum factor or the base pitch, using k={k}: "
               f"{', '.join(lost)} lose exact alignment")


# ---------------------------------------------------------------------------
# Planner


def plan_fine_regions(board: PhysicalBoard, base_pitch_nm: int,
                      options: FineRegionOptions | None = None) -> FineRegionPlan:
    """Plan fine lattice patches around the packages whose pins need them."""
    options = options or FineRegionOptions()
    lattice = base_lattice_bounds(board, base_pitch_nm)
    layers = len(board.stackup.copper_layers)
    base_nodes = lattice.plane * layers
    if not options.enabled:
        return FineRegionPlan(base_pitch_nm, (), base_nodes, 0, base_nodes, layers)
    vertices = board.outline.vertices
    p = base_pitch_nm
    # Outline bounding box, snapped outward to the base lattice.
    clip = (lattice.x0_nm, lattice.y0_nm,
            lattice.x0_nm - ((lattice.x0_nm - max(v.x_nm for v in vertices)) // p) * p,
            lattice.y0_nm - ((lattice.y0_nm - max(v.y_nm for v in vertices)) // p) * p)
    groups: list[_Group] = []
    skipped: list[tuple[str, str]] = []
    cache: dict = {}
    m = options.margin_nm
    for pose in sorted(board.placements, key=lambda item: item.reference):
        need, why = _package_need(board, pose, base_pitch_nm, options, cache)
        if need is None:
            if why is not None:
                skipped.append((pose.reference, why))
            continue
        rect = _snap_out((need.box[0] - m, need.box[1] - m, need.box[2] + m, need.box[3] + m), lattice, clip)
        if rect is None:
            skipped.append((pose.reference, "outside the board outline"))
            continue
        groups.append(_Group(rect, [(need, rect)]))
    patches = []
    for group in _merge(groups, options.merge_gap_nm):
        k, note = _group_factor(group.needs, base_pitch_nm, options.maximum_factor)
        rects = [rect for _, rect in group.needs]
        x0, y0, x1, y1 = group.rect
        cells = ((x1 - x0) // p) * ((y1 - y0) // p)
        waste = (cells - _union_cells(rects, p)) * k * k * layers
        references = tuple(sorted(need.reference for need, _ in group.needs))
        reasons = "; ".join(f"{need.reference}: {need.reason}"
                            for need, _ in sorted(group.needs, key=lambda item: item[0].reference))
        if len(group.needs) > 1:
            reasons = f"merged {len(group.needs)} packages within {_mm_text(options.merge_gap_nm)}; " + reasons
        patches.append(FinePatch(x0, y0, x1, y1, k, references, reasons + note, None, waste))
    patches.sort(key=lambda patch: patch.references)
    fine_nodes = sum(patch.extra_nodes_per_layer(p) for patch in patches) * layers
    finest = max((patch.factor for patch in patches), default=1)
    uniform = base_lattice_bounds(board, p // finest).plane * layers if finest > 1 else base_nodes
    return FineRegionPlan(p, tuple(patches), base_nodes, fine_nodes, uniform, layers, tuple(skipped))


__all__ = [
    "FinePatch",
    "FineRegionOptions",
    "FineRegionPlan",
    "LatticeBounds",
    "base_lattice_bounds",
    "candidate_factors",
    "plan_fine_regions",
]

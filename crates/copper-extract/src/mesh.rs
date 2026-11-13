//! From layout to network: a rectangular filament mesh of every copper layer.
//!
//! **Partition.** Each layer is cut into rectangles (a k-d tree: a rectangle is cut in the middle, snapped to a
//! copper edge nearby, while it is larger than the size asked for at its place; terminal regions and vias are cut
//! out of the leaves they reach). The size asked for at a place is the smallest of the requests round it, each
//! `request + (growth - 1) * distance`, and at most `coarse_nm`: a narrow shape of the loop is asked to be cut into at
//! least `across` cells over its width (a thin shape that is not along an axis by its width, about `2 area /
//! perimeter`); other narrow copper at `cell_nm`; the layout's hints at their pitch; a terminal at a third of its
//! longer side whatever the pitch; a via at the size of its own diameter. Only the pitches (`cell_nm`, `coarse_nm`,
//! the hints) are scaled by `scale` and by the budget's coarsening: the resolution of terminals, vias and the
//! narrow copper of the loop is a property of the board, not of the pitch. Fine places are not extended into rows
//! and columns across the layer as a tensor grid would: the filaments go where the requests are.
//!
//! **Filaments.** Nodes are the corners of the rectangles. A filament joins two neighbouring nodes along a side of
//! a rectangle (a rectangle next to two smaller ones has their corners as hanging nodes on its side, so the
//! filaments there follow the finer side). Its cross-section is the strip between the half-way lines to the
//! rectangles on either side, so the strips tile the copper: a bar whose edges lie on cell edges is tiled exactly
//! and has exactly `rho l / (w t)` resistance. The width of a filament is the strip width times the fraction of its
//! body that the layer's copper of the net covers; a filament whose body is covered by less than `min_coverage` is
//! dropped, and so is one whose axis is not followed by the copper along `MIN_AXIS_COVERAGE` of its length (it
//! would bridge a gap in the copper: the end of a trace, the port gap). Filaments of different nets never share a
//! node. Layers couple through the partial inductance matrix and are joined only by vias.
//!
//! **Vias** join the node nearest to the via position on every layer they pass where the net has copper within
//! `2 coarse_nm`, in a chain of barrel segments whose length is the height difference of the layers.
//!
//! **Budget.** The inductance matrix is dense, so the number of filaments is bounded (`max_filaments`). When the
//! mesh is larger and `coarsen` is on, `cell_nm` and `coarse_nm` are multiplied by 1.3 until it fits (at most ten
//! times, the flag `coarsened` says so); when it is off the mesh is refused.

use std::collections::BTreeMap;

use copper_cam::poly::{self, Shapes};

use crate::layout::{Layout, Terminal};
use crate::network::{Branch, BranchKind, Network, NodePort};
use crate::partial::{self, Axis, Bar};

/// Plating thickness of a via barrel when the board does not state one, metres.
pub const DEFAULT_PLATING_M: f64 = 25e-6;

/// What the caller may choose.
#[derive(Clone, Debug)]
pub struct MeshSpec {
    /// Largest cell in narrow features that are not on the loop, nm (the loop's own narrow copper, terminals and
    /// vias are cut finer whatever this is).
    pub cell_nm: i64,
    /// Pitch of wide copper, nm.
    pub coarse_nm: i64,
    /// Growth of the cell size per unit distance from a fine place, plus one (2: the size grows by the distance).
    pub growth: f64,
    /// A shape with an extent of at most this many nominal `cell_nm` in an axis is narrow in that axis.
    pub fine_extent_cells: f64,
    /// A narrow shape is cut into at least this many cells across its width (cells no larger than `cell_nm`).
    pub across: usize,
    /// Parallel filaments across the copper thickness (1 = uniform current over the thickness).
    pub thickness_filaments: usize,
    /// The most filaments the dense inductance matrix is allowed.
    pub max_filaments: usize,
    /// Coarsen to fit the budget instead of refusing.
    pub coarsen: bool,
    /// Least copper coverage of a filament body that makes a filament.
    pub min_coverage: f64,
    /// Multiplier of every pitch, `cell_nm`, `coarse_nm` and the layout's hints alike (1 = as given); a refinement
    /// series divides it.
    pub scale: f64,
}

impl Default for MeshSpec {
    fn default() -> Self {
        MeshSpec {
            cell_nm: 200_000,
            coarse_nm: 1_000_000,
            growth: 2.0,
            fine_extent_cells: 8.0,
            across: 2,
            thickness_filaments: 1,
            max_filaments: DEFAULT_MAX_FILAMENTS,
            coarsen: true,
            min_coverage: 0.05,
            scale: 1.0,
        }
    }
}

/// The default filament budget: the dense inductance matrix is `n^2` doubles and one frequency costs a dense complex
/// elimination of about `n/2` loops (about 25 s at 6700 filaments with the rows shared between threads, 12 cores).
pub const DEFAULT_MAX_FILAMENTS: usize = 9000;

impl MeshSpec {
    /// The same spec with all pitches (hints included) divided by `factor`: a finer mesh for a refinement series.
    pub fn refined(&self, factor: f64) -> MeshSpec {
        MeshSpec { scale: self.scale / factor, ..self.clone() }
    }
}

/// The nodes of each `(layer, net)` as `(x_nm, y_nm, node)`.
pub type LayerNodes = BTreeMap<(usize, String), Vec<(i64, i64, usize)>>;

/// A mesh node, for maps and reports.
#[derive(Clone, Debug)]
pub struct MeshNode {
    pub layer: usize,
    pub net: String,
    /// Position, metres.
    pub position: [f64; 2],
}

/// Per-layer mesh figures.
#[derive(Clone, Debug, Default)]
pub struct LayerStats {
    pub name: String,
    /// Cells of the layer's partition.
    pub cells: usize,
    pub lines_x: usize,
    pub lines_y: usize,
    pub bars: usize,
    pub smallest_cell_nm: i64,
    pub largest_cell_nm: i64,
}

/// What the mesh was.
#[derive(Clone, Debug, Default)]
pub struct MeshStats {
    pub filaments: usize,
    pub bars: usize,
    pub nodes: usize,
    pub barrels: usize,
    pub layers: Vec<LayerStats>,
    pub cell_nm: i64,
    pub coarse_nm: i64,
    /// The pitch multiplier in force (the spec's times the automatic coarsening).
    pub scale: f64,
    pub coarsen_steps: usize,
    /// Filament bodies dropped for too little copper.
    pub dropped_bars: usize,
    pub flags: Vec<String>,
    /// Terminals with no node inside their region, joined by the nearest node instead.
    pub terminals_snapped: Vec<String>,
    /// Vias that joined fewer than two layers.
    pub vias_unattached: Vec<String>,
}

/// The mesh: the network and the bookkeeping to read it.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub network: Network,
    pub nodes: Vec<MeshNode>,
    pub stats: MeshStats,
    pub port_names: Vec<String>,
    /// Network nodes of each terminal of the layout.
    pub terminal_nodes: Vec<Vec<usize>>,
    /// The nodes of each `(layer, net)` as `(x_nm, y_nm, node)`.
    pub layer_nodes: LayerNodes,
}

/// Why a mesh could not be built.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshError {
    /// Larger than the budget with coarsening off or exhausted.
    OverBudget {
        filaments: usize,
        limit: usize,
        detail: String,
    },
    /// A terminal or port refers to copper that does not exist.
    NoCopper(String),
    Invalid(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::OverBudget { filaments, limit, detail } => {
                write!(f, "the mesh needs {filaments} filaments, over the budget of {limit} ({detail})")
            }
            MeshError::NoCopper(m) | MeshError::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for MeshError {}

const NM: f64 = 1e-9;

/// Lines closer than this never coexist (terminal and via lines are otherwise kept whatever the pitch), nm.
const ABS_GAP_NM: i64 = 25_000;

/// The least share of a filament's length that the copper must span along its axis.
const MIN_AXIS_COVERAGE: f64 = 0.25;

fn rect_shapes(r: [i64; 4]) -> Shapes {
    poly::polygon(&[
        (r[0] as f64, r[1] as f64),
        (r[2] as f64, r[1] as f64),
        (r[2] as f64, r[3] as f64),
        (r[0] as f64, r[3] as f64),
    ])
}

/// The part of `strip` inside the rectangle `r`: the covered fraction of its area and the bounding box
/// `[x0, y0, x1, y1]` of the covered part (`None` when nothing is covered).
fn covered(r: [i64; 4], strip: &Shapes) -> (f64, Option<[i64; 4]>) {
    let rect_area = ((r[2] - r[0]) as f64) * ((r[3] - r[1]) as f64);
    let mut total = 0.0;
    let mut bbox: Option<[i64; 4]> = None;
    let mut grow = |b: [i64; 4]| {
        bbox = Some(match bbox {
            None => b,
            Some(o) => [o[0].min(b[0]), o[1].min(b[1]), o[2].max(b[2]), o[3].max(b[3])],
        });
    };
    let mut rect: Option<Shapes> = None;
    for shape in strip {
        let (bx0, by0, bx1, by1) = poly::bounds(shape);
        if bx0 >= r[2] || bx1 <= r[0] || by0 >= r[3] || by1 <= r[1] {
            continue;
        }
        let outer = &shape[0];
        let axis_rect = shape.len() == 1
            && outer.len() == 4
            && outer.iter().all(|p| (p.x == bx0 || p.x == bx1) && (p.y == by0 || p.y == by1));
        if axis_rect {
            let o = [bx0.max(r[0]), by0.max(r[1]), bx1.min(r[2]), by1.min(r[3])];
            total += ((o[2] - o[0]) as f64) * ((o[3] - o[1]) as f64);
            grow(o);
        } else {
            let rect = rect.get_or_insert_with(|| rect_shapes(r));
            let inter = poly::intersection(rect, &vec![shape.clone()]);
            for piece in &inter {
                total += poly::shape_area(piece);
                let b = poly::bounds(piece);
                grow([b.0, b.1, b.2, b.3]);
            }
        }
    }
    ((total / rect_area).clamp(0.0, 1.0), bbox)
}

/// A request for small cells: inside `rect` (and growing with the distance from it) the cell size must not exceed
/// `sx` in x and `sy` in y (0 = no request on that axis), nm.
#[derive(Clone, Copy, Debug)]
struct Source {
    rect: [i64; 4],
    sx: i64,
    sy: i64,
    /// Growth of the size per unit distance, plus one (`growth` of the spec for most sources).
    growth: f64,
}

/// Distance between two rectangles (0 where they touch or overlap), nm.
fn rect_distance(a: [i64; 4], b: [i64; 4]) -> f64 {
    let dx = (a[0] - b[2]).max(b[0] - a[2]).max(0) as f64;
    let dy = (a[1] - b[3]).max(b[1] - a[3]).max(0) as f64;
    dx.hypot(dy)
}

/// What the partition of one layer is made from.
struct Requests {
    sources: Vec<Source>,
    /// Rectangles (terminal regions, via points) whose edges must be cell boundaries where they cross a cell.
    hard: Vec<[i64; 4]>,
    /// Outline rectangles of the copper: split positions snap to their edges.
    soft: Vec<[i64; 4]>,
}

/// Cut the window into rectangles (a k-d tree): a cell is cut in the middle (snapped to a copper edge nearby) while
/// it is larger than the size asked for at its place, `size = request + (growth - 1) * distance` for every source and
/// at most `coarse`; a hard edge inside a cell is always a cut. Cells never get narrower than `abs_gap`.
fn partition(window: [i64; 4], req: &Requests, coarse: i64, abs_gap: i64) -> Vec<[i64; 4]> {
    let mut leaves = Vec::new();
    let mut stack = vec![window];
    while let Some(c) = stack.pop() {
        let (w, h) = (c[2] - c[0], c[3] - c[1]);
        let (mut sx, mut sy) = (coarse as f64, coarse as f64);
        for s in &req.sources {
            let d = rect_distance(s.rect, c);
            if s.sx > 0 {
                sx = sx.min(s.sx as f64 + (s.growth - 1.0) * d);
            }
            if s.sy > 0 {
                sy = sy.min(s.sy as f64 + (s.growth - 1.0) * d);
            }
        }
        let (rx, ry) = (w as f64 / sx, h as f64 / sy);
        let can_x = w >= 2 * abs_gap && rx > 1.0;
        let can_y = h >= 2 * abs_gap && ry > 1.0;
        if can_x && can_y && (rx - ry).abs() <= 1e-9 * rx.max(ry) {
            // Equally too large in both axes: cut both at once, so the result does not depend on which axis is first.
            let (mx, my) = (c[0] + w / 2, c[1] + h / 2);
            stack.push([c[0], c[1], mx, my]);
            stack.push([mx, c[1], c[2], my]);
            stack.push([c[0], my, mx, c[3]]);
            stack.push([mx, my, c[2], c[3]]);
            continue;
        }
        let axis = match (can_x, can_y) {
            (true, true) => usize::from(ry > rx),
            (true, false) => 0,
            (false, true) => 1,
            (false, false) => {
                leaves.push(c);
                continue;
            }
        };
        let (lo, hi) = (c[axis], c[axis + 2]);
        let span = hi - lo;
        let mid = lo + span / 2;
        // Snap to the nearest copper edge in the middle half of the span.
        let mut v = mid;
        let mut best = (span / 4) as f64;
        let mut tie = false;
        let other = 1 - axis;
        for r in &req.soft {
            if r[other] >= c[other + 2] || r[other + 2] <= c[other] {
                continue;
            }
            for e in [r[axis], r[axis + 2]] {
                let d = (e - mid).abs() as f64;
                if e - lo < abs_gap || hi - e < abs_gap {
                    continue;
                }
                if d < best {
                    best = d;
                    v = e;
                    tie = false;
                } else if d == best && e != v {
                    // Two edges the same distance from the middle (a mirror image): cut in the middle.
                    tie = true;
                }
            }
        }
        if tie {
            v = mid;
        }
        if axis == 0 {
            stack.push([v, c[1], c[2], c[3]]);
            stack.push([c[0], c[1], v, c[3]]);
        } else {
            stack.push([c[0], v, c[2], c[3]]);
            stack.push([c[0], c[1], c[2], v]);
        }
    }
    // Hard edges (terminal regions, via points) cut every leaf they reach, all at once, so that the result does not
    // depend on the order of the cuts (a mirror-symmetric layout gets a mirror-symmetric partition).
    let mut cut: Vec<[i64; 4]> = Vec::with_capacity(leaves.len());
    for c in leaves {
        let lines = |axis: usize| -> Vec<i64> {
            let (lo, hi) = (c[axis], c[axis + 2]);
            let other = 1 - axis;
            let mut inside: Vec<i64> = Vec::new();
            for r in &req.hard {
                if r[other] > c[other + 2] || r[other + 2] < c[other] || r[axis] > hi || r[axis + 2] < lo {
                    continue;
                }
                for v in [r[axis], r[axis + 2]] {
                    if v - lo >= abs_gap && hi - v >= abs_gap {
                        inside.push(v);
                    }
                }
            }
            inside.sort_unstable();
            let mut merged: Vec<i64> = Vec::new();
            for v in inside {
                if merged.last().is_none_or(|l| v - *l >= abs_gap) {
                    merged.push(v);
                }
            }
            let mut all = vec![lo];
            all.extend(merged);
            all.push(hi);
            all
        };
        let (xs, ys) = (lines(0), lines(1));
        for yw in ys.windows(2) {
            for xw in xs.windows(2) {
                cut.push([xw[0], yw[0], xw[1], yw[1]]);
            }
        }
    }
    cut.sort_unstable();
    cut
}

/// One side of a cell on a grid line: the interval along the line and the cell's size across it.
#[derive(Clone, Copy)]
struct Side {
    from: i64,
    to: i64,
    across: i64,
}

/// The cells' sides on every grid line of one orientation: `above` the line (the cell's lower side) and `below`.
#[derive(Default)]
struct LineSides {
    above: Vec<Side>,
    below: Vec<Side>,
}

/// Segments between consecutive nodes of every grid line: `(line, from, to, across_below, across_above)`. Nodes on a
/// line are the corners of the cells that touch it (a cell next to two smaller ones has their corners on its side as
/// hanging nodes), so the filaments on a boundary between cells of different sizes follow the finer side.
fn segments(cells: &[[i64; 4]], horizontal: bool) -> Vec<(i64, i64, i64, i64, i64)> {
    let mut lines: BTreeMap<i64, LineSides> = BTreeMap::new();
    for c in cells {
        let (a0, a1, b0, b1) = if horizontal { (c[0], c[2], c[1], c[3]) } else { (c[1], c[3], c[0], c[2]) };
        lines.entry(b0).or_default().above.push(Side { from: a0, to: a1, across: b1 - b0 });
        lines.entry(b1).or_default().below.push(Side { from: a0, to: a1, across: b1 - b0 });
    }
    let mut out = Vec::new();
    for (line, mut sides) in lines {
        sides.above.sort_unstable_by_key(|s| s.from);
        sides.below.sort_unstable_by_key(|s| s.from);
        let mut stops: Vec<i64> = sides.above.iter().chain(sides.below.iter()).flat_map(|s| [s.from, s.to]).collect();
        stops.sort_unstable();
        stops.dedup();
        let find = |list: &[Side], p: i64, q: i64| -> i64 {
            let at = list.partition_point(|s| s.to <= p);
            list.get(at).filter(|s| s.from <= p && s.to >= q).map_or(0, |s| s.across)
        };
        for w in stops.windows(2) {
            let (p, q) = (w[0], w[1]);
            let (ab, be) = (find(&sides.above, p, q), find(&sides.below, p, q));
            if ab + be > 0 {
                out.push((line, p, q, be, ab));
            }
        }
    }
    out
}

struct Attempt {
    network: Network,
    nodes: Vec<MeshNode>,
    stats: MeshStats,
    /// Nodes per `(layer, net)` as `(x_nm, y_nm, node)`.
    by_layer_net: LayerNodes,
    terminal_nodes: Vec<Vec<usize>>,
    port_names: Vec<String>,
}

fn clip_window(shapes: &Shapes, window: [i64; 4]) -> Shapes {
    poly::intersection(&poly::union(shapes), &rect_shapes(window))
}

fn build(layout: &Layout, spec: &MeshSpec, auto_scale: f64) -> Result<Attempt, MeshError> {
    let scale = spec.scale * auto_scale;
    let cell = ((spec.cell_nm as f64) * scale).round() as i64;
    let coarse = ((spec.coarse_nm as f64) * scale).round().max(cell as f64) as i64;
    let window = layout.window;
    if window[2] <= window[0] || window[3] <= window[1] {
        return Err(MeshError::Invalid("the window is empty".to_owned()));
    }
    let nt = spec.thickness_filaments.max(1);
    // Narrow means narrow against the nominal pitch, whatever the budget did to the pitch: a pour is not made narrow by coarsening.
    let fine_extent = (spec.fine_extent_cells * spec.cell_nm as f64) as i64;
    let mut network = Network::default();
    let mut nodes: Vec<MeshNode> = Vec::new();
    let mut stats = MeshStats { cell_nm: cell, coarse_nm: coarse, scale, ..MeshStats::default() };
    let mut by_layer_net: LayerNodes = BTreeMap::new();
    let mut bar_count = 0usize;
    for (li, layer) in layout.layers.iter().enumerate() {
        // Union of every net's copper, clipped to the window.
        let mut per_net: BTreeMap<&str, Shapes> = BTreeMap::new();
        for c in &layer.conductors {
            per_net.entry(c.net.as_str()).or_default().extend(c.shapes.iter().cloned());
        }
        let per_net: BTreeMap<&str, Shapes> = per_net
            .into_iter()
            .map(|(net, shapes)| (net, clip_window(&shapes, window)))
            .filter(|(_, shapes)| !shapes.is_empty())
            .collect();
        let mut layer_stats = LayerStats { name: layer.name.clone(), ..LayerStats::default() };
        if per_net.is_empty() {
            stats.layers.push(layer_stats);
            continue;
        }
        let mut req = Requests { sources: Vec::new(), hard: Vec::new(), soft: Vec::new() };
        let across = spec.across.max(1) as i64;
        let bounds_of = |contour: &poly::Contour| -> (i64, i64, i64, i64) {
            contour.iter().fold((i64::MAX, i64::MAX, i64::MIN, i64::MIN), |b, p| {
                (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y))
            })
        };
        // The conductors as the layout gives them (a track, a pad, a pour each), not their union: a trace that joins
        // a pour is still narrow.
        for conductor in &layer.conductors {
            for shape in &conductor.shapes {
                let Some(outer) = shape.first() else { continue };
                let (x0, y0, x1, y1) = bounds_of(outer);
                if x1 <= window[0] || x0 >= window[2] || y1 <= window[1] || y0 >= window[3] {
                    continue;
                }
                req.soft.push([x0, y0, x1, y1]);
                // A narrow shape is cut finely across its width: at least `across` cells, at most `cell` each.
                // Only copper on the loop (under a hint, next to a terminal) is held to that; other narrow copper
                // is cut at the pitch like any copper.
                let on_loop = conductor.on_loop;
                let size = |extent: i64| -> i64 {
                    if (extent as f64) > fine_extent as f64 {
                        0
                    } else if on_loop {
                        cell.min(extent / across).max(2 * ABS_GAP_NM)
                    } else {
                        cell
                    }
                };
                let (mut sx, mut sy) = (size(x1 - x0), size(y1 - y0));
                // A thin shape that is not along an axis (a diagonal trace) is narrow in neither bounding-box extent;
                // its width is about 2 area / perimeter, and it is cut finely in both axes.
                let axis_rect = shape.len() == 1
                    && outer.len() == 4
                    && outer.iter().all(|p| (p.x == x0 || p.x == x1) && (p.y == y0 || p.y == y1));
                if sx == 0 && sy == 0 && !axis_rect {
                    let perimeter: f64 = (0..outer.len())
                        .map(|i| {
                            let (a, b) = (&outer[i], &outer[(i + 1) % outer.len()]);
                            ((a.x - b.x) as f64).hypot((a.y - b.y) as f64)
                        })
                        .sum();
                    let width = if perimeter > 0.0 { 2.0 * poly::shape_area(shape) / perimeter } else { f64::MAX };
                    if width <= fine_extent as f64 {
                        let fine =
                            if on_loop { cell.min((width / across as f64) as i64).max(2 * ABS_GAP_NM) } else { cell };
                        (sx, sy) = (fine, fine);
                    }
                }
                if sx > 0 || sy > 0 {
                    req.sources.push(Source { rect: [x0, y0, x1, y1], sx, sy, growth: spec.growth });
                }
            }
        }
        // A slot or an opening that is not small against the pitch puts cell edges on its outline.
        for shapes in per_net.values() {
            for shape in shapes {
                for hole in shape.iter().skip(1) {
                    let (hx0, hy0, hx1, hy1) = bounds_of(hole);
                    if (hx1 - hx0).max(hy1 - hy0) > 3 * cell {
                        req.soft.push([hx0, hy0, hx1, hy1]);
                    }
                }
            }
        }
        for h in &layer.hints {
            let scaled = |v: i64| if v > 0 { (((v as f64) * scale).round() as i64).max(2 * ABS_GAP_NM) } else { 0 };
            req.sources.push(Source {
                rect: h.rect,
                sx: scaled(h.cell_x_nm),
                sy: scaled(h.cell_y_nm),
                growth: spec.growth,
            });
            req.soft.push(h.rect);
        }
        for t in layout.terminals.iter().filter(|t| t.layer == li) {
            if let Some((x0, y0, x1, y1)) = region_bounds(&t.region) {
                req.hard.push([x0, y0, x1, y1]);
                // A terminal is resolved whatever the pitch: at least three cells along its longer side.
                let size = ((x1 - x0).max(y1 - y0) / 3).max(2 * ABS_GAP_NM);
                req.sources.push(Source { rect: [x0, y0, x1, y1], sx: size, sy: size, growth: spec.growth });
            }
        }
        for v in &layout.vias {
            if v.first_layer <= li && li <= v.last_layer {
                req.hard.push([v.x, v.y, v.x, v.y]);
                // The current enters the planes at the via: its cell is as small as the via's own radius.
                let r_out = v.drill_nm / 2 + v.plating_nm;
                let size = (2 * r_out).max(2 * ABS_GAP_NM);
                req.sources.push(Source { rect: [v.x, v.y, v.x, v.y], sx: size, sy: size, growth: spec.growth });
            }
        }
        let cells = partition(window, &req, coarse, ABS_GAP_NM);
        layer_stats.cells = cells.len();
        layer_stats.smallest_cell_nm = cells.iter().map(|c| (c[2] - c[0]).min(c[3] - c[1])).min().unwrap_or(0);
        layer_stats.largest_cell_nm = cells.iter().map(|c| (c[2] - c[0]).max(c[3] - c[1])).max().unwrap_or(0);
        let hseg = segments(&cells, true);
        let vseg = segments(&cells, false);
        layer_stats.lines_x = vseg.iter().map(|s| s.0).collect::<std::collections::BTreeSet<_>>().len();
        layer_stats.lines_y = hseg.iter().map(|s| s.0).collect::<std::collections::BTreeSet<_>>().len();
        let thickness = layer.thickness_m;
        let t_sub = thickness / nt as f64;
        let mut ids: BTreeMap<(i64, i64, String), usize> = BTreeMap::new();
        let mut make_node = |x: i64, y: i64, net: &str, network: &mut Network, nodes: &mut Vec<MeshNode>| -> usize {
            let key = (x, y, net.to_owned());
            if let Some(&id) = ids.get(&key) {
                return id;
            }
            let id = network.node_count;
            network.node_count += 1;
            nodes.push(MeshNode { layer: li, net: net.to_owned(), position: [x as f64 * NM, y as f64 * NM] });
            by_layer_net.entry((li, net.to_owned())).or_default().push((x, y, id));
            ids.insert(key, id);
            id
        };
        for (&net, shapes) in &per_net {
            for horizontal in [true, false] {
                let segs = if horizontal { &hseg } else { &vseg };
                let mut i = 0;
                while i < segs.len() {
                    // All segments of one line share one strip of the net's copper.
                    let line = segs[i].0;
                    let mut j = i;
                    let (mut from, mut to, mut max_lo, mut max_hi) = (i64::MAX, i64::MIN, 0, 0);
                    while j < segs.len() && segs[j].0 == line {
                        from = from.min(segs[j].1);
                        to = to.max(segs[j].2);
                        max_lo = max_lo.max(segs[j].3);
                        max_hi = max_hi.max(segs[j].4);
                        j += 1;
                    }
                    let strip_rect = if horizontal {
                        [from, line - max_lo / 2, to, line + max_hi / 2]
                    } else {
                        [line - max_lo / 2, from, line + max_hi / 2, to]
                    };
                    let strip = poly::intersection(shapes, &rect_shapes(strip_rect));
                    if !strip.is_empty() {
                        for &(_, p, q, lo, hi) in &segs[i..j] {
                            let rect = if horizontal {
                                [p, line - lo / 2, q, line + hi / 2]
                            } else {
                                [line - lo / 2, p, line + hi / 2, q]
                            };
                            let (f, bbox) = covered(rect, &strip);
                            let Some(bbox) = bbox.filter(|_| f >= spec.min_coverage) else {
                                if f > 0.0 {
                                    stats.dropped_bars += 1;
                                }
                                continue;
                            };
                            // The copper has to run along the filament, not only stand beside it: within a band of
                            // `ABS_GAP_NM` round the filament's axis it has to span `MIN_AXIS_COVERAGE` of the length.
                            // A filament that would bridge a gap in the copper (the end of a trace, the port gap) is
                            // not made.
                            let band = if horizontal {
                                [p, line - ABS_GAP_NM, q, line + ABS_GAP_NM]
                            } else {
                                [line - ABS_GAP_NM, p, line + ABS_GAP_NM, q]
                            };
                            let spans = covered(band, &strip)
                                .1
                                .map_or(0, |b| if horizontal { b[2] - b[0] } else { b[3] - b[1] });
                            if (spans as f64) < MIN_AXIS_COVERAGE * (q - p) as f64 {
                                stats.dropped_bars += 1;
                                continue;
                            }
                            let (a, b) = if horizontal {
                                (
                                    make_node(p, line, net, &mut network, &mut nodes),
                                    make_node(q, line, net, &mut network, &mut nodes),
                                )
                            } else {
                                (
                                    make_node(line, p, net, &mut network, &mut nodes),
                                    make_node(line, q, net, &mut network, &mut nodes),
                                )
                            };
                            let length = (q - p) as f64 * NM;
                            let across_w =
                                if horizontal { (rect[3] - rect[1]) as f64 } else { (rect[2] - rect[0]) as f64 };
                            let width = across_w * NM * f;
                            let (axis, cx, cy) = if horizontal {
                                (Axis::X, (p + q) as f64 / 2.0 * NM, (bbox[1] + bbox[3]) as f64 / 2.0 * NM)
                            } else {
                                (Axis::Y, (bbox[0] + bbox[2]) as f64 / 2.0 * NM, (p + q) as f64 / 2.0 * NM)
                            };
                            for k in 0..nt {
                                let z = layer.z_m - thickness / 2.0 + (k as f64 + 0.5) * t_sub;
                                let bar = Bar::planar(axis, [cx, cy, z], length, width, t_sub);
                                network.branches.push(Branch {
                                    a,
                                    b,
                                    bar,
                                    resistance: partial::resistance(&bar, layout.resistivity, 0.0),
                                    kind: BranchKind::Plane { layer: li },
                                    group: bar_count,
                                });
                            }
                            bar_count += 1;
                            layer_stats.bars += 1;
                        }
                    }
                    i = j;
                }
            }
        }
        stats.layers.push(layer_stats);
    }
    stats.bars = bar_count;
    // Terminals: the nodes of the terminal's net and layer inside the region.
    let mut terminal_nodes: Vec<Vec<usize>> = Vec::new();
    for t in &layout.terminals {
        let candidates = by_layer_net.get(&(t.layer, t.net.clone()));
        let Some(candidates) = candidates.filter(|c| !c.is_empty()) else {
            return Err(MeshError::NoCopper(format!(
                "terminal {}: no copper of net {} on layer {} inside the window",
                t.name,
                t.net,
                layout.layers.get(t.layer).map_or("?", |l| l.name.as_str())
            )));
        };
        let grown = poly::offset(&t.region, ABS_GAP_NM, false, 1000.0);
        let mut inside: Vec<usize> = candidates
            .iter()
            .filter(|&&(x, y, _)| poly::shapes_contain(&grown, (x as f64, y as f64)))
            .map(|&(_, _, id)| id)
            .collect();
        if inside.is_empty() {
            let centre = region_bounds(&t.region).map(|b| ((b.0 + b.2) as f64 / 2.0, (b.1 + b.3) as f64 / 2.0));
            let (cx, cy) = centre.unwrap_or((0.0, 0.0));
            let nearest = candidates
                .iter()
                .min_by(|a, b| {
                    let da = (a.0 as f64 - cx).hypot(a.1 as f64 - cy);
                    let db = (b.0 as f64 - cx).hypot(b.1 as f64 - cy);
                    da.total_cmp(&db).then(a.2.cmp(&b.2))
                })
                .expect("non-empty");
            inside.push(nearest.2);
            stats.terminals_snapped.push(t.name.clone());
        }
        inside.sort_unstable();
        terminal_nodes.push(inside);
    }
    // Vias.
    let attach = 2 * coarse;
    let mut barrels = 0usize;
    for (vi, via) in layout.vias.iter().enumerate() {
        let mut attached: Vec<(usize, usize)> = Vec::new(); // (layer, node)
        for li in via.first_layer..=via.last_layer.min(layout.layers.len().saturating_sub(1)) {
            let net = via.layer_nets.iter().find(|(l, _)| *l == li).map_or(&via.net, |(_, n)| n);
            let Some(candidates) = by_layer_net.get(&(li, net.clone())) else { continue };
            let best = candidates
                .iter()
                .map(|&(x, y, id)| (((x - via.x) as f64).hypot((y - via.y) as f64), id))
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            if let Some((d, id)) = best
                && d <= attach as f64
            {
                attached.push((li, id));
            }
        }
        if attached.len() < 2 {
            stats.vias_unattached.push(via.name.clone());
            continue;
        }
        let r_out = (via.drill_nm as f64 / 2.0 + via.plating_nm as f64) * NM;
        for pair in attached.windows(2) {
            let ((la, na), (lb, nb)) = (pair[0], pair[1]);
            let (za, zb) = (layout.layers[la].z_m, layout.layers[lb].z_m);
            let length = (za - zb).abs();
            if length <= 0.0 {
                continue;
            }
            // Oriented along +z: from the lower node to the upper.
            let (lo, hi) = if za <= zb { (na, nb) } else { (nb, na) };
            let bar = Bar::barrel([via.x as f64 * NM, via.y as f64 * NM, (za + zb) / 2.0], length, r_out);
            network.branches.push(Branch {
                a: lo,
                b: hi,
                bar,
                resistance: partial::resistance(&bar, layout.resistivity, via.plating_nm as f64 * NM),
                kind: BranchKind::Barrel { via: vi },
                group: bar_count + barrels,
            });
            barrels += 1;
        }
    }
    stats.barrels = barrels;
    // Shorts and ports.
    for set in &layout.shorts {
        let mut all: Vec<usize> = set.iter().flat_map(|&t| terminal_nodes[t].iter().copied()).collect();
        all.sort_unstable();
        all.dedup();
        network.shorts.push(all);
    }
    let mut port_names = Vec::new();
    for p in &layout.ports {
        if p.plus >= terminal_nodes.len() || p.minus >= terminal_nodes.len() {
            return Err(MeshError::Invalid(format!("port {} refers to a terminal that does not exist", p.name)));
        }
        network.ports.push(NodePort { plus: terminal_nodes[p.plus].clone(), minus: terminal_nodes[p.minus].clone() });
        port_names.push(p.name.clone());
    }
    stats.filaments = network.branches.len();
    stats.nodes = network.node_count;
    Ok(Attempt { network, nodes, stats, by_layer_net, terminal_nodes, port_names })
}

fn region_bounds(region: &Shapes) -> Option<(i64, i64, i64, i64)> {
    region.iter().map(|s| poly::bounds(s)).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// Mesh a layout. Coarsens to the budget when `spec.coarsen` allows.
pub fn mesh(layout: &Layout, spec: &MeshSpec) -> Result<Mesh, MeshError> {
    let mut scale = 1.0;
    for step in 0..=10 {
        let attempt = build(layout, spec, scale)?;
        if attempt.stats.filaments <= spec.max_filaments {
            let mut stats = attempt.stats;
            stats.coarsen_steps = step;
            if step > 0 {
                stats.flags.push("coarsened".to_owned());
            }
            if !attempt.network.shorts.is_empty() {
                stats.flags.push("merged_terminals".to_owned());
            }
            return Ok(Mesh {
                network: attempt.network,
                nodes: attempt.nodes,
                stats,
                port_names: attempt.port_names,
                terminal_nodes: attempt.terminal_nodes,
                layer_nodes: attempt.by_layer_net,
            });
        }
        if !spec.coarsen || step == 10 {
            let detail = attempt
                .stats
                .layers
                .iter()
                .filter(|l| l.bars > 0)
                .map(|l| {
                    format!(
                        "{}: {} x {} lines, {} bars, cells {:.2} to {:.2} mm",
                        l.name,
                        l.lines_x,
                        l.lines_y,
                        l.bars,
                        l.smallest_cell_nm as f64 * 1e-6,
                        l.largest_cell_nm as f64 * 1e-6
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(MeshError::OverBudget {
                filaments: attempt.stats.filaments,
                limit: spec.max_filaments,
                detail: format!(
                    "after {step} coarsening step(s), pitch {:.2} mm fine, {:.2} mm coarse; {detail}",
                    attempt.stats.cell_nm as f64 * 1e-6,
                    attempt.stats.coarse_nm as f64 * 1e-6
                ),
            });
        }
        scale *= 1.3;
    }
    unreachable!("the loop returns")
}

impl Terminal {
    /// Bounding box of the region, nm.
    pub fn bounds(&self) -> Option<(i64, i64, i64, i64)> {
        region_bounds(&self.region)
    }
}

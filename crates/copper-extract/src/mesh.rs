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
//! dropped.
//!
//! **Connectivity is the copper's.** The mesh has exactly the connectivity of the copper polygons:
//! * The copper of a net on a layer is the union of its shapes; each polygon of it is one *piece*, and a filament is
//!   made from one piece only (the copper of the other pieces is not in its strip), with nodes that carry the piece:
//!   two pieces that do not touch never share a node and are never joined by a filament, however close their
//!   outlines and whatever the pitch. Filaments of different nets never share a node.
//! * A filament's copper has to run along its axis without a gap: within `ABS_GAP_NM` of the axis it has to be one
//!   chain (pieces of it no more than `GAP_TOL_NM` apart) covering `MIN_AXIS_COVERAGE` of the length, so no filament
//!   bridges a slot, an opening or the end of a trace, and a filament over a slot of one plane is not made.
//! * A node that the copper does not reach (every filament that ends on it stops short of it) is shared only by
//!   filaments whose copper inside their bodies touches (a diagonal trace is carried from filament to filament over
//!   such nodes); where it does not touch the node is split, one for each side.
//! * Every place that violates one of these (a slot or gap narrower than the filament, two pieces of copper in one
//!   filament body, a piece of copper that the meshing cut apart) is a defect of the partition and is refined: cells of
//!   half the body's extents there, at most `MAX_REFINE_ROUNDS` times, down to `2 ABS_GAP_NM`, while the budget holds.
//!   What cannot be resolved (a gap narrower than the smallest cell, the budget) is never bridged: it is listed in
//!   `MeshStats::unresolved_gaps` with the flag `unresolved_gap`, and the extraction is `indicative`.
//! * A topology check follows the build: the connected components of the copper pieces (joined by vias) are compared
//!   with those of the meshed network; a piece the mesh cut apart, or a network component that joins copper that is
//!   not one piece, is a mismatch (`MeshStats::topology`, flag `topology_mismatch`, extraction status `error`).
//!
//! **Vias** join the node nearest to the via position (in the piece of copper the via stands in, when it stands in
//! one) on every layer they pass where the net has copper within `2 coarse_nm`, in a chain of barrel segments whose length is the height difference of the layers.
//!
//! **Budget.** The inductance matrix is dense, so the number of filaments is bounded (`max_filaments`). When the
//! mesh is larger and `coarsen` is on, `cell_nm` and `coarse_nm` are multiplied by 1.3 until it fits (at most ten
//! times, the flag `coarsened` says so); when it is off the mesh is refused.

use std::collections::{BTreeMap, BTreeSet};

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
    /// Copper polygons against mesh connectivity.
    pub topology: TopologyCheck,
    /// Places where copper comes closer to other copper (or to itself across a slot) than the mesh could resolve, as
    /// `layer, net, x, y: gap`: at most `MAX_LISTED_GAPS` of them (`unresolved_gap_count` is the number).
    pub unresolved_gaps: Vec<String>,
    pub unresolved_gap_count: usize,
    /// Rounds of local refinement round gaps and cut copper (0: the first mesh was the answer).
    pub refine_rounds: usize,
    /// Refinements of the partition applied, and found but left out because the budget did not hold them (the
    /// farthest from the port's terminals go first; the repairs of a topology mismatch are never left out).
    pub refinements_applied: usize,
    pub refinements_skipped: usize,
    /// Filament bodies dropped because the copper does not run along them without a gap.
    pub gap_bars_dropped: usize,
}

/// The connectivity of the copper polygons against the connectivity of the meshed network.
#[derive(Clone, Debug, Default)]
pub struct TopologyCheck {
    /// Connected pieces of copper (polygons joined by vias) that have mesh nodes.
    pub copper_components: usize,
    /// Connected components of the meshed network (filaments and via barrels).
    pub mesh_components: usize,
    /// Polygons of copper without a single node (too small for the mesh, or all their filaments dropped).
    pub unmeshed_pieces: usize,
    /// One line for every copper piece the mesh cut apart and for every network component that joins copper that
    /// is not one piece. Empty when the mesh has exactly the connectivity of the copper.
    pub mismatches: Vec<String>,
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

/// A gap in the copper along a filament's axis this narrow or narrower is not a gap (the polygons touch, to the
/// resolution of the mesher), nm.
const GAP_TOL_NM: i64 = ABS_GAP_NM;

/// The most rounds of local refinement round gaps and cut copper.
const MAX_REFINE_ROUNDS: usize = 8;

/// Growth of the cell size round a refined defect, plus one: steeper than the general `growth` (2), so that the
/// refinement stays local (the cell size is the body's half extent at the defect and grows by three times the
/// distance from it).
const REFINE_GROWTH: f64 = 4.0;

/// The most unresolved gaps listed in the statistics.
const MAX_LISTED_GAPS: usize = 20;

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

#[derive(Clone)]
struct Attempt {
    network: Network,
    nodes: Vec<MeshNode>,
    stats: MeshStats,
    /// Nodes per `(layer, net)` as `(x_nm, y_nm, node)`.
    by_layer_net: LayerNodes,
    terminal_nodes: Vec<Vec<usize>>,
    port_names: Vec<String>,
    /// What the partition could not resolve, and the bars cut off a piece of copper.
    defects: Vec<Defect>,
    /// The pieces of copper the mesh cut apart, as `(layer, net, piece)`.
    split: BTreeSet<Piece>,
    /// The nodes of the parts a piece of copper was cut into other than its largest, `(layer, net, x_nm, y_nm)`:
    /// where the cut is.
    cut_nodes: Vec<CutNode>,
    /// For a refined mesh that is over the budget: the last mesh of the same pitch that fitted, before the
    /// refinement (its gaps are reported). Used when the pitch cannot be coarsened any further.
    fallback: Option<Box<Attempt>>,
}

/// Why a place wants a finer mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefectKind {
    /// The copper along a filament's axis has a gap (a slot or an opening): the filament would bridge it.
    Gap,
    /// Copper of two pieces of one net shares the filament's body: the pieces are closer than the cell.
    Shared,
    /// A filament of copper that is one piece with the rest was dropped (little copper along it): kept to refine
    /// where the meshing cuts a piece apart.
    Dropped,
}

/// A place where the mesh is too coarse for the copper.
#[derive(Clone, Debug)]
struct Defect {
    kind: DefectKind,
    layer: usize,
    net: String,
    /// The polygon of the net on the layer (index into the layer's union of that net's copper).
    comp: usize,
    /// The body of the filament, `[x0, y0, x1, y1]`.
    rect: [i64; 4],
    horizontal: bool,
}

impl Defect {
    /// Length of the filament along its axis, nm.
    fn length(&self) -> i64 {
        if self.horizontal { self.rect[2] - self.rect[0] } else { self.rect[3] - self.rect[1] }
    }
}

/// A piece of copper: `(layer, net, polygon index)`.
type Piece = (usize, String, usize);

/// A node on the far side of a cut: `(layer, net, x_nm, y_nm)`.
type CutNode = (usize, String, i64, i64);

/// A filament of one layer, kept to see which of its nodes the copper does not reach.
struct BarRec {
    first_branch: usize,
    count: usize,
    ends: [usize; 2],
    /// The copper along the filament stops short of the end's node.
    dangling: [bool; 2],
    /// The copper inside the filament's body, kept when an end dangles.
    piece: Option<Shapes>,
    net: String,
    comp: usize,
    rect: [i64; 4],
    horizontal: bool,
}

/// Nodes that are not where the copper is must not join copper that is not connected there. A node that no copper
/// reaches (every filament that ends on it stops short of it) is shared by the filaments of one piece of copper only
/// where the pieces of copper inside their bodies touch (a diagonal trace is carried over such nodes from filament to
/// filament); where they do not touch (a slot between them) the node is split, one node for each side, and the place
/// is a defect of the partition, refined or reported. Nodes in the copper are never split.
fn split_bridged_nodes(
    layer: usize,
    recs: &[BarRec],
    network: &mut Network,
    nodes: &mut Vec<MeshNode>,
    node_comp: &mut Vec<usize>,
    by_layer_net: &mut LayerNodes,
    defects: &mut Vec<Defect>,
) {
    // The filament ends on each node.
    let mut at: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new(); // node -> (record, end)
    for (r, rec) in recs.iter().enumerate() {
        for e in 0..2 {
            at.entry(rec.ends[e]).or_default().push((r, e));
        }
    }
    for (node, ends) in at {
        if ends.len() < 2 || ends.iter().any(|&(r, e)| !recs[r].dangling[e]) {
            continue;
        }
        // Cluster the dangling ends by whether the copper of their filaments touches.
        let mut parent: Vec<usize> = (0..ends.len()).collect();
        for i in 0..ends.len() {
            for j in (i + 1)..ends.len() {
                if find(&mut parent, i) == find(&mut parent, j) {
                    continue;
                }
                let (a, b) = (&recs[ends[i].0], &recs[ends[j].0]);
                let touch = match (&a.piece, &b.piece) {
                    (Some(pa), Some(pb)) => {
                        !poly::intersection(&poly::offset(pa, GAP_TOL_NM, false, 1000.0), pb).is_empty()
                    }
                    _ => true,
                };
                if touch {
                    union(&mut parent, i, j);
                }
            }
        }
        let mut roots: Vec<usize> = (0..ends.len()).map(|i| find(&mut parent, i)).collect();
        roots.sort_unstable();
        roots.dedup();
        if roots.len() < 2 {
            continue;
        }
        // The first cluster keeps the node, each other gets a node of its own at the same place.
        for &root in &roots[1..] {
            let id = network.node_count;
            network.node_count += 1;
            let original = nodes[node].clone();
            nodes.push(original.clone());
            node_comp.push(node_comp[node]);
            let (x, y) = ((original.position[0] / NM).round() as i64, (original.position[1] / NM).round() as i64);
            by_layer_net.entry((layer, original.net.clone())).or_default().push((x, y, id));
            for (i, &(r, e)) in ends.iter().enumerate() {
                if find(&mut parent, i) != root {
                    continue;
                }
                let rec = &recs[r];
                for k in rec.first_branch..rec.first_branch + rec.count {
                    if e == 0 {
                        network.branches[k].a = id;
                    } else {
                        network.branches[k].b = id;
                    }
                }
            }
        }
        for &(r, _) in &ends {
            let rec = &recs[r];
            defects.push(Defect {
                kind: DefectKind::Gap,
                layer,
                net: rec.net.clone(),
                comp: rec.comp,
                rect: rec.rect,
                horizontal: rec.horizontal,
            });
        }
    }
}

/// The union intervals along the axis of the copper `strip` inside `band`, sorted, chained where they are no more
/// than `GAP_TOL_NM` apart: `(start, end)` of each chain.
fn axis_chains(band: [i64; 4], strip: &Shapes, horizontal: bool) -> Vec<(i64, i64)> {
    let mut parts: Vec<(i64, i64)> = Vec::new();
    let mut rect: Option<Shapes> = None;
    for shape in strip {
        let (bx0, by0, bx1, by1) = poly::bounds(shape);
        if bx0 >= band[2] || bx1 <= band[0] || by0 >= band[3] || by1 <= band[1] {
            continue;
        }
        let outer = &shape[0];
        let axis_rect = shape.len() == 1
            && outer.len() == 4
            && outer.iter().all(|p| (p.x == bx0 || p.x == bx1) && (p.y == by0 || p.y == by1));
        let interval = |b: [i64; 4]| if horizontal { (b[0], b[2]) } else { (b[1], b[3]) };
        if axis_rect {
            parts.push(interval([bx0.max(band[0]), by0.max(band[1]), bx1.min(band[2]), by1.min(band[3])]));
        } else {
            let rect = rect.get_or_insert_with(|| rect_shapes(band));
            for piece in poly::intersection(rect, &vec![shape.clone()]) {
                let b = poly::bounds(&piece);
                parts.push(interval([b.0, b.1, b.2, b.3]));
            }
        }
    }
    parts.sort_unstable();
    let mut chains: Vec<(i64, i64)> = Vec::new();
    for (a, b) in parts {
        match chains.last_mut() {
            Some(last) if a <= last.1 + GAP_TOL_NM => last.1 = last.1.max(b),
            _ => chains.push((a, b)),
        }
    }
    chains
}

fn clip_window(shapes: &Shapes, window: [i64; 4]) -> Shapes {
    poly::intersection(&poly::union(shapes), &rect_shapes(window))
}

fn build_once(
    layout: &Layout,
    spec: &MeshSpec,
    auto_scale: f64,
    extra: &BTreeMap<usize, Vec<Source>>,
) -> Result<Attempt, MeshError> {
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
    let mut defects: Vec<Defect> = Vec::new();
    // The pieces of copper (the polygons of each net's union) of each layer, and the piece each node belongs to:
    // nodes of different pieces are different nodes whatever their position, so copper that does not touch is
    // never joined by the mesh.
    let mut layer_shapes: Vec<BTreeMap<String, Shapes>> = Vec::new();
    let mut node_comp: Vec<usize> = Vec::new();
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
        layer_shapes.push(per_net.iter().map(|(net, shapes)| ((*net).to_owned(), shapes.clone())).collect());
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
        if let Some(list) = extra.get(&li) {
            req.sources.extend(list.iter().copied());
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
        let mut recs: Vec<BarRec> = Vec::new();
        let mut ids: BTreeMap<(i64, i64, String, usize), usize> = BTreeMap::new();
        let mut make_node = |x: i64,
                             y: i64,
                             net: &str,
                             comp: usize,
                             network: &mut Network,
                             nodes: &mut Vec<MeshNode>,
                             node_comp: &mut Vec<usize>|
         -> usize {
            let key = (x, y, net.to_owned(), comp);
            if let Some(&id) = ids.get(&key) {
                return id;
            }
            let id = network.node_count;
            network.node_count += 1;
            nodes.push(MeshNode { layer: li, net: net.to_owned(), position: [x as f64 * NM, y as f64 * NM] });
            node_comp.push(comp);
            by_layer_net.entry((li, net.to_owned())).or_default().push((x, y, id));
            ids.insert(key, id);
            id
        };
        for (&net, shapes) in &per_net {
            let boxes: Vec<(i64, i64, i64, i64)> = shapes.iter().map(|s| poly::bounds(s)).collect();
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
                    // How many pieces of copper put a filament on each segment of the line.
                    let mut hits = vec![0u32; j - i];
                    for (comp, shape) in shapes.iter().enumerate() {
                        let b = boxes[comp];
                        if b.0 >= strip_rect[2] || b.2 <= strip_rect[0] || b.1 >= strip_rect[3] || b.3 <= strip_rect[1]
                        {
                            continue;
                        }
                        // The strip of this piece of copper alone: a filament never joins two pieces.
                        let strip = poly::intersection(&vec![shape.clone()], &rect_shapes(strip_rect));
                        if strip.is_empty() {
                            continue;
                        }
                        for (k, &(_, p, q, lo, hi)) in segs[i..j].iter().enumerate() {
                            let rect = if horizontal {
                                [p, line - lo / 2, q, line + hi / 2]
                            } else {
                                [line - lo / 2, p, line + hi / 2, q]
                            };
                            let defect = |kind: DefectKind| Defect {
                                kind,
                                layer: li,
                                net: net.to_owned(),
                                comp,
                                rect,
                                horizontal,
                            };
                            let (f, bbox) = covered(rect, &strip);
                            let Some(bbox) = bbox.filter(|_| f >= spec.min_coverage) else {
                                if f > 0.0 {
                                    stats.dropped_bars += 1;
                                    defects.push(defect(DefectKind::Dropped));
                                }
                                continue;
                            };
                            hits[k] += 1;
                            // The copper has to run along the filament without a gap, not only stand beside it:
                            // within a band of `ABS_GAP_NM` round the filament's axis it has to be one chain (no
                            // slot, opening or gap between pieces wider than `GAP_TOL_NM`) and span
                            // `MIN_AXIS_COVERAGE` of the length. A filament that would bridge a gap in the copper
                            // (the end of a trace, the port gap, a slot) is not made; a gap inside one is a
                            // defect of the partition and is refined.
                            let band = if horizontal {
                                [p, line - ABS_GAP_NM, q, line + ABS_GAP_NM]
                            } else {
                                [line - ABS_GAP_NM, p, line + ABS_GAP_NM, q]
                            };
                            let chains = axis_chains(band, &strip, horizontal);
                            if chains.len() > 1 {
                                stats.dropped_bars += 1;
                                stats.gap_bars_dropped += 1;
                                defects.push(defect(DefectKind::Gap));
                                continue;
                            }
                            let (c0, c1) = chains.first().copied().unwrap_or((p, p));
                            if ((c1 - c0) as f64) < MIN_AXIS_COVERAGE * (q - p) as f64 {
                                stats.dropped_bars += 1;
                                defects.push(defect(DefectKind::Dropped));
                                continue;
                            }
                            // An end whose node the copper does not reach (the copper along the filament stops short
                            // of it by more than the tolerance) is a dangling end: the node is not where the copper is.
                            let (dangling_a, dangling_b) = (c0 - p > GAP_TOL_NM, q - c1 > GAP_TOL_NM);
                            let piece =
                                (dangling_a || dangling_b).then(|| poly::intersection(&rect_shapes(rect), &strip));
                            let (a, b) = if horizontal {
                                (
                                    make_node(p, line, net, comp, &mut network, &mut nodes, &mut node_comp),
                                    make_node(q, line, net, comp, &mut network, &mut nodes, &mut node_comp),
                                )
                            } else {
                                (
                                    make_node(line, p, net, comp, &mut network, &mut nodes, &mut node_comp),
                                    make_node(line, q, net, comp, &mut network, &mut nodes, &mut node_comp),
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
                            let first_branch = network.branches.len();
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
                            recs.push(BarRec {
                                first_branch,
                                count: nt,
                                ends: [a, b],
                                dangling: [dangling_a, dangling_b],
                                piece,
                                net: net.to_owned(),
                                comp,
                                rect,
                                horizontal,
                            });
                            bar_count += 1;
                            layer_stats.bars += 1;
                        }
                    }
                    // Copper of two pieces of the net in one filament body: the pieces are closer than the cell.
                    for (k, &h) in hits.iter().enumerate() {
                        if h >= 2 {
                            let (_, p, q, lo, hi) = segs[i + k];
                            let rect = if horizontal {
                                [p, line - lo / 2, q, line + hi / 2]
                            } else {
                                [line - lo / 2, p, line + hi / 2, q]
                            };
                            defects.push(Defect {
                                kind: DefectKind::Shared,
                                layer: li,
                                net: net.to_owned(),
                                comp: 0,
                                rect,
                                horizontal,
                            });
                        }
                    }
                    i = j;
                }
            }
        }
        split_bridged_nodes(li, &recs, &mut network, &mut nodes, &mut node_comp, &mut by_layer_net, &mut defects);
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
    // The pieces of copper each via joins, for the topology check: `(layer, net, piece)`.
    let mut via_pieces: Vec<Vec<(usize, String, usize)>> = Vec::new();
    for (vi, via) in layout.vias.iter().enumerate() {
        let mut attached: Vec<(usize, usize)> = Vec::new(); // (layer, node)
        let mut pieces: Vec<(usize, String, usize)> = Vec::new();
        for li in via.first_layer..=via.last_layer.min(layout.layers.len().saturating_sub(1)) {
            let net = via.layer_nets.iter().find(|(l, _)| *l == li).map_or(&via.net, |(_, n)| n);
            let Some(candidates) = by_layer_net.get(&(li, net.clone())) else { continue };
            // The via joins the piece of copper it stands in (a node of another piece of the net is not the via's
            // copper, however near), else the nearest node.
            let inside = layer_shapes.get(li).and_then(|m| m.get(net.as_str())).and_then(|shapes| {
                shapes.iter().position(|shape| {
                    let b = poly::bounds(shape);
                    b.0 <= via.x
                        && via.x <= b.2
                        && b.1 <= via.y
                        && via.y <= b.3
                        && poly::shape_contains(shape, (via.x as f64, via.y as f64))
                })
            });
            let in_piece: Vec<&(i64, i64, usize)> = match inside {
                Some(c) => candidates.iter().filter(|(_, _, id)| node_comp[*id] == c).collect(),
                None => Vec::new(),
            };
            let pool: Vec<&(i64, i64, usize)> =
                if in_piece.is_empty() { candidates.iter().collect() } else { in_piece };
            let best = pool
                .iter()
                .map(|&&(x, y, id)| (((x - via.x) as f64).hypot((y - via.y) as f64), id))
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            if let Some((d, id)) = best
                && d <= attach as f64
            {
                attached.push((li, id));
                pieces.push((li, net.clone(), inside.unwrap_or(node_comp[id])));
            }
        }
        if attached.len() < 2 {
            stats.vias_unattached.push(via.name.clone());
            continue;
        }
        via_pieces.push(pieces);
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
    let (topology, split, cut_nodes) = check_topology(&layer_shapes, &nodes, &node_comp, &network, &via_pieces);
    if !topology.mismatches.is_empty() {
        stats.flags.push("topology_mismatch".to_owned());
    }
    stats.topology = topology;
    Ok(Attempt {
        network,
        nodes,
        stats,
        by_layer_net,
        terminal_nodes,
        port_names,
        defects,
        split,
        cut_nodes,
        fallback: None,
    })
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

/// The connected components of the copper polygons (the pieces of each net on each layer, joined where a via stands
/// in both) against those of the meshed network (filaments and barrels): every component of the network has to be
/// one component of the copper and every component of the copper that has nodes one component of the network. The
/// second value is the set of copper pieces that the mesh cut apart.
fn check_topology(
    layer_shapes: &[BTreeMap<String, Shapes>],
    nodes: &[MeshNode],
    node_comp: &[usize],
    network: &Network,
    via_pieces: &[Vec<(usize, String, usize)>],
) -> (TopologyCheck, BTreeSet<Piece>, Vec<CutNode>) {
    let mut item: BTreeMap<(usize, String, usize), usize> = BTreeMap::new();
    for (li, map) in layer_shapes.iter().enumerate() {
        for (net, shapes) in map {
            for comp in 0..shapes.len() {
                let id = item.len();
                item.insert((li, net.clone(), comp), id);
            }
        }
    }
    let mut copper: Vec<usize> = (0..item.len()).collect();
    for pieces in via_pieces {
        for pair in pieces.windows(2) {
            if let (Some(&a), Some(&b)) = (item.get(&pair[0]), item.get(&pair[1])) {
                union(&mut copper, a, b);
            }
        }
    }
    let mut mesh: Vec<usize> = (0..network.node_count).collect();
    for branch in &network.branches {
        union(&mut mesh, branch.a, branch.b);
    }
    let node_item = |n: usize| item.get(&(nodes[n].layer, nodes[n].net.clone(), node_comp[n])).copied();
    // Per piece of copper: the components of the network its nodes are in; per component: the pieces.
    let mut meshed: BTreeSet<usize> = BTreeSet::new();
    let mut of_group: BTreeMap<usize, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
    let mut of_mesh: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for n in 0..network.node_count {
        let Some(it) = node_item(n) else { continue };
        meshed.insert(it);
        let (g, m) = (find(&mut copper, it), find(&mut mesh, n));
        of_group.entry(g).or_default().entry(m).or_default().push(n);
        of_mesh.entry(m).or_default().insert(g);
    }
    let place = |n: usize| {
        format!(
            "layer {}, net {}, near ({:.2}, {:.2}) mm",
            nodes[n].layer,
            nodes[n].net,
            nodes[n].position[0] * 1e3,
            nodes[n].position[1] * 1e3
        )
    };
    let mut mismatches: Vec<String> = Vec::new();
    let mut split: BTreeSet<(usize, String, usize)> = BTreeSet::new();
    let mut cut_groups: BTreeSet<usize> = BTreeSet::new();
    let mut cut_nodes: Vec<(usize, String, i64, i64)> = Vec::new();
    for (g, parts) in &of_group {
        if parts.len() > 1 {
            cut_groups.insert(*g);
            // Every part but the one with most nodes is on the far side of a cut.
            let largest = parts.iter().max_by_key(|(m, ns)| (ns.len(), std::cmp::Reverse(**m))).map(|(m, _)| *m);
            for (m, ns) in parts {
                if Some(*m) != largest {
                    for &n in ns {
                        let q = nodes[n].position;
                        cut_nodes.push((
                            nodes[n].layer,
                            nodes[n].net.clone(),
                            (q[0] / NM).round() as i64,
                            (q[1] / NM).round() as i64,
                        ));
                    }
                }
            }
            let mut sizes: Vec<(usize, usize)> = parts.iter().map(|(m, ns)| (ns.len(), *m)).collect();
            sizes.sort_unstable();
            let small = parts[&sizes[0].1][0];
            mismatches.push(format!(
                "copper that is one piece is cut into {} pieces by the mesh (nodes {}), the smallest {}",
                parts.len(),
                sizes.iter().map(|s| s.0.to_string()).collect::<Vec<_>>().join(" + "),
                place(small)
            ));
        }
    }
    for (m, groups) in &of_mesh {
        if groups.len() > 1 {
            let first = (0..network.node_count).find(|&n| find(&mut mesh, n) == *m).unwrap_or(0);
            mismatches.push(format!(
                "the mesh joins {} separate pieces of copper into one network component, {}",
                groups.len(),
                place(first)
            ));
        }
    }
    if !cut_groups.is_empty() {
        for (key, &id) in &item {
            if cut_groups.contains(&find(&mut copper, id)) {
                split.insert(key.clone());
            }
        }
    }
    let copper_components = of_group.len();
    let mesh_components = of_mesh.len();
    (
        TopologyCheck { copper_components, mesh_components, unmeshed_pieces: item.len() - meshed.len(), mismatches },
        split,
        cut_nodes,
    )
}

fn region_bounds(region: &Shapes) -> Option<(i64, i64, i64, i64)> {
    region.iter().map(|s| poly::bounds(s)).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// The source of finer cells a defect asks for: each extent of the filament's body halved, an extent at least
/// `4 ABS_GAP_NM` long (the cells go no finer than `2 ABS_GAP_NM`); `None` when the body is already that small. Both
/// axes: the gap that the body holds may lie across its axis as well as along it.
fn refinement(defect: &Defect) -> Option<Source> {
    let r = defect.rect;
    let half = |extent: i64| if extent >= 4 * ABS_GAP_NM { extent / 2 } else { 0 };
    let (sx, sy) = (half(r[2] - r[0]), half(r[3] - r[1]));
    (sx > 0 || sy > 0).then_some(Source { rect: r, sx, sy, growth: REFINE_GROWTH })
}

/// Build the mesh and refine it locally where the partition is too coarse for the copper: a filament body that holds
/// a gap of its copper (a slot narrower than the cell), copper of two pieces, or a piece of copper that the meshing
/// cut apart gets cells of half the filament's extents, up to `MAX_REFINE_ROUNDS` times, **in order of priority while
/// the budget holds**:
/// 1. the refinements that repair a topology mismatch (dropped filaments next to the nodes of a cut-off part): all of
///    them or the pitch is coarsened;
/// 2. then the gaps and shared cells, nearest to the port's terminals first, as many as fit: the largest prefix of the
///    ordered list that keeps the mesh within the budget, found by bisection (deterministic).
///
/// What is left is reported (`unresolved_gaps`, the flag `unresolved_gap`, `refinement_stopped_by_budget` and the
/// counts of refinements applied and left out), never bridged.
fn build(layout: &Layout, spec: &MeshSpec, auto_scale: f64) -> Result<Attempt, MeshError> {
    // Where the port is: the bounding boxes of the terminals of the layout's ports.
    let anchors: Vec<[i64; 4]> = layout
        .ports
        .iter()
        .flat_map(|p| [p.plus, p.minus])
        .filter_map(|t| layout.terminals.get(t).and_then(|t| region_bounds(&t.region)))
        .map(|b| [b.0, b.1, b.2, b.3])
        .collect();
    let fits = |a: &Attempt| a.stats.filaments <= spec.max_filaments;
    let with = |extra: &BTreeMap<usize, Vec<Source>>, add: &[(usize, Source)]| {
        let mut e = extra.clone();
        for (layer, source) in add {
            e.entry(*layer).or_default().push(*source);
        }
        e
    };
    let mut extra: BTreeMap<usize, Vec<Source>> = BTreeMap::new();
    let mut attempt = build_once(layout, spec, auto_scale, &extra)?;
    let (mut applied, mut skipped, mut rounds) = (0usize, 0usize, 0usize);
    // The last mesh that was whole (no topology mismatch), before the gaps were refined.
    let mut clean: Option<Attempt> = None;
    for round in 0..=MAX_REFINE_ROUNDS {
        attempt.stats.refine_rounds = rounds;
        attempt.stats.refinements_applied = applied;
        attempt.stats.refinements_skipped = skipped;
        if !fits(&attempt) {
            // Over the budget without any refinement: the pitch has to be coarsened.
            return Ok(attempt);
        }
        // The defects of this mesh as sources of finer cells.
        let mut required: Vec<(usize, Source)> = Vec::new();
        let mut optional: Vec<(f64, usize, [i64; 4], Source)> = Vec::new();
        for d in &attempt.defects {
            let Some(source) = refinement(d) else { continue };
            match d.kind {
                // A dropped filament is refined where it may be what cut a piece of copper: next to a node of a part
                // that was cut off (within a filament length of its body), not everywhere in a big piece.
                DefectKind::Dropped => {
                    if attempt.split.contains(&(d.layer, d.net.clone(), d.comp))
                        && attempt.cut_nodes.iter().any(|(l, n, x, y)| {
                            *l == d.layer && *n == d.net && rect_distance(d.rect, [*x, *y, *x, *y]) <= d.length() as f64
                        })
                    {
                        required.push((d.layer, source));
                    }
                }
                DefectKind::Gap | DefectKind::Shared => {
                    let distance = anchors.iter().map(|a| rect_distance(d.rect, *a)).fold(f64::INFINITY, f64::min);
                    optional.push((distance, d.layer, d.rect, source));
                }
            }
        }
        optional.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        optional.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
        let optional: Vec<(usize, Source)> = optional.into_iter().map(|(_, l, _, s)| (l, s)).collect();
        if (required.is_empty() && optional.is_empty()) || round == MAX_REFINE_ROUNDS {
            break;
        }
        if !required.is_empty() {
            // First the repairs of the topology, alone: every one of them or no refinement at this pitch.
            let trial_extra = with(&extra, &required);
            let trial = build_once(layout, spec, auto_scale, &trial_extra)?;
            if fits(&trial) {
                applied += required.len();
                rounds += 1;
                extra = trial_extra;
                attempt = trial;
                continue;
            }
            return Ok(match clean {
                // Gaps were refined on a mesh that was whole, and now a repair does not fit: that mesh is the answer.
                Some(mut kept) => {
                    kept.stats.flags.push("refinement_stopped_by_budget".to_owned());
                    report_gaps(&mut kept, layout);
                    kept
                }
                // Not whole yet: coarsen, the best mesh so far goes along as the answer when no pitch holds them.
                None => {
                    let mut over = trial;
                    let mut keep = attempt;
                    keep.stats.flags.push("refinement_stopped_by_budget".to_owned());
                    report_gaps(&mut keep, layout);
                    over.fallback = Some(Box::new(keep));
                    over
                }
            });
        }
        // The mesh is whole (the copper's connectivity): now the gaps, nearest the port first, as many as fit.
        if attempt.stats.topology.mismatches.is_empty() && clean.is_none() {
            clean = Some(attempt.clone());
        }
        let all: Vec<(usize, Source)> = optional.clone();
        let full_extra = with(&extra, &all);
        let full = build_once(layout, spec, auto_scale, &full_extra)?;
        if fits(&full) {
            applied += all.len();
            rounds += 1;
            extra = full_extra;
            attempt = full;
            continue;
        }
        // The largest prefix of the ordered gaps that keeps the mesh within the budget, by bisection.
        let mut best: Option<(Attempt, BTreeMap<usize, Vec<Source>>, usize)> = None;
        let (mut lo, mut hi) = (0usize, optional.len());
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let trial_extra = with(&extra, &optional[..mid]);
            let trial = build_once(layout, spec, auto_scale, &trial_extra)?;
            if fits(&trial) {
                lo = mid;
                best = Some((trial, trial_extra, mid));
            } else {
                hi = mid;
            }
        }
        skipped += optional.len() - lo;
        if !attempt.stats.flags.iter().any(|f| f == "refinement_stopped_by_budget") {
            attempt.stats.flags.push("refinement_stopped_by_budget".to_owned());
        }
        match best {
            Some((kept, kept_extra, prefix)) => {
                applied += prefix;
                rounds += 1;
                let mut kept = kept;
                kept.stats.flags.push("refinement_stopped_by_budget".to_owned());
                attempt = kept;
                extra = kept_extra;
            }
            // Nothing more fits within the budget.
            None => break,
        }
    }
    attempt.stats.refine_rounds = rounds;
    attempt.stats.refinements_applied = applied;
    attempt.stats.refinements_skipped = skipped;
    report_gaps(&mut attempt, layout);
    Ok(attempt)
}

/// Put the gaps the mesh could not resolve into the statistics and the flags.
fn report_gaps(attempt: &mut Attempt, layout: &Layout) {
    let mut seen: BTreeSet<(usize, String, i64, i64)> = BTreeSet::new();
    let mut listed: Vec<String> = Vec::new();
    for d in attempt.defects.iter().filter(|d| matches!(d.kind, DefectKind::Gap | DefectKind::Shared)) {
        let (cx, cy) = ((d.rect[0] + d.rect[2]) / 2, (d.rect[1] + d.rect[3]) / 2);
        if !seen.insert((d.layer, d.net.clone(), cx / 200_000, cy / 200_000)) {
            continue;
        }
        if listed.len() < MAX_LISTED_GAPS {
            listed.push(format!(
                "{}, net {}, near ({:.2}, {:.2}) mm: {} that the {:.0} um cell does not resolve",
                layout.layers.get(d.layer).map_or("?", |l| l.name.as_str()),
                d.net,
                cx as f64 * 1e-6,
                cy as f64 * 1e-6,
                if d.kind == DefectKind::Gap {
                    "a gap in the copper"
                } else {
                    "copper of two pieces closer than the cell"
                },
                d.length() as f64 * 1e-3
            ));
        }
    }
    attempt.stats.unresolved_gap_count = seen.len();
    attempt.stats.unresolved_gaps = listed;
    if !seen.is_empty() {
        attempt.stats.flags.push("unresolved_gap".to_owned());
    }
}

/// The mesh of an attempt that fits the budget, after `step` coarsening steps.
fn finish(attempt: Attempt, step: usize) -> Result<Mesh, MeshError> {
    let mut stats = attempt.stats;
    stats.coarsen_steps = step;
    if step > 0 {
        stats.flags.push("coarsened".to_owned());
    }
    if !attempt.network.shorts.is_empty() {
        stats.flags.push("merged_terminals".to_owned());
    }
    Ok(Mesh {
        network: attempt.network,
        nodes: attempt.nodes,
        stats,
        port_names: attempt.port_names,
        terminal_nodes: attempt.terminal_nodes,
        layer_nodes: attempt.by_layer_net,
    })
}

/// Mesh a layout. Coarsens to the budget when `spec.coarsen` allows.
pub fn mesh(layout: &Layout, spec: &MeshSpec) -> Result<Mesh, MeshError> {
    let mut scale = 1.0;
    let mut fallback: Option<(usize, Box<Attempt>)> = None;
    for step in 0..=10 {
        let mut attempt = build(layout, spec, scale)?;
        if let Some(kept) = attempt.fallback.take() {
            // The best mesh that fitted without the whole refinement: fewest topology mismatches, then fewest
            // unresolved gaps, then the finest pitch.
            let key = |a: &Attempt| (a.stats.topology.mismatches.len(), a.stats.unresolved_gap_count);
            if fallback.as_ref().is_none_or(|(_, held)| key(&kept) < key(held)) {
                fallback = Some((step, kept));
            }
        }
        if attempt.stats.filaments > spec.max_filaments
            && (!spec.coarsen || step == 10)
            && let Some((fit_step, kept)) = fallback.take()
        {
            // The refinement does not fit at any pitch tried: the finest mesh that fitted without it, with its gaps
            // reported (flag `refinement_stopped_by_budget`).
            return finish(*kept, fit_step);
        }
        if attempt.stats.filaments <= spec.max_filaments {
            return finish(attempt, step);
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

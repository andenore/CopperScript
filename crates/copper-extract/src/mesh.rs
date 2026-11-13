//! From layout to network: a rectangular filament mesh of every copper layer.
//!
//! **Mesh.** Each layer has its own tensor grid of lines. Nodes sit on the line crossings; an x filament joins
//! two neighbouring nodes of a row, a y filament two neighbouring nodes of a column. The filament's cross-section
//! is the strip between the half-way lines to the neighbouring rows (columns), so the strips tile the copper: a bar
//! of copper whose edges lie on grid lines is exactly tiled and has exactly `rho l / (w t)` resistance. The width
//! of a filament is the strip width times the fraction of its body (the rectangle between its two nodes and the
//! strip) that the layer's copper of the net covers, so a trace edge between lines still carries the right amount of
//! copper; a filament whose body is covered by less than `min_coverage` is dropped. Filaments of different nets
//! never share a node.
//!
//! **Lines.** Lines are put on the bounding box edges of every copper shape, of every terminal and of every hint
//! rectangle, at the position of every via, and between them at a pitch of `cell_nm` where a shape is narrow in that
//! axis (an extent of at most `fine_extent_cells` cells), near a via or terminal, or inside a hint; the pitch grows by
//! `growth` per unit distance to `coarse_nm` away from such places. A shape that is wide in an axis (a pour) is
//! meshed at the coarse pitch there. The grid of a layer does not depend on the grid of any other layer: layers
//! couple through the partial inductance matrix and are joined only by vias.
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
    /// Pitch in narrow features and near vias and terminals, nm.
    pub cell_nm: i64,
    /// Pitch of wide copper, nm.
    pub coarse_nm: i64,
    /// Growth of the pitch per unit distance from a fine place.
    pub growth: f64,
    /// A shape with an extent of at most this many `cell_nm` in an axis is meshed finely in that axis.
    pub fine_extent_cells: f64,
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
            growth: 1.4,
            fine_extent_cells: 8.0,
            thickness_filaments: 1,
            max_filaments: DEFAULT_MAX_FILAMENTS,
            coarsen: true,
            min_coverage: 0.05,
            scale: 1.0,
        }
    }
}

/// The default filament budget: the dense inductance matrix is `n^2` doubles and one frequency costs a dense complex
/// elimination of about `n/2` loops, a few seconds at 2400 filaments in an optimised build.
pub const DEFAULT_MAX_FILAMENTS: usize = 2400;

impl MeshSpec {
    /// The same spec with all pitches (hints included) divided by `factor`: a finer mesh for a refinement series.
    pub fn refined(&self, factor: f64) -> MeshSpec {
        MeshSpec { scale: self.scale / factor, ..self.clone() }
    }
}

/// The nodes of each `(layer, net)` as `(x_nm, y_nm, node)`.
pub type LayerNodes = BTreeMap<(usize, String), Vec<(i64, i64, usize)>>;

/// Lines requested over an interval of one axis: `(from, to, pitch)`, nm.
type FineIntervals = Vec<(i64, i64, i64)>;

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

/// Lines of one axis over `[lo, hi]`.
fn axis_lines(
    lo: i64,
    hi: i64,
    musts: &[i64],
    fine: &[(i64, i64, i64)],
    coarse: i64,
    growth: f64,
    min_gap: i64,
) -> Vec<i64> {
    let mut m: Vec<i64> = musts.iter().copied().filter(|v| *v >= lo && *v <= hi).collect();
    m.push(lo);
    m.push(hi);
    m.sort_unstable();
    m.dedup();
    let mut ms: Vec<i64> = vec![m[0]];
    for &v in &m[1..] {
        let last = *ms.last().expect("non-empty");
        if v - last < min_gap {
            if v == hi && ms.len() > 1 {
                ms.pop();
                ms.push(v);
            } else if v == hi {
                ms.push(v);
            }
            continue;
        }
        ms.push(v);
    }
    let mut fine: Vec<(i64, i64, i64)> = fine.to_vec();
    fine.sort_unstable();
    fine.dedup();
    let size_at = |x: f64| -> f64 {
        let mut s = coarse as f64;
        for &(a, b, c) in &fine {
            let d = if x < a as f64 {
                a as f64 - x
            } else if x > b as f64 {
                x - b as f64
            } else {
                0.0
            };
            s = s.min(c as f64 + (growth - 1.0) * d);
        }
        s
    };
    // Sizes are quantised to micrometres so that mirrored places get identical sizes.
    let quant = |v: f64| ((v / 1000.0).round() as i64 * 1000).max(min_gap);
    let mut lines = vec![ms[0]];
    for w in ms.windows(2) {
        let (a, b) = (w[0], w[1]);
        // March from both ends towards the middle, each with the size of its own end, so a layout that is mirror
        // symmetric about a span's middle gets mirror symmetric lines; the gap that is left is split evenly.
        let mut left = vec![a];
        let mut right = vec![b];
        loop {
            let (lp, rp) = (*left.last().expect("start"), *right.last().expect("end"));
            let gap = rp - lp;
            let sl = {
                let s0 = size_at(lp as f64);
                quant(s0.min(size_at((lp as f64 + s0).min(b as f64))))
            };
            let sr = {
                let s0 = size_at(rp as f64);
                quant(s0.min(size_at((rp as f64 - s0).max(a as f64))))
            };
            if gap as f64 <= 2.5 * sl.max(sr) as f64 {
                let mean = (sl + sr) as f64 / 2.0;
                let n = ((gap as f64 / mean).round() as i64).max(1);
                for k in 1..n {
                    left.push(lp + gap * k / n);
                }
                break;
            }
            left.push(lp + sl);
            right.push(rp - sr);
        }
        right.reverse();
        lines.extend(left.into_iter().skip(1));
        lines.extend(right);
    }
    lines.dedup();
    lines
}

struct LayerMesh {
    xs: Vec<i64>,
    ys: Vec<i64>,
}

impl LayerMesh {
    /// The strip around line `i` of `lines`: half way to the neighbours (the end lines have half strips).
    fn dual(lines: &[i64], i: usize) -> (i64, i64) {
        let lo = if i == 0 { lines[0] } else { (lines[i - 1] + lines[i]) / 2 };
        let hi = if i + 1 == lines.len() { lines[i] } else { (lines[i] + lines[i + 1]) / 2 };
        (lo, hi)
    }
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
    let fine_extent = (spec.fine_extent_cells * cell as f64) as i64;
    let min_gap = (cell / 3).max(1000);
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
        let (mut mx, mut my) = (Vec::new(), Vec::new());
        let (mut fx, mut fy): (FineIntervals, FineIntervals) = (Vec::new(), Vec::new());
        for shapes in per_net.values() {
            for shape in shapes {
                // The outline and every hole (a slot in a plane) put lines on their bounding box edges.
                for (index, contour) in shape.iter().enumerate() {
                    let (x0, y0, x1, y1) = contour.iter().fold((i64::MAX, i64::MAX, i64::MIN, i64::MIN), |b, p| {
                        (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y))
                    });
                    // A hole (a clearance around a pad or via) or an island that is small against the pitch is
                    // resolved by the coverage fractions of the filaments; it asks for no lines of its own.
                    if index > 0 && (x1 - x0).max(y1 - y0) <= 3 * cell {
                        continue;
                    }
                    mx.extend([x0, x1]);
                    my.extend([y0, y1]);
                    if x1 - x0 <= fine_extent {
                        fx.push((x0, x1, cell));
                    }
                    if y1 - y0 <= fine_extent {
                        fy.push((y0, y1, cell));
                    }
                }
            }
        }
        for h in &layer.hints {
            if h.cell_x_nm > 0 {
                mx.extend([h.rect[0], h.rect[2]]);
                fx.push((h.rect[0], h.rect[2], ((h.cell_x_nm as f64) * scale).round() as i64));
            }
            if h.cell_y_nm > 0 {
                my.extend([h.rect[1], h.rect[3]]);
                fy.push((h.rect[1], h.rect[3], ((h.cell_y_nm as f64) * scale).round() as i64));
            }
        }
        for t in layout.terminals.iter().filter(|t| t.layer == li) {
            if let Some((x0, y0, x1, y1)) = region_bounds(&t.region) {
                mx.extend([x0, x1]);
                my.extend([y0, y1]);
                fx.push((x0 - cell, x1 + cell, cell));
                fy.push((y0 - cell, y1 + cell, cell));
            }
        }
        for v in &layout.vias {
            if v.first_layer <= li && li <= v.last_layer {
                mx.push(v.x);
                my.push(v.y);
                fx.push((v.x - 2 * cell, v.x + 2 * cell, cell));
                fy.push((v.y - 2 * cell, v.y + 2 * cell, cell));
            }
        }
        let xs = axis_lines(window[0], window[2], &mx, &fx, coarse, spec.growth, min_gap);
        let ys = axis_lines(window[1], window[3], &my, &fy, coarse, spec.growth, min_gap);
        let mesh = LayerMesh { xs, ys };
        layer_stats.lines_x = mesh.xs.len();
        layer_stats.lines_y = mesh.ys.len();
        let spacing: Vec<i64> =
            mesh.xs.windows(2).map(|w| w[1] - w[0]).chain(mesh.ys.windows(2).map(|w| w[1] - w[0])).collect();
        layer_stats.smallest_cell_nm = spacing.iter().copied().min().unwrap_or(0);
        layer_stats.largest_cell_nm = spacing.iter().copied().max().unwrap_or(0);
        let thickness = layer.thickness_m;
        let t_sub = thickness / nt as f64;
        let mut ids: BTreeMap<(usize, usize, String), usize> = BTreeMap::new();
        let mut make_node =
            |i: usize, j: usize, net: &str, network: &mut Network, nodes: &mut Vec<MeshNode>| -> usize {
                let key = (i, j, net.to_owned());
                if let Some(&id) = ids.get(&key) {
                    return id;
                }
                let id = network.node_count;
                network.node_count += 1;
                nodes.push(MeshNode {
                    layer: li,
                    net: net.to_owned(),
                    position: [mesh.xs[i] as f64 * NM, mesh.ys[j] as f64 * NM],
                });
                by_layer_net.entry((li, net.to_owned())).or_default().push((mesh.xs[i], mesh.ys[j], id));
                ids.insert(key, id);
                id
            };
        for (&net, shapes) in &per_net {
            // x filaments: row j, strip between the half-way lines.
            for j in 0..mesh.ys.len() {
                let (ylo, yhi) = LayerMesh::dual(&mesh.ys, j);
                if yhi <= ylo {
                    continue;
                }
                let strip = poly::intersection(shapes, &rect_shapes([window[0], ylo, window[2], yhi]));
                if strip.is_empty() {
                    continue;
                }
                for i in 0..mesh.xs.len() - 1 {
                    let rect = [mesh.xs[i], ylo, mesh.xs[i + 1], yhi];
                    let (f, bbox) = covered(rect, &strip);
                    let Some(bbox) = bbox.filter(|_| f >= spec.min_coverage) else {
                        if f > 0.0 {
                            stats.dropped_bars += 1;
                        }
                        continue;
                    };
                    let a = make_node(i, j, net, &mut network, &mut nodes);
                    let b = make_node(i + 1, j, net, &mut network, &mut nodes);
                    let length = (rect[2] - rect[0]) as f64 * NM;
                    let width = (yhi - ylo) as f64 * NM * f;
                    let cx = (rect[0] + rect[2]) as f64 / 2.0 * NM;
                    let cy = (bbox[1] + bbox[3]) as f64 / 2.0 * NM;
                    for k in 0..nt {
                        let z = layer.z_m - thickness / 2.0 + (k as f64 + 0.5) * t_sub;
                        let bar = Bar::planar(Axis::X, [cx, cy, z], length, width, t_sub);
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
            // y filaments: column i.
            for i in 0..mesh.xs.len() {
                let (xlo, xhi) = LayerMesh::dual(&mesh.xs, i);
                if xhi <= xlo {
                    continue;
                }
                let strip = poly::intersection(shapes, &rect_shapes([xlo, window[1], xhi, window[3]]));
                if strip.is_empty() {
                    continue;
                }
                for j in 0..mesh.ys.len() - 1 {
                    let rect = [xlo, mesh.ys[j], xhi, mesh.ys[j + 1]];
                    let (f, bbox) = covered(rect, &strip);
                    let Some(bbox) = bbox.filter(|_| f >= spec.min_coverage) else {
                        if f > 0.0 {
                            stats.dropped_bars += 1;
                        }
                        continue;
                    };
                    let a = make_node(i, j, net, &mut network, &mut nodes);
                    let b = make_node(i, j + 1, net, &mut network, &mut nodes);
                    let length = (rect[3] - rect[1]) as f64 * NM;
                    let width = (xhi - xlo) as f64 * NM * f;
                    let cx = (bbox[0] + bbox[2]) as f64 / 2.0 * NM;
                    let cy = (rect[1] + rect[3]) as f64 / 2.0 * NM;
                    for k in 0..nt {
                        let z = layer.z_m - thickness / 2.0 + (k as f64 + 0.5) * t_sub;
                        let bar = Bar::planar(Axis::Y, [cx, cy, z], length, width, t_sub);
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
        let grown = poly::offset(&t.region, (cell / 100).max(500), false, 1000.0);
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

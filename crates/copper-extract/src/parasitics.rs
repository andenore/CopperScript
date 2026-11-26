//! Layout parasitics of a routed board for a circuit simulation: the capacitance between nets and to planes, and
//! the resistance and inductance of the tracks between the pads they join.
//!
//! **Copper.** As [`crate::board`]: the routed `PhysicalBoard` exported in memory, its zones filled natively; tracks
//! (arcs flattened), pads, vias and zone fills of every net, on every copper layer. Copper without a net is left out.
//!
//! **Capacitance (2.5-D).** Each track, pad and via ring is cut across at stations along its axis (at most
//! [`ParasiticOptions::station_pitch_m`] apart). A cut is the board's cross-section along a line through the
//! station, perpendicular to the piece: every piece of copper the line crosses, on every layer, is a conductor of
//! that cross-section, with the stack-up's dielectric slabs and air above and below, in a grounded box
//! ([`ParasiticOptions::cut_half_width_m`] to each side, [`ParasiticOptions::air_m`] above and below). The piece is
//! held at 1 V and every other piece at 0 V ([`crate::capacitance`]); the charge each other piece takes, times the
//! station's share of the length, is the partial capacitance between the two pieces there. Summed over the pieces
//! of two nets this is the capacitance between the nets (the partial capacitances of the pieces of one net add up
//! to the net's when the net is one potential). A pair of pieces that both have stations is seen from both sides;
//! each side counts half. Zone fills have no stations: what couples to them is counted from the other piece's side
//! in full, and two fills of different nets on different layers add the parallel-plate capacitance of their
//! overlap where no other copper lies between. A track that crosses the cut at more than 45 degrees is not a
//! conductor of the cut (it would be extruded along the station's piece); two such tracks of different nets on
//! different layers that cross add `eps w_a' w_b' / (d sin theta)`, each width widened by its strip-over-plane
//! fringe (Hammerstad), unless a fill lies between them at the crossing. The charge that reaches the box is
//! capacitance to nothing on the board within the cut; it is reported per net (`unattributed`) and not inserted.
//!
//! **Where this is not exact.** A cut is a 2-D (extruded) field: the ends of tracks and pads, corners, the field
//! between a track's end and a pad beyond it and the 3-D shape of a crossing are not in it (the crossing term is an
//! estimate). Solder mask is not modelled: on outer layers a 20 um mask of er 3.5 raises a 0.25 mm microstrip's
//! capacitance by about 12 % (`tests/capacitance.rs`), so outer-layer values are low by about that much. The
//! dielectric is the declared stack-up's (or the assumed one, flag `assumed_stackup`); the box charge is lost.
//!
//! **Resistance and inductance.** For the nets the caller asks for ([`ParasiticOptions::networked`]), the tracks
//! between pads become a network: a node per pad, via and fill piece, and per track end; a branch per track piece
//! with its DC resistance `rho l / (w t)` and its inductance `l [C_0'^-1]_00 / c^2`, `C_0'` the vacuum capacitance
//! matrix per metre of the signal copper in the piece's middle cut (every dielectric replaced by vacuum; copper of
//! networked nets floating, fills and other nets held as the return): the TEM self-inductance of a multiconductor
//! line, exact in 2-D. Mutual inductance between nets is not inserted. Where more than half of the piece's charge
//! goes to the box (no return copper within the cut) the partial self-inductance of the bar is used instead (flag
//! `inductance_without_return`). Vias are single nodes (their barrels are ideal), fills
//! are single nodes, skin effect is not modelled. Chains of track pieces between junctions are merged in series
//! and the capacitance of a removed node shared between its two neighbours; dead-end stubs are folded into the node
//! they hang from. Other nets are one node each.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use copper_cam::board::TrackPath;
use copper_cam::geometry::flatten_arc;
use copper_cam::poly::{self, Shapes};
use copper_pcb::board::PhysicalBoard;

use crate::board::{BoardModel, copper_thickness_m, flashes, pad_shape};
use crate::capacitance::{EPSILON_0, Rect, SPEED_OF_LIGHT, Section, SectionMesh, Slab, Wall, closed_form};

const NM: f64 = 1e-9;
const MAX_ERROR: f64 = 5_000.0;
/// The narrowest copper a cut keeps, nm.
const MIN_INTERVAL_NM: f64 = 5_000.0;

/// The mesh of the board's cuts: coarser than [`SectionMesh::default`] (growth 1.3, coarsest cell a tenth of the
/// cut, nothing finer than 2 um), which a board of a few hundred distinct cuts needs. Its errors on the closed
/// forms are printed by `tests/capacitance.rs` (`the_board_mesh_is_coarser_and_says_how_much`).
pub const BOARD_MESH: SectionMesh = SectionMesh {
    fine_fraction: 1.0 / 20.0,
    fine_min: 2e-6,
    fine_max: 20e-6,
    growth: 1.3,
    coarse_fraction: 0.1,
    extrapolate: true,
};

/// What the caller chooses.
#[derive(Clone, Debug)]
pub struct ParasiticOptions {
    /// Nets whose tracks become R and L between their pads; every other net is one node.
    pub networked: BTreeSet<String>,
    /// The longest piece of length one station stands for, metres.
    pub station_pitch_m: f64,
    /// The cut's reach to each side of a station, metres.
    pub cut_half_width_m: f64,
    /// Air above the top copper and below the bottom copper in a cut, metres.
    pub air_m: f64,
    pub mesh: SectionMesh,
}

impl Default for ParasiticOptions {
    fn default() -> Self {
        ParasiticOptions {
            networked: BTreeSet::new(),
            station_pitch_m: 0.5e-3,
            cut_half_width_m: 2.5e-3,
            air_m: 2.5e-3,
            mesh: BOARD_MESH,
        }
    }
}

/// A node of a net's network.
#[derive(Clone, Debug, PartialEq)]
pub struct NetNode {
    /// `pad R1.2`, `via 12.00,4.50 mm`, `fill F.Cu`, `track end F.Cu 12.00,4.50 mm`, or the net's name for a net
    /// that is one node.
    pub label: String,
}

/// A series branch of a net: tracks between two nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub a: usize,
    pub b: usize,
    /// Ohm (DC).
    pub resistance: f64,
    /// Henry.
    pub inductance: f64,
    /// The copper length, metres.
    pub length_m: f64,
    /// Track pieces merged into the branch.
    pub pieces: usize,
}

/// One net of the result.
#[derive(Clone, Debug, PartialEq)]
pub struct NetParasitics {
    pub net: String,
    pub nodes: Vec<NetNode>,
    /// `(component reference as on the board, pad number, node)`.
    pub pads: Vec<(String, String, usize)>,
    pub branches: Vec<Branch>,
    /// Track length of the net, metres.
    pub track_length_m: f64,
    /// Capacitance to the cut's box (nothing within reach), F; not in any coupling.
    pub unattributed: f64,
}

/// A capacitance between a node of one net and a node of another.
#[derive(Clone, Debug, PartialEq)]
pub struct Coupling {
    /// `(net index, node)`, the lower net index first.
    pub a: (usize, usize),
    pub b: (usize, usize),
    pub farad: f64,
}

/// The parasitics of a board.
#[derive(Clone, Debug, PartialEq)]
pub struct Parasitics {
    pub nets: Vec<NetParasitics>,
    pub couplings: Vec<Coupling>,
    /// `assumed_stackup`, `assumed_copper_thickness`, `inductance_without_return`.
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    /// Stations cut, distinct cross-sections solved, crossings estimated.
    pub stations: usize,
    pub sections: usize,
    pub crossings: usize,
    pub seconds: f64,
}

impl Parasitics {
    pub fn net_index(&self, net: &str) -> Option<usize> {
        self.nets.iter().position(|n| n.net == net)
    }

    /// The capacitance between two nets, summed over their nodes, F.
    pub fn between(&self, a: &str, b: &str) -> f64 {
        let (Some(i), Some(j)) = (self.net_index(a), self.net_index(b)) else { return 0.0 };
        self.couplings
            .iter()
            .filter(|c| (c.a.0 == i && c.b.0 == j) || (c.a.0 == j && c.b.0 == i))
            .map(|c| c.farad)
            .sum()
    }

    /// Every pair of nets with a capacitance, largest first: `(net a, net b, farad)`.
    pub fn net_pairs(&self) -> Vec<(String, String, f64)> {
        let mut sums: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for c in &self.couplings {
            *sums.entry((c.a.0.min(c.b.0), c.a.0.max(c.b.0))).or_default() += c.farad;
        }
        let mut pairs: Vec<(String, String, f64)> =
            sums.into_iter().map(|((i, j), f)| (self.nets[i].net.clone(), self.nets[j].net.clone(), f)).collect();
        pairs.sort_by(|x, y| y.2.total_cmp(&x.2).then_with(|| (&x.0, &x.1).cmp(&(&y.0, &y.1))));
        pairs
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A track piece from `a` to `b` (nm), `width` nm.
    Track {
        a: (f64, f64),
        b: (f64, f64),
        width: f64,
    },
    /// A pad, its long axis through `centre` along `axis` over `length` (nm).
    Pad {
        centre: (f64, f64),
        axis: (f64, f64),
        length: f64,
    },
    /// A via's ring on one layer.
    Via {
        centre: (f64, f64),
        size: f64,
    },
    Fill,
}

#[derive(Clone, Debug)]
struct Piece {
    net: usize,
    layer: usize,
    shapes: Shapes,
    bounds: (i64, i64, i64, i64),
    kind: Kind,
    /// Network nodes: a track's two ends, else the node twice.
    nodes: (usize, usize),
}

impl Piece {
    fn stations(&self) -> bool {
        !matches!(self.kind, Kind::Fill)
    }

    /// Weights of the piece's two nodes at a point (nm) along it.
    fn weights_at(&self, p: (f64, f64)) -> f64 {
        match self.kind {
            Kind::Track { a, b, .. } => {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let l2 = dx * dx + dy * dy;
                if l2 <= 0.0 { 0.5 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) }
            }
            _ => 0.0,
        }
    }
}

/// The heights of one copper layer's copper, metres (`z` up, `F.Cu` at its dielectric's top).
#[derive(Clone, Copy, Debug)]
struct Level {
    z0: f64,
    z1: f64,
}

fn bounds_of(shapes: &Shapes) -> Option<(i64, i64, i64, i64)> {
    shapes.iter().map(|s| poly::bounds(s)).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// The parameter intervals `[u0, u1]` (nm) along `p + u n`, `|u| <= reach`, inside `shapes` (even-odd over every
/// contour of each shape).
fn line_intervals(shapes: &Shapes, p: (f64, f64), n: (f64, f64), reach: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for shape in shapes {
        let mut crossings: Vec<f64> = Vec::new();
        for contour in shape {
            let m = contour.len();
            for k in 0..m {
                let (a, b) = (&contour[k], &contour[(k + 1) % m]);
                let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
                // Side of each vertex relative to the line (cross product with n).
                let sa = (ax - p.0) * n.1 - (ay - p.1) * n.0;
                let sb = (bx - p.0) * n.1 - (by - p.1) * n.0;
                if (sa > 0.0) == (sb > 0.0) {
                    continue;
                }
                let t = sa / (sa - sb);
                let (x, y) = (ax + t * (bx - ax), ay + t * (by - ay));
                crossings.push((x - p.0) * n.0 + (y - p.1) * n.1);
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.chunks(2) {
            if let [u0, u1] = pair {
                let (u0, u1) = (u0.max(-reach), u1.min(reach));
                if u1 > u0 {
                    out.push((u0, u1));
                }
            }
        }
    }
    out
}

fn mm(v: f64) -> String {
    format!("{:.2}", v * 1e-6)
}

/// Union-find.
struct Sets(Vec<usize>);

impl Sets {
    fn root(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// A cross-section to solve, canonical: rectangles sorted, groups numbered by first appearance (0 the station's
/// piece, 1 the rest of its net).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CutKey(Vec<i64>);

/// Canonical group of a cut -> the piece it is and a point of it (nm); groups 0 and 1 have none.
type CutGroups = Vec<Option<(usize, (f64, f64))>>;
/// A node of the board: `(net index, node)`.
type NodeKey = (usize, usize);
/// A pair of nodes, the lower first.
type PairKey = (NodeKey, NodeKey);
/// A track segment: `(net, layer, a, b, width)`, nm.
type Segment = (usize, usize, (f64, f64), (f64, f64), f64);

/// The work of one station.
struct Station {
    piece: usize,
    /// Metres of the piece this station stands for.
    length: f64,
    /// Where it is (nm), for the nodes of the station's own piece.
    at: (f64, f64),
    key: CutKey,
    /// Canonical group -> (piece, point on it, nm); groups 0 and 1 have none.
    groups: Vec<Option<(usize, (f64, f64))>>,
    /// Whether this station's cut also gives the piece's inductance.
    inductance: bool,
}

struct Context<'a> {
    pieces: &'a [Piece],
    levels: &'a [Level],
    slabs: &'a [Slab],
    top: f64,
    bottom: f64,
    options: &'a ParasiticOptions,
}

impl Context<'_> {
    /// The cut of a station at `p` across direction `d` (unit) of piece `own`.
    fn cut(&self, own: usize, p: (f64, f64), d: (f64, f64)) -> Result<(CutKey, CutGroups), String> {
        let n = (-d.1, d.0);
        let reach = self.options.cut_half_width_m / NM;
        let (x0, x1) = ((p.0 - reach * n.0).min(p.0 + reach * n.0), (p.0 - reach * n.0).max(p.0 + reach * n.0));
        let (y0, y1) = ((p.1 - reach * n.1).min(p.1 + reach * n.1), (p.1 - reach * n.1).max(p.1 + reach * n.1));
        let own_net = self.pieces[own].net;
        // (layer, u0, u1, piece, is the station's own interval)
        let mut found: Vec<(usize, f64, f64, usize, bool)> = Vec::new();
        for (k, piece) in self.pieces.iter().enumerate() {
            let b = piece.bounds;
            if (b.2 as f64) < x0 || (b.0 as f64) > x1 || (b.3 as f64) < y0 || (b.1 as f64) > y1 {
                continue;
            }
            if let Kind::Track { a, b, .. } = piece.kind {
                let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                let l = ex.hypot(ey);
                if l > 0.0 && ((ex * d.0 + ey * d.1) / l).abs() < std::f64::consts::FRAC_1_SQRT_2 {
                    continue;
                }
            }
            for (u0, u1) in line_intervals(&piece.shapes, p, n, reach) {
                // A cut that grazes a round pad or ring meets a sliver of it: not a conductor of the cut.
                if u1 - u0 < MIN_INTERVAL_NM {
                    continue;
                }
                let centre = k == own && u0 <= 1.0 && u1 >= -1.0;
                found.push((piece.layer, u0, u1, k, centre));
            }
        }
        if !found.iter().any(|f| f.4) {
            return Err(format!("a station at {},{} mm does not lie on its own copper", mm(p.0), mm(p.1)));
        }
        // Different nets overlapping on one layer short each other.
        for (i, a) in found.iter().enumerate() {
            for b in &found[i + 1..] {
                let (na, nb) = (self.pieces[a.3].net, self.pieces[b.3].net);
                if a.0 == b.0 && na != nb && a.1.max(b.1) < a.2.min(b.2) - 1_000.0 {
                    return Err(format!(
                        "copper of two nets overlaps on one layer near {},{} mm: the layout shorts them",
                        mm(p.0 + 0.5 * (a.1.max(b.1) + a.2.min(b.2)) * n.0),
                        mm(p.1 + 0.5 * (a.1.max(b.1) + a.2.min(b.2)) * n.1)
                    ));
                }
            }
        }
        // Quantise to 0.1 um and sort; the station's own interval last so that it keeps its nodes where copper of
        // its net overlaps it.
        let q = |u: f64| (u / 100.0).round() as i64;
        found.sort_by_key(|a| (a.4, a.0, q(a.1), q(a.2), a.3));
        let mut groups: Vec<Option<(usize, (f64, f64))>> = vec![None, None];
        let mut by_piece: BTreeMap<usize, i64> = BTreeMap::new();
        let mut key = vec![found.len() as i64];
        for &(layer, u0, u1, piece, centre) in &found {
            let group = if centre {
                0
            } else if self.pieces[piece].net == own_net {
                1
            } else {
                *by_piece.entry(piece).or_insert_with(|| {
                    let u = 0.5 * (u0 + u1);
                    groups.push(Some((piece, (p.0 + u * n.0, p.1 + u * n.1))));
                    (groups.len() - 1) as i64
                })
            };
            key.extend([layer as i64, q(u0), q(u1), group]);
        }
        Ok((CutKey(key), groups))
    }

    /// The section of a key.
    fn section(&self, key: &CutKey) -> Section {
        let reach = self.options.cut_half_width_m;
        let mut conductors = Vec::new();
        for item in key.0[1..].chunks(4) {
            let level = self.levels[item[0] as usize];
            conductors.push(Rect {
                x0: item[1] as f64 * 100.0 * NM,
                x1: item[2] as f64 * 100.0 * NM,
                z0: level.z0,
                z1: level.z1,
                group: item[3] as usize,
            });
        }
        Section {
            conductors,
            slabs: self.slabs.to_vec(),
            bounds: [-reach, self.bottom - self.options.air_m, reach, self.top + self.options.air_m],
            sides: Wall::Grounded,
            ends: Wall::Grounded,
            mesh: self.options.mesh,
        }
    }
}

/// `f` over `items` on the machine's threads, in item order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = crate::linalg::parallelism().min(items.len()).max(1);
    let per = items.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let f = &f;
        let handles: Vec<_> =
            items.chunks(per).map(|chunk| scope.spawn(move || chunk.iter().map(f).collect::<Vec<R>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("a solver thread panicked")).collect()
    })
}

/// The charges with group 0 at 1 V and the rest at 0 V.
fn solve_driven(section: &Section) -> Result<crate::capacitance::Charges, String> {
    let mut potentials = vec![0.0; section.group_count().max(1)];
    potentials[0] = 1.0;
    section.solve(&potentials).map_err(|e| e.to_string())
}

/// The TEM self-inductance per metre of group 0 with the `signal` groups carrying no current and the others as
/// returns: `[C_ss^-1]_00 / c^2`, `C_ss` the vacuum capacitance matrix of the signal groups with the returns held at
/// 0 V. Also the share of group 0's charge on the box when it alone is at 1 V.
fn vacuum_inductance(section: &Section, signal: &[bool]) -> Result<(f64, f64), String> {
    let mut vacuum = section.in_vacuum();
    vacuum.mesh.extrapolate = false;
    let present: BTreeSet<usize> = vacuum.conductors.iter().map(|c| c.group).collect();
    let groups: Vec<usize> =
        present.into_iter().filter(|&g| g == 0 || signal.get(g).copied().unwrap_or(false)).collect();
    let n = groups.len();
    let mut matrix = vec![vec![0.0; n]; n];
    let mut box_share = 0.0;
    for (j, &g) in groups.iter().enumerate() {
        let mut potentials = vec![0.0; vacuum.group_count()];
        potentials[g] = 1.0;
        let q = vacuum.solve(&potentials).map_err(|e| e.to_string())?;
        for (i, &h) in groups.iter().enumerate() {
            matrix[i][j] = q.groups[h];
        }
        if g == 0 {
            box_share = -q.walls / q.groups[0];
        }
    }
    // [C^-1]_00 by Gauss-Jordan on the small dense matrix (symmetric positive definite).
    let mut e = vec![0.0; n];
    e[0] = 1.0;
    let mut a = matrix;
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs())).unwrap_or(col);
        a.swap(col, pivot);
        e.swap(col, pivot);
        let p = a[col][col];
        if p.abs() < f64::MIN_POSITIVE {
            return Err("a singular vacuum capacitance matrix".to_owned());
        }
        for row in 0..n {
            if row != col {
                let factor = a[row][col] / p;
                let pivot_row = a[col].clone();
                for (x, y) in a[row].iter_mut().zip(&pivot_row).skip(col) {
                    *x -= factor * y;
                }
                e[row] -= factor * e[col];
            }
        }
    }
    let inverse_00 = e[0] / a[0][0];
    Ok((inverse_00 / (SPEED_OF_LIGHT * SPEED_OF_LIGHT), box_share))
}

/// The copper levels (in the export's layer order) and the dielectric slabs, metres. A declared stack-up is walked
/// top down with each copper layer's own thickness, so that a dielectric's declared thickness is the distance
/// between the copper faces on either side of it (a dielectric above the first copper, solder mask, is left out).
/// Without one, the assumed heights of `copper_fullwave::stack` with 35 um copper, outer copper outside the
/// dielectric and inner copper centred on its interface. Returns whether a copper thickness was assumed.
fn stack_levels(
    board: &PhysicalBoard,
    stack: &copper_fullwave::stack::Stack,
    names: &[String],
) -> Result<(Vec<Level>, Vec<Slab>, bool), String> {
    let mut assumed = false;
    let mut by_name: BTreeMap<String, Level> = BTreeMap::new();
    let mut slabs: Vec<Slab> = Vec::new();
    if stack.declared {
        let mut z = 0.0f64;
        let mut started = false;
        for layer in &board.stackup.physical_layers {
            let thickness = layer.thickness_nm as f64 * NM;
            match layer.copper_layer {
                Some(copper) => {
                    let t = if thickness > 0.0 {
                        thickness
                    } else {
                        assumed = true;
                        35e-6
                    };
                    started = true;
                    by_name.insert(copper.value().to_owned(), Level { z0: z - t, z1: z });
                    z -= t;
                }
                None if started => {
                    let er = stack
                        .slabs
                        .iter()
                        .find(|s| s.id == layer.id)
                        .map(|s| s.er)
                        .ok_or_else(|| format!("stack-up dielectric {} has no permittivity", layer.id))?;
                    slabs.push(Slab { bottom: z - thickness, top: z, er });
                    z -= thickness;
                }
                None => {}
            }
        }
    } else {
        let n = stack.copper.len();
        for (index, (layer, z_mm)) in stack.copper.iter().enumerate() {
            let t = copper_thickness_m(board, *layer).unwrap_or_else(|| {
                assumed = true;
                35e-6
            });
            let z = z_mm * 1e-3;
            let level = if index == 0 {
                Level { z0: z, z1: z + t }
            } else if index + 1 == n {
                Level { z0: z - t, z1: z }
            } else {
                Level { z0: z - t / 2.0, z1: z + t / 2.0 }
            };
            by_name.insert(layer.value().to_owned(), level);
        }
        slabs = stack.slabs.iter().map(|s| Slab { bottom: s.bottom * 1e-3, top: s.top * 1e-3, er: s.er }).collect();
    }
    let levels = names
        .iter()
        .map(|name| {
            by_name
                .get(name)
                .copied()
                .ok_or_else(|| format!("copper layer {name} of the export is not in the stack-up"))
        })
        .collect::<Result<Vec<Level>, String>>()?;
    Ok((levels, slabs, assumed))
}

/// Extract the parasitics of `board` (its exported and filled `model`).
pub fn extract_parasitics(
    model: &BoardModel,
    board: &PhysicalBoard,
    options: &ParasiticOptions,
) -> Result<Parasitics, String> {
    let started = Instant::now();
    let cam = &model.cam;
    let stack = copper_fullwave::stack::stack(board).map_err(|e| e.to_string())?;
    let mut flags: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if !stack.declared {
        flags.push("assumed_stackup".to_owned());
    }
    // Copper levels and dielectric slabs, metres, z up with the top of the first copper at 0.
    let n_layers = cam.copper_count();
    let (levels, slabs, thickness_assumed) = stack_levels(board, &stack, &cam.copper_layers)?;
    if thickness_assumed {
        flags.push("assumed_copper_thickness".to_owned());
    }
    let top = levels.iter().map(|l| l.z1).fold(f64::NEG_INFINITY, f64::max);
    let bottom = levels.iter().map(|l| l.z0).fold(f64::INFINITY, f64::min);
    let er_between = |la: usize, lb: usize| -> (f64, f64) {
        // Distance between the two layers' facing copper and the series permittivity of what lies between, F/m.
        let (hi, lo) = if levels[la].z0 > levels[lb].z0 { (la, lb) } else { (lb, la) };
        let (z_top, z_bottom) = (levels[hi].z0, levels[lo].z1);
        let d = (z_top - z_bottom).max(1e-9);
        let mut series = 0.0;
        let mut covered = 0.0;
        for s in &slabs {
            let overlap = (s.top.min(z_top) - s.bottom.max(z_bottom)).max(0.0);
            series += overlap / s.er;
            covered += overlap;
        }
        series += (d - covered).max(0.0);
        (d, EPSILON_0 * d / series)
    };
    // Nets.
    let mut net_names: BTreeSet<String> = BTreeSet::new();
    for t in &cam.tracks {
        net_names.extend(t.net.clone());
    }
    for (_, p) in cam.pads() {
        net_names.extend(p.net.clone());
    }
    for v in &cam.vias {
        net_names.extend(v.net.clone());
    }
    for f in &model.fills {
        net_names.extend(cam.zones[f.zone].net.clone());
    }
    net_names.retain(|n| !n.is_empty());
    let nets: Vec<String> = net_names.into_iter().collect();
    let net_of = |name: &str| nets.iter().position(|n| n == name);
    let networked: Vec<bool> = nets.iter().map(|n| options.networked.contains(n)).collect();
    // Nodes per net: index 0 is the whole net for a net that is not networked.
    let mut node_labels: Vec<Vec<String>> = nets.iter().map(|n| vec![n.clone()]).collect();
    let mut pad_nodes: Vec<Vec<(String, String, usize)>> = vec![Vec::new(); nets.len()];
    let mut pieces: Vec<Piece> = Vec::new();
    let new_node = |labels: &mut Vec<Vec<String>>, net: usize, label: String| -> usize {
        if !networked[net] {
            return 0;
        }
        if labels[net].len() == 1 && labels[net][0] == nets[net] {
            labels[net][0] = label;
            return 0;
        }
        labels[net].push(label);
        labels[net].len() - 1
    };
    // Pads.
    let mut pad_list: Vec<(usize, usize, Shapes, Vec<usize>)> = Vec::new(); // (net, node, shape, layers)
    for (footprint, pad) in cam.pads() {
        let Some(net) = pad.net.as_deref().and_then(net_of) else { continue };
        let layers: Vec<usize> = (0..n_layers).filter(|&l| flashes(pad, l)).collect();
        if layers.is_empty() {
            continue;
        }
        let node = new_node(&mut node_labels, net, format!("pad {}.{}", footprint.reference, pad.number));
        pad_nodes[net].push((footprint.reference.clone(), pad.number.clone(), node));
        let shape = pad_shape(pad, MAX_ERROR);
        let (w, h) = match pad.shape {
            copper_cam::board::PadShape::Circle => (pad.width as f64, pad.width as f64),
            _ => (pad.width as f64, pad.height as f64),
        };
        let angle = -pad.angle.to_radians();
        let (along, length) = if w >= h { ((angle.cos(), angle.sin()), w) } else { ((-angle.sin(), angle.cos()), h) };
        // The axis of the pad's long side, from its outline (the export's angle convention aside).
        let axis = {
            let b = bounds_of(&shape).unwrap_or((0, 0, 1, 1));
            let (bw, bh) = ((b.2 - b.0) as f64, (b.3 - b.1) as f64);
            if (bw - bh).abs() < 1.0 {
                along
            } else if bw > bh {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            }
        };
        let length = {
            let b = bounds_of(&shape).unwrap_or((0, 0, 1, 1));
            let (bw, bh) = ((b.2 - b.0) as f64, (b.3 - b.1) as f64);
            if axis == (1.0, 0.0) {
                bw
            } else if axis == (0.0, 1.0) {
                bh
            } else {
                length
            }
        };
        let centre = (pad.center.x as f64, pad.center.y as f64);
        for &layer in &layers {
            pieces.push(Piece {
                net,
                layer,
                bounds: bounds_of(&shape).unwrap_or((0, 0, 0, 0)),
                shapes: shape.clone(),
                kind: Kind::Pad { centre, axis, length },
                nodes: (node, node),
            });
        }
        pad_list.push((net, node, shape, layers));
    }
    // Vias: a ring on every layer.
    let mut via_list: Vec<(usize, usize, (f64, f64), f64)> = Vec::new();
    for via in &cam.vias {
        let Some(net) = via.net.as_deref().and_then(net_of) else { continue };
        let centre = (via.center.x as f64, via.center.y as f64);
        let node = new_node(&mut node_labels, net, format!("via {},{} mm", mm(centre.0), mm(centre.1)));
        let ring = poly::circle(via.center, via.size as f64 / 2.0, MAX_ERROR);
        for layer in 0..n_layers {
            pieces.push(Piece {
                net,
                layer,
                bounds: bounds_of(&ring).unwrap_or((0, 0, 0, 0)),
                shapes: ring.clone(),
                kind: Kind::Via { centre, size: via.size as f64 },
                nodes: (node, node),
            });
        }
        via_list.push((net, node, centre, via.size as f64));
    }
    // Fills: one node per piece of copper.
    let mut fill_list: Vec<(usize, usize, usize, Shapes)> = Vec::new(); // (net, layer, node, shape)
    for fill in &model.fills {
        let Some(net) = cam.zones[fill.zone].net.as_deref().and_then(net_of) else { continue };
        for shape in &fill.shapes {
            let shapes: Shapes = vec![shape.clone()];
            let node = new_node(&mut node_labels, net, format!("fill {}", cam.copper_layers[fill.layer]));
            pieces.push(Piece {
                net,
                layer: fill.layer,
                bounds: poly::bounds(shape),
                shapes: shapes.clone(),
                kind: Kind::Fill,
                nodes: (node, node),
            });
            fill_list.push((net, fill.layer, node, shapes));
        }
    }
    // Tracks: flattened, split at T-junctions, ends attached to pads, vias and fills of their net.
    let mut segments: Vec<Segment> = Vec::new();
    for track in &cam.tracks {
        let Some(net) = track.net.as_deref().and_then(net_of) else { continue };
        let points: Vec<(f64, f64)> = match &track.path {
            TrackPath::Line { start, end } => vec![(start.x as f64, start.y as f64), (end.x as f64, end.y as f64)],
            TrackPath::Arc { start, mid, end } => flatten_arc(*start, *mid, *end, MAX_ERROR),
        };
        for pair in points.windows(2) {
            if (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1) > 1.0 {
                segments.push((net, track.layer, pair[0], pair[1], track.width as f64));
            }
        }
    }
    let point_key =
        |layer: usize, p: (f64, f64)| (layer, (p.0 / 1_000.0).round() as i64, (p.1 / 1_000.0).round() as i64);
    let mut endpoint_count: BTreeMap<(usize, usize, i64, i64), usize> = BTreeMap::new();
    for &(net, layer, a, b, _) in &segments {
        for p in [a, b] {
            let k = point_key(layer, p);
            *endpoint_count.entry((net, k.0, k.1, k.2)).or_default() += 1;
        }
    }
    let attached = |net: usize, layer: usize, p: (f64, f64)| -> Option<usize> {
        for (n, node, shape, layers) in &pad_list {
            if *n == net && layers.contains(&layer) && poly::shapes_contain(shape, p) {
                return Some(*node);
            }
        }
        for (n, node, centre, size) in &via_list {
            if *n == net && (p.0 - centre.0).hypot(p.1 - centre.1) <= size / 2.0 {
                return Some(*node);
            }
        }
        for (n, l, node, shape) in &fill_list {
            if *n == net && *l == layer && poly::shapes_contain(shape, p) {
                return Some(*node);
            }
        }
        None
    };
    let mut splits: Vec<Vec<(f64, (f64, f64))>> = vec![Vec::new(); segments.len()];
    for (i, &(net, layer, a, b, _)) in segments.iter().enumerate() {
        for p in [a, b] {
            let k = point_key(layer, p);
            if attached(net, layer, p).is_some() || endpoint_count[&(net, k.0, k.1, k.2)] > 1 {
                continue;
            }
            for (j, &(nj, lj, aj, bj, wj)) in segments.iter().enumerate() {
                if j == i || nj != net || lj != layer {
                    continue;
                }
                let (dx, dy) = (bj.0 - aj.0, bj.1 - aj.1);
                let l2 = dx * dx + dy * dy;
                let t = ((p.0 - aj.0) * dx + (p.1 - aj.1) * dy) / l2;
                if t <= 1e-6 || t >= 1.0 - 1e-6 {
                    continue;
                }
                let (fx, fy) = (aj.0 + t * dx, aj.1 + t * dy);
                if (p.0 - fx).hypot(p.1 - fy) <= wj / 2.0 {
                    splits[j].push((t, p));
                    break;
                }
            }
        }
    }
    let mut point_nodes: BTreeMap<(usize, usize, i64, i64), usize> = BTreeMap::new();
    let mut track_pieces: Vec<usize> = Vec::new();
    for (i, &(net, layer, a, b, width)) in segments.iter().enumerate() {
        let mut cuts = splits[i].clone();
        cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut points = vec![a];
        points.extend(cuts.into_iter().map(|(_, p)| p));
        points.push(b);
        for pair in points.windows(2) {
            let (pa, pb) = (pair[0], pair[1]);
            if (pb.0 - pa.0).hypot(pb.1 - pa.1) <= 1.0 {
                continue;
            }
            let mut node_at = |p: (f64, f64), labels: &mut Vec<Vec<String>>| -> usize {
                if let Some(node) = attached(net, layer, p) {
                    return node;
                }
                let k = point_key(layer, p);
                if let Some(&node) = point_nodes.get(&(net, k.0, k.1, k.2)) {
                    return node;
                }
                let node =
                    new_node(labels, net, format!("track end {} {},{} mm", cam.copper_layers[layer], mm(p.0), mm(p.1)));
                point_nodes.insert((net, k.0, k.1, k.2), node);
                node
            };
            let na = node_at(pa, &mut node_labels);
            let nb = node_at(pb, &mut node_labels);
            let shapes = poly::polyline(&[pa, pb], width, false, MAX_ERROR);
            pieces.push(Piece {
                net,
                layer,
                bounds: bounds_of(&shapes).unwrap_or((0, 0, 0, 0)),
                shapes,
                kind: Kind::Track { a: pa, b: pb, width },
                nodes: (na, nb),
            });
            track_pieces.push(pieces.len() - 1);
        }
    }
    // Zero-ohm joins: vias in pads or fills, pads in fills of their net.
    let mut joins: Vec<(usize, usize, usize)> = Vec::new(); // (net, node, node)
    for &(net, node, centre, _) in &via_list {
        for layer in 0..n_layers {
            if let Some(other) = attached(net, layer, centre).filter(|&o| o != node) {
                joins.push((net, node, other));
            }
        }
    }
    for (net, node, shape, layers) in &pad_list {
        if let Some(b) = bounds_of(shape) {
            let centre = ((b.0 + b.2) as f64 / 2.0, (b.1 + b.3) as f64 / 2.0);
            for (n, l, fill_node, fill) in &fill_list {
                if n == net && layers.contains(l) && poly::shapes_contain(fill, centre) && fill_node != node {
                    joins.push((*net, *node, *fill_node));
                }
            }
        }
    }
    // Connectivity of each networked net's pads.
    for net in 0..nets.len() {
        if !networked[net] {
            continue;
        }
        let mut sets = Sets((0..node_labels[net].len()).collect());
        for &k in &track_pieces {
            if pieces[k].net == net {
                sets.join(pieces[k].nodes.0, pieces[k].nodes.1);
            }
        }
        for &(n, a, b) in &joins {
            if n == net {
                sets.join(a, b);
            }
        }
        let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (reference, number, node) in &pad_nodes[net] {
            let root = sets.root(*node);
            groups.entry(root).or_default().push(format!("{reference}.{number}"));
        }
        if groups.len() > 1 {
            let listed: Vec<String> = groups.values().map(|pads| pads.join(" ")).collect();
            return Err(format!(
                "net {}: its pads are in {} pieces of copper that no track, via or fill joins ({}): the layout is not fully routed",
                nets[net],
                groups.len(),
                listed.join(" | ")
            ));
        }
    }
    // Stations.
    let context = Context { pieces: &pieces, levels: &levels, slabs: &slabs, top, bottom, options };
    let pitch = options.station_pitch_m / NM;
    let inside_own = |piece: &Piece, p: (f64, f64)| -> bool {
        // A track or via station inside a pad or fill of its own net on its layer is that copper's.
        match piece.kind {
            Kind::Track { .. } | Kind::Via { .. } => {
                pad_list
                    .iter()
                    .any(|(n, _, s, l)| *n == piece.net && l.contains(&piece.layer) && poly::shapes_contain(s, p))
                    || fill_list
                        .iter()
                        .any(|(n, l, _, s)| *n == piece.net && *l == piece.layer && poly::shapes_contain(s, p))
            }
            Kind::Pad { .. } => {
                fill_list.iter().any(|(n, l, _, s)| *n == piece.net && *l == piece.layer && poly::shapes_contain(s, p))
            }
            Kind::Fill => true,
        }
    };
    let mut stations: Vec<Station> = Vec::new();
    for (k, piece) in pieces.iter().enumerate() {
        let (start, dir, length) = match piece.kind {
            Kind::Track { a, b, .. } => {
                let l = (b.0 - a.0).hypot(b.1 - a.1);
                (a, ((b.0 - a.0) / l, (b.1 - a.1) / l), l)
            }
            Kind::Pad { centre, axis, length } => {
                ((centre.0 - axis.0 * length / 2.0, centre.1 - axis.1 * length / 2.0), axis, length)
            }
            Kind::Via { centre, size } => ((centre.0 - size / 2.0, centre.1), (1.0, 0.0), size),
            Kind::Fill => continue,
        };
        let count = (length / pitch).ceil().max(1.0) as usize;
        let wanted_inductance = matches!(piece.kind, Kind::Track { .. }) && networked[piece.net];
        let mut first_kept: Option<usize> = None;
        for s in 0..count {
            // A tiny offset off the grid keeps the cut line off polygon vertices.
            let t = (s as f64 + 0.5) / count as f64;
            let at = (start.0 + dir.0 * length * t + 0.37, start.1 + dir.1 * length * t + 0.29);
            if inside_own(piece, at) {
                continue;
            }
            let (key, groups) = context.cut(k, at, dir)?;
            first_kept.get_or_insert(stations.len());
            stations.push(Station { piece: k, length: length * NM / count as f64, at, key, groups, inductance: false });
        }
        if wanted_inductance {
            match first_kept {
                Some(first) => {
                    // The middle kept station of the piece gives its inductance.
                    let kept = stations.len() - first;
                    stations[first + kept / 2].inductance = true;
                }
                None => {
                    // A track wholly inside its own pads: no station; its inductance from its bar.
                }
            }
        }
    }
    // Solve each distinct cut once.
    let mut keys: BTreeMap<CutKey, usize> = BTreeMap::new();
    for s in &stations {
        let next = keys.len();
        keys.entry(s.key.clone()).or_insert(next);
    }
    let mut ordered: Vec<(CutKey, usize)> = keys.iter().map(|(k, &i)| (k.clone(), i)).collect();
    ordered.sort_by_key(|(_, i)| *i);
    let sections: Vec<Section> = ordered.iter().map(|(k, _)| context.section(k)).collect();

    let charges = parallel_map(&sections, solve_driven).into_iter().collect::<Result<Vec<_>, String>>()?;
    // The inductance cuts: which groups are signals (copper of a networked net that is not a fill) and float.
    let signal_mask = |station: &Station| -> Vec<bool> {
        station
            .groups
            .iter()
            .enumerate()
            .map(|(g, target)| match target {
                None => g <= 1,
                Some((piece, _)) => networked[pieces[*piece].net] && pieces[*piece].stations(),
            })
            .collect()
    };
    let mut vacuum_keys: BTreeMap<(CutKey, Vec<bool>), usize> = BTreeMap::new();
    for s in stations.iter().filter(|s| s.inductance) {
        let next = vacuum_keys.len();
        vacuum_keys.entry((s.key.clone(), signal_mask(s))).or_insert(next);
    }
    let mut vacuum_ordered: Vec<((CutKey, Vec<bool>), usize)> =
        vacuum_keys.iter().map(|(k, &i)| (k.clone(), i)).collect();
    vacuum_ordered.sort_by_key(|(_, i)| *i);
    let vacuum_inputs: Vec<(Section, Vec<bool>)> =
        vacuum_ordered.iter().map(|((k, mask), _)| (context.section(k), mask.clone())).collect();
    let vacuum = parallel_map(&vacuum_inputs, |(section, mask)| vacuum_inductance(section, mask))
        .into_iter()
        .collect::<Result<Vec<_>, String>>()?;
    // Accumulate.
    let mut sums: BTreeMap<PairKey, f64> = BTreeMap::new();
    let mut add = |a: (usize, usize), b: (usize, usize), farad: f64| {
        if a.0 == b.0 || farad <= 0.0 {
            return;
        }
        let key = if a <= b { (a, b) } else { (b, a) };
        *sums.entry(key).or_default() += farad;
    };
    let mut unattributed = vec![0.0f64; nets.len()];
    let split = |piece: &Piece, p: (f64, f64)| -> [(usize, f64); 2] {
        let t = piece.weights_at(p);
        [(piece.nodes.0, 1.0 - t), (piece.nodes.1, t)]
    };
    let mut inductance_per_m: BTreeMap<usize, f64> = BTreeMap::new();
    let mut without_return = 0usize;
    for station in &stations {
        let own = &pieces[station.piece];
        let q = &charges[keys[&station.key]];
        for (g, target) in station.groups.iter().enumerate() {
            let Some((other, at)) = target else { continue };
            let c = -q.groups.get(g).copied().unwrap_or(0.0) * station.length;
            if c <= 0.0 {
                continue;
            }
            let other_piece = &pieces[*other];
            let weight = if other_piece.stations() { 0.5 } else { 1.0 };
            for (na, wa) in split(own, station.at) {
                for (nb, wb) in split(other_piece, *at) {
                    add((own.net, na), (other_piece.net, nb), c * weight * wa * wb);
                }
            }
        }
        unattributed[own.net] += -q.walls * station.length;
        if station.inductance {
            let (per_m, box_share) = vacuum[vacuum_keys[&(station.key.clone(), signal_mask(station))]];
            if box_share > 0.5 {
                without_return += 1;
            } else {
                inductance_per_m.insert(station.piece, per_m);
            }
        }
    }
    // Crossings of tracks of different nets on different layers.
    let mut crossings = 0usize;
    for (i, &ka) in track_pieces.iter().enumerate() {
        for &kb in &track_pieces[i + 1..] {
            let (pa, pb) = (&pieces[ka], &pieces[kb]);
            if pa.net == pb.net || pa.layer == pb.layer {
                continue;
            }
            let (Kind::Track { a: a0, b: a1, width: wa }, Kind::Track { a: b0, b: b1, width: wb }) = (pa.kind, pb.kind)
            else {
                continue;
            };
            let (da, db) = ((a1.0 - a0.0, a1.1 - a0.1), (b1.0 - b0.0, b1.1 - b0.1));
            let (la, lb) = (da.0.hypot(da.1), db.0.hypot(db.1));
            let cross = da.0 * db.1 - da.1 * db.0;
            let sin = (cross / (la * lb)).abs();
            if sin < std::f64::consts::FRAC_1_SQRT_2 {
                continue;
            }
            let (ex, ey) = (b0.0 - a0.0, b0.1 - a0.1);
            let s = (ex * db.1 - ey * db.0) / cross;
            let u = (ex * da.1 - ey * da.0) / cross;
            if !(0.0..=1.0).contains(&s) || !(0.0..=1.0).contains(&u) {
                continue;
            }
            let point = (a0.0 + s * da.0, a0.1 + s * da.1);
            let (lo, hi) = (pa.layer.min(pb.layer), pa.layer.max(pb.layer));
            let shielded =
                fill_list.iter().any(|(_, l, _, shape)| *l > lo && *l < hi && poly::shapes_contain(shape, point));
            if shielded {
                continue;
            }
            let (d, eps) = er_between(pa.layer, pb.layer);
            let widen = |w: f64| closed_form::strip_over_plane_capacitance(w * NM / d, 1.0) * d / EPSILON_0;
            let c = eps * widen(wa) * widen(wb) / (d * sin);
            crossings += 1;
            for (na, wa_) in split(pa, point) {
                for (nb, wb_) in split(pb, point) {
                    add((pa.net, na), (pb.net, nb), c * wa_ * wb_);
                }
            }
        }
    }
    // Overlapping fills of different nets on different layers, where no copper lies between.
    for (i, (na, la, node_a, sa)) in fill_list.iter().enumerate() {
        for (nb, lb, node_b, sb) in &fill_list[i + 1..] {
            if na == nb || la == lb {
                continue;
            }
            let mut overlap = poly::intersection(sa, sb);
            if overlap.is_empty() {
                continue;
            }
            let (lo, hi) = ((*la).min(*lb), (*la).max(*lb));
            for (_, l, _, s) in &fill_list {
                if *l > lo && *l < hi {
                    overlap = poly::difference(&overlap, s);
                }
            }
            let area: f64 = overlap.iter().map(|s| poly::shape_area(s)).sum::<f64>() * NM * NM;
            if area <= 0.0 {
                continue;
            }
            let (d, eps) = er_between(*la, *lb);
            add((*na, *node_a), (*nb, *node_b), eps * area / d);
        }
    }
    if without_return > 0 {
        flags.push("inductance_without_return".to_owned());
        notes.push(format!(
            "{without_return} track pieces have no return copper within {} mm of them: their inductance is the bar's partial self-inductance",
            options.cut_half_width_m * 1e3
        ));
    }
    // Branches.
    let resistivity = crate::layout::COPPER_RESISTIVITY;
    let mut branches: Vec<Vec<Branch>> = vec![Vec::new(); nets.len()];
    let mut track_length = vec![0.0f64; nets.len()];
    for &k in &track_pieces {
        let piece = &pieces[k];
        let Kind::Track { a, b, width } = piece.kind else { continue };
        let length = (b.0 - a.0).hypot(b.1 - a.1) * NM;
        track_length[piece.net] += length;
        if !networked[piece.net] || piece.nodes.0 == piece.nodes.1 {
            continue;
        }
        let t = levels[piece.layer].z1 - levels[piece.layer].z0;
        let resistance = copper_emi::inductance::dc_resistance(length, width * NM, t, resistivity);
        let inductance = match inductance_per_m.get(&k) {
            Some(per_m) => per_m * length,
            None => copper_emi::inductance::self_inductance_bar(length, width * NM, t),
        };
        branches[piece.net].push(Branch {
            a: piece.nodes.0,
            b: piece.nodes.1,
            resistance,
            inductance,
            length_m: length,
            pieces: 1,
        });
    }
    // Merge the zero-ohm joins.
    let mut merged: Vec<Vec<usize>> = Vec::new();
    for (net, labels) in node_labels.iter().enumerate() {
        let mut sets = Sets((0..labels.len()).collect());
        for &(n, a, b) in &joins {
            if n == net {
                sets.join(a, b);
            }
        }
        merged.push((0..labels.len()).map(|x| sets.root(x)).collect());
    }
    let mut couplings_by_node: BTreeMap<PairKey, f64> = BTreeMap::new();
    for (((na, a), (nb, b)), f) in sums {
        let (a, b) = ((na, merged[na][a]), (nb, merged[nb][b]));
        let key = if a <= b { (a, b) } else { (b, a) };
        *couplings_by_node.entry(key).or_default() += f;
    }
    for (net, list) in branches.iter_mut().enumerate() {
        for branch in list.iter_mut() {
            branch.a = merged[net][branch.a];
            branch.b = merged[net][branch.b];
        }
        list.retain(|b| b.a != b.b);
    }
    for (net, pads) in pad_nodes.iter_mut().enumerate() {
        for pad in pads.iter_mut() {
            pad.2 = merged[net][pad.2];
        }
    }
    // Reduce each networked net: fold dead ends, merge series chains (pads are kept).
    for net in 0..nets.len() {
        if !networked[net] {
            continue;
        }
        let terminals: BTreeSet<usize> = pad_nodes[net].iter().map(|p| p.2).collect();
        loop {
            let mut degree: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
            for (i, b) in branches[net].iter().enumerate() {
                degree.entry(b.a).or_default().push(i);
                degree.entry(b.b).or_default().push(i);
            }
            let candidate = degree
                .iter()
                .find(|(node, list)| !terminals.contains(node) && (list.len() == 1 || list.len() == 2))
                .map(|(node, list)| (*node, list.clone()));
            let Some((node, list)) = candidate else { break };
            let other = |b: &Branch| if b.a == node { b.b } else { b.a };
            // Where the node's capacitances go: the neighbour (dead end), or half to each neighbour.
            let targets: Vec<(usize, f64)> = if list.len() == 1 {
                vec![(other(&branches[net][list[0]]), 1.0)]
            } else {
                vec![(other(&branches[net][list[0]]), 0.5), (other(&branches[net][list[1]]), 0.5)]
            };
            let moved: Vec<(PairKey, f64)> = couplings_by_node
                .iter()
                .filter(|((a, b), _)| *a == (net, node) || *b == (net, node))
                .map(|(k, f)| (*k, *f))
                .collect();
            for (key, f) in moved {
                couplings_by_node.remove(&key);
                let far = if key.0 == (net, node) { key.1 } else { key.0 };
                for &(target, w) in &targets {
                    let near = (net, target);
                    let k = if near <= far { (near, far) } else { (far, near) };
                    *couplings_by_node.entry(k).or_default() += f * w;
                }
            }
            if list.len() == 1 {
                branches[net].remove(list[0]);
            } else {
                let (first, second) = (branches[net][list[0]].clone(), branches[net][list[1]].clone());
                let joined = Branch {
                    a: other(&first),
                    b: other(&second),
                    resistance: first.resistance + second.resistance,
                    inductance: first.inductance + second.inductance,
                    length_m: first.length_m + second.length_m,
                    pieces: first.pieces + second.pieces,
                };
                let (hi, lo) = (list[0].max(list[1]), list[0].min(list[1]));
                branches[net].remove(hi);
                branches[net].remove(lo);
                if joined.a != joined.b {
                    branches[net].push(joined);
                }
            }
        }
    }
    // Renumber the nodes that remain.
    let mut result_nets: Vec<NetParasitics> = Vec::new();
    let mut renumber: Vec<BTreeMap<usize, usize>> = Vec::new();
    for net in 0..nets.len() {
        let mut used: BTreeSet<usize> = BTreeSet::new();
        if networked[net] {
            used.extend(pad_nodes[net].iter().map(|p| p.2));
            for b in &branches[net] {
                used.insert(b.a);
                used.insert(b.b);
            }
        }
        if used.is_empty() {
            used.insert(0);
        }
        let map: BTreeMap<usize, usize> = used.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let nodes = used
            .iter()
            .map(|&n| NetNode { label: if networked[net] { node_labels[net][n].clone() } else { nets[net].clone() } })
            .collect();
        let pads = pad_nodes[net]
            .iter()
            .map(|(r, p, n)| (r.clone(), p.clone(), if networked[net] { map[n] } else { 0 }))
            .collect();
        let list = branches[net].iter().map(|b| Branch { a: map[&b.a], b: map[&b.b], ..b.clone() }).collect();
        result_nets.push(NetParasitics {
            net: nets[net].clone(),
            nodes,
            pads,
            branches: list,
            track_length_m: track_length[net],
            unattributed: unattributed[net],
        });
        renumber.push(map);
    }
    let couplings: Vec<Coupling> = couplings_by_node
        .into_iter()
        .filter(|(_, f)| *f > 0.0)
        .map(|(((na, a), (nb, b)), farad)| {
            let node =
                |net: usize, n: usize| if networked[net] { renumber[net].get(&n).copied().unwrap_or(0) } else { 0 };
            Coupling { a: (na, node(na, a)), b: (nb, node(nb, b)), farad }
        })
        .collect();
    // Couplings that now land on one node pair are summed.
    let mut summed: BTreeMap<PairKey, f64> = BTreeMap::new();
    for c in couplings {
        *summed.entry((c.a, c.b)).or_default() += c.farad;
    }
    let couplings = summed.into_iter().map(|((a, b), farad)| Coupling { a, b, farad }).collect();
    notes.push(format!(
        "{} stations, {} distinct cross-sections solved, {crossings} crossings estimated",
        stations.len(),
        sections.len()
    ));
    Ok(Parasitics {
        nets: result_nets,
        couplings,
        flags,
        notes,
        stations: stations.len(),
        sections: sections.len(),
        crossings,
        seconds: started.elapsed().as_secs_f64(),
    })
}

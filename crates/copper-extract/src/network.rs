//! The partial-element network and its loop (mesh) solution.
//!
//! A [`Network`] is a graph of nodes joined by filaments (branches). Every branch has a resistance and a place in
//! space; the branches are coupled by the dense partial inductance matrix `L`. The branch impedance matrix at
//! angular frequency `w` is `Z_b = R + j w L`.
//!
//! **Formulation: loop analysis.** A spanning forest of the graph fixes `n_b - n_n + n_c` independent loops
//! (the fundamental cycles of the non-tree branches; `n_b` branches, `n_n` nodes, `n_c` connected components).
//! With `B` the loop-by-branch incidence matrix (entries `+-1`) the loop currents `m` solve
//! `B Z_b B^T m = -B Z_b b_p`, where `b_p` is the branch current of a unit current injected at the positive
//! terminal of a port and withdrawn at its negative terminal along the tree path between them. The complete branch
//! currents are `b = b_p + B^T m`. The port voltage is the sum of branch voltage drops `Z_b b` along that path, so
//! the port impedance matrix is `Z_ij = sum_{k in path_i} s_k (Z_b b_j)_k`.
//!
//! Compared with the nodal form `A Z_b^{-1} A^T` this works on a system of the number of loops (about half the number
//! of filaments of a planar mesh) and never inverts `Z_b`. `B L B^T` and `B R B^T` do not depend on frequency and
//! are built once; a frequency costs one dense complex elimination of the loop matrix.
//!
//! Loops inside conductors that touch no port are kept: a floating island or a floating plane piece carries induced
//! (eddy) loop currents like any other loop, as in FastHenry. Nothing here is capacitive: this is the
//! magneto-quasi-static model and the displacement current is not in it.

use std::collections::BTreeMap;

use copper_emi::spectrum::Complex;

use crate::linalg::{CMatrix, Singular};
use crate::partial::{self, Bar};

/// What a branch stands for, for the reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BranchKind {
    /// A bar of copper on layer `layer` (index into the layout).
    Plane { layer: usize },
    /// A segment of via `via` (index into the layout's vias); a through-hole pad's barrel is a via.
    Barrel { via: usize },
    /// A bar of a hand-built network.
    Free,
}

/// One filament.
#[derive(Clone, Debug)]
pub struct Branch {
    pub a: usize,
    pub b: usize,
    pub bar: Bar,
    pub resistance: f64,
    pub kind: BranchKind,
    /// The bar this filament belongs to (filaments that share a bar are in parallel and share its two end nodes).
    pub group: usize,
}

/// A port: the node sets of the positive and the negative terminal. The nodes of a set are one equipotential
/// terminal (a pad), as FastHenry's `.equiv`.
#[derive(Clone, Debug)]
pub struct NodePort {
    pub plus: Vec<usize>,
    pub minus: Vec<usize>,
}

/// The graph.
#[derive(Clone, Debug, Default)]
pub struct Network {
    pub node_count: usize,
    pub branches: Vec<Branch>,
    /// Sets of nodes that are joined by a perfect conductor.
    pub shorts: Vec<Vec<usize>>,
    pub ports: Vec<NodePort>,
}

/// Failures of the solve.
#[derive(Clone, Debug, PartialEq)]
pub enum SolveError {
    /// More filaments than the budget.
    OverBudget { filaments: usize, limit: usize },
    /// The terminals of a port coincide (shorted by the other shorts) or the port has an empty terminal.
    PortShorted(usize),
    /// The two terminals of a port are not connected by copper.
    PortDisconnected(usize),
    /// The loop matrix is singular.
    Singular,
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::OverBudget { filaments, limit } => {
                write!(f, "{filaments} filaments exceed the budget of {limit}")
            }
            SolveError::PortShorted(p) => write!(f, "port {p}: the two terminals are the same node or empty"),
            SolveError::PortDisconnected(p) => write!(f, "port {p}: the two terminals are not connected by copper"),
            SolveError::Singular => write!(f, "the loop matrix is singular"),
        }
    }
}

impl std::error::Error for SolveError {}

impl From<Singular> for SolveError {
    fn from(_: Singular) -> Self {
        SolveError::Singular
    }
}

/// The solution at one frequency.
#[derive(Clone, Debug)]
pub struct Solution {
    pub frequency: f64,
    /// Port impedance matrix `Z[i][j]` = voltage at port `i` per ampere into port `j`, ohm.
    pub z: Vec<Vec<Complex>>,
    /// Branch currents for a unit current into each port (`currents[j][branch]`), ampere.
    pub currents: Vec<Vec<Complex>>,
}

impl Solution {
    /// `Re Z_ij`, ohm.
    pub fn resistance(&self, i: usize, j: usize) -> f64 {
        self.z[i][j].re
    }

    /// `Im Z_ij / w`, henry; `NaN` at zero frequency.
    pub fn inductance(&self, i: usize, j: usize) -> f64 {
        self.z[i][j].im / (2.0 * std::f64::consts::PI * self.frequency)
    }
}

/// Prepared loop system of a network.
pub struct Solver {
    nb: usize,
    nl: usize,
    resistance: Vec<f64>,
    /// Partial inductance matrix, `nb x nb`, row major.
    inductance: Vec<f64>,
    /// Loops as sparse rows `(branch, sign)`.
    loops: Vec<Vec<(usize, f64)>>,
    /// `B R B^T` and `B L B^T`, `nl x nl`.
    br: Vec<f64>,
    bl: Vec<f64>,
    /// Port paths through the spanning forest, `(branch, sign)` from the positive to the negative terminal.
    paths: Vec<Vec<(usize, f64)>>,
    /// Branches whose rows of `Z_b b` any port needs.
    path_rows: Vec<usize>,
    /// Number of connected components, nodes after merging, loops.
    pub components: usize,
    pub merged_nodes: usize,
}

struct Dsu {
    parent: Vec<usize>,
}

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu { parent: (0..n).collect() }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

impl Solver {
    /// Build the loop system. `limit` bounds the number of filaments (the inductance matrix is dense).
    pub fn new(network: &Network, limit: usize) -> Result<Solver, SolveError> {
        let nb = network.branches.len();
        if nb > limit {
            return Err(SolveError::OverBudget { filaments: nb, limit });
        }
        // Merge nodes: shorts and the terminals of every port.
        let mut dsu = Dsu::new(network.node_count);
        for set in &network.shorts {
            for w in set.windows(2) {
                dsu.union(w[0], w[1]);
            }
        }
        for port in &network.ports {
            for set in [&port.plus, &port.minus] {
                for w in set.windows(2) {
                    dsu.union(w[0], w[1]);
                }
            }
        }
        // Dense renumbering of the representatives.
        let mut index: BTreeMap<usize, usize> = BTreeMap::new();
        let mut node_of = vec![0usize; network.node_count];
        for (n, slot) in node_of.iter_mut().enumerate() {
            let r = dsu.find(n);
            let next = index.len();
            *slot = *index.entry(r).or_insert(next);
        }
        let nn = index.len();
        let ends: Vec<(usize, usize)> = network.branches.iter().map(|b| (node_of[b.a], node_of[b.b])).collect();
        // Spanning forest by breadth first search; adjacency in branch order for determinism.
        let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nn];
        for (k, &(a, b)) in ends.iter().enumerate() {
            adjacency[a].push((k, b));
            adjacency[b].push((k, a));
        }
        let mut parent: Vec<Option<(usize, usize)>> = vec![None; nn]; // (parent node, branch)
        let mut depth = vec![0usize; nn];
        let mut component = vec![usize::MAX; nn];
        let mut in_tree = vec![false; nb];
        let mut components = 0;
        for root in 0..nn {
            if component[root] != usize::MAX {
                continue;
            }
            component[root] = components;
            let mut queue = std::collections::VecDeque::from([root]);
            while let Some(u) = queue.pop_front() {
                for &(k, v) in &adjacency[u] {
                    if component[v] == usize::MAX {
                        component[v] = components;
                        parent[v] = Some((u, k));
                        depth[v] = depth[u] + 1;
                        in_tree[k] = true;
                        queue.push_back(v);
                    }
                }
            }
            components += 1;
        }
        // Sign of traversing branch k from node `from` towards its other end: +1 when the branch is oriented that
        // way (a -> b).
        let sign_from = |k: usize, from: usize| if ends[k].0 == from { 1.0 } else { -1.0 };
        // Path between two nodes of one tree: up from `u` to the common ancestor, down to `v`.
        let tree_path = |u: usize, v: usize| -> Option<Vec<(usize, f64)>> {
            if component[u] != component[v] {
                return None;
            }
            let (mut x, mut y) = (u, v);
            let mut up = Vec::new();
            let mut down = Vec::new();
            while depth[x] > depth[y] {
                let (p, k) = parent[x].expect("not root");
                up.push((k, sign_from(k, x)));
                x = p;
            }
            while depth[y] > depth[x] {
                let (p, k) = parent[y].expect("not root");
                // Walking down from p to y: traversed from p.
                down.push((k, sign_from(k, p)));
                y = p;
            }
            while x != y {
                let (p, k) = parent[x].expect("not root");
                up.push((k, sign_from(k, x)));
                x = p;
                let (p, k) = parent[y].expect("not root");
                down.push((k, sign_from(k, p)));
                y = p;
            }
            down.reverse();
            up.extend(down);
            Some(up)
        };
        // Fundamental loops: each non-tree branch a -> b closed through the tree path from b back to a.
        let mut loops: Vec<Vec<(usize, f64)>> = Vec::new();
        for (k, &(a, b)) in ends.iter().enumerate() {
            if in_tree[k] {
                continue;
            }
            let mut lp = vec![(k, 1.0)];
            if a != b {
                lp.extend(tree_path(b, a).expect("same component"));
            }
            loops.push(lp);
        }
        let nl = loops.len();
        // Ports.
        let mut paths = Vec::new();
        for (i, port) in network.ports.iter().enumerate() {
            let (Some(&p), Some(&q)) = (port.plus.first(), port.minus.first()) else {
                return Err(SolveError::PortShorted(i));
            };
            let (p, q) = (node_of[p], node_of[q]);
            if p == q {
                return Err(SolveError::PortShorted(i));
            }
            paths.push(tree_path(p, q).ok_or(SolveError::PortDisconnected(i))?);
        }
        let mut path_rows: Vec<usize> = paths.iter().flatten().map(|&(k, _)| k).collect();
        path_rows.sort_unstable();
        path_rows.dedup();
        // Partial inductance matrix.
        let mut inductance = vec![0.0; nb * nb];
        for i in 0..nb {
            inductance[i * nb + i] = partial::self_inductance(&network.branches[i].bar);
            for j in (i + 1)..nb {
                let m = partial::mutual(&network.branches[i].bar, &network.branches[j].bar);
                inductance[i * nb + j] = m;
                inductance[j * nb + i] = m;
            }
        }
        let resistance: Vec<f64> = network.branches.iter().map(|b| b.resistance).collect();
        // B L B^T = (B L) B^T.
        let mut bl_rows = vec![0.0; nl * nb];
        for (l, lp) in loops.iter().enumerate() {
            let row = &mut bl_rows[l * nb..(l + 1) * nb];
            for &(k, s) in lp {
                let src = &inductance[k * nb..(k + 1) * nb];
                for (dst, v) in row.iter_mut().zip(src) {
                    *dst += s * v;
                }
            }
        }
        let mut bl = vec![0.0; nl * nl];
        let mut br = vec![0.0; nl * nl];
        for (l2, lp2) in loops.iter().enumerate() {
            for l1 in 0..nl {
                let row = &bl_rows[l1 * nb..(l1 + 1) * nb];
                let mut acc = 0.0;
                for &(k, s) in lp2 {
                    acc += s * row[k];
                }
                bl[l1 * nl + l2] = acc;
            }
        }
        // B R B^T: loops share a branch only through it.
        let mut users: Vec<Vec<(usize, f64)>> = vec![Vec::new(); nb];
        for (l, lp) in loops.iter().enumerate() {
            for &(k, s) in lp {
                users[k].push((l, s));
            }
        }
        for (k, list) in users.iter().enumerate() {
            for &(l1, s1) in list {
                for &(l2, s2) in list {
                    br[l1 * nl + l2] += s1 * s2 * resistance[k];
                }
            }
        }
        Ok(Solver { nb, nl, resistance, inductance, loops, br, bl, paths, path_rows, components, merged_nodes: nn })
    }

    pub fn filaments(&self) -> usize {
        self.nb
    }

    pub fn loop_count(&self) -> usize {
        self.nl
    }

    pub fn port_count(&self) -> usize {
        self.paths.len()
    }

    /// Solve at `frequency` (Hz, `>= 0`). `currents` asks for the branch currents of every port excitation.
    pub fn solve(&self, frequency: f64, currents: bool) -> Result<Solution, SolveError> {
        let w = 2.0 * std::f64::consts::PI * frequency;
        let (nb, nl, np) = (self.nb, self.nl, self.paths.len());
        let jw = Complex::new(0.0, w);
        // Particular branch currents and their voltages, per port.
        let mut rhs: Vec<Vec<Complex>> = Vec::with_capacity(np);
        for path in &self.paths {
            // Z_b b_p, dense over all branches.
            let mut zb = vec![Complex::ZERO; nb];
            for &(k, s) in path {
                zb[k] = zb[k] + Complex::new(self.resistance[k] * s, 0.0);
                let column = &self.inductance[k * nb..(k + 1) * nb]; // symmetric: row k = column k
                for (z, l) in zb.iter_mut().zip(column) {
                    *z = *z + Complex::new(0.0, w * s * l);
                }
            }
            let mut column = vec![Complex::ZERO; nl];
            for (l, lp) in self.loops.iter().enumerate() {
                let mut acc = Complex::ZERO;
                for &(k, s) in lp {
                    acc = acc + zb[k].scale(s);
                }
                column[l] = -acc;
            }
            rhs.push(column);
        }
        if nl > 0 {
            CMatrix::from_planes(nl, &self.br, &self.bl, w).solve(&mut rhs)?;
        }
        let mut z = vec![vec![Complex::ZERO; np]; np];
        let mut all_currents = Vec::new();
        for j in 0..np {
            let mut b = vec![Complex::ZERO; nb];
            for &(k, s) in &self.paths[j] {
                b[k] = b[k] + Complex::new(s, 0.0);
            }
            for (l, lp) in self.loops.iter().enumerate() {
                let m = rhs[j][l];
                for &(k, s) in lp {
                    b[k] = b[k] + m.scale(s);
                }
            }
            // Voltage drops on the branches any port path uses.
            let mut drop = BTreeMap::new();
            for &k in &self.path_rows {
                let mut acc = Complex::new(self.resistance[k], 0.0) * b[k];
                let row = &self.inductance[k * nb..(k + 1) * nb];
                let (mut sr, mut si) = (0.0, 0.0);
                for (l, v) in row.iter().zip(&b) {
                    sr += l * v.re;
                    si += l * v.im;
                }
                acc = acc + jw * Complex::new(sr, si);
                drop.insert(k, acc);
            }
            for (i, path) in self.paths.iter().enumerate() {
                let mut v = Complex::ZERO;
                for &(k, s) in path {
                    v = v + drop[&k].scale(s);
                }
                z[i][j] = v;
            }
            if currents {
                all_currents.push(b);
            }
        }
        Ok(Solution { frequency, z, currents: all_currents })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partial::Axis;

    const RHO: f64 = 1.72e-8;

    fn bar_branch(a: usize, b: usize, bar: Bar, group: usize) -> Branch {
        Branch { a, b, resistance: partial::resistance(&bar, RHO, 0.0), bar, kind: BranchKind::Free, group }
    }

    /// Two parallel straight bars of unequal width between the same two nodes, driven as a port.
    fn two_bars(w1: f64, w2: f64, pitch: f64) -> (Network, Bar, Bar) {
        let a = Bar::planar(Axis::X, [5e-3, 0.0, 0.0], 10e-3, w1, 35e-6);
        let b = Bar::planar(Axis::X, [5e-3, pitch, 0.0], 10e-3, w2, 35e-6);
        let network = Network {
            node_count: 2,
            branches: vec![bar_branch(0, 1, a, 0), bar_branch(0, 1, b, 1)],
            shorts: vec![],
            ports: vec![NodePort { plus: vec![0], minus: vec![1] }],
        };
        (network, a, b)
    }

    #[test]
    fn two_parallel_bars_follow_the_two_by_two_hand_solution() {
        let (network, a, b) = two_bars(1e-3, 0.4e-3, 1.5e-3);
        let solver = Solver::new(&network, 100).expect("solver");
        assert_eq!(solver.loop_count(), 1);
        let (ra, rb) = (partial::resistance(&a, RHO, 0.0), partial::resistance(&b, RHO, 0.0));
        let (la, lb, m) = (partial::self_inductance(&a), partial::self_inductance(&b), partial::mutual(&a, &b));
        for f in [0.0, 1e3, 1e6, 1e8, 1e9] {
            let w = 2.0 * std::f64::consts::PI * f;
            // Hand solution: Z matrix [[ra + jw la, jw m], [jw m, rb + jw lb]], same voltage across both bars.
            let (z11, z22, z12) = (Complex::new(ra, w * la), Complex::new(rb, w * lb), Complex::new(0.0, w * m));
            let det = z11 * z22 - z12 * z12;
            // I1 = V (z22 - z12)/det, I2 = V (z11 - z12)/det, I = I1 + I2, Z = V/I.
            let i1 = (z22 - z12) * det.recip();
            let i2 = (z11 - z12) * det.recip();
            let z = (i1 + i2).recip();
            let solved = solver.solve(f, true).expect("solve");
            let got = solved.z[0][0];
            assert!((got - z).abs() < 1e-9 * z.abs().max(1e-12), "f={f}: {got:?} vs {z:?}");
            // Current share of bar 1, from the solution and from the hand values.
            let share_hand = (i1 * (i1 + i2).recip()).abs();
            let c = &solved.currents[0];
            let share = (c[0] * (c[0] + c[1]).recip()).abs();
            assert!((share - share_hand).abs() < 1e-9, "f={f}: {share} vs {share_hand}");
        }
    }

    #[test]
    fn dc_share_follows_conductance_and_high_frequency_share_follows_inductance() {
        let (network, a, b) = two_bars(1e-3, 0.4e-3, 1.5e-3);
        let solver = Solver::new(&network, 100).expect("solver");
        let dc = solver.solve(0.0, true).expect("dc");
        let share_dc = dc.currents[0][0].re;
        let (ra, rb) = (partial::resistance(&a, RHO, 0.0), partial::resistance(&b, RHO, 0.0));
        assert!((share_dc - (1.0 / ra) / (1.0 / ra + 1.0 / rb)).abs() < 1e-12, "{share_dc}");
        // Far above the L/R corner the shares divide as the inverse inductances (with the mutual term).
        let hf = solver.solve(1e10, true).expect("hf");
        let c = &hf.currents[0];
        let share_hf = (c[0] * (c[0] + c[1]).recip()).abs();
        let (la, lb, m) = (partial::self_inductance(&a), partial::self_inductance(&b), partial::mutual(&a, &b));
        let want = ((lb - m) / (la + lb - 2.0 * m)).abs();
        assert!((share_hf - want).abs() < 2e-3, "{share_hf} vs {want}");
        assert!(
            (share_hf - share_dc).abs() > 0.05,
            "the shares differ between DC ({share_dc}) and high frequency ({share_hf})"
        );
    }

    #[test]
    fn a_port_across_a_shorted_pair_or_between_islands_is_refused() {
        let (mut network, _, _) = two_bars(1e-3, 1e-3, 2e-3);
        network.ports = vec![NodePort { plus: vec![0], minus: vec![0] }];
        assert_eq!(Solver::new(&network, 100).err(), Some(SolveError::PortShorted(0)));
        network.node_count = 3;
        network.ports = vec![NodePort { plus: vec![0], minus: vec![2] }];
        assert_eq!(Solver::new(&network, 100).err(), Some(SolveError::PortDisconnected(0)));
        let (network, _, _) = two_bars(1e-3, 1e-3, 2e-3);
        assert!(matches!(Solver::new(&network, 1), Err(SolveError::OverBudget { .. })));
    }

    #[test]
    fn a_floating_closed_loop_next_to_a_bar_shields_it() {
        // A bar with a floating shorted ring beside it: the eddy current in the ring lowers the port inductance.
        let a = Bar::planar(Axis::X, [5e-3, 0.0, 0.0], 10e-3, 1e-3, 35e-6);
        // Ring: two parallel bars closed at both ends by two short y bars, above the bar.
        let z = 0.3e-3;
        let top = Bar::planar(Axis::X, [5e-3, 0.0, z], 10e-3, 1e-3, 35e-6);
        let bottom = Bar::planar(Axis::X, [5e-3, 3e-3, z], 10e-3, 1e-3, 35e-6);
        let left = Bar::planar(Axis::Y, [0.0, 1.5e-3, z], 3e-3, 1e-3, 35e-6);
        let right = Bar::planar(Axis::Y, [10e-3, 1.5e-3, z], 3e-3, 1e-3, 35e-6);
        let mut network = Network {
            node_count: 2,
            branches: vec![bar_branch(0, 1, a, 0)],
            shorts: vec![],
            ports: vec![NodePort { plus: vec![0], minus: vec![1] }],
        };
        let alone = Solver::new(&network, 100).unwrap().solve(1e7, false).unwrap().inductance(0, 0);
        network.node_count = 6;
        network.branches.extend([
            bar_branch(2, 3, top, 1),
            bar_branch(4, 5, bottom, 2),
            bar_branch(2, 4, left, 3),
            bar_branch(3, 5, right, 4),
        ]);
        // Orient the y bars along +y: from the y = 0 end (top row nodes 2, 3) to the y = 3 mm end (4, 5).
        let shielded = Solver::new(&network, 100).unwrap().solve(1e7, false).unwrap().inductance(0, 0);
        assert!(shielded < alone, "{shielded} should be below {alone}");
    }
}

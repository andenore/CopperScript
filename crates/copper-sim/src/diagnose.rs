//! Naming the likely cause of a singular circuit matrix.
//!
//! Runs only after the factorisation failed, and looks at topology alone:
//!
//! 1. **No path to ground.** Nodes are joined by every element that relates
//!    the voltages of its terminals: resistors, inductors, voltage sources,
//!    VCVS and CCVS outputs, diodes and switches, and capacitors except at DC. Current sources and
//!    current-setting outputs (VCCS, CCCS) do not join their terminals, and
//!    control inputs draw no current. Each group of unknown nodes that is
//!    not joined to ground is one cause.
//! 2. **Voltage loops.** Voltage-defined elements (voltage sources, VCVS,
//!    CCVS, and inductors at DC) are added to a spanning forest one by one;
//!    one whose terminals are already joined closes a loop, reported as the
//!    forest path plus the closing element. The loop current of such a loop
//!    is undetermined whether or not its voltages agree.
//! 3. Otherwise the cause names the unknown whose row or pivot vanished.
//!
//! The topology rules can name a group that is not actually singular (a VCCS
//! controlled by its own output voltage acts as a conductance), which is why
//! they explain a failure instead of predicting one.

use crate::circuit::{Circuit, ElementKind, NodeId};
use crate::error::SingularCause;
use crate::lu::LuError;
use crate::mna::{Layout, Unknown};
use std::collections::VecDeque;

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    /// Join two sets; false if they were joined already.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        // Keep the smaller index as root so ground (0) stays a root.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.0[hi] = lo;
        true
    }
}

/// Likely causes of a singular matrix (never empty).
pub fn diagnose(c: &Circuit, layout: &Layout, dc: bool, failure: LuError) -> Vec<SingularCause> {
    let mut causes = floating_groups(c, layout, dc);
    causes.extend(voltage_loops(c, dc));
    if causes.is_empty() {
        let index = match failure {
            LuError::EmptyRow(i) | LuError::Singular(i) => Some(i),
            LuError::NonFinite => None,
        };
        let unknown = match index.and_then(|i| layout.unknowns.get(i)) {
            Some(Unknown::Node(n)) => format!("voltage of node {:?}", c.node_name(*n)),
            Some(Unknown::Branch(e)) => format!("branch current of {:?}", c.elements()[*e].name()),
            None => "matrix (non-finite entry)".to_string(),
        };
        causes.push(SingularCause::Unknown { unknown });
    }
    causes
}

fn floating_groups(c: &Circuit, layout: &Layout, dc: bool) -> Vec<SingularCause> {
    let mut uf = UnionFind::new(c.node_count());
    for e in c.elements() {
        let joins = match e.kind() {
            ElementKind::Resistor { .. }
            | ElementKind::Inductor { .. }
            | ElementKind::VoltageSource { .. }
            | ElementKind::Vcvs { .. }
            | ElementKind::Ccvs { .. }
            | ElementKind::Diode { .. }
            | ElementKind::Switch { .. } => true,
            ElementKind::Capacitor { .. } => !dc,
            // Junctions and channels conduct; behavioural current sources do not.
            ElementKind::Law { law, .. } => match &law.function {
                crate::behavioral::Function::Device(branch) => {
                    !matches!(**branch, crate::semiconductors::DeviceBranch::Depletion { .. }) || !dc
                }
                crate::behavioral::Function::Expr { .. } => law.output == crate::behavioral::Output::Voltage,
            },
            _ => false,
        };
        if joins && let Some((a, b)) = e.kind().terminals() {
            uf.union(a.index(), b.index());
        }
    }
    let ground = uf.find(0);
    let mut groups: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, row) in layout.node_row.iter().enumerate() {
        if row.is_none() {
            continue;
        }
        let root = uf.find(i);
        if root == ground {
            continue;
        }
        let name = c.node_name(NodeId(i)).to_string();
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, names)) => names.push(name),
            None => groups.push((root, vec![name])),
        }
    }
    groups.into_iter().map(|(_, nodes)| SingularCause::NoPathToGround { nodes, dc }).collect()
}

fn voltage_loops(c: &Circuit, dc: bool) -> Vec<SingularCause> {
    let n = c.node_count();
    let mut uf = UnionFind::new(n);
    // Forest adjacency: (neighbour, element index).
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut causes = Vec::new();
    for (i, e) in c.elements().iter().enumerate() {
        let voltage_defined = match e.kind() {
            ElementKind::VoltageSource { .. } | ElementKind::Vcvs { .. } | ElementKind::Ccvs { .. } => true,
            ElementKind::Law { law, .. } => law.output == crate::behavioral::Output::Voltage,
            ElementKind::Inductor { .. } => dc,
            _ => false,
        };
        if !voltage_defined {
            continue;
        }
        let Some((a, b)) = e.kind().terminals() else { continue };
        let (a, b) = (a.index(), b.index());
        if uf.union(a, b) {
            adj[a].push((b, i));
            adj[b].push((a, i));
            continue;
        }
        let mut elements: Vec<String> =
            forest_path(&adj, a, b).into_iter().map(|k| c.elements()[k].name().to_string()).collect();
        elements.push(e.name().to_string());
        causes.push(SingularCause::VoltageLoop { elements, dc });
    }
    causes
}

/// Elements on the forest path from `from` to `to` (empty when equal).
fn forest_path(adj: &[Vec<(usize, usize)>], from: usize, to: usize) -> Vec<usize> {
    let mut prev: Vec<Option<(usize, usize)>> = vec![None; adj.len()];
    let mut seen = vec![false; adj.len()];
    let mut queue = VecDeque::from([from]);
    seen[from] = true;
    while let Some(u) = queue.pop_front() {
        if u == to {
            break;
        }
        for &(v, e) in &adj[u] {
            if !seen[v] {
                seen[v] = true;
                prev[v] = Some((u, e));
                queue.push_back(v);
            }
        }
    }
    let mut path = Vec::new();
    let mut at = to;
    while let Some((u, e)) = prev[at] {
        path.push(e);
        at = u;
    }
    path.reverse();
    path
}

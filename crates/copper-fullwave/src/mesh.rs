//! The graded mesher: every line of an axis is chosen here (the solver
//! detects none from the primitives).
//!
//! Rules, in order:
//!
//! 1. **Lines.** One per copper edge or vertex, via, port terminal and
//!    region edge, with a rank (`Window` < `Via` < `Copper` < `Port`) and the
//!    net its copper belongs to.
//! 2. **Merging.** Lines closer than `min_cell` merge into the higher rank
//!    (equal ranks keep the first); the lower line's geometry moves onto it,
//!    and every move is reported. Two lines never merge when they carry
//!    different nets (the gap between two nets is real and a merge shorts
//!    them: the sliver stays and is reported, unless the gap is below
//!    `net_floor`, far under any real clearance, where the conductors are
//!    taken to overlap and the lines merge, counted in `net_merges`), nor when
//!    both are port terminals.
//! 3. **Cells between lines.** A gap is cut into equal cells no wider than
//!    1.25 times the local target, which is the copper-region cell near the
//!    fine zone and grows by 0.3 of the distance from it (the slope of a 1.3
//!    ratio) up to `max_inner`; z gaps use the requested counts.
//! 4. **Outside.** Beyond the outermost line cells grow by 1.3 per cell to
//!    `max_outer` until `air` is reached, then `pml_cells` cells of the last
//!    size (the solver's PML layers).
//! 5. **Grading.** The larger of two neighbouring cells with a ratio above
//!    1.5 is split into equal pieces, up to 24 passes; the remaining maximum
//!    ratio and the number of pairs above 1.5 are reported (a real sliver
//!    next to a regular cell is graded away in a few passes; the cap only guards a loop).

use crate::spec::{Error, Result};

/// How a candidate line ranks when two lie closer than `min_cell`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// The region edge: where slabs and clipped planes end.
    Window,
    /// A via or plated hole centre.
    Via,
    /// A copper edge or vertex.
    Copper,
    /// A port terminal or the line between them: never moved.
    Port,
}

/// A candidate line.
#[derive(Clone, Debug)]
pub struct Line {
    pub pos: f64,
    pub rank: Rank,
    /// Net ids of the copper that ends on this line (sorted, deduplicated).
    pub nets: Vec<u32>,
}

impl Line {
    pub fn new(pos: f64, rank: Rank, net: Option<u32>) -> Line {
        Line { pos, rank, nets: net.into_iter().collect() }
    }
}

/// One axis of the mesh.
#[derive(Clone, Debug)]
pub struct AxisPlan {
    pub lines: Vec<Line>,
    /// The cell of the copper region.
    pub cell: f64,
    /// The span around the features that gets `cell`; elsewhere cells grow.
    pub fine_zone: Option<(f64, f64)>,
    pub min_cell: f64,
    /// Lines of different nets closer than this merge anyway.
    pub net_floor: f64,
    /// Largest cell between the first and last line.
    pub max_inner: f64,
    /// Largest cell in the air.
    pub max_outer: f64,
    /// Air before the PML, measured from the first and last line.
    pub air: f64,
    pub pml_cells: usize,
    /// Cell counts demanded for gaps `(lo, hi, n)` (the z slabs).
    pub gap_counts: Vec<(f64, f64, usize)>,
}

/// A move of a candidate line caused by merging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub from: f64,
    pub to: f64,
}

/// The lines of one axis and what the rules did.
#[derive(Clone, Debug)]
pub struct AxisMesh {
    pub lines: Vec<f64>,
    /// Kept lines before subdivision (features and region edges).
    pub kept: Vec<f64>,
    pub moves: Vec<Move>,
    /// Cells below `min_cell`: `(position, width)` of pairs of lines of different nets kept apart.
    pub slivers: Vec<(f64, f64)>,
    /// Pairs of lines of different nets merged because they were closer than `net_floor`.
    pub net_merges: usize,
    pub max_ratio: f64,
    pub ratio_violations: usize,
}

impl AxisMesh {
    pub fn cells(&self) -> Vec<f64> {
        self.lines.windows(2).map(|w| w[1] - w[0]).collect()
    }

    pub fn smallest(&self) -> f64 {
        self.cells().into_iter().fold(f64::INFINITY, f64::min)
    }

    /// The line nearest `v`.
    pub fn nearest(&self, v: f64) -> f64 {
        self.lines.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).unwrap_or(v)
    }
}

const GROWTH: f64 = 1.3;
const RATIO: f64 = 1.5;

fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = a.iter().chain(b).copied().collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Step 2: merge candidate lines.
/// Kept lines, moves, slivers `(position, width)` and the count of cross-net merges.
type Merged = (Vec<Line>, Vec<Move>, Vec<(f64, f64)>, usize);

fn merge(plan: &AxisPlan) -> Merged {
    let mut cands: Vec<Line> = plan.lines.iter().filter(|l| l.pos.is_finite()).cloned().collect();
    cands.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let mut kept: Vec<Line> = Vec::new();
    let mut moves = Vec::new();
    let mut slivers = Vec::new();
    let mut net_merges = 0;
    for cand in cands {
        let Some(last) = kept.last_mut() else {
            kept.push(cand);
            continue;
        };
        let d = cand.pos - last.pos;
        if d < 1e-9 {
            last.rank = last.rank.max(cand.rank);
            last.nets = union(&last.nets, &cand.nets);
            continue;
        }
        if d >= plan.min_cell {
            kept.push(cand);
            continue;
        }
        let nets = union(&last.nets, &cand.nets);
        let crossing = nets.len() > 1;
        if crossing && d < plan.net_floor {
            net_merges += 1;
        }
        let separate = (crossing && d >= plan.net_floor) || (last.rank == Rank::Port && cand.rank == Rank::Port);
        if separate {
            slivers.push((last.pos, d));
            kept.push(cand);
        } else if cand.rank > last.rank {
            moves.push(Move { from: last.pos, to: cand.pos });
            let old = kept.pop().expect("a line");
            kept.push(Line { pos: cand.pos, rank: cand.rank, nets: union(&old.nets, &cand.nets) });
        } else {
            moves.push(Move { from: cand.pos, to: last.pos });
            last.nets = nets;
        }
    }
    (kept, moves, slivers, net_merges)
}

/// Target size at `x`: `cell` in the fine zone, growing by 0.3 per unit of distance.
fn target(plan: &AxisPlan, x: f64) -> f64 {
    let d = match plan.fine_zone {
        Some((lo, hi)) => (lo - x).max(x - hi).max(0.0),
        None => 0.0,
    };
    (plan.cell + (GROWTH - 1.0) * d).min(plan.max_inner.max(plan.cell))
}

/// Cut `[a, b]` into cells that follow the target size.
fn cut_gap(plan: &AxisPlan, a: f64, b: f64, out: &mut Vec<f64>) {
    if let Some((_, _, n)) = plan.gap_counts.iter().find(|(lo, hi, _)| (lo - a).abs() < 1e-9 && (hi - b).abs() < 1e-9) {
        for i in 1..=*n {
            out.push(a + (b - a) * i as f64 / *n as f64);
        }
        return;
    }
    let mut x = a;
    loop {
        let rem = b - x;
        // The cell must not exceed the target at either end: take the smaller (the end nearer the fine zone).
        let h0 = target(plan, x);
        let h = h0.min(target(plan, x + h0));
        let h = h.max(plan.min_cell.min(rem));
        if rem <= 1.6 * h {
            let n = (rem / (1.25 * h) - 1e-9).ceil().max(1.0) as usize;
            for i in 1..=n {
                out.push(x + rem * i as f64 / n as f64);
            }
            return;
        }
        x += h;
        out.push(x);
    }
}

fn extend(plan: &AxisPlan, from: f64, first_cell: f64, sign: f64, out: &mut Vec<f64>) {
    let mut h = first_cell;
    let mut x = from;
    while (x - from).abs() < plan.air {
        h = (h * GROWTH).min(plan.max_outer.max(first_cell));
        x += sign * h;
        out.push(x);
    }
    for _ in 0..plan.pml_cells {
        x += sign * h;
        out.push(x);
    }
}

/// Build the lines of one axis.
pub fn axis_mesh(plan: &AxisPlan) -> Result<AxisMesh> {
    let (kept, moves, slivers, net_merges) = merge(plan);
    if kept.len() < 2 {
        return Err(Error::Invalid("an axis needs at least two mesh lines".to_owned()));
    }
    let mut inner = vec![kept[0].pos];
    for w in kept.windows(2) {
        cut_gap(plan, w[0].pos, w[1].pos, &mut inner);
    }
    let first_cell = inner[1] - inner[0];
    let last_cell = inner[inner.len() - 1] - inner[inner.len() - 2];
    let mut below = Vec::new();
    extend(plan, inner[0], first_cell, -1.0, &mut below);
    let mut above = Vec::new();
    extend(plan, inner[inner.len() - 1], last_cell, 1.0, &mut above);
    let mut lines: Vec<f64> = below.into_iter().rev().collect();
    lines.extend(inner);
    lines.extend(above);

    for _ in 0..24 {
        let mut next = vec![lines[0]];
        let n = lines.len() - 1;
        let w: Vec<f64> = lines.windows(2).map(|p| p[1] - p[0]).collect();
        let mut changed = false;
        for i in 0..n {
            let me = w[i];
            let left = if i > 0 { w[i - 1] } else { me };
            let right = if i + 1 < n { w[i + 1] } else { me };
            let small = left.min(right);
            let k = if me > RATIO * small * (1.0 + 1e-9) {
                (me / (RATIO * small) - 1e-9).ceil().max(1.0) as usize
            } else {
                1
            };
            // Both neighbours smaller than half of me: split at most so the pieces stay above them.
            let k = k.min(8);
            if k > 1 {
                changed = true;
            }
            for j in 1..=k {
                next.push(lines[i] + me * j as f64 / k as f64);
            }
        }
        lines = next;
        if !changed {
            break;
        }
    }
    let w: Vec<f64> = lines.windows(2).map(|p| p[1] - p[0]).collect();
    let ratios: Vec<f64> = w.windows(2).map(|p| (p[0] / p[1]).max(p[1] / p[0])).collect();
    let max_ratio = ratios.iter().copied().fold(1.0, f64::max);
    let ratio_violations = ratios.iter().filter(|r| **r > RATIO + 1e-6).count();
    Ok(AxisMesh {
        lines,
        kept: kept.iter().map(|l| l.pos).collect(),
        moves,
        slivers,
        net_merges,
        max_ratio,
        ratio_violations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(lines: Vec<Line>) -> AxisPlan {
        AxisPlan {
            lines,
            cell: 0.25,
            fine_zone: Some((0.0, 10.0)),
            min_cell: 0.08,
            net_floor: 0.02,
            max_inner: 1.0,
            max_outer: 3.0,
            air: 8.0,
            pml_cells: 8,
            gap_counts: Vec::new(),
        }
    }

    #[test]
    fn near_lines_of_one_net_merge_into_the_higher_rank_and_the_move_is_reported() {
        let p = plan(vec![
            Line::new(0.0, Rank::Window, None),
            Line::new(2.0, Rank::Copper, Some(1)),
            Line::new(2.05, Rank::Port, Some(1)),
            Line::new(6.0, Rank::Copper, Some(2)),
            Line::new(10.0, Rank::Window, None),
        ]);
        let m = axis_mesh(&p).expect("mesh");
        assert!(m.kept.contains(&2.05) && !m.kept.contains(&2.0), "{:?}", m.kept);
        assert_eq!(m.moves, vec![Move { from: 2.0, to: 2.05 }]);
    }

    #[test]
    fn lines_of_different_nets_never_merge_and_the_sliver_is_reported() {
        let p = plan(vec![
            Line::new(0.0, Rank::Window, None),
            Line::new(3.0, Rank::Copper, Some(1)),
            Line::new(3.05, Rank::Copper, Some(2)),
            Line::new(10.0, Rank::Window, None),
        ]);
        let m = axis_mesh(&p).expect("mesh");
        assert!(m.lines.contains(&3.0) && m.lines.contains(&3.05));
        assert_eq!(m.slivers.len(), 1);
    }

    #[test]
    fn cells_stay_within_the_rules_and_the_air_and_pml_are_added() {
        let p = plan(vec![
            Line::new(0.0, Rank::Window, None),
            Line::new(4.0, Rank::Copper, Some(1)),
            Line::new(4.7, Rank::Copper, Some(1)),
            Line::new(10.0, Rank::Window, None),
        ]);
        let m = axis_mesh(&p).expect("mesh");
        let cells = m.cells();
        assert!(cells.iter().all(|c| *c <= 3.0 + 1e-9));
        // Inside the window the fine zone covers everything: no cell above 1.25 x 0.25.
        let inside: Vec<f64> =
            m.lines.windows(2).filter(|w| w[0] >= 0.0 && w[1] <= 10.0).map(|w| w[1] - w[0]).collect();
        assert!(inside.iter().all(|c| *c <= 0.3125 + 1e-9), "{inside:?}");
        assert!(m.lines[0] < -8.0 && *m.lines.last().unwrap() > 18.0);
        assert!(m.ratio_violations == 0, "max ratio {} in {:?}", m.max_ratio, cells);
    }

    #[test]
    fn z_gaps_follow_the_requested_counts() {
        let mut p = plan(vec![Line::new(-0.2, Rank::Copper, Some(1)), Line::new(0.0, Rank::Copper, Some(1))]);
        p.fine_zone = Some((-0.2, 0.0));
        p.min_cell = 0.05;
        p.gap_counts = vec![(-0.2, 0.0, 3)];
        let m = axis_mesh(&p).expect("mesh");
        let n = m.lines.iter().filter(|v| **v >= -0.2 - 1e-9 && **v <= 1e-9).count();
        assert_eq!(n, 4, "{:?}", m.lines);
    }
}

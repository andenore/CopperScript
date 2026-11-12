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
    /// Local refinement `(lo, hi, cell)`: inside `[lo, hi]` the target is `cell`
    /// (when smaller than the general target), growing by 0.3 per unit of distance
    /// outside, so the ratio between neighbours stays under 1.5. Lines merge and
    /// nets separate at the scale of the local cell, not of `min_cell`.
    pub zones: Vec<(f64, f64, f64)>,
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
    /// Smoothing for meshes with local refinement (x and y): a gap between two kept lines that is
    /// narrower than 0.9 of the local target becomes a refinement zone of its own, so the cells around
    /// it grow from its size by 1.3 per cell; a gap starts with at most 1.3 times the cell before it and
    /// ends in cells rounded to the target; and the free lines are relaxed where neighbouring cells
    /// differ by more than 1.4. Without them a jump between two gaps' integer cell counts makes the
    /// split-the-larger-cell pass halve the mesh all the way to the boundary. Off for meshes without
    /// refinement zones, which keep the original cutting (cells at most 1.25 times the target).
    pub smooth: bool,
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
        let min_cell = local_min_cell(plan, last.pos);
        let net_floor = plan.net_floor;
        if d < 1e-9 {
            last.rank = last.rank.max(cand.rank);
            last.nets = union(&last.nets, &cand.nets);
            continue;
        }
        if d >= min_cell {
            kept.push(cand);
            continue;
        }
        let nets = union(&last.nets, &cand.nets);
        let crossing = nets.len() > 1;
        if crossing && d < net_floor {
            net_merges += 1;
        }
        let separate = (crossing && d >= net_floor) || (last.rank == Rank::Port && cand.rank == Rank::Port);
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

/// The refinement target at `x`: the smallest `cell + 0.3 d` over the zones (infinite without zones).
fn zone_target(plan: &AxisPlan, x: f64) -> f64 {
    plan.zones
        .iter()
        .map(|(lo, hi, c)| c + (GROWTH - 1.0) * (lo - x).max(x - hi).max(0.0))
        .fold(f64::INFINITY, f64::min)
}

/// Target size at `x`: `cell` in the fine zone, growing by 0.3 per unit of
/// distance, and the refinement zones' own cells where they are smaller.
fn target(plan: &AxisPlan, x: f64) -> f64 {
    let d = match plan.fine_zone {
        Some((lo, hi)) => (lo - x).max(x - hi).max(0.0),
        None => 0.0,
    };
    let general = (plan.cell + (GROWTH - 1.0) * d).min(plan.max_inner.max(plan.cell));
    general.min(zone_target(plan, x))
}

/// The merge distance at `x`: `min_cell`, or 0.4 of the refinement cell there where that is smaller.
fn local_min_cell(plan: &AxisPlan, x: f64) -> f64 {
    let z = zone_target(plan, x);
    if z.is_finite() { plan.min_cell.min((0.4 * z).max(0.04)) } else { plan.min_cell }
}

/// Cut `[a, b]` into cells that follow the target size. `prev` is the cell just before `a` (0 at the
/// first gap): the first cell may be at most 1.3 times it, so a gap that follows a small one ramps up
/// instead of jumping, and the last cells are equal, rounded to the target, rather than the ceiling
/// of the 1.25 rule (which left 0.108 next to 0.167, a ratio of 1.55, and set off the grading cascade).
fn cut_gap(plan: &AxisPlan, a: f64, b: f64, prev: f64, out: &mut Vec<f64>) {
    if let Some((_, _, n)) = plan.gap_counts.iter().find(|(lo, hi, _)| (lo - a).abs() < 1e-9 && (hi - b).abs() < 1e-9) {
        for i in 1..=*n {
            out.push(a + (b - a) * i as f64 / *n as f64);
        }
        return;
    }
    let mut x = a;
    let mut last = prev;
    loop {
        let rem = b - x;
        // The cell must not exceed the target at either end: take the smaller (the end nearer the fine zone).
        let h0 = target(plan, x);
        let h = h0.min(target(plan, x + h0));
        let floor = local_min_cell(plan, x).min(rem);
        let h = h.max(floor);
        let h = if plan.smooth && last > 0.0 { h.min(1.3 * last).max(floor) } else { h };
        if rem <= 1.6 * h {
            let n = if plan.smooth {
                let mut n = (rem / h).round().max(1.0);
                if rem / n > 1.25 * h {
                    n += 1.0;
                }
                n as usize
            } else {
                (rem / (1.25 * h) - 1e-9).ceil().max(1.0) as usize
            };
            for i in 1..=n {
                out.push(x + rem * i as f64 / n as f64);
            }
            return;
        }
        x += h;
        out.push(x);
        last = h;
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

/// Smooth the free lines (every line that is not a kept feature line or an end) where a ratio of
/// neighbouring cells exceeds 1.4: each such line moves half way to the position that equalises its two
/// cells, so a jump between the gaps' integer cell counts spreads over a few cells and dies out, instead
/// of the split-the-larger-cell pass below halving the mesh all the way to the boundary.
fn relax(lines: &mut [f64], fixed: &[f64]) {
    let n = lines.len();
    if n < 4 {
        return;
    }
    let is_fixed = |x: f64| fixed.iter().any(|f| (f - x).abs() < 1e-9);
    let pinned: Vec<bool> = lines.iter().map(|x| is_fixed(*x)).collect();
    let ratio = |a: f64, b: f64| (a / b).max(b / a);
    for _ in 0..600 {
        let mut moved = false;
        for i in 1..n - 1 {
            if pinned[i] {
                continue;
            }
            let (a, b) = (lines[i] - lines[i - 1], lines[i + 1] - lines[i]);
            let ap = if i >= 2 { lines[i - 1] - lines[i - 2] } else { a };
            let bn = if i + 2 < n { lines[i + 2] - lines[i + 1] } else { b };
            if ratio(ap, a).max(ratio(a, b)).max(ratio(b, bn)) > 1.4 {
                let target = lines[i - 1] + 0.5 * (a + b);
                let next = lines[i] + 0.5 * (target - lines[i]);
                if (next - lines[i]).abs() > 1e-7 {
                    lines[i] = next;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
}

/// Build the lines of one axis.
pub fn axis_mesh(plan: &AxisPlan) -> Result<AxisMesh> {
    let (kept, moves, slivers, net_merges) = merge(plan);
    if kept.len() < 2 {
        return Err(Error::Invalid("an axis needs at least two mesh lines".to_owned()));
    }
    let graded;
    let plan = if plan.smooth && plan.gap_counts.is_empty() {
        let mut p = plan.clone();
        for w in kept.windows(2) {
            let g = w[1].pos - w[0].pos;
            if g < 0.9 * target(plan, 0.5 * (w[0].pos + w[1].pos)) {
                p.zones.push((w[0].pos, w[1].pos, g));
            }
        }
        graded = p;
        &graded
    } else {
        plan
    };
    let mut inner = vec![kept[0].pos];
    for w in kept.windows(2) {
        let prev = if inner.len() >= 2 { inner[inner.len() - 1] - inner[inner.len() - 2] } else { 0.0 };
        cut_gap(plan, w[0].pos, w[1].pos, prev, &mut inner);
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

    if plan.smooth {
        relax(&mut lines, &kept.iter().map(|l| l.pos).collect::<Vec<_>>());
    }
    let floor_piece = 0.9 * lines.windows(2).map(|p| p[1] - p[0]).fold(f64::INFINITY, f64::min);
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
            // Never cut a piece smaller than 0.9 of the smallest cell the features required: the time step
            // follows the smallest cell, and a ratio of 1.6 in a crowded corner is cheaper than halving dt.
            let k = if plan.smooth { k.min(((me / floor_piece).floor() as usize).max(1)) } else { k };
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
            zones: Vec::new(),
            min_cell: 0.08,
            net_floor: 0.02,
            max_inner: 1.0,
            max_outer: 3.0,
            air: 8.0,
            pml_cells: 8,
            gap_counts: Vec::new(),
            smooth: false,
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
    fn a_refinement_zone_gets_its_own_cell_and_grows_out_by_at_most_1_5() {
        let mut p = plan(vec![
            Line::new(0.0, Rank::Window, None),
            Line::new(4.0, Rank::Copper, Some(1)),
            Line::new(4.6, Rank::Copper, Some(1)),
            Line::new(10.0, Rank::Window, None),
        ]);
        p.cell = 0.5;
        p.zones = vec![(3.5, 5.0, 0.2)];
        p.smooth = true;
        p.min_cell = 0.2;
        let m = axis_mesh(&p).expect("mesh");
        let inside: Vec<f64> =
            m.lines.windows(2).filter(|w| w[0] >= 3.5 - 1e-9 && w[1] <= 5.0 + 1e-9).map(|w| w[1] - w[0]).collect();
        assert!(!inside.is_empty() && inside.iter().all(|c| *c <= 0.26 + 1e-9), "{inside:?}");
        let far: Vec<f64> = m.lines.windows(2).filter(|w| w[0] >= 7.0 && w[1] <= 10.0).map(|w| w[1] - w[0]).collect();
        assert!(far.iter().any(|c| *c > 0.4), "the zone must not refine the whole axis: {far:?}");
        assert_eq!(m.ratio_violations, 0, "max ratio {}", m.max_ratio);
    }

    #[test]
    fn a_gap_smaller_than_the_cell_is_graded_into_instead_of_halving_the_mesh() {
        // Kept lines 0.108 apart in a 0.167 cell mesh: the old equal-split grading cascaded to 0.084 everywhere.
        let mut p = plan(vec![
            Line::new(0.0, Rank::Window, None),
            Line::new(5.0, Rank::Copper, Some(1)),
            Line::new(5.108, Rank::Copper, Some(2)),
            Line::new(10.0, Rank::Window, None),
        ]);
        p.cell = 0.167;
        p.min_cell = 0.067;
        p.net_floor = 0.017;
        p.smooth = true;
        let m = axis_mesh(&p).expect("mesh");
        let regular =
            m.lines.windows(2).filter(|w| w[0] >= 0.0 && w[1] <= 3.0).map(|w| w[1] - w[0]).fold(0.0, f64::max);
        assert!(regular > 0.15, "cells far from the gap must stay near 0.167, got {regular}");
        assert!(m.max_ratio <= 1.5 + 1e-6, "ratio {}", m.max_ratio);
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

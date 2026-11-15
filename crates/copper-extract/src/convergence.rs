//! Convergence of a mesh series across the whole port matrix.
//!
//! A series of meshes of the same layout (the finest, one coarser by 1.5 in pitch, and one coarser still) is judged
//! on every entry of the inductance matrix `L` and of the resistance matrix `R` at every frequency, not on the hot
//! loop's self inductance alone (a mutual term or the resistance can move while the self inductance does not).
//!
//! **Change of an entry.** Between two meshes `a` (the finer) and `b`: a diagonal entry by itself,
//! `100 |a_ii - b_ii| / |a_ii|`; an off-diagonal entry by the geometric mean of the two diagonals of the finer mesh,
//! `100 |a_ij - b_ij| / sqrt(|a_ii a_jj|)` (a mutual can be near zero, a relative change of itself is meaningless).
//!
//! **Comparability.** Two meshes that are squeezed against the same budget, or that are in different states of
//! refinement, agree for reasons that are not convergence. The step between them is then *not established*, and the
//! reasons say why: the pitch ratio actually achieved, the filament ratio, one mesh refined and the other not,
//! different counts of unresolved gaps, different numbers of copper pieces.

use crate::hotloop::MeshRun;

/// The two finest meshes may differ by at most this in any entry of `L`, percent (the threshold of the first
/// version, which was only applied to the hot loop's self inductance at 10 MHz).
pub const L_THRESHOLD_PCT: f64 = 3.0;

/// The same for any entry of `R`, percent. See `docs/emi-noise-analysis.md` section 13 for what it rests on.
pub const R_THRESHOLD_PCT: f64 = 5.0;

/// Meshes whose filament counts are closer than this ratio are not a refinement of one another: a pitch ratio of 1.5
/// in a plane gives about 2.25 for copper that is meshed by the pitch; copper that keeps its resolution whatever the
/// pitch (terminals, vias, narrow loop copper) dilutes it, but below this the meshes are the same density.
pub const MIN_FILAMENT_RATIO: f64 = 1.10;

/// The achieved pitch ratio (coarser fine pitch over finer) below which the two meshes do not differ in pitch.
pub const MIN_PITCH_RATIO: f64 = 1.2;

/// A change between the two finest meshes that is larger than the same entry's change between the two coarser ones is
/// reported as growing when it is above this share of the entry's threshold (below it both changes are noise).
pub const GROWTH_FLOOR_SHARE: f64 = 1.0 / 3.0;

/// The definition printed with every result.
pub const DEFINITION: &str = "change of an entry between two meshes, percent: a diagonal entry of L or R relative to itself, an off-diagonal entry relative to the geometric mean of the two diagonal entries of the finer mesh (a mutual can be near zero); per frequency, for every entry of the L matrix and of the R matrix; `finest_vs_next` is the two finest meshes of the series, `next_vs_earlier` the two coarser ones; the series is `established` only when the two finest meshes are comparable (pitch ratio, filament ratio, refinement state, unresolved gaps, copper pieces); `not_contracting` names the worst entry of each matrix whose change between the two finest meshes is larger than between the two coarser ones (and above a third of its threshold)";

/// Changes of every entry between two meshes: `[frequency][row][column]`, percent.
#[derive(Clone, Debug, PartialEq)]
pub struct StepChange {
    pub l_pct: Vec<Vec<Vec<f64>>>,
    pub r_pct: Vec<Vec<Vec<f64>>>,
    /// Whether the two meshes are comparable, and if not why not.
    pub comparable: bool,
    pub comparability: Vec<String>,
}

/// The entry that changed most.
#[derive(Clone, Debug, PartialEq)]
pub struct Worst {
    /// `L` or `R`.
    pub matrix: &'static str,
    pub row: String,
    pub col: String,
    pub frequency_hz: f64,
    pub pct: f64,
}

impl Worst {
    pub fn text(&self) -> String {
        if self.row == self.col {
            format!(
                "{} of {} at {:.0} MHz changes by {:.1} %",
                self.matrix,
                self.row,
                self.frequency_hz * 1e-6,
                self.pct
            )
        } else {
            format!(
                "{} between {} and {} at {:.0} MHz changes by {:.1} % (of the geometric mean of the diagonals)",
                self.matrix,
                self.row,
                self.col,
                self.frequency_hz * 1e-6,
                self.pct
            )
        }
    }
}

/// The result for a series.
#[derive(Clone, Debug, PartialEq)]
pub struct Convergence {
    pub definition: &'static str,
    pub l_threshold_pct: f64,
    pub r_threshold_pct: f64,
    /// The two finest meshes; `None` for a single mesh.
    pub finest_vs_next: Option<StepChange>,
    /// The two coarser meshes; `None` with fewer than three.
    pub next_vs_earlier: Option<StepChange>,
    /// Largest change of an entry of `L` and of `R` between the two finest meshes.
    pub worst_l: Option<Worst>,
    pub worst_r: Option<Worst>,
    /// The two finest meshes are comparable: the sensitivity is evidence of convergence.
    pub established: bool,
    pub reasons_not_established: Vec<String>,
    /// Per frequency, the length over which a sheet of the copper's thickness lets current crowd to an edge,
    /// `(rho / t) / (2 pi f mu0)`, micrometres: where it is far below the pitch the model cannot resolve the
    /// crowding and R (which rises with refinement there) is not expected to converge.
    pub crowding_length_um: Vec<f64>,
    /// Entries (the worst of each matrix) whose change between the two finest meshes is larger than between the two
    /// coarser ones, both steps comparable: the series is not contracting there, so the last change is not an error
    /// bound. One line each; empty when every entry's change shrinks (or is below the floor).
    pub not_contracting: Vec<String>,
}

/// The worst entry of a matrix whose change grows with refinement, as a sentence.
fn growing(
    fine: &[Vec<Vec<f64>>],
    earlier: &[Vec<Vec<f64>>],
    matrix: &'static str,
    threshold: f64,
    labels: &[String],
    freqs: &[f64],
) -> Option<String> {
    let mut best: Option<(Worst, f64)> = None;
    for (f, m) in fine.iter().enumerate() {
        for (i, row) in m.iter().enumerate() {
            for (j, &pct) in row.iter().enumerate() {
                let Some(&before) = earlier.get(f).and_then(|e| e.get(i)).and_then(|r| r.get(j)) else { continue };
                if pct.is_nan() || pct <= threshold * GROWTH_FLOOR_SHARE || pct <= before {
                    continue;
                }
                if best.as_ref().is_some_and(|b| b.0.pct >= pct) {
                    continue;
                }
                best = Some((
                    Worst {
                        matrix,
                        row: labels.get(i).cloned().unwrap_or_else(|| format!("port {i}")),
                        col: labels.get(j).cloned().unwrap_or_else(|| format!("port {j}")),
                        frequency_hz: freqs.get(f).copied().unwrap_or(0.0),
                        pct,
                    },
                    before,
                ));
            }
        }
    }
    best.map(|(w, before)| {
        format!(
            "{} between the two finest meshes, {before:.1} % between the two coarser: the change grows with refinement, so the series is not yet converging in this entry and the last change is not an error bound",
            w.text()
        )
    })
}

/// Change of every entry of `a` against `b`, percent: `[row][column]`.
pub fn entry_changes(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    let scale = if i == j { a[i][i].abs() } else { (a[i][i] * a[j][j]).abs().sqrt() };
                    if scale > 0.0 { 100.0 * (a[i][j] - b[i][j]).abs() / scale } else { 0.0 }
                })
                .collect()
        })
        .collect()
}

fn resistance_matrix(run: &MeshRun) -> &[Vec<Vec<f64>>] {
    &run.resistance_matrix_ohm
}

fn step(a: &MeshRun, b: &MeshRun) -> StepChange {
    let freqs = a.inductance_matrix_nh.len().min(b.inductance_matrix_nh.len());
    let l_pct = (0..freqs).map(|f| entry_changes(&a.inductance_matrix_nh[f], &b.inductance_matrix_nh[f])).collect();
    let r_pct = (0..freqs.min(resistance_matrix(a).len()).min(resistance_matrix(b).len()))
        .map(|f| entry_changes(&resistance_matrix(a)[f], &resistance_matrix(b)[f]))
        .collect();
    let mut why = Vec::new();
    let filament_ratio = a.filaments as f64 / b.filaments.max(1) as f64;
    if filament_ratio < MIN_FILAMENT_RATIO {
        why.push(format!(
            "the {} mesh has {} filaments and the {} mesh {} (ratio {:.2}, less than {MIN_FILAMENT_RATIO}): they are the same density, squeezed against the same budget, so their agreement is not evidence of convergence",
            a.role, a.filaments, b.role, b.filaments, filament_ratio
        ));
    }
    let pitch_ratio = b.cell_nm as f64 / a.cell_nm.max(1) as f64;
    if pitch_ratio < MIN_PITCH_RATIO {
        why.push(format!(
            "the fine pitch of the {} mesh ({:.3} mm) is not coarser than the {} mesh's ({:.3} mm) by 1.5: ratio {:.2}",
            b.role,
            b.cell_nm as f64 * 1e-6,
            a.role,
            a.cell_nm as f64 * 1e-6,
            pitch_ratio
        ));
    }
    // The refinement state: a mesh whose refinement was cut short by the budget is not comparable with one whose
    // refinement was complete (or not needed). A local refinement that finished is not a difference.
    if (a.refinements_skipped > 0) != (b.refinements_skipped > 0) {
        why.push(format!(
            "the {} mesh left {} refinement(s) out because of the filament budget and the {} mesh {}: different states of refinement",
            a.role, a.refinements_skipped, b.role, b.refinements_skipped
        ));
    }
    if a.unresolved_gap_count != b.unresolved_gap_count {
        why.push(format!(
            "the meshes leave different numbers of gaps unresolved ({} against {})",
            a.unresolved_gap_count, b.unresolved_gap_count
        ));
    }
    if a.copper_components != b.copper_components {
        why.push(format!(
            "the meshes see different numbers of copper pieces ({} against {})",
            a.copper_components, b.copper_components
        ));
    }
    StepChange { l_pct, r_pct, comparable: why.is_empty(), comparability: why }
}

fn worst(changes: &[Vec<Vec<f64>>], matrix: &'static str, labels: &[String], freqs: &[f64]) -> Option<Worst> {
    let mut best: Option<Worst> = None;
    for (f, m) in changes.iter().enumerate() {
        for (i, row) in m.iter().enumerate() {
            for (j, &pct) in row.iter().enumerate() {
                if pct.is_nan() || best.as_ref().is_some_and(|b| b.pct >= pct) {
                    continue;
                }
                best = Some(Worst {
                    matrix,
                    row: labels.get(i).cloned().unwrap_or_else(|| format!("port {i}")),
                    col: labels.get(j).cloned().unwrap_or_else(|| format!("port {j}")),
                    frequency_hz: freqs.get(f).copied().unwrap_or(0.0),
                    pct,
                });
            }
        }
    }
    best
}

/// Judge a series (finest first) over every entry of both matrices.
pub fn assess(runs: &[MeshRun], freqs: &[f64], labels: &[String], thickness_m: f64) -> Convergence {
    let finest_vs_next = (runs.len() >= 2).then(|| step(&runs[0], &runs[1]));
    let next_vs_earlier = (runs.len() >= 3).then(|| step(&runs[1], &runs[2]));
    let (worst_l, worst_r) = match &finest_vs_next {
        Some(s) => (worst(&s.l_pct, "L", labels, freqs), worst(&s.r_pct, "R", labels, freqs)),
        None => (None, None),
    };
    let reasons_not_established = match &finest_vs_next {
        Some(s) => s.comparability.clone(),
        None => vec!["a single mesh: no evidence of convergence".to_owned()],
    };
    let not_contracting = match (&finest_vs_next, &next_vs_earlier) {
        (Some(a), Some(b)) if a.comparable && b.comparable => [
            growing(&a.l_pct, &b.l_pct, "L", L_THRESHOLD_PCT, labels, freqs),
            growing(&a.r_pct, &b.r_pct, "R", R_THRESHOLD_PCT, labels, freqs),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    };
    Convergence {
        not_contracting,
        definition: DEFINITION,
        l_threshold_pct: L_THRESHOLD_PCT,
        r_threshold_pct: R_THRESHOLD_PCT,
        established: finest_vs_next.as_ref().is_some_and(|s| s.comparable),
        finest_vs_next,
        next_vs_earlier,
        worst_l,
        worst_r,
        reasons_not_established,
        crowding_length_um: freqs
            .iter()
            .map(|&f| {
                if f > 0.0 && thickness_m > 0.0 {
                    1e6 * (crate::layout::COPPER_RESISTIVITY / thickness_m)
                        / (2.0 * std::f64::consts::PI * f * 4e-7 * std::f64::consts::PI)
                } else {
                    f64::INFINITY
                }
            })
            .collect(),
    }
}

impl Convergence {
    /// Reasons the status is not `ok`: a series that is not established, or the entries beyond their thresholds
    /// between the two finest meshes (the worst of each matrix named).
    pub fn status_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if !self.established {
            reasons.push(format!("mesh convergence is not established: {}", self.reasons_not_established.join("; ")));
            return reasons;
        }
        for (worst, threshold) in [(&self.worst_l, self.l_threshold_pct), (&self.worst_r, self.r_threshold_pct)] {
            if let Some(w) = worst
                && w.pct > threshold
            {
                reasons.push(format!(
                    "the two finest meshes differ by more than {threshold} % in an entry of the {} matrix: {}",
                    w.matrix,
                    w.text()
                ));
            }
        }
        reasons.extend(self.not_contracting.iter().cloned());
        reasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mutual_is_normalised_by_the_diagonals_and_a_diagonal_by_itself() {
        let a = vec![vec![4.0, 0.2], vec![0.2, 1.0]];
        let b = vec![vec![4.0, 0.1], vec![0.1, 1.1]];
        let c = entry_changes(&a, &b);
        assert!((c[0][0]).abs() < 1e-12);
        assert!((c[1][1] - 10.0).abs() < 1e-9);
        // 0.1 against sqrt(4 x 1) = 2: 5 %, not 50 %.
        assert!((c[0][1] - 5.0).abs() < 1e-9 && (c[1][0] - 5.0).abs() < 1e-9, "{c:?}");
    }
}

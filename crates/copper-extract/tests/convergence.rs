//! The convergence assessment of a mesh series over every entry of the L and R matrices
//! (`copper_extract::convergence`): on analytic cases (a bar, two coupled bars, a trace over a plane) the entry-wise
//! changes between three pitches, and what the status says when only a mutual, only R, or a pair of meshes that is not
//! comparable is the problem.

use copper_extract::convergence::{Convergence, assess};
use copper_extract::extract::extract;
use copper_extract::hotloop::{MeshRun, run_info};
use copper_extract::layout::{Hint, Layout};
use copper_extract::mesh::MeshSpec;

const MM: i64 = 1_000_000;
const T: f64 = 35e-6;

fn spec(cell_um: f64, coarse_um: f64) -> MeshSpec {
    MeshSpec {
        cell_nm: (cell_um * 1000.0) as i64,
        coarse_nm: (coarse_um * 1000.0) as i64,
        coarsen: false,
        max_filaments: 12000,
        ..MeshSpec::default()
    }
}

/// Two bars 10 mm long, 0.6 mm wide, 1.5 mm apart, each with a port across its ends.
fn two_bars() -> Layout {
    let mut layout = Layout::new([0, -MM, 10 * MM, 3 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let port = |layout: &mut Layout, net: &str, y0: i64| {
        layout.add_rect(top, net, [0, y0, 10 * MM, y0 + 600_000]);
        let a = layout.add_terminal_rect(&format!("{net} start"), top, net, [0, y0, 1000, y0 + 600_000]);
        let b = layout.add_terminal_rect(&format!("{net} end"), top, net, [10 * MM - 1000, y0, 10 * MM, y0 + 600_000]);
        layout.add_port(net, a, b);
    };
    port(&mut layout, "A", 0);
    port(&mut layout, "B", 2_100_000);
    layout
}

/// The series of a layout at three pitches, finest first, as the hot-loop extraction would report it.
fn series(layout: &Layout, pitches_um: [f64; 3], freqs: &[f64]) -> Vec<MeshRun> {
    let roles = ["standard", "coarser", "coarsest"];
    pitches_um
        .iter()
        .zip(roles)
        .map(|(&p, role)| {
            let mut layout = layout.clone();
            // A uniform mesh across the bars at the pitch (the cells along them are the coarse pitch): the cleanest
            // refinement series, as in tests/analytic.rs.
            for y0 in [0, 2_100_000] {
                layout.layers[0].hints.push(Hint {
                    rect: [0, y0, 10 * MM, y0 + 600_000],
                    cell_x_nm: 0,
                    cell_y_nm: (p * 1000.0) as i64,
                });
            }
            let x = extract(&layout, &spec(p, p * 8.0), freqs, false).expect("extract");
            run_info(role, &x)
        })
        .collect()
}

fn describe(name: &str, c: &Convergence) {
    println!("{name}: established {} {:?}", c.established, c.reasons_not_established);
    for (k, s) in [("finest_vs_next", &c.finest_vs_next), ("next_vs_earlier", &c.next_vs_earlier)] {
        if let Some(s) = s {
            println!("  {k}: L% {:?}\n  {k}: R% {:?}", s.l_pct, s.r_pct);
        }
    }
}

fn labels() -> Vec<String> {
    vec!["A".to_owned(), "B".to_owned()]
}

#[test]
fn two_coupled_bars_converge_in_every_entry_of_l_and_r_where_the_crowding_is_resolved() {
    // Pitches 62.5 / 125 / 250 um across the bars, at 1 and 10 MHz (crowding lengths 62 and 6 um).
    let freqs = [1e6, 1e7];
    let runs = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    let c = assess(&runs, &freqs, &labels(), T);
    describe("two bars, 1 and 10 MHz", &c);
    assert!(c.established, "{:?}", c.reasons_not_established);
    let step = c.finest_vs_next.as_ref().unwrap();
    // Every entry of L, self and mutual, at every frequency: well under 3 %.
    for f in &step.l_pct {
        for row in f {
            for &v in row {
                assert!(v < 0.5, "{step:?}");
            }
        }
    }
    // R: the diagonal changes 0.18 % at 1 MHz and 2.4 % at 10 MHz between 62.5 and 125 um; the mutual R (the shared
    // current path, near zero) as a share of the diagonals even less.
    assert!(step.r_pct[0][0][0] < 0.5 && step.r_pct[1][0][0] < 3.0, "{:?}", step.r_pct);
    assert!(step.r_pct[0][0][1] < step.r_pct[0][0][0] + 1e-9 || step.r_pct[0][0][1] < 0.5);
    assert!(c.status_reasons().is_empty(), "{:?}", c.status_reasons());
    // The mutual is a real number: 2.9 nH between bars of 7.7 nH.
    assert!(runs[0].inductance_matrix_nh[0][0][1] > 2.5);
    // The next step is smaller than the first for L (the series converges).
    let earlier = c.next_vs_earlier.as_ref().unwrap();
    assert!(earlier.l_pct[0][0][0] > step.l_pct[0][0][0] * 0.5, "{earlier:?} {step:?}");
}

#[test]
fn r_at_a_frequency_where_the_crowding_is_unresolved_does_not_converge_and_the_status_says_which_entry() {
    // At 100 MHz the crowding length is 0.6 um, far below any pitch: R between the two finest meshes changes by
    // 20 % on the analytic bars (L by 0.3 %): the status names the R matrix, the port and the frequency.
    let freqs = [1e6, 1e8];
    let runs = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    let c = assess(&runs, &freqs, &labels(), T);
    describe("two bars, 1 and 100 MHz", &c);
    assert!(c.established);
    assert!(c.worst_l.as_ref().unwrap().pct < 1.0, "{:?}", c.worst_l);
    let worst = c.worst_r.as_ref().unwrap();
    assert_eq!((worst.matrix, worst.frequency_hz), ("R", 1e8));
    assert!(worst.pct > 10.0, "{worst:?}");
    let reasons = c.status_reasons();
    // Two reasons: the entry is beyond the threshold, and its change grows with refinement (16.7 % then 20.6 %).
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons[1].contains("R of ") && reasons[1].contains("grows with refinement"), "{reasons:?}");
    assert!(c.not_contracting.iter().all(|r| !r.starts_with("L ")), "{:?}", c.not_contracting);
    assert!(
        reasons[0].contains("R matrix") && reasons[0].contains("100 MHz") && reasons[0].contains("R of "),
        "{reasons:?}"
    );
    assert!(c.crowding_length_um[1] < 1.0 && c.crowding_length_um[0] > 50.0, "{:?}", c.crowding_length_um);
}

#[test]
fn only_a_mutual_that_moves_is_named_as_the_mutual() {
    let freqs = [1e6, 1e7];
    let mut runs = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    // The coarser mesh's mutual L is 0.4 nH (5 % of the 7.7 nH diagonals) away from the finest mesh's; nothing else moves.
    for f in 0..2 {
        runs[1].inductance_matrix_nh[f][0][1] = runs[0].inductance_matrix_nh[f][0][1] + 0.4;
        runs[1].inductance_matrix_nh[f][1][0] = runs[0].inductance_matrix_nh[f][1][0] + 0.4;
    }
    let c = assess(&runs, &freqs, &labels(), T);
    let worst = c.worst_l.as_ref().unwrap();
    assert_eq!((worst.row.as_str(), worst.col.as_str(), worst.matrix), ("A", "B", "L"));
    assert!((worst.pct - 100.0 * 0.4 / runs[0].inductance_matrix_nh[0][0][0]).abs() < 0.2, "{worst:?}");
    let reasons = c.status_reasons();
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("L matrix") && reasons[0].contains("between A and B"), "{reasons:?}");
    // The self inductances of the same series would have passed on their own.
    assert!(c.finest_vs_next.as_ref().unwrap().l_pct[0][0][0] < 0.1);
}

#[test]
fn only_r_that_moves_is_named_as_r() {
    let freqs = [1e6, 1e7];
    let mut runs = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    runs[1].resistance_matrix_ohm[0][1][1] *= 1.08;
    let c = assess(&runs[..2], &freqs, &labels(), T);
    let reasons = c.status_reasons();
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("R matrix") && reasons[0].contains("of B at 1 MHz"), "{reasons:?}");
    assert!(c.worst_l.as_ref().unwrap().pct < 0.5);
}

#[test]
fn meshes_that_are_not_comparable_say_so_instead_of_reporting_a_clean_figure() {
    let freqs = [1e6, 1e7];
    let runs = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    // (a) Two meshes of the same density (the 8998 / 8999 of a series squeezed against the budget).
    let mut squeezed = runs.clone();
    squeezed[1].filaments = squeezed[0].filaments + 1;
    squeezed[1].cell_nm = squeezed[0].cell_nm * 3 / 2;
    let c = assess(&squeezed, &freqs, &labels(), T);
    assert!(!c.established);
    assert!(c.reasons_not_established.iter().any(|r| r.contains("same density")), "{:?}", c.reasons_not_established);
    assert!(c.status_reasons()[0].starts_with("mesh convergence is not established"), "{:?}", c.status_reasons());
    // (b) One mesh whose refinement the budget cut short, against one whose refinement was complete.
    let mut cut = runs.clone();
    cut[0].refinements_skipped = 85;
    let c = assess(&cut, &freqs, &labels(), T);
    assert!(!c.established);
    assert!(c.reasons_not_established.iter().any(|r| r.contains("different states of refinement")));
    // (c) Different numbers of unresolved gaps, and of copper pieces.
    let mut gaps = runs.clone();
    gaps[0].unresolved_gap_count = 31;
    gaps[1].unresolved_gap_count = 23;
    gaps[1].copper_components += 1;
    let c = assess(&gaps, &freqs, &labels(), T);
    assert!(c.reasons_not_established.iter().any(|r| r.contains("31 against 23")), "{:?}", c.reasons_not_established);
    assert!(c.reasons_not_established.iter().any(|r| r.contains("copper pieces")));
    // (d) The honest series is comparable, and a single mesh is not established either.
    assert!(assess(&runs, &freqs, &labels(), T).established);
    let one = assess(&runs[..1], &freqs, &labels(), T);
    assert!(!one.established && one.finest_vs_next.is_none());
}

#[test]
fn a_change_that_grows_with_refinement_is_named_even_when_it_is_under_the_threshold() {
    let freqs = [1e6];
    let honest = series(&two_bars(), [62.5, 125.0, 250.0], &freqs);
    assert!(assess(&honest, &freqs, &labels(), T).not_contracting.is_empty());
    // The finest mesh's L of A moves 2 % (under the 3 % threshold) where the step before moved a few hundredths.
    let mut runs = honest.clone();
    runs[0].inductance_matrix_nh[0][0][0] *= 1.02;
    let c = assess(&runs, &freqs, &labels(), T);
    assert_eq!(c.not_contracting.len(), 1, "{:?}", c.not_contracting);
    assert!(
        c.not_contracting[0].contains("L of A at 1 MHz") && c.not_contracting[0].contains("grows with refinement"),
        "{:?}",
        c.not_contracting
    );
    assert_eq!(c.status_reasons(), c.not_contracting);
    // A step that is not comparable is not judged for growth.
    let mut cut = runs.clone();
    cut[2].unresolved_gap_count = 3;
    assert!(assess(&cut, &freqs, &labels(), T).not_contracting.is_empty());
}

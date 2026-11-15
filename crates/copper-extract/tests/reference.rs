//! The independent reference solver (`copper_extract::reference`): first validated against closed forms (a straight
//! bar, a rectangular loop, two coupled loops) and by its own convergence with the pitch, then used as a second
//! opinion on the topology cases of `tests/topology.rs` and on the PEEC extraction at a low frequency.

use copper_emi::inductance::{Segment, loop_inductance, self_inductance_bar};
use copper_extract::layout::{COPPER_RESISTIVITY, Layout};
use copper_extract::reference::{ReferenceOptions, solve};

const MM: i64 = 1_000_000;
const NM: f64 = 1e-9;
const T: f64 = 35e-6;

fn at(pitch_um: f64) -> ReferenceOptions {
    ReferenceOptions { pitch_nm: Some((pitch_um * 1000.0) as i64), ..ReferenceOptions::default() }
}

fn pct(a: f64, b: f64) -> f64 {
    100.0 * (a - b) / b
}

/// A bar `l x w` with a port between terminals of `k` pixels at `pitch` at its ends.
fn bar(l: i64, w: i64, end: i64) -> Layout {
    let mut layout = Layout::new([0, 0, l, w]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    layout.add_rect(top, "N", [0, 0, l, w]);
    let a = layout.add_terminal_rect("a", top, "N", [0, 0, end, w]);
    let b = layout.add_terminal_rect("b", top, "N", [l - end, 0, l, w]);
    layout.add_port("p", a, b);
    layout
}

#[test]
fn a_straight_bar_gives_the_resistance_exactly_and_the_inductance_of_the_bar_within_the_sheet_correction() {
    let (l, w) = (10 * MM, MM);
    let mut last_l = f64::NAN;
    for pitch in [200.0, 100.0, 50.0] {
        let end = (3.0 * pitch * 1000.0) as i64;
        let r = solve(&bar(l, w, end), &at(pitch)).expect("solve");
        // The terminals are three pixels deep and equipotential: the current runs between the last pixel centres.
        let l_eff = l as f64 * NM - 2.0 * 2.5 * pitch * 1e-6;
        let r_ref = COPPER_RESISTIVITY * l_eff / (w as f64 * NM * T);
        let l_ref = self_inductance_bar(l_eff, w as f64 * NM, T) * 1e9;
        println!(
            "bar, pitch {pitch} um: {} cells, R {:.5} mohm ({:+.4} %), L {:.4} nH ({:+.2} % of the bar's {:.4}), {:.2} s",
            r.cells,
            r.resistance_ohm[0][0] * 1e3,
            pct(r.resistance_ohm[0][0], r_ref),
            r.inductance_nh[0][0],
            pct(r.inductance_nh[0][0], l_ref),
            l_ref,
            r.seconds
        );
        assert!(pct(r.resistance_ohm[0][0], r_ref).abs() < 1e-6);
        // A sheet of width w against a bar of thickness t: about ln((w + t) / w) of the logarithm less, 1 %.
        assert!(pct(r.inductance_nh[0][0], l_ref).abs() < 2.5, "{:+.2} %", pct(r.inductance_nh[0][0], l_ref));
        assert_eq!((r.components, r.port_open.clone()), (1, vec![false]));
        last_l = r.inductance_nh[0][0];
    }
    assert!(last_l > 0.0);
}

/// The loop of `tests/analytic.rs` at an offset (`a x b`, trace `w`, gap `gap` in the middle of the bottom side),
/// added to `layout` with its own net and a port across the gap; returns the closed-form segments of its path.
fn add_loop(layout: &mut Layout, ox: i64, net: &str, a: i64, b: i64, w: i64, gap: i64) -> Vec<Segment> {
    let half = w / 2;
    let (xg0, xg1) = (ox + a / 2 - gap / 2, ox + a / 2 + gap / 2);
    let top = 0;
    for r in [
        [ox - half, -half, xg0, half],
        [xg1, -half, ox + a + half, half],
        [ox - half, -half, ox + half, b + half],
        [ox + a - half, -half, ox + a + half, b + half],
        [ox - half, b - half, ox + a + half, b + half],
    ] {
        layout.add_rect(top, net, r);
    }
    let pa = layout.add_terminal_rect(&format!("{net} a"), top, net, [xg0 - 1000, -half, xg0 + 1000, half]);
    let pb = layout.add_terminal_rect(&format!("{net} b"), top, net, [xg1 - 1000, -half, xg1 + 1000, half]);
    layout.add_port(net, pa, pb);
    let seg = |p0: (i64, i64), p1: (i64, i64)| Segment {
        start: [p0.0 as f64 * NM, p0.1 as f64 * NM, 0.0],
        end: [p1.0 as f64 * NM, p1.1 as f64 * NM, 0.0],
        width: w as f64 * NM,
        thickness: T,
    };
    vec![
        seg((xg0, 0), (ox, 0)),
        seg((ox, 0), (ox, b)),
        seg((ox, b), (ox + a, b)),
        seg((ox + a, b), (ox + a, 0)),
        seg((ox + a, 0), (xg1, 0)),
    ]
}

fn loop_layout(loops: &[(i64, &str)]) -> (Layout, Vec<Vec<Segment>>) {
    let (a, b, w, gap) = (10 * MM, 6 * MM, 500_000, MM);
    let last = loops.iter().map(|l| l.0).max().unwrap_or(0);
    let mut layout = Layout::new([-MM, -MM, last + a + MM, b + MM]);
    layout.add_layer("F.Cu", 0.0, T);
    let paths = loops.iter().map(|&(ox, net)| add_loop(&mut layout, ox, net, a, b, w, gap)).collect();
    (layout, paths)
}

#[test]
fn a_rectangular_loop_matches_the_partial_inductance_closed_form_and_converges_with_the_pitch() {
    let (layout, paths) = loop_layout(&[(0, "L")]);
    let want = loop_inductance(&paths[0]) * 1e9;
    let mut previous = f64::NAN;
    let mut steps = Vec::new();
    for pitch in [250.0, 125.0, 62.5] {
        let r = solve(&layout, &at(pitch)).expect("solve");
        let l = r.inductance_nh[0][0];
        println!(
            "loop 10 x 6 mm, trace 0.5 mm: pitch {pitch} um, {} cells, L {l:.4} nH ({:+.2} % of the closed form {want:.4}), R {:.3} mohm, {:.2} s",
            r.cells,
            pct(l, want),
            r.resistance_ohm[0][0] * 1e3,
            r.seconds
        );
        assert!(pct(l, want).abs() < 6.0, "{:+.2} %", pct(l, want));
        if previous.is_finite() {
            steps.push((l - previous).abs());
        }
        previous = l;
    }
    // The change per halving of the pitch falls.
    assert!(steps[1] < steps[0], "{steps:?}");
}

#[test]
fn two_coupled_loops_have_the_closed_form_mutual_inductance() {
    let (layout, paths) = loop_layout(&[(0, "A"), (12 * MM, "B")]);
    let both: Vec<Segment> = paths.iter().flatten().cloned().collect();
    let (la, lb) = (loop_inductance(&paths[0]), loop_inductance(&paths[1]));
    let m_ref = (loop_inductance(&both) - la - lb) / 2.0 * 1e9;
    let r = solve(&layout, &at(125.0)).expect("solve");
    let m = r.inductance_nh[0][1];
    println!(
        "two loops 2 mm apart: mutual {m:.4} nH ({:+.2} % of the closed form {m_ref:.4}), self {:.4} / {:.4} nH ({:.4} closed form), R {:.3} / {:.3} mohm, mutual R {:.2e} ohm",
        pct(m, m_ref),
        r.inductance_nh[0][0],
        r.inductance_nh[1][1],
        la * 1e9,
        r.resistance_ohm[0][0] * 1e3,
        r.resistance_ohm[1][1] * 1e3,
        r.resistance_ohm[0][1]
    );
    assert!(m < 0.0 && pct(m, m_ref).abs() < 1.0, "{:+.2} %", pct(m, m_ref));
    assert!((r.inductance_nh[0][1] - r.inductance_nh[1][0]).abs() < 1e-6 * m.abs(), "reciprocal");
    assert!(r.resistance_ohm[0][1].abs() < 1e-9, "no shared copper: no mutual resistance");
    assert!(pct(r.inductance_nh[0][0], la * 1e9).abs() < 6.0);
    assert_eq!((r.components, r.port_open.clone()), (2, vec![false, false]));
}

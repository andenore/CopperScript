//! Acceptance of the ring-down fit against a long run: a ringing loop whose record decays, cut early, completed by the
//! fit and compared with the decayed record over the whole analysis band (`docs/emi-noise-analysis.md`, section 11).
//!
//! The loop is the one of `shorted_line.rs` (a 10 x 6 mm loop of 0.5 mm trace 0.2 mm over a plane, two 10 x 20 mm
//! pads on both sides of a 1 mm gap, the port across the gap) on a coarse mesh: the geometry is not resolved and the
//! numbers say nothing about the inductance of such a loop, but its record is a real FDTD ring-down of a loop with
//! a resonance in the band, and the port resistance sets the Q of the ring (`Z = V / I` does not depend on it).

use copper_fullwave::fit::{FitCheck, HOLDOUT_ERROR_LIMIT};
use copper_fullwave::solve::Window;
use copper_fullwave::testing::*;
use copper_fullwave::{PortColumn, PortSpec, RegionModel, RegionSpec, Terminal};
use copper_physical::CopperLayer;
use ems::Complex64;

const MM: i64 = 1_000_000;
const BAND: (f64, f64) = (50e6, 150e6);

/// The ringing loop at `cell_nm`, port impedance `z_ref` (the series resistance that damps the ring), the run ended
/// at `cap_ns` of simulated time at the latest, with the ring-down fit on or off.
pub fn ringing_loop(
    cell_nm: i64,
    z_ref: f64,
    ring_fit: bool,
    cap_ns: Option<f64>,
    decay_db: f64,
) -> (PortColumn, usize) {
    let mut board = bare_board();
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, 200_000, "4.3", "0.002"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let (cx, cy, a, b, w, gap) = (50 * MM, 50 * MM, 10 * MM, 6 * MM, 500_000, MM);
    let (x0, x1, y0, y1) = (cx - a / 2, cx + a / 2, cy - b / 2, cy + b / 2);
    let (net, f) = ("L", CopperLayer::Front);
    let (gl, gr) = (cx - gap / 2, cx + gap / 2);
    board.tracks.push(track(net, (x0, y0), (x1, y0), w, f));
    board.tracks.push(track(net, (x0, y0), (x0, y1), w, f));
    board.tracks.push(track(net, (x1, y0), (x1, y1), w, f));
    board.tracks.push(track(net, (x0, y1), (gl - w / 2, y1), w, f));
    board.tracks.push(track(net, (gr + w / 2, y1), (x1, y1), w, f));
    board.zones.push(zone(net, &[f], [gl - 10 * MM, y1 - w / 2, gl, y1 + 20 * MM]));
    board.zones.push(zone(net, &[f], [gr, y1 - w / 2, gr + 10 * MM, y1 + 20 * MM]));
    board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
    let mut spec = RegionSpec::new([gl - 10 * MM, y0 - w / 2, gr + 10 * MM, y1 + 20 * MM]);
    spec.margin_nm = 4 * MM;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.smooth_mesh = true;
    spec.cell_nm = Some(cell_nm);
    spec.ring_fit = ring_fit;
    spec.max_time_ns = cap_ns;
    spec.decay_db = decay_db;
    spec.ports = vec![
        PortSpec::gap(
            "loop",
            Terminal::rect(f, [gl - w, y1 - w / 2, gl, y1 + w / 2]),
            Terminal::rect(f, [gr, y1 - w / 2, gr + w, y1 + w / 2]),
        )
        .with_z_ref(z_ref),
    ];
    let model = RegionModel::build(&board, &spec).expect("model");
    let col = model.solve_port(0).expect("solve");
    println!(
        "ringing loop (cell {} um, port {z_ref} ohm, fit {ring_fit}): {} cells, {} steps, {:.1} ns, converged {}, stopped by fit {}, {:.0} s, resonance {:?} MHz",
        cell_nm / 1000,
        model.mesh.cells,
        col.stats.steps,
        col.stats.simulated_ns,
        col.stats.converged,
        col.stats.stopped_by_fit,
        col.stats.seconds,
        col.first_resonance_hz().map(|f| (f * 1e-6).round())
    );
    (col, model.mesh.cells)
}

/// Agreement of two complex responses over the band: the largest and the rms of `|a - b| / |b|`.
fn agreement(a: &[Complex64], b: &[Complex64]) -> (f64, f64) {
    let rel: Vec<f64> = a.iter().zip(b).map(|(x, y)| (*x - *y).norm() / y.norm().max(1e-30)).collect();
    let max = rel.iter().copied().fold(0.0_f64, f64::max);
    let rms = (rel.iter().map(|r| r * r).sum::<f64>() / rel.len() as f64).sqrt();
    (max, rms)
}

fn band_l(z: &[Complex64], freqs: &[f64]) -> f64 {
    copper_fullwave::solve::inductance_of(freqs, z, BAND.0, BAND.1).expect("band").0
}

/// One cut of a decayed record: what the plain transform, the fit and the check say against the decayed record.
struct CutRow {
    cut_ns: f64,
    plain_l_pct: f64,
    fitted_l_pct: Option<f64>,
    plain: (f64, f64),
    fitted: Option<(f64, f64)>,
    check: Option<FitCheck>,
}

fn cut_rows(col: &PortColumn, cuts_ns: &[f64]) -> Vec<CutRow> {
    let freqs = &col.freqs;
    let reference = col.record.z_at(freqs, None, Window::Rectangular);
    let l_ref = band_l(&reference, freqs);
    let mut rows = Vec::new();
    for &cut_ns in cuts_ns {
        let n = ((cut_ns * 1e-9 - col.record.t0_i) / col.record.dt) as usize;
        if n + 16 > col.record.i.len() {
            continue;
        }
        let rec = copper_fullwave::solve::Record {
            v: col.record.v[..n].to_vec(),
            i: col.record.i[..n].to_vec(),
            ..col.record.clone()
        };
        let plain = rec.z_at(freqs, None, Window::Rectangular);
        let fitted = rec.checked_fit(freqs, 3.9e-9, BAND);
        rows.push(CutRow {
            cut_ns,
            plain_l_pct: 100.0 * (band_l(&plain, freqs) / l_ref - 1.0),
            fitted_l_pct: fitted.as_ref().map(|(d, _, _)| 100.0 * (band_l(&d.z(), freqs) / l_ref - 1.0)),
            plain: agreement(&plain, &reference),
            fitted: fitted.as_ref().map(|(d, _, _)| agreement(&d.z(), &reference)),
            check: fitted.map(|(_, _, c)| c),
        });
    }
    rows
}

fn print_rows(rows: &[CutRow]) {
    for r in rows {
        let hold = r.check.as_ref().and_then(|c| c.holdout.as_ref());
        println!(
            "  cut {:>5.1} ns: plain L {:+7.2} % (response max {:.3} rms {:.3}); fitted L {} (response {}); withheld error {} ({}), spread {}, growth {}, min Re Z/|Z| {}, {}",
            r.cut_ns,
            r.plain_l_pct,
            r.plain.0,
            r.plain.1,
            r.fitted_l_pct.map_or("none".to_owned(), |v| format!("{v:+7.2} %")),
            r.fitted.map_or("none".to_owned(), |(m, s)| format!("max {m:.3} rms {s:.3}")),
            hold.map_or("none".to_owned(), |h| format!("{:.2} %", 100.0 * h.error())),
            hold.map_or("-".to_owned(), |h| format!("{:.1} ns held", h.held_s * 1e9)),
            r.check.as_ref().map_or("-".to_owned(), |c| format!("{:.2} %", 100.0 * c.spread)),
            r.check.as_ref().map_or("-".to_owned(), |c| format!("{:.3}", c.max_growth)),
            r.check.as_ref().map_or("-".to_owned(), |c| format!("{:.3}", c.min_re_z_ratio)),
            r.check.as_ref().map_or("no fit".to_owned(), |c| if c.accepted() {
                "ACCEPTED".to_owned()
            } else {
                format!("REJECTED ({})", c.violations.len())
            })
        );
    }
}

/// Timing and the table of cuts for one port impedance (study).
fn study(cell_nm: i64, z_ref: f64, cuts: &[f64]) -> Vec<CutRow> {
    let (col, _) = ringing_loop(cell_nm, z_ref, false, Some(400.0), -35.0);
    assert!(col.stats.converged, "the reference run must decay (it ran {:.1} ns)", col.stats.simulated_ns);
    println!(
        "decayed run: {:.1} ns, {} steps; first resonance {:?} Hz",
        col.stats.simulated_ns,
        col.stats.steps,
        col.first_resonance_hz()
    );
    let rows = cut_rows(&col, cuts);
    print_rows(&rows);
    rows
}

#[test]
#[ignore = "study: about 2 minutes; run with `cargo test -p copper-fullwave --test ring_fit_acceptance -- --ignored study_cuts --nocapture`"]
fn study_cuts_of_decayed_runs_against_the_decayed_run() {
    let cell = std::env::var("RF_CELL").ok().and_then(|v| v.parse().ok()).unwrap_or(800_000);
    for z_ref in [50.0, 150.0, 300.0] {
        println!("port {z_ref} ohm");
        study(cell, z_ref, &[5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0, 24.0, 32.0, 40.0]);
    }
}

/// The long run and the same loop cut at several times: wherever the check accepts a fit, its complex response over
/// the whole band (50 MHz to 1 GHz, 181 points) agrees with the long run's to the stated bound; the plain transform
/// of the same cuts does not. The long run is the Q 2.7 case (port 50 ohm) on the coarse mesh taken to -70 dB (the run to -35 dB of
/// `solve_port` differs from it by up to 27 % in the response, see the study below).
#[test]
fn an_accepted_fit_agrees_with_the_long_run_over_the_whole_band() {
    let (col, cells) = ringing_loop(1_200_000, 50.0, false, Some(400.0), -70.0);
    assert!(col.stats.converged, "the reference run must decay");
    println!("{cells} cells");
    let rows = cut_rows(&col, &[5.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0]);
    print_rows(&rows);
    let mut accepted = 0;
    for r in &rows {
        let Some(check) = &r.check else { continue };
        if check.accepted() {
            accepted += 1;
            let (max, rms) = r.fitted.expect("accepted fits have a response");
            assert!(max < ACCEPTED_RESPONSE_MAX && rms < ACCEPTED_RESPONSE_RMS, "cut {} ns: {max} {rms}", r.cut_ns);
            assert!(r.fitted_l_pct.expect("L").abs() < 1.0, "cut {} ns: L {:?}", r.cut_ns, r.fitted_l_pct);
        }
    }
    assert!(accepted >= 2, "at least two of the cuts must be accepted");
    // The earliest cut: rejected (a tail of a fraction of a period cannot be tested), and its plain transform is
    // wrong in the response by far more than any accepted fit.
    assert!(!rows[0].check.as_ref().is_some_and(FitCheck::accepted));
    assert!(rows[0].plain.1 > 10.0 * ACCEPTED_RESPONSE_RMS, "{:?}", rows[0].plain);
    assert!(
        rows.iter().all(|r| r
            .check
            .as_ref()
            .is_none_or(|c| c.holdout.as_ref().is_none_or(|h| h.error() < HOLDOUT_ERROR_LIMIT)))
    );
}

/// Bounds on the complex agreement of an accepted fit with the long run, over the whole band: the largest and the
/// rms relative difference of the response. The measured values are in the docs (0.6 % and 0.15 % on the finer mesh).
const ACCEPTED_RESPONSE_MAX: f64 = 0.03;
const ACCEPTED_RESPONSE_RMS: f64 = 0.01;

/// The frequency dependence of the disagreement between a fitted completion and a longer run (study).
#[test]
#[ignore = "study: about 3 minutes; Q 8 loop, the reference run to -70 dB against one to -35 dB, `-- --ignored study_reference --nocapture`"]
fn study_reference_quality_and_where_the_response_disagrees() {
    let (long, _) = ringing_loop(800_000, 150.0, false, Some(400.0), -70.0);
    let freqs = long.freqs.clone();
    let reference = long.record.z_at(&freqs, None, Window::Rectangular);
    // The run to -35 dB is the same record cut where it reached -35 dB.
    let at35 = long.stats.trace.iter().find(|(_, db)| *db < -35.0 && true).map(|(t, _)| *t);
    println!("long run {:.1} ns; -35 dB first reached at {:?} ns", long.stats.simulated_ns, at35);
    if let Some(t) = at35 {
        let n = ((t * 1e-9 - long.record.t0_i) / long.record.dt) as usize;
        let rec = copper_fullwave::solve::Record {
            v: long.record.v[..n].to_vec(),
            i: long.record.i[..n].to_vec(),
            ..long.record.clone()
        };
        let z35 = rec.z_at(&freqs, None, Window::Rectangular);
        let (max, rms) = agreement(&z35, &reference);
        println!("the -35 dB record against the -70 dB record: response max {max:.4} rms {rms:.4}");
    }
    for cut in [10.0, 16.0, 24.0] {
        let n = ((cut * 1e-9 - long.record.t0_i) / long.record.dt) as usize;
        let rec = copper_fullwave::solve::Record {
            v: long.record.v[..n].to_vec(),
            i: long.record.i[..n].to_vec(),
            ..long.record.clone()
        };
        let Some((d, _, c)) = rec.checked_fit(&freqs, 3.9e-9, BAND) else { continue };
        let z = d.z();
        let (max, rms) = agreement(&z, &reference);
        println!("cut {cut} ns: fitted against the -70 dB record: max {max:.4} rms {rms:.4}; {}", c.summary());
        for k in (0..freqs.len()).step_by(12) {
            println!(
                "   {:7.1} MHz |Zref| {:9.3} |Zfit| {:9.3} rel {:.4}",
                freqs[k] * 1e-6,
                reference[k].norm(),
                z[k].norm(),
                (z[k] - reference[k]).norm() / reference[k].norm()
            );
        }
    }
}

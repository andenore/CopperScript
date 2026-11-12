//! Validation of the region model against known answers (synthetic boards).

use copper_fullwave::testing::*;
use copper_fullwave::{Fidelity, PortSpec, RegionModel, RegionSpec, Terminal};
use copper_physical::CopperLayer;

const MM: i64 = 1_000_000;

/// The validation meshes of cases 3a-3e keep the edge rule they were validated with (a third of the lateral
/// cell, nothing for the vertical) so that they stay as they were; `VALIDATION_RULE=default` runs them with
/// the default rule of the model instead (the numbers of the section 11 table, column "default rule").
fn validation_rule(spec: &mut RegionSpec) {
    if std::env::var("VALIDATION_RULE").as_deref() != Ok("default") {
        spec.edge_inset = 1.0 / 3.0;
        spec.edge_inset_z = 0.0;
    }
}

/// Hammerstad-Jensen microstrip impedance and effective permittivity, zero thickness.
fn hammerstad(w_over_h: f64, er: f64) -> (f64, f64) {
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / w_over_h).sqrt());
    let z = if w_over_h <= 1.0 {
        60.0 / er_eff.sqrt() * (8.0 / w_over_h + w_over_h / 4.0).ln()
    } else {
        120.0 * std::f64::consts::PI / (er_eff.sqrt() * (w_over_h + 1.393 + 0.667 * (w_over_h + 1.444).ln()))
    };
    (z, er_eff)
}

/// A straight microstrip of `length` and `width` (nm) on `height` of `er`, ports at both ends.
fn microstrip(height: i64, width: i64, length: i64, er: &str, fidelity: Fidelity, z_ref: f64) -> RegionModel {
    microstrip_cell(height, width, length, er, fidelity, z_ref, None)
}

fn microstrip_cell(
    height: i64,
    width: i64,
    length: i64,
    er: &str,
    fidelity: Fidelity,
    z_ref: f64,
    cell: Option<i64>,
) -> RegionModel {
    let mut board = bare_board();
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, height, er, "0.0"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let (x0, x1, y) = (20 * MM, 20 * MM + length, 50 * MM);
    board.tracks.push(track("SIG", (x0 + width / 2, y), (x1 - width / 2, y), width, CopperLayer::Front));
    board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
    let stub = 500_000;
    let end = |a: i64, b: i64| Terminal::rect(CopperLayer::Front, [a, y - width / 2, b, y + width / 2]);
    let half = (width / 2 + 4 * height).max(3 * MM);
    let mut spec = RegionSpec::new([x0, y - half, x1, y + half]);
    validation_rule(&mut spec);
    spec.margin_nm = 2 * MM;
    spec.band_hz = (0.5e9, 3.0e9);
    spec.fidelity = fidelity;
    spec.cell_nm = cell;
    spec.ports = vec![
        PortSpec::to_plane("p1", end(x0, x0 + stub), CopperLayer::Back).with_z_ref(z_ref),
        PortSpec::to_plane("p2", end(x1 - stub, x1), CopperLayer::Back).with_z_ref(z_ref),
    ];
    RegionModel::build(&board, &spec).expect("model")
}

use copper_fullwave::{PortColumn, PortMatrix, db};
use ems::Complex64;
use std::sync::OnceLock;

/// Characteristic impedance from the two-port S parameters of a line section
/// between ports of real impedance `z_ref`.
fn z0_from_s(s11: Complex64, s21: Complex64, z_ref: f64) -> f64 {
    let num = (Complex64::ONE + s11) * (Complex64::ONE + s11) - s21 * s21;
    let den = (Complex64::ONE - s11) * (Complex64::ONE - s11) - s21 * s21;
    (num / den).sqrt().re * z_ref
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Z0 of a solved microstrip: the median over the band of the S-parameter extraction.
fn extracted_z0(m: &PortMatrix, z_ref: f64) -> f64 {
    median((0..m.freqs.len()).map(|f| z0_from_s(m.s[0][0][f], m.s[1][0][f], z_ref)).collect())
}

/// The 0.5 mm, w/h = 2 microstrip on er 4.3 at coarse fidelity, solved once and shared by 3a and 3e.
fn reference_line() -> &'static PortMatrix {
    static CELL: OnceLock<PortMatrix> = OnceLock::new();
    CELL.get_or_init(|| {
        let model = microstrip(500_000, 1_000_000, 20 * MM, "4.3", Fidelity::Coarse, 50.0);
        let t = std::time::Instant::now();
        let m = model.solve_all().expect("solve");
        println!(
            "reference line: {} cells, {} steps of {:.4} ps, {:.1} s",
            model.mesh.cells,
            m.stats[0].steps,
            m.stats[0].dt_ps,
            t.elapsed().as_secs_f64()
        );
        m
    })
}

#[test]
fn describe_a_microstrip_model_before_running() {
    let model = microstrip(500_000, 1_000_000, 20 * MM, "4.3", Fidelity::Coarse, 50.0);
    let text = model.describe();
    assert!(model.mesh.cells > 10_000 && model.mesh.cells < 2_000_000, "{}", model.mesh.cells);
    assert!(text.contains("estimate before running") && text.contains("flags:"));
    assert_eq!(model.ports.len(), 2);
    assert_eq!(model.mesh.ratio_violations, 0);
}

/// A model that costs more than its budget is refused before the time loop.
#[test]
fn an_over_budget_run_is_refused_not_started() {
    let mut model = microstrip(500_000, 1_000_000, 20 * MM, "4.3", Fidelity::Coarse, 50.0);
    model.spec.budget.max_cell_updates = 1.0e6;
    let err = model.solve_port(0).expect_err("over budget");
    assert!(matches!(err, copper_fullwave::Error::Budget(_)), "{err}");
}

/// The wall-clock guard stops a run that outlasts its budget and reports what it had done.
#[test]
fn a_run_that_outlasts_its_wall_clock_budget_is_stopped_with_partial_statistics() {
    let mut model = microstrip(500_000, 1_000_000, 20 * MM, "4.3", Fidelity::Coarse, 50.0);
    // An optimistic rate lets the pre-run check pass; the guard then holds the run to one second.
    model.spec.budget.assumed_rate = 1.0e13;
    model.spec.budget.max_seconds = 1.0;
    model.spec.budget.wall_margin = 1.0;
    let started = std::time::Instant::now();
    match model.solve_port(0) {
        Err(copper_fullwave::Error::BudgetExceeded(p)) => {
            println!("{p}");
            assert!(p.steps > 0 && p.seconds >= 1.0 && p.cells == model.mesh.cells);
            assert!(started.elapsed().as_secs_f64() < 30.0);
            assert!(p.to_string().starts_with("budget_exceeded"));
        }
        other => panic!("expected the guard to stop the run, got {:?}", other.map(|c| c.stats.steps)),
    }
}

/// 3a: impedance of a microstrip from the simulated S parameters against Hammerstad-Jensen.
#[test]
fn microstrip_impedance_matches_hammerstad_jensen() {
    let m = reference_line();
    let (z_hj, _) = hammerstad(2.0, 4.3);
    let z0 = extracted_z0(m, 50.0);
    println!("3a: Z0 {z0:.2} ohm, Hammerstad-Jensen {z_hj:.2} ohm ({:+.1} %)", 100.0 * (z0 / z_hj - 1.0));
    assert!((z0 / z_hj - 1.0).abs() < 0.05, "Z0 {z0} against {z_hj}");
    // The line is nearly matched to 50 ohm: |S21| near 0 dB across the band.
    let worst = (0..m.freqs.len()).map(|f| db(m.s[1][0][f])).fold(f64::INFINITY, f64::min);
    assert!(worst > -0.5, "worst |S21| {worst} dB");
}

/// 3a, second half: terminated in the extracted impedance the line reflects less than -15 dB.
#[test]
fn microstrip_terminated_in_its_impedance_is_matched() {
    let z0 = extracted_z0(reference_line(), 50.0);
    let model = microstrip(500_000, 1_000_000, 20 * MM, "4.3", Fidelity::Coarse, z0);
    let matched = model.solve_all().expect("solve");
    let s11 = (0..matched.freqs.len()).map(|f| db(matched.s[0][0][f])).fold(f64::NEG_INFINITY, f64::max);
    let s21 = (0..matched.freqs.len()).map(|f| db(matched.s[1][0][f])).fold(f64::INFINITY, f64::min);
    println!("3a: terminated in {z0:.2} ohm: worst S11 {s11:.1} dB, worst S21 {s21:.2} dB over 0.5-3 GHz");
    assert!(s11 < -15.0, "worst S11 {s11} dB");
    assert!(s21 > -0.5, "worst S21 {s21} dB");
}

/// 3e: reciprocity and passivity of the same two-port.
#[test]
fn two_port_is_reciprocal_and_passive() {
    let m = reference_line();
    let mut worst_db = 0.0_f64;
    let mut worst_deg = 0.0_f64;
    let mut worst_power = 0.0_f64;
    for f in 0..m.freqs.len() {
        let (s12, s21) = (m.s[0][1][f], m.s[1][0][f]);
        worst_db = worst_db.max((db(s12) - db(s21)).abs());
        let deg = (s12.arg() - s21.arg()).to_degrees();
        let deg = (deg + 180.0).rem_euclid(360.0) - 180.0;
        worst_deg = worst_deg.max(deg.abs());
        let p1 = m.s[0][0][f].norm_sqr() + m.s[1][0][f].norm_sqr();
        let p2 = m.s[0][1][f].norm_sqr() + m.s[1][1][f].norm_sqr();
        worst_power = worst_power.max(p1).max(p2);
    }
    println!(
        "3e: worst |S12|/|S21| difference {worst_db:.4} dB, phase {worst_deg:.3} deg, worst |S11|^2+|S21|^2 {worst_power:.5}"
    );
    assert!(worst_db < 0.1, "{worst_db} dB");
    assert!(worst_deg < 3.0, "{worst_deg} deg");
    assert!(worst_power < 1.0 + 1e-3, "{worst_power}");
}

#[test]
fn touchstone_export_has_the_header_and_one_line_per_frequency() {
    let m = reference_line();
    let text = m.touchstone();
    assert!(text.contains("# Hz S RI R 50"));
    let data = text.lines().filter(|l| !l.starts_with('!') && !l.starts_with('#')).count();
    assert_eq!(data, m.freqs.len());
}

/// 3a over the dielectric heights of the task, w/h = 2.
#[test]
#[ignore = "about 1 min: microstrip over 0.2 mm and 1.5 mm; run with `cargo test -p copper-fullwave --test validation -- --ignored microstrip_heights --nocapture`"]
fn microstrip_heights() {
    for h in [200_000_i64, 1_500_000] {
        let model = microstrip(h, 2 * h, 20 * MM, "4.3", Fidelity::Coarse, 50.0);
        let t = std::time::Instant::now();
        let m = model.solve_all().expect("solve");
        let (z_hj, _) = hammerstad(2.0, 4.3);
        let z0 = extracted_z0(&m, 50.0);
        println!(
            "3a: h {:.1} mm w {:.1} mm: {} cells, Z0 {z0:.2} vs {z_hj:.2} ({:+.1} %), {:.1} s",
            h as f64 * 1e-6,
            2.0 * h as f64 * 1e-6,
            model.mesh.cells,
            100.0 * (z0 / z_hj - 1.0),
            t.elapsed().as_secs_f64()
        );
        assert!((z0 / z_hj - 1.0).abs() < 0.05, "h {h}: Z0 {z0} against {z_hj}");
    }
}

mod loops {
    use super::*;
    use copper_emi::inductance::{Segment, loop_inductance, loop_inductance_over_plane, via_inductance};

    pub struct Loop {
        pub model: RegionModel,
        pub closed_form: f64,
        pub board: copper_pcb::board::PhysicalBoard,
    }

    /// Options of the loop board.
    #[derive(Clone, Copy)]
    pub struct Opts {
        pub a: i64,
        pub b: i64,
        pub w: i64,
        pub h: i64,
        pub gap: i64,
        pub plane: bool,
        pub fidelity: Fidelity,
        pub margin: i64,
        pub air: Option<i64>,
        pub extend: bool,
        pub plane_margin: Option<i64>,
        /// Override of the copper cell, nm.
        pub cell: Option<i64>,
        /// Override of the 1/3 edge rule fraction.
        pub inset: Option<f64>,
        /// Join the port to the bars' ends by straps (a strap port).
        pub strap: bool,
        /// Scale of the excitation pulse's bandwidth.
        pub pulse: f64,
        /// Smooth meshes (as the hot loop uses).
        pub smooth: bool,
        /// Keep the edge rule the validation meshes were validated with (see `validation_rule`).
        pub pinned: bool,
    }

    pub fn default_opts() -> Opts {
        Opts {
            a: 10 * MM,
            b: 6 * MM,
            w: 500_000,
            h: 200_000,
            gap: 500_000,
            plane: true,
            fidelity: Fidelity::Coarse,
            margin: 5 * MM,
            air: None,
            extend: false,
            plane_margin: None,
            cell: None,
            inset: None,
            strap: false,
            pulse: 1.0,
            smooth: false,
            pinned: true,
        }
    }

    /// A rectangular trace loop of `a` x `b` (nm, centre lines) with trace width `w`, `h` above a plane
    /// on B.Cu (or in air, `plane` false), one in-plane port in a gap `gap` long in the middle of the top bar.
    pub fn build(o: Opts) -> Loop {
        let mut board = bare_board();
        let er = if o.plane { "4.3" } else { "1.0" };
        declare_stackup(
            &mut board,
            &[
                (Some(CopperLayer::Front), 35_000, "", ""),
                (None, o.h, er, "0.0"),
                (Some(CopperLayer::Back), 35_000, "", ""),
            ],
        );
        let (cx, cy) = (50 * MM, 50 * MM);
        let (x0, x1, y0, y1) = (cx - o.a / 2, cx + o.a / 2, cy - o.b / 2, cy + o.b / 2);
        let (net, l, w) = ("L", CopperLayer::Front, o.w);
        board.tracks.push(track(net, (x0, y0), (x1, y0), w, l));
        board.tracks.push(track(net, (x0, y0), (x0, y1), w, l));
        board.tracks.push(track(net, (x1, y0), (x1, y1), w, l));
        let (gl, gr) = (cx - o.gap / 2, cx + o.gap / 2);
        board.tracks.push(track(net, (x0, y1), (gl - w / 2, y1), w, l));
        board.tracks.push(track(net, (gr + w / 2, y1), (x1, y1), w, l));
        if o.plane {
            // The plane reaches `plane_margin` beyond the loop, else the whole board.
            let r = match o.plane_margin {
                Some(m) => [x0 - m, y0 - m, x1 + m, y1 + m],
                None => [0, 0, 100 * MM, 100 * MM],
            };
            board.zones.push(zone("GND", &[CopperLayer::Back], r));
        }
        let mut spec = RegionSpec::new([x0 - w / 2, y0 - w / 2, x1 + w / 2, y1 + w / 2]);
        if o.pinned {
            validation_rule(&mut spec);
        }
        spec.margin_nm = o.margin;
        spec.band_hz = (50e6, 1.0e9);
        spec.fidelity = o.fidelity;
        spec.air_margin_nm = o.air;
        spec.extend_planes = o.extend;
        spec.cell_nm = o.cell;
        spec.pulse_scale = o.pulse;
        spec.smooth_mesh = o.smooth;
        if let Some(f) = o.inset {
            spec.edge_inset = f;
            spec.edge_inset_z = 0.0;
        }
        let port = PortSpec::gap(
            "loop",
            Terminal::rect(l, [gl - w, y1 - w / 2, gl, y1 + w / 2]),
            Terminal::rect(l, [gr, y1 - w / 2, gr + w, y1 + w / 2]),
        );
        spec.ports = vec![if o.strap { port.with_strap() } else { port }];
        let model = RegionModel::build(&board, &spec).expect("model");
        let m = 1e-9;
        let seg = |p: (i64, i64), q: (i64, i64)| Segment {
            start: [p.0 as f64 * m, p.1 as f64 * m, o.h as f64 * m],
            end: [q.0 as f64 * m, q.1 as f64 * m, o.h as f64 * m],
            width: w as f64 * m,
            thickness: 35e-6,
        };
        let bars = [seg((x0, y0), (x1, y0)), seg((x1, y0), (x1, y1)), seg((x1, y1), (x0, y1)), seg((x0, y1), (x0, y0))];
        let closed_form = if o.plane { loop_inductance_over_plane(&bars, 0.0) } else { loop_inductance(&bars) };
        Loop { model, closed_form, board }
    }

    /// The same loop on F.Cu / In1.Cu / B.Cu (0.2 mm each), plane on B.Cu, with the bottom bar
    /// either on F.Cu (`dropped` false) or on In1.Cu behind two vias. Returns the model, the closed
    /// form of the bars at their heights and the two vias' Johnson-Graham estimate (zero when not dropped).
    pub fn build_via(dropped: bool, fidelity: Fidelity) -> (RegionModel, f64, f64) {
        let mut board = bare_board();
        declare_stackup(
            &mut board,
            &[
                (Some(CopperLayer::Front), 35_000, "", ""),
                (None, 200_000, "4.3", "0.0"),
                (Some(CopperLayer::In1), 35_000, "", ""),
                (None, 200_000, "4.3", "0.0"),
                (Some(CopperLayer::Back), 35_000, "", ""),
            ],
        );
        let (cx, cy, a, b, w, gap) = (50 * MM, 50 * MM, 10 * MM, 6 * MM, 500_000, 500_000);
        let (x0, x1, y0, y1) = (cx - a / 2, cx + a / 2, cy - b / 2, cy + b / 2);
        let (net, f, i1) = ("L", CopperLayer::Front, CopperLayer::In1);
        let bottom = if dropped { i1 } else { f };
        board.tracks.push(track(net, (x0, y0), (x1, y0), w, bottom));
        board.tracks.push(track(net, (x0, y0), (x0, y1), w, f));
        board.tracks.push(track(net, (x1, y0), (x1, y1), w, f));
        let (gl, gr) = (cx - gap / 2, cx + gap / 2);
        board.tracks.push(track(net, (x0, y1), (gl - w / 2, y1), w, f));
        board.tracks.push(track(net, (gr + w / 2, y1), (x1, y1), w, f));
        if dropped {
            board.vias.push(via(net, (x0, y0), f, i1));
            board.vias.push(via(net, (x1, y0), f, i1));
        }
        board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
        let mut spec = RegionSpec::new([x0 - w / 2, y0 - w / 2, x1 + w / 2, y1 + w / 2]);
        validation_rule(&mut spec);
        spec.margin_nm = 5 * MM;
        spec.band_hz = (50e6, 1.0e9);
        spec.fidelity = fidelity;
        spec.ports = vec![PortSpec::gap(
            "loop",
            Terminal::rect(f, [gl - w, y1 - w / 2, gl, y1 + w / 2]),
            Terminal::rect(f, [gr, y1 - w / 2, gr + w, y1 + w / 2]),
        )];
        let model = RegionModel::build(&board, &spec).expect("model");
        let m = 1e-9;
        let seg = |p: (i64, i64), q: (i64, i64), h: f64| Segment {
            start: [p.0 as f64 * m, p.1 as f64 * m, h * m],
            end: [q.0 as f64 * m, q.1 as f64 * m, h * m],
            width: w as f64 * m,
            thickness: 35e-6,
        };
        let h_top = 400_000.0;
        let h_bottom = if dropped { 200_000.0 } else { 400_000.0 };
        let bars = [
            seg((x0, y0), (x1, y0), h_bottom),
            seg((x1, y0), (x1, y1), h_top),
            seg((x1, y1), (x0, y1), h_top),
            seg((x0, y1), (x0, y0), h_top),
        ];
        let planar = loop_inductance_over_plane(&bars, 0.0);
        let via_l = if dropped { 2.0 * via_inductance(0.2e-3, 0.3e-3) } else { 0.0 };
        (model, planar, via_l)
    }
}

/// L = Im(Z)/(2 pi f): the mean over `lo..hi` Hz and the spread (max-min)/mean over it.
fn inductance_band(col: &PortColumn, lo: f64, hi: f64) -> (f64, f64) {
    let ls: Vec<f64> = col
        .freqs
        .iter()
        .zip(&col.z_in)
        .filter(|(f, _)| **f >= lo && **f <= hi)
        .map(|(f, z)| z.im / (2.0 * std::f64::consts::PI * f))
        .collect();
    let mean = ls.iter().sum::<f64>() / ls.len() as f64;
    let spread = (ls.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - ls.iter().copied().fold(f64::INFINITY, f64::min))
        / mean;
    (mean, spread)
}

fn report_loop(name: &str, lp: &loops::Loop, col: &PortColumn) -> (f64, f64) {
    let (l, spread) = inductance_band(col, 50e6, 150e6);
    let l500 = inductance_band(col, 450e6, 550e6).0;
    println!(
        "{name}: FDTD {:.3} nH over 50-150 MHz (spread {:.2} %, {:.3} nH at 500 MHz), closed form {:.3} nH ({:+.1} %); {} cells, {} steps of {:.4} ps, {:.1} s, converged {}",
        l * 1e9,
        100.0 * spread,
        l500 * 1e9,
        lp.closed_form * 1e9,
        100.0 * (l / lp.closed_form - 1.0),
        lp.model.mesh.cells,
        col.stats.steps,
        col.stats.dt_ps,
        col.stats.seconds,
        col.stats.converged
    );
    (l, spread)
}

/// 3b, over a plane: 10 x 6 mm loop of 0.5 mm trace, 0.2 mm above the plane.
#[test]
#[ignore = "about 35 s; run with `cargo test -p copper-fullwave --test validation -- --ignored loop_over_plane --nocapture`"]
fn loop_over_plane() {
    let lp = loops::build(loops::default_opts());
    let col = lp.model.solve_port(0).expect("solve");
    let (l, spread) = report_loop("3b plane", &lp, &col);
    assert!(spread < 0.03, "L not flat over 50-150 MHz: {spread}");
    assert!((l / lp.closed_form - 1.0).abs() < 0.15, "{} nH against {} nH", l * 1e9, lp.closed_form * 1e9);
}

/// 3b, free space (er 1, no plane).
#[test]
#[ignore = "about 35 s; run with `cargo test -p copper-fullwave --test validation -- --ignored loop_in_free_space --nocapture`"]
fn loop_in_free_space() {
    let lp = loops::build(loops::Opts { plane: false, ..loops::default_opts() });
    let col = lp.model.solve_port(0).expect("solve");
    let (l, spread) = report_loop("3b free space", &lp, &col);
    assert!(spread < 0.03, "L not flat over 50-150 MHz: {spread}");
    assert!((l / lp.closed_form - 1.0).abs() < 0.15, "{} nH against {} nH", l * 1e9, lp.closed_form * 1e9);
}

/// 3c: the bottom bar dropped to In1.Cu through two vias.
#[test]
#[ignore = "about 80 s (two runs); run with `cargo test -p copper-fullwave --test validation -- --ignored loop_with_vias --nocapture`"]
fn loop_with_vias() {
    let (m0, planar0, _) = loops::build_via(false, Fidelity::Coarse);
    let (m1, planar1, via_l) = loops::build_via(true, Fidelity::Coarse);
    let c0 = m0.solve_port(0).expect("solve");
    let c1 = m1.solve_port(0).expect("solve");
    let (l0, s0) = inductance_band(&c0, 50e6, 150e6);
    let (l1, s1) = inductance_band(&c1, 50e6, 150e6);
    println!(
        "3c: all on F.Cu {:.3} nH (closed form {:.3}); bottom bar on In1.Cu through 2 vias {:.3} nH (closed form bars {:.3} + vias {:.3} = {:.3}); spreads {:.1} %, {:.1} %; FDTD delta {:+.3} nH, closed-form delta {:+.3} nH; {} wires; {:.0} s + {:.0} s",
        l0 * 1e9,
        planar0 * 1e9,
        l1 * 1e9,
        planar1 * 1e9,
        via_l * 1e9,
        (planar1 + via_l) * 1e9,
        100.0 * s0,
        100.0 * s1,
        (l1 - l0) * 1e9,
        (planar1 + via_l - planar0) * 1e9,
        m1.wires.len(),
        c0.stats.seconds,
        c1.stats.seconds
    );
    assert!(m1.wires.len() == 2);
    // Direction: the dropped bar is closer to the plane, so the loop falls, as the closed form says.
    assert_eq!((l1 - l0).signum(), (planar1 + via_l - planar0).signum());
    assert!((l1 / (planar1 + via_l) - 1.0).abs() < 0.15, "{} nH against {} nH", l1 * 1e9, (planar1 + via_l) * 1e9);
}

/// 3d: a 6 x 4 mm loop at coarse and fine fidelity.
#[test]
#[ignore = "about 3 min; run with `cargo test -p copper-fullwave --test validation -- --ignored mesh_convergence --nocapture`"]
fn mesh_convergence() {
    let o = loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), ..loops::default_opts() };
    let coarse = loops::build(o);
    let fine = loops::build(loops::Opts { fidelity: Fidelity::Fine, ..o });
    let cc = coarse.model.solve_port(0).expect("solve");
    let cf = fine.model.solve_port(0).expect("solve");
    let (lc, _) = report_loop("3d coarse", &coarse, &cc);
    let (lf, _) = report_loop("3d fine", &fine, &cf);
    println!("3d: coarse/fine differ by {:+.2} %", 100.0 * (lc / lf - 1.0));
    assert!((lc / lf - 1.0).abs() < 0.05);
}

/// Convergence of L with the cell on the 6 x 4 mm loop, and its dependence on the edge rule.
#[test]
#[ignore = "about 10 min; run with `cargo test -p copper-fullwave --test validation -- --ignored convergence_order --nocapture`"]
fn convergence_order() {
    let base =
        loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), smooth: true, ..loops::default_opts() };
    // The default edge rule, and the earlier one: a third of the lateral cell, nothing for the vertical.
    for inset in [None, Some(1.0 / 3.0)] {
        let mut values = Vec::new();
        for cell in [250_000_i64, 167_000, 125_000, 83_000] {
            let lp = loops::build(loops::Opts { cell: Some(cell), inset, pinned: false, ..base });
            let col = lp.model.solve_port(0).expect("solve");
            let (l, _) = inductance_band(&col, 50e6, 150e6);
            println!(
                "inset {:?} cell {:.3} mm: L {:.4} nH, closed form {:.4} nH, {} cells, {} steps, {:.0} s",
                inset,
                cell as f64 * 1e-6,
                l * 1e9,
                lp.closed_form * 1e9,
                lp.model.mesh.cells,
                col.stats.steps,
                col.stats.seconds
            );
            values.push((cell as f64 * 1e-6, l * 1e9));
        }
        // Observed order from three cells with ratios 1.5 and 1.336.
        let (h0, h1, h2) = (values[0].0, values[1].0, values[2].0);
        let (d01, d12) = (values[0].1 - values[1].1, values[1].1 - values[2].1);
        println!(
            "inset {:?}: differences {:+.4} and {:+.4} nH over h {:.3} -> {:.3} -> {:.3} mm",
            inset, d01, d12, h0, h1, h2
        );
        if inset.is_none() {
            // The default rule: L moves by less than 3 % between a 0.25 and a 0.083 mm cell (the earlier rule: 6.4 %).
            assert!((values[0].1 / values[3].1 - 1.0).abs() < 0.03, "{values:?}");
        }
    }
}

fn env_i(name: &str, default: i64) -> i64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// One loop from the environment (`LP_*`, nm), for the attribution study of the cell dependence.
#[test]
#[ignore = "study; parameters from LP_* environment variables"]
fn loop_probe() {
    let o = loops::Opts {
        a: env_i("LP_A", 6 * MM),
        b: env_i("LP_B", 4 * MM),
        w: env_i("LP_W", 500_000),
        h: env_i("LP_H", 200_000),
        gap: env_i("LP_GAP", 500_000),
        margin: 3 * MM,
        air: Some(5 * MM),
        smooth: env_i("LP_SMOOTH", 1) == 1,
        cell: Some(env_i("LP_CELL", 250_000)),
        pinned: false,
        inset: std::env::var("LP_INSET").ok().map(|v| v.parse::<f64>().unwrap() / 1000.0),
        ..loops::default_opts()
    };
    let lp = loops::build(o);
    let col = lp.model.solve_port(0).expect("solve");
    let (l, spread) = inductance_band(&col, 50e6, 150e6);
    println!(
        "LOOP a {} b {} w {} h {} gap {} cell {} inset {:?}: L {:.4} nH (spread {:.2} %), closed form {:.4} nH, {} cells, {} steps, {:.0} s",
        o.a,
        o.b,
        o.w,
        o.h,
        o.gap,
        env_i("LP_CELL", 250_000),
        o.inset,
        l * 1e9,
        100.0 * spread,
        lp.closed_form * 1e9,
        lp.model.mesh.cells,
        col.stats.steps,
        col.stats.seconds
    );
}

/// The meshes of the convergence sweep, without running them.
#[test]
#[ignore = "diagnostic: prints the mesh of the 6 x 4 mm loop at three cells"]
fn mesh_report_by_cell() {
    let base = loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), ..loops::default_opts() };
    for cell in [250_000_i64, 167_000, 125_000] {
        let lp = loops::build(loops::Opts { cell: Some(cell), ..base });
        println!("cell {cell}:");
        if std::env::var("MESH_DUMP").is_ok() {
            let a = &lp.model.axes[0];
            println!("  kept {:?}", a.kept.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>());
            println!("  cells {:?}", a.cells().iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>());
        }
        for line in lp.model.describe().lines().filter(|l| {
            l.starts_with("mesh") || l.starts_with("geometry") || l.starts_with("sliver") || l.starts_with("estimate")
        }) {
            println!("  {line}");
        }
    }
}

/// Strap port against direct port on the same loop (gap 3 mm): the strap fills most of the gap with PEC and
/// puts the port in its middle, so L changes by about the closed-form strap inductance less the gap's own.
#[test]
#[ignore = "about 1 min (two runs); run with `cargo test -p copper-fullwave --test validation -- --ignored strap_port --nocapture`"]
fn strap_port_against_direct_port() {
    let o = loops::Opts { gap: 3 * MM, ..loops::default_opts() };
    let direct = loops::build(o);
    let strapped = loops::build(loops::Opts { strap: true, ..o });
    assert!(!strapped.model.straps.is_empty() && direct.model.straps.is_empty());
    let (l0, _) = inductance_band(&direct.model.solve_port(0).expect("solve"), 50e6, 150e6);
    let (l1, _) = inductance_band(&strapped.model.solve_port(0).expect("solve"), 50e6, 150e6);
    let bias = copper_fullwave::hotloop::strap_inductance_nh(&strapped.board, &strapped.model).expect("estimate");
    println!(
        "strap port: direct {:.3} nH, strapped {:.3} nH (difference {:+.3} nH), closed-form strap {bias:.3} nH, closed form of the loop {:.3} nH; {}",
        l0 * 1e9,
        l1 * 1e9,
        (l1 - l0) * 1e9,
        direct.closed_form * 1e9,
        strapped.model.ports[0].description
    );
    // The difference stays inside the strap's closed-form inductance (the direct port already has the gap's own).
    assert!(
        ((l1 - l0) * 1e9).abs() < bias.max(0.1 * l0 * 1e9),
        "difference {} nH against strap {bias} nH",
        (l1 - l0) * 1e9
    );
}

/// 6: the plane that ends 5 mm beyond the loop, whole in a window with a 5 mm margin (the reference), cut by a
/// window with a 3 mm margin, and cut by that window but continued through the air into the PML.
#[test]
#[ignore = "about 2 min (three runs); run with `cargo test -p copper-fullwave --test validation -- --ignored extend_planes --nocapture`"]
fn extend_planes_against_truncated() {
    let o = loops::Opts { plane_margin: Some(5 * MM), margin: 5 * MM, ..loops::default_opts() };
    let cases = [
        ("plane ends 5 mm beyond the loop, window margin 5 mm (whole plane)", o),
        ("window margin 3 mm, plane cut at the window edge", loops::Opts { margin: 3 * MM, ..o }),
        ("window margin 3 mm, plane continued into the PML", loops::Opts { margin: 3 * MM, extend: true, ..o }),
    ];
    let mut reference = 0.0;
    for (name, opts) in cases {
        let lp = loops::build(opts);
        let (l, s) = inductance_band(&lp.model.solve_port(0).expect("solve"), 50e6, 150e6);
        if reference == 0.0 {
            reference = l;
        }
        println!(
            "extend_planes: {name}: {:.3} nH ({:+.2} % against the whole plane), spread {:.2} %, flags {:?}",
            l * 1e9,
            100.0 * (l / reference - 1.0),
            100.0 * s,
            lp.model.flags
        );
    }
}

/// 6: the same model at 1, 2 and 6 threads.
#[test]
#[ignore = "about 1 min; run with `cargo test -p copper-fullwave --test validation -- --ignored thread_determinism --nocapture`"]
fn thread_determinism() {
    let o = loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), ..loops::default_opts() };
    let mut model = loops::build(o).model;
    let mut results = Vec::new();
    for threads in [1, 2, 6] {
        model.spec.threads = threads;
        let col = model.solve_port(0).expect("solve");
        println!(
            "threads {threads}: {} steps, {:.1} s, threads used {}",
            col.stats.steps, col.stats.seconds, col.stats.threads
        );
        results.push(col);
    }
    for k in 1..results.len() {
        let identical = results[0]
            .z_in
            .iter()
            .zip(&results[k].z_in)
            .all(|(a, b)| a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits());
        let worst = results[0]
            .z_in
            .iter()
            .zip(&results[k].z_in)
            .map(|(a, b)| (*a - *b).norm() / a.norm())
            .fold(0.0_f64, f64::max);
        let (l0, _) = inductance_band(&results[0], 50e6, 150e6);
        let (lk, _) = inductance_band(&results[k], 50e6, 150e6);
        println!(
            "1 thread against {} threads: Z bit-identical {identical}, steps {} / {}, worst relative difference of Z {worst:.3e}, L {:.6} / {:.6} nH",
            [1, 2, 6][k],
            results[0].stats.steps,
            results[k].stats.steps,
            l0 * 1e9,
            lk * 1e9
        );
    }
}

/// 3: how the run ends. Energy against time and L(f) from truncated, windowed records against the full one.
fn termination_report(name: &str, lp: &loops::Loop, col: &PortColumn) {
    use copper_fullwave::solve::{Window, inductance_of};
    let est = lp.model.estimate();
    println!(
        "{name}: {} cells, dt {:.4} ps, {} steps ({:.2} ns) against {} expected ({:.2} ns), converged {}",
        lp.model.mesh.cells,
        col.stats.dt_ps,
        col.stats.steps,
        col.stats.simulated_ns,
        est.steps,
        est.sim_time * 1e9,
        col.stats.converged
    );
    let marks: Vec<String> = col
        .stats
        .trace
        .iter()
        .step_by((col.stats.trace.len() / 14).max(1))
        .map(|(t, d)| format!("{t:.2}ns:{d:.0}dB"))
        .collect();
    println!("  energy: {}", marks.join(" "));
    let full = col.record.z_at(&col.freqs, None, Window::Rectangular);
    let (lf, _) = inductance_of(&col.freqs, &full, 50e6, 150e6).expect("band");
    let end = col.stats.simulated_ns * 1e-9;
    let mut row = format!("  L full {:.4} nH; truncated at (fraction of the record):", lf * 1e9);
    for frac in [0.3, 0.45, 0.6, 0.8, 1.0] {
        for (tag, w) in [("rect", Window::Rectangular), ("taper", Window::Taper(0.5))] {
            let z = col.record.z_at(&col.freqs, Some(end * frac), w);
            let (l, _) = inductance_of(&col.freqs, &z, 50e6, 150e6).expect("band");
            row.push_str(&format!(" {:.1}ns/{tag} {:+.2} %;", end * frac * 1e9, 100.0 * (l / lf - 1.0)));
        }
    }
    println!("{row}");
}

#[test]
#[ignore = "about 3 min (four runs); run with `cargo test -p copper-fullwave --test validation -- --ignored termination_study --nocapture`"]
fn termination_study() {
    let cases = [
        ("3b over plane", loops::default_opts()),
        ("3b free space", loops::Opts { plane: false, ..loops::default_opts() }),
        ("3d coarse", loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), ..loops::default_opts() }),
        (
            "3d fine",
            loops::Opts {
                a: 6 * MM,
                b: 4 * MM,
                margin: 3 * MM,
                air: Some(5 * MM),
                fidelity: Fidelity::Fine,
                ..loops::default_opts()
            },
        ),
    ];
    for (name, o) in cases {
        let lp = loops::build(o);
        let col = lp.model.solve_port(0).expect("solve");
        termination_report(name, &lp, &col);
    }
}

/// The excitation's bandwidth sets the shortest run: L against the pulse scale.
#[test]
#[ignore = "about 3 min; run with `cargo test -p copper-fullwave --test validation -- --ignored pulse_scale --nocapture`"]
fn pulse_scale_study() {
    let cases = [
        ("3b over plane", loops::default_opts()),
        ("3b free space", loops::Opts { plane: false, ..loops::default_opts() }),
        ("3d coarse", loops::Opts { a: 6 * MM, b: 4 * MM, margin: 3 * MM, air: Some(5 * MM), ..loops::default_opts() }),
    ];
    for (name, o) in cases {
        let mut reference = 0.0;
        for scale in [1.0, 1.5, 2.0, 3.0] {
            let lp = loops::build(loops::Opts { pulse: scale, ..o });
            let col = lp.model.solve_port(0).expect("solve");
            let (l, spread) = inductance_band(&col, 50e6, 150e6);
            let l500 = inductance_band(&col, 450e6, 550e6).0;
            if scale == 1.0 {
                reference = l;
            }
            println!(
                "{name}: pulse x{scale}: L {:.4} nH ({:+.2} % against x1), 500 MHz {:.3} nH, spread {:.2} %, {} steps, {:.1} s, converged {}",
                l * 1e9,
                100.0 * (l / reference - 1.0),
                l500 * 1e9,
                100.0 * spread,
                col.stats.steps,
                col.stats.seconds,
                col.stats.converged
            );
        }
    }
}

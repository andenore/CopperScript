//! Validation of the region model against known answers (synthetic boards).

use copper_fullwave::testing::*;
use copper_fullwave::{Fidelity, PortSpec, RegionModel, RegionSpec, Terminal};
use copper_physical::CopperLayer;

const MM: i64 = 1_000_000;

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
        spec.margin_nm = o.margin;
        spec.band_hz = (50e6, 1.0e9);
        spec.fidelity = o.fidelity;
        spec.air_margin_nm = o.air;
        spec.extend_planes = o.extend;
        spec.ports = vec![PortSpec::gap(
            "loop",
            Terminal::rect(l, [gl - w, y1 - w / 2, gl, y1 + w / 2]),
            Terminal::rect(l, [gr, y1 - w / 2, gr + w, y1 + w / 2]),
        )];
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
        Loop { model, closed_form }
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

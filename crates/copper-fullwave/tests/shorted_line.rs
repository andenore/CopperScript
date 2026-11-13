//! Shorted microstrip sections over a plane, shorted by a wall of vertical wires and driven by a lumped
//! port, against `Z0 tan(beta l)` (Hammerstad-Jensen Z0 and eps_eff): the ground truth for the port
//! model and the cell dependence of the loop inductance (`docs/emi-noise-analysis.md` section 11).

use copper_fullwave::testing::*;
use copper_fullwave::{PortSpec, RegionModel, RegionSpec, Sheet, Terminal, Wire};
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

#[derive(Clone, Copy, Debug)]
pub struct P {
    /// Dielectric height, strip width and length of the line from the wall to the port centre (nm).
    pub h: i64,
    pub w: i64,
    pub l: i64,
    pub cell: i64,
    /// Vertical port of footprint length `d` (nm) at the left end; else an in-plane port in a gap of
    /// `d` nm in the middle of a line shorted at both ends (`l` each side).
    pub vertical: bool,
    pub d: i64,
    /// Wall wires on every `step`-th y line (1 = every line).
    pub step: usize,
    /// Edge rule fractions `(lateral, vertical)`; `None` is the default rule.
    pub inset: Option<(f64, f64)>,
    pub smooth: bool,
    /// Loss tangent of the dielectric (0: lossless).
    pub loss: f64,
    /// Complete and end a record that has not decayed with the ring-down fit.
    pub ring_fit: bool,
    /// Simulated-time cap, ns.
    pub cap_ns: Option<f64>,
}

pub struct Out {
    pub column: copper_fullwave::PortColumn,
    pub l_fdtd: f64,
    pub l_ref: f64,
    pub spread: f64,
    pub cells: usize,
    pub smallest: [f64; 3],
    pub lines: [usize; 3],
    pub steps: u64,
    pub seconds: f64,
    pub port: String,
    pub wall_x: Vec<f64>,
    pub port_x: (f64, f64),
    pub z0: f64,
}

fn bbox(s: &Sheet) -> (f64, f64, f64, f64) {
    let xs: Vec<f64> = s.ring.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = s.ring.iter().map(|p| p.1).collect();
    (
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

fn nearest(lines: &[f64], v: f64) -> f64 {
    lines.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).expect("lines")
}

pub fn run(p: P) -> Out {
    let mut board = bare_board();
    let er = 4.3;
    let loss = format!("{}", p.loss);
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, p.h, "4.3", loss.as_str()),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let f = CopperLayer::Front;
    let y = 50 * MM;
    let cx = 50 * MM;
    let (xa, xb) =
        if p.vertical { (cx - p.d / 2, cx - p.d / 2 + p.l) } else { (cx - p.l - p.d / 2, cx + p.l + p.d / 2) };
    let mut spec = RegionSpec::new([xa, y - 3 * MM, xb, y + 3 * MM]);
    if p.vertical {
        board.tracks.push(track("S", (xa + p.w / 2, y), (xb - p.w / 2, y), p.w, f));
        spec.ports = vec![PortSpec::to_plane(
            "p",
            Terminal::rect(f, [xa, y - p.w / 2, xa + p.d, y + p.w / 2]),
            CopperLayer::Back,
        )];
    } else {
        let (gl, gr) = (cx - p.d / 2, cx + p.d / 2);
        board.tracks.push(track("S", (xa + p.w / 2, y), (gl - p.w / 2, y), p.w, f));
        board.tracks.push(track("S", (gr + p.w / 2, y), (xb - p.w / 2, y), p.w, f));
        spec.ports = vec![PortSpec::gap(
            "p",
            Terminal::rect(f, [gl - p.w, y - p.w / 2, gl, y + p.w / 2]),
            Terminal::rect(f, [gr, y - p.w / 2, gr + p.w, y + p.w / 2]),
        )];
    }
    board.zones.push(zone("GND", &[CopperLayer::Back], [xa - 20 * MM, 0, xb + 20 * MM, 100 * MM]));
    spec.margin_nm = 3 * MM;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.cell_nm = Some(p.cell);
    spec.smooth_mesh = p.smooth;
    spec.ring_fit = p.ring_fit;
    spec.max_time_ns = p.cap_ns;
    if let Some((a, b)) = p.inset {
        spec.edge_inset = a;
        spec.edge_inset_z = b;
    }
    let mut model = RegionModel::build(&board, &spec).expect("model");
    // The walls: wires on the mesh lines at the free ends of the strips, across their inset width.
    let strips: Vec<Sheet> = model.sheets.iter().filter(|s| s.kind == "track").cloned().collect();
    let zt = strips[0].z;
    let zb = model.sheets.iter().find(|s| s.kind == "zone").map(|s| s.z).expect("plane");
    let mut wall_x = Vec::new();
    if p.vertical {
        wall_x.push(bbox(&strips[0]).1);
    } else {
        let mut v: Vec<_> = strips.iter().map(bbox).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        wall_x.push(v[0].0);
        wall_x.push(v[v.len() - 1].1);
    }
    let b0 = bbox(&strips[0]);
    let ylines: Vec<f64> =
        model.axes[1].lines.iter().copied().filter(|v| *v >= b0.2 - 1e-6 && *v <= b0.3 + 1e-6).collect();
    let mut picked: Vec<f64> = ylines.iter().copied().step_by(p.step).collect();
    if let Some(last) = ylines.last()
        && !picked.contains(last)
    {
        picked.push(*last);
    }
    let xlines = model.axes[0].lines.clone();
    let wall_lines: Vec<f64> = wall_x.iter().map(|w| nearest(&xlines, *w)).collect();
    for &xline in &wall_lines {
        for &yy in &picked {
            model.wires.push(Wire {
                kind: "via",
                net: "S".into(),
                net_id: 1,
                x: xline,
                y: yy,
                z_top: zt,
                z_bottom: zb,
                radius: 0.0,
            });
        }
    }
    if std::env::var("SL_XLINES").is_ok() {
        let c: Vec<String> = model.axes[0].cells().iter().map(|c| format!("{c:.3}")).collect();
        println!("XCELLS {}", c.join(" "));
    }
    if std::env::var("SL_ZLINES").is_ok() {
        println!(
            "ZLINES {:?}
YLINES {:?}",
            model.axes[2].lines, ylines
        );
    }
    let port = &model.ports[0];
    let port_x = (port.start[0].min(port.stop[0]), port.start[0].max(port.stop[0]));
    let col = model.solve_port(0).expect("solve");
    let (z0, ee) = hammerstad(p.w as f64 / p.h as f64, er);
    let c0 = 299_792_458.0;
    let tanline = |len_mm: f64, fr: f64| z0 * (2.0 * std::f64::consts::PI * fr * ee.sqrt() / c0 * len_mm * 1e-3).tan();
    let centre = 0.5 * (port_x.0 + port_x.1);
    let lens: Vec<f64> = wall_lines.iter().map(|w| (w - centre).abs()).collect();
    let (mut ls, mut rs) = (Vec::new(), Vec::new());
    for (fr, z) in col.freqs.iter().zip(&col.z_in) {
        if *fr >= 50e6 && *fr <= 150e6 {
            let w = 2.0 * std::f64::consts::PI * fr;
            ls.push(z.im / w);
            rs.push(lens.iter().map(|l| tanline(*l, *fr)).sum::<f64>() / w);
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let l_fdtd = mean(&ls);
    let spread = (ls.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - ls.iter().copied().fold(f64::INFINITY, f64::min))
        / l_fdtd;
    Out {
        column: col.clone(),
        l_fdtd,
        l_ref: mean(&rs),
        spread,
        cells: model.mesh.cells,
        smallest: model.mesh.smallest,
        lines: model.mesh.lines,
        steps: col.stats.steps,
        seconds: col.stats.seconds,
        port: model.ports[0].description.clone(),
        wall_x: wall_lines,
        port_x,
        z0,
    }
}

fn env_i(name: &str, default: i64) -> i64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// One shorted-line case from the environment (`SL_*`), for the attribution study.
#[test]
#[ignore = "study; parameters from SL_* environment variables"]
fn shorted_probe() {
    let p = P {
        h: env_i("SL_H", 500_000),
        w: env_i("SL_W", 1_000_000),
        l: env_i("SL_L", 15_000_000),
        cell: env_i("SL_CELL", 250_000),
        vertical: env_i("SL_VERT", 1) == 1,
        d: env_i("SL_D", 250_000),
        step: env_i("SL_STEP", 1) as usize,
        inset: std::env::var("SL_INSET")
            .ok()
            .map(|v| (v.parse::<f64>().unwrap() / 1000.0, env_i("SL_INSETZ", 0) as f64 / 1000.0)),
        smooth: env_i("SL_SMOOTH", 1) == 1,
        loss: std::env::var("SL_LOSS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0),
        ring_fit: false,
        cap_ns: None,
    };
    let o = run(p);
    println!(
        "SHORTED {:?}\n  L_fdtd {:.4} nH, L_ref {:.4} nH, diff {:+.4} nH ({:+.2} %), spread {:.2} %, Z0 {:.2}, {} cells ({:?} lines, smallest {:.3?} mm), {} steps, {:.0} s\n  {}\n  wall x {:?}, port x {:?}",
        p,
        o.l_fdtd * 1e9,
        o.l_ref * 1e9,
        (o.l_fdtd - o.l_ref) * 1e9,
        100.0 * (o.l_fdtd / o.l_ref - 1.0),
        100.0 * o.spread,
        o.z0,
        o.cells,
        o.lines,
        o.smallest,
        o.steps,
        o.seconds,
        o.port,
        o.wall_x,
        o.port_x
    );
}

/// The line alone: two lengths, the difference of their inductances against the reference's, which removes the
/// port, the wall and every end effect (`SL_*` as `shorted_probe`, `SL_L` and half of it).
#[test]
#[ignore = "study; parameters from SL_* environment variables"]
fn shorted_difference() {
    let p = P {
        h: env_i("SL_H", 500_000),
        w: env_i("SL_W", 1_000_000),
        l: env_i("SL_L", 10_000_000),
        cell: env_i("SL_CELL", 250_000),
        vertical: env_i("SL_VERT", 1) == 1,
        d: env_i("SL_D", 250_000),
        step: env_i("SL_STEP", 1) as usize,
        inset: std::env::var("SL_INSET")
            .ok()
            .map(|v| (v.parse::<f64>().unwrap() / 1000.0, env_i("SL_INSETZ", 0) as f64 / 1000.0)),
        smooth: env_i("SL_SMOOTH", 1) == 1,
        loss: 0.0,
        ring_fit: false,
        cap_ns: None,
    };
    let long = run(p);
    let short = run(P { l: p.l / 2, ..p });
    let (dl, dr) = (long.l_fdtd - short.l_fdtd, long.l_ref - short.l_ref);
    println!(
        "DIFF h {} w {} cell {} inset {:?} d {}: long {:.4} short {:.4} nH, offsets {:+.4} / {:+.4} nH, line error {:+.2} % (delta L {:.4} against {:.4} nH), {} cells",
        p.h,
        p.w,
        p.cell,
        p.inset,
        p.d,
        long.l_fdtd * 1e9,
        short.l_fdtd * 1e9,
        (long.l_fdtd - long.l_ref) * 1e9,
        (short.l_fdtd - short.l_ref) * 1e9,
        100.0 * (dl / dr - 1.0),
        dl * 1e9,
        dr * 1e9,
        long.cells
    );
}

fn band_l(z: &[ems::Complex64], freqs: &[f64]) -> f64 {
    copper_fullwave::solve::inductance_of(freqs, z, 50e6, 150e6).expect("band").0
}

/// A 10 x 6 mm loop of 0.5 mm trace, 0.2 mm over a plane, with two 10 x 20 mm pads on both sides of a 1 mm gap in the
/// top bar and the port across the gap: the pads are 2 x 38 pF to the plane (19 pF across the port), which
/// resonates with the loop inductance at about 400 MHz, with the Q of the dielectric (`loss` is its tangent).
fn ringing_loop(z_ref: f64, ring_fit: bool, cap_ns: Option<f64>) -> (copper_fullwave::PortColumn, RegionModel) {
    let mut board = bare_board();
    let loss = "0.002";
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, 200_000, "4.3", loss),
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
    spec.cell_nm = Some(400_000);
    spec.ring_fit = ring_fit;
    spec.max_time_ns = cap_ns;
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
        "ringing loop (port {z_ref} ohm, {}, fit {ring_fit}): {} cells, {} steps, {:.1} ns, converged {}, stopped by fit {}, {:.0} s, resonance {:?} MHz",
        spec.max_time_ns.map_or(String::new(), |t| format!("cap {t} ns")),
        model.mesh.cells,
        col.stats.steps,
        col.stats.simulated_ns,
        col.stats.converged,
        col.stats.stopped_by_fit,
        col.stats.seconds,
        col.first_resonance_hz().map(|f| (f * 1e-6).round())
    );
    (col, model)
}

/// The inductance at 50-150 MHz of the ringing loop from a decayed run, from the same record cut at several
/// times and completed by the fit, and from a run that the fit ended. Z = V/I does not depend on the port
/// impedance, which sets the damping of the ring (the series resistance of the port: Q about 2.7 at 50 ohm,
/// about 27 at 500 ohm).
/// Z and S of a run completed by the ring-down fit come from the same completed spectra:
/// `S11 = (Z - Z0)/(Z + Z0)` of the returned `Z`, also through `PortMatrix` and Touchstone.
#[test]
#[ignore = "about 65 s; run with `cargo test -p copper-fullwave --test shorted_line -- --ignored a_fit_completed_run`; the unit tests in fit.rs cover the same logic on synthetic records"]
fn a_fit_completed_run_has_s_and_z_from_the_same_spectra() {
    let (col, _) = ringing_loop(500.0, true, Some(18.0));
    assert!(!col.stats.converged && col.fit.is_some(), "the run is cut and completed by the fit: {:?}", col.fit);
    assert!(col.uncorrected_ports.is_empty());
    let z0 = 500.0;
    for (k, (z, s)) in col.z_in.iter().zip(&col.s[0]).enumerate() {
        let want = (*z - z0) / (*z + z0);
        assert!((*s - want).norm() < 1e-9 * (1.0 + want.norm()), "f[{k}]: S11 {s} vs {want} from Z {z}");
    }
    // The plain transform of the cut record is a different response: the fit changed Z.
    let plain = col.record.z_at(&col.freqs, None, copper_fullwave::solve::Window::Rectangular);
    let moved = plain.iter().zip(&col.z_in).map(|(a, b)| (*a - *b).norm() / b.norm()).fold(0.0_f64, f64::max);
    assert!(moved > 1e-3, "the fit left Z unchanged ({moved})");
}

#[test]
#[ignore = "about 10 minutes; run with `cargo test -p copper-fullwave --test shorted_line -- --ignored ring_fit_against_decayed_run --nocapture`"]
fn ring_fit_against_decayed_run() {
    // Q 2.7: decays in 10 ns.
    let (low_q, _) = ringing_loop(50.0, false, Some(400.0));
    let freqs = low_q.freqs.clone();
    let l_low = band_l(&low_q.z_in, &freqs);
    // Q 27: the reference is a run to -35 dB.
    let (decayed, _) = ringing_loop(500.0, false, Some(400.0));
    assert!(decayed.stats.converged, "the reference run must decay");
    let rec = &decayed.record;
    let l_decayed = band_l(&decayed.z_in, &freqs);
    println!(
        "decayed runs: L {:.4} nH (port 500 ohm, {:.1} ns), {:.4} nH (port 50 ohm, {:.1} ns) over 50-150 MHz",
        l_decayed * 1e9,
        decayed.stats.simulated_ns,
        l_low * 1e9,
        low_q.stats.simulated_ns
    );
    for cut_ns in [5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 24.0, 40.0] {
        let cut = cut_ns * 1e-9;
        let plain = rec.z_at(&freqs, Some(cut), copper_fullwave::solve::Window::Rectangular);
        match rec.z_fitted(&freqs, Some(cut), 3.9e-9, (50e6, 150e6)) {
            Some((z, info)) => println!(
                "cut at {cut_ns} ns: plain {:+.2} %, fitted {:+.2} % (orders spread {:.2} %, ring {:?})",
                100.0 * (band_l(&plain, &freqs) / l_decayed - 1.0),
                100.0 * (band_l(&z, &freqs) / l_decayed - 1.0),
                100.0 * info.spread,
                info.ring_hz.map(|(f, q)| ((f * 1e-6).round(), (q * 10.0).round() / 10.0))
            ),
            None => {
                println!(
                    "cut at {cut_ns} ns: plain {:+.2} %, no fit",
                    100.0 * (band_l(&plain, &freqs) / l_decayed - 1.0)
                )
            }
        }
    }
    let (live, _) = ringing_loop(500.0, true, Some(400.0));
    let l_live = band_l(&live.z_in, &freqs);
    println!(
        "fit-ended run: L {:+.2} % against the decayed run, {} steps against {}, {:.1} ns against {:.1} ns, stopped by fit {}, {:.0} s against {:.0} s",
        100.0 * (l_live / l_decayed - 1.0),
        live.stats.steps,
        decayed.stats.steps,
        live.stats.simulated_ns,
        decayed.stats.simulated_ns,
        live.stats.stopped_by_fit,
        live.stats.seconds,
        decayed.stats.seconds
    );
    assert!(live.stats.stopped_by_fit);
    assert!((l_live / l_decayed - 1.0).abs() < 0.02);
}

fn standard(h: i64, w: i64, l: i64, cell: i64, vertical: bool) -> P {
    P {
        h,
        w,
        l,
        cell,
        vertical,
        d: if vertical { cell } else { 2 * MM },
        step: 1,
        inset: None,
        smooth: true,
        loss: 0.0,
        ring_fit: false,
        cap_ns: None,
    }
}

/// The cheap regression test of the default edge rule (about 15 s): a 10 mm shorted line at the 0.25 mm cell the rule
/// was fitted at. The line term is 2.92 nH; what the model adds is the end effect of the port and of the wall,
/// +0.126 nH measured (+4.3 %), and the rule is what sets it: edges on the mesh lines as drawn read -0.187 nH (-6.4 %),
/// the rule of a third +0.101 nH.
#[test]
fn default_edge_rule_keeps_a_short_shorted_line_at_the_closed_form() {
    let o = run(standard(500_000, 1_000_000, 10 * MM, 250_000, true));
    let diff = (o.l_fdtd - o.l_ref) * 1e9;
    println!(
        "short shorted line: {:.4} nH against {:.4} nH ({diff:+.4} nH), {} cells, {} steps",
        o.l_fdtd * 1e9,
        o.l_ref * 1e9,
        o.cells,
        o.steps
    );
    assert!(diff > 0.03 && diff < 0.25, "end effect {diff} nH");
    assert!((o.l_fdtd / o.l_ref - 1.0).abs() < 0.06, "{} nH against {} nH", o.l_fdtd * 1e9, o.l_ref * 1e9);
}

/// The ground truth of the port and edge model: shorted lines of 20 mm against `Z0 tan(beta l)`, at the
/// standard cell and one step finer. The line term dominates (4 to 6 nH); what remains is the end effect of
/// the port and of the wall (0.05 to 0.2 nH, independent of the cell).
#[test]
#[ignore = "about 4 minutes; run with `cargo test -p copper-fullwave --test shorted_line -- --ignored shorted_line_against_the_closed_form --nocapture`"]
fn shorted_line_against_the_closed_form() {
    // (height, width, vertical port): the fitted wide and narrow lines and two geometries the edge rule was not fitted on.
    let cases = [
        (500_000, 1_000_000, true, "w/h 2, vertical port (the standard case)"),
        (200_000, 500_000, true, "w/h 2.5, h < cell, vertical port"),
        (500_000, 500_000, true, "w/h 1 (not fitted)"),
        (300_000, 1_500_000, true, "w/h 5 (not fitted)"),
        (500_000, 1_000_000, false, "in-plane port in a 2 mm gap, shorted at both ends"),
    ];
    let mut failures = Vec::new();
    for (h, w, vertical, name) in cases {
        let l = if vertical { 20 * MM } else { 10 * MM };
        let coarse = run(standard(h, w, l, 250_000, vertical));
        let fine = run(standard(h, w, l, 167_000, vertical));
        let err = |o: &Out| 100.0 * (o.l_fdtd / o.l_ref - 1.0);
        let slope = (coarse.l_fdtd - fine.l_fdtd) * 1e9 / (0.250 - 0.167);
        println!(
            "{name}: 0.25 mm {:.4} nH ({:+.2} % of {:.4} nH), 0.167 mm {:.4} nH ({:+.2} %), cell dependence {:+.2} nH/mm",
            coarse.l_fdtd * 1e9,
            err(&coarse),
            coarse.l_ref * 1e9,
            fine.l_fdtd * 1e9,
            err(&fine),
            slope
        );
        // 3 % of the closed form at the standard cell; the cell dependence of the 20 mm line under 1.5 nH/mm
        // (the line of the standard case: under 1; the line thinner than a cell is the hardest, 1.1).
        if err(&coarse).abs() >= 3.0 || slope.abs() >= 1.5 {
            failures.push(format!("{name}: {:+.2} %, {slope:+.2} nH/mm", err(&coarse)));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

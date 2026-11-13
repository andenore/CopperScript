//! The strap bias of a strap port, measured: a microstrip line of known inductance driven through the strap of a
//! port, the strap length varied (`docs/emi-noise-analysis.md`, section 11).
//!
//! A shorted microstrip line (width `w`, length `l`, over a plane, shorted by a wall of wires at its far end)
//! has a pad `A` at its start. A second pad `B`, shorted to the plane by a wall of wires at its far end, lies at a
//! distance `d` from `A` along the line's axis. The port is across `A` and `B` and carries a strap
//! (`PortSpec::with_strap`): bars of the narrowest pad dimension from the pad edges to a port gap of two cells in the
//! middle, which is the construction of a strap port on a real board. The loop is then the line, the strap and
//! `B`'s wall, so the inductance is `L(d) = L_line + L_strap(d) + L_ends`: the line is the same at every `d`, and
//! the increase of `L` with `d` is the strap's own inductance (a microstrip bar of its width over the same plane),
//! measured, to set beside `port_strap_nh` (the closed form of the strap bars over the plane, which the hot-loop
//! result reports as the bias). The three cases are an in-plane strap over a plane (two geometries) and a strap in the
//! air on posts over a foreign pad (the construction used under the exposed pad of a package).

use copper_fullwave::testing::*;
use copper_fullwave::{PortSpec, RegionModel, RegionSpec, Sheet, Terminal, Wire};
use copper_physical::CopperLayer;

const MM: i64 = 1_000_000;

fn hammerstad(w_over_h: f64, er: f64) -> (f64, f64) {
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / w_over_h).sqrt());
    let z = if w_over_h <= 1.0 {
        60.0 / er_eff.sqrt() * (8.0 / w_over_h + w_over_h / 4.0).ln()
    } else {
        120.0 * std::f64::consts::PI / (er_eff.sqrt() * (w_over_h + 1.393 + 0.667 * (w_over_h + 1.444).ln()))
    };
    (z, er_eff)
}

fn nearest(lines: &[f64], v: f64) -> f64 {
    lines.iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).expect("lines")
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

#[derive(Clone, Copy, Debug)]
struct Case {
    h: i64,
    w: i64,
    l: i64,
    /// A foreign pad between A and B, so that the strap has to run in the air on posts.
    air: bool,
    cell: i64,
}

#[derive(Debug)]
struct Out {
    d_mm: f64,
    l_nh: f64,
    strap_cf_nh: f64,
    strap_bars: usize,
    port: String,
    cells: usize,
    seconds: f64,
    in_air: bool,
}

fn run(c: Case, d: i64) -> Out {
    let started = std::time::Instant::now();
    let mut board = bare_board();
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, c.h, "4.3", "0"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let f = CopperLayer::Front;
    let y = 50 * MM;
    let pad = 600_000;
    // The line from x = 0 to l, pad B from -d - pad to -d.
    board.tracks.push(track("S", (c.w / 2, y), (c.l - c.w / 2, y), c.w, f));
    board.tracks.push(track("G", (-d - pad + c.w / 2, y), (-d - c.w / 2, y), c.w, f));
    if c.air {
        // Foreign copper in the gap between the pads, 0.3 mm clear of both: the strap cannot lie on it.
        board.tracks.push(track("X", (-d + 300_000 + c.w / 2, y), (-300_000 - c.w / 2, y), c.w, f));
    }
    board.zones.push(zone("GND", &[CopperLayer::Back], [-d - 20 * MM, 0, c.l + 20 * MM, 100 * MM]));
    let mut spec = RegionSpec::new([-d - pad - MM, y - 3 * MM, c.l + MM, y + 3 * MM]);
    spec.margin_nm = 3 * MM;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.cell_nm = Some(c.cell);
    spec.smooth_mesh = true;
    spec.ring_fit = false;
    spec.ports = vec![
        PortSpec::gap(
            "p",
            Terminal::rect(f, [0, y - c.w / 2, pad, y + c.w / 2]),
            Terminal::rect(f, [-d - pad, y - c.w / 2, -d, y + c.w / 2]),
        )
        .with_strap(),
    ];
    let mut model = RegionModel::build(&board, &spec).expect("model");
    assert!(!model.straps.is_empty(), "the port has a strap");
    let strips: Vec<Sheet> = model.sheets.iter().filter(|s| s.kind == "track").cloned().collect();
    let zt = strips[0].z;
    let zb = model.sheets.iter().find(|s| s.kind == "zone").map(|s| s.z).expect("plane");
    let (xl, yl) = (model.axes[0].lines.clone(), model.axes[1].lines.clone());
    // Walls: at the far end of the line (net S) and at the far end of pad B (net G).
    for (net, x_edge, net_id) in [
        ("S", strips.iter().filter(|s| s.net == "S").map(|s| bbox(s).1).fold(f64::NEG_INFINITY, f64::max), 1),
        ("G", strips.iter().filter(|s| s.net == "G").map(|s| bbox(s).0).fold(f64::INFINITY, f64::min), 2),
    ] {
        let s = strips.iter().find(|s| s.net == net).expect("strip");
        let b = bbox(s);
        let x = nearest(&xl, x_edge);
        for yy in yl.iter().copied().filter(|v| *v >= b.2 - 1e-6 && *v <= b.3 + 1e-6) {
            model.wires.push(Wire {
                kind: "via",
                net: net.into(),
                net_id,
                x,
                y: yy,
                z_top: zt,
                z_bottom: zb,
                radius: 0.0,
            });
        }
    }
    let strap_cf = copper_fullwave::hotloop::strap_inductance_nh(&board, &model).expect("closed form of the strap");
    let col = model.solve_port(0).expect("solve");
    let (l, _) = copper_fullwave::solve::inductance_of(&col.freqs, &col.z_in, 50e6, 150e6).expect("band");
    Out {
        d_mm: d as f64 * 1e-6,
        l_nh: l * 1e9,
        strap_cf_nh: strap_cf,
        strap_bars: model.straps.len(),
        port: model.ports[0].description.clone(),
        cells: model.mesh.cells,
        seconds: started.elapsed().as_secs_f64(),
        in_air: model.flags.iter().any(|f| f == "port_strap_in_air"),
    }
}

/// The line alone, `Z0 tan(beta l) / omega` of the zero-thickness line over 50-150 MHz, nH.
fn line_closed_form(h: i64, w: i64, l: i64) -> f64 {
    let (z0, ee) = hammerstad(w as f64 / h as f64, 4.3);
    let len = l as f64 * 1e-9;
    let ls: Vec<f64> = (0..=10)
        .map(|k| {
            let fr = 50e6 + 10e6 * f64::from(k);
            z0 * (2.0 * std::f64::consts::PI * fr * ee.sqrt() / 299_792_458.0 * len).tan()
                / (2.0 * std::f64::consts::PI * fr)
        })
        .collect();
    1e9 * ls.iter().sum::<f64>() / ls.len() as f64
}

/// Least-squares line `L = a + b d` through `(d, L)`.
fn fit(points: &[(f64, f64)]) -> (f64, f64) {
    let n = points.len() as f64;
    let (sx, sy): (f64, f64) = points.iter().fold((0.0, 0.0), |s, p| (s.0 + p.0, s.1 + p.1));
    let (sxx, sxy): (f64, f64) = points.iter().fold((0.0, 0.0), |s, p| (s.0 + p.0 * p.0, s.1 + p.0 * p.1));
    let b = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    ((sy - b * sx) / n, b)
}

fn series(name: &str, c: Case, ds: &[i64]) -> Vec<Out> {
    let line = line_closed_form(c.h, c.w, c.l);
    println!(
        "strap bias, {name}: h {} w {} l {} mm, cell {} mm; the line alone (Hammerstad-Jensen, zero thickness) {line:.4} nH",
        c.h as f64 * 1e-6,
        c.w as f64 * 1e-6,
        c.l as f64 * 1e-6,
        c.cell as f64 * 1e-6
    );
    let outs: Vec<Out> = ds.iter().map(|d| run(c, *d)).collect();
    for o in &outs {
        println!(
            "  d {:.2} mm: L {:.4} nH, closed-form strap {:.4} nH ({} bars{}), {} cells, {:.0} s; {}",
            o.d_mm,
            o.l_nh,
            o.strap_cf_nh,
            o.strap_bars,
            if o.in_air { ", in the air" } else { "" },
            o.cells,
            o.seconds,
            o.port
        );
    }
    let measured = fit(&outs.iter().map(|o| (o.d_mm, o.l_nh)).collect::<Vec<_>>());
    let closed = fit(&outs.iter().map(|o| (o.d_mm, o.strap_cf_nh)).collect::<Vec<_>>());
    println!(
        "  measured L(d) = {:.4} + {:.4} nH/mm x d; closed-form strap(d) = {:.4} + {:.4} nH/mm x d; L(0) against the line alone {:+.4} nH; slope measured / closed form {:.3}",
        measured.0,
        measured.1,
        closed.0,
        closed.1,
        measured.0 - line,
        measured.1 / closed.1
    );
    // The strap's share at the largest d: the measured increase over d -> 0 and the closed form.
    let last = outs.last().expect("a run");
    println!(
        "  at d {:.2} mm: strap increase over the extrapolated L(0) {:.4} nH, closed form {:.4} nH",
        last.d_mm,
        last.l_nh - measured.0,
        last.strap_cf_nh
    );
    outs
}

#[test]
#[ignore = "study: about 5 minutes; `cargo test -p copper-fullwave --test strap_bias -- --ignored study_strap_bias --nocapture`"]
fn study_strap_bias_measured_against_the_closed_form() {
    let ds = [1_500_000, 2_500_000, 3_500_000];
    series("in-plane strap, wide line", Case { h: 500_000, w: MM, l: 12 * MM, air: false, cell: 250_000 }, &ds);
    series("in-plane strap, narrow line", Case { h: 300_000, w: 600_000, l: 12 * MM, air: false, cell: 250_000 }, &ds);
    series(
        "strap in the air on posts over a foreign pad",
        Case { h: 500_000, w: MM, l: 12 * MM, air: true, cell: 250_000 },
        &[2_500_000, 3_500_000, 4_500_000],
    );
}

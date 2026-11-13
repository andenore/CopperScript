//! Analytic validation of the extraction: every case compares the extracted port values with a closed form written
//! here or taken from `copper_emi::inductance`, over a mesh-refinement series. The cheap cases run by default; the
//! long series are `#[ignore]`d and their measured numbers are recorded in `docs/emi-noise-analysis.md`
//! (section 13).
//!
//! Run the studies with
//! `cargo test -p copper-extract --test analytic -- --ignored --nocapture`.

use std::f64::consts::PI;

use copper_cam::poly;
use copper_emi::inductance::{Segment, loop_inductance, loop_inductance_over_plane, mutual_bars, self_inductance_bar};
use copper_emi::spectrum::Complex;
use copper_extract::extract::{Extraction, extract};
use copper_extract::layout::{COPPER_RESISTIVITY, Conductor, Hint, Layout, ViaGeom};
use copper_extract::mesh::MeshSpec;
use copper_extract::partial::Axis;

const MM: i64 = 1_000_000;
const T: f64 = 35e-6;
const NM: f64 = 1e-9;

fn rel(a: f64, b: f64) -> f64 {
    (a - b) / b
}

fn nh(l: f64) -> f64 {
    l * 1e9
}

fn spec(cell_um: f64, coarse_um: f64) -> MeshSpec {
    MeshSpec {
        cell_nm: (cell_um * 1000.0) as i64,
        coarse_nm: (coarse_um * 1000.0) as i64,
        coarsen: false,
        max_filaments: 5000,
        ..MeshSpec::default()
    }
}

fn rect_terminal(layout: &mut Layout, name: &str, layer: usize, net: &str, rects: &[[i64; 4]]) -> usize {
    let t = layout.add_terminal_rect(name, layer, net, rects[0]);
    for r in &rects[1..] {
        let extra = poly::polygon(&[
            (r[0] as f64, r[1] as f64),
            (r[2] as f64, r[1] as f64),
            (r[2] as f64, r[3] as f64),
            (r[0] as f64, r[3] as f64),
        ]);
        layout.terminals[t].region.extend(extra);
    }
    t
}

// ---------------------------------------------------------------------------------------------------------------------
// Straight bar
// ---------------------------------------------------------------------------------------------------------------------

/// The straight bar `[0, l] x [0, w]` on one layer with a port between its end faces.
fn bar_layout(l: i64, w: i64) -> Layout {
    let mut layout = Layout::new([0, 0, l, w]);
    let layer = layout.add_layer("F.Cu", 0.0, T);
    layout.add_rect(layer, "N", [0, 0, l, w]);
    let a = layout.add_terminal_rect("a", layer, "N", [-1000, 0, 1000, w]);
    let b = layout.add_terminal_rect("b", layer, "N", [l - 1000, 0, l + 1000, w]);
    layout.add_port("p", a, b);
    layout
}

#[test]
fn straight_bar_matches_the_partial_self_inductance_and_dc_resistance() {
    let (l, w) = (10 * MM, MM);
    let l_ref = self_inductance_bar(l as f64 * NM, w as f64 * NM, T);
    let r_ref = COPPER_RESISTIVITY * l as f64 * NM / (w as f64 * NM * T);
    println!("bar 10 x 1 x 0.035 mm: closed form L {:.4} nH, R {:.4} mohm", nh(l_ref), r_ref * 1e3);
    let mut last = f64::NAN;
    for (cell, coarse) in [(250.0, 2000.0), (125.0, 1000.0), (62.5, 500.0)] {
        // A uniform mesh over the bar (a hint at the pitch): the exactness of R below is a property of a mesh that
        // tiles the bar uniformly, which the adaptive partition does not do next to the terminals of a bar that is
        // wide against the pitch.
        let mut layout = bar_layout(l, w);
        layout.layers[0].hints.push(Hint { rect: [0, 0, l, w], cell_x_nm: 0, cell_y_nm: (cell * 1000.0) as i64 });
        let x = extract(&layout, &spec(cell, coarse), &[0.0, 1e4], false).expect("extract");
        let (lx, rx) = (x.inductance(1, 0, 0), x.resistance(0, 0, 0));
        println!(
            "  cell {cell} um: {} filaments, L(10 kHz) {:.4} nH ({:+.3} %), R(DC) {:.4} mohm ({:+.4} %), {:.2} s assembly",
            x.mesh.stats.filaments,
            nh(lx),
            100.0 * rel(lx, l_ref),
            rx * 1e3,
            100.0 * rel(rx, r_ref),
            x.assemble_seconds
        );
        last = rel(lx, l_ref);
        assert!(rel(rx, r_ref).abs() < 1e-6, "DC resistance {rx} vs {r_ref}");
    }
    assert!(last.abs() < 0.02, "inductance of the finest mesh is {:+.2} % off the closed form", 100.0 * last);
}

#[test]
fn the_current_density_of_a_bar_crowds_to_its_edges_as_frequency_rises() {
    // With rows across the width the resistance rises and the inductance falls with frequency; a thick bar with
    // filaments across the thickness shows the skin effect in R as well.
    let layout = bar_layout(10 * MM, MM);
    let mut s = spec(125.0, 1000.0);
    let thin = extract(&layout, &s, &[0.0, 1e6, 1e8, 1e9], false).expect("extract");
    let (r0, r1, r2, r3) =
        (thin.resistance(0, 0, 0), thin.resistance(1, 0, 0), thin.resistance(2, 0, 0), thin.resistance(3, 0, 0));
    assert!(r0 < r1 && r1 < r2 && r2 < r3, "{r0} {r1} {r2} {r3}");
    assert!(thin.inductance(3, 0, 0) < thin.inductance(1, 0, 0));
    s.thickness_filaments = 4;
    let thick = extract(&layout, &s, &[0.0, 1e9], false).expect("extract");
    println!(
        "bar R(1 GHz)/R(DC): 1 filament across the thickness {:.2}, 4 filaments {:.2}",
        r3 / r0,
        thick.resistance(1, 0, 0) / thick.resistance(0, 0, 0)
    );
    assert!(
        thick.resistance(1, 0, 0) / thick.resistance(0, 0, 0) > 1.2 * r3 / r0,
        "thickness filaments add the skin effect"
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Two parallel bars
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn two_parallel_bars_share_current_by_conductance_then_by_inductance() {
    // Bars of 1.0 and 0.4 mm width, 10 mm long, 1.5 mm between centres, joined at both ends by the terminals.
    let (l, w1, w2, pitch) = (10 * MM, 1_000_000_i64, 400_000_i64, 1_500_000_i64);
    let y1 = (0, w1);
    let y2 = (pitch - w2 / 2 + w1 / 2, pitch + w2 / 2 + w1 / 2);
    let mut layout = Layout::new([0, -MM, l, 3 * MM]);
    let layer = layout.add_layer("F.Cu", 0.0, T);
    layout.add_rect(layer, "N", [0, y1.0, l, y1.1]);
    layout.add_rect(layer, "N", [0, y2.0, l, y2.1]);
    let a = rect_terminal(&mut layout, "a", layer, "N", &[[-1000, y1.0, 1000, y1.1], [-1000, y2.0, 1000, y2.1]]);
    let b = rect_terminal(
        &mut layout,
        "b",
        layer,
        "N",
        &[[l - 1000, y1.0, l + 1000, y1.1], [l - 1000, y2.0, l + 1000, y2.1]],
    );
    layout.add_port("p", a, b);
    // The reference: two bars, closed-form self and mutual inductances (the filament-averaged mutual of
    // `copper_emi::inductance::mutual_bars` is an independent code path from the Hoer-Love mutual the extraction
    // uses), resistances rho l / (w t), solved as a 2 x 2 system.
    let seg = |y0: i64, y1: i64| Segment {
        start: [0.0, (y0 + y1) as f64 / 2.0 * NM, 0.0],
        end: [l as f64 * NM, (y0 + y1) as f64 / 2.0 * NM, 0.0],
        width: (y1 - y0) as f64 * NM,
        thickness: T,
    };
    let (s1, s2) = (seg(y1.0, y1.1), seg(y2.0, y2.1));
    let (la, lb) = (self_inductance_bar(s1.length(), s1.width, T), self_inductance_bar(s2.length(), s2.width, T));
    let m = mutual_bars(&s1, &s2, 6, 1);
    let r = |s: &Segment| COPPER_RESISTIVITY * s.length() / (s.width * T);
    let (ra, rb) = (r(&s1), r(&s2));
    let dc_share = (1.0 / ra) / (1.0 / ra + 1.0 / rb);
    println!(
        "two bars (1.0 and 0.4 mm): DC share of the wide bar {dc_share:.4}; L {:.3} and {:.3} nH, M {:.3} nH",
        nh(la),
        nh(lb),
        nh(m)
    );
    let mid = l as f64 * 0.5 * NM;
    let freqs = [0.0, 1e5, 1e6, 1e7, 1e8, 1e9];
    let mut worst = 0.0_f64;
    for (cell, coarse) in [(250.0, 2000.0), (125.0, 1000.0), (62.5, 500.0)] {
        let x = extract(&layout, &spec(cell, coarse), &freqs, true).expect("extract");
        let columns: Vec<f64> =
            x.mesh.network.branches.iter().filter(|b| b.bar.axis == Axis::X).map(|b| b.bar.centre[0]).collect();
        let nearest = columns.iter().copied().min_by(|a, b| (a - mid).abs().total_cmp(&(b - mid).abs())).unwrap();
        let cut = |f: usize, lo: i64, hi: i64| {
            x.cut_current(f, 0, 0, Axis::X, |c| {
                (c[0] - nearest).abs() < 1e-9 && c[1] > lo as f64 * NM && c[1] < hi as f64 * NM
            })
        };
        let mut line = format!("  cell {cell} um ({} filaments):", x.mesh.stats.filaments);
        for (fi, &f) in freqs.iter().enumerate() {
            let (c1, c2) = (cut(fi, y1.0, y1.1), cut(fi, y2.0, y2.1));
            let share = c1.abs() / (c1.abs() + c2.abs());
            let w = 2.0 * PI * f;
            let (z11, z22, z12) = (Complex::new(ra, w * la), Complex::new(rb, w * lb), Complex::new(0.0, w * m));
            let det = z11 * z22 - z12 * z12;
            let (i1, i2) = ((z22 - z12) * det.recip(), (z11 - z12) * det.recip());
            let hand = i1.abs() / (i1.abs() + i2.abs());
            line.push_str(&format!(" f={f:.0e}: {share:.4} vs {hand:.4} ({:+.1} %);", 100.0 * rel(share, hand)));
            if fi == 0 {
                assert!(rel(share, dc_share).abs() < 1e-3, "DC share {share} vs {dc_share}");
            }
            assert!(rel(share, hand).abs() < 0.015, "share {share} vs the 2 x 2 solution {hand} at {f} Hz");
            worst = worst.max(rel(share, hand).abs());
        }
        println!("{line}");
    }
    println!("  largest difference to the 2 x 2 solution over the series and frequencies: {:.2} %", 100.0 * worst);
}

// ---------------------------------------------------------------------------------------------------------------------
// Rectangular loop, free space and over a plane
// ---------------------------------------------------------------------------------------------------------------------

/// A rectangular loop of centreline `a x b` and trace width `w` with a gap `gap` in the middle of the bottom side, the
/// port across the gap; optionally a floating plane `plane x plane` centred on the loop at depth `h` with fine lines
/// of pitch `plane_cell` across each side of the loop. Returns the layout and the closed-form segments of the path.
fn loop_layout(a: i64, b: i64, w: i64, gap: i64, plane: Option<(i64, i64)>, plane_cell: i64) -> (Layout, Vec<Segment>) {
    let half = w / 2;
    let (xg0, xg1) = (a / 2 - gap / 2, a / 2 + gap / 2);
    let margin = MM;
    let mut layout = Layout::new([-margin, -margin, a + margin, b + margin]);
    if let Some((p, _)) = plane {
        let (cx, cy) = (a / 2, b / 2);
        layout.window = [cx - p / 2, cy - p / 2, cx + p / 2, cy + p / 2];
    }
    let top = layout.add_layer("F.Cu", 0.0, T);
    for r in [
        [-half, -half, xg0, half],
        [xg1, -half, a + half, half],
        [-half, -half, half, b + half],
        [a - half, -half, a + half, b + half],
        [-half, b - half, a + half, b + half],
    ] {
        layout.add_rect(top, "L", r);
    }
    let pa = layout.add_terminal_rect("a", top, "L", [xg0 - 1000, -half, xg0 + 1000, half]);
    let pb = layout.add_terminal_rect("b", top, "L", [xg1 - 1000, -half, xg1 + 1000, half]);
    layout.add_port("p", pa, pb);
    if let Some((p, h)) = plane {
        let pl = layout.add_layer("In1.Cu", -(h as f64) * NM, T);
        let (cx, cy) = (a / 2, b / 2);
        layout.add_rect(pl, "GND", [cx - p / 2, cy - p / 2, cx + p / 2, cy + p / 2]);
        // Fine lines across each side of the loop, four heights either side of it; the long direction stays coarse.
        let reach = 4 * h;
        let fine = plane_cell;
        for (rect, cx, cy) in [
            ([-reach, -reach, a + reach, reach], 0, fine),
            ([-reach, b - reach, a + reach, b + reach], 0, fine),
            ([-reach, -reach, reach, b + reach], fine, 0),
            ([a - reach, -reach, a + reach, b + reach], fine, 0),
        ] {
            layout.layers[pl].hints.push(Hint { rect, cell_x_nm: cx, cell_y_nm: cy });
        }
    }
    let seg = |p0: (i64, i64), p1: (i64, i64)| Segment {
        start: [p0.0 as f64 * NM, p0.1 as f64 * NM, 0.0],
        end: [p1.0 as f64 * NM, p1.1 as f64 * NM, 0.0],
        width: w as f64 * NM,
        thickness: T,
    };
    let path = vec![
        seg((xg0, 0), (0, 0)),
        seg((0, 0), (0, b)),
        seg((0, b), (a, b)),
        seg((a, b), (a, 0)),
        seg((a, 0), (xg1, 0)),
    ];
    (layout, path)
}

#[test]
fn rectangular_loop_in_free_space_matches_the_partial_inductance_closed_form() {
    let (a, b, gap) = (10 * MM, 6 * MM, MM);
    for w in [500_000_i64, 250_000] {
        let (layout, path) = loop_layout(a, b, w, gap, None, 0);
        let want = loop_inductance(&path);
        println!(
            "free-space loop 10 x 6 mm, trace {:.2} mm, gap 1 mm: closed form {:.4} nH",
            w as f64 * 1e-6,
            nh(want)
        );
        let mut last = f64::NAN;
        for (cell, coarse) in [(250.0, 1500.0), (125.0, 750.0)] {
            let x = extract(&layout, &spec(cell, coarse), &[1e4, 1e6, 1e8], false).expect("extract");
            let l = x.inductance(0, 0, 0);
            println!(
                "  cell {cell} um: {} filaments; L(10 kHz) {:.4} ({:+.2} % of the closed form), L(1 MHz) {:.4}, L(100 MHz) {:.4} nH; R(10 kHz) {:.2} mohm",
                x.mesh.stats.filaments,
                nh(l),
                100.0 * rel(l, want),
                nh(x.inductance(1, 0, 0)),
                nh(x.inductance(2, 0, 0)),
                x.resistance(0, 0, 0) * 1e3
            );
            last = rel(l, want);
        }
        // The closed form runs its segments along the centreline and double counts the corner squares; the meshed
        // current cuts the corners. The difference scales with the trace width.
        assert!(last.abs() < 0.05, "{:+.2} % off the closed form", 100.0 * last);
    }
}

/// The image-plane reference of the loop over a plane at depth `h` (metres).
fn image_reference(path: &[Segment], h: f64) -> f64 {
    loop_inductance_over_plane(path, -h)
}

#[test]
fn rectangular_loop_over_a_plane_agrees_with_the_image_result_where_the_current_is_uniform_in_the_trace() {
    let (a, b, w, gap, h) = (10 * MM, 6 * MM, 500_000, MM, 200_000);
    let (_, path) = loop_layout(a, b, w, gap, None, 0);
    let want = image_reference(&path, h as f64 * NM);
    println!(
        "loop over a plane 0.2 mm below: image result {:.4} nH (free space {:.4} nH)",
        nh(want),
        nh(loop_inductance(&path))
    );
    // Same mesh pitch, growing plane (the only change is its extent).
    for plane in [16 * MM, 32 * MM] {
        let (layout, _) = loop_layout(a, b, w, gap, Some((plane, h)), 400_000);
        let x = extract(&layout, &spec(400.0, 3000.0), &[1e6, 3e6, 1e7], false).expect("extract");
        let l = x.inductance(0, 0, 0);
        println!(
            "  plane {} mm: {} filaments; L(1 MHz) {:.4} nH ({:+.2} % of the image result), L(3 MHz) {:.4}, L(10 MHz) {:.4} ({:+.2} %) nH; {:.1}+{:.1} s",
            plane / MM,
            x.mesh.stats.filaments,
            nh(l),
            100.0 * rel(l, want),
            nh(x.inductance(1, 0, 0)),
            nh(x.inductance(2, 0, 0)),
            100.0 * rel(x.inductance(2, 0, 0), want),
            x.assemble_seconds,
            x.solve_seconds
        );
        assert!(rel(l, want).abs() < 0.05, "L(1 MHz) {} nH vs the image result {} nH", nh(l), nh(want));
    }
}

#[test]
#[ignore = "about a minute: mesh series and plane growth of the loop over a plane"]
fn study_loop_over_a_plane_mesh_series_and_plane_growth() {
    let (a, b, w, gap, h) = (10 * MM, 6 * MM, 500_000, MM, 200_000);
    let (_, path) = loop_layout(a, b, w, gap, None, 0);
    let want = image_reference(&path, h as f64 * NM);
    println!("image result {:.4} nH", nh(want));
    println!("mesh series on a 24 mm plane:");
    for fine in [800_000, 500_000, 350_000, 250_000] {
        let (layout, _) = loop_layout(a, b, w, gap, Some((24 * MM, h)), fine);
        let x = extract(&layout, &spec(fine as f64 / 1000.0, 3000.0), &[1e6, 1e7, 1e8], false).expect("extract");
        println!(
            "  fine pitch {} um: {} filaments, L(1 MHz) {:.4} ({:+.2} %), L(10 MHz) {:.4} ({:+.2} %), L(100 MHz) {:.4} nH; {:.1} s",
            fine / 1000,
            x.mesh.stats.filaments,
            nh(x.inductance(0, 0, 0)),
            100.0 * rel(x.inductance(0, 0, 0), want),
            nh(x.inductance(1, 0, 0)),
            100.0 * rel(x.inductance(1, 0, 0), want),
            nh(x.inductance(2, 0, 0)),
            x.assemble_seconds
        );
    }
    println!("plane growth at a 0.5 mm pitch:");
    for plane in [16 * MM, 24 * MM, 32 * MM, 48 * MM, 64 * MM] {
        let (layout, _) = loop_layout(a, b, w, gap, Some((plane, h)), 500_000);
        let x = extract(&layout, &MeshSpec { max_filaments: 6000, ..spec(500.0, 3000.0) }, &[1e6, 1e7], false)
            .expect("extract");
        println!(
            "  plane {} mm: {} filaments, L(1 MHz) {:.4} ({:+.2} %), L(10 MHz) {:.4} nH ({:+.2} %)",
            plane / MM,
            x.mesh.stats.filaments,
            nh(x.inductance(0, 0, 0)),
            100.0 * rel(x.inductance(0, 0, 0), want),
            nh(x.inductance(1, 0, 0)),
            100.0 * rel(x.inductance(1, 0, 0), want)
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Shorted microstrip
// ---------------------------------------------------------------------------------------------------------------------

/// Hammerstad-Jensen microstrip impedance and effective permittivity, zero thickness (the formulas of
/// `crates/copper-fullwave/tests/shorted_line.rs`).
fn hammerstad(w_over_h: f64, er: f64) -> (f64, f64) {
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / w_over_h).sqrt());
    let z = if w_over_h <= 1.0 {
        60.0 / er_eff.sqrt() * (8.0 / w_over_h + w_over_h / 4.0).ln()
    } else {
        120.0 * PI / (er_eff.sqrt() * (w_over_h + 1.393 + 0.667 * (w_over_h + 1.444).ln()))
    };
    (z, er_eff)
}

/// Low-frequency inductance of a shorted microstrip of length `l` (m): `Z0 sqrt(eps_eff) l / c`, the limit of
/// `Z0 tan(beta l) / omega`. The product `Z0 sqrt(eps_eff)` is the air-filled impedance, so the result does not
/// depend on the permittivity (4.3 is the value of the full-wave test).
fn shorted_line_reference(w: f64, h: f64, l: f64) -> f64 {
    let (z0, ee) = hammerstad(w / h, 4.3);
    z0 * ee.sqrt() * l / 299_792_458.0
}

/// The same with the strip thickness `t` accounted for by the classical effective width
/// `w + t/pi (1 + ln(2 h / t))` (Wheeler), for the comparison at the real 35 um copper.
fn shorted_line_reference_thick(w: f64, h: f64, t: f64, l: f64) -> f64 {
    shorted_line_reference(w + t / PI * (1.0 + (2.0 * h / t).ln()), h, l)
}

/// A strip `[0, l] x [-w/2, w/2]` on the top layer and a plane `plane_w` wide under it (depth `h`), the port between
/// the strip's left end and the plane below it, the far end shorted to the plane by an ideal wall (`wall = None`) or
/// by `n` barrel vias.
fn microstrip_layout(l: i64, w: i64, h: i64, plane_w: i64, wall: Option<usize>, fine: i64, t: f64) -> Layout {
    let ext = 3 * MM;
    let mut layout = Layout::new([-ext, -plane_w / 2, l + ext, plane_w / 2]);
    let top = layout.add_layer("F.Cu", 0.0, t);
    let plane = layout.add_layer("B.Cu", -(h as f64) * NM, t);
    layout.add_rect(top, "S", [0, -w / 2, l, w / 2]);
    layout.add_rect(plane, "GND", [-ext, -plane_w / 2, l + ext, plane_w / 2]);
    // The plane is fine across the strip and four heights beside it; coarse along it.
    layout.layers[plane].hints.push(Hint {
        rect: [-ext, -w / 2 - 4 * h, l + ext, w / 2 + 4 * h],
        cell_x_nm: 0,
        cell_y_nm: fine,
    });
    let a = layout.add_terminal_rect("strip", top, "S", [-1000, -w / 2, 1000, w / 2]);
    let b = layout.add_terminal_rect("plane", plane, "GND", [-1000, -w / 2, 1000, w / 2]);
    layout.add_port("p", a, b);
    match wall {
        None => {
            let c = layout.add_terminal_rect("strip end", top, "S", [l - 1000, -w / 2, l + 1000, w / 2]);
            let d = layout.add_terminal_rect("plane end", plane, "GND", [l - 1000, -w / 2, l + 1000, w / 2]);
            layout.shorts.push(vec![c, d]);
        }
        Some(n) => {
            // Vias of net S that end on the plane layer's copper of net GND: a deliberate short at the strip's end.
            for k in 0..n {
                let y = -w / 2 + (2 * k as i64 + 1) * w / (2 * n as i64);
                layout.vias.push(ViaGeom {
                    name: format!("wall{k}"),
                    x: l - 300_000,
                    y,
                    net: "S".to_owned(),
                    first_layer: top,
                    last_layer: plane,
                    drill_nm: 100_000,
                    plating_nm: 25_000,
                    layer_nets: vec![(plane, "GND".to_owned())],
                });
            }
        }
    }
    layout
}

#[test]
fn shorted_microstrip_matches_the_hammerstad_jensen_inductance() {
    // A 10 mm line (the full-wave suite uses 20 mm lines and the same widths and heights).
    let (l, w, h) = (10 * MM, MM, 500_000);
    let want0 = shorted_line_reference(w as f64 * NM, h as f64 * NM, l as f64 * NM);
    let want_t = shorted_line_reference_thick(w as f64 * NM, h as f64 * NM, T, l as f64 * NM);
    println!(
        "shorted microstrip w 1.0 h 0.5 l 10 mm: Z0 sqrt(eps_eff) l / c = {:.4} nH (zero thickness), {:.4} nH (35 um strip, Wheeler)",
        nh(want0),
        nh(want_t)
    );
    let mut hf = f64::NAN;
    for fine in [500_000_i64, 250_000] {
        let layout = microstrip_layout(l, w, h, 14 * MM, None, fine, T);
        let x = extract(&layout, &spec(fine as f64 / 1000.0, 2000.0), &[1e6, 1e7, 1e8], false).expect("extract");
        println!(
            "  fine pitch {} um: {} filaments, L(1 MHz) {:.4} ({:+.2} %), L(10 MHz) {:.4} ({:+.2} % of zero thickness, {:+.2} % of Wheeler), L(100 MHz) {:.4} nH",
            fine / 1000,
            x.mesh.stats.filaments,
            nh(x.inductance(0, 0, 0)),
            100.0 * rel(x.inductance(0, 0, 0), want0),
            nh(x.inductance(1, 0, 0)),
            100.0 * rel(x.inductance(1, 0, 0), want0),
            100.0 * rel(x.inductance(1, 0, 0), want_t),
            nh(x.inductance(2, 0, 0))
        );
        hf = rel(x.inductance(1, 0, 0), want0);
    }
    assert!(hf.abs() < 0.05, "L(10 MHz) is {:+.2} % off the zero-thickness line", 100.0 * hf);
}

#[test]
#[ignore = "about two minutes: four geometries of the full-wave suite at 20 mm, 35, 5 and 1 um copper, and a barrel wall"]
fn study_shorted_microstrip() {
    for (h, w) in [(500_000_i64, 1_000_000_i64), (200_000, 500_000), (500_000, 500_000), (300_000, 1_500_000)] {
        let l = 20 * MM;
        let want0 = shorted_line_reference(w as f64 * NM, h as f64 * NM, l as f64 * NM);
        for t in [35e-6, 5e-6, 1e-6] {
            let want_t = shorted_line_reference_thick(w as f64 * NM, h as f64 * NM, t, l as f64 * NM);
            let layout = microstrip_layout(l, w, h, 16 * MM, None, 250_000, t);
            let x = extract(
                &layout,
                &MeshSpec { max_filaments: 5000, ..spec(250.0, 2000.0) },
                &[1e6, 1e7, 1e8, 1e9],
                false,
            );
            match x {
                Ok(x) => println!(
                    "h {:.1} w {:.1} t {:.0} um: {} filaments; zero-thickness {:.4} nH, Wheeler {:.4}; L(1M) {:.4} ({:+.1} %), L(10M) {:.4} ({:+.1} % / {:+.1} %), L(100M) {:.4} ({:+.1} % / {:+.1} %), L(1G) {:.4} nH",
                    h as f64 / 1e6,
                    w as f64 / 1e6,
                    t * 1e6,
                    x.mesh.stats.filaments,
                    nh(want0),
                    nh(want_t),
                    nh(x.inductance(0, 0, 0)),
                    100.0 * rel(x.inductance(0, 0, 0), want0),
                    nh(x.inductance(1, 0, 0)),
                    100.0 * rel(x.inductance(1, 0, 0), want0),
                    100.0 * rel(x.inductance(1, 0, 0), want_t),
                    nh(x.inductance(2, 0, 0)),
                    100.0 * rel(x.inductance(2, 0, 0), want0),
                    100.0 * rel(x.inductance(2, 0, 0), want_t),
                    nh(x.inductance(3, 0, 0))
                ),
                Err(e) => println!("h {h} w {w} t {t}: {e}"),
            }
        }
    }
    // The wall of vias instead of the ideal short (the full-wave model's wall is a row of vertical wires too).
    let (l, w, h) = (10 * MM, MM, 500_000);
    let ideal = extract(&microstrip_layout(l, w, h, 14 * MM, None, 250_000, T), &spec(250.0, 2000.0), &[1e7], false)
        .expect("extract");
    for n in [2, 4] {
        let x = extract(&microstrip_layout(l, w, h, 14 * MM, Some(n), 250_000, T), &spec(250.0, 2000.0), &[1e7], false)
            .expect("extract");
        println!(
            "wall of {n} vias: L(10 MHz) {:.4} nH against {:.4} nH for the ideal short ({} vias unattached)",
            nh(x.inductance(0, 0, 0)),
            nh(ideal.inductance(0, 0, 0)),
            x.mesh.stats.vias_unattached.len()
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Slot in the plane under a trace
// ---------------------------------------------------------------------------------------------------------------------

/// A strip over a plane with a slot of length `slot` (along the strip) and width 2 mm centred under the strip's middle.
fn slot_layout(l: i64, w: i64, h: i64, plane_w: i64, slot: i64, fine: i64) -> Layout {
    let mut layout = microstrip_layout(l, w, h, plane_w, None, fine, T);
    if slot > 0 {
        let plane = 1;
        let full = layout.layers[plane].conductors.remove(0).shapes;
        let cutter = poly::polygon(&[
            ((l / 2 - slot / 2) as f64, -1_000_000.0),
            ((l / 2 + slot / 2) as f64, -1_000_000.0),
            ((l / 2 + slot / 2) as f64, 1_000_000.0),
            ((l / 2 - slot / 2) as f64, 1_000_000.0),
        ]);
        let shapes = poly::difference(&full, &cutter);
        layout.layers[plane].conductors.push(Conductor { net: "GND".to_owned(), shapes, on_loop: false });
        layout.layers[plane].hints.push(Hint {
            rect: [l / 2 - slot / 2 - 2 * MM, -3 * MM, l / 2 + slot / 2 + 2 * MM, 3 * MM],
            cell_x_nm: fine,
            cell_y_nm: fine,
        });
    }
    layout
}

#[test]
fn a_slot_under_the_trace_raises_the_inductance_between_the_unslotted_and_the_fully_slotted_limits() {
    let (l, w, h) = (10 * MM, 500_000, 200_000);
    let mut values = Vec::new();
    for slot in [0, 2 * MM, 4 * MM, 6 * MM, 8 * MM, 10 * MM] {
        let layout = slot_layout(l, w, h, 12 * MM, slot, 500_000);
        let x =
            extract(&layout, &MeshSpec { max_filaments: 6000, ..spec(500.0, 1500.0) }, &[1e7], false).expect("extract");
        values.push((slot, x.inductance(0, 0, 0), x.mesh.stats.filaments));
    }
    let reference = shorted_line_reference_thick(w as f64 * NM, h as f64 * NM, T, l as f64 * NM);
    println!(
        "trace 10 mm x 0.5 mm over a plane 0.2 mm below, plane 12 mm wide, slot 2 mm wide; unslotted reference (Hammerstad, 35 um) {:.4} nH",
        nh(reference)
    );
    for (slot, v, n) in &values {
        println!(
            "  slot {:>2} mm: {:.4} nH ({:.2}x the unslotted value), {n} filaments",
            slot / MM,
            nh(*v),
            v / values[0].1
        );
    }
    for pair in values.windows(2) {
        assert!(pair[1].1 > pair[0].1, "inductance must rise with the slot length: {values:?}");
    }
    let (first, last) = (values[0].1, values.last().unwrap().1);
    assert!(last > 1.3 * first, "a slot over the whole trace is at least 30 % above the unslotted line");
    assert!(rel(first, reference).abs() < 0.10, "unslotted {} vs {}", nh(first), nh(reference));
}

// ---------------------------------------------------------------------------------------------------------------------
// Parallel vias
// ---------------------------------------------------------------------------------------------------------------------

/// Two 14 x 14 mm pours of one net on two layers 0.2 mm apart, joined by vias at `via_xy`; the port between a pad at
/// the centre of the top pour and a pad at the centre of the bottom pour.
fn via_array_layout(via_xy: &[(i64, i64)]) -> Layout {
    let half = 6 * MM;
    let mut layout = Layout::new([-half, -half, half, half]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let bottom = layout.add_layer("In1.Cu", -200e-6, T);
    layout.add_rect(top, "A", [-half, -half, half, half]);
    layout.add_rect(bottom, "A", [-half, -half, half, half]);
    let a = layout.add_terminal_rect("top pad", top, "A", [-MM, -MM, MM, MM]);
    let b = layout.add_terminal_rect("bottom pad", bottom, "A", [-MM, -MM, MM, MM]);
    layout.add_port("p", a, b);
    for (k, &(x, y)) in via_xy.iter().enumerate() {
        layout.vias.push(ViaGeom {
            name: format!("v{k}"),
            x,
            y,
            net: "A".to_owned(),
            first_layer: top,
            last_layer: bottom,
            drill_nm: 300_000,
            plating_nm: 25_000,
            layer_nets: Vec::new(),
        });
    }
    layout
}

fn via_spec() -> MeshSpec {
    // One uniform pitch: the mesh is mirror symmetric about the centre pad, so symmetric vias see identical meshes.
    spec(1000.0, 1000.0)
}

#[test]
fn parallel_vias_share_current_equally_when_symmetric_and_less_when_farther() {
    let r = 3 * MM;
    let symmetric = [(r, 0), (0, r), (-r, 0), (0, -r)];
    let x = extract(&via_array_layout(&symmetric), &via_spec(), &[0.0, 1e6, 1e8], true).expect("extract");
    assert!(x.mesh.stats.vias_unattached.is_empty());
    let shares = |x: &Extraction, f: usize, n: usize| x.via_shares(f, 0, &(0..n).collect::<Vec<_>>());
    println!("4 symmetric vias at 3 mm: {} filaments", x.mesh.stats.filaments);
    for f in 0..3 {
        let s = shares(&x, f, 4);
        println!(
            "  f={:.0e}: shares {:?}",
            x.frequencies[f],
            s.iter().map(|v| format!("{:.5}", v.share)).collect::<Vec<_>>()
        );
        for v in &s {
            assert!((v.share - 0.25).abs() < 1e-6, "share {} at {} Hz", v.share, x.frequencies[f]);
        }
    }
    // One via moved from 3 mm to 5 mm from the pads: it carries less, at every frequency, and the shares still add to 1.
    let moved = [(r, 0), (0, r), (-r, 0), (0, -5 * MM)];
    let x = extract(&via_array_layout(&moved), &via_spec(), &[0.0, 1e6, 1e8], true).expect("extract");
    for f in 0..3 {
        let s = shares(&x, f, 4);
        println!(
            "  one via at 5 mm, f={:.0e}: shares {:?}",
            x.frequencies[f],
            s.iter().map(|v| format!("{:.4}", v.share)).collect::<Vec<_>>()
        );
        assert!(s[3].share < s[0].share && s[3].share < 0.25, "{s:?}");
        assert!((s.iter().map(|v| v.share).sum::<f64>() - 1.0).abs() < 1e-9);
    }
    // Resistance: DC share follows conductance (the path through the farther via is longer); the current is carried
    // to the pads by the pours, so a farther via is also a larger loop at high frequency.
    let dc = shares(&x, 0, 4);
    let hf = shares(&x, 2, 4);
    assert!(dc[3].share < 0.25 && hf[3].share < 0.25);
}

// ---------------------------------------------------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_extraction_is_bit_for_bit_repeatable() {
    let (layout, _) = loop_layout(10 * MM, 6 * MM, 500_000, MM, None, 0);
    let run = || extract(&layout, &spec(250.0, 1500.0), &[1e5, 1e7], false).expect("extract");
    let (a, b) = (run(), run());
    assert_eq!(a.mesh.stats.filaments, b.mesh.stats.filaments);
    for f in 0..2 {
        assert_eq!(a.solutions[f].z[0][0].re.to_bits(), b.solutions[f].z[0][0].re.to_bits());
        assert_eq!(a.solutions[f].z[0][0].im.to_bits(), b.solutions[f].z[0][0].im.to_bits());
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Several ports
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn a_two_port_matrix_is_reciprocal_and_its_mutual_term_is_the_closed_form_mutual() {
    // Two separate 10 mm bars 1.5 mm apart (centre to centre), each with its own port between its end faces: the
    // impedance matrix is Z11 = R1 + jw L1, Z22 = R2 + jw L2, Z12 = Z21 = jw M with the closed-form partial mutual.
    let (l, w, pitch) = (10 * MM, 500_000_i64, 1_500_000_i64);
    let mut layout = Layout::new([0, -MM, l, 3 * MM]);
    let layer = layout.add_layer("F.Cu", 0.0, T);
    layout.add_rect(layer, "A", [0, 0, l, w]);
    layout.add_rect(layer, "B", [0, pitch, l, pitch + w]);
    let a0 = layout.add_terminal_rect("a0", layer, "A", [-1000, 0, 1000, w]);
    let a1 = layout.add_terminal_rect("a1", layer, "A", [l - 1000, 0, l + 1000, w]);
    let b0 = layout.add_terminal_rect("b0", layer, "B", [-1000, pitch, 1000, pitch + w]);
    let b1 = layout.add_terminal_rect("b1", layer, "B", [l - 1000, pitch, l + 1000, pitch + w]);
    layout.add_port("a", a0, a1);
    layout.add_port("b", b0, b1);
    let seg = |y0: i64| Segment {
        start: [0.0, (y0 + w / 2) as f64 * NM, 0.0],
        end: [l as f64 * NM, (y0 + w / 2) as f64 * NM, 0.0],
        width: w as f64 * NM,
        thickness: T,
    };
    let m_ref = mutual_bars(&seg(0), &seg(pitch), 6, 1);
    let self_ref = self_inductance_bar(l as f64 * NM, w as f64 * NM, T);
    let x = extract(&layout, &spec(125.0, 1000.0), &[1e4, 1e6], false).expect("extract");
    for f in 0..2 {
        let z = &x.solutions[f].z;
        let w_rad = 2.0 * PI * x.frequencies[f];
        let (m12, m21) = (z[0][1].im / w_rad, z[1][0].im / w_rad);
        println!(
            "f={:.0e}: L11 {:.4} L22 {:.4} nH (bar {:.4}); M12 {:.4} M21 {:.4} nH against the closed form {:.4} nH; Re Z12 {:.2e}",
            x.frequencies[f],
            nh(x.inductance(f, 0, 0)),
            nh(x.inductance(f, 1, 1)),
            nh(self_ref),
            nh(m12),
            nh(m21),
            nh(m_ref),
            z[0][1].re
        );
        assert!((m12 - m21).abs() < 1e-6 * m12.abs(), "reciprocity {m12} vs {m21}");
    }
    let w_rad = 2.0 * PI * x.frequencies[0];
    let m12 = x.solutions[0].z[0][1].im / w_rad;
    assert!(rel(m12, m_ref).abs() < 0.02, "mutual {} nH vs the closed form {} nH", nh(m12), nh(m_ref));
    assert!(rel(x.inductance(0, 0, 0), self_ref).abs() < 0.02);
}

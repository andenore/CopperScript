//! Independent validation of the full-wave edge rule (`RegionSpec::edge_inset`, `edge_inset_z`: 0.23 of the lateral
//! cell and 0.11 of the vertical cell) on geometries it was not fitted on, against the quasi-static extraction of this
//! crate as the reference (valid at low frequency; copper of 5 um read at 100 MHz, the full-wave model's copper
//! being zero-thickness PEC) and, where one exists, a closed form. Nothing here refits the rule: the default rule runs.
//!
//! The rule was fitted on two microstrip lines (h 0.2 / w 0.5 and h 0.5 / w 1.0 mm) and checked on three more
//! (`crates/copper-fullwave/tests/shorted_line.rs`). The cases here are a right-angle bend, a rectangular loop over a
//! plane, a line over a plane with a slot under it, and a via transition between two layers, all on a line of h 0.3 mm
//! and w 0.8 mm. Every case is driven by a vertical or in-plane lumped port at one end and shorted by a wall of vertical
//! wires at the other, as the validation lines are; what the port and the wall add (0.05 to 0.2 nH) is in the control
//! line of the same run, and the corner, slot or via excess is the case minus its control in both tools.
//!
//! The solver runs take 10 to 130 s each in the debug build with optimised dependencies: the default tests run two,
//! the `study_*` tests are ignored and their numbers are in `docs/emi-noise-analysis.md`, section 11.

use std::f64::consts::PI;
use std::time::Instant;

use copper_cam::poly;
use copper_extract::extract::extract;
use copper_extract::layout::{Conductor, Hint, Layout, ViaGeom};
use copper_extract::mesh::MeshSpec;
use copper_fullwave::testing::*;
use copper_fullwave::{PortSpec, RegionModel, RegionSpec, Sheet, Terminal, Wire};
use copper_physical::CopperLayer;

const MM: i64 = 1_000_000;
const NM: f64 = 1e-9;
/// Copper thickness of the reference extraction: thin against the full-wave model's zero-thickness sheets (the
/// shorted-microstrip study of `analytic.rs` measures 5 um at 100 MHz within 1.5 % of the zero-thickness line).
const T_REF: f64 = 5e-6;
const F_REF: f64 = 100e6;

fn hammerstad(w_over_h: f64, er: f64) -> (f64, f64) {
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / (2.0 * (1.0 + 12.0 / w_over_h).sqrt());
    let z = if w_over_h <= 1.0 {
        60.0 / er_eff.sqrt() * (8.0 / w_over_h + w_over_h / 4.0).ln()
    } else {
        120.0 * PI / (er_eff.sqrt() * (w_over_h + 1.393 + 0.667 * (w_over_h + 1.444).ln()))
    };
    (z, er_eff)
}

/// Mean over 50-150 MHz of `Im Z / omega` of the port column, nH.
fn band_l(col: &copper_fullwave::PortColumn) -> f64 {
    let ls: Vec<f64> = col
        .freqs
        .iter()
        .zip(&col.z_in)
        .filter(|(f, _)| **f >= 50e6 && **f <= 150e6)
        .map(|(f, z)| z.im / (2.0 * PI * f))
        .collect();
    1e9 * ls.iter().sum::<f64>() / ls.len() as f64
}

fn extract_spec(fine_um: f64) -> MeshSpec {
    MeshSpec {
        cell_nm: (fine_um * 1000.0) as i64,
        coarse_nm: 2_000_000,
        coarsen: false,
        max_filaments: 7000,
        ..MeshSpec::default()
    }
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

/// A rectangle `[x0, y0, x1, y1]` (nm) as the track a board would draw it as: along its long side, with the square
/// caps that extend half a width past the end points.
fn rect_track(net: &str, r: [i64; 4], layer: CopperLayer) -> copper_pcb::board::TrackSegment {
    let (dx, dy) = (r[2] - r[0], r[3] - r[1]);
    if dx >= dy {
        let y = (r[1] + r[3]) / 2;
        track(net, (r[0] + dy / 2, y), (r[2] - dy / 2, y), dy, layer)
    } else {
        let x = (r[0] + r[2]) / 2;
        track(net, (x, r[1] + dx / 2), (x, r[3] - dx / 2), dx, layer)
    }
}

fn rect_points(r: [i64; 4]) -> Vec<copper_physical::Point> {
    use copper_physical::Point;
    vec![Point::new(r[0], r[1]), Point::new(r[2], r[1]), Point::new(r[2], r[3]), Point::new(r[0], r[3])]
}

fn bounds(rects: &[[i64; 4]]) -> [i64; 4] {
    [
        rects.iter().map(|r| r[0]).min().unwrap(),
        rects.iter().map(|r| r[1]).min().unwrap(),
        rects.iter().map(|r| r[2]).max().unwrap(),
        rects.iter().map(|r| r[3]).max().unwrap(),
    ]
}

fn grow(r: [i64; 4], by: i64) -> [i64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

// ---------------------------------------------------------------------------------------------------------------------
// Microstrip family: a strip made of rectangles over a plane (optionally slotted), vertical port at the start of the
// first rectangle, wall of wires across the far end of the last, which runs along x or along y.
// ---------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Ms {
    h: i64,
    rects: Vec<[i64; 4]>,
    plane: [i64; 4],
    slot: Option<[i64; 4]>,
    /// The last rectangle runs along y (the wall is a row of wires along x at its far end), else along x.
    end_along_y: bool,
}

fn ms_fdtd(m: &Ms, cell: i64) -> (f64, usize, f64) {
    let started = Instant::now();
    let mut board = bare_board();
    declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, m.h, "4.3", "0"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    let f = CopperLayer::Front;
    for r in &m.rects {
        board.tracks.push(rect_track("S", *r, f));
    }
    let mut plane = zone("GND", &[CopperLayer::Back], m.plane);
    if let Some(s) = m.slot {
        plane.holes.push(rect_points(s));
    }
    board.zones.push(plane);
    let all = bounds(&m.rects);
    let mut spec = RegionSpec::new(grow(all, 2 * MM));
    spec.margin_nm = 3 * MM;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.cell_nm = Some(cell);
    spec.smooth_mesh = true;
    spec.ring_fit = false;
    let first = m.rects[0];
    spec.ports = vec![PortSpec::to_plane(
        "p",
        Terminal::rect(f, [first[0], first[1], first[0] + cell, first[3]]),
        CopperLayer::Back,
    )];
    let mut model = RegionModel::build(&board, &spec).expect("model");
    let strips: Vec<Sheet> = model.sheets.iter().filter(|s| s.kind == "track").cloned().collect();
    let zt = strips[0].z;
    let zb = model.sheets.iter().find(|s| s.kind == "zone").map(|s| s.z).expect("plane");
    let last = bbox(&strips[strips.len() - 1]);
    let (xl, yl) = (model.axes[0].lines.clone(), model.axes[1].lines.clone());
    let mut wires = Vec::new();
    if m.end_along_y {
        let y = nearest(&yl, last.3);
        for x in xl.iter().copied().filter(|x| *x >= last.0 - 1e-6 && *x <= last.1 + 1e-6) {
            wires.push((x, y));
        }
    } else {
        let x = nearest(&xl, last.1);
        for y in yl.iter().copied().filter(|y| *y >= last.2 - 1e-6 && *y <= last.3 + 1e-6) {
            wires.push((x, y));
        }
    }
    for (x, y) in wires {
        model.wires.push(Wire { kind: "via", net: "S".into(), net_id: 1, x, y, z_top: zt, z_bottom: zb, radius: 0.0 });
    }
    let col = model.solve_port(0).expect("solve");
    (band_l(&col), model.mesh.cells, started.elapsed().as_secs_f64())
}

fn ms_layout(m: &Ms, fine_nm: i64) -> Layout {
    let h = m.h as f64;
    let mut layout = Layout::new(m.plane);
    let top = layout.add_layer("F.Cu", 0.0, T_REF);
    let plane = layout.add_layer("B.Cu", -h * NM, T_REF);
    for r in &m.rects {
        layout.add_rect(top, "S", *r);
    }
    match m.slot {
        None => layout.add_rect(plane, "GND", m.plane),
        Some(s) => {
            let full = poly::polygon(&[
                (m.plane[0] as f64, m.plane[1] as f64),
                (m.plane[2] as f64, m.plane[1] as f64),
                (m.plane[2] as f64, m.plane[3] as f64),
                (m.plane[0] as f64, m.plane[3] as f64),
            ]);
            let cutter = poly::polygon(&[
                (s[0] as f64, s[1] as f64),
                (s[2] as f64, s[1] as f64),
                (s[2] as f64, s[3] as f64),
                (s[0] as f64, s[3] as f64),
            ]);
            let shapes = poly::difference(&full, &cutter);
            layout.layers[plane].conductors.push(Conductor { net: "GND".to_owned(), shapes, on_loop: false });
        }
    }
    // The plane is fine across each rectangle and four heights beside it; the corner is fine both ways.
    for r in &m.rects {
        let (dx, dy) = (r[2] - r[0], r[3] - r[1]);
        let reach = 4 * m.h;
        let hint = if dx >= dy {
            Hint { rect: [r[0], r[1] - reach, r[2], r[3] + reach], cell_x_nm: 0, cell_y_nm: fine_nm }
        } else {
            Hint { rect: [r[0] - reach, r[1], r[2] + reach, r[3]], cell_x_nm: fine_nm, cell_y_nm: 0 }
        };
        layout.layers[plane].hints.push(hint);
    }
    if m.rects.len() > 1 {
        let b = m.rects[1];
        let reach = 4 * m.h;
        layout.layers[plane]
            .hints
            .push(Hint::square([b[0] - reach, b[1] - reach, b[2] + reach, b[1] + reach], fine_nm));
    }
    if let Some(s) = m.slot {
        layout.layers[plane].hints.push(Hint::square(grow(s, 2 * MM), fine_nm));
    }
    let first = m.rects[0];
    let a = layout.add_terminal_rect("strip", top, "S", [first[0] - 1000, first[1], first[0] + 1000, first[3]]);
    let b = layout.add_terminal_rect("plane", plane, "GND", [first[0] - 1000, first[1], first[0] + 1000, first[3]]);
    layout.add_port("p", a, b);
    let end = m.rects[m.rects.len() - 1];
    let region = if m.end_along_y {
        [end[0], end[3] - 1000, end[2], end[3] + 1000]
    } else {
        [end[2] - 1000, end[1], end[2] + 1000, end[3]]
    };
    let c = layout.add_terminal_rect("strip end", top, "S", region);
    let d = layout.add_terminal_rect("plane end", plane, "GND", region);
    layout.shorts.push(vec![c, d]);
    layout
}

fn ms_extract(m: &Ms, fine_um: f64) -> (f64, usize) {
    let layout = ms_layout(m, (fine_um * 1000.0) as i64);
    let x = extract(&layout, &extract_spec(fine_um), &[F_REF], false).expect("extract");
    (x.inductance(0, 0, 0) * 1e9, x.mesh.stats.filaments)
}

/// The closed-form inductance of a straight shorted microstrip from the port centre to the wall (zero thickness), nH.
fn straight_closed_form(h: i64, w: i64, length: i64) -> f64 {
    let (z0, ee) = hammerstad(w as f64 / h as f64, 4.3);
    let l = length as f64 * 1e-9;
    let ls: Vec<f64> = (0..=10)
        .map(|k| {
            let f = 50e6 + 10e6 * f64::from(k);
            z0 * (2.0 * PI * f * ee.sqrt() / 299_792_458.0 * l).tan() / (2.0 * PI * f)
        })
        .collect();
    1e9 * ls.iter().sum::<f64>() / ls.len() as f64
}

const H: i64 = 300_000;
const W: i64 = 800_000;

fn straight(len: i64) -> Ms {
    let rects = vec![[0, -W / 2, len, W / 2]];
    Ms { h: H, plane: grow(bounds(&rects), 5 * MM), rects, slot: None, end_along_y: false }
}

fn bend(l1: i64, l2: i64) -> Ms {
    let rects = vec![[0, -W / 2, l1 + W / 2, W / 2], [l1 - W / 2, -W / 2, l1 + W / 2, l2]];
    Ms { h: H, plane: grow(bounds(&rects), 5 * MM), rects, slot: None, end_along_y: true }
}

fn slotted(len: i64, slot_len: i64) -> Ms {
    let mut m = straight(len);
    m.slot = Some([len / 2 - slot_len / 2, -MM, len / 2 + slot_len / 2, MM]);
    m
}

struct Row {
    name: String,
    cell: i64,
    fdtd: f64,
    extract: Vec<(f64, f64)>,
    cells: usize,
    seconds: f64,
}

fn ms_row(name: &str, m: &Ms, cell: i64, fine_um: &[f64]) -> Row {
    let (fdtd, cells, seconds) = ms_fdtd(m, cell);
    let extract = fine_um.iter().map(|f| (*f, ms_extract(m, *f).0)).collect();
    let row = Row { name: name.to_owned(), cell, fdtd, extract, cells, seconds };
    print_row(&row);
    row
}

fn print_row(r: &Row) {
    let e: Vec<String> = r
        .extract
        .iter()
        .map(|(f, l)| format!("extract {f:.0} um {l:.4} nH (full wave {:+.2} %)", 100.0 * (r.fdtd / l - 1.0)))
        .collect();
    println!(
        "{}: cell {:.3} mm: full wave {:.4} nH ({} cells, {:.0} s); {}",
        r.name,
        r.cell as f64 * 1e-6,
        r.fdtd,
        r.cells,
        r.seconds,
        e.join("; ")
    );
}

/// The excess of a case over its control in both tools (nH) and how the full wave's compares with the reference's.
fn excess(case: &Row, control: &Row) -> (f64, f64) {
    let fw = case.fdtd - control.fdtd;
    let ex = case.extract.last().unwrap().1 - control.extract.last().unwrap().1;
    (fw, ex)
}

/// A right angle: 6 mm and 6 mm against the straight line of 12 mm (the same centreline length) at the standard cell.
/// The corner excess is the difference of the two in each tool.
#[test]
fn the_default_edge_rule_keeps_a_right_angle_bend_at_the_extraction_in_excess_of_the_straight_line() {
    let cell = 250_000;
    let control = ms_row("straight 12 mm (control)", &straight(12 * MM), cell, &[250.0]);
    let case = ms_row("bend 6 + 6 mm", &bend(6 * MM, 6 * MM), cell, &[250.0]);
    let (fw, ex) = excess(&case, &control);
    println!("corner excess: full wave {fw:+.4} nH, extraction {ex:+.4} nH");
    // The straight control sits within a few percent of the closed form and the extraction.
    let closed = straight_closed_form(H, W, 12 * MM - 125_000);
    println!("straight closed form {closed:.4} nH");
    assert!(
        (control.fdtd / control.extract[0].1 - 1.0).abs() < 0.08,
        "control: {} vs {}",
        control.fdtd,
        control.extract[0].1
    );
    assert!((case.fdtd / case.extract[0].1 - 1.0).abs() < 0.08, "bend: {} vs {}", case.fdtd, case.extract[0].1);
    // The corner changes the inductance by a fraction of a nanohenry in both tools, in the same direction.
    assert!((fw - ex).abs() < 0.15, "corner excess {fw} against {ex}");
}

#[test]
#[ignore = "study: about 10 minutes; `cargo test -p copper-extract --test edge_rule -- --ignored study_bend --nocapture`"]
fn study_bend_and_slot_series() {
    for cell in [250_000, 167_000] {
        let control = ms_row("straight 12 mm (control)", &straight(12 * MM), cell, &[250.0, 167.0]);
        let case = ms_row("bend 6 + 6 mm", &bend(6 * MM, 6 * MM), cell, &[250.0, 167.0]);
        let (fw, ex) = excess(&case, &control);
        println!("  corner excess at {} mm: full wave {fw:+.4} nH, extraction {ex:+.4} nH", cell as f64 * 1e-6);
        // The slotted plane has many more filaments: the reference is read at the two coarser pitches.
        let control_s = ms_row("straight 12 mm, slot-case control", &straight(12 * MM), cell, &[333.0, 250.0]);
        let slot = ms_row("straight 12 mm, 4 mm slot", &slotted(12 * MM, 4 * MM), cell, &[333.0, 250.0]);
        let (fw, ex) = excess(&slot, &control_s);
        println!("  slot excess at {} mm: full wave {fw:+.4} nH, extraction {ex:+.4} nH", cell as f64 * 1e-6);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Loop over a plane
// ---------------------------------------------------------------------------------------------------------------------

fn loop_fdtd(a: i64, b: i64, w: i64, gap: i64, h: i64, cell: i64) -> (f64, usize, f64) {
    let started = Instant::now();
    let mut board = bare_board();
    declare_stackup(
        &mut board,
        &[(Some(CopperLayer::Front), 35_000, "", ""), (None, h, "4.3", "0"), (Some(CopperLayer::Back), 35_000, "", "")],
    );
    let (cx, cy) = (50 * MM, 50 * MM);
    let (x0, x1, y0, y1) = (cx - a / 2, cx + a / 2, cy - b / 2, cy + b / 2);
    let f = CopperLayer::Front;
    let (gl, gr) = (cx - gap / 2, cx + gap / 2);
    board.tracks.push(track("L", (x0, y0), (gl - w / 2, y0), w, f));
    board.tracks.push(track("L", (gr + w / 2, y0), (x1, y0), w, f));
    board.tracks.push(track("L", (x0, y0), (x0, y1), w, f));
    board.tracks.push(track("L", (x1, y0), (x1, y1), w, f));
    board.tracks.push(track("L", (x0, y1), (x1, y1), w, f));
    board.zones.push(zone("GND", &[CopperLayer::Back], [x0 - 5 * MM, y0 - 5 * MM, x1 + 5 * MM, y1 + 5 * MM]));
    let mut spec =
        RegionSpec::new([x0 - w / 2 - 2 * MM, y0 - w / 2 - 2 * MM, x1 + w / 2 + 2 * MM, y1 + w / 2 + 2 * MM]);
    spec.margin_nm = 3 * MM - w / 2;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.cell_nm = Some(cell);
    spec.smooth_mesh = true;
    spec.ring_fit = false;
    spec.ports = vec![PortSpec::gap(
        "loop",
        Terminal::rect(f, [gl - w, y0 - w / 2, gl, y0 + w / 2]),
        Terminal::rect(f, [gr, y0 - w / 2, gr + w, y0 + w / 2]),
    )];
    let model = RegionModel::build(&board, &spec).expect("model");
    let col = model.solve_port(0).expect("solve");
    (band_l(&col), model.mesh.cells, started.elapsed().as_secs_f64())
}

/// The loop of `analytic.rs` over a plane (a floating plane under it, `plane` wide), at the position of the full-wave
/// loop (origin at the loop's corner).
fn loop_extract(a: i64, b: i64, w: i64, gap: i64, h: i64, plane: i64, fine_um: f64) -> (f64, usize) {
    let half = w / 2;
    let (xg0, xg1) = (a / 2 - gap / 2, a / 2 + gap / 2);
    let (cx, cy) = (a / 2, b / 2);
    let window = [cx - plane / 2, cy - plane / 2, cx + plane / 2, cy + plane / 2];
    let mut layout = Layout::new(window);
    let top = layout.add_layer("F.Cu", 0.0, T_REF);
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
    let pl = layout.add_layer("B.Cu", -(h as f64) * NM, T_REF);
    layout.add_rect(pl, "GND", window);
    let fine = (fine_um * 1000.0) as i64;
    let reach = 4 * h;
    for (rect, hx, hy) in [
        ([-reach, -reach, a + reach, reach], 0, fine),
        ([-reach, b - reach, a + reach, b + reach], 0, fine),
        ([-reach, -reach, reach, b + reach], fine, 0),
        ([a - reach, -reach, a + reach, b + reach], fine, 0),
    ] {
        layout.layers[pl].hints.push(Hint { rect, cell_x_nm: hx, cell_y_nm: hy });
    }
    let spec = MeshSpec { max_filaments: 7000, ..extract_spec(fine_um) };
    let x = extract(&layout, &spec, &[F_REF], false).expect("extract");
    (x.inductance(0, 0, 0) * 1e9, x.mesh.stats.filaments)
}

fn loop_closed_form(a: i64, b: i64, w: i64, gap: i64, h: i64) -> f64 {
    use copper_emi::inductance::{Segment, loop_inductance_over_plane};
    let (xg0, xg1) = (a / 2 - gap / 2, a / 2 + gap / 2);
    let seg = |p0: (i64, i64), p1: (i64, i64)| Segment {
        start: [p0.0 as f64 * NM, p0.1 as f64 * NM, h as f64 * NM],
        end: [p1.0 as f64 * NM, p1.1 as f64 * NM, h as f64 * NM],
        width: w as f64 * NM,
        thickness: 35e-6,
    };
    let path = vec![
        seg((xg0, 0), (0, 0)),
        seg((0, 0), (0, b)),
        seg((0, b), (a, b)),
        seg((a, b), (a, 0)),
        seg((a, 0), (xg1, 0)),
    ];
    1e9 * loop_inductance_over_plane(&path, 0.0)
}

/// An 8 x 5 mm loop of 0.6 mm trace, 0.3 mm over a plane, 1 mm gap: the loop of the validation (`3b`) at other
/// dimensions, against the closed form of its path over the image plane and the extraction.
#[test]
#[ignore = "study: about 6 minutes; `cargo test -p copper-extract --test edge_rule -- --ignored study_loop --nocapture`"]
fn study_loop_over_a_plane() {
    let (a, b, w, gap, h) = (8 * MM, 5 * MM, 600_000, MM, H);
    let closed = loop_closed_form(a, b, w, gap, h);
    println!(
        "loop {} x {} mm, w {} mm, h {} mm: closed form over the image plane {closed:.4} nH",
        a / MM,
        b / MM,
        w as f64 * 1e-6,
        h as f64 * 1e-6
    );
    for fine in [250.0, 167.0] {
        let (l, n) = loop_extract(a, b, w, gap, h, a + 10 * MM, fine);
        println!(
            "  extraction {fine:.0} um: {l:.4} nH ({n} filaments), against the closed form {:+.2} %",
            100.0 * (l / closed - 1.0)
        );
    }
    for cell in [250_000, 167_000] {
        let (l, cells, s) = loop_fdtd(a, b, w, gap, h, cell);
        println!(
            "  full wave cell {:.3} mm: {l:.4} nH ({cells} cells, {s:.0} s): against the closed form {:+.2} %",
            cell as f64 * 1e-6,
            100.0 * (l / closed - 1.0)
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Via transition between two layers
// ---------------------------------------------------------------------------------------------------------------------

const D1: i64 = 200_000;
const D2: i64 = 400_000;
const D3: i64 = 200_000;
const XV: i64 = 6 * MM;
const LEG: i64 = 6 * MM;
/// The stitching vias that join the two planes beside the signal via: at (+-1.5, +-1.5) mm.
const STITCH: i64 = 1_500_000;
/// The outer radius of the extraction's via tube in cells of the full-wave mesh: a one-cell PEC wire on a Yee grid
/// has an effective radius of about 0.23 cell (a reasoned figure, not a measured one; the study varies it).
const VIA_RADIUS_CELLS: f64 = 0.23;

fn octagon_nm(c: (i64, i64), r_nm: f64) -> Vec<(f64, f64)> {
    let k = r_nm / (PI / 8.0).cos();
    (0..8)
        .map(|i| {
            let a = PI * (f64::from(i) / 4.0 + 0.125);
            (c.0 as f64 + k * a.cos(), c.1 as f64 + k * a.sin())
        })
        .collect()
}

fn via_fdtd(cell: i64) -> (f64, usize, f64) {
    let started = Instant::now();
    let mut board = bare_board();
    use CopperLayer::*;
    declare_stackup(
        &mut board,
        &[
            (Some(Front), 35_000, "", ""),
            (None, D1, "4.3", "0"),
            (Some(In1), 35_000, "", ""),
            (None, D2, "4.3", "0"),
            (Some(In2), 35_000, "", ""),
            (None, D3, "4.3", "0"),
            (Some(Back), 35_000, "", ""),
        ],
    );
    let a = [0, -W / 2, XV + W / 2, W / 2];
    let b = [XV - W / 2, -W / 2, XV + LEG, W / 2];
    board.tracks.push(rect_track("S", a, Front));
    board.tracks.push(rect_track("S", b, Back));
    board.vias.push(via("S", (XV, 0), Front, Back));
    for (sx, sy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
        board.vias.push(via("GND", (XV + sx * STITCH, sy * STITCH), In1, In2));
    }
    let plane = grow(bounds(&[a, b]), 5 * MM);
    board.zones.push(zone("GND", &[In1, In2], plane));
    let mut spec = RegionSpec::new(grow(bounds(&[a, b]), 2 * MM));
    spec.margin_nm = 3 * MM;
    spec.air_margin_nm = Some(4 * MM);
    spec.band_hz = (50e6, 1.0e9);
    spec.cell_nm = Some(cell);
    spec.smooth_mesh = true;
    spec.ring_fit = false;
    spec.ports = vec![PortSpec::to_plane("p", Terminal::rect(Front, [a[0], a[1], a[0] + cell, a[3]]), In1)];
    let mut model = RegionModel::build(&board, &spec).expect("model");
    let strips: Vec<Sheet> = model.sheets.iter().filter(|s| s.kind == "track").cloned().collect();
    let zb_strip = strips.iter().find(|s| s.layer == Back).expect("bottom strip");
    let z_in2 = model.sheets.iter().find(|s| s.kind == "zone" && s.layer == In2).map(|s| s.z).expect("In2 plane");
    let last = bbox(zb_strip);
    let (xl, yl) = (model.axes[0].lines.clone(), model.axes[1].lines.clone());
    let x = nearest(&xl, last.1);
    for y in yl.iter().copied().filter(|y| *y >= last.2 - 1e-6 && *y <= last.3 + 1e-6) {
        model.wires.push(Wire {
            kind: "via",
            net: "S".into(),
            net_id: 1,
            x,
            y,
            z_top: zb_strip.z,
            z_bottom: z_in2,
            radius: 0.0,
        });
    }
    let col = model.solve_port(0).expect("solve");
    (band_l(&col), model.mesh.cells, started.elapsed().as_secs_f64())
}

fn via_extract(cell: i64, fine_um: f64, radius_cells: f64) -> (f64, usize) {
    let a = [0, -W / 2, XV + W / 2, W / 2];
    let b = [XV - W / 2, -W / 2, XV + LEG, W / 2];
    let plane = grow(bounds(&[a, b]), 5 * MM);
    let mut layout = Layout::new(plane);
    let z1 = -(D1 as f64) * NM;
    let z2 = -((D1 + D2) as f64) * NM;
    let z3 = -((D1 + D2 + D3) as f64) * NM;
    let top = layout.add_layer("F.Cu", 0.0, T_REF);
    let in1 = layout.add_layer("In1.Cu", z1, T_REF);
    let in2 = layout.add_layer("In2.Cu", z2, T_REF);
    let bot = layout.add_layer("B.Cu", z3, T_REF);
    layout.add_rect(top, "S", a);
    layout.add_rect(bot, "S", b);
    // The planes, with the octagonal antipad the full-wave model cuts round the signal via (barrel radius 0.3 mm plus
    // the 0.2 mm clearance of the zone).
    let full = poly::polygon(&[
        (plane[0] as f64, plane[1] as f64),
        (plane[2] as f64, plane[1] as f64),
        (plane[2] as f64, plane[3] as f64),
        (plane[0] as f64, plane[3] as f64),
    ]);
    let hole = poly::polygon(&octagon_nm((XV, 0), 500_000.0));
    for p in [in1, in2] {
        let shapes = poly::difference(&full, &hole);
        layout.layers[p].conductors.push(Conductor { net: "GND".to_owned(), shapes, on_loop: false });
        layout.layers[p].hints.push(Hint::square(grow([XV, 0, XV, 0], 1_200_000), (fine_um * 1000.0) as i64));
    }
    // A wire in the full-wave model has an effective radius of about 0.23 of the cell; a tube of that outer radius is
    // the extraction's counterpart (the tube of the board's 0.3 mm via would be a different, fatter conductor).
    let plating = 10_000;
    let outer = (radius_cells * cell as f64) as i64;
    let drill = 2 * (outer - plating);
    layout.vias.push(ViaGeom {
        name: "signal".into(),
        x: XV,
        y: 0,
        net: "S".into(),
        first_layer: top,
        last_layer: bot,
        drill_nm: drill,
        plating_nm: plating,
        layer_nets: Vec::new(),
    });
    for (k, (sx, sy)) in [(1, 1), (1, -1), (-1, 1), (-1, -1)].into_iter().enumerate() {
        layout.vias.push(ViaGeom {
            name: format!("stitch{k}"),
            x: XV + sx * STITCH,
            y: sy * STITCH,
            net: "GND".into(),
            first_layer: in1,
            last_layer: in2,
            drill_nm: drill,
            plating_nm: plating,
            layer_nets: Vec::new(),
        });
    }
    let reach = 4 * D1;
    layout.layers[in1].hints.push(Hint {
        rect: [a[0], a[1] - reach, a[2], a[3] + reach],
        cell_x_nm: 0,
        cell_y_nm: (fine_um * 1000.0) as i64,
    });
    layout.layers[in2].hints.push(Hint {
        rect: [b[0], b[1] - reach, b[2], b[3] + reach],
        cell_x_nm: 0,
        cell_y_nm: (fine_um * 1000.0) as i64,
    });
    let ta = layout.add_terminal_rect("strip", top, "S", [a[0] - 1000, a[1], a[0] + 1000, a[3]]);
    let tb = layout.add_terminal_rect("plane", in1, "GND", [a[0] - 1000, a[1], a[0] + 1000, a[3]]);
    layout.add_port("p", ta, tb);
    let end = [b[2] - 1000, b[1], b[2] + 1000, b[3]];
    let c = layout.add_terminal_rect("strip end", bot, "S", end);
    let d = layout.add_terminal_rect("plane end", in2, "GND", end);
    layout.shorts.push(vec![c, d]);
    let spec = MeshSpec { max_filaments: 11_000, ..extract_spec(fine_um) };
    let x = extract(&layout, &spec, &[F_REF], false).expect("extract");
    (x.inductance(0, 0, 0) * 1e9, x.mesh.stats.filaments)
}

#[test]
#[ignore = "study: about 6 minutes; `cargo test -p copper-extract --test edge_rule -- --ignored study_via --nocapture`"]
fn study_via_transition() {
    for cell in [250_000, 167_000] {
        let (l, cells, s) = via_fdtd(cell);
        let refs: Vec<String> = [250.0, 167.0]
            .iter()
            .map(|f| {
                let (e, n) = via_extract(cell, *f, VIA_RADIUS_CELLS);
                format!("extraction {f:.0} um {e:.4} nH ({n} filaments, full wave {:+.2} %)", 100.0 * (l / e - 1.0))
            })
            .collect();
        println!(
            "via transition, cell {:.3} mm: full wave {l:.4} nH ({cells} cells, {s:.0} s); {}",
            cell as f64 * 1e-6,
            refs.join("; ")
        );
    }
}

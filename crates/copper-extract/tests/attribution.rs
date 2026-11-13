//! Attribution of the difference between the extracted hot loop of `examples/emi_buck` and the traced closed form
//! of `EMI-HOT-LOOP` (section 13.8 of `docs/emi-noise-analysis.md`).
//!
//! The closed form (`copper_emi::inductance::loop_inductance_over_plane` of straight bars from pad centre to pad
//! centre, 1 mm wide, 35 um thick, at the height of the plane layer) is evaluated here on exactly the geometry that
//! is then handed to the extractor, so that every step of the walk from one to the other changes one thing.

use copper_emi::inductance::{Segment, loop_inductance, loop_inductance_over_plane};
use copper_extract::extract::extract;
use copper_extract::layout::{Conductor, Hint, Layout, Terminal};
use copper_extract::mesh::MeshSpec;

use std::path::{Path, PathBuf};

use copper_drc::emi::emi_check;
use copper_extract::board::board_model;
use copper_fullwave::hotloop::HotLoopInput;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::PhysicalBoard;
use copper_pcb::physicalize::{Footprints, physicalize};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

fn placed(name: &str) -> PhysicalBoard {
    let design = compile_design_file(&example(name).join("board.copper"), false, false).expect("compiles");
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    physicalize(&design, Footprints::Proxy, &options, false, false).expect("physicalizes")
}

const MM: f64 = 1e6;
const T: f64 = 35e-6;
/// Pad centres of the placed `emi_buck` loop in order (mm): CIN.1, U1.1, U1.3, D1.2, D1.1, CIN.2.
const PADS: [(f64, f64); 6] = [(25.5, 29.0), (27.5, 28.73), (27.5, 31.27), (25.0, 34.0), (23.0, 34.0), (23.5, 29.0)];
/// Height of the plane layer under the pad layer, metres (In1.Cu of the four-layer default stack-up).
const H: f64 = 0.533_333_333e-3;
const CENTRE: (f64, f64) = (25.0, 31.0);

fn rot(p: (f64, f64), angle: f64) -> (f64, f64) {
    let (s, c) = angle.sin_cos();
    let (dx, dy) = (p.0 - CENTRE.0, p.1 - CENTRE.1);
    (CENTRE.0 + c * dx - s * dy, CENTRE.1 + s * dx + c * dy)
}

fn bar_polygon(a: (f64, f64), b: (f64, f64), w: f64) -> Vec<(f64, f64)> {
    let l = (b.0 - a.0).hypot(b.1 - a.1);
    let n = (-(b.1 - a.1) / l * w / 2.0, (b.0 - a.0) / l * w / 2.0);
    vec![(a.0 + n.0, a.1 + n.1), (b.0 + n.0, b.1 + n.1), (b.0 - n.0, b.1 - n.1), (a.0 - n.0, a.1 - n.1)]
}

#[derive(Clone)]
struct Study {
    /// Width of the bars and of the square pads, mm.
    width: f64,
    pads: bool,
    plane: bool,
    gap_mm: f64,
    angle_deg: f64,
    cell_um: f64,
    coarse_um: f64,
    /// Plane margin round the loop's bounding box, mm.
    margin: f64,
    /// Scale of the loop's geometry about its centre (widths, gap and plane height stay).
    k: f64,
    /// Closed form only: every bar is cut by this length at both ends (mm).
    trim: f64,
    /// Multiplier of every pitch of the mesh (a refinement series divides it).
    scale: f64,
}

impl Study {
    fn new() -> Study {
        Study {
            width: 1.0,
            pads: false,
            plane: true,
            gap_mm: 0.2,
            angle_deg: 0.0,
            cell_um: 200.0,
            coarse_um: 1000.0,
            margin: 3.0,
            k: 1.0,
            trim: 0.0,
            scale: 1.0,
        }
    }

    fn pad(&self, i: usize) -> (f64, f64) {
        (CENTRE.0 + self.k * (PADS[i].0 - CENTRE.0), CENTRE.1 + self.k * (PADS[i].1 - CENTRE.1))
    }

    /// The path points in order with the gap points inserted into the switch leg.
    fn path(&self) -> Vec<(f64, f64)> {
        let (p1, p2) = (self.pad(1), self.pad(2));
        let mid = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
        let g = self.gap_mm / 2.0;
        let mut pts = Vec::new();
        for i in 0..PADS.len() {
            pts.push(self.pad(i));
            if i == 1 && self.gap_mm > 0.0 {
                pts.push((mid.0, mid.1 - g));
                pts.push((mid.0, mid.1 + g));
            }
        }
        pts
    }

    /// The closed-form segments of the open path (the gap carries no copper), metres.
    fn segments(&self) -> Vec<Segment> {
        let pts = self.path();
        let z = if self.plane { H } else { 0.0 };
        let n = pts.len();
        let a = self.angle_deg.to_radians();
        let gap_from = if self.gap_mm > 0.0 { Some(3) } else { None };
        let mut out = Vec::new();
        for i in 0..n {
            if Some(i + 1) == gap_from {
                continue;
            }
            let (p, q) = (rot(pts[i], a), rot(pts[(i + 1) % n], a));
            let l = (q.0 - p.0).hypot(q.1 - p.1);
            let (u, v) = ((q.0 - p.0) / l, (q.1 - p.1) / l);
            let (p, q) = ((p.0 + u * self.trim, p.1 + v * self.trim), (q.0 - u * self.trim, q.1 - v * self.trim));
            out.push(Segment {
                start: [p.0 * 1e-3, p.1 * 1e-3, z],
                end: [q.0 * 1e-3, q.1 * 1e-3, z],
                width: self.width * 1e-3,
                thickness: T,
            });
        }
        out
    }

    fn closed_form_nh(&self) -> f64 {
        let s = self.segments();
        1e9 * if self.plane { loop_inductance_over_plane(&s, 0.0) } else { loop_inductance(&s) }
    }

    fn layout(&self) -> Layout {
        let a = self.angle_deg.to_radians();
        let r = |p: (f64, f64)| {
            let q = rot(p, a);
            (q.0 * MM, q.1 * MM)
        };
        let pts = self.path();
        let n = pts.len();
        let gap_from = if self.gap_mm > 0.0 { Some(3) } else { None };
        let mut corners: Vec<(f64, f64)> = Vec::new();
        for i in 0..PADS.len() {
            let p = self.pad(i);
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                corners.push(r((p.0 + dx * self.width, p.1 + dy * self.width)));
            }
        }
        let fold = |f: fn(f64, f64) -> f64, init: f64, k: usize| {
            corners.iter().map(|p| if k == 0 { p.0 } else { p.1 }).fold(init, f)
        };
        let (x0, x1) = (fold(f64::min, f64::MAX, 0), fold(f64::max, f64::MIN, 0));
        let (y0, y1) = (fold(f64::min, f64::MAX, 1), fold(f64::max, f64::MIN, 1));
        let m = self.margin * MM;
        let window = [(x0 - m) as i64, (y0 - m) as i64, (x1 + m) as i64, (y1 + m) as i64];
        let mut layout = Layout::new(window);
        let top = layout.add_layer("F.Cu", 0.0, T);
        for i in 0..n {
            if Some(i + 1) == gap_from {
                continue;
            }
            let (p, q) = (r(pts[i]), r(pts[(i + 1) % n]));
            layout.add_polygon(top, "L", &bar_polygon(p, q, self.width * MM));
        }
        if self.pads {
            for i in 0..PADS.len() {
                let p = self.pad(i);
                let h = self.width / 2.0;
                let c: Vec<(f64, f64)> =
                    [(-h, -h), (h, -h), (h, h), (-h, h)].iter().map(|(dx, dy)| r((p.0 + dx, p.1 + dy))).collect();
                layout.add_polygon(top, "L", &c);
            }
        }
        // The port across the gap: thin terminals on the two bar ends.
        let g = self.gap_mm / 2.0;
        let (p1, p2) = (self.pad(1), self.pad(2));
        let mid = ((p1.0 + p2.0) / 2.0, (p1.1 + p2.1) / 2.0);
        let depth = self.cell_um * 1000.0 * 0.4;
        let (e0, e1) = (r((mid.0, mid.1 - g)), r((mid.0, mid.1 + g)));
        let len = (e1.0 - e0.0).hypot(e1.1 - e0.1);
        let dir = ((e1.0 - e0.0) / len, (e1.1 - e0.1) / len);
        let term = |layout: &mut Layout, name: &str, end: (f64, f64), sign: f64| {
            let inner = (end.0 - sign * dir.0 * depth, end.1 - sign * dir.1 * depth);
            let region = copper_cam::poly::polygon(&bar_polygon(end, inner, self.width * MM * 1.2));
            layout.terminals.push(Terminal { name: name.to_owned(), layer: top, net: "L".to_owned(), region });
            layout.terminals.len() - 1
        };
        let ta = term(&mut layout, "a", e0, 1.0);
        let tb = term(&mut layout, "b", e1, -1.0);
        layout.add_port("p", ta, tb);
        if self.plane {
            let pl = layout.add_layer("In1.Cu", -H, T);
            let pc = self.cell_um * 1000.0;
            let shapes = copper_cam::poly::union(&copper_cam::poly::polygon(&corners));
            let hull = rect_hull(&corners);
            let corridor =
                copper_cam::poly::offset(&copper_cam::poly::union(&[shapes, hull].concat()), m as i64, true, 5000.0);
            layout.layers[pl].conductors.push(Conductor { net: "GND".to_owned(), shapes: corridor, on_loop: false });
            let reach = (4.0 * H * 1e9) as i64;
            let rect = [(x0 as i64) - reach, (y0 as i64) - reach, (x1 as i64) + reach, (y1 as i64) + reach];
            layout.layers[pl].hints.push(Hint { rect, cell_x_nm: (2.0 * pc) as i64, cell_y_nm: (2.0 * pc) as i64 });
        }
        layout
    }

    fn extract_nh(&self, freqs: &[f64]) -> (usize, Vec<f64>) {
        let layout = self.layout();
        let spec = MeshSpec {
            cell_nm: (self.cell_um * 1000.0) as i64,
            coarse_nm: (self.coarse_um * 1000.0) as i64,
            coarsen: false,
            max_filaments: 9000,
            scale: self.scale,
            ..MeshSpec::default()
        };
        let x = extract(&layout, &spec, freqs, false).expect("extract");
        (x.mesh.stats.filaments, (0..freqs.len()).map(|f| x.inductance(f, 0, 0) * 1e9).collect())
    }
}

/// The rectangle that spans the loop's pads (a plane under the whole loop, not only under its copper).
fn rect_hull(corners: &[(f64, f64)]) -> copper_cam::poly::Shapes {
    let x0 = corners.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let x1 = corners.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let y0 = corners.iter().map(|p| p.1).fold(f64::MAX, f64::min);
    let y1 = corners.iter().map(|p| p.1).fold(f64::MIN, f64::max);
    copper_cam::poly::polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

fn report_and_input(board: &PhysicalBoard) -> (copper_drc::emi::EmiCheckReport, HotLoopInput) {
    let design = compile_design_file(&example("emi_buck").join("board.copper"), false, false).expect("compiles");
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    let report = emi_check(board, &view, &model);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("finding");
    let input = HotLoopInput::from_finding(finding, "buck").expect("input");
    (report, input)
}

fn closed_form_of_the_finding(report: &copper_drc::emi::EmiCheckReport) -> f64 {
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("finding");
    match finding.measures.get("loop_inductance_nh") {
        Some(copper_json::Value::Float(f)) => *f,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_study_geometry_is_the_geometry_of_the_closed_form_of_the_finding() {
    // The walk from the closed form to the extraction starts from the finding's own number: the six bars from pad
    // centre to pad centre, 1 mm wide, 35 um thick, at the height of the plane layer, evaluated by
    // `loop_inductance_over_plane`, and in free space by `loop_inductance`.
    let board = placed("emi_buck");
    let (report, input) = report_and_input(&board);
    assert_eq!(input.pads.len(), 6);
    let finding = closed_form_of_the_finding(&report);
    let study = Study { gap_mm: 0.0, ..Study::new() }.closed_form_nh();
    assert!((study / finding - 1.0).abs() < 1e-3, "study {study} nH, finding {finding} nH");
    // The pad centres and widths the study takes are the placed board's.
    let model = board_model(&board).expect("fill");
    for (i, (component, number)) in input.pads.iter().enumerate() {
        let (_, pad) = model
            .cam
            .pads()
            .find(|(f, p)| &f.reference == component && &p.number == number)
            .expect("pad on the exported board");
        let (x, y) = (pad.center.x as f64 * 1e-6, pad.center.y as f64 * 1e-6);
        assert!((x - PADS[i].0).abs() < 1e-6 && (y - PADS[i].1).abs() < 1e-6, "pad {component}.{number}: {x} {y}");
        assert!(pad.width.min(pad.height) >= 1_000_000, "the closed form's width clamp (1 mm) is the pad's");
    }
}

#[test]
fn the_deficit_to_the_closed_form_falls_as_the_loop_grows_against_the_trace_width() {
    // The closed form sums the partial inductances of bars along centrelines that overlap at the corners; the meshed
    // current cuts the corners. Same path, same 1 mm width, the path scaled by k about its centre: the extracted
    // inductance at 1 MHz over the closed form of the same open path (free space, gap of 0.2 mm in the switch bar).
    let mut deficits = Vec::new();
    for k in [1.0, 2.0, 4.0] {
        let s = Study { plane: false, k, ..Study::new() };
        let (_, l) = s.extract_nh(&[1e6]);
        deficits.push(l[0] / s.closed_form_nh() - 1.0);
    }
    println!("deficits at k = 1, 2, 4: {deficits:?}");
    assert!(deficits[0] < -0.05, "{deficits:?}");
    assert!(deficits[2].abs() < 0.05, "{deficits:?}");
    assert!(deficits[0].abs() > deficits[1].abs() && deficits[0].abs() > deficits[2].abs(), "{deficits:?}");
}

#[test]
fn a_quarter_turn_of_the_loop_moves_the_result_less_than_other_angles_do() {
    let l = |angle: f64| Study { plane: false, angle_deg: angle, ..Study::new() }.extract_nh(&[1e6]).1[0];
    let base = l(0.0);
    // The partition cuts x before y and rounds half cells down, so a quarter turn is not an exact symmetry of the mesh.
    let quarter: Vec<f64> = [90.0, 180.0].iter().map(|a| l(*a) / base - 1.0).collect();
    println!("free-space loop, L(1 MHz) relative to angle 0 at 90 and 180 degrees: {quarter:?}");
    assert!(quarter.iter().all(|q| q.abs() < 0.03), "{quarter:?}");
    // Other angles: the same closed form, a different staircase. The spread is what the Manhattan mesh costs.
    let spread: Vec<f64> = [20.0, 45.0].iter().map(|a| l(*a) / base - 1.0).collect();
    println!("free-space loop, L(1 MHz) relative to angle 0 at 20 and 45 degrees: {spread:?}");
    assert!(spread.iter().all(|s| s.abs() < 0.10), "{spread:?}");
}

#[test]
fn a_filament_does_not_bridge_the_port_gap_at_the_pitches_and_angles_of_the_study() {
    // The first version of the adaptive mesher put a filament of 14 % coverage across a 0.2 mm port gap when a cell
    // edge fell a third of a cell short of the end of the copper: the port then read 0.035 nH, a short, instead of
    // about 6.5 nH. The pitches below are equal to and finer than the gap (0.2 mm). With the gap in a single net (as
    // here, where the loop is one conductor), a pitch of twice the gap and a gap that is not along an axis still lets a
    // filament bridge it (20 degrees at 400 um read 0.106 nH, 45 degrees 0.097 nH): that case is not covered. The
    // board layouts give the two halves of the switch bar different nets, which nothing can bridge.
    for angle in [0.0, 20.0, 45.0] {
        for (cell_um, scale) in [(200.0, 1.0), (100.0, 1.0)] {
            let s = Study { plane: false, angle_deg: angle, cell_um, coarse_um: 5.0 * cell_um, scale, ..Study::new() };
            let l = s.extract_nh(&[1e6]).1[0];
            let want = s.closed_form_nh();
            assert!(
                l > 0.5 * want && l < 1.1 * want,
                "angle {angle}, cell {cell_um} um x {scale}: {l} nH against {want} nH"
            );
        }
    }
    let s = Study { plane: false, cell_um: 400.0, coarse_um: 2000.0, ..Study::new() };
    let l = s.extract_nh(&[1e6]).1[0];
    assert!(l > 0.5 * s.closed_form_nh(), "{l} nH");
}

/// The attribution table of section 13.8 (`cargo test -p copper-extract --test attribution -- --ignored --nocapture
/// study_attribution`, a few minutes).
#[test]
#[ignore = "a few minutes: the walk from the closed form to the extraction on the geometry of emi_buck"]
fn study_attribution() {
    let a = Study { gap_mm: 0.0, ..Study::new() };
    let b = Study { gap_mm: 0.0, plane: false, ..Study::new() };
    println!(
        "closed form, 6 segments: over plane {:.4} nH, free space {:.4} nH",
        a.closed_form_nh(),
        b.closed_form_nh()
    );
    let row = |label: &str, s: &Study, scales: &[f64]| {
        let want = s.closed_form_nh();
        let mut out = format!("{label}: closed form of the same open path {want:.4} nH;");
        for &scale in scales {
            let s = Study { scale, ..s.clone() };
            let (n, l) = s.extract_nh(&[1e6, 1e7]);
            out.push_str(&format!(
                " [scale {scale}: {n} fil, L1 {:.4} ({:+.1} %), L10 {:.4} ({:+.1} %)]",
                l[0],
                100.0 * (l[0] / want - 1.0),
                l[1],
                100.0 * (l[1] / want - 1.0)
            ));
        }
        println!("{out}");
    };
    // The plane layer is meshed at twice the pitch of the copper; its filaments grow fast with the scale.
    for plane in [false, true] {
        let scales: &[f64] = if plane { &[1.5, 1.0] } else { &[1.0, 0.7, 0.5] };
        for width in [1.0, 0.5, 0.25] {
            row(&format!("plane {plane} w {width} bars"), &Study { width, plane, ..Study::new() }, scales);
        }
        row(&format!("plane {plane} w 1 bars + square pads"), &Study { pads: true, plane, ..Study::new() }, scales);
    }
    // (The scaled loops over a plane need more filaments than the study's budget.)
    for k in [2.0, 4.0] {
        row(&format!("free space w 1 loop scaled x{k}"), &Study { k, plane: false, ..Study::new() }, &[1.5, 1.0]);
    }
    for angle in [0.0, 20.0, 30.0, 45.0, 90.0] {
        row(
            &format!("free space angle {angle}"),
            &Study { plane: false, angle_deg: angle, ..Study::new() },
            &[1.0, 0.7],
        );
    }
    for trim in [0.0, 0.125, 0.25] {
        println!(
            "closed form with every bar cut by {trim} mm at both ends: free {:.4}, plane {:.4} nH",
            Study { trim, plane: false, ..Study::new() }.closed_form_nh(),
            Study { trim, ..Study::new() }.closed_form_nh()
        );
    }
}

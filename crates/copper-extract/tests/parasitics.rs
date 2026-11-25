//! The board assembly of `copper_extract::parasitics` on a self-written routed board: two tracks on `F.Cu` over a
//! ground plane on `In1.Cu` (0.2 mm of er 4.3 between), routed at two lengths. The difference of the two boards is
//! the uniform middle of the tracks, so per millimetre it must be the 2-D cross-section's own values (the ends,
//! pads and jogs are the same on both boards and cancel):
//!
//! - capacitance to the plane and between the tracks: the cut's charges, within 0.1 %;
//! - resistance `rho / (w t)` exactly; inductance `[C_0'^-1]_00 / c^2` of the vacuum cut, within 1 %;
//! - the tracks against the closed forms (Hammerstad and Jensen, zero thickness) as a sanity check: the 35 um copper
//!   and the neighbour change them by a few per cent, printed with `--nocapture`.
//!
//! A board with a net left unrouted is refused with the pads it leaves apart.

use std::fs;
use std::path::PathBuf;

use copper_extract::board::board_model;
use copper_extract::capacitance::{Rect, SPEED_OF_LIGHT, Section, Slab, Wall, closed_form};
use copper_extract::parasitics::{ParasiticOptions, Parasitics, extract_parasitics};
use copper_lang::compiler::compile_design_file;
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

const PART: &str = r#"part Pad2 {
    category = "connector.header";
    pin S { number = "1"; domains = "analog"; directions = "passive"; }
    pin G { number = "2"; domains = "ground"; directions = "passive"; }
}
"#;

/// The board for tracks `length` mm long: A1, A2 joined by SIG at y = 20 mm; B1, B2 by SIG2, which runs 0.45 mm
/// beside SIG (centre to centre) between x = 14 and x = length + 8.
fn source(length: f64) -> String {
    format!(
        r#"board Pair {{
    import p "./parts";
    mechanical {{
        outline rectangle {{ width = 60mm; height = 40mm; }}
        stackup {{
            copper F.Cu {{ thickness = 0.035mm; }}
            dielectric P1 {{ thickness = 0.2mm; er = 4.3; loss_tangent = 0.02; material = "prepreg"; type = prepreg; }}
            copper In1.Cu {{ thickness = 0.035mm; }}
            dielectric C1 {{ thickness = 1.06mm; er = 4.6; loss_tangent = 0.02; material = "FR4"; type = core; }}
            copper In2.Cu {{ thickness = 0.035mm; }}
            dielectric P2 {{ thickness = 0.2mm; er = 4.3; loss_tangent = 0.02; material = "prepreg"; type = prepreg; }}
            copper B.Cu {{ thickness = 0.035mm; }}
        }}
    }}
    component A1: p.Pad2 {{ footprint = "PAD2"; }}
    component A2: p.Pad2 {{ footprint = "PAD2"; }}
    component B1: p.Pad2 {{ footprint = "PAD2"; }}
    component B2: p.Pad2 {{ footprint = "PAD2"; }}
    supply GND {{ net = GND; voltage = 0V; external = true; }}
    net SIG {{ A1.S; A2.S; }}
    net SIG2 {{ B1.S; B2.S; }}
    net GND {{ A1.G; A2.G; B1.G; B2.G; }}
    constraint copper_zone(GND) {{ id = "plane"; layers = "In1.Cu"; polygon_mm = "1,1; 59,1; 59,39; 1,39"; island_policy = keep_all; }}
    constraint fixed_placement(A1) {{ x = 10mm; y = 20mm; rotation = 180; }}
    constraint fixed_placement(A2) {{ x = {a2}mm; y = 20mm; rotation = 0; }}
    constraint fixed_placement(B1) {{ x = 15mm; y = 23mm; rotation = 0; }}
    constraint fixed_placement(B2) {{ x = {b2}mm; y = 23mm; rotation = 0; }}
}}
"#,
        a2 = length + 12.0,
        b2 = length + 9.0
    )
}

fn track(net: &str, a: (f64, f64), b: (f64, f64)) -> TrackSegment {
    let mm = |x: f64, y: f64| Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 };
    TrackSegment {
        net: net.to_owned(),
        start: mm(a.0, a.1),
        end: mm(b.0, b.1),
        width_nm: 250_000,
        layer: CopperLayer::Front,
    }
}

fn board(name: &str, length: f64, routed: bool) -> PhysicalBoard {
    let dir: PathBuf = std::env::temp_dir().join(format!("copper-parasitics-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    fs::write(dir.join("parts").join("pad2.copper"), PART).unwrap();
    fs::write(dir.join("board.copper"), source(length)).unwrap();
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (60.0, 40.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    let _ = fs::remove_dir_all(&dir);
    if routed {
        board.tracks.push(track("SIG", (11.0, 20.0), (length + 11.0, 20.0)));
    }
    let y = 20.45;
    board.tracks.push(track("SIG2", (14.0, 23.0), (14.0, y)));
    board.tracks.push(track("SIG2", (14.0, y), (length + 8.0, y)));
    board.tracks.push(track("SIG2", (length + 8.0, y), (length + 8.0, 23.0)));
    board
}

fn extract(name: &str, length: f64) -> Parasitics {
    let b = board(name, length, true);
    let model = board_model(&b).expect("fill");
    let networked = ["SIG".to_owned(), "SIG2".to_owned()].into();
    let options = ParasiticOptions { networked, ..ParasiticOptions::default() };
    extract_parasitics(&model, &b, &options).unwrap_or_else(|e| panic!("{e}"))
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b) / b
}

/// The cross-section of the uniform middle, as the board extraction builds it: the declared stack-up walked top
/// down (35 um copper, 0.2 mm of er 4.3 between the copper faces, and so on), the plane cut off at the cut's reach.
fn middle(options: &ParasiticOptions) -> Section {
    let (t, reach, mm) = (35e-6, options.cut_half_width_m, 1e-3);
    let in1 = (-t - 0.2 * mm - t, -t - 0.2 * mm);
    let bottom = -4.0 * t - 1.46 * mm;
    Section {
        conductors: vec![
            Rect { x0: -0.125 * mm, x1: 0.125 * mm, z0: -t, z1: 0.0, group: 0 },
            Rect { x0: 0.325 * mm, x1: 0.575 * mm, z0: -t, z1: 0.0, group: 1 },
            Rect { x0: -reach, x1: reach, z0: in1.0, z1: in1.1, group: 2 },
        ],
        slabs: vec![
            Slab { bottom: -t - 0.2 * mm, top: -t, er: 4.3 },
            Slab { bottom: -2.0 * t - 1.26 * mm, top: -2.0 * t - 0.2 * mm, er: 4.6 },
            Slab { bottom: -3.0 * t - 1.46 * mm, top: -3.0 * t - 1.26 * mm, er: 4.3 },
        ],
        bounds: [-reach, bottom - options.air_m, reach, options.air_m],
        sides: Wall::Grounded,
        ends: Wall::Grounded,
        mesh: options.mesh,
    }
}

#[test]
fn the_middle_of_two_tracks_over_a_plane_is_the_cross_section_per_millimetre() {
    let (short, long) = (extract("short", 10.0), extract("long", 20.0));
    let delta_mm = 10.0;
    let per_mm = |f: &dyn Fn(&Parasitics) -> f64| (f(&long) - f(&short)) / delta_mm;
    let c_plane = per_mm(&|p| p.between("SIG", "GND"));
    let c_pair = per_mm(&|p| p.between("SIG", "SIG2"));
    let options = ParasiticOptions::default();
    let section = middle(&options);
    let q = section.solve(&[1.0, 0.0, 0.0]).unwrap();
    let (cut_plane, cut_pair) = (-q.groups[2] * 1e-3, -q.groups[1] * 1e-3);
    println!("{} stations, {} cuts solved, {:.1} s (the long board)", long.stations, long.sections, long.seconds);
    println!(
        "SIG to GND {:.4} pF/mm (cut {:.4}, {:+.3} %), SIG to SIG2 {:.4} pF/mm (cut {:.4}, {:+.3} %)",
        c_plane * 1e12,
        cut_plane * 1e12,
        100.0 * rel(c_plane, cut_plane),
        c_pair * 1e12,
        cut_pair * 1e12,
        100.0 * rel(c_pair, cut_pair)
    );
    assert!(rel(c_plane, cut_plane).abs() < 1e-3, "{c_plane} against {cut_plane}");
    assert!(rel(c_pair, cut_pair).abs() < 1e-3, "{c_pair} against {cut_pair}");
    // Against the closed forms, zero thickness and without the neighbour: what thickness and SIG2 change.
    let alone = closed_form::microstrip_capacitance(0.25 / 0.2, 4.3) * 1e-3;
    println!(
        "a lone zero-thickness 0.25 mm microstrip on 0.2 mm (Hammerstad and Jensen): {:.4} pF/mm; SIG's total here {:.4} pF/mm",
        alone * 1e12,
        (c_plane + c_pair) * 1e12
    );
    assert!(((c_plane + c_pair) / alone - 1.0).abs() < 0.1);
    // Resistance and inductance of SIG's one branch between its two pads.
    let branch = |p: &Parasitics| {
        let net = &p.nets[p.net_index("SIG").unwrap()];
        assert_eq!(net.pads.len(), 2, "{:?}", net.pads);
        assert_eq!(net.branches.len(), 1, "{:?}", net.branches);
        net.branches[0].clone()
    };
    let (a, b) = (branch(&short), branch(&long));
    let r_per_mm = (b.resistance - a.resistance) / delta_mm;
    let expected_r = copper_extract::layout::COPPER_RESISTIVITY * 1e-3 / (0.25e-3 * 35e-6);
    assert!(rel(r_per_mm, expected_r).abs() < 1e-9, "{r_per_mm} against {expected_r}");
    // SIG2 carries no current: [C_0^-1]_00 of the 2 x 2 vacuum matrix of the two tracks, the plane the return.
    let mut vacuum = section.in_vacuum();
    vacuum.mesh.extrapolate = false;
    let (q0, q1) = (vacuum.solve(&[1.0, 0.0, 0.0]).unwrap(), vacuum.solve(&[0.0, 1.0, 0.0]).unwrap());
    let (c00, c01, c11) = (q0.groups[0], q1.groups[0], q1.groups[1]);
    let expected_l = 1e-3 * c11 / (c00 * c11 - c01 * c01) / (SPEED_OF_LIGHT * SPEED_OF_LIGHT);
    let l_per_mm = (b.inductance - a.inductance) / delta_mm;
    println!("SIG: {:.3} mohm/mm, {:.4} nH/mm (cut {:.4} nH/mm)", r_per_mm * 1e3, l_per_mm * 1e9, expected_l * 1e9);
    assert!(rel(l_per_mm, expected_l).abs() < 0.01, "{l_per_mm} against {expected_l}");
    // For scale, a lone zero-thickness strip at the same height: the 35 um copper (t/h 0.175) widens the track
    // and lowers its inductance by about a tenth.
    let lone = 1e-3 / (SPEED_OF_LIGHT * SPEED_OF_LIGHT * closed_form::strip_over_plane_capacitance(1.25, 1.0));
    println!("a lone zero-thickness strip at w/h 1.25: {:.4} nH/mm", lone * 1e9);
    assert!(l_per_mm < lone && l_per_mm > 0.85 * lone, "{l_per_mm} against {lone}");
    // The plane takes the field: what reaches the cut's box (2.5 mm away) is a few per cent.
    let net = &long.nets[long.net_index("SIG").unwrap()];
    let share = net.unattributed / long.between("SIG", "GND");
    println!("SIG: {:.3} pF to the box, {:.1} % of its capacitance to GND", net.unattributed * 1e12, 100.0 * share);
    assert!(share < 0.05, "{share}");
    assert!(long.flags.is_empty(), "{:?}", long.flags);
}

#[test]
fn a_net_left_unrouted_is_refused_with_its_pads() {
    let b = board("unrouted", 10.0, false);
    let model = board_model(&b).expect("fill");
    let options = ParasiticOptions { networked: ["SIG".to_owned()].into(), ..ParasiticOptions::default() };
    let error = extract_parasitics(&model, &b, &options).expect_err("SIG is not routed");
    assert!(error.contains("net SIG") && error.contains("not fully routed"), "{error}");
    assert!(error.contains("A1.1") && error.contains("A2.1"), "{error}");
}

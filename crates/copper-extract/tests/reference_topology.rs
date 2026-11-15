//! The independent reference solver against the mesher on the topology cases of `tests/topology.rs` (open or connected,
//! number of copper pieces) and, for a multi-port case with mutuals, against the PEEC extraction at a low frequency.

use copper_extract::extract::{Error, extract};
use copper_extract::layout::{COPPER_RESISTIVITY, Layout, ViaGeom};
use copper_extract::mesh::{MeshSpec, mesh};
use copper_extract::network::SolveError;
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

/// What the mesher says of a layout: the port is open, and the number of copper pieces of its topology check.
fn mesher_says(layout: &Layout) -> (bool, usize) {
    let spec = MeshSpec::default();
    let components = mesh(layout, &spec).expect("mesh").stats.topology.copper_components;
    match extract(layout, &spec, &[1e3], false) {
        Err(Error::Solve(SolveError::PortDisconnected(0))) => (true, components),
        Ok(_) => (false, components),
        Err(other) => panic!("{other}"),
    }
}

/// Two rectangles of one net `gap` apart in a small window; terminals at the far ends.
fn two_rectangles(gap: i64) -> Layout {
    let mut layout = Layout::new([0, 0, 3 * MM, MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let mid = 3 * MM / 2;
    let (a, b) = ([100_000, 100_000, mid - gap / 2, 900_000], [mid + gap / 2, 100_000, 2_900_000, 900_000]);
    layout.add_rect(top, "S", a);
    layout.add_rect(top, "S", b);
    let ta = layout.add_terminal_rect("left", top, "S", [a[0], a[1], a[0] + 300_000, a[3]]);
    let tb = layout.add_terminal_rect("right", top, "S", [b[2] - 300_000, b[1], b[2], b[3]]);
    layout.add_port("p", ta, tb);
    layout
}

#[test]
fn slots_from_0_03_to_1_mm_are_open_for_the_reference_and_the_mesher_alike() {
    for gap_um in [30i64, 100, 200, 400, 1000] {
        let layout = two_rectangles(gap_um * 1000);
        // The pixel is half the gap, so that a pixel centre falls in the gap.
        let r = solve(&layout, &at(gap_um as f64 / 2.0)).expect("solve");
        let (open, pieces) = mesher_says(&layout);
        println!(
            "slot {gap_um} um: reference open {} components {} / mesher open {open} pieces {pieces}",
            r.port_open[0], r.components
        );
        assert!(r.port_open[0] && open, "slot {gap_um} um");
        assert_eq!(r.components, pieces, "slot {gap_um} um");
        assert!(r.resistance_ohm[0][0].is_nan());
    }
}

#[test]
fn a_narrow_neck_is_connected_for_both_and_has_the_resistance_of_a_neck() {
    for neck_um in [150i64, 250] {
        let mut layout = Layout::new([0, 0, 12 * MM, 6 * MM]);
        let top = layout.add_layer("F.Cu", 0.0, T);
        let (a, b) = ([MM, MM, 5 * MM, 5 * MM], [7 * MM, MM, 11 * MM, 5 * MM]);
        layout.add_rect(top, "S", a);
        layout.add_rect(top, "S", b);
        let neck = neck_um * 1000;
        let y0 = 3 * MM - neck / 2;
        layout.add_rect(top, "S", [5 * MM, y0, 7 * MM, y0 + neck]);
        let ta = layout.add_terminal_rect("left", top, "S", [a[0], a[1], a[0] + MM / 2, a[3]]);
        let tb = layout.add_terminal_rect("right", top, "S", [b[2] - MM / 2, b[1], b[2], b[3]]);
        layout.add_port("p", ta, tb);
        let r = solve(&layout, &at(neck_um as f64 / 3.0)).expect("solve");
        let (open, pieces) = mesher_says(&layout);
        let peec = extract(&layout, &MeshSpec::default(), &[1e3], false).expect("extract").resistance(0, 0, 0);
        let neck_r = COPPER_RESISTIVITY * 2e-3 / (neck as f64 * NM * T);
        println!(
            "neck {neck_um} um: reference R {:.3} mohm (the neck alone {:.3}), PEEC {:.3} mohm; components {} / {pieces}",
            r.resistance_ohm[0][0] * 1e3,
            neck_r * 1e3,
            peec * 1e3,
            r.components
        );
        assert!(!r.port_open[0] && !open);
        assert_eq!(r.components, pieces);
        assert!(r.resistance_ohm[0][0] > neck_r && r.resistance_ohm[0][0] < 1.6 * neck_r);
        assert!(
            pct(peec, r.resistance_ohm[0][0]).abs() < 10.0,
            "PEEC {peec} against the reference {}",
            r.resistance_ohm[0][0]
        );
    }
}

/// Two islands `gap` apart; the port from a terminal at the gap's edge to the far end of the right or the left island.
fn islands_with_a_terminal_at_the_gap(gap: i64, across: bool) -> Layout {
    let mut layout = Layout::new([0, 0, 2 * MM, MM / 2]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let mid = MM;
    let (a, b) = ([100_000, 100_000, mid - gap / 2, 400_000], [mid + gap / 2, 100_000, 1_900_000, 400_000]);
    layout.add_rect(top, "S", a);
    layout.add_rect(top, "S", b);
    let near = layout.add_terminal_rect("at the gap", top, "S", [a[2] - 200_000, a[1], a[2], a[3]]);
    let far = if across {
        layout.add_terminal_rect("far right", top, "S", [b[2] - 200_000, b[1], b[2], b[3]])
    } else {
        layout.add_terminal_rect("far left", top, "S", [a[0], a[1], a[0] + 200_000, a[3]])
    };
    layout.add_port("p", near, far);
    layout
}

#[test]
fn a_terminal_at_a_gap_is_open_across_and_connected_on_its_own_island_for_both() {
    for gap_um in [10i64, 20, 30, 50] {
        let pitch = gap_um as f64 / 2.0;
        let (lay_across, lay_same) = (
            islands_with_a_terminal_at_the_gap(gap_um * 1000, true),
            islands_with_a_terminal_at_the_gap(gap_um * 1000, false),
        );
        let across = solve(&lay_across, &at(pitch)).expect("solve");
        let same = solve(&lay_same, &at(pitch)).expect("solve");
        let ((open_across, _), (open_same, _)) = (mesher_says(&lay_across), mesher_says(&lay_same));
        println!(
            "gap {gap_um} um: across: reference open {} / mesher open {open_across}; same island: reference open {} R {:.3} mohm / mesher open {open_same}",
            across.port_open[0],
            same.port_open[0],
            same.resistance_ohm[0][0] * 1e3
        );
        assert!(across.port_open[0] && open_across, "gap {gap_um} um");
        assert!(!same.port_open[0] && !open_same, "gap {gap_um} um");
    }
}

#[test]
fn a_via_beside_an_island_it_does_not_touch_joins_nothing_for_both() {
    let mut layout = Layout::new([0, 0, 12 * MM, 6 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let bottom = layout.add_layer("B.Cu", -0.2e-3, T);
    layout.add_rect(top, "S", [4 * MM, MM, 7 * MM, 5 * MM]);
    layout.add_rect(bottom, "S", [0, 0, 12 * MM, 6 * MM]);
    layout.vias.push(ViaGeom {
        name: "stray".to_owned(),
        x: 3 * MM,
        y: 3 * MM,
        net: "S".to_owned(),
        first_layer: 0,
        last_layer: 1,
        drill_nm: 300_000,
        plating_nm: 25_000,
        layer_nets: Vec::new(),
    });
    let island = layout.add_terminal_rect("island", top, "S", [6 * MM, MM, 7 * MM, 5 * MM]);
    let plane = layout.add_terminal_rect("plane", bottom, "S", [10 * MM, MM, 11 * MM, 5 * MM]);
    layout.add_port("p", island, plane);
    let r = solve(&layout, &at(250.0)).expect("solve");
    let (open, pieces) = mesher_says(&layout);
    assert!(r.port_open[0] && open);
    assert_eq!(r.components, pieces);
    // The same via on the island joins it to the plane for both.
    layout.vias[0].x = 5 * MM;
    let r = solve(&layout, &at(250.0)).expect("solve");
    let (open, _) = mesher_says(&layout);
    assert!(!r.port_open[0] && !open);
    println!(
        "a via on the island: reference R {:.3} mohm, L {:.3} nH",
        r.resistance_ohm[0][0] * 1e3,
        r.inductance_nh[0][0]
    );
}

/// Two bars 10 mm long, 0.6 mm wide, 1.5 mm apart, each with a port between wide terminals at its ends.
fn two_bars() -> Layout {
    let mut layout = Layout::new([0, -MM, 10 * MM, 3 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    for (net, y0) in [("A", 0), ("B", 2_100_000)] {
        layout.add_rect(top, net, [0, y0, 10 * MM, y0 + 600_000]);
        let a = layout.add_terminal_rect(&format!("{net} start"), top, net, [0, y0, 500_000, y0 + 600_000]);
        let b =
            layout.add_terminal_rect(&format!("{net} end"), top, net, [10 * MM - 500_000, y0, 10 * MM, y0 + 600_000]);
        layout.add_port(net, a, b);
    }
    layout
}

#[test]
fn a_two_port_case_with_a_mutual_agrees_between_the_reference_and_the_peec_extraction_at_a_low_frequency() {
    let layout = two_bars();
    let r = solve(&layout, &at(50.0)).expect("solve");
    let spec =
        MeshSpec { cell_nm: 62_500, coarse_nm: 500_000, coarsen: false, max_filaments: 12000, ..MeshSpec::default() };
    let mut dense = layout.clone();
    for y0 in [0, 2_100_000] {
        dense.layers[0].hints.push(copper_extract::layout::Hint {
            rect: [0, y0, 10 * MM, y0 + 600_000],
            cell_x_nm: 0,
            cell_y_nm: 62_500,
        });
    }
    let x = extract(&dense, &spec, &[100.0, 1e3], false).expect("extract");
    println!("reference R {:?} ohm, L {:?} nH", r.resistance_ohm, r.inductance_nh);
    for i in 0..2 {
        for j in 0..2 {
            let (rl, pl) = (r.inductance_nh[i][j], x.inductance(1, i, j) * 1e9);
            let (rr, pr) = (r.resistance_ohm[i][j], x.resistance(1, i, j));
            println!(
                "entry ({i},{j}): L reference {rl:.4} PEEC {pl:.4} nH ({:+.2} %); R reference {rr:.5e} PEEC {pr:.5e} ohm",
                pct(rl, pl)
            );
            assert!(pct(rl, pl).abs() < 3.0, "L ({i},{j})");
            if i == j {
                assert!(pct(rr, pr).abs() < 3.0, "R ({i},{j})");
            }
        }
    }
    // 100 Hz and 1 kHz are both in the DC limit.
    assert!((x.inductance(0, 0, 0) / x.inductance(1, 0, 0) - 1.0).abs() < 1e-4);
    assert!(r.inductance_nh[0][1] > 2.0, "the mutual between the bars is a real coupling");
}

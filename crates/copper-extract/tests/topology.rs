//! The meshed network has the connectivity of the copper polygons: copper that does not touch is never joined (a
//! slot is not bridged, whatever the pitch), copper that touches stays joined (a narrow neck), and a plane with a
//! slot makes the return current detour. Default mesh settings throughout: the cases are those of a user who did not
//! tune anything.

use copper_cam::poly;
use copper_extract::extract::{Error, extract};
use copper_extract::layout::{Conductor, Layout, ViaGeom};
use copper_extract::mesh::{MeshSpec, mesh};
use copper_extract::network::SolveError;

const MM: i64 = 1_000_000;
const T: f64 = 35e-6;

/// Two copper rectangles of one net on one layer, `slot` apart, with a port between a patch of each.
fn two_rectangles(slot: i64) -> Layout {
    let mut layout = Layout::new([0, 0, 12 * MM, 6 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let mid = 6 * MM;
    let a = [MM, MM, mid - slot / 2, 5 * MM];
    let b = [mid + slot / 2, MM, 11 * MM, 5 * MM];
    layout.add_rect(top, "S", a);
    layout.add_rect(top, "S", b);
    let ta = layout.add_terminal_rect("left", top, "S", [a[0], a[1], a[0] + MM / 2, a[3]]);
    let tb = layout.add_terminal_rect("right", top, "S", [b[2] - MM / 2, b[1], b[2], b[3]]);
    layout.add_port("p", ta, tb);
    layout
}

#[test]
fn two_rectangles_across_a_slot_are_an_open_circuit_at_default_settings() {
    let mut wrong: Vec<String> = Vec::new();
    for slot_um in [100, 200, 400, 1000] {
        let layout = two_rectangles(slot_um * 1000);
        match extract(&layout, &MeshSpec::default(), &[1e3], false) {
            Err(Error::Solve(SolveError::PortDisconnected(0))) => {}
            Ok(x) => wrong.push(format!(
                "slot {slot_um} um: a finite resistance {:.3} mOhm across copper that does not touch",
                x.resistance(0, 0, 0) * 1e3
            )),
            Err(other) => wrong.push(format!("slot {slot_um} um: {other}")),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_narrow_neck_of_copper_stays_connected() {
    // Two pads joined by a narrow, 2 mm long copper neck: R = rho l / (w t) (6.6 mOhm at 0.15 mm) plus the pads.
    for neck_um in [150i64, 250] {
        let neck = neck_um * 1000;
        let mut layout = Layout::new([0, 0, 12 * MM, 6 * MM]);
        let top = layout.add_layer("F.Cu", 0.0, T);
        let (a, b) = ([MM, MM, 5 * MM, 5 * MM], [7 * MM, MM, 11 * MM, 5 * MM]);
        layout.add_rect(top, "S", a);
        layout.add_rect(top, "S", b);
        let y0 = 3 * MM - neck / 2;
        layout.add_rect(top, "S", [5 * MM, y0, 7 * MM, y0 + neck]);
        let ta = layout.add_terminal_rect("left", top, "S", [a[0], a[1], a[0] + MM / 2, a[3]]);
        let tb = layout.add_terminal_rect("right", top, "S", [b[2] - MM / 2, b[1], b[2], b[3]]);
        layout.add_port("p", ta, tb);
        let x =
            extract(&layout, &MeshSpec::default(), &[1e3], false).unwrap_or_else(|e| panic!("neck {neck_um} um: {e}"));
        let r = x.resistance(0, 0, 0);
        let neck_r = copper_extract::layout::COPPER_RESISTIVITY * 2e-3 / (neck as f64 * 1e-9 * T);
        println!(
            "neck {neck_um} um: R {:.3} mOhm, {:.3} of the neck alone; dropped bars {}, rounds {}",
            r * 1e3,
            r / neck_r,
            x.mesh.stats.dropped_bars,
            x.mesh.stats.refine_rounds
        );
        assert!(
            r > 0.8 * neck_r && r < 1.6 * neck_r,
            "neck {neck_um} um: R {:.3} mOhm against the neck's {:.3} mOhm",
            r * 1e3,
            neck_r * 1e3
        );
        assert!(x.mesh.stats.topology.mismatches.is_empty(), "{:?}", x.mesh.stats.topology);
    }
}

/// A 0.8 mm trace on the top layer over a plane 0.2 mm below, shorted to the plane at its far end; the port is
/// between the trace's start and the plane right under it. `slot`: a slot of that width in the plane across the
/// trace's path, 6 mm long (the return current can go round its ends).
fn trace_over_plane(slot: Option<i64>) -> Layout {
    let mut layout = Layout::new([0, -8 * MM, 24 * MM, 8 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let bottom = layout.add_layer("B.Cu", -0.2e-3, T);
    layout.add_rect(top, "S", [2 * MM, -400_000, 22 * MM, 400_000]);
    let full = [0, -8 * MM, 24 * MM, 8 * MM];
    match slot {
        None => layout.add_rect(bottom, "GND", full),
        Some(w) => {
            let outer = poly::polygon(&[
                (full[0] as f64, full[1] as f64),
                (full[2] as f64, full[1] as f64),
                (full[2] as f64, full[3] as f64),
                (full[0] as f64, full[3] as f64),
            ]);
            let x = 12 * MM;
            let hole = poly::polygon(&[
                ((x - w / 2) as f64, (-3 * MM) as f64),
                ((x + w / 2) as f64, (-3 * MM) as f64),
                ((x + w / 2) as f64, (3 * MM) as f64),
                ((x - w / 2) as f64, (3 * MM) as f64),
            ]);
            layout.layers[bottom].conductors.push(Conductor {
                net: "GND".to_owned(),
                shapes: poly::difference(&outer, &hole),
                on_loop: false,
            });
        }
    }
    let ts = layout.add_terminal_rect("trace start", top, "S", [2 * MM, -400_000, 2 * MM + 400_000, 400_000]);
    let tg = layout.add_terminal_rect("plane start", bottom, "GND", [2 * MM, -400_000, 2 * MM + 400_000, 400_000]);
    layout.add_port("p", ts, tg);
    // The shorting via at the far end joins the trace to the plane.
    layout.vias.push(ViaGeom {
        name: "wall".to_owned(),
        x: 21 * MM + 600_000,
        y: 0,
        net: "S".to_owned(),
        first_layer: 0,
        last_layer: 1,
        drill_nm: 300_000,
        plating_nm: 25_000,
        layer_nets: vec![(1, "GND".to_owned())],
    });
    layout
}

#[test]
fn a_slot_under_a_trace_makes_the_return_current_detour() {
    let l = |slot: Option<i64>| {
        let x = extract(&trace_over_plane(slot), &MeshSpec::default(), &[1e8], false).expect("extraction");
        assert!(x.mesh.stats.topology.mismatches.is_empty(), "{:?}", x.mesh.stats.topology);
        x.inductance(0, 0, 0) * 1e9
    };
    let plain = l(None);
    println!("unslotted {plain:.3} nH");
    for slot_um in [400i64, 1000] {
        let slotted = l(Some(slot_um * 1000));
        println!("slot {slot_um} um: {slotted:.3} nH");
        assert!(slotted > 1.2 * plain, "slot {slot_um} um: {slotted:.3} nH against {plain:.3} nH without the slot");
    }
}

#[test]
fn a_clean_mesh_reports_copper_and_mesh_components_that_agree() {
    let layout = trace_over_plane(None);
    let mesh = mesh(&layout, &MeshSpec::default()).expect("mesh");
    let t = &mesh.stats.topology;
    assert!(t.mismatches.is_empty(), "{t:?}");
    assert_eq!(t.copper_components, 1, "the top trace and the plane are joined by the via: one piece of copper");
    assert_eq!(t.mesh_components, 1);
    assert!(mesh.stats.unresolved_gaps.is_empty());
}

#[test]
fn a_gap_narrower_than_the_smallest_cell_is_reported_and_never_bridged() {
    // 30 um between two rectangles: below the 50 um the mesher does not go under. No filament joins them, and the
    // statistics say a gap was not resolved.
    let layout = two_rectangles(30_000);
    let m = mesh(&layout, &MeshSpec::default()).expect("mesh");
    assert!(m.stats.unresolved_gap_count >= 1, "{:?}", m.stats);
    assert!(
        m.stats.unresolved_gaps.iter().all(|g| g.contains("F.Cu") && g.contains("net S")),
        "{:?}",
        m.stats.unresolved_gaps
    );
    assert!(m.stats.flags.iter().any(|f| f == "unresolved_gap"), "{:?}", m.stats.flags);
    assert!(m.stats.topology.mismatches.is_empty(), "{:?}", m.stats.topology);
    assert_eq!(m.stats.topology.copper_components, 2);
    assert_eq!(m.stats.topology.mesh_components, 2);
    assert!(matches!(
        extract(&layout, &MeshSpec::default(), &[1e3], false),
        Err(Error::Solve(SolveError::PortDisconnected(0)))
    ));
}

#[test]
fn resolved_slots_report_their_refinement_and_no_unresolved_gap() {
    for slot_um in [100, 200, 400, 1000] {
        let m = mesh(&two_rectangles(slot_um * 1000), &MeshSpec::default()).expect("mesh");
        assert_eq!(m.stats.unresolved_gap_count, 0, "slot {slot_um} um: {:?}", m.stats.unresolved_gaps);
        assert!(m.stats.topology.mismatches.is_empty(), "slot {slot_um} um: {:?}", m.stats.topology);
        assert_eq!((m.stats.topology.copper_components, m.stats.topology.mesh_components), (2, 2));
    }
}

/// Eight blocks of one net in a row, 0.137 mm apart, 4 mm wide; the port is across the first gap (a terminal in each of
/// the two blocks at its sides). The other six gaps are far from it.
fn blocks_in_a_row() -> Layout {
    let mut layout = Layout::new([0, 0, 40 * MM, 8 * MM]);
    let top = layout.add_layer("F.Cu", 0.0, T);
    let pitch = 4_137_000;
    let mut starts = Vec::new();
    for k in 0..8 {
        let x0 = MM + 123_000 + k * pitch;
        starts.push(x0);
        layout.add_rect(top, "S", [x0, MM, x0 + 4 * MM, 7 * MM]);
    }
    let a = layout.add_terminal_rect("block 0", top, "S", [starts[0] + 3 * MM, 3 * MM, starts[0] + 4 * MM, 5 * MM]);
    let b = layout.add_terminal_rect("block 1", top, "S", [starts[1], 3 * MM, starts[1] + MM, 5 * MM]);
    layout.add_port("p", a, b);
    layout
}

#[test]
fn the_refinement_nearest_the_port_goes_first_when_the_budget_cannot_hold_them_all() {
    let layout = blocks_in_a_row();
    let unlimited = MeshSpec { max_filaments: 100_000, coarsen: false, ..MeshSpec::default() };
    let full = mesh(&layout, &unlimited).expect("mesh");
    assert_eq!(full.stats.unresolved_gap_count, 0, "{:?}", full.stats.unresolved_gaps);
    assert!(full.stats.refinements_skipped == 0 && full.stats.refinements_applied > 0);
    // A budget a little under what the full refinement needs.
    let budget = full.stats.filaments - (full.stats.filaments / 6);
    let spec = MeshSpec { max_filaments: budget, coarsen: false, ..MeshSpec::default() };
    let m = mesh(&layout, &spec).expect("a mesh within the budget");
    let s = &m.stats;
    println!(
        "full {} filaments, budget {budget}: {} filaments, refinements applied {} skipped {}, unresolved {} {:?}",
        full.stats.filaments,
        s.filaments,
        s.refinements_applied,
        s.refinements_skipped,
        s.unresolved_gap_count,
        s.unresolved_gaps
    );
    assert!(s.filaments <= budget);
    assert!(s.refinements_skipped > 0 && s.refinements_applied > 0, "{s:?}");
    assert!(s.flags.iter().any(|f| f == "refinement_stopped_by_budget"), "{:?}", s.flags);
    assert!(s.flags.iter().any(|f| f == "unresolved_gap"), "{:?}", s.flags);
    // The gap beside the port (x from 5.123 to 5.26 mm) is resolved; what is reported is far from it.
    assert!(s.unresolved_gap_count >= 1);
    let x_of = |text: &str| -> f64 {
        let at = text.find("near (").expect("a place") + 6;
        text[at..].split(',').next().unwrap().trim().parse().unwrap()
    };
    for gap in &s.unresolved_gaps {
        assert!(x_of(gap) > 9.0, "the gap at the port is not resolved: {gap}");
    }
    // Nothing is bridged: the pieces are the components, in the mesh and in the copper, and the port is open.
    assert!(s.topology.mismatches.is_empty(), "{:?}", s.topology);
    assert_eq!((s.topology.copper_components, s.topology.mesh_components), (8, 8));
    assert!(matches!(extract(&layout, &spec, &[1e3], false), Err(Error::Solve(SolveError::PortDisconnected(0)))));
}

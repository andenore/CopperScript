//! The extraction on the example boards: `examples/emi_buck` placed and routed (the tracks of the CLI test of
//! `emi-check --run`) and `examples/emi_buck_filter`. The full-wave values quoted are those recorded in
//! `docs/emi-noise-analysis.md` section 11; the FDTD solver is not run here.

use std::path::{Path, PathBuf};

use copper_drc::emi::{EmiCheckReport, emi_check};
use copper_extract::board::board_model;
use copper_extract::hotloop::{HotLoopExtraction, HotLoopOptions, extract_hot_loop};
use copper_fullwave::hotloop::HotLoopInput;
use copper_json::Value;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
}

fn placed(name: &str) -> PhysicalBoard {
    let design = compile_design_file(&example(name).join("board.copper"), false, false).expect("compiles");
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false).unwrap();
    physicalize(&design, Footprints::Proxy, &options, false, false).expect("physicalizes")
}

/// The CLI test's tracks along the hot loop of `emi_buck`: three 0.4 mm legs.
fn route_emi_buck(board: &mut PhysicalBoard) {
    let mm = |x: f64, y: f64| Point { x_nm: (x * 1e6).round() as i64, y_nm: (y * 1e6).round() as i64 };
    let mut track = |net: &str, a: (f64, f64), b: (f64, f64)| {
        board.tracks.push(TrackSegment {
            net: net.to_owned(),
            start: mm(a.0, a.1),
            end: mm(b.0, b.1),
            width_nm: 400_000,
            layer: CopperLayer::Front,
        });
    };
    track("VIN", (25.5, 29.0), (25.5, 28.73));
    track("VIN", (25.5, 28.73), (27.5, 28.73));
    track("SW", (27.5, 31.27), (25.0, 31.27));
    track("SW", (25.0, 31.27), (25.0, 34.0));
    track("GND", (23.0, 34.0), (23.5, 34.0));
    track("GND", (23.5, 34.0), (23.5, 29.0));
}

fn report_of(name: &str, board: &PhysicalBoard) -> EmiCheckReport {
    let design = compile_design_file(&example(name).join("board.copper"), false, false).expect("compiles");
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    emi_check(board, &view, &model)
}

fn number(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Float(f)) => *f,
        Some(Value::Int(i)) => i.to_string().parse().unwrap(),
        other => panic!("{other:?}"),
    }
}

struct Case {
    extraction: HotLoopExtraction,
    closed_form_nh: f64,
    traced: String,
}

fn run(name: &str, board: &PhysicalBoard, options: &HotLoopOptions) -> Case {
    let report = report_of(name, board);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("hot loop finding");
    let input = HotLoopInput::from_finding(finding, "buck").expect("input");
    let model = board_model(board).expect("fill");
    let extraction = extract_hot_loop(&model, board, &input, options).expect("extraction");
    Case {
        extraction,
        closed_form_nh: number(finding.measures.get("loop_inductance_nh")),
        traced: finding.measures.get("trace_status").and_then(Value::as_str).unwrap_or("").to_owned(),
    }
}

fn describe(label: &str, case: &Case) {
    let x = &case.extraction;
    let round = |v: &[f64], k: f64| v.iter().map(|v| (v * k).round() / k).collect::<Vec<_>>();
    println!(
        "{label}: closed form {:.3} nH ({}); extracted L(1/10/100 MHz) {:?} nH, R {:?} mohm; mesh difference {:?} %; status {} {:?}; flags {:?}; filaments {}; {:.1} s; vias {} tracks {} pads {} fill shapes {}",
        case.closed_form_nh,
        case.traced,
        round(&x.inductance_nh, 1000.0),
        x.resistance_ohm.iter().map(|v| (v * 1e6).round() / 1000.0).collect::<Vec<_>>(),
        round(&x.mesh_difference_pct, 100.0),
        x.status,
        x.status_reasons,
        x.flags,
        x.meshes.iter().map(|m| m.filaments.to_string()).collect::<Vec<_>>().join("+"),
        x.seconds,
        x.vias,
        x.tracks,
        x.pads,
        x.fill_shapes,
    );
}

#[test]
fn emi_buck_placed_and_routed() {
    let options = HotLoopOptions::default();
    let placed_board = placed("emi_buck");
    let a = run("emi_buck", &placed_board, &options);
    describe("emi_buck placed", &a);
    let mut routed = placed("emi_buck");
    route_emi_buck(&mut routed);
    let b = run("emi_buck", &routed, &options);
    describe("emi_buck routed", &b);
    for (case, name) in [(&a, "placed"), (&b, "routed")] {
        let x = &case.extraction;
        // Sanity bounds only (screening evidence is compared in the documentation, not tuned here): the extracted
        // interconnect inductance is positive, below the closed form's whole path with its plane shortcut, and
        // within a factor two of it; the resistance is milliohms.
        assert!(
            x.inductance_nh.iter().all(|l| *l > 0.3 * case.closed_form_nh && *l < 1.2 * case.closed_form_nh),
            "{name}: {:?} vs {}",
            x.inductance_nh,
            case.closed_form_nh
        );
        assert!(x.resistance_ohm.iter().all(|r| *r > 1e-3 && *r < 0.1), "{name}: {:?}", x.resistance_ohm);
        assert!(
            x.inductance_nh[0] >= x.inductance_nh[2],
            "L falls with frequency as current crowds: {:?}",
            x.inductance_nh
        );
        assert!(x.resistance_ohm[0] <= x.resistance_ohm[2], "R rises with frequency: {:?}", x.resistance_ohm);
        assert_eq!(x.nets, ["GND", "SW", "VIN"]);
        assert!(x.flags.iter().any(|f| f == "assumed_stackup"), "{name}: {:?}", x.flags);
    }
    // The routed loop is longer than the placed one.
    assert!(b.extraction.inductance_nh[1] > a.extraction.inductance_nh[1]);
}

#[test]
fn the_extraction_of_a_board_is_repeatable() {
    let board = placed("emi_buck");
    let options = HotLoopOptions { second_mesh: false, ..HotLoopOptions::default() };
    let a = run("emi_buck", &board, &options);
    let b = run("emi_buck", &board, &options);
    for (x, y) in a.extraction.inductance_nh.iter().zip(&b.extraction.inductance_nh) {
        assert_eq!(x.to_bits(), y.to_bits());
    }
    assert_eq!(a.extraction.meshes[0].filaments, b.extraction.meshes[0].filaments);
}

#[test]
fn emi_buck_filter_hot_loop() {
    let board = placed("emi_buck_filter");
    let a = run("emi_buck_filter", &board, &HotLoopOptions::default());
    describe("emi_buck_filter placed", &a);
    let x = &a.extraction;
    assert!(
        x.inductance_nh.iter().all(|l| *l > 0.3 * a.closed_form_nh && *l < 1.2 * a.closed_form_nh),
        "{:?} vs {}",
        x.inductance_nh,
        a.closed_form_nh
    );
    assert_eq!(x.cell, "U1");
}

/// The mesh series of the two board cases (recorded in `docs/emi-noise-analysis.md`): the budget is lifted so that the
/// pitch can be halved, which takes several minutes.
#[test]
#[ignore = "several minutes: mesh series of emi_buck placed and routed with a large filament budget"]
fn study_emi_buck_mesh_series() {
    for routed in [false, true] {
        let mut board = placed("emi_buck");
        if routed {
            route_emi_buck(&mut board);
        }
        for (scale, budget) in [(2.0, 2400), (1.5, 4000), (1.0, 7000), (0.75, 10000)] {
            let mut options = HotLoopOptions { second_mesh: false, ..HotLoopOptions::default() };
            options.mesh.scale = scale;
            options.mesh.max_filaments = budget;
            options.mesh.coarsen = false;
            let started = std::time::Instant::now();
            let c = std::panic::catch_unwind(|| run("emi_buck", &board, &options));
            match c {
                Ok(c) => println!(
                    "{} scale {scale}: {} filaments, L(1/10/100 MHz) {:?} nH, R(10 MHz) {:.2} mohm, {:.0} s",
                    if routed { "routed" } else { "placed" },
                    c.extraction.meshes[0].filaments,
                    c.extraction.inductance_nh.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
                    c.extraction.resistance_ohm[1] * 1e3,
                    started.elapsed().as_secs_f64()
                ),
                Err(_) => println!("scale {scale}: over the budget of {budget}"),
            }
        }
    }
}

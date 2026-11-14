//! A capacitor of the hot net whose pads copper does not join to the hot capacitor's gets a straight bar there, that
//! pad only; a capacitor that copper joins keeps exactly its copper. `examples/emi_buck` with a second input
//! capacitor CIN2 (routed to the hot capacitor's pads by tracks) and a third, CIN3 (placed, not routed).

use std::fs;
use std::path::{Path, PathBuf};

use copper_drc::emi::emi_check;
use copper_extract::board::{board_model, build_layout, loop_ports};
use copper_extract::hotloop::{HotLoopExtraction, HotLoopOptions, extract_hot_loop};
use copper_fullwave::hotloop::HotLoopInput;
use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi;
use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck")
}

/// A copy of the example with the capacitors added: `CIN2` at (24.5, 27), `CIN3` at (21, 33) when `third`.
fn project(name: &str, third: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-extract-legs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("parts")).unwrap();
    for entry in fs::read_dir(example().join("parts")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
    }
    let mut board = fs::read_to_string(example().join("board.copper")).unwrap();
    let mut edits: Vec<(&str, String)> = vec![
        (
            "    component CBOOT:",
            "    component CIN2: p.Cap { value = 10uF; footprint = \"C_0805\"; }\n    component CBOOT:".to_owned(),
        ),
        ("net VIN { U1.VIN; CIN.A; }", "net VIN { U1.VIN; CIN.A; CIN2.A; }".to_owned()),
        ("D1.A; COUT.B;", "D1.A; CIN2.B; COUT.B;".to_owned()),
        (
            "    constraint fixed_placement(J1)",
            "    constraint fixed_placement(CIN2) { x = 24.5mm; y = 27mm; rotation = 180; }\n    constraint fixed_placement(J1)"
                .to_owned(),
        ),
    ];
    if third {
        edits.extend([
            (
                "    component CBOOT:",
                "    component CIN3: p.Cap { value = 10uF; footprint = \"C_0805\"; }\n    component CBOOT:".to_owned(),
            ),
            ("CIN.A; CIN2.A; }", "CIN.A; CIN2.A; CIN3.A; }".to_owned()),
            ("D1.A; CIN2.B;", "D1.A; CIN2.B; CIN3.B;".to_owned()),
            (
                "    constraint fixed_placement(J1)",
                "    constraint fixed_placement(CIN3) { x = 21mm; y = 33mm; rotation = 180; }\n    constraint fixed_placement(J1)"
                    .to_owned(),
            ),
        ]);
    }
    for (from, to) in &edits {
        assert!(board.contains(from), "the example has no {from:?}");
        board = board.replacen(from, to, 1);
    }
    fs::write(dir.join("board.copper"), board).unwrap();
    dir
}

fn route(board: &mut PhysicalBoard) {
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
    // The hot loop (as the CLI test routes it).
    track("VIN", (25.5, 29.0), (25.5, 28.73));
    track("VIN", (25.5, 28.73), (27.5, 28.73));
    track("SW", (27.5, 31.27), (25.0, 31.27));
    track("SW", (25.0, 31.27), (25.0, 34.0));
    track("GND", (23.0, 34.0), (23.5, 34.0));
    track("GND", (23.5, 34.0), (23.5, 29.0));
    // CIN2: its pads on the hot capacitor's two pads.
    track("VIN", (25.5, 27.0), (25.5, 29.0));
    track("GND", (23.5, 27.0), (23.5, 29.0));
}

struct Case {
    extraction: HotLoopExtraction,
    board: PhysicalBoard,
    input: HotLoopInput,
}

fn run(name: &str, ports: &[&str], legs: bool) -> Case {
    let dir = project(name, true);
    let design = compile_design_file(&dir.join("board.copper"), false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    route(&mut board);
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    let report = emi_check(&board, &view, &model);
    let _ = fs::remove_dir_all(&dir);
    let finding = report.findings.iter().find(|f| f.finding.code == "EMI-HOT-LOOP").expect("hot loop finding");
    let input = HotLoopInput::from_finding(finding, "buck").expect("input");
    let mut hot = HotLoopOptions::default();
    hot.mesh.max_filaments = 3000;
    hot.series = false;
    hot.board.capacitor_ports = ports.iter().map(|p| (*p).to_owned()).collect();
    hot.board.connection_legs = legs;
    let filled = board_model(&board).expect("fill");
    let extraction = extract_hot_loop(&filled, &board, &input, &hot).expect("extraction");
    Case { extraction, board, input }
}

#[test]
fn an_unconnected_capacitor_gets_no_port_no_bar_and_changes_nothing_of_the_others() {
    // CIN3 is on the board (its pads are copper) but no copper joins it to the hot capacitor. Asking for a port on
    // it, with the default options, gives it neither a port nor a bar: the others read exactly as without the request.
    let without = run("without", &["CIN2"], false);
    let asked = run("asked", &["CIN2", "CIN3"], false);
    let (a, b) = (&without.extraction, &asked.extraction);
    assert_eq!(b.port_labels, ["hot loop", "CIN2"], "{:?}", b.port_labels);
    assert!(b.synthesized_legs.is_empty() && !b.connection_legs, "{:?}", b.synthesized_legs);
    assert!(a.unconnected_capacitors.is_empty());
    assert_eq!(b.unconnected_capacitors.len(), 1, "{:?}", b.unconnected_capacitors);
    assert_eq!(b.unconnected_capacitors[0].capacitor, "CIN3");
    assert_eq!(b.unconnected_capacitors[0].pads.len(), 2, "{:?}", b.unconnected_capacitors);
    assert!(
        b.unconnected_capacitors[0].pads.iter().any(|p| p.starts_with("CIN3.1") && p.contains("VIN")),
        "{:?}",
        b.unconnected_capacitors
    );
    assert!(b.notes.iter().any(|n| n.contains("CIN3") && n.contains("no port and no bar")), "{:?}", b.notes);
    // CIN2 self, hot-loop self and their mutual: the same numbers, to 1e-9 relative, at every frequency.
    assert_eq!(a.inductance_matrix_nh.len(), b.inductance_matrix_nh.len());
    for (ma, mb) in a.inductance_matrix_nh.iter().zip(&b.inductance_matrix_nh) {
        for i in 0..2 {
            for j in 0..2 {
                assert!(
                    (ma[i][j] - mb[i][j]).abs() <= 1e-9 * ma[i][j].abs(),
                    "[{i}][{j}] {} against {}",
                    mb[i][j],
                    ma[i][j]
                );
            }
        }
    }
    assert_eq!(a.window_mm, b.window_mm);
    assert_eq!(a.meshes[0].filaments, b.meshes[0].filaments);
}

#[test]
fn explicit_connection_legs_are_per_pad_and_leave_a_routed_capacitor_its_copper() {
    let alone = run("alone", &["CIN2"], true);
    let both = run("both", &["CIN2", "CIN3"], true);
    for (name, case) in [("CIN2 alone", &alone), ("CIN2 and CIN3", &both)] {
        let x = &case.extraction;
        println!(
            "{name}: ports {:?}, status {} {:?}, legs {:?}, matrix at 10 MHz {:?}",
            x.port_labels, x.status, x.status_reasons, x.synthesized_legs, x.inductance_matrix_nh[1]
        );
    }
    // CIN2 is joined by copper: nothing is synthesized for it, in either run.
    assert!(alone.extraction.synthesized_legs.is_empty(), "{:?}", alone.extraction.synthesized_legs);
    assert!(!alone.extraction.connection_legs);
    let legs = &both.extraction.synthesized_legs;
    assert!(!legs.is_empty() && legs.iter().all(|l| l.capacitor == "CIN3"), "{legs:?}");
    assert_eq!(legs.len(), 2, "both pads of CIN3 are unconnected: {legs:?}");
    assert!(both.extraction.connection_legs);
    // CIN2's own port is what it is without CIN3: its self term and its coupling to the hot loop.
    let index = |x: &HotLoopExtraction, label: &str| x.port_labels.iter().position(|l| l == label).expect(label);
    let m_alone = &alone.extraction.inductance_matrix_nh[1];
    let m_both = &both.extraction.inductance_matrix_nh[1];
    let (a2, b2) = (index(&alone.extraction, "CIN2"), index(&both.extraction, "CIN2"));
    let (self_alone, self_both) = (m_alone[a2][a2], m_both[b2][b2]);
    assert!(
        (self_both - self_alone).abs() < 0.03 * self_alone,
        "CIN2 self inductance {self_both} nH with CIN3 against {self_alone} nH without it"
    );
    // The coupling to the hot loop moves a little because a capacitor with a port widens the window and the plane
    // corridor round the loop (CIN3 sits left of the others); it stays of the same size.
    let (mut_alone, mut_both) = (m_alone[0][a2], m_both[0][b2]);
    assert!(
        mut_alone > 0.0 && (mut_both - mut_alone).abs() < 0.15 * mut_alone,
        "hot loop - CIN2 mutual {mut_both} nH with CIN3 against {mut_alone} nH without it"
    );
    // The layout says the same: one straight bar for each pad of CIN3, none for CIN2.
    let model = board_model(&both.board).expect("fill");
    let ports = loop_ports(&both.input).expect("ports");
    let options = copper_extract::board::BoardOptions {
        capacitor_ports: vec!["CIN2".to_owned(), "CIN3".to_owned()],
        connection_legs: true,
        ..Default::default()
    };
    let layout = build_layout(&model, &both.board, &both.input, &ports, &options, 200_000, 1_000_000).expect("layout");
    assert_eq!(layout.synthesized_legs, both.extraction.synthesized_legs);
    for leg in &layout.synthesized_legs {
        assert!(leg.length_mm > 3.0 && leg.length_mm < 8.0, "{leg:?}");
    }
    // CIN3 is reported as synthesized in the multi-port result: its pad terminals are named.
    assert!(
        both.extraction.notes.iter().any(|n| n.contains("CIN3") && n.contains("straight bar")),
        "{:?}",
        both.extraction.notes
    );
    assert!(
        !both.extraction.notes.iter().any(|n| n.contains("CIN2") && n.contains("straight bar")),
        "{:?}",
        both.extraction.notes
    );
}

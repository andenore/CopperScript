//! The board of `examples/emi_return_path` with its hand-placed copper, shared by the return-path tests and by the
//! CLI test that writes `examples/emi_return_path/routed/final-board.state.json`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi::{self, EmiModel};
use copper_lang::model::FlatElectricalView;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_physical::{CopperLayer, Point};

pub fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_return_path")
}

pub fn mm_point(x: f64, y: f64) -> Point {
    Point::new((x * 1e6).round() as i64, (y * 1e6).round() as i64)
}

pub fn track(net: &str, layer: CopperLayer, from: (f64, f64), to: (f64, f64), width_mm: f64) -> TrackSegment {
    TrackSegment {
        net: net.to_owned(),
        start: mm_point(from.0, from.1),
        end: mm_point(to.0, to.1),
        width_nm: (width_mm * 1e6).round() as i64,
        layer,
    }
}

pub fn through_via(net: &str, at: (f64, f64)) -> Via {
    Via {
        net: net.to_owned(),
        position: mm_point(at.0, at.1),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    }
}

/// The hand-placed copper of the four cases (the cases are described in the example's `board.copper`).
pub fn route(board: &mut PhysicalBoard) {
    let (front, back) = (CopperLayer::Front, CopperLayer::Back);
    // (a) a pair over the left pour of In1.Cu, clear of every edge.
    board.tracks.push(track("LANE_A_P", front, (8.0, 20.0), (34.0, 20.0), 0.12));
    board.tracks.push(track("LANE_A_N", front, (8.0, 20.27), (34.0, 20.27), 0.12));
    // (b) a pair crossing the cut between the pours (x from 39 to 41 mm).
    board.tracks.push(track("LANE_B_P", front, (30.0, 40.0), (50.0, 40.0), 0.12));
    board.tracks.push(track("LANE_B_N", front, (30.0, 40.27), (50.0, 40.27), 0.12));
    // (c) F.Cu over In1.Cu, a via, B.Cu over In2.Cu. A ground via 1.5 mm from CLK_S's via; none near CLK_U's.
    board.tracks.push(track("CLK_S", front, (10.0, 55.0), (25.0, 55.0), 0.15));
    board.tracks.push(track("CLK_S", back, (25.0, 55.0), (40.0, 55.0), 0.15));
    board.vias.push(through_via("CLK_S", (25.0, 55.0)));
    board.vias.push(through_via("GND", (26.5, 55.0)));
    board.tracks.push(track("CLK_U", front, (10.0, 62.0), (25.0, 62.0), 0.15));
    board.tracks.push(track("CLK_U", back, (25.0, 62.0), (40.0, 62.0), 0.15));
    board.vias.push(through_via("CLK_U", (25.0, 62.0)));
    board.vias.push(through_via("GND", (37.0, 62.0)));
    // (d) 3 mm from the board edge and 1 mm from the edge of the plane.
    board.tracks.push(track("CLK_E", front, (3.0, 30.0), (3.0, 50.0), 0.15));
}

pub struct Loaded {
    pub board: PhysicalBoard,
    pub view: FlatElectricalView,
    pub model: EmiModel,
}

/// The example compiled and physicalized with its proxy footprints; `routed` says whether the hand-placed copper is added.
pub fn load(routed: bool) -> Loaded {
    load_from(&example().join("board.copper"), routed)
}

fn load_from(path: &Path, routed: bool) -> Loaded {
    let design = compile_design_file(path, false, false).unwrap_or_else(|e| panic!("{e}"));
    let options = copper_pcb::configuration::physical_options(&design, (100.0, 80.0), None, "generic", false, false)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let mut board = physicalize(&design, Footprints::Proxy, &options, false, false).unwrap_or_else(|e| panic!("{e:?}"));
    if routed {
        route(&mut board);
    }
    let view = elaborate(&design.electrical).expect("elaborates");
    let model = emi::analyze(&view);
    Loaded { board, view, model }
}

/// [`load`] on a copy of the example whose files are edited first: `(file under the example, from, to)`; the copy
/// is named `name` in the temporary directory and removed again.
pub fn load_edited(name: &str, edits: &[(&str, &str, &str)], routed: bool) -> Loaded {
    let source = example();
    let dir = std::env::temp_dir().join(format!("copper-return-path-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    for entry in std::fs::read_dir(source.join("parts")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
    }
    std::fs::copy(source.join("board.copper"), dir.join("board.copper")).unwrap();
    for (file, from, to) in edits {
        let path = dir.join(file);
        if from.is_empty() {
            std::fs::write(&path, to).unwrap();
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{file} has no {from:?}");
        std::fs::write(&path, text.replace(from, to)).unwrap();
    }
    let loaded = load_from(&dir.join("board.copper"), routed);
    let _ = std::fs::remove_dir_all(&dir);
    loaded
}

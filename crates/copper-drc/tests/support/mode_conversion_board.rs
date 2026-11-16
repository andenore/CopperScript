//! The board of `examples/emi_mode_conversion` with its hand-placed copper, shared by the mode-conversion tests and by
//! the CLI test that writes `examples/emi_mode_conversion/routed/final-board.state.json`.

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_mode_conversion")
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

/// The hand-placed copper of the four lanes (described in the example's `board.copper`), all on F.Cu over the whole
/// ground plane of In1.Cu, 0.12 mm wide, the N leg 0.27 mm from the P leg. Lengths in mm: A: 30 and 40; B: 30 and 30;
/// C: 12 and 16, then 10 and 10, then 12 and 12; D: 12 and 12, then 10 and 16.
pub fn route(board: &mut PhysicalBoard) {
    let front = CopperLayer::Front;
    let mut pair = |p: &str, n: &str, y: f64, p_x: (f64, f64), n_x: (f64, f64)| {
        board.tracks.push(track(p, front, (p_x.0, y), (p_x.1, y), 0.12));
        board.tracks.push(track(n, front, (n_x.0, y + 0.27), (n_x.1, y + 0.27), 0.12));
    };
    // (a) a skew of 10 mm.
    pair("LANE_A_P", "LANE_A_N", 20.0, (8.0, 38.0), (8.0, 48.0));
    // (b) no skew.
    pair("LANE_B_P", "LANE_B_N", 40.0, (8.0, 38.0), (8.0, 38.0));
    // (c) a skew of 4 mm in the section before the choke, none after.
    pair("LANE_C_P", "LANE_C_N", 50.0, (8.0, 20.0), (8.0, 24.0));
    pair("LANE_C_P_CAP", "LANE_C_N_CAP", 50.0, (22.0, 32.0), (26.0, 36.0));
    pair("LANE_C_P_OUT", "LANE_C_N_OUT", 50.0, (34.0, 46.0), (38.0, 50.0));
    // (d) no skew before the choke, 6 mm after it.
    pair("LANE_D_P", "LANE_D_N", 60.0, (8.0, 20.0), (8.0, 20.0));
    pair("LANE_D_P_OUT", "LANE_D_N_OUT", 60.0, (22.0, 32.0), (22.0, 38.0));
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
    let dir = std::env::temp_dir().join(format!("copper-mode-conversion-{name}-{}", std::process::id()));
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

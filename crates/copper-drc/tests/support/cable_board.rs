//! The board of `examples/emi_cable_common_mode` with its hand-placed copper, shared by the cable common-mode tests and by
//! the CLI test that writes `examples/emi_cable_common_mode/routed/final-board.state.json`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use copper_lang::compiler::compile_design_file;
use copper_lang::elaborate::elaborate;
use copper_lang::emi::{self, EmiModel};
use copper_lang::model::FlatElectricalView;
use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_pcb::physicalize::{Footprints, physicalize};
use copper_pcb::transform::transformed_local_point;
use copper_physical::{CopperLayer, Point};

pub fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_cable_common_mode")
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

/// The y of the sense path of the ground-noise lanes of J3: the ground pad of U3 (12.5, 12.635) to the ground pad of J3
/// (90.5, 12.635), mm (proxy footprints: five pins, the fifth at local (+2.5, +0.635)).
pub const SENSE_Y: f64 = 12.635;
/// The x range of the routes of the ground-noise lanes, mm.
pub const GN_X: (f64, f64) = (20.0, 80.0);

/// The hand-placed copper (described in the example's `board.copper`), all on F.Cu over the whole ground plane of In1.Cu,
/// 0.12 mm wide, the N leg 0.27 mm from the P leg. Lengths in mm: N: 30 and 36; Z: 12 and 12, then 10 and 16; R and S: 12
/// and 16, then 12 and 12; C: 12 and 16, then 10 and 10, then 12 and 12; D: 12 and 12, then 10 and 16; M as D.
pub fn route(board: &mut PhysicalBoard) {
    let front = CopperLayer::Front;
    let mut pair = |p: &str, n: &str, y: f64, p_x: (f64, f64), n_x: (f64, f64)| {
        board.tracks.push(track(p, front, (p_x.0, y), (p_x.1, y), 0.12));
        board.tracks.push(track(n, front, (n_x.0, y + 0.27), (n_x.1, y + 0.27), 0.12));
    };
    // LANE_N: 6 mm of skew, no choke.
    pair("LANE_N_P", "LANE_N_N", 20.0, (8.0, 38.0), (8.0, 44.0));
    // LANE_Z: a choke of zero impedance, the 6 mm after it.
    pair("LANE_Z_P", "LANE_Z_N", 25.0, (8.0, 20.0), (8.0, 20.0));
    pair("LANE_Z_P_OUT", "LANE_Z_N_OUT", 25.0, (22.0, 32.0), (22.0, 38.0));
    // LANE_R and LANE_S: 4 mm of skew before the choke that declares one magnitude.
    pair("LANE_R_P", "LANE_R_N", 30.0, (8.0, 20.0), (8.0, 24.0));
    pair("LANE_R_P_OUT", "LANE_R_N_OUT", 30.0, (26.0, 38.0), (26.0, 38.0));
    pair("LANE_S_P", "LANE_S_N", 35.0, (8.0, 20.0), (8.0, 24.0));
    pair("LANE_S_P_OUT", "LANE_S_N_OUT", 35.0, (26.0, 38.0), (26.0, 38.0));
    // LANE_C: 4 mm of skew before the choke, none after.
    pair("LANE_C_P", "LANE_C_N", 50.0, (8.0, 20.0), (8.0, 24.0));
    pair("LANE_C_P_CAP", "LANE_C_N_CAP", 50.0, (22.0, 32.0), (26.0, 36.0));
    pair("LANE_C_P_OUT", "LANE_C_N_OUT", 50.0, (34.0, 46.0), (38.0, 50.0));
    // LANE_D: no skew before the choke, 6 mm after it.
    pair("LANE_D_P", "LANE_D_N", 60.0, (8.0, 20.0), (8.0, 20.0));
    pair("LANE_D_P_OUT", "LANE_D_N_OUT", 60.0, (22.0, 32.0), (22.0, 38.0));
    // LANE_M (on J2): the channel of LANE_D.
    pair("LANE_M_P", "LANE_M_N", 66.0, (8.0, 20.0), (8.0, 20.0));
    pair("LANE_M_P_OUT", "LANE_M_N_OUT", 66.0, (22.0, 32.0), (22.0, 38.0));
    // The ground-noise lanes of J3 beside the sense path: SE1 2 mm above it, the pair 1 mm either side of it. SE2 and the
    // clocks are not routed.
    board.tracks.push(track("SE1", front, (GN_X.0, SENSE_Y + 2.0), (GN_X.1, SENSE_Y + 2.0), 0.12));
    board.tracks.push(track("GN_P", front, (GN_X.0, SENSE_Y + 1.0), (GN_X.1, SENSE_Y + 1.0), 0.12));
    board.tracks.push(track("GN_N", front, (GN_X.0, SENSE_Y - 1.0), (GN_X.1, SENSE_Y - 1.0), 0.12));
    // The shield pin SH of J2 (88, 72) + (+2.5, +0.635): three vias to the ground planes.
    for at in [(92.0, 72.635), (92.0, 74.0), (92.0, 71.3)] {
        board.vias.push(through_via("GND", at));
    }
}

pub struct Loaded {
    pub board: PhysicalBoard,
    pub view: FlatElectricalView,
    pub model: EmiModel,
}

impl Loaded {
    /// The centre of pad `number` of component `reference`, mm.
    pub fn pad_mm(&self, reference: &str, number: &str) -> (f64, f64) {
        let placement = self.board.placements.iter().find(|p| p.reference == reference).expect("placed");
        let footprint = &self.board.footprints[&placement.footprint];
        let pad = footprint.pads.iter().find(|pad| pad.number == number).expect("pad");
        let at = transformed_local_point(placement, pad.position);
        (at.x_nm as f64 * 1e-6, at.y_nm as f64 * 1e-6)
    }
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
    let dir = std::env::temp_dir().join(format!("copper-cable-{name}-{}", std::process::id()));
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

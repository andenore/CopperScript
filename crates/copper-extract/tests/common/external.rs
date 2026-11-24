//! Two mirrored, narrow external PCB contours with one shared return contact.
//! No device/package model: VIN/VOUT rails are 20 mm, rail-to-return separation 4 mm.
use std::path::Path;

use copper_pcb::board::{PhysicalBoard, TrackSegment};
use copper_physical::{CopperLayer, Point};

pub fn board() -> PhysicalBoard {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck_boost/board.copper");
    let design = copper_lang::compiler::compile_design_file(&source, false, false).unwrap();
    let options =
        copper_pcb::configuration::physical_options(&design, (100.0, 100.0), None, "generic", false, false).unwrap();
    let mut board = copper_pcb::physicalize::physicalize(
        &design,
        copper_pcb::physicalize::Footprints::Proxy,
        &options,
        false,
        false,
    )
    .unwrap();
    board.zones.clear();
    for placement in &mut board.placements {
        placement.rotation_degrees = "0".parse().unwrap();
        placement.position = match placement.reference.as_str() {
            "U1" => Point::new(40_000_000, 34_000_000),
            "CIN" => Point::new(20_000_000, 32_000_000),
            "COUT" => {
                placement.rotation_degrees = "180".parse().unwrap();
                Point::new(60_000_000, 36_000_000)
            }
            _ => Point::new(90_000_000, 90_000_000),
        };
    }
    let u = board.placements.iter().find(|p| p.reference == "U1").unwrap().footprint.clone();
    for pad in &mut board.footprints.get_mut(&u).unwrap().pads {
        pad.position = match pad.number.as_str() {
            "3" => Point::new(0, -4_000_000),
            "4" => Point::new(0, 4_000_000),
            "5" => Point::new(0, 0),
            _ => Point::new(pad.number.parse::<i64>().unwrap() * 1_000_000, 20_000_000),
        };
        pad.size.width_nm = 200_000;
        pad.size.height_nm = 200_000;
        pad.shape = copper_physical::footprint::PadShape::Rectangle;
    }
    for reference in ["CIN", "COUT"] {
        let key = board.placements.iter().find(|p| p.reference == reference).unwrap().footprint.clone();
        for pad in &mut board.footprints.get_mut(&key).unwrap().pads {
            pad.position = Point::new(0, if pad.number == "1" { -2_000_000 } else { 2_000_000 });
            pad.size.width_nm = 200_000;
            pad.size.height_nm = 200_000;
            pad.shape = copper_physical::footprint::PadShape::Rectangle;
        }
    }
    for (net, a, b) in [
        ("VIN", (20_000_000, 30_000_000), (40_000_000, 30_000_000)),
        ("GND", (20_000_000, 34_000_000), (60_000_000, 34_000_000)),
        ("VOUT", (40_000_000, 38_000_000), (60_000_000, 38_000_000)),
    ] {
        board.tracks.push(TrackSegment {
            net: net.to_owned(),
            start: Point::new(a.0, a.1),
            end: Point::new(b.0, b.1),
            width_nm: 200_000,
            layer: CopperLayer::Front,
        });
    }
    copper_fullwave::testing::declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, 1_000_000, "4.3", "0.02"),
            (Some(CopperLayer::In1), 35_000, "", ""),
            (None, 500_000, "4.3", "0.02"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    board
}

/// Three independent controller ports: rail pins 3/7, return pins 5/8.
/// The added 12 mm branches join the existing rails 10 mm from CIN.
pub fn paired_board() -> PhysicalBoard {
    let mut board = board();
    let key = board.placements.iter().find(|p| p.reference == "U1").unwrap().footprint.clone();
    let pads = &mut board.footprints.get_mut(&key).unwrap().pads;
    for (original, number, y) in [("3", "7", -6_000_000), ("5", "8", 2_000_000)] {
        let mut pad = pads.iter().find(|p| p.number == original).unwrap().clone();
        pad.number = number.to_owned();
        pad.position = Point::new(0, y);
        pads.push(pad);
        let net =
            board.nets.iter_mut().find(|n| n.pads.iter().any(|p| p.component == "U1" && p.pad == original)).unwrap();
        net.pads.push(copper_pcb::board::PadReference::new("U1", number));
    }
    // A single return land represented by overlapping same-number primitives,
    // plus paste-only apertures. Its copper union is the original 0.2 mm square.
    let original = pads.iter_mut().find(|p| p.number == "5").unwrap();
    original.position.y_nm = -50_000;
    original.size.height_nm = 100_000;
    let mut upper = original.clone();
    upper.position.y_nm = 25_000;
    upper.size.height_nm = 150_000;
    pads.push(upper.clone());
    upper.kind = copper_physical::footprint::PadKind::Aperture;
    upper.has_solder_mask = false;
    upper.has_solder_paste = true;
    upper.number.clear();
    pads.push(upper);
    for (net, y, branch_y) in [("VIN", 30_000_000, 28_000_000), ("GND", 34_000_000, 36_000_000)] {
        for (a, b) in [((30_000_000, y), (30_000_000, branch_y)), ((30_000_000, branch_y), (40_000_000, branch_y))] {
            board.tracks.push(TrackSegment {
                net: net.to_owned(),
                start: Point::new(a.0, a.1),
                end: Point::new(b.0, b.1),
                width_nm: 200_000,
                layer: CopperLayer::Front,
            });
        }
    }
    board
}

/// A disposable source matching the paired physical fixture, exercising actual DRC
/// terminal discovery instead of inserting terminal groups in the report by hand.
pub fn paired_source(directory: &Path) -> std::path::PathBuf {
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/emi_buck_boost");
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    for entry in std::fs::read_dir(original.join("parts")).unwrap() {
        let entry = entry.unwrap();
        let mut source = std::fs::read_to_string(entry.path()).unwrap();
        if entry.file_name() == "buck_boost.copper" {
            source = source.replace("    pin FB", "    pin VIN2 { number = \"7\"; domains = \"power\"; directions = \"input\"; emi_role = hot_supply; }\n    pin PGND2 { number = \"8\"; domains = \"ground\"; directions = \"input\"; emi_role = hot_return; }\n    pin FB");
        }
        std::fs::write(directory.join("parts").join(entry.file_name()), source).unwrap();
    }
    let source = std::fs::read_to_string(original.join("board.copper"))
        .unwrap()
        .replace("net VIN { U1.VIN;", "net VIN { U1.VIN; U1.VIN2;")
        .replace("net GND { U1.PGND;", "net GND { U1.PGND; U1.PGND2;");
    let path = directory.join("board.copper");
    std::fs::write(&path, source).unwrap();
    path
}

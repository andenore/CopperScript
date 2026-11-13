//! The native zone fill in the region model (`RegionSpec::zone_fills`): a pour that a foreign track splits into two
//! pieces, one of them an island that the fill removes. The outline minus clearances keeps both; the real fill keeps
//! the piece that joins a via of its own net. Model build only, no solver run.

use copper_fullwave::fill::zone_fill_set;
use copper_fullwave::testing::*;
use copper_fullwave::{PortSpec, RegionModel, RegionSpec, Terminal};
use copper_pcb::board::{IslandPolicy, PadReference, PhysicalBoard, PhysicalNet, Placement};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

const MM: i64 = 1_000_000;

/// A footprint with one 1 x 1 mm GND pad.
fn gnd_pad_footprint() -> PhysicalFootprint {
    PhysicalFootprint {
        name: "G".to_owned(),
        pads: vec![FootprintPad {
            number: "1".to_owned(),
            position: Point::new(0, 0),
            size: Size { width_nm: MM, height_nm: MM },
            kind: PadKind::Smd,
            shape: PadShape::Rectangle,
            rotation_degrees: Decimal::zero(),
            drill: None,
            roundrect_ratio_ppm: 0,
            has_solder_mask: true,
            has_solder_paste: true,
            zone_connection: None,
            heatsink: false,
            remove_unused_layers: false,
            connector_contact: false,
        }],
        body_size: Size { width_nm: MM, height_nm: MM },
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Default::default(),
    }
}

/// A GND pour of 20 x 10 mm on F.Cu cut in two by a 0.4 mm track of net `S` across it; a GND pad (J1.1) in the left piece
/// joins it to the net; the right piece touches no pad of its own net.
fn split_pour() -> PhysicalBoard {
    let mut board = bare_board();
    two_layer(&mut board, 500_000, "4.3");
    board.footprints.insert("G".to_owned(), gnd_pad_footprint());
    board.placements = vec![Placement {
        reference: "J1".to_owned(),
        footprint: "G".to_owned(),
        position: Point::new(45 * MM, 50 * MM),
        rotation_degrees: Decimal::zero(),
        side: BoardSide::Front,
        value: String::new(),
        source_path: None,
    }];
    board.nets = vec![
        PhysicalNet { name: "S".to_owned(), pads: Vec::new() },
        PhysicalNet {
            name: "GND".to_owned(),
            pads: vec![PadReference { component: "J1".to_owned(), pad: "1".to_owned() }],
        },
    ];
    let f = CopperLayer::Front;
    let mut pour = zone("GND", &[f], [40 * MM, 45 * MM, 60 * MM, 55 * MM]);
    pour.island_policy = IslandPolicy::RemoveAll;
    pour.clearance_nm = Some(200_000);
    board.zones.push(pour);
    board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
    board.tracks.push(track("S", (50 * MM, 44 * MM), (50 * MM, 56 * MM), 400_000, f));
    board.tracks.push(track("S", (42 * MM, 40 * MM), (48 * MM, 40 * MM), 400_000, f));
    board
}

fn spec(board: &PhysicalBoard, native: bool) -> RegionSpec {
    let mut spec = RegionSpec::new([38 * MM, 38 * MM, 62 * MM, 58 * MM]);
    spec.margin_nm = 3 * MM;
    spec.cell_nm = Some(500_000);
    spec.ports = vec![PortSpec::to_plane(
        "p",
        Terminal::rect(CopperLayer::Front, [42 * MM, 40 * MM - 200_000, 42 * MM + 500_000, 40 * MM + 200_000]),
        CopperLayer::Back,
    )];
    if native {
        spec.zone_fills = Some(zone_fill_set(board).expect("the native fill"));
    }
    spec
}

fn front_zone_pieces(model: &RegionModel) -> Vec<(f64, f64)> {
    let mut pieces: Vec<(f64, f64)> = model
        .sheets
        .iter()
        .filter(|s| s.kind == "zone" && s.layer == CopperLayer::Front)
        .map(|s| {
            let xs: Vec<f64> = s.ring.iter().map(|p| p.0).collect();
            (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        })
        .collect();
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0));
    pieces
}

#[test]
fn the_native_fill_replaces_the_outline_and_drops_the_island_the_outline_keeps() {
    let board = split_pour();
    let outline = RegionModel::build(&board, &spec(&board, false)).expect("outline model");
    let native = RegionModel::build(&board, &spec(&board, true)).expect("native model");
    let (po, pn) = (front_zone_pieces(&outline), front_zone_pieces(&native));
    println!("outline pieces {po:?}\nnative pieces {pn:?}");
    println!("native flags {:?}, counts {:?}", native.flags, native.counts.zone_fills);
    assert!(
        outline.flags.iter().any(|f| f == "zone_outline_used") && !outline.flags.iter().any(|f| f == "zone_fill_used")
    );
    assert!(native.flags.iter().any(|f| f == "zone_fill_used"));
    // The outline pours both sides of the foreign track (clearance 0.2 mm each side of its 0.4 mm).
    assert!(po.iter().any(|p| p.1 <= 49.8) && po.iter().any(|p| p.0 >= 50.2), "{po:?}");
    // The fill removed the right piece (an island: no copper of its net) and kept the left one.
    assert!(pn.iter().any(|p| p.1 <= 49.8), "{pn:?}");
    assert!(!pn.iter().any(|p| p.0 >= 50.2), "the island must be gone: {pn:?}");
    // The plane on B.Cu has a fill too (no foreign copper on that layer).
    assert!(native.sheets.iter().any(|s| s.kind == "zone" && s.layer == CopperLayer::Back));
    assert!(native.counts.zone_fills >= 2);
}

#[test]
fn a_zone_without_a_fill_keeps_its_outline_and_says_so() {
    // A fill set that holds only the plane on B.Cu: the pour on F.Cu has none and stays the outline minus clearances.
    let board = split_pour();
    let mut s = spec(&board, true);
    let mut set = (**s.zone_fills.as_ref().expect("fills")).clone();
    set.fills.retain(|f| f.layer == CopperLayer::Back);
    s.zone_fills = Some(std::sync::Arc::new(set));
    let model = RegionModel::build(&board, &s).expect("model");
    assert!(model.flags.iter().any(|f| f == "zone_outline_used") && model.flags.iter().any(|f| f == "zone_fill_used"));
    assert_eq!(front_zone_pieces(&model).len(), 2, "both halves of the outline");
}

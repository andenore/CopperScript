//! The hot-loop region of a synthetic asynchronous buck: window, nets, port, bridges and added
//! traces, without running the solver.

use copper_fullwave::hotloop::{HotLoopInput, HotLoopOptions, LegInfo, hot_loop_spec};
use copper_fullwave::testing::{bare_board, zone};
use copper_fullwave::{Error, PortKind, RegionModel, Terminal};
use copper_pcb::board::{PadReference, PhysicalNet, Placement};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardSide, CopperLayer, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

const MM: i64 = 1_000_000;

fn pad(number: &str, x: i64, y: i64, w: i64, h: i64) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position: Point::new(x, y),
        size: Size { width_nm: w, height_nm: h },
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
    }
}

fn footprint(name: &str, pads: Vec<FootprintPad>) -> PhysicalFootprint {
    PhysicalFootprint {
        name: name.to_owned(),
        pads,
        body_size: Size { width_nm: 3 * MM, height_nm: 3 * MM },
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

fn place(reference: &str, footprint: &str, x: i64, y: i64) -> Placement {
    Placement {
        reference: reference.to_owned(),
        footprint: footprint.to_owned(),
        position: Point::new(x, y),
        rotation_degrees: Decimal::zero(),
        side: BoardSide::Front,
        value: String::new(),
        source_path: None,
    }
}

fn reference(component: &str, pad: &str) -> PadReference {
    PadReference { component: component.to_owned(), pad: pad.to_owned() }
}

/// U1 (VIN pad 1, SW pad 3 facing each other), CIN across VIN and GND, D1 across SW and GND, GND plane on In1.
fn buck_board() -> copper_pcb::board::PhysicalBoard {
    let mut board = bare_board();
    board.footprints.insert(
        "U".to_owned(),
        footprint("U", vec![pad("1", 0, -1_270_000, 600_000, 500_000), pad("3", 0, 1_270_000, 600_000, 500_000)]),
    );
    board.footprints.insert(
        "P".to_owned(),
        footprint("P", vec![pad("1", -700_000, 0, 600_000, 1_000_000), pad("2", 700_000, 0, 600_000, 1_000_000)]),
    );
    board.placements = vec![
        place("U1", "U", 30 * MM, 30 * MM),
        place("CIN", "P", 24 * MM, 28 * MM),
        place("D1", "P", 24 * MM, 34 * MM),
    ];
    board.nets = vec![
        PhysicalNet { name: "VIN".to_owned(), pads: vec![reference("U1", "1"), reference("CIN", "2")] },
        PhysicalNet { name: "SW".to_owned(), pads: vec![reference("U1", "3"), reference("D1", "2")] },
        PhysicalNet { name: "GND".to_owned(), pads: vec![reference("CIN", "1"), reference("D1", "1")] },
    ];
    board.zones.push(zone("GND", &[CopperLayer::In1], [0, 0, 100 * MM, 100 * MM]));
    copper_fullwave::testing::declare_stackup(
        &mut board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, 400_000, "4.3", "0.02"),
            (Some(CopperLayer::In1), 35_000, "", ""),
            (None, 1_000_000, "4.3", "0.02"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
    board
}

fn input() -> HotLoopInput {
    HotLoopInput {
        cell: "U1".to_owned(),
        topology: "buck".to_owned(),
        capacitor: "CIN".to_owned(),
        pads: vec![
            ("CIN".to_owned(), "2".to_owned()),
            ("U1".to_owned(), "1".to_owned()),
            ("U1".to_owned(), "3".to_owned()),
            ("D1".to_owned(), "2".to_owned()),
            ("D1".to_owned(), "1".to_owned()),
            ("CIN".to_owned(), "1".to_owned()),
        ],
        labels: [
            "CIN supply CIN.2",
            "hot supply U1.1",
            "switch U1.3",
            "D1 switch side D1.2",
            "D1 rail side D1.1",
            "CIN return CIN.1",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
        nets: vec!["VIN".to_owned(), "GND".to_owned(), "SW".to_owned()],
        closed_form_nh: Some(5.0),
        legs: Vec::new(),
        trace_status: "no_copper".to_owned(),
        polygon_mm: Vec::new(),
        switch_pad: None,
    }
}

#[test]
fn an_asynchronous_buck_gets_a_switch_port_two_bridges_and_three_straight_legs() {
    let board = buck_board();
    let (spec, notes) = hot_loop_spec(&board, &input(), &HotLoopOptions::default()).expect("spec");
    assert_eq!(spec.ports.len(), 1);
    match &spec.ports[0].kind {
        PortKind::Gap { from, to } => {
            assert_eq!(*from, Terminal::pad("U1", "1"));
            assert_eq!(*to, Terminal::pad("U1", "3"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(spec.bridges.len(), 2);
    assert_eq!(spec.extra_traces.len(), 3);
    assert_eq!(notes.len(), 3);
    // The nets: the loop's and the plane's.
    let nets = spec.nets.as_ref().expect("nets");
    assert!(nets.contains("VIN") && nets.contains("SW") && nets.contains("GND"));
    // The margin is at least ten dielectric heights (0.4 mm) and the default 5 mm.
    assert_eq!(spec.margin_nm, 5 * MM);
    let model = RegionModel::build(&board, &spec).expect("model");
    println!("{}", model.describe());
    assert_eq!(model.counts.bridges, 2);
    assert_eq!(model.counts.added_traces, 3);
    assert!(model.flags.iter().any(|f| f == "zone_outline_used"));
}

#[test]
fn routed_legs_get_no_straight_trace() {
    let board = buck_board();
    let mut input = input();
    input.legs =
        vec![LegInfo { from: input.labels[0].clone(), to: input.labels[1].clone(), kind: "traced".to_owned() }];
    let (spec, _) = hot_loop_spec(&board, &input, &HotLoopOptions::default()).expect("spec");
    assert_eq!(spec.extra_traces.len(), 2);
}

#[test]
fn other_topologies_and_odd_loops_are_refused_clearly() {
    let board = buck_board();
    let mut boost = input();
    boost.topology = "boost".to_owned();
    assert!(matches!(hot_loop_spec(&board, &boost, &HotLoopOptions::default()), Err(Error::NotSupported(_))));
    let mut odd = input();
    odd.pads.pop();
    odd.labels.pop();
    assert!(matches!(hot_loop_spec(&board, &odd, &HotLoopOptions::default()), Err(Error::NotSupported(_))));
}

/// U1 with VIN (pad 1) on the left, SW (pad 3) on the right and a row above, GND (pad 2) above VIN:
/// no pair of the three faces another across a free gap.
fn sync_board() -> copper_pcb::board::PhysicalBoard {
    let mut board = buck_board();
    board.footprints.insert(
        "Q".to_owned(),
        footprint(
            "Q",
            vec![
                pad("1", -1_500_000, 0, 600_000, 1_000_000),
                pad("2", -1_500_000, 2_000_000, 600_000, 1_000_000),
                pad("3", 1_500_000, 1_800_000, 600_000, 1_000_000),
            ],
        ),
    );
    board.placements = vec![place("U1", "Q", 30 * MM, 30 * MM), place("CIN", "P", 24 * MM, 28 * MM)];
    board.nets = vec![
        PhysicalNet { name: "VIN".to_owned(), pads: vec![reference("U1", "1"), reference("CIN", "2")] },
        PhysicalNet { name: "SW".to_owned(), pads: vec![reference("U1", "3")] },
        PhysicalNet { name: "GND".to_owned(), pads: vec![reference("U1", "2"), reference("CIN", "1")] },
    ];
    board
}

fn sync_input() -> HotLoopInput {
    HotLoopInput {
        cell: "U1".to_owned(),
        topology: "buck".to_owned(),
        capacitor: "CIN".to_owned(),
        pads: vec![
            ("CIN".to_owned(), "2".to_owned()),
            ("U1".to_owned(), "1".to_owned()),
            ("U1".to_owned(), "2".to_owned()),
            ("CIN".to_owned(), "1".to_owned()),
        ],
        labels: ["CIN supply CIN.2", "hot supply U1.1", "hot return U1.2", "CIN return CIN.1"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        nets: vec!["VIN".to_owned(), "GND".to_owned()],
        closed_form_nh: Some(5.0),
        legs: Vec::new(),
        trace_status: "no_copper".to_owned(),
        polygon_mm: Vec::new(),
        switch_pad: Some(("U1".to_owned(), "3".to_owned())),
    }
}

/// A synchronous cell with a known switch pad: the port is across the high-side switch (hot supply pad to
/// switch pad), the low-side switch is a bridge to the hot return pad, and the pads that do not face each
/// other are joined to the port by a strap.
#[test]
fn a_synchronous_buck_gets_a_strap_port_across_the_high_side_switch() {
    let board = sync_board();
    let (spec, _) = hot_loop_spec(&board, &sync_input(), &HotLoopOptions::default()).expect("spec");
    match &spec.ports[0].kind {
        PortKind::Gap { from, to } => {
            assert_eq!(*from, Terminal::pad("U1", "1"));
            assert_eq!(*to, Terminal::pad("U1", "3"));
        }
        other => panic!("{other:?}"),
    }
    let bridged: Vec<_> = spec.bridges.iter().map(|b| (b.a.label(), b.b.label())).collect();
    assert!(bridged.contains(&("U1.3".to_owned(), "U1.2".to_owned())), "{bridged:?}");
    assert_eq!(spec.bridges.len(), 2);
    let model = RegionModel::build(&board, &spec).expect("model");
    println!("{}", model.describe());
    assert!(!model.straps.is_empty(), "pads that overlap in neither x nor y need a strap");
    assert!(model.flags.iter().any(|f| f == "port_strap"));
    assert!(model.ports[0].description.contains("strap port"), "{}", model.ports[0].description);
    // The port box lies between the pads, on the strap, and every strap piece is on the pads' layer.
    let p = &model.ports[0];
    assert_eq!(p.start[2], p.stop[2]);
    let nh = copper_fullwave::hotloop::strap_inductance_nh(&board, &model).expect("a closed-form estimate");
    println!("strap inductance {nh:.3} nH");
    assert!(nh > 0.1 && nh < 10.0, "{nh}");
    // A compact loop is refined as a whole.
    assert_eq!(spec.refine.len(), 1, "{:?}", spec.refine);
    assert_eq!(spec.refine[0].name, "loop");
}

/// A large loop is refined around the port, the bridges and the pads only.
#[test]
fn a_large_loop_is_refined_around_its_port_bridges_and_pads() {
    let mut board = buck_board();
    board.placements = vec![
        place("U1", "U", 30 * MM, 30 * MM),
        place("CIN", "P", 10 * MM, 28 * MM),
        place("D1", "P", 10 * MM, 44 * MM),
    ];
    let (spec, _) = hot_loop_spec(&board, &input(), &HotLoopOptions::default()).expect("spec");
    let names: Vec<&str> = spec.refine.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"U1-switch port") && names.iter().any(|n| n.starts_with("pad ")), "{names:?}");
    assert!(!names.contains(&"loop"));
}

/// Without a switch pad the four-pad loop keeps the port across the hot supply and hot return pads.
#[test]
fn a_synchronous_buck_without_a_switch_pad_ports_across_supply_and_return() {
    let board = sync_board();
    let mut input = sync_input();
    input.switch_pad = None;
    let (spec, _) = hot_loop_spec(&board, &input, &HotLoopOptions::default()).expect("spec");
    match &spec.ports[0].kind {
        PortKind::Gap { from, to } => {
            assert_eq!(*from, Terminal::pad("U1", "1"));
            assert_eq!(*to, Terminal::pad("U1", "2"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(spec.bridges.len(), 1);
}

#[test]
fn facing_pads_with_a_free_gap_keep_the_direct_port_and_get_no_strap() {
    let board = buck_board();
    let (spec, _) = hot_loop_spec(&board, &input(), &HotLoopOptions::default()).expect("spec");
    let model = RegionModel::build(&board, &spec).expect("model");
    assert!(model.straps.is_empty());
    assert!(!model.flags.iter().any(|f| f == "port_strap"));
    assert!(!model.ports[0].description.contains("strap port"));
    // The refined mesh puts at least one line pair at the refined cell near the port.
    assert!(model.mesh.smallest[0] <= 0.26 && model.mesh.smallest[1] <= 0.26, "{:?}", model.mesh.smallest);
}

/// U1 as a QFN: VIN (pad 1) on the left, SW (pad 3) on the right, a GND exposed pad (pad 9) between them.
fn qfn_board() -> copper_pcb::board::PhysicalBoard {
    let mut board = sync_board();
    board.footprints.insert(
        "Q".to_owned(),
        footprint(
            "Q",
            vec![
                pad("1", -2_500_000, 0, 600_000, 1_000_000),
                pad("2", -2_500_000, 1_500_000, 600_000, 1_000_000),
                pad("3", 2_500_000, 1_100_000, 600_000, 1_000_000),
                pad("9", 0, 700_000, 3_200_000, 3_000_000),
            ],
        ),
    );
    board.nets = vec![
        PhysicalNet { name: "VIN".to_owned(), pads: vec![reference("U1", "1"), reference("CIN", "2")] },
        PhysicalNet { name: "SW".to_owned(), pads: vec![reference("U1", "3")] },
        PhysicalNet {
            name: "GND".to_owned(),
            pads: vec![reference("U1", "2"), reference("U1", "9"), reference("CIN", "1")],
        },
    ];
    board
}

/// Pads on opposite sides of an exposed pad: a strap on the layer would short the exposed pad to the
/// switch node, so the strap runs in the air on two posts, and the solver finds the port edges free.
#[test]
fn pads_on_both_sides_of_an_exposed_pad_get_an_air_bridge() {
    let board = qfn_board();
    let (spec, _) = hot_loop_spec(&board, &sync_input(), &HotLoopOptions::default()).expect("spec");
    let mut model = RegionModel::build(&board, &spec).expect("model");
    println!("{}", model.describe());
    assert!(model.flags.iter().any(|f| f == "port_strap_in_air"), "{:?}", model.flags);
    assert!(model.straps.iter().all(|s| s.in_air && s.z > model.stack.z(CopperLayer::Front).unwrap()));
    assert_eq!(model.wires.iter().filter(|w| w.kind == "post").count(), 2);
    assert!(model.ports[0].description.contains("air bridge"), "{}", model.ports[0].description);
    // The port box is above the copper: it is free of PEC, so the solver setup passes and only the budget stops the run.
    model.spec.budget.max_cell_updates = 1.0;
    assert!(matches!(model.solve_port(0), Err(Error::Budget(_))));
}

/// A pour on the layer of the pads is cut around the copper of other nets (with the zone clearance), so it
/// does not short the loop; its own net's pads stay connected.
#[test]
fn a_pour_on_the_pad_layer_is_cut_around_foreign_copper() {
    let mut board = buck_board();
    board.zones.clear();
    board.zones.push(zone("GND", &[CopperLayer::Front, CopperLayer::In1], [0, 0, 100 * MM, 100 * MM]));
    let (spec, _) = hot_loop_spec(&board, &input(), &HotLoopOptions::default()).expect("spec");
    let model = RegionModel::build(&board, &spec).expect("model");
    assert!(model.flags.iter().any(|f| f == "zone_cut_around_foreign_copper"), "{:?}", model.flags);
    assert!(!model.flags.iter().any(|f| f == "zone_overlaps_foreign_copper"), "{:?}", model.flags);
    let top = model.sheets.iter().find(|s| s.kind == "zone" && s.layer == CopperLayer::Front).expect("top pour");
    let plane = model.sheets.iter().find(|s| s.kind == "zone" && s.layer == CopperLayer::In1).expect("plane");
    assert!(top.ring.len() > plane.ring.len() + 8, "{} against {}", top.ring.len(), plane.ring.len());
}

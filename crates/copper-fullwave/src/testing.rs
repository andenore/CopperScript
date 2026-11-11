//! Synthetic boards for the tests: a stack-up, tracks, zones, vias and pads
//! built in code so the validation cases are exact and fast.

use copper_lang::mechanical::stackup::StackupLayer;
use copper_pcb::board::{CopperZone, IslandPolicy, PhysicalBoard, Stackup, TrackSegment, Via, ZoneConnection};
use copper_physical::{BoardOutline, CopperLayer, DesignRules, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

/// A board with only an outline and a two-layer stack-up; every other field is empty.
pub fn bare_board() -> PhysicalBoard {
    PhysicalBoard {
        name: "synthetic".to_owned(),
        outline: BoardOutline::polygon(vec![
            Point::new(0, 0),
            Point::new(100_000_000, 0),
            Point::new(100_000_000, 100_000_000),
            Point::new(0, 100_000_000),
        ])
        .expect("outline"),
        footprints: IndexMap::new(),
        placements: Vec::new(),
        nets: Vec::new(),
        stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
        rules: DesignRules::default(),
        tracks: Vec::new(),
        vias: Vec::new(),
        arcs: Vec::new(),
        metadata: IndexMap::new(),
        regions: Vec::new(),
        keepouts: Vec::new(),
        placement_rules: Vec::new(),
        relative_rules: Vec::new(),
        placement_groups: Vec::new(),
        power_domains: Vec::new(),
        net_routing_rules: Vec::new(),
        zones: Vec::new(),
        polygons: Vec::new(),
        copper_keepouts: Vec::new(),
        rigid_clusters: Vec::new(),
        hard_macros: Vec::new(),
        materialized_macros: Vec::new(),
        via_in_pad_rules: Vec::new(),
        mechanical_holes: Vec::new(),
        mechanical_slots: Vec::new(),
        datums: Vec::new(),
        boundary_edges: Vec::new(),
        attachments: Vec::new(),
        component_heights: Vec::new(),
        body_overhangs: Vec::new(),
        assembly_envelopes: Vec::new(),
        assembly_access: Vec::new(),
        match_groups: Vec::new(),
        component_hole_clearances: Vec::new(),
        decoupling_links: Vec::new(),
        silkscreen: Vec::new(),
        power_feeds: Vec::new(),
        mechanical_references: Vec::new(),
    }
}

fn decimal(text: &str) -> Decimal {
    Decimal::parse(text).expect("decimal")
}

/// A declared stack-up from `(copper layer or dielectric, thickness nm, er, loss tangent)`.
pub fn declare_stackup(board: &mut PhysicalBoard, layers: &[(Option<CopperLayer>, i64, &str, &str)]) {
    board.stackup.copper_layers = layers.iter().filter_map(|l| l.0).collect();
    board.stackup.physical_layers = layers
        .iter()
        .enumerate()
        .map(|(i, (copper, thickness, er, tan))| StackupLayer {
            id: format!("L{i}"),
            copper_layer: *copper,
            thickness_nm: *thickness,
            material: None,
            relative_permittivity: if copper.is_none() { Some(decimal(er)) } else { None },
            loss_tangent: if copper.is_none() { Some(decimal(tan)) } else { None },
            dielectric_type: None,
        })
        .collect();
    board.stackup.thickness_nm = layers.iter().map(|l| l.1).sum();
}

/// F.Cu, `height_nm` of dielectric `er` (loss tangent 0.02) and B.Cu as the plane.
pub fn two_layer(board: &mut PhysicalBoard, height_nm: i64, er: &str) {
    declare_stackup(
        board,
        &[
            (Some(CopperLayer::Front), 35_000, "", ""),
            (None, height_nm, er, "0.02"),
            (Some(CopperLayer::Back), 35_000, "", ""),
        ],
    );
}

pub fn track(net: &str, a: (i64, i64), b: (i64, i64), width_nm: i64, layer: CopperLayer) -> TrackSegment {
    TrackSegment { net: net.to_owned(), start: Point::new(a.0, a.1), end: Point::new(b.0, b.1), width_nm, layer }
}

/// A rectangular zone `[x0, y0, x1, y1]` (nm) on `layers`.
pub fn zone(net: &str, layers: &[CopperLayer], rect: [i64; 4]) -> CopperZone {
    CopperZone {
        id: format!("zone-{net}"),
        net: net.to_owned(),
        layers: layers.to_vec(),
        outer: vec![
            Point::new(rect[0], rect[1]),
            Point::new(rect[2], rect[1]),
            Point::new(rect[2], rect[3]),
            Point::new(rect[0], rect[3]),
        ],
        holes: Vec::new(),
        priority: 0,
        clearance_nm: Some(200_000),
        minimum_width_nm: 200_000,
        pad_connection: ZoneConnection::Solid,
        thermal_gap_nm: 300_000,
        thermal_spoke_width_nm: 300_000,
        island_policy: IslandPolicy::KeepAll,
        minimum_island_area_nm2: None,
        reserve_routing: false,
        allow_same_net_hard_macro_overlap: false,
    }
}

pub fn via(net: &str, at: (i64, i64), from: CopperLayer, to: CopperLayer) -> Via {
    Via {
        net: net.to_owned(),
        position: Point::new(at.0, at.1),
        size_nm: 600_000,
        drill_nm: 300_000,
        from_layer: from,
        to_layer: to,
        technology: None,
        finish: "standard".to_owned(),
    }
}

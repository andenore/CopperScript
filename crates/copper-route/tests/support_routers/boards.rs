//! Board builders over the board crates only (`copper-pcb`, `copper-physical`).
#![allow(dead_code)]

use copper_pcb::board::{
    CopperZone, IslandPolicy, NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, ReturnViaPolicy,
    RouteKind, Stackup, TrackSegment, TuningStyle, Via, ZoneConnection,
};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardOutline, CopperKeepout, CopperLayer, DesignRules, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

pub const MM: i64 = 1_000_000;

/// `nm_from_mm` of a decimal millimetre value.
pub fn mm(value: f64) -> i64 {
    (value * MM as f64).round() as i64
}

/// `Point.mm(x, y)`.
pub fn pt(x: f64, y: f64) -> Point {
    Point::new(mm(x), mm(y))
}

/// `FootprintPad(number, Point.mm(x, y), Size.mm(w, h))`: an SMD round-rect pad.
pub fn pad(number: &str, x: f64, y: f64, w: f64, h: f64) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position: pt(x, y),
        size: Size { width_nm: mm(w), height_nm: mm(h) },
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::from_i64(0),
        drill: None,
        roundrect_ratio_ppm: 250_000,
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    }
}

/// A circular SMD pad (a BGA ball land).
pub fn ball(number: &str, x: f64, y: f64, diameter: f64) -> FootprintPad {
    FootprintPad { shape: PadShape::Circle, ..pad(number, x, y, diameter, diameter) }
}

/// `PhysicalFootprint(name, pads, Size.mm(w, h))`.
pub fn footprint(name: &str, pads: Vec<FootprintPad>, w: f64, h: f64) -> PhysicalFootprint {
    PhysicalFootprint {
        name: name.to_owned(),
        pads,
        body_size: Size { width_nm: mm(w), height_nm: mm(h) },
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    }
}

/// `test/one-pad`: a single 0.6 mm pad on a 1 mm body.
pub fn one_pad() -> PhysicalFootprint {
    footprint("test/one-pad", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 1.0, 1.0)
}

/// An `n` x `n` ball array at `pitch` mm with `diameter` mm balls, numbered
/// row by row from 1 (top-left), centred on the origin.
pub fn ball_array(name: &str, n: i64, pitch: f64, diameter: f64) -> PhysicalFootprint {
    let half = (n - 1) as f64 / 2.0;
    let mut pads = Vec::new();
    for row in 0..n {
        for column in 0..n {
            let number = (row * n + column + 1).to_string();
            pads.push(ball(&number, (column as f64 - half) * pitch, (row as f64 - half) * pitch, diameter));
        }
    }
    footprint(name, pads, n as f64 * pitch, n as f64 * pitch)
}

pub fn placement(reference: &str, footprint: &str, x: f64, y: f64) -> Placement {
    Placement::new(reference, footprint, pt(x, y), &Decimal::from_i64(0)).expect("placement")
}

pub fn rotated(reference: &str, footprint: &str, x: f64, y: f64, degrees: i64) -> Placement {
    Placement::new(reference, footprint, pt(x, y), &Decimal::from_i64(degrees)).expect("placement")
}

pub fn net(name: &str, pads: &[(&str, &str)]) -> PhysicalNet {
    PhysicalNet { name: name.to_owned(), pads: pads.iter().map(|(c, p)| PadReference::new(c, p)).collect() }
}

/// `Stackup` with `count` copper layers (F.Cu, In1.Cu, ..., B.Cu).
pub fn layers(count: usize) -> Vec<CopperLayer> {
    let mut result = vec![CopperLayer::Front];
    for index in 1..count.saturating_sub(1) {
        result.push(CopperLayer::parse(&format!("In{index}.Cu")).expect("inner layer"));
    }
    if count > 1 {
        result.push(CopperLayer::Back);
    }
    result
}

/// `PhysicalBoard(name, BoardOutline.rectangle(w, h), footprints, placements, nets)`
/// with Python's defaults for everything else.
pub fn board(
    name: &str,
    width: f64,
    height: f64,
    footprints: Vec<PhysicalFootprint>,
    placements: Vec<Placement>,
    nets: Vec<PhysicalNet>,
) -> PhysicalBoard {
    PhysicalBoard {
        name: name.to_owned(),
        outline: BoardOutline::rectangle(mm(width), mm(height), None).expect("outline"),
        footprints: footprints.into_iter().map(|f| (f.name.clone(), f)).collect(),
        placements,
        nets,
        stackup: Stackup::new(layers(2)),
        rules: DesignRules::default(),
        tracks: Vec::new(),
        vias: Vec::new(),
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

/// The 0.15 mm clearance / 0.2 mm track / 0.6 mm via rules of the router tests.
pub fn fine_rules() -> DesignRules {
    DesignRules {
        minimum_clearance_nm: mm(0.15),
        minimum_track_width_nm: mm(0.15),
        default_track_width_nm: mm(0.2),
        default_via_size_nm: mm(0.6),
        default_via_drill_nm: mm(0.3),
        minimum_hole_clearance_nm: mm(0.25),
        ..DesignRules::default()
    }
}

/// `tests/test_unified_router.py::_board`: a 26 mm square board, one
/// `test/one-pad` part per terminal at `(x + 3, y + 3)` mm named `NET_i`.
pub fn pad_board(nets: &[(&str, &[(f64, f64)])], copper_layers: usize, tracks: Vec<TrackSegment>) -> PhysicalBoard {
    let footprint = one_pad();
    let mut placements = Vec::new();
    let mut physical = Vec::new();
    for (name, points) in nets {
        let mut pads = Vec::new();
        for (index, (x, y)) in points.iter().enumerate() {
            let reference = format!("{name}_{index}");
            placements.push(placement(&reference, &footprint.name, x + 3.0, y + 3.0));
            pads.push(PadReference::new(&reference, "1"));
        }
        physical.push(PhysicalNet { name: (*name).to_owned(), pads });
    }
    let mut result = board("Unified", 26.0, 26.0, vec![footprint], placements, physical);
    result.stackup = Stackup::new(layers(copper_layers));
    result.rules = fine_rules();
    result.tracks = tracks;
    result
}

/// Adds the footprints, placements and nets of `other` to `board`.
pub fn merge(mut board: PhysicalBoard, other: PhysicalBoard) -> PhysicalBoard {
    board.footprints.extend(other.footprints);
    board.placements.extend(other.placements);
    board.nets.extend(other.nets);
    board
}

pub fn track(net: &str, start: (f64, f64), end: (f64, f64), width: f64, layer: CopperLayer) -> TrackSegment {
    TrackSegment { net: net.to_owned(), start: pt(start.0, start.1), end: pt(end.0, end.1), width_nm: mm(width), layer }
}

pub fn via(net: &str, at: (f64, f64), size: f64, drill: f64, from: CopperLayer, to: CopperLayer) -> Via {
    Via {
        net: net.to_owned(),
        position: pt(at.0, at.1),
        size_nm: mm(size),
        drill_nm: mm(drill),
        from_layer: from,
        to_layer: to,
        technology: None,
        finish: "standard".to_owned(),
    }
}

/// `NetRoutingRule(net)` with Python's defaults.
pub fn rule(net: &str) -> NetRoutingRule {
    NetRoutingRule {
        net: net.to_owned(),
        kind: RouteKind::General,
        priority: 0,
        width_nm: None,
        clearance_nm: None,
        allowed_layers: Vec::new(),
        max_vias: None,
        max_length_nm: None,
        differential_partner: None,
        pair_gap_nm: None,
        max_skew_nm: None,
        topology: "point_to_point".to_owned(),
        target_impedance_ohms: None,
        maximum_uncoupled_length_nm: None,
        maximum_stub_length_nm: None,
        tuning_amplitude_limit_nm: None,
        require_return_vias: false,
        reserve_corridor: false,
        return_via_net: None,
        maximum_return_via_distance_nm: None,
        impedance_evidence_digest: None,
        return_via_policy: ReturnViaPolicy::Always,
        shared_reference_layer: None,
        target_single_ended_ohms: None,
        impedance_tolerance_percent: None,
        layer_group: None,
        breakout_length_nm: None,
        breakout_width_nm: None,
        breakout_gap_nm: None,
        breakout_clearance_nm: None,
        tuning_style: TuningStyle::Bumps,
        tuning_spacing_nm: None,
        tuning_group: None,
    }
}

pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    vec![pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)]
}

/// `CopperZone(id, net, layers, polygon)` with Python's defaults.
pub fn zone(id: &str, net: &str, layers: &[CopperLayer], outer: Vec<Point>) -> CopperZone {
    CopperZone {
        id: id.to_owned(),
        net: net.to_owned(),
        layers: layers.to_vec(),
        outer,
        holes: Vec::new(),
        priority: 0,
        clearance_nm: None,
        minimum_width_nm: mm(0.25),
        pad_connection: ZoneConnection::Thermal,
        thermal_gap_nm: mm(0.3),
        thermal_spoke_width_nm: mm(0.3),
        island_policy: IslandPolicy::RemoveBelowArea,
        minimum_island_area_nm2: Some(10_000_000_000_000),
        reserve_routing: false,
        allow_same_net_hard_macro_overlap: false,
    }
}

/// `CopperKeepout(id, layers, polygon)` with Python's defaults (tracks, vias and zones).
pub fn copper_keepout(id: &str, layers: &[CopperLayer], vertices: Vec<Point>) -> CopperKeepout {
    CopperKeepout {
        id: id.to_owned(),
        layers: layers.to_vec(),
        vertices,
        block_tracks: true,
        block_vias: true,
        block_pads: false,
        block_zones: true,
        block_footprints: false,
    }
}

/// `pcbir.bench.track_bends(tracks)["bends"]`: direction changes where
/// exactly two same-net tracks meet on one layer.
pub fn track_bends(tracks: &[TrackSegment]) -> usize {
    let mut far_ends: IndexMap<(String, CopperLayer, Point), Vec<Point>> = IndexMap::new();
    for track in tracks {
        if track.start != track.end {
            far_ends.entry((track.net.clone(), track.layer, track.start)).or_default().push(track.end);
            far_ends.entry((track.net.clone(), track.layer, track.end)).or_default().push(track.start);
        }
    }
    let mut bends = 0;
    for ((_, _, node), ends) in &far_ends {
        if ends.len() != 2 {
            continue;
        }
        let (ux, uy) = ((node.x_nm - ends[0].x_nm) as f64, (node.y_nm - ends[0].y_nm) as f64);
        let (vx, vy) = ((ends[1].x_nm - node.x_nm) as f64, (ends[1].y_nm - node.y_nm) as f64);
        if (ux * vy - uy * vx).abs().atan2(ux * vx + uy * vy).to_degrees() > 1.0 {
            bends += 1;
        }
    }
    bends
}

/// The summed Euclidean length of `net`'s tracks.
pub fn net_length(tracks: &[TrackSegment], net: &str) -> f64 {
    tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| ((t.end.x_nm - t.start.x_nm) as f64).hypot((t.end.y_nm - t.start.y_nm) as f64))
        .sum()
}

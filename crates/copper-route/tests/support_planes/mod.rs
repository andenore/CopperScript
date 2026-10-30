//! Small synthetic boards for the plane, stitching, fanout, via and tuning
//! behaviour tests ported from the former Python suite (`tests/test_*.py`
//! at d39067a). Each builder mirrors the Python dataclass defaults.

#![allow(dead_code)]

pub mod lanes;

use std::collections::BTreeSet;

use copper_pcb::board::{CopperZone, IslandPolicy, PadReference, PhysicalBoard, PhysicalNet, Placement, Stackup};
use copper_pcb::board::{TrackSegment, Via, ZoneConnection};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardOutline, CopperKeepout, CopperLayer, DesignRules, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

pub const MM: i64 = 1_000_000;

/// `nm_from_mm` of a decimal millimetre value, rounded to the nanometre.
pub fn nm(mm: f64) -> i64 {
    (mm * MM as f64).round() as i64
}

/// `Point.mm(x, y)`.
pub fn mm(x: f64, y: f64) -> Point {
    Point::new(nm(x), nm(y))
}

/// `Size.mm(width, height)`.
pub fn size(width: f64, height: f64) -> Size {
    Size { width_nm: nm(width), height_nm: nm(height) }
}

/// `FootprintPad(number, position, size)`: an SMD roundrect pad.
pub fn pad(number: &str, at: Point, dimensions: Size) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position: at,
        size: dimensions,
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

/// `PhysicalFootprint(name, pads, body_size)`.
pub fn footprint(name: &str, pads: Vec<FootprintPad>, body: Size) -> PhysicalFootprint {
    PhysicalFootprint {
        name: name.to_owned(),
        pads,
        body_size: body,
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

/// `Placement(reference, footprint, position)` at rotation 0 on the front.
pub fn place(reference: &str, footprint: &str, at: Point) -> Placement {
    Placement::new(reference, footprint, at, &Decimal::from_i64(0)).expect("placement")
}

/// `Placement(...)` with a rotation in whole degrees.
pub fn place_rotated(reference: &str, footprint: &str, at: Point, degrees: i64) -> Placement {
    Placement::new(reference, footprint, at, &Decimal::from_i64(degrees)).expect("placement")
}

pub fn pad_ref(component: &str, pad: &str) -> PadReference {
    PadReference::new(component, pad)
}

pub fn pads(items: &[(&str, &str)]) -> BTreeSet<PadReference> {
    items.iter().map(|(component, pad)| pad_ref(component, pad)).collect()
}

/// `PhysicalNet(name, pads)`.
pub fn net(name: &str, members: &[(&str, &str)]) -> PhysicalNet {
    PhysicalNet { name: name.to_owned(), pads: members.iter().map(|(c, p)| pad_ref(c, p)).collect() }
}

/// The axis-aligned ring from `(x0, y0)` to `(x1, y1)` in millimetres.
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
    vec![mm(x0, y0), mm(x1, y0), mm(x1, y1), mm(x0, y1)]
}

/// `CopperZone(id, net, layers, outline)` with the Python defaults.
pub fn zone(id: &str, net: &str, layers: &[CopperLayer], outer: Vec<Point>) -> CopperZone {
    CopperZone {
        id: id.to_owned(),
        net: net.to_owned(),
        layers: layers.to_vec(),
        outer,
        holes: Vec::new(),
        priority: 0,
        clearance_nm: None,
        minimum_width_nm: 250_000,
        pad_connection: ZoneConnection::Thermal,
        thermal_gap_nm: 300_000,
        thermal_spoke_width_nm: 300_000,
        island_policy: IslandPolicy::RemoveBelowArea,
        minimum_island_area_nm2: Some(10_000_000_000_000),
        reserve_routing: false,
        allow_same_net_hard_macro_overlap: false,
    }
}

/// `CopperKeepout(id, layers, outline)`: blocks everything by default.
pub fn keepout(id: &str, layers: &[CopperLayer], vertices: Vec<Point>) -> CopperKeepout {
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

/// A through via (`Via(net, position, size, drill, F.Cu, B.Cu)`).
pub fn via(net: &str, at: Point, size_nm: i64, drill_nm: i64) -> Via {
    Via {
        net: net.to_owned(),
        position: at,
        size_nm,
        drill_nm,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    }
}

pub fn track(net: &str, start: Point, end: Point, width_nm: i64, layer: CopperLayer) -> TrackSegment {
    TrackSegment { net: net.to_owned(), start, end, width_nm, layer }
}

pub fn four_layers() -> Stackup {
    Stackup::new(vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back])
}

/// An empty `PhysicalBoard(name, BoardOutline.rectangle(width, height))`
/// with a two-layer stackup and default rules.
pub fn board(name: &str, width_mm: f64, height_mm: f64) -> PhysicalBoard {
    PhysicalBoard {
        name: name.to_owned(),
        outline: BoardOutline::rectangle(nm(width_mm), nm(height_mm), None).expect("outline"),
        footprints: IndexMap::new(),
        placements: Vec::new(),
        nets: Vec::new(),
        stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
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

/// Adds (or replaces) a footprint definition.
pub fn add_footprint(board: &mut PhysicalBoard, definition: PhysicalFootprint) {
    board.footprints.insert(definition.name.clone(), definition);
}

/// The finding codes of `run_physical_drc(board)`.
pub fn drc_codes(board: &PhysicalBoard) -> BTreeSet<String> {
    copper_drc::run_physical_drc(board, copper_drc::Policy::default())
        .expect("drc")
        .findings
        .into_iter()
        .map(|finding| finding.code)
        .collect()
}

/// Asserts none of `codes` is reported by DRC.
pub fn assert_drc_clear(board: &PhysicalBoard, codes: &[&str]) {
    let found = drc_codes(board);
    let hit: Vec<&&str> = codes.iter().filter(|code| found.contains(**code)).collect();
    assert!(hit.is_empty(), "unexpected DRC findings {hit:?} (all: {found:?})");
}

/// `point in (track.start, track.end)` for some track.
pub fn touches(tracks: &[TrackSegment], point: Point) -> bool {
    tracks.iter().any(|track| track.start == point || track.end == point)
}

pub fn is_diagonal(track: &TrackSegment) -> bool {
    let dx = (track.end.x_nm - track.start.x_nm).abs();
    let dy = (track.end.y_nm - track.start.y_nm).abs();
    dx == dy && dx != 0
}

pub fn is_octilinear(track: &TrackSegment) -> bool {
    track.start.x_nm == track.end.x_nm || track.start.y_nm == track.end.y_nm || is_diagonal(track)
}

/// `NetRoutingRule(net)` with the Python defaults.
pub fn rule(net: &str) -> copper_pcb::board::NetRoutingRule {
    use copper_pcb::board::{ReturnViaPolicy, RouteKind, TuningStyle};
    copper_pcb::board::NetRoutingRule {
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

/// `document[key]` of a JSON object.
pub fn field<'a>(document: &'a copper_json::Value, key: &str) -> &'a copper_json::Value {
    document.as_object().and_then(|object| object.get(key)).unwrap_or_else(|| panic!("missing {key}"))
}

/// An integer JSON value, `None` for `null`.
pub fn int(value: &copper_json::Value) -> Option<i64> {
    match value {
        copper_json::Value::Int(number) => Some(i64::try_from(number).expect("i64")),
        copper_json::Value::Null => None,
        other => panic!("integer expected, got {other:?}"),
    }
}

pub fn text(value: &copper_json::Value) -> &str {
    value.as_str().expect("string")
}

/// Hard DRC errors besides the expected partial-routing state (`_hard`).
pub fn hard_findings(board: &PhysicalBoard) -> BTreeSet<String> {
    copper_drc::run_physical_drc(board, copper_drc::Policy::default())
        .expect("drc")
        .findings
        .into_iter()
        .filter(|finding| {
            finding.severity == copper_drc::Severity::Error
                && finding.code != "DRC-ROUTE-INCOMPLETE"
                && finding.code != "DRC-OPEN-NET"
        })
        .map(|finding| finding.code)
        .collect()
}

/// `route_global(board)` with the default options.
pub fn global(board: &PhysicalBoard) -> copper_route::GlobalRoutingResult {
    copper_route::route_global(board, &copper_route::GlobalRouterOptions::default(), None, None, None).expect("global")
}

/// The summed length of `net`'s tracks (`_track_length`).
pub fn net_length(tracks: &[TrackSegment], net: &str) -> i64 {
    let mine: Vec<&TrackSegment> = tracks.iter().filter(|track| track.net == net).collect();
    copper_route::critical::track_length(&mine)
}

/// A stand-in `kicad-cli` (KiCad 10.0.6) whose `pcb drc` writes `report`
/// to its `--output` and exits with `status`, as the Python tests' fake
/// runner did. Created under `directory`, which must be private to the test.
#[cfg(unix)]
pub fn fake_kicad_cli(directory: &std::path::Path, report: &str, status: i32) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(directory).expect("fake kicad directory");
    let report_path = directory.join("report.json");
    std::fs::write(&report_path, report).expect("fake report");
    let script = directory.join("kicad-cli");
    let body = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 10.0.6; exit 0; fi\n\
         if [ \"$1\" = pcb ] && [ \"$2\" = drc ]; then\n\
         while [ $# -gt 0 ]; do if [ \"$1\" = --output ]; then cp '{}' \"$2\"; fi; shift; done\n\
         [ {status} -ne 0 ] && echo failed >&2\nexit {status}\nfi\necho unexpected \"$@\" >&2\nexit 9\n",
        report_path.display()
    );
    std::fs::write(&script, body).expect("fake kicad-cli");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("executable");
    script
}

/// A fresh, empty directory for one test under Cargo's test scratch area.
pub fn scratch(name: &str) -> std::path::PathBuf {
    // Unit tests (which include this module by path) have no CARGO_TARGET_TMPDIR.
    let root = option_env!("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let directory = root.join("support_planes").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

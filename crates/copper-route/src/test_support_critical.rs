//! Board builders shared by the critical-routing and differential-pair unit
//! tests ported from the former Python suite (`tests/test_local_critical.py`
//! and its neighbours at d39067a).

use indexmap::IndexMap;

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, RouteKind, TrackSegment};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardOutline, CopperKeepout, CopperLayer, Point};
use copper_units::Decimal;

use crate::routing::{GlobalRouteSegment, GlobalRouterOptions, GlobalRoutingResult};

/// Nanometres in one millimetre.
pub(crate) const MM: i64 = 1_000_000;

/// `Point.mm(x, y)` for whole or fractional millimetres given in nanometres.
pub(crate) fn at(x_nm: i64, y_nm: i64) -> Point {
    Point::new(x_nm, y_nm)
}

/// `Point.mm(x, y)` for whole millimetres.
pub(crate) fn mm(x: i64, y: i64) -> Point {
    Point::new(x * MM, y * MM)
}

/// `FootprintPad(number, position, Size(size, size))`: a square SMD pad.
pub(crate) fn smd_pad(number: &str, position: Point, size_nm: i64) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position,
        size: Size { width_nm: size_nm, height_nm: size_nm },
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::zero(),
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

/// `PhysicalFootprint(name, pads, body)` with the dataclass defaults.
pub(crate) fn footprint(name: &str, pads: Vec<FootprintPad>, body: Size) -> PhysicalFootprint {
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

/// `Placement(reference, footprint, position)`.
pub(crate) fn pose(reference: &str, footprint: &str, position: Point) -> Placement {
    Placement::new(reference, footprint, position, &Decimal::zero()).expect("a valid pose")
}

/// `PhysicalNet(name, (PadReference(reference, pad), ...))`.
pub(crate) fn net(name: &str, pads: &[(&str, &str)]) -> PhysicalNet {
    PhysicalNet {
        name: name.to_owned(),
        pads: pads.iter().map(|(component, pad)| PadReference::new(component, pad)).collect(),
    }
}

/// `NetRoutingRule(net, kind)` with the remaining dataclass defaults.
pub(crate) fn rule(net: &str, kind: RouteKind) -> NetRoutingRule {
    NetRoutingRule { kind, ..crate::routing_acceptance::general_rule(net) }
}

/// `PhysicalBoard(name, BoardOutline.rectangle(w, h), footprints, placements, nets)`.
pub(crate) fn board(
    name: &str,
    width_mm: i64,
    height_mm: i64,
    footprints: Vec<PhysicalFootprint>,
    placements: Vec<Placement>,
    nets: Vec<PhysicalNet>,
) -> PhysicalBoard {
    crate::routing_acceptance::board(
        name,
        outline(width_mm * MM, height_mm * MM),
        footprints.into_iter().map(|item| (item.name.clone(), item)).collect(),
        placements,
        nets,
    )
}

/// `BoardOutline.rectangle(width, height)` in nanometres.
pub(crate) fn outline(width_nm: i64, height_nm: i64) -> BoardOutline {
    BoardOutline::rectangle(width_nm, height_nm, None).expect("outline")
}

/// A track-blocking `CopperKeepout` with the dataclass defaults.
pub(crate) fn keepout(id: &str, layers: &[CopperLayer], vertices: Vec<Point>) -> CopperKeepout {
    crate::routing_acceptance::keepout(id, layers.to_vec(), vertices)
}

/// The axis-aligned rectangle `(x0, y0)`-`(x1, y1)` in nanometres.
pub(crate) fn rectangle(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Point> {
    vec![at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)]
}

/// A front-layer track-blocking keepout over a millimetre rectangle.
pub(crate) fn front_wall(id: &str, x0: i64, y0: i64, x1: i64, y1: i64) -> CopperKeepout {
    keepout(id, &[CopperLayer::Front], rectangle(x0, y0, x1, y1))
}

/// `TrackSegment(net, start, end, width, layer)`.
pub(crate) fn track(net: &str, start: Point, end: Point, width_nm: i64, layer: CopperLayer) -> TrackSegment {
    TrackSegment { net: net.to_owned(), start, end, width_nm, layer }
}

/// `route_global(board, GlobalRouterOptions(tile_size_nm=tile))`.
pub(crate) fn global_route(board: &PhysicalBoard, tile_size_nm: i64) -> GlobalRoutingResult {
    let options = GlobalRouterOptions { tile_size_nm, ..GlobalRouterOptions::default() };
    crate::routing::route_global(board, &options, None, None, None).expect("global route")
}

/// `GlobalRouteSegment(net, layer, a, b, half_width, resource)` along a polyline.
pub(crate) fn guide_segments(net: &str, layer: CopperLayer, vertices: &[Point], resource: &str) -> Vec<GlobalRouteSegment> {
    vertices
        .windows(2)
        .map(|pair| GlobalRouteSegment {
            net: net.to_owned(),
            layer,
            start: pair[0],
            end: pair[1],
            guide_half_width_nm: MM,
            resource_id: resource.to_owned(),
        })
        .collect()
}

/// The guide for `net` replaced by explicit segments, its accesses reset to
/// their pads and stripped of vias (the synthetic guides of the Python tests).
pub(crate) fn synthetic_guide(
    mut global: GlobalRoutingResult,
    net: &str,
    segments: Vec<GlobalRouteSegment>,
) -> GlobalRoutingResult {
    for route in global.routes.iter_mut().filter(|route| route.net == net) {
        route.segments = segments.clone();
        route.vias.clear();
        for access in &mut route.accesses {
            access.access_position = access.pad_position;
            access.tracks.clear();
            access.via = None;
        }
    }
    global
}

/// `run_physical_drc(board).findings` less incomplete-route findings and the
/// routing-budget warnings (connected copper over a length budget is a
/// review warning reported separately).
pub(crate) fn hard_findings(board: &PhysicalBoard) -> Vec<String> {
    const BUDGET: [&str; 3] = ["DRC-MAX-LENGTH", "DRC-DIFF-SKEW", "DRC-LENGTH-MATCH"];
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("physical DRC");
    report
        .report
        .findings
        .iter()
        .filter(|f| f.code != "DRC-ROUTE-INCOMPLETE" && !BUDGET.contains(&f.code.as_str()))
        .map(|f| format!("{}: {}", f.code, f.message))
        .collect()
}

/// Whether the explicit copper of `board` joins every pad of `net`.
pub(crate) fn net_connected(board: &PhysicalBoard, net: &str) -> bool {
    let connectivity =
        copper_drc::connectivity::explicit_copper_connectivity(board, copper_drc::connectivity::Candidate::default())
            .expect("connectivity");
    let net = board.nets.iter().find(|item| item.name == net).expect("a declared net");
    connectivity.net_connected(net)
}

/// Whether every track is horizontal, vertical or diagonal.
pub(crate) fn octilinear(tracks: &[TrackSegment]) -> bool {
    tracks.iter().all(|t| {
        t.start.x_nm == t.end.x_nm
            || t.start.y_nm == t.end.y_nm
            || (t.start.x_nm - t.end.x_nm).abs() == (t.start.y_nm - t.end.y_nm).abs()
    })
}

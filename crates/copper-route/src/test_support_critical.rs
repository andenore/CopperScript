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
pub(crate) fn guide_segments(
    net: &str,
    layer: CopperLayer,
    vertices: &[Point],
    resource: &str,
) -> Vec<GlobalRouteSegment> {
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

/// `NetRoutingRule(net, DIFFERENTIAL, partner, width, gap)`.
pub(crate) fn differential_rule(net: &str, partner: &str, width_nm: i64, gap_nm: i64) -> NetRoutingRule {
    NetRoutingRule {
        width_nm: Some(width_nm),
        differential_partner: Some(partner.to_owned()),
        pair_gap_nm: Some(gap_nm),
        ..rule(net, RouteKind::Differential)
    }
}

/// `tests/test_critical_routing.py::_pair_board`: a USB pair between two
/// two-pad terminals J1 (5, 12) and J2 (35, 12) on a 40 x 25 mm board.
pub(crate) fn usb_pair_board() -> PhysicalBoard {
    let pads = vec![smd_pad("1", at(0, -500_000), 400_000), smd_pad("2", at(0, 500_000), 400_000)];
    let pair = footprint("test/two-pad-pair", pads, Size { width_nm: MM, height_nm: 2 * MM });
    let poses = vec![pose("J1", &pair.name, mm(5, 12)), pose("J2", &pair.name, mm(35, 12))];
    let nets = vec![net("USB_DP", &[("J1", "1"), ("J2", "1")]), net("USB_DM", &[("J1", "2"), ("J2", "2")])];
    let mut board = board("CriticalPair", 40, 25, vec![pair], poses, nets);
    board.net_routing_rules = [("USB_DP", "USB_DM"), ("USB_DM", "USB_DP")]
        .into_iter()
        .map(|(net, partner)| NetRoutingRule {
            priority: 100,
            max_skew_nm: Some(10 * MM),
            maximum_uncoupled_length_nm: Some(10 * MM),
            target_impedance_ohms: Some(90),
            ..differential_rule(net, partner, 250_000, 200_000)
        })
        .collect();
    board
}

/// The codes of every physical DRC finding of `board`.
pub(crate) fn finding_codes(board: &PhysicalBoard) -> indexmap::IndexSet<String> {
    let report = crate::drc_report::run_physical_drc(board, copper_drc::Policy::default()).expect("physical DRC");
    report.report.findings.iter().map(|f| f.code.clone()).collect()
}

/// Whether `board` has none of the given DRC codes.
pub(crate) fn free_of(board: &PhysicalBoard, codes: &[&str]) -> bool {
    let found = finding_codes(board);
    !codes.iter().any(|code| found.contains(*code))
}

/// The guides of `global` by net, as `_route_pair` takes them.
pub(crate) fn guides_by_net(global: &GlobalRoutingResult) -> IndexMap<String, crate::routing::GlobalNetRoute> {
    global.routes.iter().map(|route| (route.net.clone(), route.clone())).collect()
}

/// `GlobalViaProposal(net, position, from, to, "test-via")`.
pub(crate) fn via_proposal(
    net: &str,
    position: Point,
    from: CopperLayer,
    to: CopperLayer,
) -> crate::routing::GlobalViaProposal {
    crate::routing::GlobalViaProposal {
        net: net.to_owned(),
        position,
        from_layer: from,
        to_layer: to,
        resource_id: "test-via".to_owned(),
    }
}

/// Fixed-point millimetres (`"1.4"`) in nanometres.
pub(crate) fn nm(text: &str) -> i64 {
    copper_pcb::nm_from_mm_text(text).expect("millimetres")
}

/// `_rows(centres, pitch)`: `(P, N)` land offsets around each pair centre.
pub(crate) fn rows(centres: &[&str], pitch: &str) -> Vec<(i64, i64)> {
    let half = nm(pitch) / 2;
    centres.iter().map(|centre| (nm(centre) - half, nm(centre) + half)).collect()
}

/// `CopperZone(id, net, layers, outline)` with the dataclass defaults.
pub(crate) fn zone(id: &str, net: &str, layers: &[CopperLayer], outer: Vec<Point>) -> copper_pcb::board::CopperZone {
    copper_pcb::board::CopperZone {
        id: id.to_owned(),
        net: net.to_owned(),
        layers: layers.to_vec(),
        outer,
        holes: Vec::new(),
        priority: 0,
        clearance_nm: None,
        minimum_width_nm: 250_000,
        pad_connection: copper_physical::footprint::ZoneConnection::Thermal,
        thermal_gap_nm: 300_000,
        thermal_spoke_width_nm: 300_000,
        island_policy: copper_pcb::board::IslandPolicy::RemoveBelowArea,
        minimum_island_area_nm2: Some(10_000_000_000_000),
        reserve_routing: false,
        allow_same_net_hard_macro_overlap: false,
    }
}

/// `_bundle_board` / `_crossing_board`: a package row (U1 at (8, 10), lands
/// facing right at x = 9.4 mm) and a connector row (J1 at (22, 10), lands
/// facing left at x = 20.6 mm) joined by one pair `NAME_P`/`NAME_N` per lane
/// on a 30 x 20 mm board with 0.1 mm rules. `profile(net, partner)` gives
/// each member's rule.
pub(crate) fn row_pair_board(
    name: &str,
    package_rows: &[(i64, i64)],
    connector_rows: &[(i64, i64)],
    names: &[&str],
    profile: impl Fn(&str, &str, &str) -> NetRoutingRule,
) -> PhysicalBoard {
    let (mut package_pads, mut connector_pads, mut nets, mut rules) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let land = |number: &str, x: i64, y: i64, height: i64| FootprintPad {
        size: Size { width_nm: 600_000, height_nm: height },
        ..smd_pad(number, at(x, y), 600_000)
    };
    for (index, ((up, un), ((jp, jn), lane))) in package_rows.iter().zip(connector_rows.iter().zip(names)).enumerate() {
        let (p, n) = ((2 * index + 1).to_string(), (2 * index + 2).to_string());
        package_pads.extend([land(&p, 1_400_000, *up, 250_000), land(&n, 1_400_000, *un, 250_000)]);
        connector_pads.extend([land(&p, -1_400_000, *jp, 200_000), land(&n, -1_400_000, *jn, 200_000)]);
        let (positive, negative) = (format!("{lane}_P"), format!("{lane}_N"));
        nets.push(net(&positive, &[("U1", &p), ("J1", &p)]));
        nets.push(net(&negative, &[("U1", &n), ("J1", &n)]));
        rules.push(profile(lane, &positive, &negative));
        rules.push(profile(lane, &negative, &positive));
    }
    let body = Size { width_nm: 3 * MM, height_nm: 8 * MM };
    let package = footprint("test/package-row", package_pads, body);
    let connector = footprint("test/connector-row", connector_pads, body);
    let poses = vec![pose("U1", &package.name, mm(8, 10)), pose("J1", &connector.name, mm(22, 10))];
    let mut board = board(name, 30, 20, vec![package, connector], poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    board.net_routing_rules = rules;
    board
}

/// `Via(net, position, size, drill)`: a standard F.Cu-B.Cu through via.
pub(crate) fn through_via(net: &str, position: Point, size_nm: i64, drill_nm: i64) -> copper_pcb::board::Via {
    copper_pcb::board::Via {
        net: net.to_owned(),
        position,
        size_nm,
        drill_nm,
        from_layer: CopperLayer::Front,
        to_layer: CopperLayer::Back,
        technology: None,
        finish: "standard".to_owned(),
    }
}

/// The joint searches' default bounds (`maximum_searches=8`,
/// `maximum_states=30_000`, `pitch_nm=1 mm`, no aggregate cap).
pub(crate) fn search_bounds() -> crate::pair_search::SearchBounds {
    crate::pair_search::SearchBounds {
        maximum_searches: 8,
        maximum_states: 30_000,
        pitch_nm: MM,
        maximum_total_states: None,
    }
}

/// Every candidate of `paired_via_candidates` (`via = true`) or
/// `paired_candidates`, at most `limit`.
pub(crate) fn pair_candidates(
    board: &PhysicalBoard,
    routes: &IndexMap<String, crate::routing::GlobalNetRoute>,
    (first, second): (&str, &str),
    via: bool,
    bounds: crate::pair_search::SearchBounds,
    stats: &mut crate::pair_search::PairSearchStats,
    limit: usize,
) -> crate::Result<Vec<crate::pair_search::PairSearchCandidate>> {
    let rule = |net: &str| board.net_routing_rules.iter().find(|r| r.net == net).expect("a pair rule").clone();
    let (first_rule, second_rule) = (rule(first), rule(second));
    let mut found = Vec::new();
    let mut accept = |candidate, _: &mut crate::pair_search::PairSearchStats| {
        found.push(candidate);
        Ok(found.len() >= limit)
    };
    if via {
        crate::pair_vias::paired_via_candidates(
            board,
            &first_rule,
            &second_rule,
            &routes[first],
            &routes[second],
            bounds,
            crate::pair_vias::ViaRestriction::default(),
            stats,
            &mut accept,
        )?;
    } else {
        crate::pair_search::paired_candidates(
            board,
            &first_rule,
            &second_rule,
            &routes[first],
            &routes[second],
            bounds,
            None,
            stats,
            &mut accept,
        )?;
    }
    Ok(found)
}

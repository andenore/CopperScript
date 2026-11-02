//! Plan R1 for ordinary nets: wide copper necks down next to fine-pitch
//! lands; ported from `tests/test_ordinary_neck_down.py`. The stub-pruning
//! case is not ported (`prune_track_stubs` has no Rust port, see
//! `route_cleanup`).

mod support_planes;

use std::collections::BTreeSet;

use copper_drc::breakout::BreakoutRegions;
use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::detailed::DetailedRoutingResult;
use copper_route::pin_escape::checked_access_path;
use copper_route::route_smoothing::smooth_owned_tracks;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use support_planes::*;

const FRONT: CopperLayer = CopperLayer::Front;
const WIDE: i64 = 400_000;
const NARROW: i64 = 200_000;

/// `_board`: a five-pin 0.4 mm-pitch row (U1) and a large land (J1).
///
/// The 0.25 mm lands leave 0.15 mm between them, so a 0.4 mm track on U1.3
/// comes within 0.075 mm of U1.2 and U1.4; the board clearance is 0.1 mm.
/// VDD is 0.4 mm wide with, by default, a 0.2 mm breakout width 1 mm from
/// its lands. `rotated` adds a second row (U2) turned by 90 degrees.
fn neck_board(breakout: bool, rotated: bool, nets: &[(&str, &[(&str, &str)])]) -> PhysicalBoard {
    let mut board = board("OrdinaryNeckDown", 20.0, 20.0);
    let row = (0..5).map(|i| pad(&(i + 1).to_string(), Point::new(400_000 * (i - 2), 0), size(0.25, 0.6))).collect();
    add_footprint(&mut board, footprint("test/fine-row", row, size(3.0, 2.0)));
    add_footprint(&mut board, footprint("test/land", vec![pad("1", Point::new(0, 0), size(1.2, 1.2))], size(2.0, 2.0)));
    board.placements = if rotated {
        vec![
            place("U1", "test/fine-row", mm(6.0, 6.0)),
            place("J1", "test/land", mm(15.0, 6.0)),
            place_rotated("U2", "test/fine-row", mm(12.0, 13.0), 90),
            place("J2", "test/land", mm(4.0, 15.0)),
        ]
    } else {
        vec![place("U1", "test/fine-row", mm(10.0, 8.0)), place("J1", "test/land", mm(14.0, 14.0))]
    };
    board.nets = nets.iter().map(|(name, members)| net(name, members)).collect();
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    let mut vdd = NetRoutingRule { width_nm: Some(WIDE), ..rule("VDD") };
    if breakout {
        vdd.breakout_length_nm = Some(1_000_000);
        vdd.breakout_width_nm = Some(NARROW);
    }
    board.net_routing_rules = vec![vdd];
    board
}

const VDD: &[(&str, &[(&str, &str)])] = &[("VDD", &[("U1", "3"), ("J1", "1")])];

fn route(board: &PhysicalBoard, options: &UnifiedRouterOptions) -> DetailedRoutingResult {
    route_unified(board, &global(board), options, None, None, None, None, None).unwrap()
}

/// VDD copper below its permitted minimum; full width is legal inside too.
fn misnecked(board: &PhysicalBoard, tracks: &[TrackSegment]) -> Vec<TrackSegment> {
    let regions = BreakoutRegions::new(board);
    tracks
        .iter()
        .filter(|t| {
            t.net == "VDD"
                && t.width_nm < if regions.region(&t.net, &[t.start, t.end]).is_some() { NARROW } else { WIDE }
        })
        .cloned()
        .collect()
}

fn hard_errors(board: &PhysicalBoard) -> BTreeSet<String> {
    copper_drc::run_physical_drc(board, copper_drc::Policy::default())
        .unwrap()
        .findings
        .into_iter()
        .filter(|f| f.severity == copper_drc::Severity::Error && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| f.code)
        .collect()
}

fn connected(board: &PhysicalBoard) -> bool {
    let graph = explicit_copper_connectivity(board, Candidate::default()).unwrap();
    board.nets.iter().all(|net| graph.net_connected(net))
}

fn with_tracks(board: &PhysicalBoard, tracks: &[TrackSegment]) -> PhysicalBoard {
    let mut copy = board.clone();
    copy.tracks = tracks.to_vec();
    copy
}

#[test]
fn a_wide_ordinary_net_reaches_a_fine_pitch_pin_only_by_necking_down() {
    // At its full 0.4 mm no copper can touch U1.3: there is no pin access.
    let plain = neck_board(false, false, VDD);
    let guide = global(&plain);
    assert!(!guide.routes[0].connected);
    assert!(
        guide.routes[0].diagnostics.iter().any(|d| d == "no legal bounded local/global access for U1.3"),
        "{:?}",
        guide.routes[0].diagnostics
    );
    assert!(!route(&plain, &UnifiedRouterOptions::default()).nets[0].connected);

    let board = neck_board(true, false, VDD);
    let routed = route(&board, &UnifiedRouterOptions::default());
    assert!(routed.nets[0].connected, "{:?}", routed.nets[0].diagnostics);
    let tracks = &routed.board.tracks;
    assert_eq!(tracks.iter().map(|t| t.width_nm).collect::<BTreeSet<_>>(), [NARROW, WIDE].into_iter().collect());
    assert!(misnecked(&board, tracks).is_empty());
    let regions = BreakoutRegions::new(&board);
    let held: BTreeSet<Option<String>> = tracks.iter().map(|t| regions.region("VDD", &[t.start, t.end])).collect();
    let expected: BTreeSet<Option<String>> =
        [Some("pad:U1.3:2".to_owned()), Some("pad:J1.1:0".to_owned()), None].into_iter().collect();
    assert_eq!(held, expected);
    let report = copper_drc::run_physical_drc(&routed.board, copper_drc::Policy::default()).unwrap();
    let codes: BTreeSet<&str> = report.findings.iter().map(|f| f.code.as_str()).collect();
    for code in ["DRC-CLEARANCE", "DRC-SHORT", "DRC-TRACK-WIDTH", "DRC-OPEN-NET"] {
        assert!(!codes.contains(code), "{code} in {codes:?}");
    }
    let necked: Vec<_> = report.breakout_relaxations.iter().filter(|item| item.check == "track_width").collect();
    assert!(!necked.is_empty());
    assert!(
        necked.iter().all(|item| (item.required_nm, item.relaxed_nm, item.measured_nm) == (WIDE, NARROW, Some(NARROW)))
    );
    // The large J1 land no longer gets a gratuitous neckdown.
    let lands: BTreeSet<&str> = necked.iter().flat_map(|item| item.regions.iter().map(|r| r.1.as_str())).collect();
    assert_eq!(lands, ["pad:U1.3:2"].into_iter().collect());
}

#[test]
fn routing_checks_and_emits_each_centreline_as_its_pieces() {
    let board = neck_board(true, false, VDD);
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let (start, end) = (mm(10.0, 8.0), mm(10.0, 11.0));
    assert!(!index.can_track("VDD", start, end, WIDE, FRONT).unwrap());
    assert!(index.can_route("VDD", start, end, WIDE, FRONT).unwrap());
    let pieces = index.route_pieces("VDD", start, end, WIDE, FRONT).unwrap();
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    let (inside, outside) = (&pieces[0], &pieces[1]);
    assert_eq!((inside.start, inside.width_nm, outside.end, outside.width_nm), (start, NARROW, end, WIDE));
    assert_eq!(inside.end, outside.start);
    assert!(BreakoutRegions::new(&board).region("VDD", &[inside.start, inside.end]).is_some());
    // Pad access paths are the same pieces, with movable conflicts too.
    let access = checked_access_path(&board, &index, "VDD", start, end, WIDE, FRONT, false, true, None).unwrap();
    assert_eq!(access, Some(pieces.clone()));
    let blockers = index.route_blockers("VDD", start, end, WIDE, FRONT).unwrap();
    assert!(blockers.0.is_empty() && !blockers.1, "{blockers:?}");
    // Global pin access offers the necked lead-in.
    let accesses = &global(&board).routes[0].accesses;
    assert!(accesses.iter().any(|access| access.tracks.iter().any(|t| t.width_nm == NARROW)));
    // A net without breakout properties is one track of its width.
    let plain_board = neck_board(false, false, VDD);
    let plain = RoutingClearanceIndex::new(&plain_board).unwrap();
    assert!(plain.breakout.ordinary.is_empty());
    assert_eq!(plain.route_pieces("VDD", start, end, WIDE, FRONT).unwrap(), [track("VDD", start, end, WIDE, FRONT)]);
    assert_eq!(
        plain.can_route("VDD", start, end, WIDE, FRONT).unwrap(),
        plain.can_track("VDD", start, end, WIDE, FRONT).unwrap()
    );
    // A critical net with breakout properties keeps the critical router's own cut.
    let mut critical = board.clone();
    critical.net_routing_rules.iter_mut().for_each(|r| r.kind = RouteKind::Clock);
    let regions = BreakoutRegions::new(&critical);
    assert_eq!(regions.declared.iter().collect::<Vec<_>>(), ["VDD"]);
    assert!(regions.ordinary.is_empty());
}

#[test]
fn smoothing_a_necked_staircase_checks_and_emits_its_pieces() {
    let board = neck_board(true, false, VDD);
    let regions = BreakoutRegions::new(&board);
    let corners = [mm(10.0, 8.0), mm(10.0, 8.6), mm(10.4, 8.6), mm(10.4, 11.0), mm(14.0, 11.0), mm(14.0, 14.0)];
    let staircase: Vec<TrackSegment> = corners.windows(2).map(|p| track("VDD", p[0], p[1], WIDE, FRONT)).collect();
    let owned = regions.split_tracks(&staircase, Some(WIDE));
    assert!(misnecked(&board, &owned).is_empty());
    assert!(hard_errors(&with_tracks(&board, &owned)).is_empty());
    let owned_board = with_tracks(&board, &owned);
    let mut index = RoutingClearanceIndex::new(&owned_board).unwrap();
    let result = smooth_owned_tracks(&board, &owned, &mut index, 2).unwrap();
    let mut sorted_result = result.clone();
    let mut sorted_owned = owned.clone();
    let key = |t: &TrackSegment| (t.start.x_nm, t.start.y_nm, t.end.x_nm, t.end.y_nm, t.width_nm);
    sorted_result.sort_by_key(key);
    sorted_owned.sort_by_key(key);
    assert_ne!(sorted_result, sorted_owned);
    let final_board = with_tracks(&board, &result);
    assert!(misnecked(&board, &result).is_empty());
    assert_eq!(result.iter().map(|t| t.width_nm).collect::<BTreeSet<_>>(), [NARROW, WIDE].into_iter().collect());
    let errors = hard_errors(&final_board);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(connected(&final_board));
    // The staircase becomes a checked chord. Its open-space pieces may now
    // use the full width; only genuinely obstructed pin access stays necked.
    assert!(result.iter().any(|t| t.start.x_nm != t.end.x_nm && t.start.y_nm != t.end.y_nm));
    let index = RoutingClearanceIndex::new(&final_board).unwrap();
    assert!(result.iter().all(|t| index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer).unwrap()));
}

#[test]
fn neck_down_routing_is_deterministic() {
    let board = neck_board(true, true, &[("VDD", &[("U1", "3"), ("U2", "3"), ("J1", "1")])]);
    let options = UnifiedRouterOptions { route_smoothing: true, ..UnifiedRouterOptions::default() };
    let (first, second) = (route(&board, &options), route(&board, &options));
    assert!(first.nets[0].connected, "{:?}", first.nets[0].diagnostics);
    assert_eq!((&first.board.tracks, &first.board.vias), (&second.board.tracks, &second.board.vias));
    assert_eq!(first.routing_fingerprint, second.routing_fingerprint);
    assert_eq!(first.to_json(), second.to_json());
}

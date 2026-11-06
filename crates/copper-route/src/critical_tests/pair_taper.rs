//! `tests/test_pair_taper.py`: D-PHY plan R2, aligned pair channels with
//! mismatched-pitch 45-degree tapers.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_physical::Point;
use copper_physical::footprint::{FootprintPad, Size};

use crate::critical::{CriticalOptions, CriticalRoutingResult, aligned_pair_paths, route_critical_nets, route_pair};
use crate::test_support_critical::*;

const CENTRE_Y: i64 = 10 * MM;

/// `_taper_board(chip_pitch, connector_pitch, connector_shift, uncoupled_limit)`:
/// a package pair to a connector pair across a straight 0.4 mm-pitch
/// channel. The pair profile is 0.2 mm width + 0.2 mm gap; both pairs share
/// one midpoint unless the connector is shifted.
fn taper_board(chip_pitch: &str, connector_pitch: &str, connector_shift: &str, limit: Option<&str>) -> PhysicalBoard {
    let (chip_half, connector_half, shift) = (nm(chip_pitch) / 2, nm(connector_pitch) / 2, nm(connector_shift));
    let land = |number: &str, x: i64, y: i64, height: i64| FootprintPad {
        size: Size { width_nm: 600_000, height_nm: height },
        ..smd_pad(number, at(x, y), 600_000)
    };
    let body = Size { width_nm: 3 * MM, height_nm: 3 * MM };
    let chip = footprint(
        "test/package-pair",
        vec![land("1", 1_400_000, -chip_half, 250_000), land("2", 1_400_000, chip_half, 250_000)],
        body,
    );
    let connector = footprint(
        "test/connector-pair",
        vec![
            land("1", -1_400_000, shift - connector_half, 200_000),
            land("2", -1_400_000, shift + connector_half, 200_000),
        ],
        body,
    );
    let poses = vec![pose("U1", &chip.name, mm(8, 10)), pose("J1", &connector.name, mm(22, 10))];
    let nets = vec![net("LANE_P", &[("U1", "1"), ("J1", "1")]), net("LANE_N", &[("U1", "2"), ("J1", "2")])];
    let mut board = board("PairTaper", 30, 20, vec![chip, connector], poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    board.net_routing_rules = [("LANE_P", "LANE_N"), ("LANE_N", "LANE_P")]
        .into_iter()
        .map(|(net, partner)| NetRoutingRule {
            priority: 100,
            maximum_uncoupled_length_nm: limit.map(nm),
            ..differential_rule(net, partner, 200_000, 200_000)
        })
        .collect();
    board
}

fn mirror(point: Point) -> Point {
    at(point.x_nm, 2 * CENTRE_Y - point.y_nm)
}

fn p(x: &str, y: &str) -> Point {
    at(nm(x), nm(y))
}

fn segments(tracks: &[TrackSegment], net: &str) -> Vec<(Point, Point)> {
    let mut items: Vec<(Point, Point)> = tracks.iter().filter(|t| t.net == net).map(|t| (t.start, t.end)).collect();
    items.sort_by_key(|(start, end)| (start.x_nm, end.x_nm));
    items
}

fn route(board: &PhysicalBoard) -> CriticalRoutingResult {
    let guides = global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    route_critical_nets(board.clone(), &guides, &CriticalOptions::default(), None).expect("critical routing")
}

fn routes(board: &PhysicalBoard) -> indexmap::IndexMap<String, crate::routing::GlobalNetRoute> {
    guides_by_net(&global_route(board, crate::routing::GlobalRouterOptions::default().tile_size_nm))
}

fn taper() -> i64 {
    (50_000.0 * 2f64.sqrt()).round() as i64
}

#[test]
fn mismatched_pitch_lands_use_the_aligned_channel_with_symmetric_tapers() {
    let board = taper_board("0.5", "0.4", "0", None);
    let result = route(&board);
    let pair = &result.nets[0];
    assert!(pair.connected, "{:?}", pair.diagnostics);
    assert_eq!(pair.strategy, "aligned_pair");
    // Each 0.5 mm land tapers 0.05 mm inward at 45 degrees onto its lane;
    // the 0.4 mm connector lands already sit on the lanes and need no taper.
    let p_tracks = segments(&result.locked_tracks, "LANE_P");
    assert_eq!(p_tracks, vec![(p("9.4", "9.75"), p("9.45", "9.8")), (p("9.45", "9.8"), p("20.6", "9.8"))]);
    let mirrored: Vec<(Point, Point)> = p_tracks.iter().map(|(s, e)| (mirror(*s), mirror(*e))).collect();
    assert_eq!(segments(&result.locked_tracks, "LANE_N"), mirrored);
    // Taper length is uncoupled length, reported per member.
    assert_eq!(pair.uncoupled_lengths_nm, vec![taper(), taper()]);
    assert_eq!(pair.lengths_nm[0] - pair.coupled_length_nm, taper());
    assert_eq!(pair.skew_nm, 0);
    assert!(free_of(&result.board, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"]));
    let again = route(&board);
    assert!(again.routing_fingerprint == result.routing_fingerprint && again.locked_tracks == result.locked_tracks);
}

#[test]
fn tapers_at_both_ends_go_inward_and_outward_symmetrically() {
    let board = taper_board("0.5", "0.3", "0", None);
    let result = route(&board);
    let pair = &result.nets[0];
    assert!(pair.connected && pair.strategy == "aligned_pair", "{:?}", pair.diagnostics);
    let p_tracks = segments(&result.locked_tracks, "LANE_P");
    // Inward from the 0.5 mm package lands, outward to the 0.3 mm connector lands.
    assert_eq!(
        p_tracks,
        vec![
            (p("9.4", "9.75"), p("9.45", "9.8")),
            (p("9.45", "9.8"), p("20.55", "9.8")),
            (p("20.55", "9.8"), p("20.6", "9.85")),
        ]
    );
    let mirrored: Vec<(Point, Point)> = p_tracks.iter().map(|(s, e)| (mirror(*s), mirror(*e))).collect();
    assert_eq!(segments(&result.locked_tracks, "LANE_N"), mirrored);
    assert_eq!(pair.uncoupled_lengths_nm, vec![2 * taper(), 2 * taper()]);
}

#[test]
fn taper_uncoupled_length_is_held_to_the_declared_limit() {
    let pair = |board: &PhysicalBoard| {
        let rules = &board.net_routing_rules;
        route_pair(board, &rules[0], &rules[1], &routes(board), false, board, &board.tracks).expect("pair").0
    };
    let tight = taper_board("0.5", "0.4", "0", Some("0.05"));
    let rejected = pair(&tight);
    assert!(rejected.strategy == "aligned_pair" && !rejected.connected, "{rejected:?}");
    assert_eq!(rejected.uncoupled_lengths_nm, vec![taper(), taper()]);
    assert_eq!(rejected.diagnostics, vec![format!("pair uncoupled length {} nm exceeds 50000 nm", taper())]);
    let loose = taper_board("0.5", "0.4", "0", Some("0.08"));
    let accepted = pair(&loose);
    assert!(accepted.strategy == "aligned_pair" && accepted.connected, "{accepted:?}");
}

#[test]
fn the_channel_requires_one_midpoint_and_the_same_member_order() {
    let offset = 200_000;
    let aligned = |board: &PhysicalBoard| {
        let routes = routes(board);
        aligned_pair_paths(&routes["LANE_N"], &routes["LANE_P"], offset).is_some()
    };
    assert!(aligned(&taper_board("0.5", "0.4", "0", None)));
    // A connector pair whose midpoint is 0.1 mm off the package midpoint
    // needs a jog, not a straight channel; the exact searches handle it.
    assert!(!aligned(&taper_board("0.5", "0.4", "0.1", None)));
    // Swapped members at one end would need a crossing.
    assert!(!aligned(&taper_board("0.5", "-0.4", "0", None)));
}

#[test]
fn equal_pitch_channels_keep_their_tapers() {
    let board = taper_board("1.0", "1.0", "0", None);
    let routes = routes(&board);
    let (first, second) = aligned_pair_paths(&routes["LANE_N"], &routes["LANE_P"], 200_000).expect("a channel");
    // Equal 1.0 mm pitch at both ends: identical 0.3 mm tapers, as before R2.
    let spans: Vec<(Point, Point)> = first.iter().map(|(_, start, end)| (*start, *end)).collect();
    assert_eq!(
        spans,
        vec![
            (p("9.4", "10.5"), p("9.7", "10.2")),
            (p("9.7", "10.2"), p("20.3", "10.2")),
            (p("20.3", "10.2"), p("20.6", "10.5")),
        ]
    );
    let mirrored: Vec<(Point, Point)> = second.iter().map(|(_, start, end)| (mirror(*start), mirror(*end))).collect();
    assert_eq!(mirrored, spans);
}

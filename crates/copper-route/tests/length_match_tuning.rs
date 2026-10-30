//! Length-match tuning of accepted critical groups (D-PHY plan R3), ported
//! from `tests/test_length_match_tuning.py`.

mod support_planes;

use copper_drc::breakout::BreakoutRegions;
use copper_drc::signal_integrity::{
    copper_layer_depths_nm, routed_match_length_nm, verify_match_groups, via_length_nm,
};
use copper_json::Value;
use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind, Stackup, TrackSegment, Via};
use copper_physical::geometry::segment_distance_squared;
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::critical::{
    CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus, critical_lane_table, critical_net_document,
    route_critical_nets,
};
use copper_route::critical_tuning::{TunerOptions, UnitTuner, bump_chamfers, bump_gain};
use num_bigint::BigInt;
use num_rational::BigRational;
use support_planes::lanes::*;
use support_planes::*;

fn critical(board: &PhysicalBoard) -> CriticalRoutingResult {
    route_critical_nets(board.clone(), &global(board), &CriticalOptions::default(), None).expect("critical")
}

fn critical_with(board: &PhysicalBoard, guides: &copper_route::GlobalRoutingResult) -> CriticalRoutingResult {
    route_critical_nets(board.clone(), guides, &CriticalOptions::default(), None).expect("critical")
}

/// The single match-tuning document of a result.
fn only_tuning(result: &CriticalRoutingResult) -> &Value {
    assert_eq!(result.match_tuning.len(), 1, "one match group");
    &result.match_tuning[0]
}

fn status(tuning: &Value) -> &str {
    text(field(tuning, "status"))
}

fn added(tuning: &Value) -> Vec<(String, i64)> {
    field(tuning, "members")
        .as_array()
        .unwrap()
        .iter()
        .map(|member| (text(field(member, "net")).to_owned(), int(field(member, "added_length_nm")).unwrap()))
        .collect()
}

fn added_of(tuning: &Value, net: &str) -> i64 {
    added(tuning).into_iter().find(|(name, _)| name == net).expect("member").1
}

/// Length/skew review warnings of connected copper (`_budget_warnings`).
fn budget_warnings(board: &PhysicalBoard) -> Vec<String> {
    copper_drc::run_physical_drc(board, copper_drc::Policy::default())
        .expect("drc")
        .findings
        .into_iter()
        .filter(|f| {
            f.severity == copper_drc::Severity::Warning
                && (f.code.starts_with("DRC-LENGTH") || f.code.starts_with("DRC-MAX-LENGTH"))
        })
        .map(|f| f.code)
        .collect()
}

fn lane(tracks: &[TrackSegment], name: &str) -> Vec<TrackSegment> {
    let prefix = format!("{name}_");
    tracks.iter().filter(|track| track.net.starts_with(&prefix)).cloned().collect()
}

fn of_net(tracks: &[TrackSegment], net: &str) -> Vec<TrackSegment> {
    tracks.iter().filter(|track| track.net == net).cloned().collect()
}

fn ends(tracks: &[TrackSegment]) -> impl Iterator<Item = Point> + '_ {
    tracks.iter().flat_map(|track| [track.start, track.end])
}

fn squared(value: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(value) * BigInt::from(value))
}

/// Least squared distance between any track of `first` and any of `second`.
fn least_gap_squared(first: &[TrackSegment], second: &[TrackSegment]) -> BigRational {
    first
        .iter()
        .flat_map(|a| second.iter().map(move |b| segment_distance_squared(a.start, a.end, b.start, b.end)))
        .min()
        .expect("tracks")
}

const COMPASS: [(i64, i64); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Compass index of an axis-aligned or exactly 45-degree track (`_heading`).
fn heading(track: &TrackSegment) -> usize {
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    assert!((dx == 0) != (dy == 0) || (dx.abs() == dy.abs() && dx != 0), "{track:?}");
    COMPASS.iter().position(|step| *step == (dx.signum(), dy.signum())).unwrap()
}

/// Vertices of a continuous path that turn by more than 45 degrees, except `keep`.
fn sharp_bends(path: &[TrackSegment], keep: &[Point]) -> Vec<Point> {
    path.windows(2)
        .filter(|pair| {
            let (a, b) = (heading(&pair[0]), heading(&pair[1]));
            !keep.contains(&pair[0].end) && ((b + 8 - a) % 8).min((a + 8 - b) % 8) > 1
        })
        .map(|pair| pair[0].end)
        .collect()
}

fn continuous(path: &[TrackSegment]) -> bool {
    path.windows(2).all(|pair| pair[0].end == pair[1].start)
}

/// Squared line distance from each track of `first` to its nearest parallel track of `second`.
fn lane_offsets(first: &[TrackSegment], second: &[TrackSegment]) -> Vec<BigRational> {
    first
        .iter()
        .map(|a| {
            let nearest = second
                .iter()
                .filter(|b| heading(b) % 4 == heading(a) % 4)
                .min_by_key(|b| segment_distance_squared(a.start, a.end, b.start, b.end))
                .expect("parallel track");
            let (dx, dy) = (i128::from(a.end.x_nm - a.start.x_nm), i128::from(a.end.y_nm - a.start.y_nm));
            let cross =
                dx * i128::from(nearest.start.y_nm - a.start.y_nm) - dy * i128::from(nearest.start.x_nm - a.start.x_nm);
            BigRational::new(BigInt::from(cross * cross), BigInt::from(dx * dx + dy * dy))
        })
        .collect()
}

fn empty_board() -> PhysicalBoard {
    board("Empty", 60.0, 60.0)
}

fn tuner(board: &PhysicalBoard, tracks: &[TrackSegment], options: TunerOptions) -> UnitTuner {
    let obstacles = RoutingClearanceIndex::new(board).unwrap();
    UnitTuner::new(
        board,
        &obstacles,
        tracks.to_vec(),
        &["A".to_owned(), "B".to_owned()],
        SPACING,
        AMPLITUDE,
        nm(0.1),
        options,
    )
    .unwrap()
}

fn net_docs(result: &CriticalRoutingResult) -> Vec<Value> {
    result.nets.iter().map(critical_net_document).collect()
}

fn with_breakout(board: &mut PhysicalBoard, length: f64, width: Option<f64>, clearance: Option<f64>) {
    for rule in &mut board.net_routing_rules {
        rule.breakout_length_nm = Some(nm(length));
        rule.breakout_width_nm = width.map(nm);
        rule.breakout_clearance_nm = clearance.map(nm);
    }
}

#[test]
fn pairs_of_a_match_group_are_tuned_as_pairs_into_the_group_skew() {
    let board = Lanes::new(&THREE_LANES).build();
    let result = critical(&board);
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned");
    assert_eq!(int(field(tuning, "skew_before_nm")), Some(nm(8.0)));
    assert_eq!(text(field(tuning, "reason")), "");
    let skew_after = int(field(tuning, "skew_after_nm")).unwrap();
    assert!(skew_after <= nm(0.1));
    // Both members of a pair gain the same length, so the pair's own skew is
    // kept within its own max_skew; the longest pair is untouched.
    assert_eq!((added_of(tuning, "LANE_A_N"), added_of(tuning, "LANE_A_P")), (0, 0));
    assert!(added_of(tuning, "LANE_B_N") == added_of(tuning, "LANE_B_P") && added_of(tuning, "LANE_B_N") > 0);
    assert!(added_of(tuning, "LANE_C_N") == added_of(tuning, "LANE_C_P") && added_of(tuning, "LANE_C_N") > 0);
    for name in ["LANE_A", "LANE_B", "LANE_C"] {
        let tracks = &result.board.tracks;
        let skew = net_length(tracks, &format!("{name}_N")) - net_length(tracks, &format!("{name}_P"));
        assert!(skew.abs() <= PAIR_SKEW, "{name} skew {skew}");
    }
    let checks = verify_match_groups(&result.board).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!((checks[0].status, checks[0].skew_nm), ("pass", Some(skew_after)));
    assert!(hard_findings(&result.board).is_empty(), "{:?}", hard_findings(&result.board));
    // Bumps are no taller than the amplitude limit and both members bend
    // together at the pair's own lane spacing.
    for (name, y) in [("LANE_B", 14.0), ("LANE_C", 22.0)] {
        let tracks = lane(&result.board.tracks, name);
        let rise = ends(&tracks).map(|p| (p.y_nm - nm(y)).abs()).max().unwrap();
        assert!(rise <= SPACING / 2 + AMPLITUDE, "{name} rises {rise}");
        let first: Vec<_> = tracks.iter().filter(|t| t.net.ends_with("_N")).cloned().collect();
        let second: Vec<_> = tracks.iter().filter(|t| t.net.ends_with("_P")).cloned().collect();
        assert_eq!(least_gap_squared(&first, &second), squared(SPACING));
    }
    // Results and the lane table report the tuned copper.
    assert!(result.nets.iter().all(|item| item.lengths_nm == vec![nm(28.0), nm(28.0)] && item.skew_nm == 0));
    let lanes = critical_lane_table(&result.board, &result.nets).unwrap();
    assert!(lanes.iter().all(|lane| int(field(lane, "routed_length_nm")) == Some(nm(28.0))));
    let document = copper_json::parse(&result.to_json().unwrap()).unwrap();
    let tuned = &field(&document, "match_tuning").as_array().unwrap()[0];
    assert_eq!(status(tuned), "tuned");
    assert_eq!(field(tuned, "bumps"), field(tuning, "bumps"));
    let member = &field(tuned, "members").as_array().unwrap()[2];
    assert_eq!(text(field(member, "net")), "LANE_B_N");
    assert_eq!(int(field(member, "length_before_nm")), Some(nm(20.0)));
    assert_eq!(int(field(member, "length_after_nm")), Some(nm(28.0)));
    assert_eq!(int(field(member, "added_length_nm")), Some(nm(8.0)));
}

#[test]
fn a_single_ended_member_is_tuned_alone() {
    let mut board = Lanes::new(&TWO_LANES).build();
    add_footprint(&mut board, footprint("test/one-land", vec![pad("1", mm(0.0, 0.0), size(0.6, 0.3))], size(1.0, 1.0)));
    board.placements.push(place("U9", "test/one-land", mm(5.0, 22.0)));
    board.placements.push(place("J9", "test/one-land", mm(21.0, 22.0)));
    board.nets.push(net("CLOCK_REF", &[("U9", "1"), ("J9", "1")]));
    board.net_routing_rules.push(NetRoutingRule {
        kind: RouteKind::Clock,
        priority: 100,
        width_nm: Some(WIDTH),
        tuning_amplitude_limit_nm: Some(AMPLITUDE),
        ..rule("CLOCK_REF")
    });
    board.match_groups = vec![NetMatchGroup {
        id: "lanes".to_owned(),
        nets: ["LANE_A_N", "LANE_A_P", "LANE_B_N", "LANE_B_P", "CLOCK_REF"].map(str::to_owned).to_vec(),
        max_skew_nm: nm(0.1),
    }];
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned", "{}", text(field(tuning, "reason")));
    let expected =
        [("LANE_A_N", 0), ("LANE_A_P", 0), ("LANE_B_N", nm(4.0)), ("LANE_B_P", nm(4.0)), ("CLOCK_REF", nm(12.0))];
    let mut got = added(tuning);
    got.sort();
    let mut expected: Vec<(String, i64)> = expected.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    expected.sort();
    assert_eq!(got, expected);
    let clock = result.nets.iter().find(|item| item.nets == ["CLOCK_REF"]).unwrap();
    assert_eq!(clock.lengths_nm, vec![nm(28.0)]);
    assert!(hard_findings(&result.board).is_empty());
}

#[test]
fn bumps_respect_the_amplitude_limit_and_prefer_the_free_side() {
    // A wall 0.5 mm above LANE_B leaves the side below free: all bumps go
    // below, no taller than the amplitude limit. A full-height bump adds
    // 2 x 1 mm less its chamfers, so 4 mm takes four levelled bumps.
    let board = Lanes::new(&TWO_LANES).keepouts(vec![lane_keepout("wall-above", 6.0, 14.8, 28.0, 17.0)]).build();
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!((status(tuning), int(field(tuning, "bumps"))), ("tuned", Some(4)));
    let (base, top) = bump_chamfers(AMPLITUDE, WIDTH, SPACING);
    assert!(3 * bump_gain(AMPLITUDE, base, top) < nm(4.0));
    let tracks = lane(&result.board.tracks, "LANE_B");
    let ys: Vec<i64> = ends(&tracks).map(|p| p.y_nm).collect();
    assert_eq!(*ys.iter().max().unwrap(), nm(14.2));
    let lowest = *ys.iter().min().unwrap();
    assert!(nm(13.8) - AMPLITUDE <= lowest && lowest < nm(13.8) - nm(0.8), "lowest {lowest}");
    assert!(hard_findings(&result.board).is_empty());

    // Walls on both sides leave 0.5 mm: more, lower bumps within that room.
    let narrow = Lanes::new(&TWO_LANES)
        .keepouts(vec![
            lane_keepout("wall-above", 6.0, 14.8, 28.0, 17.0),
            lane_keepout("wall-below", 6.0, 11.0, 28.0, 13.2),
        ])
        .build();
    let result = critical(&narrow);
    let tuning = only_tuning(&result);
    assert_eq!((status(tuning), int(field(tuning, "bumps"))), ("tuned", Some(10)));
    let tracks = lane(&result.board.tracks, "LANE_B");
    assert!(ends(&tracks).all(|p| nm(13.3) <= p.y_nm && p.y_nm <= nm(14.7)));
    assert!(hard_findings(&result.board).is_empty());
}

#[test]
fn a_unit_that_cannot_match_the_longest_member_adds_the_minimum_within_the_limit() {
    // A loose 6 mm group limit: matching the 7.2 mm pair to the 14.15 mm one
    // needs 6.95 mm, more than three 0.6 mm bumps on its short run can add.
    // The least length that reaches the limit (0.95 mm) still fits.
    let lanes: [Lane; 2] = [("LANE_A", 6.0, 5.0, 19.15), ("LANE_B", 14.0, 5.0, 12.2)];
    let mut builder = Lanes::new(&lanes);
    builder.max_skew = 6.0;
    builder.amplitude = Some(nm(0.6));
    let mut board = builder.build();
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned");
    assert_eq!(int(field(tuning, "skew_before_nm")), Some(nm(6.95)));
    assert_eq!(int(field(tuning, "skew_after_nm")), Some(nm(6.0)));
    assert_eq!(int(field(tuning, "bumps")), Some(2));
    let nonzero: Vec<(String, i64)> = added(tuning).into_iter().filter(|(_, v)| *v != 0).collect();
    assert_eq!(nonzero, vec![("LANE_B_N".to_owned(), nm(0.95)), ("LANE_B_P".to_owned(), nm(0.95))]);
    assert!(hard_findings(&result.board).is_empty());

    // With breakout regions the least length keeps a margin inside the limit:
    // a 45-degree piece re-cut at a region boundary may round differently.
    with_breakout(&mut board, 1.0, Some(0.15), None);
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned");
    assert_eq!(int(field(tuning, "skew_after_nm")), Some(nm(6.0) - 32));
    assert!(hard_findings(&result.board).is_empty());
}

#[test]
fn tuning_lands_where_there_is_room_along_the_member() {
    // Below LANE_B is blocked along its whole run, above it from x = 9 to 21 mm.
    let board = Lanes::new(&TWO_LANES)
        .keepouts(vec![
            lane_keepout("below", 6.0, 11.0, 28.0, 13.69),
            lane_keepout("above-middle", 9.0, 14.31, 21.0, 17.0),
        ])
        .build();
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned", "{}", text(field(tuning, "reason")));
    let tracks = lane(&result.board.tracks, "LANE_B");
    let raised: Vec<_> = tracks.iter().filter(|t| t.start.y_nm.max(t.end.y_nm) > nm(14.2)).collect();
    assert!(!raised.is_empty());
    assert!(raised.iter().all(|t| t.start.x_nm.max(t.end.x_nm) < nm(9.0) || t.start.x_nm.min(t.end.x_nm) > nm(21.0)));
    assert_eq!(ends(&tracks).map(|p| p.y_nm).min(), Some(nm(13.8)));
    assert!(hard_findings(&result.board).is_empty());
}

#[test]
fn impossible_tuning_restores_the_copper_and_warns() {
    let cases: Vec<(Lanes, &str)> = vec![
        (
            Lanes::new(&TWO_LANES).keepouts(vec![
                lane_keepout("above", 6.0, 14.305, 28.0, 17.0),
                lane_keepout("below", 6.0, 11.0, 28.0, 13.695),
            ]),
            "LANE_B_N/LANE_B_P: insufficient tuning room",
        ),
        (
            Lanes { amplitude: None, ..Lanes::new(&TWO_LANES) },
            "LANE_B_N/LANE_B_P: no tuning_amplitude_limit is declared",
        ),
        (Lanes { uncoupled: Some(2.0), ..Lanes::new(&TWO_LANES) }, "LANE_B_N/LANE_B_P: pair uncoupled length"),
    ];
    for (builder, reason) in cases {
        let board = builder.build();
        let guides = global(&board);
        let result = critical_with(&board, &guides);
        let mut unmatched = board.clone();
        unmatched.match_groups.clear();
        let baseline = critical_with(&unmatched, &guides);
        let tuning = only_tuning(&result);
        assert_eq!(status(tuning), "failed");
        assert!(text(field(tuning, "reason")).starts_with(reason), "{}", text(field(tuning, "reason")));
        assert_eq!(int(field(tuning, "skew_before_nm")), Some(nm(4.0)));
        assert_eq!(int(field(tuning, "skew_after_nm")), Some(nm(4.0)));
        assert_eq!(int(field(tuning, "bumps")), Some(0));
        assert!(added(tuning).iter().all(|(_, v)| *v == 0));
        // Every pair is still accepted; the copper and results are exactly
        // those of never tuning, and the stage warns: the group is left for review.
        assert!(result.nets.iter().all(|item| item.connected && item.diagnostics.is_empty()));
        assert_eq!(result.locked_tracks, baseline.locked_tracks);
        assert_eq!(result.locked_vias, baseline.locked_vias);
        assert_eq!(net_docs(&result), net_docs(&baseline));
        assert_eq!(result.routing_fingerprint, baseline.routing_fingerprint);
        assert_eq!(result.status, CriticalRoutingStatus::Warning);
        assert_ne!(baseline.status, CriticalRoutingStatus::Failed);
        assert_eq!(result.board.metadata["critical_routing"], "complete");
        assert!(budget_warnings(&result.board).contains(&"DRC-LENGTH-MATCH".to_owned()));
        assert!(!hard_findings(&result.board).contains("DRC-LENGTH-MATCH"));
        let document = copper_json::parse(&result.to_json().unwrap()).unwrap();
        assert_eq!(field(&field(&document, "match_tuning").as_array().unwrap()[0], "reason"), field(tuning, "reason"));
    }
}

#[test]
fn groups_within_their_limit_are_untouched() {
    let lanes: [Lane; 4] = [
        ("LANE_A", 6.0, 5.0, 33.0),
        ("LANE_B", 12.0, 5.0, 33.0),
        ("LANE_C", 18.0, 5.0, 33.0),
        ("LANE_D", 24.0, 5.0, 25.0),
    ];
    let board = Lanes {
        groups: Some(vec![("matched", vec!["LANE_A", "LANE_B"]), ("short", vec!["LANE_C", "LANE_D"])]),
        ..Lanes::new(&lanes)
    }
    .build();
    let guides = global(&board);
    let result = critical_with(&board, &guides);
    let mut unmatched = board.clone();
    unmatched.match_groups.clear();
    let baseline = critical_with(&unmatched, &guides);
    let outcomes: Vec<(&str, &str)> =
        result.match_tuning.iter().map(|item| (text(field(item, "id")), status(item))).collect();
    assert_eq!(outcomes, vec![("matched", "within_limit"), ("short", "tuned")]);
    let matched = &result.match_tuning[0];
    assert_eq!(int(field(matched, "skew_before_nm")), Some(0));
    assert_eq!(int(field(matched, "skew_after_nm")), Some(0));
    assert_eq!(int(field(matched, "bumps")), Some(0));
    for name in ["LANE_A", "LANE_B", "LANE_C"] {
        assert_eq!(lane(&result.locked_tracks, name), lane(&baseline.locked_tracks, name), "{name}");
    }
    assert_ne!(lane(&result.locked_tracks, "LANE_D"), lane(&baseline.locked_tracks, "LANE_D"));

    // A board whose only group is within its limit is identical to no group.
    let alone = Lanes::new(&lanes[..2]).build();
    let guides = global(&alone);
    let result = critical_with(&alone, &guides);
    let mut unmatched = alone.clone();
    unmatched.match_groups.clear();
    let baseline = critical_with(&unmatched, &guides);
    assert_eq!(status(&result.match_tuning[0]), "within_limit");
    assert_eq!(result.locked_tracks, baseline.locked_tracks);
    assert_eq!(net_docs(&result), net_docs(&baseline));
    assert_eq!(result.routing_fingerprint, baseline.routing_fingerprint);
}

#[test]
fn groups_with_an_unrouted_member_are_left_to_final_verification() {
    let mut board = Lanes::new(&TWO_LANES).build();
    board.net_routing_rules.retain(|rule| !rule.net.starts_with("LANE_B"));
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "incomplete");
    assert_eq!(int(field(tuning, "skew_before_nm")), None);
    assert_eq!(int(field(tuning, "skew_after_nm")), None);
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
}

#[test]
fn tuning_is_deterministic() {
    let board = Lanes::new(&THREE_LANES).build();
    let guides = global(&board);
    let first = critical_with(&board, &guides);
    let second = critical_with(&board, &guides);
    assert_eq!(first.board, second.board);
    assert_eq!(first.locked_tracks, second.locked_tracks);
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
}

#[test]
fn candidates_completing_an_over_skew_group_are_not_rejected_during_routing() {
    let board = Lanes { amplitude: None, ..Lanes::new(&TWO_LANES) }.build();
    let routed = critical(&board);
    assert!(routed.nets.len() == 2 && routed.nets.iter().all(|item| item.connected));
    let mut together = board.clone();
    together.tracks = lane(&routed.locked_tracks, "LANE_A");
    together.tracks.extend(lane(&routed.locked_tracks, "LANE_B"));
    // Together the two pairs violate the group skew; the tuning pass, not
    // candidate validation, judges the group (the validation half lives in
    // the crate's `test_support_planes` module).
    assert!(budget_warnings(&together).contains(&"DRC-LENGTH-MATCH".to_owned()));
}

#[test]
fn pair_bumps_keep_the_lane_spacing_in_any_direction() {
    let board = empty_board();
    let layer = CopperLayer::Front;
    let shapes: Vec<Vec<TrackSegment>> = vec![
        vec![
            track("A", mm(5.0, 10.0), mm(25.0, 10.0), WIDTH, layer),
            track("A", mm(25.0, 10.0), mm(25.0, 30.0), WIDTH, layer),
            track("B", mm(5.0, 9.6), mm(25.4, 9.6), WIDTH, layer),
            track("B", mm(25.4, 9.6), mm(25.4, 30.0), WIDTH, layer),
        ],
        vec![
            track("A", mm(30.0, 50.0), mm(30.0, 10.0), WIDTH, layer),
            track("B", mm(30.4, 10.0), mm(30.4, 50.0), WIDTH, layer),
        ],
    ];
    // Fifteen full-height bumps.
    let (base, top) = bump_chamfers(AMPLITUDE, WIDTH, SPACING);
    let added_nm = 15 * bump_gain(AMPLITUDE, base, top);
    for tracks in &shapes {
        let (tuned, bumps, reason) = tuner(&board, tracks, TunerOptions::default()).tune(added_nm).unwrap();
        let tuned = tuned.expect("tuned");
        assert_eq!((reason.as_str(), bumps), ("", 15));
        let corners: Vec<Point> = ends(tracks).collect();
        for net in ["A", "B"] {
            // Each member gains the same length, its path stays continuous and
            // no bump rises more than the amplitude limit off its own lane.
            assert_eq!(net_length(&tuned, net), net_length(tracks, net) + added_nm);
            let path = of_net(&tuned, net);
            assert!(continuous(&path));
            assert!(sharp_bends(&path, &corners).is_empty());
            let own = of_net(tracks, net);
            let rise = ends(&path)
                .map(|p| own.iter().map(|t| segment_distance_squared(p, p, t.start, t.end)).min().unwrap())
                .max()
                .unwrap();
            assert_eq!(rise, squared(AMPLITUDE));
        }
        assert_eq!(least_gap_squared(&of_net(&tuned, "A"), &of_net(&tuned, "B")), squared(SPACING));
        // The bump count is bounded.
        let bounded = TunerOptions { bump_limit: 4, ..TunerOptions::default() };
        let (none, _, reason) = tuner(&board, tracks, bounded).tune(added_nm).unwrap();
        assert!(none.is_none() && reason.contains("the limit of 4 bumps"), "{reason}");
    }
}

#[test]
fn pair_bumps_have_coupled_45_degree_corners_and_add_exact_lengths() {
    let board = empty_board();
    let layer = CopperLayer::Front;
    let tracks = vec![
        track("A", mm(5.0, 10.0), mm(45.0, 10.0), WIDTH, layer),
        track("B", mm(5.0, 9.6), mm(45.0, 9.6), WIDTH, layer),
    ];
    let tuner = tuner(&board, &tracks, TunerOptions::default());
    // The outside chamfer of a coupled 45-degree bend is longer by
    // floor((2 - sqrt 2) x spacing).
    let offset = 234_314;
    assert_eq!(bump_chamfers(AMPLITUDE, WIDTH, SPACING), (WIDTH, WIDTH + offset));
    // Targets from a few nanometres (single 45-degree ramps) to full-height
    // chamfered bumps.
    for added_nm in [4, 20_000, nm(0.3), nm(1.5), nm(7.0)] {
        let (tuned, bumps, reason) = tuner.tune(added_nm).unwrap();
        let tuned = tuned.expect("tuned");
        assert!(reason.is_empty() && bumps != 0, "{added_nm}: {reason}");
        let a = of_net(&tuned, "A");
        let b = of_net(&tuned, "B");
        for path in [&a, &b] {
            assert!(continuous(path));
            assert!(sharp_bends(path, &[]).is_empty());
            assert_eq!(copper_route::critical::track_length(&path.iter().collect::<Vec<_>>()), nm(40.0) + added_nm);
        }
        // The lanes stay the pair spacing apart through every bump, nowhere
        // closer and at most two nanometres farther (integer 45-degree corners).
        assert_eq!(least_gap_squared(&a, &b), squared(SPACING));
        for (first, second) in [(&a, &b), (&b, &a)] {
            assert!(lane_offsets(first, second).iter().all(|o| squared(SPACING) <= *o && *o < squared(SPACING + 2)));
        }
        if added_nm == nm(7.0) {
            // Full chamfers: each member is inside the turn at two corners of a
            // bump (leg = width) and outside it at the other two.
            for path in [&a, &b] {
                let mut legs: Vec<i64> = path
                    .iter()
                    .filter(|t| t.start.x_nm != t.end.x_nm && t.start.y_nm != t.end.y_nm)
                    .map(|t| (t.end.x_nm - t.start.x_nm).abs())
                    .collect();
                legs.sort();
                let count = 2 * bumps as usize;
                let mut expected = vec![WIDTH; count];
                expected.extend(vec![WIDTH + offset; count]);
                assert_eq!(legs, expected);
            }
        }
    }
}

/// The Python test drove `critical_preflight.main` on an example board with
/// the routing monkeypatched; the observable contract is the console line,
/// which `match_tuning_line` renders from the tuned group.
#[test]
fn preflight_line_reports_the_tuned_group() {
    let board = Lanes::new(&THREE_LANES).build();
    let routed = critical(&board);
    let tuning = only_tuning(&routed);
    let bumps = int(field(tuning, "bumps")).unwrap();
    let line = copper_route::critical_tuning::match_tuning_line(&tuning_result(tuning));
    let expected = format!(
        "match group lanes: tuned, skew 8.000 -> 0.000 mm (max 0.100 mm); added \
         LANE_B_N +8.000, LANE_B_P +8.000, LANE_C_N +4.000, LANE_C_P +4.000 mm; {bumps} bumps"
    );
    assert!(line.contains(&expected), "{line}");
}

/// A `MatchTuningResult` rebuilt from its document (units omitted: the
/// plain bump line does not report them).
fn tuning_result(document: &Value) -> copper_route::critical_tuning::MatchTuningResult {
    copper_route::critical_tuning::MatchTuningResult {
        id: text(field(document, "id")).to_owned(),
        max_skew_nm: int(field(document, "max_skew_nm")).unwrap(),
        status: status(document).to_owned(),
        skew_before_nm: int(field(document, "skew_before_nm")),
        skew_after_nm: int(field(document, "skew_after_nm")),
        members: field(document, "members")
            .as_array()
            .unwrap()
            .iter()
            .map(|member| copper_route::critical_tuning::MatchTuningMember {
                net: text(field(member, "net")).to_owned(),
                length_before_nm: int(field(member, "length_before_nm")).unwrap(),
                length_after_nm: int(field(member, "length_after_nm")).unwrap(),
            })
            .collect(),
        bumps: int(field(document, "bumps")).unwrap(),
        reason: text(field(document, "reason")).to_owned(),
        units: Vec::new(),
        tuning_groups: Vec::new(),
    }
}

#[test]
fn tuned_copper_of_breakout_nets_follows_the_breakout_regions() {
    let mut board = Lanes::new(&THREE_LANES).build();
    // Regions reach 9.5 mm from each land and cover most of every lane, so
    // tuning works next to necked breakout copper.
    with_breakout(&mut board, 9.5, Some(0.15), None);
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned");
    assert!(int(field(tuning, "skew_after_nm")).unwrap() <= int(field(tuning, "max_skew_nm")).unwrap());
    assert!(hard_findings(&result.board).is_empty());
    let tracks = &result.board.tracks;
    // Every piece is already cut at the boundaries with the region's width:
    // necked inside a region, the net's own width outside.
    assert_eq!(&BreakoutRegions::new(&result.board).split_tracks(tracks, Some(WIDTH)), tracks);
    let widths: std::collections::BTreeSet<i64> = tracks.iter().map(|t| t.width_nm).collect();
    assert_eq!(widths, [nm(0.15), WIDTH].into_iter().collect());
}

#[test]
fn tuning_bumps_stay_out_of_breakout_regions_where_there_is_room_elsewhere() {
    // LANE_B (x 5 to 29 mm) is 4 mm short of LANE_A and needs 0.3 mm. It
    // has 4 mm breakout regions at both ends. Below it is blocked; above it
    // the free room is a full 1 mm inside both regions but only about 0.4 mm
    // in the open run just before the J2 region. A bump in a region would
    // take the room neighbouring lands need to escape, so the lower slot
    // outside the regions is used first.
    let mut board = Lanes {
        max_skew: 3.7,
        ..Lanes::new(&TWO_LANES).keepouts(vec![
            lane_keepout("below", 6.0, 11.0, 28.0, 13.69),
            lane_keepout("above-middle", 9.5, 14.31, 21.0, 17.0),
            lane_keepout("above-low", 21.0, 14.81, 24.6, 17.0),
        ])
    }
    .build();
    with_breakout(&mut board, 4.0, None, Some(0.1));
    let result = critical(&board);
    let tuning = only_tuning(&result);
    assert_eq!(status(tuning), "tuned", "{}", text(field(tuning, "reason")));
    let regions = BreakoutRegions::new(&result.board);
    let tracks = lane(&result.board.tracks, "LANE_B");
    let raised: Vec<_> = tracks.iter().filter(|t| t.start.y_nm.max(t.end.y_nm) > nm(14.2)).cloned().collect();
    assert!(!raised.is_empty());
    assert!(!ends(&raised).any(|point| regions.inside_any(point)));
    assert!(hard_findings(&result.board).is_empty());
}

#[test]
fn pair_members_beside_vias_wider_than_the_lane_spacing_still_take_bumps() {
    // A pair runs 1.2 mm apart between its vias (three lane spacings), as
    // beside a paired via transition: both members bulge apart over the same
    // intervals and gain the same length. 1.7 mm apart they are no run.
    for (partner_y, tuned) in [(8.8, true), (8.3, false)] {
        let board = empty_board();
        let layer = CopperLayer::Front;
        let tracks = vec![
            track("A", mm(5.0, 10.0), mm(45.0, 10.0), WIDTH, layer),
            track("B", mm(5.0, partner_y), mm(45.0, partner_y), WIDTH, layer),
        ];
        let added_nm = nm(0.5);
        let (result, bumps, reason) = tuner(&board, &tracks, TunerOptions::default()).tune(added_nm).unwrap();
        if !tuned {
            assert!(result.is_none() && reason.contains("insufficient tuning room"), "{reason}");
            continue;
        }
        let result = result.expect("tuned");
        assert!(reason.is_empty() && bumps != 0);
        for net in ["A", "B"] {
            assert_eq!(net_length(&result, net), nm(40.0) + added_nm);
        }
        // A bulges up (+y), away from B below it, and B down.
        let a = of_net(&result, "A");
        let b = of_net(&result, "B");
        assert!(ends(&a).map(|p| p.y_nm).max().unwrap() > nm(10.0));
        assert_eq!(ends(&a).map(|p| p.y_nm).min(), Some(nm(10.0)));
        assert!(ends(&b).map(|p| p.y_nm).min().unwrap() < nm(8.8));
    }
}

fn lane_a_via(at: Point, from: CopperLayer, to: CopperLayer) -> Via {
    Via { from_layer: from, to_layer: to, ..via("LANE_A_P", at, nm(0.6), nm(0.3)) }
}

#[test]
fn match_lengths_count_each_via_between_the_layers_its_tracks_use() {
    // LANE_A changes from F.Cu to B.Cu and back through two vias; on a
    // 1.6 mm two-layer board each adds 1.6 mm to its match length. A via its
    // tracks meet on one layer only adds nothing.
    let mut board = Lanes::new(&TWO_LANES).build();
    board.stackup = Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]);
    let depths = copper_layer_depths_nm(&board.stackup);
    assert_eq!(depths.get(&CopperLayer::Front), Some(&0));
    assert_eq!(depths.get(&CopperLayer::Back), Some(&nm(1.6)));
    assert_eq!(depths.len(), 2);
    let (front, back) = (CopperLayer::Front, CopperLayer::Back);
    board.tracks = vec![
        track("LANE_A_P", mm(5.0, 6.0), mm(10.0, 6.0), WIDTH, front),
        track("LANE_A_P", mm(10.0, 6.0), mm(20.0, 6.0), WIDTH, back),
        track("LANE_A_P", mm(20.0, 6.0), mm(25.0, 6.0), WIDTH, front),
        track("LANE_A_P", mm(25.0, 6.0), mm(33.0, 6.0), WIDTH, front),
    ];
    board.vias = [10.0, 20.0, 25.0].iter().map(|x| lane_a_via(mm(*x, 6.0), front, back)).collect();
    assert_eq!(via_length_nm(&board, "LANE_A_P").unwrap(), 2 * nm(1.6));
    assert_eq!(routed_match_length_nm(&board, "LANE_A_P").unwrap(), nm(28.0) + 2 * nm(1.6));
}

#[test]
fn blind_and_stacked_vias_count_only_the_layers_they_plate() {
    // F.Cu -> In1.Cu through a blind via, In1.Cu -> B.Cu through a buried via
    // stacked on it: the path is the full board depth, not each via's reach
    // over every layer whose tracks end at that point.
    let (front, in1, back) = (CopperLayer::Front, CopperLayer::In1, CopperLayer::Back);
    let mut board = Lanes::new(&TWO_LANES).build();
    board.stackup = four_layers();
    let depths = copper_layer_depths_nm(&board.stackup);
    board.tracks = vec![
        track("LANE_A_P", mm(5.0, 6.0), mm(10.0, 6.0), WIDTH, front),
        track("LANE_A_P", mm(10.0, 6.0), mm(10.0, 6.3), WIDTH, in1),
        track("LANE_A_P", mm(10.0, 6.3), mm(20.0, 6.3), WIDTH, back),
    ];
    board.vias = vec![lane_a_via(mm(10.0, 6.0), front, in1), lane_a_via(mm(10.0, 6.3), in1, back)];
    assert_eq!(depths[&back] - depths[&front], nm(1.6));
    assert_eq!(via_length_nm(&board, "LANE_A_P").unwrap(), nm(1.6));
}

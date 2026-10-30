//! Crate-private behaviour tests ported from the former Python suite
//! (planes, stitching, fanout, vias, tuning and cleanup). The board
//! builders are shared with the integration tests in `tests/`.

#[path = "../tests/support_planes/mod.rs"]
mod support_planes;

/// `tests/test_length_match_tuning.py`.
mod length_match_tuning {
    use super::support_planes::lanes::*;
    use super::support_planes::*;
    use crate::critical::{CriticalOptions, route_critical_nets, validate_candidate};

    fn lane(tracks: &[copper_pcb::board::TrackSegment], name: &str) -> Vec<copper_pcb::board::TrackSegment> {
        let prefix = format!("{name}_");
        tracks.iter().filter(|track| track.net.starts_with(&prefix)).cloned().collect()
    }

    #[test]
    fn candidates_completing_an_over_skew_group_are_not_rejected_during_routing() {
        let board = Lanes { amplitude: None, ..Lanes::new(&TWO_LANES) }.build();
        let routed = route_critical_nets(board.clone(), &global(&board), &CriticalOptions::default(), None).unwrap();
        let second = routed.nets[1].clone();
        assert!(routed.nets[0].connected && second.connected);
        let tracks_a = lane(&routed.locked_tracks, "LANE_A");
        let tracks_b = lane(&routed.locked_tracks, "LANE_B");
        // Together the two pairs violate the group skew, yet the candidate
        // that completes the group passes validation: the tuning pass judges
        // the group.
        let (accepted, accepted_tracks, _) =
            validate_candidate(&board, second.clone(), tracks_b.clone(), Vec::new(), &tracks_a, &[], None).unwrap();
        assert!(accepted.connected && accepted.diagnostics.is_empty());
        assert_eq!(accepted_tracks, tracks_b);
        // Other hard findings still reject it.
        let mut blocked = board.clone();
        blocked.copper_keepouts = vec![lane_keepout("on-lane", 14.0, 13.0, 16.0, 15.0)];
        let (rejected, rejected_tracks, _) =
            validate_candidate(&blocked, second, tracks_b, Vec::new(), &tracks_a, &[], None).unwrap();
        assert!(!rejected.connected && rejected_tracks.is_empty());
        assert!(!rejected.diagnostics.iter().any(|message| message.starts_with("DRC-LENGTH-MATCH")));
    }
}

/// `tests/test_serpentine_tuning.py`: S-shaped (two-sided) serpentines (D-PHY plan R10).
mod serpentine_tuning {
    use super::support_planes::paths::*;
    use super::support_planes::*;
    use crate::RoutingClearanceIndex;
    use crate::critical::track_length;
    use crate::critical_tuning::{
        MATCH_TUNING_LEG_LIMIT, TunerOptions, UnitTuner, UnitTuning, leg_loss, serpentine_path, window_capacity,
    };
    use copper_pcb::board::{PhysicalBoard, TrackSegment, TuningStyle};
    use copper_physical::{CopperLayer, Point};

    const WIDTH: i64 = 140_000;
    const GAP: i64 = 260_000;
    const SPACING: i64 = WIDTH + GAP;
    const AMPLITUDE: i64 = 350_000;
    const CLEARANCE: i64 = 520_000;
    const LAYER: CopperLayer = CopperLayer::Front;

    fn empty() -> PhysicalBoard {
        board("Empty", 60.0, 60.0)
    }

    fn lane(net: &str, a: Point, b: Point) -> TrackSegment {
        track(net, a, b, WIDTH, LAYER)
    }

    /// `PAIRS`: along x, and members in opposite directions along y.
    fn pairs() -> Vec<(&'static str, Vec<TrackSegment>)> {
        vec![
            ("along x", vec![lane("A", mm(5.0, 10.0), mm(45.0, 10.0)), lane("B", mm(5.0, 9.6), mm(45.0, 9.6))]),
            ("reversed", vec![lane("A", mm(30.0, 50.0), mm(30.0, 10.0)), lane("B", mm(30.4, 10.0), mm(30.4, 50.0))]),
        ]
    }

    fn serpentine() -> TunerOptions {
        TunerOptions { style: TuningStyle::Serpentine, ..TunerOptions::default() }
    }

    fn tuner(
        tracks: &[TrackSegment],
        board: &PhysicalBoard,
        nets: &[&str],
        spacing: i64,
        options: TunerOptions,
    ) -> UnitTuner {
        let obstacles = RoutingClearanceIndex::new(board).unwrap();
        let nets: Vec<String> = nets.iter().map(|net| net.to_string()).collect();
        UnitTuner::new(board, &obstacles, tracks.to_vec(), &nets, spacing, AMPLITUDE, CLEARANCE, options).unwrap()
    }

    fn pair_tuner(tracks: &[TrackSegment], board: &PhysicalBoard) -> UnitTuner {
        tuner(tracks, board, &["A", "B"], SPACING, serpentine())
    }

    fn length(tracks: &[TrackSegment]) -> i64 {
        track_length(&tracks.iter().collect::<Vec<_>>())
    }

    /// Signed distance of every corner from the original line (`_offsets`).
    fn offsets(path: &[TrackSegment], line: i64, horizontal: bool) -> Vec<i64> {
        ends(path).map(|p| if horizontal { p.y_nm } else { p.x_nm } - line).collect()
    }

    fn tracks(plan: &UnitTuning) -> &[TrackSegment] {
        plan.tracks.as_deref().expect("tuned copper")
    }

    #[test]
    fn serpentine_legs_alternate_sides_and_add_exact_equal_lengths() {
        for (shape, lanes) in pairs() {
            let tuner = pair_tuner(&lanes, &empty());
            // The leg pitch is the lane width plus the default leg gap, the
            // larger of 3 x width and the clearance.
            assert_eq!(tuner.windows[0].pitch, SPACING + WIDTH + CLEARANCE);
            for added in [nm(1.5), nm(4.1), nm(7.0)] {
                let plan = tuner.plan(added).unwrap();
                assert!(plan.reason.is_empty() && plan.style() == "serpentine" && plan.bumps == 0, "{shape}");
                assert_eq!(plan.legs as usize, plan.amplitudes_nm.len() + 1);
                assert!(plan.amplitudes_nm.iter().all(|h| nm(0.234) < *h && *h <= AMPLITUDE));
                let (low, high) = (plan.amplitudes_nm.iter().min().unwrap(), plan.amplitudes_nm.iter().max().unwrap());
                assert!(high - low <= 1, "levelled");
                let (a, b) = (of_net(tracks(&plan), "A"), of_net(tracks(&plan), "B"));
                for (net, path) in [("A", &a), ("B", &b)] {
                    // Each member gains exactly the length asked for, stays
                    // continuous and turns by 45 degrees at most.
                    assert_eq!(length(path), length(&of_net(&lanes, net)) + added);
                    assert!(continuous(path));
                    assert!(sharp_bends(path, &[]).is_empty());
                }
                // The members keep the pair spacing through every leg and corner.
                assert_eq!(least_gap_squared(&a, &b), squared(SPACING));
                for (first, second) in [(&a, &b), (&b, &a)] {
                    assert!(
                        lane_offsets(first, second).iter().all(|o| squared(SPACING) <= *o && *o < squared(SPACING + 2))
                    );
                }
                // The tops alternate sides: the trace crosses its original
                // line and never rises more than the amplitude limit off it.
                let horizontal = shape == "along x";
                let line = if horizontal { lanes[0].start.y_nm } else { lanes[0].start.x_nm };
                let off = offsets(&a, line, horizontal);
                assert!(*off.iter().min().unwrap() < 0 && *off.iter().max().unwrap() > 0);
                assert_eq!(off.iter().map(|o| o.abs()).max().unwrap(), *high);
            }
        }
    }

    #[test]
    fn each_leg_adds_its_length_less_one_leg_loss_for_both_members() {
        // Serpentines of one to five tops, at uneven heights: every member
        // gains 2 x the sum of heights less (tops + 1) leg losses.
        let pitch = SPACING + WIDTH + CLEARANCE;
        let cases: [&[i64]; 4] = [
            &[300_000],
            &[300_000, 280_001],
            &[250_017, 333_333, 300_000],
            &[240_000, 350_000, 240_000, 350_000, 299_999],
        ];
        for heights in cases {
            let sides: Vec<i64> = (0..heights.len()).map(|i| if i % 2 == 0 { 1 } else { -1 }).collect();
            // Every leg is at least 240 um long: up to (240 um - d) // 2.
            for chamfer in [1, 1_000, 2_843] {
                let expected =
                    2 * heights.iter().sum::<i64>() - (heights.len() as i64 + 1) * leg_loss(chamfer, SPACING);
                for member in [1, -1] {
                    let corners = serpentine_path(nm(10.0), pitch, SPACING, &sides, heights, member, chamfer);
                    let mut points = vec![Point::new(nm(5.0), 0)];
                    points.extend(corners.iter().map(|(x, y)| Point::new(*x, *y)));
                    points.push(Point::new(nm(40.0), 0));
                    let path: Vec<TrackSegment> = points
                        .windows(2)
                        .filter(|pair| pair[0] != pair[1])
                        .map(|pair| lane("A", pair[0], pair[1]))
                        .collect();
                    assert_eq!(length(&path) - nm(35.0), expected, "{heights:?} chamfer {chamfer}");
                }
            }
        }
        // A single net (no partner) loses 2 x (2c - round(c sqrt 2)) per leg.
        assert_eq!(leg_loss(7_000, 0), 2 * (14_000 - 9_899));
    }

    #[test]
    fn a_single_net_snakes_about_its_own_line() {
        let single = vec![lane("A", mm(5.0, 10.0), mm(45.0, 10.0))];
        let tuner = tuner(&single, &empty(), &["A"], 0, serpentine());
        assert_eq!(tuner.windows[0].pitch, WIDTH + CLEARANCE);
        let plan = tuner.plan(nm(5.0)).unwrap();
        assert_eq!(plan.style(), "serpentine");
        let path = of_net(tracks(&plan), "A");
        assert_eq!(length(&path), nm(45.0));
        assert!(sharp_bends(&path, &[]).is_empty());
        let off = offsets(&path, nm(10.0), true);
        assert!(off.iter().any(|o| *o > 0) && off.iter().any(|o| *o < 0));
    }

    fn walled(x1: f64) -> PhysicalBoard {
        let mut board = empty();
        board.copper_keepouts =
            vec![keepout("wall", &[CopperLayer::Front, CopperLayer::Back], rect(4.0, 7.0, x1, 9.33))];
        board
    }

    #[test]
    fn serpentines_fall_back_to_one_sided_bumps() {
        // A wall at the clearance below the pair leaves room on one side
        // only: no serpentine fits, so the unit gets R3's one-sided bumps upward.
        let board = walled(46.0);
        let lanes = pairs().remove(0).1;
        let tuner = pair_tuner(&lanes, &board);
        assert!(tuner.windows.is_empty());
        let plan = tuner.plan(nm(1.0)).unwrap();
        assert!(plan.reason.is_empty() && plan.style() == "bumps" && plan.legs == 0 && plan.bumps != 0);
        assert!(offsets(&of_net(tracks(&plan), "B"), nm(9.6), true).iter().all(|o| *o >= 0));
        let bumps = tuner_bumps(&lanes, &board).tune(nm(1.0)).unwrap().0;
        assert_eq!(plan.tracks, bumps);
        // Too little length for tops tall enough to have 45-degree corners on
        // both members also falls back to bumps.
        let small = pair_tuner(&lanes, &empty()).plan(nm(0.2)).unwrap();
        assert!(small.style() == "bumps" && small.bumps != 0);
        // A wall on one side over part of the run: the serpentine takes the
        // free stretch, another run keeps bumps.
        let plan = pair_tuner(&lanes, &walled(25.0)).plan(nm(3.0)).unwrap();
        assert_eq!(plan.style(), "serpentine");
        assert!(tracks(&plan).iter().filter(|t| t.start.y_nm < nm(9.6)).all(|t| t.start.x_nm > nm(25.0)));
    }

    fn tuner_bumps(lanes: &[TrackSegment], board: &PhysicalBoard) -> UnitTuner {
        tuner(lanes, board, &["A", "B"], SPACING, TunerOptions::default())
    }

    #[test]
    fn the_leg_count_is_bounded() {
        let lanes = pairs().remove(0).1;
        let options = TunerOptions { leg_limit: 6, bump_limit: 3, ..serpentine() };
        let tuner = tuner(&lanes, &empty(), &["A", "B"], SPACING, options);
        // Six legs hold five tops: their full capacity fits, more does not
        // (nor in the three bumps of the fallback).
        let five = window_capacity(&tuner.windows[0], &[AMPLITUDE; 5]);
        let plan = tuner.plan(five - five % 2).unwrap();
        assert!(plan.reason.is_empty() && plan.legs == 6, "{}", plan.reason);
        let plan = tuner.plan(five + 2).unwrap();
        assert!(plan.tracks.is_none());
        assert!(plan.reason.contains("the limit of 3 bumps; serpentine: insufficient tuning room"), "{}", plan.reason);
        assert!(plan.reason.ends_with("in 6 leg(s), the limit of 6 legs"), "{}", plan.reason);
        assert_eq!(MATCH_TUNING_LEG_LIMIT, 32);
    }

    #[test]
    fn a_declared_leg_gap_sets_the_pitch() {
        let lanes = pairs().remove(0).1;
        let options = TunerOptions { leg_gap_nm: Some(nm(0.8)), ..serpentine() };
        let tuner = tuner(&lanes, &empty(), &["A", "B"], SPACING, options);
        assert_eq!(tuner.windows[0].pitch, SPACING + WIDTH + nm(0.8));
        let plan = tuner.plan(nm(3.0)).unwrap();
        let mut legs: Vec<i64> =
            of_net(tracks(&plan), "A").iter().filter(|t| t.start.x_nm == t.end.x_nm).map(|t| t.start.x_nm).collect();
        legs.sort();
        legs.dedup();
        // Facing legs of one member are the gap plus one width apart (centres).
        assert_eq!(legs.windows(2).map(|pair| pair[1] - pair[0]).min(), Some(nm(0.8) + WIDTH));
    }

    #[test]
    fn bumps_in_the_open_run_win_over_a_serpentine_beside_the_lands() {
        // One net along x. Below it, x 25 to 45 mm is blocked, so a
        // serpentine only fits at x < 25 mm. Every slot starting before
        // x = 26 mm counts as inside a breakout region beside the lands here.
        // One-sided bumps further along add the length outside it and win;
        // with every slot outside, the serpentine still comes first.
        let mut board = empty();
        board.copper_keepouts =
            vec![keepout("below", &[CopperLayer::Front, CopperLayer::Back], rect(25.0, 8.0, 46.0, 9.8))];
        let single = vec![lane("A", mm(5.0, 10.0), mm(45.0, 10.0))];
        let added = nm(1.2);
        let free = tuner(&single, &board, &["A"], 0, serpentine()).plan(added).unwrap();
        assert!(free.legs != 0 && free.tracks.is_some());
        let mut beside = tuner(&single, &board, &["A"], 0, serpentine());
        let near: Vec<_> = beside
            .ordered
            .iter()
            .copied()
            .chain(beside.windows.iter().flat_map(|window| window.tops.iter().copied()))
            .filter(|slot| slot.start < nm(26.0))
            .collect();
        beside.inside = near.into_iter().collect();
        let plan = beside.plan(added).unwrap();
        assert!(plan.tracks.is_some() && plan.legs == 0);
        let moved: Vec<_> =
            tracks(&plan).iter().filter(|t| t.start.y_nm != nm(10.0) || t.end.y_nm != nm(10.0)).collect();
        assert!(!moved.is_empty() && moved.iter().all(|t| t.start.x_nm.min(t.end.x_nm) > nm(25.5)));
        assert_eq!(length(tracks(&plan)), nm(40.0) + added);
    }
}

/// `tests/test_group_tuning.py::test_tuning_group_steps_are_dropped_when_a_unit_reroutes`.
mod group_tuning {
    use super::support_planes::track;
    use crate::critical::CriticalNetResult;
    use crate::critical_match::{Committed, RoutingTuning, drop_group_steps};
    use crate::critical_tuning::MatchTuningUnit;
    use copper_pcb::board::TrackSegment;
    use copper_physical::{CopperLayer, Point};
    use indexmap::IndexMap;

    fn unit(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// A result told apart by its `strategy` (the Python test used strings).
    fn result(name: &str) -> CriticalNetResult {
        CriticalNetResult {
            strategy: name.to_owned(),
            ..CriticalNetResult::new(Vec::new(), true, 0, 0, Vec::new(), 0, Vec::new())
        }
    }

    fn step(nets: Vec<String>, group: &str) -> MatchTuningUnit {
        MatchTuningUnit {
            lanes: if group.is_empty() { Vec::new() } else { nets.clone() },
            nets,
            stage: "routing".to_owned(),
            style: "bumps".to_owned(),
            added_length_nm: 2,
            bumps: 1,
            legs: 0,
            amplitudes_nm: vec![5],
            group: group.to_owned(),
        }
    }

    fn strategies(results: &[CriticalNetResult]) -> Vec<&str> {
        results.iter().map(|result| result.strategy.as_str()).collect()
    }

    #[test]
    fn tuning_group_steps_are_dropped_when_a_unit_reroutes() {
        let (a, b, c) = (unit(&["A_N", "A_P"]), unit(&["B_N", "B_P"]), unit(&["C_N", "C_P"]));
        let tracks: Vec<Vec<TrackSegment>> = [&a, &b, &c]
            .iter()
            .enumerate()
            .map(|(index, unit)| {
                let y = index as i64;
                vec![track(&unit[0], Point::new(0, y), Point::new(10, y), 1, CopperLayer::Front)]
            })
            .collect();
        let tuned = |index: usize| [tracks[index].clone(), tracks[index].clone()].concat();
        let mut committed: Committed = vec![
            (a.clone(), tuned(0), Vec::new()),
            (b.clone(), tuned(1), Vec::new()),
            (c.clone(), tracks[2].clone(), Vec::new()),
        ];
        let mut results = vec![result("a"), result("b-tuned"), result("c")];
        let index: IndexMap<Vec<String>, usize> =
            [(a.clone(), 0), (b.clone(), 1), (c.clone(), 2)].into_iter().collect();
        let ab = [a.clone(), b.clone()].concat();
        let mut tuning = RoutingTuning {
            early: vec![step(ab.clone(), "pair-ab"), step(a.clone(), "")],
            routed: ["A_N", "A_P", "B_N", "B_P"].iter().map(|net| ((*net).to_owned(), 1)).collect(),
            failed_targets: [(ab, 3), (c.clone(), 3)].into_iter().collect(),
            as_routed: [
                (a.clone(), (tracks[0].clone(), result("a-routed"))),
                (b.clone(), (tracks[1].clone(), result("b-routed"))),
            ]
            .into_iter()
            .collect(),
            group_failures: IndexMap::new(),
        };
        // A re-routed: B goes back to its copper as routed, every step of A and B goes.
        assert!(drop_group_steps(std::slice::from_ref(&a), &mut committed, &mut results, &index, &mut tuning));
        assert_eq!(committed[1], (b.clone(), tracks[1].clone(), Vec::new()));
        assert_eq!(strategies(&results), ["a", "b-routed", "c"]);
        assert!(tuning.early.is_empty());
        assert_eq!(tuning.failed_targets.clone().into_iter().collect::<Vec<_>>(), [(c.clone(), 3)]);
        let kept: Vec<(&Vec<String>, &str)> =
            tuning.as_routed.iter().map(|(unit, (_, result))| (unit, result.strategy.as_str())).collect();
        assert_eq!(kept, [(&a, "a-routed")]);
        assert_eq!(tuning.as_routed[&a].0, tracks[0]);
        assert_eq!(
            tuning.routed.clone().into_iter().collect::<Vec<_>>(),
            [("A_N".to_owned(), 1), ("A_P".to_owned(), 1)]
        );
        // Without group steps nothing changes.
        assert!(!drop_group_steps(&[c], &mut committed, &mut results, &index, &mut tuning));
    }
}

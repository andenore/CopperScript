//! `tests/test_pair_refine.py`: bounded joint shared-spine shortcuts
//! (`paired_shortcuts`) and the owner that validates them.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_physical::CopperLayer::{Back as BACK, Front as FRONT};
use copper_physical::Point;
use copper_physical::footprint::Size;

use crate::critical::{improve_pair_spine, route_pair_exact, track_length, validate_candidate};
use crate::pair_refine::{Lanes, PairRefinementStats, compact, paired_shortcuts};
use crate::pair_search::{PORT_LENGTHS_NM, PairSearchCandidate, Port, PortRequest, lane_paths, tracks};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

fn reversed(lane: &[TrackSegment]) -> Vec<TrackSegment> {
    lane.iter().rev().map(|t| TrackSegment { start: t.end, end: t.start, ..t.clone() }).collect()
}

/// `fixture()`: pair A/B from J1 (5, 12) to J2 (35, 12) and a joint
/// candidate that detours north over (14..28, 16..24) mm.
fn fixture() -> (PhysicalBoard, PairSearchCandidate) {
    let pads = vec![smd_pad("1", at(0, -500_000), 400_000), smd_pad("2", at(0, 500_000), 400_000)];
    let pair = footprint("pair", pads, Size { width_nm: MM, height_nm: 2 * MM });
    let nets = vec![net("A", &[("J1", "1"), ("J2", "1")]), net("B", &[("J1", "2"), ("J2", "2")])];
    let mut board = board(
        "PairRefine",
        40,
        32,
        vec![pair],
        vec![pose("J1", "pair", mm(5, 12)), pose("J2", "pair", mm(35, 12))],
        nets,
    );
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            max_vias: Some(0),
            max_skew_nm: Some(10 * MM),
            maximum_uncoupled_length_nm: Some(10 * MM),
            ..differential_rule(name, partner, 250_000, 200_000)
        })
        .collect();
    let (width, offset) = (250_000, 225_000);
    let port = |a: Point, b: Point, component: &str, center: Point| -> Port {
        let index = RoutingClearanceIndex::new(&board).expect("index");
        PortRequest {
            board: &board,
            index: &index,
            first_name: "A",
            second_name: "B",
            first: a,
            second: b,
            component,
            second_component: component,
            width,
            offset,
            clearance: board.rules.minimum_clearance_nm,
            layer: FRONT,
            lengths: &PORT_LENGTHS_NM,
            allow_local_fallback: true,
        }
        .ports()
        .expect("ports")
        .into_iter()
        .find(|p| p.center == center)
        .expect("a port")
    };
    let start = port(at(5 * MM, 11_500_000), at(5 * MM, 12_500_000), "J1", mm(6, 12));
    let end = port(at(35 * MM, 11_500_000), at(35 * MM, 12_500_000), "J2", mm(34, 12));
    let points: Vec<Point> = [
        (6, 12),
        (7, 12),
        (8, 12),
        (10, 12),
        (14, 16),
        (14, 20),
        (18, 24),
        (24, 24),
        (28, 20),
        (28, 16),
        (32, 12),
        (34, 12),
    ]
    .into_iter()
    .map(|(x, y)| mm(x, y))
    .collect();
    let (first_lane, second_lane) =
        lane_paths(&points, start.heading, (end.heading + 4) % 8, offset, start.sign).expect("lanes");
    let lane = |name: &str, lane: &[Point], lead: &[TrackSegment], tail: &[TrackSegment]| {
        let mut all = lead.to_vec();
        all.extend(tracks(name, lane, width, FRONT, None));
        all.extend(reversed(tail));
        all
    };
    let first = lane("A", &first_lane, &start.first, &end.first);
    let second = lane("B", &second_lane, &start.second, &end.second);
    let candidate = PairSearchCandidate {
        first,
        second,
        expanded_states: 12,
        search_index: 1,
        spine: points,
        start_port: Some(start),
        end_port: Some(end),
        via_pairs: Vec::new(),
        return_vias: Vec::new(),
    };
    (board, candidate)
}

fn shortcuts(
    board: &PhysicalBoard,
    candidate: &PairSearchCandidate,
    attempts: i64,
    stats: &mut PairRefinementStats,
) -> Vec<Lanes> {
    let rules = &board.net_routing_rules;
    paired_shortcuts(board, candidate, &rules[0], &rules[1], attempts, stats).expect("shortcuts")
}

fn length(lane: &[TrackSegment]) -> i64 {
    track_length(&lane.iter().collect::<Vec<_>>())
}

fn with_tracks(board: &PhysicalBoard, lanes: &[&[TrackSegment]]) -> PhysicalBoard {
    let mut routed = board.clone();
    routed.tracks.extend(lanes.iter().flat_map(|lane| lane.iter().cloned()));
    routed
}

#[test]
fn joint_shortcuts_shorten_detour_preserve_exact_ports_and_are_deterministic() {
    let (board, candidate) = fixture();
    let before = board.clone();
    let mut stats = PairRefinementStats::default();
    let proposals = shortcuts(&board, &candidate, 256, &mut stats);
    assert_eq!(proposals, shortcuts(&board, &candidate, 256, &mut PairRefinementStats::default()));
    assert!(stats.candidates == proposals.len() as i64 && !proposals.is_empty() && stats.attempts <= 256);
    let last = proposals.last().expect("a proposal");
    assert!(length(&last.0) <= length(&candidate.first) && length(&last.1) <= length(&candidate.second));
    assert!(length(&last.0) + length(&last.1) < length(&candidate.first) + length(&candidate.second));
    let (start, end) = (candidate.start_port.as_ref().unwrap(), candidate.end_port.as_ref().unwrap());
    for proposal in &proposals {
        for (lane, lead, tail) in [(&proposal.0, &start.first, &end.first), (&proposal.1, &start.second, &end.second)] {
            assert_eq!(&lane[..lead.len()], &lead[..]);
            assert_eq!(&lane[lane.len() - tail.len()..], &reversed(tail)[..]);
        }
        assert!(free_of(
            &with_tracks(&board, &[&proposal.0, &proposal.1]),
            &["DRC-SHORT", "DRC-CLEARANCE", "DRC-OPEN-NET"]
        ));
    }
    assert!(board == before && board.tracks.is_empty());
}

#[test]
fn shortcut_cannot_cut_a_keepout_or_reserved_foreign_route() {
    for obstacle in ["keepout", "reservation"] {
        let (mut board, candidate) = fixture();
        if obstacle == "keepout" {
            board.copper_keepouts = vec![front_wall("block", 19 * MM, 8 * MM, 21 * MM, 18 * MM)];
        } else {
            board.nets.push(net("FOREIGN", &[]));
            board.tracks = vec![track("FOREIGN", mm(20, 8), mm(20, 18), 500_000, FRONT)];
        }
        let proposals = shortcuts(&board, &candidate, 256, &mut PairRefinementStats::default());
        assert!(!proposals.is_empty(), "{obstacle}");
        for proposal in &proposals {
            let routed = with_tracks(&board, &[&proposal.0, &proposal.1]);
            assert!(free_of(&routed, &["DRC-SHORT", "DRC-CLEARANCE", "DRC-KEEPOUT", "DRC-OPEN-NET"]), "{obstacle}");
        }
    }
}

#[test]
fn refinement_is_bounded_and_missing_provenance_fails_closed() {
    let (board, candidate) = fixture();
    let mut stats = PairRefinementStats::default();
    shortcuts(&board, &candidate, 2, &mut stats);
    assert!(stats.attempts <= 2);
    let mut spent = PairRefinementStats { attempts: 2, candidates: 0 };
    assert!(shortcuts(&board, &candidate, 2, &mut spent).is_empty());
    assert_eq!(spent.attempts, 2);
    let none = |change: &dyn Fn(&mut PairSearchCandidate)| {
        let mut altered = candidate.clone();
        change(&mut altered);
        shortcuts(&board, &altered, 256, &mut PairRefinementStats::default()).is_empty()
    };
    assert!(none(&|c| c.spine.clear()));
    assert!(none(&|c| c.start_port = None));
    assert!(none(&|c| {
        c.first.remove(0);
    }));
    assert!(none(&|c| c.spine = vec![c.spine[0], *c.spine.last().unwrap()]));
    let rules = &board.net_routing_rules;
    assert!(
        paired_shortcuts(&board, &candidate, &rules[0], &rules[1], 0, &mut PairRefinementStats::default()).is_err()
    );
    let points = [at(0, 0), at(1, 0), at(2, 0), at(1, 0)];
    assert_eq!(compact(&points), vec![at(0, 0), at(2, 0), at(1, 0)]);
}

#[test]
fn owner_preserves_complete_accepted_incumbent_on_bad_shortcut() {
    // Python injected each defective shortcut into `_improve_pair_spine`;
    // here the owner keeps its accepted incumbent over the real shortcuts,
    // and each defective proposal fails the same owner gates.
    for defect in ["gap", "profile", "native_open", "width", "layer"] {
        let (mut board, candidate) = fixture();
        if defect == "profile" {
            for rule in &mut board.net_routing_rules {
                rule.max_skew_nm = Some(100);
            }
        }
        let rules = board.net_routing_rules.clone();
        let routes = guides_by_net(&global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm));
        let owner = |lanes: &Lanes| {
            let (proposed, tracks, vias) =
                route_pair_exact(&board, &rules[0], &rules[1], &routes, lanes, &board, (&[], &[]), &board.tracks)
                    .expect("pair");
            validate_candidate(&board, proposed, tracks, vias, &[], &[], None).expect("validated")
        };
        let incumbent = owner(&(candidate.first.clone(), candidate.second.clone()));
        assert!(incumbent.0.connected, "{defect}: {:?}", incumbent.0);
        let mut proposal =
            shortcuts(&board, &candidate, 256, &mut PairRefinementStats::default()).pop().expect("a shortcut");
        match defect {
            "gap" => proposal.1 = proposal.0.clone(),
            "profile" => {
                // Accepted incumbent fits its own skew; the shorter path need not fit.
                let lane = &mut proposal.1;
                let (i, edge) = lane
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.start.y_nm == t.end.y_nm && t.end.x_nm - t.start.x_nm > 2 * MM)
                    .map(|(i, t)| (i, t.clone()))
                    .expect("a long east edge");
                let bump = at(edge.start.x_nm + 100_000, edge.start.y_nm + 100_000);
                let far = at(edge.end.x_nm - 100_000, bump.y_nm);
                lane.splice(
                    i..=i,
                    [
                        TrackSegment { end: bump, ..edge.clone() },
                        TrackSegment { start: bump, end: far, ..edge.clone() },
                        TrackSegment { start: far, ..edge.clone() },
                    ],
                );
                // Over its skew budget the shortcut is only a warned fallback,
                // never better than an incumbent within it.
                let rejected = route_pair_exact(
                    &board,
                    &rules[0],
                    &rules[1],
                    &routes,
                    &proposal,
                    &board,
                    (&[], &[]),
                    &board.tracks,
                )
                .expect("pair")
                .0;
                assert!(rejected.warnings.iter().any(|w| w.contains("skew")), "{rejected:?}");
            }
            "width" => {
                for t in proposal.0.iter_mut().chain(proposal.1.iter_mut()) {
                    t.width_nm = 200_000;
                }
            }
            "layer" => {
                for t in proposal.0.iter_mut().chain(proposal.1.iter_mut()) {
                    t.layer = BACK;
                }
            }
            _ => {
                proposal.0.remove(0);
            }
        }
        if matches!(defect, "gap" | "native_open") {
            assert!(!owner(&proposal).0.connected, "{defect}");
        }
        let selected = improve_pair_spine(
            &board,
            &board,
            (&rules[0], &rules[1]),
            &routes,
            &candidate,
            incumbent.clone(),
            &[],
            &[],
        )
        .expect("refinement");
        assert!(selected.0.connected, "{defect}");
        assert_eq!(selected.0.nets, incumbent.0.nets);
        assert!(selected.0.lengths_nm.iter().zip(&incumbent.0.lengths_nm).all(|(a, b)| a <= b));
    }
}

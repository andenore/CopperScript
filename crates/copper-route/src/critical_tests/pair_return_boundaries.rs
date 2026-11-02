//! `tests/test_pair_return_boundaries.py`: exact return-via sites on the
//! boundary of the narrow domain both signal vias of a pair allow.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Stackup, Via};
use copper_physical::CopperLayer::{Back as B, Front as F, In1, In2, In3, In4};
use copper_physical::{MechanicalHole, Point};
use indexmap::IndexSet;

use crate::pair_search::{HEADS, Port};
use crate::pair_vias::{TransitionTrace, return_positions, return_via, transition_spacing};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::test_support_critical::*;

struct Fixture {
    board: PhysicalBoard,
    rules: (NetRoutingRule, NetRoutingRule),
    pair: Vec<Via>,
    port: Port,
}

/// `fixture(angle, center, names)`: an unplaced six-layer board, a pair's
/// two signal vias `transition_spacing` apart about `center` (mm) along
/// `angle` degrees, both needing a return via within 0.75 mm.
fn fixture(angle: f64, center: (&str, &str), names: [&str; 3]) -> Fixture {
    let mut board =
        board("BoundaryDomain", 40, 25, Vec::new(), Vec::new(), names.iter().map(|n| net(n, &[])).collect());
    board.stackup = Stackup::new(vec![F, In1, In2, In3, In4, B]);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_hole_clearance_nm = 200_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    let member = |net: &str, partner: &str| NetRoutingRule {
        clearance_nm: Some(290_000),
        allowed_layers: vec![F, B],
        max_vias: Some(2),
        require_return_vias: true,
        return_via_net: Some(names[2].to_owned()),
        maximum_return_via_distance_nm: Some(750_000),
        ..differential_rule(net, partner, 145_000, 150_000)
    };
    let rules = (member(names[0], names[1]), member(names[1], names[0]));
    board.net_routing_rules = vec![rules.0.clone(), rules.1.clone()];
    let midpoint = at(nm(center.0), nm(center.1));
    let offset = (transition_spacing(&board, &rules.0, &rules.1) + 1) / 2;
    let (dx, dy) = (angle.to_radians().cos(), angle.to_radians().sin());
    let pair = [(names[0], -1.0), (names[1], 1.0)]
        .into_iter()
        .map(|(net, sign)| {
            let position = at(
                midpoint.x_nm + (sign * offset as f64 * dx).round() as i64,
                midpoint.y_nm + (sign * offset as f64 * dy).round() as i64,
            );
            through_via(net, position, 450_000, 200_000)
        })
        .collect();
    let port = Port { center: midpoint, heading: 0, sign: 1, first: Vec::new(), second: Vec::new(), preference: 0 };
    Fixture { board, rules, pair, port }
}

fn standard() -> Fixture {
    fixture(0.0, ("12.3", "8.7"), ["first", "second", "reference"])
}

fn can_via(index: &RoutingClearanceIndex<'_>, net: &str, position: Point) -> bool {
    index.can_via(net, position, 450_000, F, B, Some(200_000), false, None).expect("via check")
}

/// `reserved`: the signal vias, legal and locked in a fresh index.
fn reserved<'b>(board: &'b PhysicalBoard, pair: &[Via]) -> RoutingClearanceIndex<'b> {
    let mut index = RoutingClearanceIndex::new(board).expect("index");
    for via in pair {
        assert!(index.can_via(&via.net, via.position, via.size_nm, F, B, Some(via.drill_nm), false, None).unwrap());
        index.add_via(via.clone(), true, None).expect("reserved");
    }
    index
}

/// `old_positions`: the former fixed slots and 0.125 mm lattice.
fn old_positions(port: &Port) -> Vec<Point> {
    let (dx, dy) = HEADS[port.heading];
    let mut positions: IndexSet<Point> = IndexSet::new();
    for distance in ["0.8", "1", "1.25", "1.5", "2"] {
        for sign in [1, -1] {
            positions
                .insert(at(port.center.x_nm + sign * dx * nm(distance), port.center.y_nm + sign * dy * nm(distance)));
        }
    }
    for x in -16..=16i64 {
        for y in -16..=16i64 {
            if x != 0 || y != 0 {
                positions.insert(at(port.center.x_nm + x * 125_000, port.center.y_nm + y * 125_000));
            }
        }
    }
    positions.into_iter().collect()
}

fn propose(fixture: &Fixture, board: &PhysicalBoard, index: &RoutingClearanceIndex<'_>) -> Option<Vec<Via>> {
    propose_with(board, (&fixture.rules.0, &fixture.rules.1), &fixture.pair, &fixture.port, index)
}

fn propose_with(
    board: &PhysicalBoard,
    rules: (&NetRoutingRule, &NetRoutingRule),
    pair: &[Via],
    port: &Port,
    index: &RoutingClearanceIndex<'_>,
) -> Option<Vec<Via>> {
    return_via(board, index, port, pair, rules, F, B, &mut TransitionTrace::default(), None).expect("return via")
}

fn distance(a: Point, b: Point) -> f64 {
    ((a.x_nm - b.x_nm) as f64).hypot((a.y_nm - b.y_nm) as f64)
}

fn within(point: Point, pair: &[Via], limit: i64) -> bool {
    pair.iter().all(|via| distance(point, via.position) <= limit as f64)
}

fn with_rules(fixture: &mut Fixture, change: impl Fn(NetRoutingRule) -> NetRoutingRule) {
    fixture.rules = (change(fixture.rules.0.clone()), change(fixture.rules.1.clone()));
    fixture.board.net_routing_rules = vec![fixture.rules.0.clone(), fixture.rules.1.clone()];
}

fn all_clear(index: &RoutingClearanceIndex<'_>, pair: &[Via], returned: &[Via]) -> bool {
    let vias: Vec<Via> = pair.iter().chain(returned).cloned().collect();
    index.candidate_vias_clear(&vias).expect("clearance")
}

#[test]
fn old_grid_misses_nonempty_narrow_return_domain() {
    let fixture = standard();
    let index = reserved(&fixture.board, &fixture.pair);
    assert!(
        !old_positions(&fixture.port)
            .into_iter()
            .any(|p| within(p, &fixture.pair, 750_000) && can_via(&index, "reference", p))
    );
    let returned = propose(&fixture, &fixture.board, &index).expect("a return via");
    assert_eq!(returned.len(), 1);
    assert!(all_clear(&index, &fixture.pair, &returned));
    for via in &fixture.pair {
        let d = distance(returned[0].position, via.position);
        assert!((740_000.0..=750_000.0).contains(&d), "{d}");
    }
    assert!(fixture.board.tracks.is_empty() && fixture.board.vias.is_empty());
}

#[test]
fn boundary_sites_follow_actual_translated_rotated_vias() {
    for (angle, center, names) in [
        (0.0, ("12.3", "8.7"), ["P", "N", "foil"]),
        (90.0, ("24.625", "17.375"), ["east", "west", "return"]),
        (45.0, ("16.01", "11.003"), ["odd1", "odd2", "shell"]),
        (23.0, ("6.127", "5.319"), ["renamed-a", "renamed-b", "renamed-plane"]),
    ] {
        let fixture = fixture(angle, center, names);
        let index = reserved(&fixture.board, &fixture.pair);
        let before = index.additions().len();
        let first = propose(&fixture, &fixture.board, &index).unwrap_or_else(|| panic!("{names:?}"));
        assert_eq!(Some(&first), propose(&fixture, &fixture.board, &index).as_ref());
        assert_eq!(first[0].net, names[2]);
        assert_eq!((first[0].size_nm, first[0].drill_nm), (fixture.pair[0].size_nm, fixture.pair[0].drill_nm));
        assert_eq!((first[0].from_layer, first[0].to_layer), (F, B));
        assert_eq!(index.additions().len(), before);
        assert!(all_clear(&index, &fixture.pair, &first));
        let squared = |v: &Via| {
            let (dx, dy) = (
                (first[0].position.x_nm - v.position.x_nm) as i128,
                (first[0].position.y_nm - v.position.y_nm) as i128,
            );
            dx * dx + dy * dy
        };
        assert!(fixture.pair.iter().all(|v| squared(v) <= 750_000i128 * 750_000), "{names:?}");
    }
}

#[test]
fn sites_replace_futile_slots_without_increasing_existing_allowance() {
    for limit in ["0.75", "1.5", "3"] {
        let mut fixture = standard();
        with_rules(&mut fixture, |r| NetRoutingRule { maximum_return_via_distance_nm: Some(nm(limit)), ..r });
        let rules = (&fixture.rules.0, &fixture.rules.1);
        let positions = return_positions(&fixture.board, &fixture.port, &fixture.pair, rules, "reference", nm(limit));
        let old = old_positions(&fixture.port);
        assert_eq!(old.len(), 1090);
        assert!(positions.len() <= old.len(), "{limit}");
        assert_eq!(positions.iter().collect::<IndexSet<_>>().len(), positions.len());
        assert_eq!(
            positions,
            return_positions(&fixture.board, &fixture.port, &fixture.pair, rules, "reference", nm(limit))
        );
        // All old sites that can actually satisfy exact geometry remain present.
        let index = reserved(&fixture.board, &fixture.pair);
        for p in old {
            if within(p, &fixture.pair, nm(limit)) && can_via(&index, "reference", p) {
                assert!(positions.contains(&p), "{limit}: {p:?}");
            }
        }
    }
}

#[test]
fn actual_via_sizes_not_default_guess_drive_boundary_sites() {
    let mut fixture = fixture(17.0, ("12.3", "8.7"), ["first", "second", "reference"]);
    fixture.board.rules.default_via_size_nm = 800_000;
    fixture.pair[1].size_nm = 430_000;
    let index = reserved(&fixture.board, &fixture.pair);
    let returned = propose(&fixture, &fixture.board, &index).expect("a return via");
    assert_eq!(returned[0].size_nm, 450_000);
    assert!(all_clear(&index, &fixture.pair, &returned));
}

#[test]
fn impossible_distance_has_no_relaxed_fallback() {
    let mut fixture = standard();
    with_rules(&mut fixture, |r| NetRoutingRule { maximum_return_via_distance_nm: Some(730_000), ..r });
    let index = reserved(&fixture.board, &fixture.pair);
    assert!(propose(&fixture, &fixture.board, &index).is_none());
    assert!([&fixture.rules.0, &fixture.rules.1].iter().all(|r| r.maximum_return_via_distance_nm == Some(730_000)));
}

#[test]
fn wrong_via_process_cannot_become_a_return() {
    for process in ["inner_span", "unknown_technology", "filled_finish"] {
        let mut fixture = standard();
        let board = fixture.board.clone();
        let index = reserved(&board, &fixture.pair);
        match process {
            "inner_span" => fixture.pair[0].to_layer = In2,
            "unknown_technology" => fixture.pair[0].technology = Some("unqualified-process".to_owned()),
            _ => fixture.pair[0].finish = "filled-capped".to_owned(),
        }
        assert!(propose(&fixture, &board, &index).is_none(), "{process}");
    }
}

#[test]
fn wrong_net_binding_fails_closed() {
    for failure in ["foreign_signal", "missing_return", "different_returns"] {
        let mut fixture = standard();
        let board = fixture.board.clone();
        let index = reserved(&board, &fixture.pair);
        match failure {
            "foreign_signal" => fixture.pair[0].net = "unrelated".to_owned(),
            "missing_return" => {
                fixture.rules.0.return_via_net = Some("not-declared".to_owned());
                fixture.rules.1.return_via_net = Some("not-declared".to_owned());
            }
            _ => fixture.rules.1.return_via_net = Some("another-return".to_owned()),
        }
        assert!(propose(&fixture, &board, &index).is_none(), "{failure}");
    }
}

#[test]
fn old_geometry_index_cannot_authorize_current_source() {
    let fixture = standard();
    let index = reserved(&fixture.board, &fixture.pair);
    let mut new_source = fixture.board.clone();
    new_source.vias = vec![through_via("reference", mm(25, 20), 450_000, 200_000)];
    assert!(propose(&fixture, &new_source, &index).is_none());
    // Source occurrence multiplicity is not collapsed by a set check.
    let mut old_one = new_source.clone();
    old_one.vias = [new_source.vias.clone(), new_source.vias.clone()].concat();
    let new_index = reserved(&new_source, &fixture.pair);
    assert!(propose(&fixture, &old_one, &new_index).is_none());
}

#[test]
fn exact_all_layer_or_drill_blockers_reject_boundary_sites() {
    for blocker in ["inner_copper", "holes"] {
        let fixture = standard();
        let (cx, cy) = (fixture.port.center.x_nm, fixture.port.center.y_nm);
        let mut blocked = fixture.board.clone();
        if blocker == "inner_copper" {
            blocked.tracks = [1, -1]
                .into_iter()
                .map(|sign| {
                    track(
                        "obstacle",
                        at(cx - 2 * MM, cy + sign * 645_000),
                        at(cx + 2 * MM, cy + sign * 645_000),
                        100_000,
                        In3,
                    )
                })
                .collect();
            blocked.nets.push(net("obstacle", &[]));
        } else {
            blocked.mechanical_holes = [1, -1]
                .into_iter()
                .map(|sign| {
                    MechanicalHole::new(&sign.to_string(), at(cx, cy + sign * 645_000), 300_000, 0).expect("hole")
                })
                .collect();
        }
        let index = reserved(&blocked, &fixture.pair);
        assert!(index.candidate_vias_clear(&fixture.pair).unwrap());
        assert!(propose(&fixture, &blocked, &index).is_none(), "{blocker}");
        // An index with extra immutable prefix geometry is permitted, but its
        // constraints still reject returns; none of that prefix is removed.
        if blocker == "inner_copper" {
            let mut source = blocked.clone();
            source.tracks.clear();
            assert!(propose(&fixture, &source, &index).is_none());
        }
        assert!(blocked.vias.is_empty());
    }
}

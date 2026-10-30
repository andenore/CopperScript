//! `tests/test_pair_tuning_spans.py`: bounded candidate-span tuning with real
//! copper and ownership (`tune_virtual_pair_spans`). No global, detailed or
//! pair search: proposals use actual typed copper and the existing tuner,
//! clearance and profile checks.

use std::cell::RefCell;

use copper_drc::breakout::BreakoutRegions;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment, Via};
use copper_physical::CopperLayer::{self, Back as BACK, Front as FRONT};
use copper_physical::footprint::Size;
use indexmap::IndexMap;

use crate::pair_tuning::{PairTuning, tune_pair};
use crate::pair_tuning_spans::{Node, tune_virtual_pair_spans};
use crate::routing::{GlobalNetRoute, GridNode, PinAccess};
use crate::test_support_critical::*;

const W: i64 = 145_000;
const GAP: i64 = 150_000;
const SKEW: i64 = 63_500;
const AMP: i64 = 1_000_000;

type Lanes = (Vec<TrackSegment>, Vec<TrackSegment>);

/// `fixture(length, neck, spacing)`: pair A/B with 2 mm breakouts (neck
/// width `neck`); B's far land sits 451 703 nm further out, so B is longer.
fn fixture(length: i64, neck: i64, spacing: i64) -> (PhysicalBoard, Lanes) {
    let land = |number: &str, x: i64, y: i64| smd_pad(number, at(x, y), W);
    let body = Size { width_nm: MM, height_nm: MM };
    let start = footprint("start", vec![land("1", 0, 0), land("2", 0, spacing)], body);
    let end = footprint("end", vec![land("1", 0, 0), land("2", 451_703, spacing)], body);
    let poses = vec![pose("J1", "start", mm(5, 5)), pose("J2", "end", at(5 * MM + length, 5 * MM))];
    let nets = vec![net("A", &[("J1", "1"), ("J2", "1")]), net("B", &[("J1", "2"), ("J2", "2")]), net("OBSTACLE", &[])];
    let mut board = board("SpanFixture", 30, 15, vec![start, end], poses, nets);
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 200_000;
    board.net_routing_rules = [("A", "B"), ("B", "A")]
        .into_iter()
        .map(|(name, partner)| NetRoutingRule {
            clearance_nm: Some(290_000),
            max_skew_nm: Some(SKEW),
            max_vias: Some(2),
            max_length_nm: Some(63_600_000),
            tuning_amplitude_limit_nm: Some(AMP),
            breakout_length_nm: Some(2 * MM),
            breakout_width_nm: Some(neck),
            breakout_gap_nm: Some(100_000),
            breakout_clearance_nm: Some(100_000),
            ..differential_rule(name, partner, W, GAP)
        })
        .collect();
    let a = vec![track("A", mm(5, 5), at(5 * MM + length, 5 * MM), W, FRONT)];
    let b = vec![track("B", at(5 * MM, 5 * MM + spacing), at(5_451_703 + length, 5 * MM + spacing), W, FRONT)];
    let regions = BreakoutRegions::new(&board);
    let lanes = (regions.split_tracks(&a, Some(W)), regions.split_tracks(&b, Some(W)));
    (board, lanes)
}

fn standard() -> (PhysicalBoard, Lanes) {
    fixture(10 * MM, 120_000, W + GAP)
}

/// `fragment`: every horizontal piece cut at 0.5 mm marks.
fn fragment(lane: &[TrackSegment]) -> Vec<TrackSegment> {
    let mut out = Vec::new();
    for t in lane {
        let mut marks: Vec<i64> = (t.start.x_nm..t.end.x_nm).step_by(500_000).collect();
        marks.push(t.end.x_nm);
        for pair in marks.windows(2) {
            out.push(TrackSegment { start: at(pair[0], t.start.y_nm), end: at(pair[1], t.end.y_nm), ..t.clone() });
        }
    }
    out
}

fn fragments(lanes: &Lanes) -> Lanes {
    (fragment(&lanes.0), fragment(&lanes.1))
}

fn plain_tuner(a: &mut Vec<TrackSegment>, b: &mut Vec<TrackSegment>, skew: i64, limit: i64, margin: i64) -> PairTuning {
    tune_pair(a, b, skew, limit, margin, None)
}

fn length(tracks: &[TrackSegment]) -> i64 {
    let refs: Vec<&TrackSegment> = tracks.iter().collect();
    crate::critical::track_length(&refs)
}

/// `attempt`: the production fallback order; accepted legacy geometry is
/// never changed.
fn attempt(
    board: &PhysicalBoard,
    lanes: &Lanes,
    held: Option<&PhysicalBoard>,
    protected: &[Node],
    proposed: &[Via],
    amplitude: i64,
) -> (i64, Vec<TrackSegment>, Vec<TrackSegment>) {
    let (mut first, mut second) = lanes.clone();
    let added = if plain_tuner(&mut first, &mut second, SKEW, amplitude, 32).added_nm == 0 {
        tune_virtual_pair_spans(
            &mut first,
            &mut second,
            SKEW,
            amplitude,
            32,
            board,
            held.unwrap_or(board),
            protected,
            proposed,
            &plain_tuner,
        )
        .expect("span tuning")
    } else {
        length(&first) - length(&lanes.0)
    };
    (added, first, second)
}

fn simple(board: &PhysicalBoard, lanes: &Lanes) -> (i64, Vec<TrackSegment>, Vec<TrackSegment>) {
    attempt(board, lanes, None, &[], &[], AMP)
}

/// `guides`: deliberately not successful global routes; exact-construction
/// guides with the real source terminal identities.
fn guides(board: &PhysicalBoard) -> IndexMap<String, GlobalNetRoute> {
    let poses: IndexMap<&str, &copper_pcb::board::Placement> =
        board.placements.iter().map(|p| (p.reference.as_str(), p)).collect();
    board
        .nets
        .iter()
        .filter(|net| ["A", "B"].contains(&net.name.as_str()))
        .map(|net| {
            let accesses = net
                .pads
                .iter()
                .enumerate()
                .map(|(i, pad)| {
                    let pose = poses[pad.component.as_str()];
                    let land =
                        board.footprints[&pose.footprint].pads.iter().find(|p| p.number == pad.pad).expect("pad");
                    let position = copper_pcb::transform::transformed_local_point(pose, land.position);
                    PinAccess::new(pad.clone(), position, GridNode::new(0, i as i64, 0), position, FRONT)
                })
                .collect();
            let route = GlobalNetRoute {
                net: net.name.clone(),
                connected: false,
                accesses,
                segments: Vec::new(),
                vias: Vec::new(),
                length_nm: 0,
                diagnostics: vec!["exact-construction fixture; no global route claimed".to_owned()],
                deferred_to_zone: false,
            };
            (net.name.clone(), route)
        })
        .collect()
}

fn exact_pair(board: &PhysicalBoard, lanes: &Lanes) -> crate::critical::CriticalNetResult {
    let rules = &board.net_routing_rules;
    crate::critical::route_pair_exact(board, &rules[0], &rules[1], &guides(board), lanes, board, (&[], &[]))
        .expect("exact pair")
        .0
}

#[test]
fn fragmented_source_breakout_lanes_compensate_without_losing_anchors() {
    let (board, lanes) = standard();
    let parts = fragments(&lanes);
    let (mut a, mut b) = parts.clone();
    assert_eq!(plain_tuner(&mut a, &mut b, SKEW, AMP, 32).added_nm, 0);
    let (added, a, b) = simple(&board, &parts);
    assert!(added > 0 && (length(&a) - length(&b)).abs() <= SKEW, "{added}");
    assert_eq!(a[0].start, parts.0[0].start);
    assert_eq!(a.last().unwrap().end, parts.0.last().unwrap().end);
    assert_eq!(b, parts.1);
    let mut checked = board.clone();
    checked.tracks = a.iter().chain(&b).cloned().collect();
    let candidate = copper_drc::connectivity::Candidate { exclude_internal_connections: true, ..Default::default() };
    let graph = copper_drc::connectivity::explicit_copper_connectivity(&checked, candidate).expect("connectivity");
    assert!(board.nets.iter().filter(|n| n.name == "A" || n.name == "B").all(|n| graph.net_connected(n)));
    // Exact lane connectivity was checked above. The source scene has not
    // passed a complete routing pipeline, so its bookkeeping stays incomplete.
    let report = crate::drc_report::run_physical_drc(&checked, copper_drc::Policy::default()).expect("DRC");
    let errors: Vec<&str> = report
        .report
        .findings
        .iter()
        .filter(|f| f.severity == copper_drc::Severity::Error)
        .map(|f| f.code.as_str())
        .collect();
    assert_eq!(errors, ["DRC-ROUTE-INCOMPLETE"]);
    // Emitted regions are still cut and carry their exact source width.
    assert_eq!(BreakoutRegions::new(&board).split_tracks(&a, Some(W)), a);
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn exact_constructor_uses_fallback_only_with_full_held_context() {
    // Python's `tuning_board=None` call skipped the span fallback and left
    // the pair over its skew budget (451 703 nm, a warning); every Rust
    // caller passes the held board, so only the tuned outcome exists.
    let (board, lanes) = standard();
    let fixed = exact_pair(&board, &fragments(&lanes));
    assert!(fixed.connected && fixed.skew_nm <= SKEW && fixed.warnings.is_empty(), "{fixed:?}");
    assert!(fixed.tuned_length_nm > 0);
}

#[test]
fn legacy_success_never_enters_span_fallback() {
    // Python failed if the span fallback ran after the legacy tuner succeeded;
    // here the unfragmented lanes must tune exactly as the legacy tuner alone.
    let (board, lanes) = standard();
    let baseline = exact_pair(&board, &lanes);
    assert!(baseline.connected && baseline.tuned_length_nm > 0, "{baseline:?}");
    let (mut a, mut b) = lanes.clone();
    let regions = BreakoutRegions::new(&board);
    let legacy = tune_pair(&mut a, &mut b, SKEW, AMP, 32, Some(&regions)).added_nm;
    assert_eq!(baseline.tuned_length_nm, legacy);
}

#[test]
fn source_equal_subdivisions_do_not_acquire_disposable_ownership() {
    for place in ["source", "held", "source_reverse"] {
        let (board, lanes) = standard();
        let parts = fragments(&lanes);
        // Exact geometry covers all shorter-lane pieces; clones do not authorize it.
        let mut edge = track("A", parts.0[0].start, parts.0.last().unwrap().end, W, FRONT);
        if place == "source_reverse" {
            (edge.start, edge.end) = (edge.end, edge.start);
        }
        let mut with_edge = board.clone();
        with_edge.tracks = vec![edge.clone()];
        let (source, held) = if place == "held" { (board.clone(), with_edge) } else { (with_edge.clone(), with_edge) };
        let (added, a, b) = attempt(&source, &parts, Some(&held), &[], &[], AMP);
        assert_eq!(added, 0, "{place}");
        assert_eq!((a, b), parts, "{place}");
        assert_eq!(held.tracks, vec![edge]);
    }
}

#[test]
fn branch_is_unsupported_and_atomic() {
    let (board, lanes) = standard();
    let mut parts = fragments(&lanes);
    parts.0.push(track("A", mm(10, 5), at(10 * MM, 4_900_000), 120_000, FRONT));
    let (added, a, b) = simple(&board, &parts);
    assert_eq!(added, 0);
    assert_eq!((a, b), parts);
}

#[test]
fn explicit_contact_and_via_nodes_split_otherwise_virtual_runs() {
    let (board, lanes) = fixture(3 * MM, 120_000, 600_000);
    let parts = fragments(&lanes);
    // Two genuine transitions exhaust the source max_vias 2; no new allowance.
    let vias: Vec<Via> = [6 * MM, 7 * MM]
        .into_iter()
        .map(|x| Via { from_layer: FRONT, to_layer: BACK, ..through_via("A", at(x, 5 * MM), 450_000, 200_000) })
        .collect();
    let (added, a, b) = attempt(&board, &parts, None, &[], &vias, AMP);
    assert_eq!(added, 0);
    assert_eq!((a, b), parts);
    let nodes: Vec<Node> = parts.0[..parts.0.len() - 1].iter().map(|t| ("A".to_owned(), FRONT, t.end)).collect();
    let (added, a, b) = attempt(&board, &parts, None, &nodes, &[], AMP);
    assert_eq!(added, 0);
    assert_eq!((a, b), parts);
}

#[test]
fn equal_width_does_not_authorize_crossing_source_region_boundaries() {
    let (board, lanes) = fixture(10 * MM, W, W + GAP);
    let parts = fragments(&lanes);
    let captured: RefCell<Vec<TrackSegment>> = RefCell::new(Vec::new());
    let observer = |a: &mut Vec<TrackSegment>, b: &mut Vec<TrackSegment>, skew: i64, limit: i64, margin: i64| {
        captured.borrow_mut().extend(a.iter().cloned());
        plain_tuner(a, b, skew, limit, margin)
    };
    let (mut a, mut b) = parts.clone();
    tune_virtual_pair_spans(&mut a, &mut b, SKEW, AMP, 32, &board, &board, &[], &[], &observer).expect("tuning");
    let regions = BreakoutRegions::new(&board);
    let captured = captured.into_inner();
    assert!(!captured.is_empty());
    // Every virtual host remains one original source region, even if both
    // neighbouring regions use exactly the same width.
    for host in &captured {
        let owned: Vec<&TrackSegment> = parts
            .0
            .iter()
            .filter(|t| {
                t.start.y_nm == host.start.y_nm
                    && host.start.x_nm <= t.start.x_nm
                    && t.start.x_nm < t.end.x_nm
                    && t.end.x_nm <= host.end.x_nm
            })
            .collect();
        let mut owners: Vec<Option<String>> = owned.iter().map(|t| regions.region(&t.net, &[t.start, t.end])).collect();
        owners.dedup();
        assert!(owned.is_empty() || owners.len() == 1, "{host:?}: {owners:?}");
    }
}

#[test]
fn failed_or_blocked_compensation_preserves_exact_occurrences() {
    for reason in ["amplitude", "stale_intent", "lost_prefix", "blocked"] {
        let (board, lanes) = standard();
        let parts = fragments(&lanes);
        let (mut source, mut held) = (board.clone(), board.clone());
        let amplitude = if reason == "amplitude" { 0 } else { AMP };
        match reason {
            "stale_intent" => {
                held.metadata.insert("stale".to_owned(), "context".to_owned());
            }
            "lost_prefix" => {
                // Required occurrence multiplicity cannot be projected out of context.
                let edge = track("OBSTACLE", mm(20, 12), mm(21, 12), 100_000, FRONT);
                source.tracks = vec![edge.clone(), edge.clone()];
                held.tracks = vec![edge];
            }
            "blocked" => {
                held.tracks = vec![track("OBSTACLE", at(13 * MM, 4_750_000), at(14_800_000, 4_750_000), 100_000, FRONT)]
            }
            _ => {}
        }
        let (added, a, b) = attempt(&source, &parts, Some(&held), &[], &[], amplitude);
        assert_eq!(added, 0, "{reason}");
        assert_eq!((a, b), parts, "{reason}");
    }
}

#[test]
fn renaming_and_reversal_preserve_success_and_determinism() {
    let (board, lanes) = standard();
    let parts = fragments(&lanes);
    let first = simple(&board, &parts);
    assert_eq!(first, simple(&board, &parts));
    assert!(first.0 > 0);
    let reverse = |lane: &[TrackSegment]| -> Vec<TrackSegment> {
        lane.iter().rev().map(|t| TrackSegment { start: t.end, end: t.start, ..t.clone() }).collect()
    };
    let reversed = simple(&board, &(reverse(&parts.0), reverse(&parts.1)));
    assert!(reversed.0 > 0 && (length(&reversed.1) - length(&reversed.2)).abs() <= SKEW);
    let names = IndexMap::from([("A", "RENAMED_SIGNAL"), ("B", "RENAMED_PARTNER"), ("OBSTACLE", "OTHER")]);
    let mut renamed = board.clone();
    for net in &mut renamed.nets {
        net.name = names[net.name.as_str()].to_owned();
    }
    for rule in &mut renamed.net_routing_rules {
        rule.net = names[rule.net.as_str()].to_owned();
        rule.differential_partner = rule.differential_partner.as_deref().map(|p| names[p].to_owned());
    }
    let rename = |lane: &[TrackSegment]| -> Vec<TrackSegment> {
        lane.iter().map(|t| TrackSegment { net: names[t.net.as_str()].to_owned(), ..t.clone() }).collect()
    };
    let other = simple(&renamed, &(rename(&parts.0), rename(&parts.1)));
    assert_eq!(other.0, first.0);
}

#[test]
fn unordered_edges_ending_at_previous_start_are_not_merged() {
    for layer in [CopperLayer::Front, CopperLayer::Back] {
        let (mut board, _) = standard();
        board.placements.clear();
        board.nets.clear();
        board.net_routing_rules.clear();
        // A legal corner supplied in reverse list order. The second edge ends at
        // the first edge's start, but is not a forward continuation of that edge.
        let mut first = vec![track("A", mm(10, 5), mm(11, 5), W, layer), track("A", mm(10, 4), mm(10, 5), W, layer)];
        let mut second = vec![track("B", mm(5, 10), mm(20, 10), W, layer)];
        let before = (first.clone(), second.clone());
        let unexpected = |_: &mut Vec<TrackSegment>, _: &mut Vec<TrackSegment>, _: i64, _: i64, _: i64| -> PairTuning {
            panic!("no collinear span exists to tune")
        };
        let added =
            tune_virtual_pair_spans(&mut first, &mut second, SKEW, AMP, 32, &board, &board, &[], &[], &unexpected)
                .expect("tuning");
        assert_eq!(added, 0);
        assert_eq!((first, second), before);
    }
}

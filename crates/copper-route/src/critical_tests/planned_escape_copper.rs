//! `tests/test_planned_escape_copper.py`: promised exits join the ordinary
//! stage as copper, and the fanout keeps them.

use copper_json::Value;
use copper_pcb::board::{PhysicalBoard, Stackup, TrackSegment, Via};
use copper_physical::CopperLayer::{self, Back as BACK, Front as FRONT, In1, In2};
use copper_physical::footprint::Size;
use indexmap::IndexSet;

use crate::critical::{CriticalRoutingResult, CriticalRoutingStatus};
use crate::fanout::{FanoutOptions, route_fanout};
use crate::flow::{copper_keepouts, plane_pin_halos, release_escape_masks};
use crate::progress::Progress;
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

/// `test_unified_router._board(nets, layers)`: one 0.6 mm pad per point
/// (offset 3 mm) on a 26 x 26 mm board with 0.15 mm rules.
fn unified_board(nets: &[(&str, &[(i64, i64)])], layers: usize) -> PhysicalBoard {
    let one = footprint("test/one-pad", vec![smd_pad("1", at(0, 0), 600_000)], Size { width_nm: MM, height_nm: MM });
    let (mut poses, mut physical) = (Vec::new(), Vec::new());
    for (name, points) in nets {
        let references: Vec<String> = (0..points.len()).map(|i| format!("{name}_{i}")).collect();
        for (reference, (x, y)) in references.iter().zip(*points) {
            poses.push(pose(reference, "test/one-pad", mm(x + 3, y + 3)));
        }
        let pads: Vec<(&str, &str)> = references.iter().map(|r| (r.as_str(), "1")).collect();
        physical.push(net(name, &pads));
    }
    let mut board = board("Unified", 26, 26, vec![one], poses, physical);
    let mut stack = vec![FRONT];
    stack.extend([In1, In2].into_iter().take(layers - 2));
    stack.push(BACK);
    board.stackup = Stackup::new(stack);
    board.rules.minimum_clearance_nm = 150_000;
    board.rules.minimum_track_width_nm = 150_000;
    board.rules.default_track_width_nm = 200_000;
    board.rules.default_via_size_nm = 600_000;
    board.rules.default_via_drill_nm = 300_000;
    board.rules.minimum_hole_clearance_nm = 250_000;
    board
}

type Reserved = (Vec<TrackSegment>, Vec<Via>, Vec<copper_physical::CopperKeepout>);

/// `_reserved(board)`: S's promised stem and via, held as escape masks.
fn reserved(board: &PhysicalBoard) -> Reserved {
    let stem = track("S", mm(13, 10), mm(13, 12), 200_000, FRONT);
    let via = through_via("S", mm(13, 12), 600_000, 300_000);
    let keepouts =
        copper_keepouts(board, std::slice::from_ref(&stem), std::slice::from_ref(&via), "escape-reserve", Some(board));
    (vec![stem], vec![via], keepouts)
}

#[derive(Default)]
struct Events(Vec<(String, String, Vec<(&'static str, Value)>)>);

impl Progress for Events {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
        self.0.push((phase.to_owned(), event.to_owned(), details));
    }
}

impl Events {
    fn joined(&self) -> Vec<&Vec<(&'static str, Value)>> {
        self.0.iter().filter(|(p, e, _)| p == "ordinary_escapes" && e == "joined").map(|(_, _, d)| d).collect()
    }
}

fn detail<'a>(details: &'a [(&'static str, Value)], key: &str) -> &'a Value {
    &details.iter().find(|(k, _)| *k == key).expect("a detail").1
}

fn critical(board: &PhysicalBoard) -> CriticalRoutingResult {
    CriticalRoutingResult {
        status: CriticalRoutingStatus::Success,
        board: board.clone(),
        nets: Vec::new(),
        locked_tracks: Vec::new(),
        locked_vias: Vec::new(),
        global_routing_fingerprint: String::new(),
        routing_fingerprint: String::new(),
        reserved_track_count: 0,
        reserved_via_count: 0,
        bundles: Vec::new(),
        match_tuning: Vec::new(),
        plane_reservation_track_count: 0,
        plane_reservation_via_count: 0,
    }
}

fn release(
    reserved: &Reserved,
    masked: &PhysicalBoard,
    pre: PhysicalBoard,
    options: &UnifiedRouterOptions,
    events: &mut Events,
) -> PhysicalBoard {
    let mut result = critical(masked);
    let mut observer: Option<crate::progress::ProgressCallback<'_>> = Some(events);
    release_escape_masks(Some(reserved), &mut result, &mut None, pre, options, &mut observer).expect("released")
}

#[test]
fn planned_escapes_join_the_ordinary_board_exact_checked() {
    let board = unified_board(&[("S", &[(10, 7), (10, 17)]), ("C", &[(1, 9), (19, 9)])], 2);
    let reserved = reserved(&board);
    let mut masked = board.clone();
    masked.copper_keepouts = reserved.2.clone();
    let options = UnifiedRouterOptions::default();
    let mut events = Events::default();
    let pre = release(&reserved, &masked, masked.clone(), &options, &mut events);
    // The masks are gone and the promised stem and via are the net's copper.
    assert!(!pre.copper_keepouts.iter().any(|k| k.id.starts_with("escape-reserve")));
    assert!(pre.tracks.contains(&reserved.0[0]) && pre.vias.contains(&reserved.1[0]));
    let joined = events.joined();
    assert_eq!(joined.len(), 1);
    let expected: Vec<(&str, Value)> = vec![
        ("tracks", Value::from(1i64)),
        ("vias", Value::from(1i64)),
        ("reused_tracks", Value::from(0i64)),
        ("reused_vias", Value::from(0i64)),
        ("dropped_tracks", Value::from(0i64)),
        ("dropped_vias", Value::from(0i64)),
        ("dropped_nets", Value::Array(Vec::new())),
    ];
    assert_eq!(joined[0], &expected);
    // A promised via another step already placed on its site is reused: no
    // second drill at the same point.
    let mut events = Events::default();
    let mut with_via = masked.clone();
    with_via.vias = reserved.1.clone();
    let reused = release(&reserved, &masked, with_via, &options, &mut events);
    assert_eq!(reused.vias.iter().filter(|v| **v == reserved.1[0]).count(), 1);
    assert!(reused.tracks.contains(&reserved.0[0]));
    let joined = events.joined()[0];
    let counts: Vec<&Value> =
        ["vias", "reused_vias", "dropped_vias", "tracks"].iter().map(|k| detail(joined, k)).collect();
    assert_eq!(counts, [0i64, 1, 0, 1].map(Value::from).iter().collect::<Vec<_>>());
    // Off: the masks go and the copper stays out, as before.
    let off = UnifiedRouterOptions { planned_escapes_as_copper: false, ..UnifiedRouterOptions::default() };
    let plain = release(&reserved, &masked, masked.clone(), &off, &mut Events::default());
    assert!(plain.tracks == board.tracks && plain.vias == board.vias);
    // A promised via a preparation step has since made illegal is left out
    // and counted; the legal stem still joins.
    let crossing = track("C", mm(12, 12), mm(14, 12), 200_000, FRONT);
    let mut crossed = masked.clone();
    crossed.tracks = vec![crossing];
    let mut events = Events::default();
    let cut = release(&reserved, &masked, crossed, &options, &mut events);
    assert!(!cut.vias.contains(&reserved.1[0]));
    let joined = events.joined()[0];
    assert_eq!(detail(joined, "dropped_vias"), &Value::from(1i64));
    let count = |key: &str| match detail(joined, key) {
        Value::Int(n) => n.to_string().parse::<i64>().expect("a count"),
        other => panic!("{other:?}"),
    };
    assert_eq!(count("dropped_tracks") + count("tracks"), 1);
    assert_eq!(detail(joined, "dropped_nets"), &Value::Array(vec![Value::from("S")]));
}

/// The items of `after` beyond those of `before` (`Counter` difference).
fn added<T: PartialEq + Clone>(after: &[T], before: &[T]) -> Vec<T> {
    let mut remaining = before.to_vec();
    let mut result = Vec::new();
    for item in after {
        if let Some(i) = remaining.iter().position(|other| other == item) {
            remaining.remove(i);
        } else {
            result.push(item.clone());
        }
    }
    result
}

#[test]
fn the_ordinary_stage_fanout_reuses_the_joined_planned_escapes() {
    let mut board = unified_board(&[("S", &[(10, 7), (10, 17)]), ("C", &[(1, 9), (19, 9)])], 4);
    board.footprints["test/one-pad"].pads.push(smd_pad("2", mm(-1, 0), 600_000));
    let options = FanoutOptions { minimum_component_pads: 1, ..FanoutOptions::default() };
    let only = IndexSet::from(["S".to_owned()]);
    // The planning stage's escapes, reserved as masks while critical nets route.
    let planned = route_fanout(&board, Some(&options), Some(&only)).expect("fanout");
    let tracks = added(&planned.board.tracks, &board.tracks);
    let vias = added(&planned.board.vias, &board.vias);
    assert!(!tracks.is_empty() && !vias.is_empty());
    let keepouts = copper_keepouts(&board, &tracks, &vias, "escape-reserve", Some(&board));
    let reserved = (tracks, vias, keepouts);
    let mut masked = board.clone();
    masked.copper_keepouts = reserved.2.clone();
    let pre = release(&reserved, &masked, masked.clone(), &UnifiedRouterOptions::default(), &mut Events::default());
    // The ordinary stage's fanout, with the same options, finds its exits
    // already there: the same accesses, no new copper.
    let again = route_fanout(&pre, Some(&options), Some(&only)).expect("fanout");
    assert_eq!(again.accesses, planned.accesses);
    assert!(again.created_tracks.is_empty() && again.created_vias.is_empty());
    assert!(again.board == pre);
}

#[test]
fn fine_pitch_plane_pins_keep_a_via_site_free_from_planned_escapes() {
    // A 0.5 mm row on the right edge of a package: P's plane is on B.Cu
    // only, so P needs a stitching via in front of its land; G has a pour on
    // its own layer and S is a signal.
    let land = |number: &str, y: i64| copper_physical::footprint::FootprintPad {
        size: Size { width_nm: 600_000, height_nm: 250_000 },
        ..smd_pad(number, at(1_400_000, y), 600_000)
    };
    let package = footprint(
        "test/row",
        vec![land("1", -500_000), land("2", 0), land("3", 500_000)],
        Size { width_nm: 3 * MM, height_nm: 3 * MM },
    );
    let nets = vec![net("S", &[("U1", "1")]), net("P", &[("U1", "2")]), net("G", &[("U1", "3")])];
    let mut board = board("PlanePins", 20, 20, vec![package], vec![pose("U1", "test/row", mm(10, 10))], nets);
    board.stackup = Stackup::new(vec![FRONT, In1, In2, BACK]);
    let plane = rectangle(0, 0, 20 * MM, 20 * MM);
    board.zones = vec![zone("p-plane", "P", &[BACK], plane.clone()), zone("g-pour", "G", &[FRONT, In1], plane)];
    // With 0.8 mm vias and 0.2 mm clearance a via in front of P would block
    // its neighbours' straight escapes at this pitch: no halo.
    assert!(plane_pin_halos(&board).is_empty());
    board.rules.minimum_clearance_nm = 100_000;
    board.rules.minimum_track_width_nm = 100_000;
    board.rules.default_track_width_nm = 150_000;
    board.rules.default_via_size_nm = 450_000;
    board.rules.default_via_drill_nm = 200_000;
    let halos = plane_pin_halos(&board);
    assert_eq!(halos.len(), 1);
    let halo = &halos[0];
    assert!(halo.layers == [CopperLayer::Front] && halo.block_vias && halo.block_tracks);
    let xs: Vec<i64> = halo.vertices.iter().map(|v| v.x_nm).collect();
    let ys: Vec<i64> = halo.vertices.iter().map(|v| v.y_nm).collect();
    let (gap, via) = (board.rules.minimum_clearance_nm, board.rules.default_via_size_nm);
    let range = |values: &[i64]| (*values.iter().min().unwrap(), *values.iter().max().unwrap());
    // From one clearance past P's outer edge (x = 11.7 mm), a via and a
    // clearance outward, one via plus clearance either side of its centre.
    assert_eq!(range(&xs), (11_700_000 + gap, 11_700_000 + 2 * gap + via));
    assert_eq!(range(&ys), (10 * MM - via / 2 - gap, 10 * MM + via / 2 + gap));
}

//! One exact clearance index across commit, repair and pour re-check, and
//! via technology spans in the unified router's copper (ported from the
//! former `tests/test_unified_commit_index.py`).

mod support_routers;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, PhysicalNet, TrackSegment, Via, ViaKind, ViaTechnology};
use copper_physical::CopperLayer;
use copper_route::detailed::DetailedRoutingResult;
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::routing_vias::physical_via_span;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use copper_route::unified_router::router::{NetRoute, Router};
use copper_units::Decimal;
use indexmap::{IndexMap, IndexSet};
use support_routers::routes::{drc, guides};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;
const IN1: CopperLayer = CopperLayer::In1;
const IN2: CopperLayer = CopperLayer::In2;

fn opts() -> UnifiedRouterOptions {
    UnifiedRouterOptions::default()
}

/// Crossing horizontal and vertical nets: one negotiation iteration leaves
/// exact clashes that the repair rounds have to rip up and settle.
fn grid_board() -> PhysicalBoard {
    let mut owned = Vec::new();
    for i in 0..7 {
        let at = 3.0 + 2.3 * i as f64;
        owned.push((format!("H{i}"), [(2.0, at), (18.0, at)]));
        owned.push((format!("V{i}"), [(at, 2.0), (at, 18.0)]));
    }
    let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
    pad_board(&nets, 2, vec![])
}

type Seeder<'a> = copper_route::unified_router::route::EscapeSeeder<'a>;

fn run(
    board: &PhysicalBoard,
    options: &UnifiedRouterOptions,
    seeder: Option<&mut Seeder<'_>>,
) -> (DetailedRoutingResult, Events) {
    let mut events = Events::default();
    let result =
        route_unified(board, &guides(board), options, None, None, None, seeder, Some(&mut events)).expect("route");
    (result, events)
}

fn same_result(first: &DetailedRoutingResult, second: &DetailedRoutingResult) {
    assert_eq!(first.board.tracks, second.board.tracks);
    assert_eq!(first.board.vias, second.board.vias);
    let summary = |r: &DetailedRoutingResult| {
        r.nets.iter().map(|n| (n.net.clone(), n.connected, n.track_count, n.via_count, n.length_nm)).collect::<Vec<_>>()
    };
    assert_eq!(summary(first), summary(second));
}

fn with(board: &PhysicalBoard, held: &[(Vec<TrackSegment>, Vec<Via>)]) -> PhysicalBoard {
    let mut copy = board.clone();
    for (tracks, vias) in held {
        copy.tracks.extend(tracks.iter().cloned());
        copy.vias.extend(vias.iter().cloned());
    }
    copy
}

#[test]
fn a_router_refuses_an_index_that_does_not_hold_its_board() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let mut index = RoutingClearanceIndex::new(&board).expect("index");
    index.add_track(track("A", (6.0, 13.0), (9.0, 13.0), 0.2, F), false, None).expect("add");
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    assert!(Router::new(&board, &opts(), &nets, IndexSet::new(), None, index).is_err());
    let fresh = RoutingClearanceIndex::new(&board).expect("index");
    assert!(Router::new(&board, &opts(), &nets, IndexSet::new(), None, fresh).is_ok());
}

#[test]
fn repair_on_the_shared_index_commits_the_same_copper() {
    // Python also counted index builds and removals by patching the index.
    let board = grid_board();
    for iterations in [1, 2] {
        let base = UnifiedRouterOptions { maximum_iterations: iterations, commit_rounds: 2, ..opts() };
        let (fresh, _) = run(&board, &UnifiedRouterOptions { share_commit_index: false, ..base.clone() }, None);
        let (shared, events) = run(&board, &base, None);
        assert!(
            events
                .of("unified_commit", "finished")
                .iter()
                .any(|d| int_of(detail(d, "round")) == 2 && detail(d, "adopted") == &copper_json::Value::Bool(true)),
            "iterations {iterations}"
        );
        same_result(&fresh, &shared);
    }
}

#[test]
fn seeded_repair_shares_the_index_only_for_pure_copper_additions() {
    // A spans an impassable wall while B can connect on its left.
    let mut board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 4.0), (6.0, 4.0)])], 2, vec![]);
    board.copper_keepouts = vec![copper_physical::CopperKeepout {
        block_zones: false,
        ..copper_keepout("wall", &[F, B], rect(12.8, 0.0, 13.2, 26.0))
    }];
    let mut seen = 0;
    let mut adds_nothing = |partial: &PhysicalBoard, _: &IndexSet<String>| {
        seen += 1;
        Ok(partial.clone())
    };
    let options = UnifiedRouterOptions { maximum_iterations: 1, commit_rounds: 3, ..opts() };
    let (reference, _) =
        run(&board, &UnifiedRouterOptions { share_commit_index: false, ..options.clone() }, Some(&mut adds_nothing));
    assert!(!reference.board.tracks.is_empty());
    let connected: IndexSet<&str> = reference.nets.iter().filter(|n| n.connected).map(|n| n.net.as_str()).collect();
    assert_eq!(connected, IndexSet::from(["B"]));
    let (result, _) = run(&board, &options, Some(&mut adds_nothing));
    same_result(&reference, &result);
    let mut touches_metadata = |partial: &PhysicalBoard, _: &IndexSet<String>| {
        let mut changed = partial.clone();
        changed.metadata.insert("seeded".to_owned(), "yes".to_owned());
        Ok(changed)
    };
    let (result, _) = run(&board, &options, Some(&mut touches_metadata));
    same_result(&reference, &result);
    assert!(seen > 0);
}

#[test]
fn pour_recheck_on_the_shared_index_matches_fresh_indexes() {
    // `tests/test_unified_router.py::_walled_plane_pad_board`.
    let mut board = pad_board(&[("GND", &[(10.0, 10.0), (20.0, 2.0)]), ("A", &[(11.9, 5.0), (11.9, 15.0)])], 2, vec![]);
    let corners = [(14.5, 11.5), (11.5, 11.5), (11.5, 14.5), (14.5, 14.5)];
    board.tracks = corners.windows(2).map(|pair| track("W", pair[0], pair[1], 0.2, F)).collect();
    board.vias = [3.0, 18.0].iter().flat_map(|x| [3.0, 23.0].map(|y| via("GND", (*x, y), 0.6, 0.3, F, B))).collect();
    board.zones = vec![
        zone("gnd-f", "GND", &[F], rect(0.5, 0.5, 20.0, 25.5)),
        zone("gnd-b", "GND", &[B], rect(0.5, 0.5, 25.5, 25.5)),
    ];
    board.nets.push(net("W", &[]));
    let (fresh, _) = run(&board, &UnifiedRouterOptions { share_commit_index: false, ..opts() }, None);
    let (shared, events) = run(&board, &opts(), None);
    let recheck = events.phase("unified_pour_recheck")[0];
    assert_eq!(detail(recheck, "adopted"), &copper_json::Value::Bool(true));
    assert!(!detail(recheck, "steps").as_array().expect("steps").is_empty());
    same_result(&fresh, &shared);
}

#[test]
fn repair_rejections_name_this_calls_copper_as_movable() {
    // B's copper crosses A's straight path on both layers.
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(10.0, 3.0), (10.0, 17.0)])], 2, vec![]);
    let held = vec![(
        vec![track("B", (13.0, 6.0), (13.0, 20.0), 0.2, F), track("B", (13.0, 6.0), (13.0, 20.0), 0.2, B)],
        Vec::new(),
    )];
    let partial = with(&board, &held);
    // The shared index: the base board plus this call's mutable copper.
    let mut index = RoutingClearanceIndex::new(&board).expect("index");
    for t in &held[0].0 {
        index.add_track(t.clone(), false, None).expect("add");
    }
    let nets: Vec<&PhysicalNet> = board.nets.iter().filter(|n| n.name == "A").collect();
    let options = UnifiedRouterOptions { maximum_iterations: 1, ..opts() };
    let mut router = Router::new(&partial, &options, &nets, IndexSet::new(), None, index).expect("router");
    let g = router.lattice.clone();
    let iy = ((mm(13.0) - g.y0) as f64 / g.pitch as f64).round() as i64;
    let path: Vec<i64> =
        [7.0, 19.0].iter().map(|x| iy * g.nx + ((mm(*x) - g.x0) as f64 / g.pitch as f64).round() as i64).collect();
    let route = NetRoute {
        paths: vec![path.clone()],
        leads: vec![Vec::new()],
        nodes: path.iter().copied().collect(),
        length: 1.0,
        ..NetRoute::empty(true, "")
    };
    let routes = IndexMap::from([("A".to_owned(), route)]);
    let (_, rejected, complete) = router.commit(&routes, &IndexSet::new()).expect("commit");
    assert!(rejected.contains_key("A") && !complete.contains("A"));
    assert!(router.rejections["A"].iter().any(|(_, _, blockers, _, _)| blockers.contains(&"B".to_owned())));
}

// -- via technology spans

fn technology(id: &str, kind: ViaKind, first: CopperLayer, second: CopperLayer) -> ViaTechnology {
    ViaTechnology {
        id: id.to_owned(),
        kind,
        from_layer: first,
        to_layer: second,
        minimum_drill_nm: mm(0.1),
        minimum_annular_ring_nm: mm(0.1),
        maximum_aspect_ratio: Decimal::from_i64(12),
    }
}

fn through() -> ViaTechnology {
    technology("through", ViaKind::Through, F, B)
}

fn blind() -> ViaTechnology {
    technology("blind-f1", ViaKind::Blind, F, IN1)
}

fn four_layer(technologies: Vec<ViaTechnology>, allowed: Option<Vec<CopperLayer>>) -> PhysicalBoard {
    let mut board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 4, vec![]);
    board.stackup.via_technologies = technologies;
    if let Some(layers) = allowed {
        // A wall across F.Cu: A must change layers within `layers`.
        board.tracks = vec![track("W", (13.0, 1.5), (13.0, 24.5), 0.2, F)];
        board.nets.push(net("W", &[]));
        board.net_routing_rules = vec![NetRoutingRule { allowed_layers: layers, ..rule("A") }];
    }
    board
}

fn router_of(board: &PhysicalBoard) -> Router<'_, '_> {
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    Router::new(board, &opts(), &nets, IndexSet::new(), None, RoutingClearanceIndex::new(board).expect("index"))
        .expect("router")
}

/// A path that changes from F.Cu to `layer` at the centre, then runs 4 nodes.
fn layer_change(router: &Router<'_, '_>, layer: i64) -> (Vec<i64>, i64) {
    let g = &router.lattice;
    let (ix, iy) = (g.nx / 2, g.ny / 2);
    let node = |l: i64, x: i64| l * g.plane + iy * g.nx + x;
    (vec![node(0, ix), node(layer, ix), node(layer, ix + 4)], iy * g.nx + ix)
}

#[test]
fn copper_emits_the_physical_via_span_of_each_layer_change() {
    let board = four_layer(vec![through(), blind()], None);
    for (layer, second) in [(1, IN1), (2, IN2), (3, B)] {
        let mut router = router_of(&board);
        let (path, xy) = layer_change(&router, layer);
        let (_, vias) = router.copper(0, &path, &Vec::new(), None).expect("copper");
        let expected = physical_via_span(&board, F, second).expect("span");
        let spans: Vec<_> = vias.iter().map(|v| (v.from_layer, v.to_layer, v.technology.clone())).collect();
        assert_eq!(spans, std::slice::from_ref(&expected));
        let mut route = NetRoute { paths: vec![path], leads: vec![Vec::new()], ..NetRoute::empty(true, "") };
        router.finish_route(&mut route, 0);
        assert_eq!(route.via_xy, IndexSet::from([xy]));
        let layers = &board.stackup.copper_layers;
        let index = |l: CopperLayer| layers.iter().position(|x| *x == l).expect("layer") as i64;
        let (low, high) = (index(expected.0).min(index(expected.1)), index(expected.0).max(index(expected.1)));
        let claimed: IndexSet<i64> = route.claims.iter().map(|n| n / router.lattice.plane).collect();
        let mut expected_layers: IndexSet<i64> = (low..=high).collect();
        expected_layers.insert(layer);
        // A blind via claims only its own layers; a through via all four.
        assert_eq!(
            claimed.iter().copied().collect::<std::collections::BTreeSet<_>>(),
            expected_layers.into_iter().collect()
        );
    }
}

#[test]
fn without_technologies_every_via_is_the_former_through_via() {
    let board = four_layer(Vec::new(), None);
    for layer in 1..=3 {
        let mut router = router_of(&board);
        let (path, xy) = layer_change(&router, layer);
        let (_, vias) = router.copper(0, &path, &Vec::new(), None).expect("copper");
        let spec = &router.specs[0];
        let at = router.lattice.column_point(xy);
        assert_eq!(
            vias,
            [Via {
                net: "A".to_owned(),
                position: at,
                size_nm: spec.via_size,
                drill_nm: spec.via_drill,
                from_layer: F,
                to_layer: B,
                technology: None,
                finish: "standard".to_owned()
            }]
        );
        let mut route = NetRoute { paths: vec![path], leads: vec![Vec::new()], ..NetRoute::empty(true, "") };
        router.finish_route(&mut route, 0);
        let claimed: std::collections::BTreeSet<i64> = route.claims.iter().map(|n| n / router.lattice.plane).collect();
        assert_eq!(claimed, (0..4).collect());
    }
}

#[test]
fn layer_changes_at_one_site_share_one_via_spanning_them() {
    let board = four_layer(vec![through(), blind()], None);
    let router = router_of(&board);
    let g = &router.lattice;
    let (ix, iy) = (g.nx / 2, g.ny / 2);
    let node = |l: i64, x: i64| l * g.plane + iy * g.nx + x;
    let first = vec![node(0, ix), node(1, ix)];
    let second = vec![node(1, ix), node(2, ix), node(2, ix + 3)];
    let spans = router.via_sites("A", &[first, second]);
    assert_eq!(spans.len(), 1);
    let (site, span) = spans.first().expect("a site");
    assert_eq!(*site, iy * g.nx + ix);
    let expected = physical_via_span(&board, F, IN2).expect("span");
    let span = span.as_ref().expect("a span");
    assert_eq!((span.0, span.1, span.2.clone()), expected);
}

#[test]
fn routed_blind_vias_follow_the_declared_technology() {
    let board = four_layer(vec![through(), blind()], Some(vec![F, IN1]));
    let (result, _) = run(&board, &opts(), None);
    assert!(result.nets[0].connected);
    let vias: Vec<&Via> = result.board.vias.iter().filter(|v| v.net == "A").collect();
    assert!(!vias.is_empty());
    assert!(vias.iter().all(|v| (v.from_layer, v.to_layer, v.technology.as_deref()) == (F, IN1, Some("blind-f1"))));
    let report = drc(&result.board);
    let summary = copper_route::route_summary::connection_summary(&result.board, Some(&report)).expect("summary");
    assert_eq!(summary_int(&summary, "drc", "errors"), 0);
}

#[test]
fn a_layer_change_no_technology_serves_is_rejected_with_its_reason() {
    let mut board = four_layer(vec![blind()], Some(vec![F, B]));
    board.tracks.clear();
    board.copper_keepouts = vec![copper_physical::CopperKeepout {
        block_vias: false,
        block_zones: false,
        ..copper_keepout("wall", &[F], rect(12.8, 0.0, 13.2, 26.0))
    }];
    assert!(physical_via_span(&board, F, B).is_none());
    let (result, _) = run(&board, &opts(), None);
    let net = result.nets.iter().find(|n| n.net == "A").expect("A");
    assert!(!net.connected && net.diagnostics[0].contains("no_via_technology"), "{:?}", net.diagnostics);
    assert!(!result.board.vias.iter().any(|v| v.net == "A"));
}

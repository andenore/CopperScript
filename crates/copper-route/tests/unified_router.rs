//! Behaviour of the unified negotiated-congestion router on small synthetic
//! boards (ported from the former `tests/test_unified_router.py`; the tests of
//! crate-private helpers live in `src/unified_router/tests.rs`).

mod support_routers;

use copper_pcb::board::{NetMatchGroup, PhysicalNet, Via};
use copper_physical::{BoardSide, Bounds, CopperLayer, DesignRules};
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::routing_costs::LayerBias;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::rasters::{FOREIGN, FREE};
use copper_route::unified_router::route::route_unified;
use copper_route::unified_router::router::Router;
use copper_route::unified_router::search::Targets;
use copper_route::unified_router::{ReplayRequest, replay_unified};
use indexmap::{IndexMap, IndexSet};
use support_routers::routes::{Routed, drc, errors, errors_except_opens, global, guides, route, route_with};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const B: CopperLayer = CopperLayer::Back;

fn opts() -> UnifiedRouterOptions {
    UnifiedRouterOptions::default()
}

fn names(items: &[&str]) -> IndexSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn clean(routed: &Routed) {
    assert_eq!(routed.opens(), 0, "opens: {:?}", routed.summary);
    assert_eq!(routed.errors(), 0, "DRC errors: {:?}", errors(&routed.drc));
}

fn window(values: [f64; 4]) -> Option<(i64, i64, i64, i64)> {
    Some((mm(values[0]), mm(values[1]), mm(values[2]), mm(values[3])))
}

fn new_router<'p>(board: &'p PhysicalBoard, options: &UnifiedRouterOptions, nets: &[&PhysicalNet]) -> Router<'p, 'p> {
    let index = RoutingClearanceIndex::new(board).expect("index");
    Router::new(board, options, nets, IndexSet::new(), None, index).expect("router")
}

use copper_pcb::board::PhysicalBoard;

#[test]
fn single_net_routes_with_exact_clean_copper() {
    let routed = route(&pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]), &opts());
    clean(&routed);
    assert!(routed.result.nets.iter().all(|net| net.connected));
    assert_eq!(routed.result.board.metadata["detailed_router"], "unified");
}

#[test]
fn crossing_nets_negotiate_through_a_second_layer() {
    // A horizontal and a vertical net must cross: one needs vias.
    let board = pad_board(&[("H", &[(2.0, 10.0), (18.0, 10.0)]), ("V", &[(10.0, 2.0), (10.0, 18.0)])], 2, vec![]);
    let routed = route(&board, &opts());
    clean(&routed);
    assert!(routed.result.metrics.via_count >= 2);
}

#[test]
fn many_parallel_crossings_converge_without_conflicts() {
    let mut owned: Vec<(String, [(f64, f64); 2])> = Vec::new();
    for i in 0..5 {
        owned.push((format!("H{i}"), [(2.0, 4.0 + 2.0 * i as f64), (18.0, 4.0 + 2.0 * i as f64)]));
    }
    for i in 0..5 {
        owned.push((format!("V{i}"), [(4.0 + 2.5 * i as f64, 2.0), (4.0 + 2.5 * i as f64, 18.0)]));
    }
    let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
    clean(&route(&pad_board(&nets, 2, vec![]), &opts()));
}

#[test]
fn existing_copper_is_fixed_and_avoided() {
    let wall: Vec<_> = [F, B].iter().map(|layer| track("BLOCK", (13.0, 3.5), (13.0, 22.5), 0.3, *layer)).collect();
    let board =
        pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("BLOCK", &[(10.0, 0.5), (10.0, 19.5)])], 2, wall.clone());
    let routed = route(&board, &opts());
    // The wall net is complete already; A must go around it, not through.
    assert_eq!(routed.errors(), 0, "{:?}", errors(&routed.drc));
    assert!(wall.iter().all(|t| routed.result.board.tracks.contains(t)));
}

#[test]
fn unroutable_net_is_reported_not_raised() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let routed = route(&board, &UnifiedRouterOptions { maximum_iterations: 1, maximum_expansions: 5, ..opts() });
    assert_eq!(summary_int(&routed.summary, "signal", "connections"), 1);
    assert!(!routed.result.nets[0].connected || routed.opens() == 0);
}

/// Sleeps past the time budget as the first unseeded repair ends, before a
/// seeded alternative could start (Python patched `perf_counter` instead).
struct ExpireAfterRepair {
    events: Events,
    sleep: std::time::Duration,
}

impl copper_route::Progress for ExpireAfterRepair {
    fn observe(&mut self, phase: &str, event: &str, details: Vec<(&'static str, copper_json::Value)>) {
        self.events.observe(phase, event, details);
        if phase == "unified_iteration"
            && event == "finished"
            && self.events.count("unified_iteration", "finished") == 2
        {
            std::thread::sleep(self.sleep);
        }
    }
}

#[test]
fn expired_repair_budget_does_not_start_another_escape_seed() {
    let fixed = track("B", (6.0, 5.0), (20.0, 5.0), 0.2, F);
    let board =
        pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 2.0), (17.0, 2.0)])], 2, vec![fixed.clone()]);
    let guides = guides(&board);
    let mut seeds: Vec<IndexSet<String>> = Vec::new();
    let mut seed = |partial: &PhysicalBoard, nets: &IndexSet<String>| {
        seeds.push(nets.clone());
        Ok(partial.clone())
    };
    let options = UnifiedRouterOptions {
        maximum_iterations: 1,
        maximum_expansions: 1,
        commit_rounds: 2,
        time_budget_s: Some(2.0),
        ..opts()
    };
    let mut observer = ExpireAfterRepair { events: Events::default(), sleep: std::time::Duration::from_millis(2100) };
    let result = route_unified(&board, &guides, &options, None, None, None, Some(&mut seed), Some(&mut observer))
        .expect("route");
    assert!(seeds.is_empty());
    assert_eq!(observer.events.count("unified_iteration", "finished"), 2);
    assert!(result.board.tracks.contains(&fixed) && result.board.placements == board.placements);
    assert!(result.nets.iter().find(|n| n.net == "B").expect("B").connected);
    assert!(!result.nets.iter().find(|n| n.net == "A").expect("A").connected);
    assert!(
        observer
            .events
            .of("unified_repair", "skipped")
            .iter()
            .any(|d| detail(d, "reason").as_str() == Some("time_budget"))
    );
}

#[test]
fn local_grid_window_preserves_whole_source_and_exact_copper() {
    let fixed = track("B", (6.0, 5.0), (20.0, 5.0), 0.2, F);
    let board =
        pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("B", &[(3.0, 2.0), (17.0, 2.0)])], 2, vec![fixed.clone()]);
    let guides = guides(&board);
    let mut events = Events::default();
    let options =
        UnifiedRouterOptions { pitch_nm: Some(mm(0.1)), grid_window_nm: window([5.0, 11.0, 21.0, 15.0]), ..opts() };
    let only = names(&["A"]);
    let result =
        route_unified(&board, &guides, &options, None, Some(&only), None, None, Some(&mut events)).expect("route");
    assert!(result.nets[0].connected);
    assert!(result.board.outline == board.outline && result.board.placements == board.placements);
    assert_eq!(result.board.tracks[..1], [fixed]);
    assert_eq!(result.board.nets, board.nets);
    assert!(errors(&drc(&result.board)).is_empty());
    let setup = events.of("unified_setup", "finished");
    let grid: Vec<i64> = detail(setup[0], "grid").as_array().expect("grid").iter().map(int_of).collect();
    assert_eq!(grid, [161, 41, 2]);
}

#[test]
fn geometry_entering_a_grid_window_still_blocks_it() {
    let walls: Vec<_> = [F, B].iter().map(|layer| track("W", (13.0, 10.0), (13.0, 16.0), 0.3, *layer)).collect();
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)]), ("W", &[])], 2, walls.clone());
    let guides = guides(&board);
    let options = UnifiedRouterOptions {
        maximum_iterations: 2,
        commit_rounds: 1,
        grid_window_nm: window([5.0, 11.0, 21.0, 15.0]),
        ..opts()
    };
    let only = names(&["A"]);
    let result = route_unified(&board, &guides, &options, None, Some(&only), None, None, None).expect("route");
    assert!(!result.nets[0].connected);
    assert_eq!(result.board.tracks[..2], walls[..]);
    assert_eq!(result.board.outline, board.outline);
}

#[test]
fn grid_window_refuses_invalid_bounds_and_excluded_targets() {
    // The tuple arity and the bool/float checks of Python have no Rust
    // equivalent: the option is a typed `(i64, i64, i64, i64)`.
    let error = UnifiedRouterOptions { grid_window_nm: Some((1, 0, 0, 1)), ..opts() }.validated().unwrap_err();
    assert!(error.to_string().contains("grid_window_nm"), "{error}");
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let guides = global(&board, 2.0);
    for bounds in [[10.0, 11.0, 18.0, 15.0], [30.0, 30.0, 40.0, 40.0]] {
        let options = UnifiedRouterOptions { grid_window_nm: window(bounds), ..opts() };
        let error = route_unified(&board, &guides, &options, None, None, None, None, None).unwrap_err();
        assert!(error.to_string().contains("grid_window_nm"), "{error}");
    }
}

#[test]
fn fine_window_fits_two_declared_widths_without_relaxing_clearance() {
    let tiny = footprint("tiny", vec![pad("1", 0.0, 0.0, 0.15, 0.15)], 0.3, 0.3);
    let mut placements = Vec::new();
    let mut nets = Vec::new();
    for (name, x, ys) in [("A", 10.3, [3.5, 20.5]), ("B", 10.75, [4.5, 21.5])] {
        let mut refs = Vec::new();
        for (i, y) in ys.iter().enumerate() {
            let reference = format!("{name}{i}");
            placements.push(placement(&reference, "tiny", x, *y));
            refs.push((reference, "1"));
        }
        let pads: Vec<(&str, &str)> = refs.iter().map(|(r, p)| (r.as_str(), *p)).collect();
        nets.push(net(name, &pads));
    }
    let mut board = board("FineChannels", 26.0, 26.0, vec![tiny], placements, nets);
    board.copper_keepouts = vec![
        copper_keepout("left", &[F, B], rect(0.5, 6.0, 10.0, 18.0)),
        copper_keepout("right", &[F, B], rect(11.05, 6.0, 25.5, 18.0)),
    ];
    board.net_routing_rules = ["A", "B"]
        .iter()
        .map(|n| copper_pcb::board::NetRoutingRule {
            width_nm: Some(mm(0.2)),
            clearance_nm: Some(mm(0.2)),
            allowed_layers: vec![F],
            max_vias: Some(0),
            ..rule(n)
        })
        .collect();
    let guides = global(&board, 2.0);
    let mut results = Vec::new();
    for pitch in [0.25, 0.025] {
        let options = UnifiedRouterOptions {
            pitch_nm: Some(mm(pitch)),
            maximum_iterations: 16,
            commit_rounds: 2,
            grid_window_nm: window([9.0, 2.0, 12.0, 22.0]),
            ..opts()
        };
        let result = route_unified(&board, &guides, &options, None, None, None, None, None).expect("route");
        assert_eq!(result.board.copper_keepouts, board.copper_keepouts);
        assert_eq!(result.board.net_routing_rules, board.net_routing_rules);
        assert!(errors_except_opens(&drc(&result.board)).is_empty());
        results.push(result);
    }
    assert!(!results[0].nets.iter().all(|net| net.connected));
    assert!(results[1].nets.iter().all(|net| net.connected));
    assert!(errors(&drc(&results[1].board)).is_empty());
}

#[test]
fn pad_groups_sharing_an_attach_node_both_keep_their_lead_ins() {
    // A fine-pitch connector row on one net: neighbouring pads attach to the
    // same grid node, and each needs its own lead-in.
    let row = footprint(
        "test/row",
        (0..5).map(|i| pad(&(i + 1).to_string(), 0.0, 0.4 * i as f64, 0.2, 0.2)).collect(),
        0.6,
        2.2,
    );
    let mut pads: Vec<(String, String)> = (1..=5).map(|i| ("J".to_owned(), i.to_string())).collect();
    pads.push(("T".to_owned(), "1".to_owned()));
    let pads: Vec<(&str, &str)> = pads.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let mut board = board(
        "Unified",
        26.0,
        26.0,
        vec![row, one_pad()],
        vec![placement("J", "test/row", 11.5, 12.1), placement("T", "test/one-pad", 20.0, 13.0)],
        vec![net("ROW", &pads)],
    );
    board.rules = DesignRules {
        minimum_clearance_nm: mm(0.1),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.15),
        ..fine_rules()
    };
    clean(&route(&board, &opts()));
}

#[test]
fn routing_is_deterministic() {
    // Python compared its native A* kernel with the pure-Python search; Rust
    // has one search, so the property kept is that equal inputs give equal copper.
    let mut owned: Vec<(String, [(f64, f64); 2])> = Vec::new();
    for i in 0..4 {
        owned.push((format!("H{i}"), [(2.0, 4.0 + 2.0 * i as f64), (18.0, 4.0 + 2.0 * i as f64)]));
        owned.push((format!("V{i}"), [(4.0 + 3.0 * i as f64, 2.0), (4.0 + 3.0 * i as f64, 18.0)]));
    }
    let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
    let board = pad_board(&nets, 2, vec![]);
    let first = route(&board, &opts());
    let again = route(&board, &opts());
    assert!(first.opens() == 0 && again.opens() == 0);
    assert_eq!(first.result.board.tracks, again.result.board.tracks);
    assert_eq!(first.result.board.vias, again.result.board.vias);
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("copper-route-unified-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir.join("inputs.state.json")
}

#[test]
fn saved_inputs_replay_to_the_same_copper() {
    let board = pad_board(&[("H", &[(2.0, 10.0), (18.0, 10.0)]), ("V", &[(10.0, 2.0), (10.0, 18.0)])], 2, vec![]);
    let state = scratch("replay");
    let path = state.to_string_lossy().into_owned();
    let first = route(&board, &UnifiedRouterOptions { save_inputs_path: Some(path.clone()), ..opts() });
    let text = std::fs::read_to_string(&state).expect("state");
    let again = replay_unified(&text, &path, &ReplayRequest::default(), None).expect("replay");
    let set = |tracks: &[copper_pcb::board::TrackSegment]| tracks.iter().cloned().collect::<IndexSet<_>>();
    assert_eq!(set(&again.board.tracks), set(&first.result.board.tracks));
    assert_eq!(
        again.board.vias.iter().collect::<IndexSet<_>>(),
        first.result.board.vias.iter().collect::<IndexSet<_>>()
    );
    let _ = std::fs::remove_dir_all(state.parent().expect("dir"));
}

#[test]
fn replay_rejects_unknown_and_excluded_net_selection() {
    let board = pad_board(&[("H", &[(2.0, 10.0), (18.0, 10.0)]), ("V", &[(10.0, 2.0), (10.0, 18.0)])], 2, vec![]);
    let state = scratch("select");
    let path = state.to_string_lossy().into_owned();
    route(&board, &UnifiedRouterOptions { save_inputs_path: Some(path.clone()), ..opts() });
    let text = std::fs::read_to_string(&state).expect("state");
    let request = ReplayRequest { only_nets: Some(names(&["H,V"])), ..ReplayRequest::default() };
    let error = replay_unified(&text, &path, &request, None).unwrap_err();
    assert!(error.to_string().contains("unknown replay nets: H,V"), "{error}");
    // A selection saved with the inputs (Python edited the state file instead).
    let options = UnifiedRouterOptions { save_inputs_path: Some(path.clone()), ..opts() };
    let only = names(&["H"]);
    route_unified(&board, &guides(&board), &options, None, Some(&only), None, None, None).expect("route");
    let limited = std::fs::read_to_string(&state).expect("state");
    let request = ReplayRequest { only_nets: Some(names(&["H", "V"])), ..ReplayRequest::default() };
    let error = replay_unified(&limited, &path, &request, None).unwrap_err();
    assert!(error.to_string().contains("outside saved replay selection: V"), "{error}");
    let request = ReplayRequest { only_nets: Some(names(&["H"])), ..ReplayRequest::default() };
    let result = replay_unified(&limited, &path, &request, None).expect("replay");
    assert_eq!(result.nets.iter().map(|n| n.net.as_str()).collect::<Vec<_>>(), ["H"]);
    assert!(result.nets.iter().all(|net| net.connected));
    let _ = std::fs::remove_dir_all(state.parent().expect("dir"));
}

/// `_board` plus a 5x5 ball array (1 mm pitch, 0.4 mm balls) as U at (10, 6).
fn bga_board(a: (f64, f64)) -> PhysicalBoard {
    let mut board = pad_board(&[("A", &[(3.0, a.1), (17.0, a.1)])], 2, vec![]);
    board.footprints.insert("test/bga25".to_owned(), ball_array("test/bga25", 5, 1.0, 0.4));
    board.placements.push(placement("U", "test/bga25", 10.0, 6.0));
    board
}

#[test]
fn ball_field_exact_drops_the_raster_margin_only_inside_ball_fields() {
    let board = bga_board((3.0, 15.0));
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let free = |exact: bool| {
        let options = UnifiedRouterOptions { pitch_nm: Some(mm(0.25)), ball_field_exact: exact, ..opts() };
        let mut router = new_router(&board, &options, &nets);
        let g = router.lattice.clone();
        let owner = router.rasters.track(mm(0.1), mm(0.1)).0.to_vec();
        let count = |x0: f64, y0: f64, x1: f64, y1: f64| {
            let ix = |x: f64| (mm(x) - g.x0).div_euclid(g.pitch);
            let iy = |y: f64| (mm(y) - g.y0).div_euclid(g.pitch);
            let mut free = 0;
            for y in iy(y0).max(0)..=iy(y1).min(g.ny - 1) {
                for x in ix(x0).max(0)..=ix(x1).min(g.nx - 1) {
                    free += usize::from(owner[(y * g.nx + x) as usize] == FREE);
                }
            }
            free
        };
        (count(7.0, 3.0, 13.0, 9.0), count(2.0, 13.0, 18.0, 17.0))
    };
    let (plain_field, plain_outside) = free(false);
    let (exact_field, exact_outside) = free(true);
    assert!(exact_field > plain_field, "lands in the field lose their margin");
    assert_eq!(exact_outside, plain_outside, "the rest of the board keeps it");
    assert!(opts().ball_field_exact, "on by default");
}

/// 5x5 ball arrays at `parts`, with nets between their balls, on `_board({"X": ...})`.
fn array_board(parts: &[(&str, (f64, f64))], nets: &[(&str, &[(&str, &str)])]) -> PhysicalBoard {
    let mut board = pad_board(&[("X", &[(3.0, 18.0), (17.0, 18.0)])], 2, vec![]);
    board.footprints.insert("test/bga25".to_owned(), ball_array("test/bga25", 5, 1.0, 0.4));
    for (reference, (x, y)) in parts {
        board.placements.push(placement(reference, "test/bga25", *x, *y));
    }
    for (name, pads) in nets {
        board.nets.push(net(name, pads));
    }
    board
}

#[test]
fn bus_bundles_route_the_nets_between_the_bus_parts_with_the_bus() {
    let mut board = array_board(
        &[("U", (8.0, 6.0)), ("V", (16.0, 6.0))],
        &[("A", &[("U", "15"), ("V", "11")]), ("B", &[("U", "20"), ("V", "16")]), ("C", &[("U", "10"), ("V", "6")])],
    );
    board.match_groups =
        vec![NetMatchGroup { id: "bus".to_owned(), nets: vec!["A".into(), "B".into()], max_skew_nm: mm(5.0) }];
    let global_route = global(&board, 2.0);
    let bus_nets = |bundles: bool| {
        let mut events = Events::default();
        route_with(
            &board,
            &global_route,
            &UnifiedRouterOptions { maximum_iterations: 5, bus_bundles: bundles, ..opts() },
            &mut events,
        );
        int_of(detail(events.phase("unified_buses")[0], "nets"))
    };
    assert_eq!(bus_nets(false), 2, "the group alone");
    assert_eq!(bus_nets(true), 3, "with C, whose pads are all on U and V");
}

#[test]
fn a_pad_with_its_own_dogbone_attaches_on_every_layer_of_the_via() {
    // A's first pad (at (6, 6) mm) has a stub to its via: the pad group must
    // offer the via on every layer, whatever node the connectivity graph picks.
    let mut board = pad_board(&[("A", &[(3.0, 3.0), (15.0, 15.0)])], 4, vec![]);
    let copper = board.stackup.copper_layers.clone();
    let dogbone = via("A", (7.0, 7.0), 0.6, 0.3, copper[0], copper[3]);
    board.tracks = vec![track("A", (6.0, 6.0), (7.0, 7.0), 0.2, copper[0])];
    board.vias = vec![dogbone.clone()];
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let router = new_router(&board, &UnifiedRouterOptions { pitch_nm: Some(mm(0.25)), ..opts() }, &nets);
    let g = &router.lattice;
    let spec = &router.specs[0];
    let mut attach_layers = IndexSet::new();
    for component in &spec.components {
        let near = component.iter().any(|(node, _, _)| {
            let (_, x, _) = unpack(g, *node);
            (g.x0 + x * g.pitch - dogbone.position.x_nm).abs() < mm(2.0)
        });
        if near {
            attach_layers.extend(component.iter().map(|(node, _, _)| unpack(g, *node).0));
        }
    }
    assert_eq!(attach_layers, (0..g.nl).collect::<IndexSet<_>>());
}

/// `grid.unpack(node)` of a uniform lattice: `(layer, x, y)`.
fn unpack(g: &copper_route::unified_router::lattice::RouterLattice, node: i64) -> (i64, i64, i64) {
    let layer = node / g.plane;
    let xy = node % g.plane;
    (layer, xy % g.nx, xy / g.nx)
}

#[test]
fn via_budget_is_never_exceeded() {
    // V may not use vias and H only one: whatever H does, it stays in budget.
    let mut board = pad_board(&[("H", &[(2.0, 10.0), (18.0, 10.0)]), ("V", &[(10.0, 2.0), (10.0, 18.0)])], 2, vec![]);
    board.net_routing_rules = vec![
        copper_pcb::board::NetRoutingRule { max_vias: Some(1), ..rule("H") },
        copper_pcb::board::NetRoutingRule { max_vias: Some(0), ..rule("V") },
    ];
    let routed = route(&board, &opts());
    assert!(routed.result.board.vias.iter().filter(|v| v.net == "H").count() <= 1);
    let by_code = summary_value(&routed.summary, "drc", "errors_by_code");
    assert!(by_code.get("DRC-MAX-VIAS").is_none());
}

fn walled_plane_pad_board() -> PhysicalBoard {
    // GND_1 lies outside the F.Cu pour, so GND gets copper (a via) in the first pass.
    let mut board = pad_board(&[("GND", &[(10.0, 10.0), (20.0, 2.0)]), ("A", &[(11.9, 5.0), (11.9, 15.0)])], 2, vec![]);
    let corners = [(14.5, 11.5), (11.5, 11.5), (11.5, 14.5), (14.5, 14.5)];
    // A U open towards net A.
    board.tracks = corners.windows(2).map(|pair| track("W", pair[0], pair[1], 0.2, F)).collect();
    board.vias = [3.0, 18.0].iter().flat_map(|x| [3.0, 23.0].map(|y| via("GND", (*x, y), 0.6, 0.3, F, B))).collect();
    board.zones = vec![
        zone("gnd-f", "GND", &[F], rect(0.5, 0.5, 20.0, 25.5)),
        zone("gnd-b", "GND", &[B], rect(0.5, 0.5, 25.5, 25.5)),
    ];
    board.nets.push(net("W", &[]));
    board
}

#[test]
fn pour_recheck_reconnects_a_plane_pad_walled_off_by_signal_copper() {
    use copper_route::plane_estimate::estimate_plane_connectivity;
    let board = walled_plane_pad_board();
    let unchecked = route(&board, &UnifiedRouterOptions { pour_recheck: false, ..opts() });
    let estimate = estimate_plane_connectivity(&unchecked.result.board).expect("estimate");
    assert_eq!(estimate.open_pads, IndexMap::from([("GND".to_owned(), vec!["GND_0.1".to_owned()])]));
    let checked = route(&board, &opts());
    assert_eq!(estimate_plane_connectivity(&checked.result.board).expect("estimate").opens, 0);
    clean(&checked);
    assert!(checked.result.board.vias.iter().any(|v| v.net == "GND"));
}

#[test]
fn a_neck_only_node_is_left_inside_its_region() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    let mut router = new_router(&board, &opts(), &nets);
    let g = router.lattice.clone();
    let y = g.ny / 2;
    let index = |x: i64| y * g.nx + x;
    let (source, left, target) = (index(40), index(39), index(60));
    // The source is legal only at neck width; its region is itself and its left neighbour.
    let spec = &mut router.specs[0];
    spec.neck = IndexSet::from([source]);
    spec.region = IndexSet::from([source, left]);
    spec.neck_mid = IndexSet::new();
    let region = spec.region.clone();
    let found = router
        .search(0, &[(source, 0.0)], Targets { nodes: &[target], extra: None }, 1.0)
        .expect("search")
        .expect("a path");
    assert!(region.contains(&found.0[1]));
}

fn bus_pads() -> PhysicalBoard {
    pad_board(
        &[("D0", &[(2.0, 7.0), (18.0, 7.0)]), ("D1", &[(2.0, 9.0), (18.0, 9.0)]), ("X", &[(10.0, 2.0), (10.0, 16.0)])],
        2,
        vec![],
    )
}

fn group(nets: &[&str], skew: f64) -> Vec<NetMatchGroup> {
    vec![NetMatchGroup {
        id: "bus".to_owned(),
        nets: nets.iter().map(|n| (*n).to_owned()).collect(),
        max_skew_nm: mm(skew),
    }]
}

#[test]
fn bus_members_route_first_and_are_reported_as_routed() {
    let mut board = bus_pads();
    board.match_groups = group(&["D0", "D1"], 5.0);
    let mut events = Events::default();
    let result =
        route_with(&board, &guides(&board), &UnifiedRouterOptions { buses_first: true, ..opts() }, &mut events);
    let report = drc(&result.board);
    let summary = copper_route::route_summary::connection_summary(&result.board, Some(&report)).expect("summary");
    assert_eq!(summary_int(&summary, "signal", "opens"), 0);
    assert_eq!(summary_int(&summary, "drc", "errors"), 0);
    assert!(events.phase("unified_buses").iter().any(|d| int_of(detail(d, "complete")) == 2));
    let by_name = |name: &str| result.nets.iter().find(|n| n.net == name).expect("net");
    assert!(by_name("D0").connected && by_name("D1").connected && by_name("D0").track_count > 0);
}

#[test]
fn bus_defaults_leave_a_board_without_match_groups_unchanged() {
    assert!(opts().buses_first && opts().bus_skew_tuning);
    let board = bus_pads();
    let global_route = guides(&board);
    let default = route_unified(&board, &global_route, &opts(), None, None, None, None, None).expect("route");
    let off = UnifiedRouterOptions { buses_first: false, bus_skew_tuning: false, ..opts() };
    let off = route_unified(&board, &global_route, &off, None, None, None, None, None).expect("route");
    assert_eq!(default.board.tracks, off.board.tracks);
    assert_eq!(default.board.vias, off.board.vias);
    assert!(!default.board.metadata.contains_key("bus_skew"));
}

#[test]
fn bus_members_over_their_length_leave_the_bus_phase_and_route_with_a_warning() {
    let mut board = bus_pads();
    board.match_groups = group(&["D0", "D1"], 5.0);
    board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { max_length_nm: Some(mm(10.0)), ..rule("D0") }];
    let mut events = Events::default();
    let options = UnifiedRouterOptions { buses_first: true, bus_length_budget: true, ..opts() };
    let result = route_with(&board, &guides(&board), &options, &mut events);
    let buses = events.phase("unified_buses");
    // D0 needs 16 mm against a 10 mm limit: the main negotiation routes it,
    // and its connected copper is kept with a length warning, not an open.
    assert!(!buses.is_empty());
    assert_eq!(int_of(detail(buses[0], "complete")), 1);
    assert_eq!(int_of(detail(buses[0], "over_length")), 1);
    let by_name = |name: &str| result.nets.iter().find(|n| n.net == name).expect("net");
    assert!(by_name("D0").connected && by_name("D1").connected && by_name("D0").diagnostics.is_empty());
    assert!(by_name("D0").warnings.join(" ").contains("exceeds"));
    assert!(result.board.tracks.iter().any(|t| t.net == "D0"));
}

#[test]
fn a_length_limited_net_takes_the_short_path_through_congestion() {
    let board = pad_board(&[("L", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let detoured = |board: &PhysicalBoard| {
        let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
        let mut router = new_router(board, &UnifiedRouterOptions { length_priority: true, ..opts() }, &nets);
        let g = router.lattice.clone();
        // A congested wall across the straight path on every layer, open only far away.
        let x = (13_000_000 - g.x0).div_euclid(g.pitch);
        for layer in 0..g.nl {
            for y in 0..g.ny {
                if (y * g.pitch + g.y0 - 13_000_000).abs() < 2_000_000 {
                    router.occupancy.occ[(layer * g.plane + y * g.nx + x) as usize] = 50;
                }
            }
        }
        let route = router.route_net(0, 10.0).expect("route");
        (route.connected, router.route_length_nm(&route))
    };
    let (connected, free_length) = detoured(&board);
    assert!(connected && free_length > mm(15.0) as f64, "without the rule: a detour");
    let mut limited = board.clone();
    limited.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { max_length_nm: Some(mm(15.0)), ..rule("L") }];
    let (connected, limited_length) = detoured(&limited);
    assert!(connected && limited_length <= mm(15.0) as f64, "with it: straight through");
}

fn converging(count: usize) -> Vec<(String, [(f64, f64); 2])> {
    (0..count).map(|i| (format!("N{i}"), [(2.0, 3.0 + 2.5 * i as f64), (18.0, 12.0 - 2.0 * i as f64)])).collect()
}

#[test]
fn route_smoothing_straightens_staircases_without_losing_connectivity() {
    // Three converging nets: grid search has no turn cost, so around each
    // other's copper equal-length staircases of 45-degree jogs survive.
    let owned = converging(3);
    let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
    let board = pad_board(&nets, 2, vec![]);
    let plain = route(&board, &UnifiedRouterOptions { route_smoothing: false, ..opts() });
    let smoothed = route(&board, &opts());
    clean(&smoothed);
    assert!(smoothed.result.nets.iter().all(|net| net.connected));
    assert!(track_bends(&smoothed.result.board.tracks) < track_bends(&plain.result.board.tracks));
    assert!(smoothed.result.metrics.total_length_nm <= plain.result.metrics.total_length_nm);
}

#[test]
fn fixed_copper_keeps_its_own_net_clearance_on_the_grid() {
    // S passes 0.7 mm from a pad of P: enough for S's own 0.15 mm clearance,
    // not for P's 0.4 mm. The exact check spaces them by the larger one.
    let mut board = pad_board(&[("S", &[(2.0, 10.0), (18.0, 10.0)]), ("P", &[(10.0, 10.7)])], 2, vec![]);
    board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { clearance_nm: Some(mm(0.4)), ..rule("P") }];
    let routed = route(&board, &opts());
    clean(&routed);
    assert!(routed.result.nets.iter().all(|net| net.connected));
}

#[test]
fn wide_nets_negotiate_room_for_each_others_width() {
    // Three converging 0.5 mm nets on a 0.2 mm default board.
    let owned = converging(3);
    let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
    let mut board = pad_board(&nets, 2, vec![]);
    board.net_routing_rules =
        owned.iter().map(|(n, _)| copper_pcb::board::NetRoutingRule { width_nm: Some(mm(0.5)), ..rule(n) }).collect();
    let routed = route(&board, &opts());
    clean(&routed);
    assert!(routed.result.nets.iter().all(|net| net.connected));
}

/// The escape bias of the tests: `net` away from F.Cu between x = 9 and 17 mm.
fn escape_bias(net: &str, rate: i64) -> LayerBias {
    LayerBias {
        net: net.to_owned(),
        layer: F,
        area: Bounds { min_x: mm(9.0), min_y: 0, max_x: mm(17.0), max_y: mm(26.0) },
        rate,
    }
}

#[test]
fn escape_bias_moves_a_net_off_the_layer_inside_its_area() {
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let plain = route(&board, &opts());
    let biased = route(&board, &UnifiedRouterOptions { escape_layer_biases: vec![escape_bias("A", 200)], ..opts() });
    clean(&biased);
    let middle: IndexSet<CopperLayer> = biased
        .result
        .board
        .tracks
        .iter()
        .filter(|t| {
            let mid = (t.start.x_nm + t.end.x_nm) / 2;
            mm(10.0) < mid && mid < mm(16.0)
        })
        .map(|t| t.layer)
        .collect();
    let plain_layers: IndexSet<CopperLayer> = plain.result.board.tracks.iter().map(|t| t.layer).collect();
    assert_eq!(plain_layers, IndexSet::from([F]));
    assert_eq!(middle, IndexSet::from([B]));
}

#[test]
fn negative_escape_bias_is_refused() {
    assert!(UnifiedRouterOptions { escape_layer_biases: vec![escape_bias("A", -1)], ..opts() }.validated().is_err());
}

fn bus_board(amplitude: Option<f64>, max_length: Option<f64>) -> PhysicalBoard {
    let mut board = pad_board(&[("A", &[(3.0, 6.0), (11.0, 6.0)]), ("B", &[(3.0, 14.0), (17.0, 14.0)])], 2, vec![]);
    board.net_routing_rules = ["A", "B"]
        .iter()
        .map(|n| copper_pcb::board::NetRoutingRule {
            width_nm: Some(mm(0.2)),
            tuning_amplitude_limit_nm: amplitude.map(mm),
            max_length_nm: max_length.map(mm),
            ..rule(n)
        })
        .collect();
    board.match_groups = group(&["A", "B"], 2.0);
    board
}

/// `bus_board` plus a third net C, which keeps the bus a proper subset.
fn bus_board_with_c(amplitude: Option<f64>) -> PhysicalBoard {
    merge(bus_board(amplitude, None), pad_board(&[("C", &[(3.0, 18.0), (17.0, 18.0)])], 2, vec![]))
}

#[test]
fn buses_first_with_skew_tuning_reports_untunable_members_on_the_board() {
    let board = bus_board_with_c(None);
    let global_route = global(&board, 2.0);
    let run = |options: UnifiedRouterOptions| {
        route_unified(&board, &global_route, &options, None, None, None, None, None).expect("route")
    };
    let result = run(UnifiedRouterOptions { buses_first: true, bus_skew_tuning: true, ..opts() });
    assert!(result.nets.iter().all(|net| net.connected));
    assert!(result.board.metadata["bus_skew"].contains("no tuning_amplitude_limit is declared"));
    let plain = run(UnifiedRouterOptions { buses_first: true, bus_skew_tuning: false, ..opts() });
    assert!(!plain.board.metadata.contains_key("bus_skew"));
    let default = run(opts());
    assert_eq!(default.board.metadata["bus_skew"], result.board.metadata["bus_skew"]);
}

#[test]
fn tuned_bus_copper_survives_route_smoothing() {
    let board = bus_board_with_c(Some(1.0));
    let global_route = global(&board, 2.0);
    let run = |tuning: bool| {
        let options =
            UnifiedRouterOptions { buses_first: true, bus_skew_tuning: tuning, route_smoothing: true, ..opts() };
        route_unified(&board, &global_route, &options, None, None, None, None, None).expect("route")
    };
    let plain = run(false);
    assert!(net_length(&plain.board.tracks, "B") - net_length(&plain.board.tracks, "A") > mm(2.0) as f64);
    let tuned = run(true);
    assert!(tuned.nets.iter().all(|net| net.connected) && !tuned.board.metadata.contains_key("bus_skew"));
    assert!((net_length(&tuned.board.tracks, "B") - net_length(&tuned.board.tracks, "A")).abs() <= mm(2.0) as f64);
    assert!(errors(&drc(&tuned.board)).is_empty());
}

fn stitched_pad_board() -> PhysicalBoard {
    // Each GND pad reaches the B.Cu plane only through its own via-in-pad,
    // and a ring of foreign copper leaves no room for a new via beside it.
    let mut board = pad_board(&[("GND", &[(5.0, 5.0), (15.0, 15.0)])], 2, vec![]);
    let mut wall = Vec::new();
    for (cx, cy) in [(8.0, 8.0), (18.0, 18.0)] {
        let corners = [
            (cx - 0.8, cy - 0.8),
            (cx + 0.8, cy - 0.8),
            (cx + 0.8, cy + 0.8),
            (cx - 0.8, cy + 0.8),
            (cx - 0.8, cy - 0.8),
        ];
        wall.extend(corners.windows(2).map(|pair| track("W", pair[0], pair[1], 0.2, F)));
    }
    board.tracks = wall;
    board.vias = vec![via("GND", (8.0, 8.0), 0.3, 0.15, F, B), via("GND", (18.0, 18.0), 0.3, 0.15, F, B)];
    board.zones = vec![zone("gnd-b", "GND", &[B], rect(0.5, 0.5, 25.5, 25.5))];
    board.nets.push(net("W", &[]));
    board
}

#[test]
fn pads_already_stitched_into_their_plane_count_as_joined() {
    let board = stitched_pad_board();
    let routed = route(&board, &opts());
    let gnd = routed.net("GND");
    assert!(gnd.connected && gnd.diagnostics.is_empty());
    // Nothing new is added for a plane net whose pads already reach it.
    let vias: Vec<&Via> = routed.result.board.vias.iter().filter(|v| v.net == "GND").collect();
    assert_eq!(vias, board.vias.iter().collect::<Vec<_>>());
    assert!(!routed.result.board.tracks.iter().any(|t| t.net == "GND"));
}

#[test]
fn adjacent_breakout_regions_do_not_emit_an_illegal_merged_neck() {
    let wall = track("BLOCK", (5.0, 13.65), (12.0, 13.65), 0.2, F);
    let mut board = pad_board(&[("N", &[(3.0, 10.0), (8.0, 10.0)]), ("BLOCK", &[])], 2, vec![wall.clone()]);
    board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule {
        width_nm: Some(mm(1.0)),
        breakout_width_nm: Some(mm(0.2)),
        breakout_length_nm: Some(mm(3.0)),
        allowed_layers: vec![F],
        max_vias: Some(0),
        ..rule("N")
    }];
    let options = UnifiedRouterOptions { direct: true, route_smoothing: false, maximum_iterations: 4, ..opts() };
    let routed = route(&board, &options);
    assert!(routed.result.nets[0].connected);
    assert_eq!(routed.opens(), 0);
    assert!(!routed.drc.findings.iter().any(|f| f.code == "DRC-TRACK-WIDTH"));
    assert!(routed.result.board.tracks.contains(&wall));
}

fn opposite_pad_via_board(hole_clearance: f64) -> PhysicalBoard {
    let cap = footprint("cap", vec![pad("1", 0.0, 0.0, 0.95, 0.9)], 1.6, 1.0);
    let pin = footprint("pin", vec![pad("1", 0.0, 0.0, 0.3, 1.55)], 1.0, 2.0);
    let mut back = placement("C", "cap", 13.0, 10.0);
    back.side = BoardSide::Back;
    let mut board = board(
        "OwnPadVia",
        26.0,
        26.0,
        vec![cap, pin],
        vec![back, placement("U", "pin", 13.0, 11.95)],
        vec![net("N", &[("C", "1"), ("U", "1")]), net("X", &[])],
    );
    board.rules = DesignRules {
        minimum_clearance_nm: mm(0.2),
        default_track_width_nm: mm(0.2),
        default_via_size_nm: mm(0.45),
        default_via_drill_nm: mm(0.2),
        minimum_hole_clearance_nm: mm(hole_clearance),
        ..DesignRules::default()
    };
    board.net_routing_rules = vec![copper_pcb::board::NetRoutingRule {
        width_nm: Some(mm(0.2)),
        clearance_nm: Some(mm(0.2)),
        max_vias: Some(1),
        max_length_nm: Some(mm(2.2)),
        ..rule("N")
    }];
    board
}

#[test]
fn via_raster_distinguishes_own_pad_halo_from_forbidden_annulus() {
    for hole_clearance in [0.2, 0.5] {
        let board = opposite_pad_via_board(hole_clearance);
        let nets = [&board.nets[0]];
        let mut router = new_router(&board, &UnifiedRouterOptions { pitch_nm: Some(mm(0.25)), ..opts() }, &nets);
        let g = router.lattice.clone();
        let index = RoutingClearanceIndex::new(&board).expect("index");
        let at = pt(13.0, 10.75);
        assert!(index.can_via("N", at, mm(0.45), F, B, Some(mm(0.2)), false, None).expect("can_via"));
        assert!(!index.can_via("X", at, mm(0.45), F, B, Some(mm(0.2)), false, None).expect("can_via"));
        let own = router.net_ids["N"];
        let owners = router.rasters.via(mm(0.45), mm(0.2), mm(0.2), None).to_vec();
        let xy = |x: f64, y: f64| {
            (((mm(y) - g.y0) as f64 / g.pitch as f64).round() as i64 * g.nx
                + ((mm(x) - g.x0) as f64 / g.pitch as f64).round() as i64) as usize
        };
        assert_eq!(owners[xy(13.0, 10.75)], own);
        assert_eq!(owners[xy(13.0, 10.5)], FOREIGN);
        assert!(!index.can_via("N", pt(13.0, 10.5), mm(0.45), F, B, Some(mm(0.2)), false, None).expect("can_via"));
    }
}

#[test]
fn opposite_face_pad_gap_routes_with_ordinary_via_and_exact_length_budget() {
    let board = opposite_pad_via_board(0.2);
    let routed = route(&board, &UnifiedRouterOptions { maximum_iterations: 8, commit_rounds: 2, ..opts() });
    assert!(routed.net("N").connected);
    clean(&routed);
    assert_eq!(routed.result.board.vias.len(), 1);
    assert_eq!(routed.result.board.net_routing_rules, board.net_routing_rules);
}

#[test]
fn pipeline_keeps_unified_settings_for_later_native_transactions() {
    use copper_place::options::PlannerOptions;
    use copper_route::flow::{RoutingPipelineOptions, run_routing_pipeline};
    use copper_route::routeflow::PlacementRoutingFeedbackOptions;
    let board = pad_board(&[("A", &[(3.0, 10.0), (17.0, 10.0)])], 2, vec![]);
    let options = UnifiedRouterOptions { maximum_iterations: 3, commit_rounds: 1, ..opts() };
    for stop in [None, Some("placement")] {
        let placement_options = PlannerOptions {
            candidate_count: 1,
            analytical_iterations: 0,
            refinement_passes: 0,
            fixed_references: board.placements.iter().map(|p| p.reference.clone()).collect(),
            ..PlannerOptions::default()
        }
        .validated()
        .expect("planner options");
        let pipeline = RoutingPipelineOptions {
            unified_options: Some(options.clone()),
            stop_after: stop.map(str::to_owned),
            placement_options: Some(placement_options),
            feedback_options: Some(PlacementRoutingFeedbackOptions { maximum_iterations: 1, ..Default::default() }),
            ..RoutingPipelineOptions::default()
        };
        let result = run_routing_pipeline(board.clone(), pipeline, None).expect("pipeline");
        assert_eq!(result.unified_options, options);
        if stop.is_none() {
            assert_eq!(result.detailed.board.metadata["detailed_router"], "unified");
        }
    }
}

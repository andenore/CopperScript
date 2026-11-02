//! Unified routing over a nested (multi-resolution) lattice (ported from the
//! former `tests/test_unified_nested_routing.py`).

mod support_routers;

use copper_json::Value;
use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, Stackup};
use copper_physical::footprint::{FootprintPad, PhysicalFootprint, Size};
use copper_physical::{CopperLayer, DesignRules, Point};
use copper_route::fine_regions::FineRegionOptions;
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::unified_router::router::{NetRoute, Router};
use copper_route::unified_router::{ReplayRequest, UnifiedRouterOptions, fine_region_plan, replay_unified};
use indexmap::IndexSet;
use support_routers::routes::{Routed, drc, errors_except_opens, global, route_with};
use support_routers::*;

const F: CopperLayer = CopperLayer::Front;
const FINE: i64 = 125_000;

fn opts() -> UnifiedRouterOptions {
    UnifiedRouterOptions::default()
}

/// `test_fine_regions._ball_array`: a full n x n grid of 0.35 mm circular balls.
fn ball_array(n: i64, pitch_nm: i64, name: &str) -> PhysicalFootprint {
    let half = (n - 1) * pitch_nm / 2;
    let mut pads = Vec::new();
    for row in 0..n {
        for col in 0..n {
            pads.push(FootprintPad {
                position: Point::new(col * pitch_nm - half, row * pitch_nm - half),
                ..ball(&format!("R{row}C{col}"), 0.0, 0.0, 0.35)
            });
        }
    }
    let width = mm(1.0) + 2 * half;
    PhysicalFootprint { body_size: Size { width_nm: width, height_nm: width }, ..footprint(name, pads, 1.0, 1.0) }
}

/// `test_fine_regions._qfn32`: QFN-32 5x5 mm, 0.5 mm pitch, with an exposed pad.
fn qfn32() -> PhysicalFootprint {
    let (step, offset) = (500_000i64, 2_437_500i64);
    let half = 7 * step / 2;
    let (length, width) = (875_000, 250_000);
    let mut pads = Vec::new();
    let mut number = 1;
    for side in 0..4 {
        for i in 0..8 {
            let along = i * step - half;
            let (position, size) = match side {
                0 => (Point::new(-offset, along), (length, width)),
                1 => (Point::new(along, offset), (width, length)),
                2 => (Point::new(offset, -along), (length, width)),
                _ => (Point::new(-along, -offset), (width, length)),
            };
            pads.push(FootprintPad {
                position,
                size: Size { width_nm: size.0, height_nm: size.1 },
                ..pad(&number.to_string(), 0.0, 0.0, 0.1, 0.1)
            });
            number += 1;
        }
    }
    pads.push(pad(&number.to_string(), 0.0, 0.0, 3.45, 3.45));
    PhysicalFootprint {
        body_size: Size { width_nm: 2 * offset, height_nm: 2 * offset },
        ..footprint("test/qfn32_0.5", pads, 1.0, 1.0)
    }
}

/// An n x n 0.75 mm ball array whose balls each connect to one pad on a ring around it.
fn bga_board(width: f64, height: f64, layer_count: usize, n: i64, centre: (f64, f64)) -> PhysicalBoard {
    let name = format!("test/bga{}_0.75", n * n);
    let array = ball_array(n, 750_000, &name);
    let ring_pad = footprint("test/pad", vec![pad("1", 0.0, 0.0, 0.5, 0.5)], 1.0, 1.0);
    let (cx, cy) = (mm(centre.0), mm(centre.1));
    let mut balls: Vec<&FootprintPad> = array.pads.iter().collect();
    balls.sort_by(|a, b| {
        let angle = |p: &FootprintPad| (p.position.y_nm as f64).atan2(p.position.x_nm as f64);
        angle(a).total_cmp(&angle(b))
    });
    let mut placements = vec![Placement { position: Point::new(cx, cy), ..placement("U1", &name, 0.0, 0.0) }];
    let mut nets = Vec::new();
    let ring = mm(6.0) as f64;
    let step = 2.0 * std::f64::consts::PI / balls.len() as f64;
    let lattice = mm(0.25) as f64;
    for (index, ball) in balls.iter().enumerate() {
        let angle = -std::f64::consts::PI + (index as f64 + 0.5) * step;
        let (dx, dy) = (angle.cos(), angle.sin());
        let scale = ring / dx.abs().max(dy.abs());
        let x = cx + ((dx * scale / lattice).round_ties_even() * lattice) as i64;
        let y = cy + ((dy * scale / lattice).round_ties_even() * lattice) as i64;
        let reference = format!("P{index}");
        placements.push(Placement { position: Point::new(x, y), ..placement(&reference, "test/pad", 0.0, 0.0) });
        nets.push(PhysicalNet {
            name: format!("N{index}"),
            pads: vec![PadReference::new("U1", &ball.number), PadReference::new(&reference, "1")],
        });
    }
    let name = "NestedBGA";
    let mut board = board(name, width, height, vec![array, ring_pad], placements, nets);
    board.stackup = Stackup::new(layers(layer_count));
    board.rules = DesignRules {
        minimum_clearance_nm: mm(0.1),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.1),
        default_via_size_nm: mm(0.4),
        default_via_drill_nm: mm(0.2),
        minimum_hole_clearance_nm: mm(0.15),
        ..DesignRules::default()
    };
    board
}

fn default_bga() -> PhysicalBoard {
    bga_board(20.0, 20.0, 4, 5, (10.0, 10.0))
}

fn route_events(board: &PhysicalBoard, options: &UnifiedRouterOptions) -> (Routed, Events) {
    let mut bare = board.clone();
    bare.tracks.clear();
    bare.vias.clear();
    let guides = global(&bare, 2.0);
    let mut events = Events::default();
    let result = route_with(board, &guides, options, &mut events);
    let report = drc(&result.board);
    let summary = copper_route::route_summary::connection_summary(&result.board, Some(&report)).expect("summary");
    (Routed { result, drc: report, summary }, events)
}

fn setup(events: &Events) -> &Vec<(&'static str, Value)> {
    events.of("unified_setup", "finished")[0]
}

fn router<'p>(board: &'p PhysicalBoard, options: &UnifiedRouterOptions) -> Router<'p, 'p> {
    let nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    Router::new(board, options, &nets, IndexSet::new(), None, RoutingClearanceIndex::new(board).expect("index"))
        .expect("router")
}

fn dense() -> UnifiedRouterOptions {
    UnifiedRouterOptions { fine_regions: "dense".to_owned(), ..opts() }
}

fn off() -> UnifiedRouterOptions {
    UnifiedRouterOptions { fine_regions: "off".to_owned(), ..opts() }
}

// (a) a board without a full ball array: the default is today's router

#[test]
fn off_is_the_default_copper() {
    let one: Vec<(String, Vec<(f64, f64)>)> = vec![("A".into(), vec![(3.0, 10.0), (17.0, 10.0)])];
    let cross = vec![("H".into(), vec![(2.0, 10.0), (18.0, 10.0)]), ("V".into(), vec![(10.0, 2.0), (10.0, 18.0)])];
    let mut grid = Vec::new();
    for i in 0..3 {
        grid.push((format!("H{i}"), vec![(2.0, 4.0 + 2.0 * i as f64), (18.0, 4.0 + 2.0 * i as f64)]));
    }
    for i in 0..3 {
        grid.push((format!("V{i}"), vec![(4.0 + 3.0 * i as f64, 2.0), (4.0 + 3.0 * i as f64, 18.0)]));
    }
    for owned in [one, cross, grid] {
        let nets: Vec<(&str, &[(f64, f64)])> = owned.iter().map(|(n, p)| (n.as_str(), &p[..])).collect();
        let board = pad_board(&nets, 2, vec![]);
        let default = support_routers::routes::route(&board, &opts()).result;
        let plain = support_routers::routes::route(&board, &off()).result;
        assert_eq!(plain.board, default.board);
        assert_eq!(plain.nets, default.nets);
        assert_eq!(plain.metrics, default.metrics);
        // Without a qualifying package dense plans no patch either.
        let auto = support_routers::routes::route(
            &board,
            &UnifiedRouterOptions { fine_regions: "auto".into(), ..opts() }.validated().expect("options"),
        )
        .result;
        assert!(auto.board.tracks == default.board.tracks && auto.board.vias == default.board.vias);
    }
}

fn qfn_board() -> PhysicalBoard {
    let mut board = bga_board(20.0, 20.0, 4, 3, (10.0, 10.0));
    board.footprints.clear();
    board.footprints.insert("test/qfn32_0.5".to_owned(), qfn32());
    board.footprints.insert("test/pad".to_owned(), footprint("test/pad", vec![pad("1", 0.0, 0.0, 0.5, 0.5)], 1.0, 1.0));
    board.placements = vec![
        placement("U2", "test/qfn32_0.5", 10.0, 10.0),
        placement("P1", "test/pad", 3.0, 3.0),
        placement("P2", "test/pad", 17.0, 4.0),
        placement("P3", "test/pad", 16.0, 17.0),
    ];
    board.nets = vec![
        net("A", &[("U2", "1"), ("P1", "1")]),
        net("B", &[("U2", "12"), ("P2", "1")]),
        net("C", &[("U2", "20"), ("P3", "1")]),
    ];
    board
}

#[test]
fn default_without_a_ball_array_is_the_plain_uniform_lattice_and_off_copper() {
    let board = qfn_board();
    assert_eq!(opts().fine_regions, "arrays");
    let plan = fine_region_plan(&board, &opts()).expect("plan").expect("a plan");
    assert!(plan.patches.is_empty() && plan.skipped[0].0 == "U2");
    let default_router = router(&board, &opts());
    assert!(!default_router.lattice.is_nested());
    assert!(default_router.pitch == default_router.lattice.pitch && default_router.pitch == mm(0.25));
    assert_eq!(default_router.lattice.signature(), router(&board, &off()).lattice.signature());
    let (default, events) = route_events(&board, &opts());
    let (plain, off_events) = route_events(&board, &off());
    assert!(
        default.result.board.tracks == plain.result.board.tracks
            && default.result.board.vias == plain.result.board.vias
    );
    assert_eq!(default.result.metrics, plain.result.metrics);
    assert_eq!(default.drc.findings, plain.drc.findings);
    let keys = |e: &Events| setup(e).iter().map(|(k, _)| *k).collect::<IndexSet<_>>();
    assert_eq!(keys(&events), IndexSet::from(["nets", "grid", "seconds"]));
    assert_eq!(keys(&events), keys(&off_events));
    // dense refines the QFN instead.
    let nested = router(&board, &dense());
    let factors: Vec<i64> = nested.lattice.blocks()[1..].iter().map(|b| b.factor).collect();
    assert!(nested.lattice.is_nested() && factors == [4]);
}

#[test]
fn options_validate_the_mode() {
    // Python also refused a dict for `fine_region_options`; the Rust field is typed.
    let error = UnifiedRouterOptions { fine_regions: "on".into(), ..opts() }.validated().unwrap_err();
    assert!(error.to_string().contains("fine_regions"), "{error}");
    assert_eq!(
        UnifiedRouterOptions { fine_regions: "auto".into(), ..opts() }.validated().expect("options").fine_regions,
        "dense"
    );
    assert!(UnifiedRouterOptions { fine_regions: "uniform".into(), ..opts() }.validated().is_err());
}

#[test]
fn the_mode_decides_dense_packages_and_the_options_the_other_knobs() {
    let mut board = bga_board(40.0, 20.0, 4, 5, (10.0, 10.0));
    board.footprints.insert("test/qfn32_0.5".to_owned(), qfn32());
    board.placements.push(placement("U2", "test/qfn32_0.5", 30.0, 10.0));
    let plan = |options: UnifiedRouterOptions| fine_region_plan(&board, &options).expect("plan");
    assert!(plan(off()).is_none());
    let knobs = FineRegionOptions { margin_nm: mm(0.5), dense_packages: true, ..FineRegionOptions::default() };
    let arrays = plan(UnifiedRouterOptions { fine_region_options: Some(knobs), ..opts() }).expect("plan");
    let refs = |p: &copper_route::fine_regions::FineRegionPlan| {
        p.patches.iter().map(|p| p.references.clone()).collect::<Vec<_>>()
    };
    assert_eq!(refs(&arrays), [vec!["U1".to_owned()]], "dense_packages=True ignored");
    let default = plan(opts()).expect("plan");
    assert_eq!(arrays.patches[0].x0_nm - default.patches[0].x0_nm, mm(1.0), "the margin applies");
    let dense_plan = plan(UnifiedRouterOptions { fine_region_options: Some(FineRegionOptions::default()), ..dense() })
        .expect("plan");
    assert_eq!(refs(&dense_plan), [vec!["U1".to_owned()], vec!["U2".to_owned()]], "dense_packages=False ignored");
    assert_eq!(
        plan(UnifiedRouterOptions { fine_regions: "auto".into(), ..opts() }.validated().expect("options")),
        Some(dense_plan)
    );
    // pitch_nm is the base pitch under patches.
    let coarse = plan(UnifiedRouterOptions { pitch_nm: Some(mm(0.5)), ..opts() }).expect("plan");
    assert!(coarse.base_pitch_nm == mm(0.5) && coarse.patches[0].factor == 4);
}

// (b), (c) a fine ball array: nested against uniform at the fine pitch

#[test]
fn bga_routes_completely_on_the_nested_lattice_and_matches_uniform_completion() {
    let board = default_bga();
    let (nested, events) = route_events(&board, &opts());
    assert!(nested.opens() == 0 && nested.result.nets.iter().all(|n| n.connected));
    assert_eq!(nested.errors(), 0);
    let setup_details = setup(&events);
    let patches = detail(setup_details, "patches").as_array().expect("patches");
    assert!(patches.len() == 1 && int_of(patches[0].get("factor").expect("factor")) == 2);
    let router = router(&board, &dense());
    let g = router.lattice.as_nested().expect("nested");
    assert_eq!(int_of(detail(setup_details, "nodes")), g.size);
    let pose = &board.placements[0];
    for ball in &board.footprints[&pose.footprint].pads {
        let node = g.snap(0, copper_pcb::transform::transformed_local_point(pose, ball.position)).expect("a node");
        assert_eq!(g.block_index(node % g.plane), 1);
    }
    // (c) uniform at the fine pitch reaches the same completion on more nodes.
    let (uniform, uniform_events) = route_events(&board, &UnifiedRouterOptions { pitch_nm: Some(FINE), ..off() });
    assert!(uniform.opens() == 0 && uniform.errors() == 0);
    let grid: Vec<i64> = detail(setup(&uniform_events), "grid").as_array().expect("grid").iter().map(int_of).collect();
    assert!(int_of(detail(setup_details, "nodes")) < grid[0] * grid[1] * grid[2]);
}

// (d), (e) portals

/// Two nets on one layer from balls inside the patch to pads outside it.
fn crossing_board() -> PhysicalBoard {
    let mut board = default_bga();
    board.nets.retain(|n| n.name == "N0" || n.name == "N12");
    let refs: IndexSet<String> = board.nets.iter().flat_map(|n| n.pads.iter().map(|p| p.component.clone())).collect();
    board.placements.retain(|p| refs.contains(&p.reference));
    board
}

fn point_of(g: &copper_route::routing_lattice::NestedLattice, node: i64) -> (i64, Point) {
    (node / g.plane, g.column_point(node % g.plane))
}

#[test]
fn a_path_from_inside_a_patch_crosses_a_portal_and_commits() {
    let board = crossing_board();
    let mut router = router(&board, &dense());
    let g = router.lattice.as_nested().expect("nested").clone();
    let (routes, conflicts, _) = router.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    assert!(conflicts.is_empty() && routes.values().all(|r| r.connected));
    let mut crossed = 0;
    for route in routes.values() {
        for path in &route.paths {
            let points: Vec<(i64, Point)> = path.iter().map(|n| point_of(&g, *n)).collect();
            // Collapse portal steps (twins share their point): then no point repeats.
            let collapsed: Vec<_> =
                points.iter().enumerate().filter(|(i, p)| *i == 0 || **p != points[i - 1]).map(|(_, p)| *p).collect();
            assert_eq!(collapsed.len(), collapsed.iter().collect::<IndexSet<_>>().len());
            let portals: Vec<(i64, i64)> =
                path.windows(2).filter(|w| g.twin(w[0]) == w[1]).map(|w| (w[0], w[1])).collect();
            crossed += portals.len();
            for (a, b) in &portals {
                let blocks = IndexSet::from([g.block_index(a % g.plane), g.block_index(b % g.plane)]);
                assert_eq!(blocks, IndexSet::from([0, 1]));
            }
            let blocks: IndexSet<usize> = path.iter().map(|n| g.block_index(n % g.plane)).collect();
            if blocks.len() == 2 {
                assert!(!portals.is_empty());
            }
        }
    }
    assert!(crossed > 0);
    let (accepted, rejected, complete) = router.commit(&routes, &conflicts).expect("commit");
    assert!(rejected.is_empty() && complete == routes.keys().cloned().collect::<IndexSet<_>>());
    let mut copper = board.clone();
    for (tracks, vias) in accepted.values() {
        assert!(tracks.iter().all(|t| t.start != t.end));
        copper.tracks.extend(tracks.iter().cloned());
        copper.vias.extend(vias.iter().cloned());
    }
    assert!(errors_except_opens(&drc(&copper)).is_empty());
}

#[test]
fn claims_cover_portal_twins_so_two_nets_conflict_at_one_boundary_point() {
    let board = crossing_board();
    let mut router = router(&board, &dense());
    let g = router.lattice.as_nested().expect("nested").clone();
    let patch = &g.blocks[1];
    let fine = g.snap(0, Point::new(patch.x0, patch.y0 + 8 * g.pitch)).expect("node");
    let base = g.twin(fine);
    assert!(g.block_index(fine % g.plane) == 1 && g.block_index(base % g.plane) == 0);
    // Net A passes through the fine node, net B through the coarse one.
    let (inner, outer) = (fine + 1, base - 1);
    let mut a = NetRoute { paths: vec![vec![inner, fine]], leads: vec![Vec::new()], ..NetRoute::empty(true, "") };
    let mut b = NetRoute { paths: vec![vec![outer, base]], leads: vec![Vec::new()], ..NetRoute::empty(true, "") };
    router.finish_route(&mut a, 0);
    router.finish_route(&mut b, 1);
    for route in [&a, &b] {
        assert!(route.claims.contains(&fine) && route.claims.contains(&base));
        assert!(route.nodes.contains(&fine) && route.nodes.contains(&base));
    }
    router.occupancy.add(&a, 1);
    router.occupancy.add(&b, 1);
    assert!(router.occupancy.occ[fine as usize] == 2 && router.occupancy.occ[base as usize] == 2);
    let (name_a, name_b) = (router.specs[0].name.clone(), router.specs[1].name.clone());
    let routes = indexmap::IndexMap::from([(name_a.clone(), a), (name_b.clone(), b)]);
    assert_eq!(router.conflicts(&routes).expect("conflicts"), IndexSet::from([name_a, name_b]));
}

#[test]
fn two_nets_forced_through_one_boundary_point_conflict_instead_of_both_committing() {
    // A keepout wall on the patch boundary leaves one opening, one track wide,
    // at a portal twin pair: both front-only nets need that site.
    let mut board = default_bga();
    board.nets.retain(|n| n.pads[0].pad == "R1C0" || n.pads[0].pad == "R3C0");
    let sides: Vec<(String, i64)> =
        board.nets.iter().zip([-1, 1]).map(|(n, s)| (n.pads[1].component.clone(), s)).collect();
    board.placements.retain(|p| p.reference == "U1" || sides.iter().any(|(r, _)| *r == p.reference));
    for pose in &mut board.placements {
        if let Some((_, side)) = sides.iter().find(|(r, _)| *r == pose.reference) {
            pose.position = Point::new(mm(3.0), mm(10.0) + side * mm(3.0));
        }
    }
    board.net_routing_rules = board
        .nets
        .iter()
        .map(|n| NetRoutingRule { allowed_layers: vec![F], max_vias: Some(0), ..rule(&n.name) })
        .collect();
    let x = router(&board, &dense()).lattice.blocks()[1].x0;
    let (gap, half, opening) = (mm(10.0), mm(0.05), mm(0.2));
    board.copper_keepouts = [(mm(-1.0), gap - opening), (gap + opening, mm(21.0))]
        .iter()
        .enumerate()
        .map(|(i, (y0, y1))| {
            let corners = vec![
                Point::new(x - half, *y0),
                Point::new(x + half, *y0),
                Point::new(x + half, *y1),
                Point::new(x - half, *y1),
            ];
            copper_keepout(&format!("wall{i}"), &[F], corners)
        })
        .collect();
    let mut router = router(&board, &UnifiedRouterOptions { maximum_iterations: 12, commit_rounds: 1, ..dense() });
    let g = router.lattice.as_nested().expect("nested").clone();
    let fine = g.snap(0, Point::new(x, gap)).expect("node");
    let base = g.twin(fine);
    assert!(g.block_index(fine % g.plane) == 1 && g.block_index(base % g.plane) == 0);
    let (routes, conflicts, _) = router.negotiate(None, None, &mut Vec::new()).expect("negotiate");
    for route in routes.values() {
        assert!(route.paths.iter().any(|p| p.contains(&base) && p.contains(&fine)));
        assert!(route.claims.contains(&fine) && route.claims.contains(&base));
    }
    assert!(router.occupancy.occ[fine as usize] == 2 && router.occupancy.occ[base as usize] == 2);
    assert_eq!(conflicts, board.nets.iter().map(|n| n.name.clone()).collect::<IndexSet<_>>());
    let (accepted, rejected, complete) = router.commit(&routes, &conflicts).expect("commit");
    assert!(complete.len() == 1 && rejected.len() == 1);
    let mut copper = board.clone();
    copper.tracks = accepted.values().flat_map(|(t, _)| t.iter().cloned()).collect();
    assert!(errors_except_opens(&drc(&copper)).is_empty());
}

// (f) congestion export on a nested lattice: the empty-board half is a unit
// test of the private `congestion_map` (`src/unified_router/tests.rs`).

#[test]
fn congestion_export_is_finite_on_a_nested_lattice() {
    let (nested, _) = route_events(&default_bga(), &UnifiedRouterOptions { export_congestion: true, ..opts() });
    let congestion = nested.result.congestion.expect("a congestion map");
    assert!(congestion.utilisation().iter().flatten().all(|u| u.is_finite()));
    assert!(congestion.demand.iter().chain(&congestion.capacity).flatten().all(|v| v.is_finite()));
}

// (g) a search window is not supported with patches

#[test]
fn grid_window_with_fine_regions_raises() {
    let board = default_bga();
    let guides = global(&board, 2.0);
    let options = UnifiedRouterOptions { grid_window_nm: Some((mm(2.0), mm(2.0), mm(18.0), mm(18.0))), ..dense() };
    let error =
        copper_route::unified_router::route::route_unified(&board, &guides, &options, None, None, None, None, None)
            .unwrap_err();
    assert!(error.to_string().contains("grid_window_nm is not supported with fine_regions"), "{error}");
}

// (h) replay of a state saved before the field existed

/// Removes the fine-region fields of the saved options and refreshes the digest.
fn strip_fine_region_fields(text: &str) -> String {
    fn strip(value: &mut Value) -> bool {
        match value {
            Value::Object(object) => {
                if object.get("$type").and_then(Value::as_str).is_some_and(|t| t.ends_with(":UnifiedRouterOptions")) {
                    let Some(Value::Object(fields)) = object.get_mut("fields") else { panic!("fields") };
                    for name in ["fine_regions", "fine_region_options"] {
                        assert!(fields.shift_remove(name).is_some(), "{name}");
                    }
                    return true;
                }
                object.values_mut().any(strip)
            }
            Value::Array(items) => items.iter_mut().any(strip),
            _ => false,
        }
    }
    let Value::Object(mut document) = copper_json::parse(text).expect("JSON") else { panic!("object") };
    assert!(strip(document.get_mut("value").expect("value")));
    let digest = copper_state::content_digest(&document["value"]);
    document.insert("content_digest".to_owned(), Value::from(digest.as_str()));
    copper_json::Dumps::compact().dumps(&Value::Object(document)) + "\n"
}

#[test]
fn replay_of_a_state_without_the_field_uses_the_default_and_honours_an_override() {
    let mut board = bga_board(30.0, 20.0, 4, 5, (10.0, 10.0));
    board.footprints.insert("test/qfn32_0.5".to_owned(), qfn32());
    board.placements.push(placement("U2", "test/qfn32_0.5", 25.0, 10.0));
    board.nets.truncate(3);
    let dir = std::env::temp_dir().join(format!("copper-route-nested-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch");
    let state = dir.join("old.state.json");
    let path = state.to_string_lossy().into_owned();
    let options = UnifiedRouterOptions { save_inputs_path: Some(path.clone()), ..off() };
    copper_route::unified_router::route::route_unified(
        &board,
        &global(&board, 2.0),
        &options,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("route");
    let text = strip_fine_region_fields(&std::fs::read_to_string(&state).expect("state"));
    let saved = copper_route::state_codec::load_inputs(&text, &path).expect("old state");
    assert_eq!(saved.options.fine_regions, "arrays", "the dataclass default");
    let patches = |mode: Option<&str>| {
        let overrides = mode.map(|m| vec![("fine_regions".to_owned(), Value::from(m))]).unwrap_or_default();
        let mut events = Events::default();
        let request = ReplayRequest { overrides, ..ReplayRequest::default() };
        let result = replay_unified(&text, &path, &request, Some(&mut events)).expect("replay");
        assert!(result.nets.iter().all(|n| n.connected));
        let details = setup(&events);
        let Some((_, Value::Array(items))) = details.iter().find(|(k, _)| *k == "patches") else { return Vec::new() };
        items
            .iter()
            .map(|p| {
                let f = |k: &str| match p.get(k) {
                    Some(Value::Float(v)) => (v * 1000.0).round() / 1000.0,
                    other => panic!("{k}: {other:?}"),
                };
                ((f("x0"), f("y0")), int_of(p.get("factor").expect("factor")))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(patches(None), [((6.75, 6.75), 2)]);
    assert_eq!(patches(Some("dense")), [((6.75, 6.75), 2), ((20.5, 5.5), 4)]);
    assert!(patches(Some("off")).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// (i) the S2/S3 memory fixture

#[test]
fn memory_fixture_default_refines_the_array_at_a_third_of_the_uniform_nodes() {
    let board = bga_board(100.0, 80.0, 6, 5, (50.0, 40.0));
    let (routed, events) = route_events(&board, &opts());
    let details = setup(&events);
    let factors: Vec<i64> = detail(details, "patches")
        .as_array()
        .expect("patches")
        .iter()
        .map(|p| int_of(p.get("factor").expect("factor")))
        .collect();
    assert_eq!(factors, [2]);
    let router = router(&board, &opts());
    assert!(router.lattice.is_nested() && router.lattice.blocks().len() == 2);
    assert!(routed.result.nets.len() == 25 && routed.result.nets.iter().all(|n| n.connected));
    assert!(routed.opens() == 0 && routed.errors() == 0);
    let uniform = copper_route::fine_regions::base_lattice_bounds(&board, FINE).expect("bounds");
    let uniform = uniform.plane() * 6;
    assert_eq!(int_of(detail(details, "uniform_nodes_at_finest")), uniform);
    assert!(int_of(detail(details, "nodes")) * 3 < uniform);
}

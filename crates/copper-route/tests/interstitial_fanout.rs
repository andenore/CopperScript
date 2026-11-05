//! Ordinary BGA dogbones use exact copper shapes, never bounding-box
//! waivers; ported from `tests/test_interstitial_fanout.py`. The
//! `_legal_choices`/`_maze_choices` cases are in `src/test_support_planes.rs`.

mod support_planes;

use std::cell::RefCell;

use copper_pcb::board::{PadReference, PhysicalBoard, Via};
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::global_terminals::register_terminal_capacity;
use copper_route::plane::{PlaneStitchOptions, stitch_zone_pads};
use copper_route::routing::{
    AccessOptions, GlobalRouterOptions, Graph, PinAccess, build_graph, pin_access_candidates, via_resource_id,
};
use support_planes::*;

const LAYERS: [CopperLayer; 2] = [CopperLayer::Front, CopperLayer::Back];

fn can_via(index: &RoutingClearanceIndex<'_>, via: &Via) -> bool {
    index
        .can_via(&via.net, via.position, via.size_nm, via.from_layer, via.to_layer, Some(via.drill_nm), false, None)
        .unwrap()
}

#[test]
fn native_project_uses_declared_small_ordinary_vias() {
    let mut board = interstitial_board();
    board.vias = vec![via("N5", mm(9.0, 9.0), 450_000, 200_000)];
    let manifest = copper_pcb::kicad_pcb::generate(&board).unwrap();
    let project = copper_json::parse(&manifest.artifacts[1].content).unwrap();
    let rules = field(field(field(&project, "board"), "design_settings"), "rules");
    let number = |key: &str| match field(rules, key) {
        copper_json::Value::Float(value) => *value,
        other => panic!("{key}: {other:?}"),
    };
    assert_eq!(number("min_via_diameter"), 0.45);
    assert_eq!(number("min_through_hole_diameter"), 0.2);
    assert_eq!(number("min_via_annular_width"), 0.125);
    // The installed-KiCad half (native DRC of the written project) is not ported.
}

#[test]
fn plane_launch_uses_exact_round_pad_interstice_without_via_in_pad() {
    let mut board = interstitial_board();
    board.zones = vec![zone("reference", "N5", &[CopperLayer::Back], rect(1.0, 1.0, 11.0, 11.0))];
    let options = PlaneStitchOptions {
        step_nm: 125_000,
        maze_step_nm: 125_000,
        maximum_radius_nm: 600_000,
        only_pads: Some(pads(&[("U", "5")])),
        // A two-layer board's B.Cu pour is a surface zone; Python called
        // `_stitch_land` on it directly.
        include_surface_zones: true,
        ..PlaneStitchOptions::default()
    };
    let result = stitch_zone_pads(&board, Some(&options)).unwrap();
    assert_eq!(result.stitched_pads, [pad_ref("U", "5")]);
    let added: Vec<&Via> = result.board.vias.iter().filter(|via| via.net == "N5").collect();
    assert_eq!(added.len(), 1);
    let via = added[0];
    assert_ne!(via.finish, "filled-capped");
    assert!(can_via(&RoutingClearanceIndex::new(&board).unwrap(), via));
    let tracks: Vec<_> = result.board.tracks.iter().filter(|track| track.net == "N5").collect();
    assert!(!tracks.is_empty());
    assert_eq!((tracks[0].start, tracks[tracks.len() - 1].end), (mm(5.0, 5.0), via.position));
}

/// `_build_graph` without sampled via sites or via resources, and the
/// pad's exact access candidates (`_terminal_setup`).
fn terminal_setup(board: &PhysicalBoard) -> (Graph, Vec<PinAccess>, GlobalRouterOptions) {
    let options = GlobalRouterOptions {
        tile_size_nm: 2_500_000,
        escape_step_nm: 125_000,
        pin_access_candidates: 16,
        ..GlobalRouterOptions::default()
    };
    let mut graph = build_graph(board, &options, false).unwrap();
    for sites in graph.via_sites.values_mut() {
        sites.clear();
    }
    graph.resources.retain(|_, resource| resource.layer.is_some());
    let index = RoutingClearanceIndex::new(board).unwrap();
    let choices =
        pin_access_candidates(board, &graph, &pad_ref("U", "5"), "N5", &LAYERS, None, &index, &options).unwrap();
    (graph, choices, options)
}

fn key(net: &str, pad: &str) -> (String, PadReference) {
    (net.to_owned(), pad_ref("U", pad))
}

fn offered(items: Vec<((String, PadReference), Vec<PinAccess>)>) -> AccessOptions {
    items.into_iter().collect()
}

/// `_site_vias`: one via at every registered capacity site.
fn site_vias(graph: &Graph) -> Vec<Via> {
    let mut vias = Vec::new();
    for (name, points) in &graph.via_sites {
        let parts: Vec<&str> = name.split(':').collect();
        let (low, high): (usize, usize) = (parts[1].parse().unwrap(), parts[2].parse().unwrap());
        for position in points {
            let mut site = via(&format!("slot{}", vias.len()), *position, 450_000, 200_000);
            (site.from_layer, site.to_layer) = (graph.layers[low], graph.layers[high]);
            vias.push(site);
        }
    }
    vias
}

#[test]
fn global_terminal_can_use_exact_round_pad_port_without_sampled_grid_via() {
    let board = interstitial_board();
    let (graph, accesses, _) = terminal_setup(&board);
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let launched: Vec<&PinAccess> = accesses.iter().filter(|access| access.via.is_some()).collect();
    assert!(!launched.is_empty());
    assert!(launched.iter().any(|access| access.via.as_ref().unwrap().position == mm(5.375, 5.375)));
    for access in launched {
        assert!(graph.legal_nodes.contains(&access.node) && access.access_position == graph.point(access.node));
        assert!(can_via(&index, access.via.as_ref().unwrap()));
        for track in &access.tracks {
            assert!(index.can_track(&track.net, track.start, track.end, track.width_nm, track.layer).unwrap());
        }
    }
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

#[test]
fn registered_terminal_slots_are_real_coexisting_and_budgeted() {
    let board = interstitial_board();
    let (graph, choices, _) = terminal_setup(&board);
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let (result, held) =
        register_terminal_capacity(&board, &graph, offered(vec![(key("N5", "5"), choices)]), None).unwrap();
    assert_eq!(result.legal_nodes, graph.legal_nodes);
    assert!(held[&key("N5", "5")].iter().any(|access| access.via.is_some()));
    let pool = site_vias(&result);
    assert!(!pool.is_empty());
    assert!(index.candidate_vias_clear(&pool).unwrap());
    assert!(pool.iter().all(|site| can_via(&index, site)));
    for resource in result.resources.values() {
        if resource.layer.is_none() {
            assert_eq!(resource.capacity as usize, result.via_sites[&resource.identifier].len());
        }
    }
    // The Python test then set MAX_TERMINAL_CAPACITY_PROPOSALS to 0 to check
    // the budget; the Rust constant is not patchable (see PORTED-planes.md).
    assert!(board.tracks.is_empty() && board.vias.is_empty());
}

fn first_launch(choices: &[PinAccess]) -> PinAccess {
    choices.iter().find(|access| access.via.is_some()).expect("a via launch").clone()
}

#[test]
fn terminal_launch_displaces_only_colliding_sampled_witnesses() {
    let board = interstitial_board();
    let (graph, choices, _) = terminal_setup(&board);
    let launch = first_launch(&choices);
    let launch_via = launch.via.clone().unwrap();
    let identifier = via_resource_id(&graph, launch.node, launch_via.from_layer, launch_via.to_layer);
    let near = Point::new(launch_via.position.x_nm + 200_000, launch_via.position.y_nm);
    let far = Point::new(1_000_000, 1_000_000);
    // One sampled witness collides with the launch, one is far away.
    let mut sampled = graph.clone();
    sampled.via_sites.insert(identifier.clone(), vec![near, far]);
    let index = RoutingClearanceIndex::new(&board).unwrap();
    let witness = Via { net: "w".to_owned(), position: near, ..launch_via.clone() };
    assert!(!index.candidate_vias_clear(&[launch_via.clone(), witness]).unwrap());

    let (result, held) =
        register_terminal_capacity(&board, &sampled, offered(vec![(key("N5", "5"), vec![launch.clone()])]), None)
            .unwrap();

    assert_eq!(held[&key("N5", "5")], [launch]);
    assert_eq!(result.via_sites[&identifier], [far, launch_via.position]);
    assert!(index.candidate_vias_clear(&site_vias(&result)).unwrap());
    for resource in result.resources.values() {
        if resource.layer.is_none() && resource.identifier == identifier {
            assert_eq!(resource.capacity, 2);
        }
    }
}

#[test]
fn witness_under_an_accepted_launch_is_never_displaced() {
    let board = interstitial_board();
    let (graph, choices, _) = terminal_setup(&board);
    let launch = first_launch(&choices);
    let launch_via = launch.via.clone().unwrap();
    let identifier = via_resource_id(&graph, launch.node, launch_via.from_layer, launch_via.to_layer);
    let near = Point::new(launch_via.position.x_nm + 200_000, launch_via.position.y_nm);
    // N4's launch sits exactly on the sampled witness and is accepted first.
    let sitting = PinAccess {
        pad: pad_ref("U", "4"),
        via: Some(Via { net: "N4".to_owned(), position: near, ..launch_via.clone() }),
        ..launch.clone()
    };
    let mut sampled = graph.clone();
    sampled.via_sites.insert(identifier.clone(), vec![near]);

    let (result, held) = register_terminal_capacity(
        &board,
        &sampled,
        offered(vec![(key("N4", "4"), vec![sitting.clone()]), (key("N5", "5"), vec![launch])]),
        None,
    )
    .unwrap();

    assert_eq!(held[&key("N4", "4")], [sitting]);
    assert!(held[&key("N5", "5")].is_empty());
    assert_eq!(result.via_sites[&identifier], [near]);
}

#[test]
fn only_a_pad_left_without_access_tries_its_wider_candidates() {
    let board = interstitial_board();
    let (graph, choices, _) = terminal_setup(&board);
    let launch = first_launch(&choices);
    let vias: Vec<PinAccess> = choices.iter().filter(|access| access.via.is_some()).cloned().collect();
    let calls: RefCell<Vec<(String, PadReference)>> = RefCell::new(Vec::new());
    let expand = |pad: &(String, PadReference)| -> copper_route::Result<Vec<PinAccess>> {
        calls.borrow_mut().push(pad.clone());
        Ok(vias.clone())
    };

    // The pad's only offered launch is illegal, so it alone expands.
    let mut blocked = launch.clone();
    blocked.node.x_index = 1_000_000;
    let (_, held) =
        register_terminal_capacity(&board, &graph, offered(vec![(key("N5", "5"), vec![blocked])]), Some(&expand))
            .unwrap();
    assert_eq!(*calls.borrow(), [key("N5", "5")]);
    let kept = &held[&key("N5", "5")];
    assert!(kept.len() == 1 && kept[0].via.is_some(), "{kept:?}");

    calls.borrow_mut().clear();
    register_terminal_capacity(&board, &graph, offered(vec![(key("N5", "5"), vec![launch])]), Some(&expand)).unwrap();
    assert!(calls.borrow().is_empty());
}

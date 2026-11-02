//! Fresh global guides can enter through already verified package copper
//! (ported from the former `tests/test_global_reserved_access.py`). Python's
//! `FanoutResult.routing_accesses` is built here from the fanout's via
//! accesses and surface accesses.

mod support_routers;

use copper_pcb::board::{PadReference, PadViaInPadRule, PhysicalBoard, PhysicalNet, Stackup};
use copper_physical::footprint::{PadShape, footprint_geometry_digest};
use copper_physical::{CopperLayer, DesignRules};
use copper_route::fanout::{FanoutResult, route_fanout};
use copper_route::pin_escape::Anchor;
use copper_route::routing::build_graph;
use copper_route::routing_clearance::RoutingClearanceIndex;
use copper_route::{GlobalRouterOptions, route_global};
use indexmap::IndexMap;
use support_routers::*;

fn options() -> GlobalRouterOptions {
    GlobalRouterOptions { tile_size_nm: mm(2.0), ..GlobalRouterOptions::default() }
}

/// `tests/test_fanout_and_plane_verify.py::_dense_board`.
fn dense_board() -> PhysicalBoard {
    let pads = (0..12).map(|i| pad(&(i + 1).to_string(), -2.75 + 0.5 * i as f64, 0.0, 0.25, 0.5)).collect();
    let dense = footprint("dense", pads, 6.0, 2.0);
    let terminal = footprint("terminal", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 1.0, 1.0);
    board(
        "Fanout",
        20.0,
        20.0,
        vec![dense, terminal],
        vec![placement("U1", "dense", 8.0, 10.0), placement("J1", "terminal", 16.0, 10.0)],
        vec![net("SIGNAL", &[("U1", "1"), ("J1", "1")])],
    )
}

fn routing_accesses(fanout: &FanoutResult) -> IndexMap<PadReference, Anchor> {
    let mut anchors: IndexMap<PadReference, Anchor> =
        fanout.accesses.iter().map(|(pad, point)| (pad.clone(), Anchor::Point(*point))).collect();
    for (pad, access) in fanout.surface_accesses.iter().flatten() {
        anchors.insert(pad.clone(), Anchor::Access(access.clone()));
    }
    anchors
}

#[test]
fn global_guide_uses_actual_owned_launch_without_proposing_a_duplicate_via() {
    let original = dense_board();
    let fanout = route_fanout(&original, None, None).expect("fanout");
    let anchors = routing_accesses(&fanout);
    let pad = PadReference::new("U1", "1");
    let before = (fanout.board.tracks.clone(), fanout.board.vias.clone());
    let result = route_global(&original, &options(), None, Some(&anchors), Some(&fanout.board)).expect("global");
    let route = result.routes.iter().find(|r| r.net == "SIGNAL").expect("SIGNAL");
    assert!(route.connected, "{:?}", route.diagnostics);
    let access = route.accesses.iter().find(|a| a.pad == pad).expect("U1.1 access");
    assert_eq!(access.retained_via.as_ref(), Some(&fanout.board.vias[0]));
    assert!(access.via.is_none());
    assert!(fanout.created_tracks.iter().all(|t| access.tracks.contains(t)));
    let retained = access.retained_via.as_ref().expect("retained").position;
    assert!(!route.vias.iter().any(|v| v.position == retained));
    assert_eq!((fanout.board.tracks.clone(), fanout.board.vias.clone()), before);
    assert!(result.fixed_prefix_digest.is_some());
    let again = route_global(&original, &options(), None, Some(&anchors), Some(&fanout.board)).expect("global");
    assert_eq!(result, again);
}

#[test]
fn invalid_reserved_launch_fails_closed_without_substituting_a_new_exit() {
    for damage in ["missing_track", "wrong_position", "foreign_via"] {
        let fanout = route_fanout(&dense_board(), None, None).expect("fanout");
        let mut anchors = routing_accesses(&fanout);
        let mut held = fanout.board.clone();
        match damage {
            "missing_track" => held.tracks.clear(),
            "wrong_position" => {
                anchors.insert(PadReference::new("U1", "1"), Anchor::Point(pt(6.0, 8.0)));
            }
            _ => {
                held.nets.push(PhysicalNet { name: "OTHER".to_owned(), pads: Vec::new() });
                for via in &mut held.vias {
                    via.net = "OTHER".to_owned();
                }
            }
        }
        let mut source = dense_board();
        source.nets = held.nets.clone();
        if damage == "foreign_via" {
            let error = route_global(&source, &GlobalRouterOptions::default(), None, Some(&anchors), Some(&held))
                .expect_err("foreign via");
            assert!(error.to_string().contains("fixed global prefix fails fresh physical DRC"), "{error}");
            continue;
        }
        let result = route_global(&source, &options(), None, Some(&anchors), Some(&held)).expect("global");
        let route = &result.routes[0];
        assert!(!route.connected, "{damage}");
        assert!(route.diagnostics[0].contains("no legal bounded"), "{damage}: {:?}", route.diagnostics);
    }
}

#[test]
fn fixed_prefix_cannot_change_placement_or_source_copper() {
    let source = dense_board();
    let fanout = route_fanout(&source, None, None).expect("fanout");
    let anchors = routing_accesses(&fanout);
    let mut shifted = fanout.board.clone();
    shifted.placements[0].position = pt(9.0, 10.0);
    let options = GlobalRouterOptions::default();
    let error = route_global(&source, &options, None, Some(&anchors), Some(&shifted)).expect_err("shifted");
    assert!(error.to_string().contains("changes source contracts"), "{error}");
    let error = route_global(&source, &options, None, Some(&anchors), None).expect_err("no prefix");
    assert!(error.to_string().contains("require an explicitly fixed"), "{error}");
}

/// `tests/test_hard_macros.py::fixture`'s bound macro: a chip and a passive
/// joined by one immutable macro track.
fn macro_board() -> PhysicalBoard {
    let fp = footprint("test", vec![pad("1", 0.0, 0.0, 0.2, 0.2)], 0.5, 0.5);
    let digest = footprint_geometry_digest(&fp);
    let source = board(
        "MacroTest",
        30.0,
        30.0,
        vec![fp],
        vec![placement("U1", "test", 10.0, 10.0), placement("R1", "test", 12.0, 10.0)],
        vec![net("N", &[("U1", "1"), ("R1", "1")])],
    );
    let region = r#""id":"private","layers":["F.Cu"],"vertices":[[1000000,-500000],[3000000,-500000],[3000000,500000],[1000000,500000]],"block_vias":true"#;
    let asset = format!(
        concat!(
            r#"{{"allowed_rotations":[0,45,90],"anchor":"chip","internal_clearance_nm":0,"isolated_pads":[],"#,
            r#""keepouts":[{{{region},"block_tracks":false,"block_zones":true}}],"#,
            r#""members":[{{"center_nm":[0,0],"edge_clearance_nm":250000,"footprint":"test","footprint_digest":"{digest}","reference":"chip","rotation_degrees":"0"}},"#,
            r#"{{"center_nm":[2000000,0],"edge_clearance_nm":250000,"footprint":"test","footprint_digest":"{digest}","reference":"passive","rotation_degrees":"0"}}],"#,
            r#""pad_nets":[["chip","1","signal"],["passive","1","signal"]],"#,
            r#""ports":[{{"layer":"F.Cu","name":"signal","net":"signal","pads":[["chip","1"],["passive","1"]],"point":{{"pad":["chip","1"]}}}}],"#,
            r#""production_publishable":false,"protected_regions":[{{{region},"block_tracks":true,"block_zones":false}}],"#,
            r#""required_layers":["F.Cu","B.Cu"],"schema":"copperlib-physical-hard-macro/v0.1","source":"synthetic test","#,
            r#""tracks":[{{"layer":"F.Cu","net":"signal","points":[{{"pad":["chip","1"]}},{{"pad":["passive","1"]}}],"width_nm":200000}}],"#,
            r#""unresolved":["synthetic, not fabrication"],"vias":[]}}"#,
            "\n"
        ),
        region = region,
        digest = digest
    );
    let raw = asset.into_bytes();
    let sha: String = {
        use sha2::Digest;
        sha2::Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect()
    };
    let bindings: IndexMap<String, String> =
        [("chip", "U1"), ("passive", "R1")].iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect();
    let nets: IndexMap<String, String> = IndexMap::from([("signal".to_owned(), "N".to_owned())]);
    copper_pcb::hard_macros::bind_hard_macro(
        source,
        std::path::Path::new("macro.json"),
        &raw,
        &sha,
        "unit",
        &bindings,
        &nets,
    )
    .expect("bind")
}

#[test]
fn fixed_global_prefix_preserves_immutable_macro_tracks() {
    let source = macro_board();
    let held = copper_drc::macros::materialize_hard_macros(source.clone()).expect("materialize");
    let before = held.tracks.clone();
    let result = route_global(&source, &GlobalRouterOptions::default(), None, None, Some(&held)).expect("global");
    assert!(result.routes[0].connected);
    assert_eq!(held.tracks, before);
    let mut changed = held.clone();
    changed.tracks[0].width_nm = 100_000;
    let error = route_global(&source, &GlobalRouterOptions::default(), None, None, Some(&changed)).expect_err("tamper");
    assert!(error.to_string().contains("immutable"), "{error}");
}

/// `tests/test_pad_via_arrays.py::_array_board(rows=3, columns=3, pitch_nm=750_000)`:
/// a QFN-16 with an exposed GND land between two resistors.
fn array_board() -> PhysicalBoard {
    let ep = 3.1;
    let reach = ep / 2.0 + 0.55;
    let mut pads = Vec::new();
    for (side, (x, y)) in [(-1.0, 0.0), (0.0, 1.0), (1.0, 0.0), (0.0, -1.0)].iter().enumerate() {
        for index in 0..4 {
            let offset = (index as f64 - 1.5) * 0.65;
            let number = (side * 4 + index + 1).to_string();
            pads.push(if *x != 0.0 {
                pad(&number, x * reach, offset, 0.8, 0.3)
            } else {
                pad(&number, offset, y * reach, 0.3, 0.8)
            });
        }
    }
    pads.push(copper_physical::footprint::FootprintPad { shape: PadShape::Rectangle, ..pad("17", 0.0, 0.0, ep, ep) });
    let qfn = footprint("test/qfn16-ep", pads, 2.0 * reach + 2.0, 2.0 * reach + 2.0);
    let resistor = footprint("test/0603", vec![pad("1", -0.8, 0.0, 0.8, 0.9), pad("2", 0.8, 0.0, 0.8, 0.9)], 3.2, 1.8);
    let mut result = board(
        "ViaArray",
        30.0,
        20.0,
        vec![qfn, resistor],
        vec![
            placement("U1", "test/qfn16-ep", 15.0, 10.0),
            placement("R1", "test/0603", 7.0, 10.0),
            placement("R2", "test/0603", 23.0, 10.0),
        ],
        vec![
            net("GND", &[("U1", "17"), ("U1", "4"), ("R1", "1")]),
            net("A", &[("U1", "1"), ("R1", "2")]),
            net("B", &[("U1", "9"), ("R2", "1")]),
        ],
    );
    result.stackup = Stackup::new(layers(6));
    result.zones =
        vec![zone("ground", "GND", &[CopperLayer::parse("In1.Cu").expect("In1")], rect(1.0, 1.0, 29.0, 19.0))];
    result.metadata.insert("fabrication_profile".to_owned(), "jlcpcb-six-layer".to_owned());
    result.rules = DesignRules {
        minimum_clearance_nm: mm(0.09),
        minimum_track_width_nm: mm(0.09),
        default_track_width_nm: mm(0.2),
        ..DesignRules::default()
    };
    result.via_in_pad_rules = vec![PadViaInPadRule {
        pad: PadReference::new("U1", "17"),
        rows: Some(3),
        columns: Some(3),
        pitch_nm: Some(750_000),
    }];
    result
}

#[test]
fn fixed_global_prefix_cannot_drop_required_array_multiplicity() {
    copper_drc::macros::register_array_materializer(copper_route::pad_via_arrays::materialize_via_in_pad_arrays);
    let source = array_board();
    let held = copper_drc::macros::materialize_hard_macros(source.clone()).expect("materialize");
    assert_eq!(held.vias.len(), 9);
    let mut dropped = held.clone();
    dropped.vias.pop();
    let error = route_global(&source, &GlobalRouterOptions::default(), None, None, Some(&dropped)).expect_err("drop");
    assert!(error.to_string().contains("immutable copper or required arrays"), "{error}");
    assert_eq!(held.vias.len(), 9);
    assert!(source.vias.is_empty());
}

/// Adapted: Python patched `local_access_path` to prove the checked corner
/// avoids the bounded maze; the result (a retained launch through the held
/// track, every piece clear) is checked. The two-node graph is the real
/// graph of a one-layer board instead of a hand-built `_Graph`.
#[test]
fn reserved_port_tries_checked_corner_before_a_bounded_maze() {
    let mut source = board(
        "DetailedRoute",
        20.0,
        12.0,
        vec![one_pad()],
        vec![placement("J1", "test/one-pad", 3.0, 6.0), placement("J2", "test/one-pad", 17.0, 6.0)],
        vec![net("SIGNAL", &[("J1", "1"), ("J2", "1")])],
    );
    let launch = pt(4.0, 6.0);
    let held_track = track("SIGNAL", (3.0, 6.0), (4.0, 6.0), 0.25, CopperLayer::Front);
    let held_via = via("SIGNAL", (4.0, 6.0), 0.6, 0.3, CopperLayer::Front, CopperLayer::Back);
    source.tracks = vec![held_track.clone()];
    source.vias = vec![held_via.clone()];
    let graph = build_graph(
        &{
            let mut bare = source.clone();
            bare.tracks.clear();
            bare.vias.clear();
            bare
        },
        &options(),
        false,
    )
    .expect("graph");
    let clear = RoutingClearanceIndex::new(&source).expect("index");
    let choices = copper_route::global_reserved_access::reserved_pin_access_candidates(
        &source,
        &graph,
        &PadReference::new("J1", "1"),
        "SIGNAL",
        &[CopperLayer::Front],
        None,
        &clear,
        &GlobalRouterOptions::default(),
        &Anchor::Point(launch),
    )
    .expect("choices");
    assert!(!choices.is_empty());
    for choice in &choices {
        assert!(!choice.region_only && choice.retained_via.as_ref() == Some(&held_via));
        assert_eq!(choice.tracks[0], held_track);
        for t in &choice.tracks {
            assert!(clear.can_route(&t.net, t.start, t.end, t.width_nm, t.layer).expect("route"));
        }
    }
    assert_eq!((source.tracks, source.vias), (vec![held_track], vec![held_via]));
}

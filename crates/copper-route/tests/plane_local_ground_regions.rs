//! Authored local ground regions keep adjacent package approaches available;
//! ported from `tests/test_plane_local_ground_regions.py`.

mod support_planes;

use copper_pcb::board::{CopperZone, PhysicalBoard};
use copper_pcb::transform::transformed_local_point;
use copper_physical::CopperLayer;
use copper_route::RoutingClearanceIndex;
use copper_route::plane::{PlaneStitchOptions, PlaneStitchResult, stitch_zone_pads};
use indexmap::IndexSet;
use support_planes::*;

fn surface() -> PlaneStitchOptions {
    PlaneStitchOptions { include_surface_zones: true, ..PlaneStitchOptions::default() }
}

fn stitch(board: &PhysicalBoard, options: Option<&PlaneStitchOptions>) -> PlaneStitchResult {
    stitch_zone_pads(board, options).unwrap()
}

#[test]
fn local_region_keeps_individual_returns_behind_interleaved_signal_lands() {
    for (rotation, name) in [(0, "U"), (90, "RENAMED"), (180, "ADC_FREE"), (270, "OTHER")] {
        let (board, pose) = local_ground_scene(rotation, name);
        let result = stitch(&board, Some(&surface()));
        assert!(result.complete() && result.added_via_count == 3, "{name}: {:?}", result.pending_pads);
        // The explicit region, rather than a package or net-specific
        // direction, supplies the return topology. Every foreign signal
        // retains its approach.
        let index = RoutingClearanceIndex::new(&result.board).unwrap();
        for (i, x) in [(1, -1.5), (3, -0.5), (5, 0.5), (7, 1.5)] {
            let start = transformed_local_point(&pose, mm(x, -3.0));
            let end = transformed_local_point(&pose, mm(x, -6.0));
            assert!(index.can_track(&format!("S{i}"), start, end, 200_000, CopperLayer::Front).unwrap(), "{name} S{i}");
        }
        assert_eq!(result.board.vias[..1], board.vias[..], "{name}");
        let via_sets: IndexSet<&Vec<copper_pcb::board::Via>> = result.copper_uses.iter().map(|u| &u.vias).collect();
        assert_eq!(via_sets.len(), 3, "{name}");
        assert!(result.copper_uses.iter().all(|u| u.kind == "contact"), "{name}");
        assert!(hard_findings(&result.board).is_empty(), "{name}: {:?}", hard_findings(&result.board));
        let repeated = stitch(&result.board, Some(&surface()));
        assert_eq!(repeated.board.tracks, result.board.tracks, "{name}");
        assert_eq!(repeated.board.vias, result.board.vias, "{name}");
    }
}

#[test]
fn unreserved_or_distant_surface_region_does_not_rewrite_default_via_ranking() {
    let (board, _) = local_ground_scene(0, "U");
    let legacy = PhysicalBoard { zones: board.zones[..1].to_vec(), ..board.clone() };
    let baseline = stitch(&legacy, Some(&surface()));
    let unreserved = CopperZone { reserve_routing: false, ..board.zones[1].clone() };
    let distant = CopperZone { outer: rect(15.0, 15.0, 18.0, 18.0), ..board.zones[1].clone() };
    for region in [unreserved, distant] {
        let id = region.reserve_routing;
        let variant = PhysicalBoard { zones: vec![board.zones[0].clone(), region], ..board.clone() };
        let result = stitch(&variant, Some(&surface()));
        assert_eq!(result.board.tracks, baseline.board.tracks, "reserved={id}");
        assert_eq!(result.board.vias, baseline.board.vias, "reserved={id}");
    }
}

#[test]
fn unreachable_local_region_keeps_legal_outside_contacts_available() {
    let (mut board, _) = local_ground_scene(0, "U");
    board.copper_keepouts = vec![keepout("blocked-local-approach", &[CopperLayer::Front], rect(2.0, 8.1, 18.0, 8.7))];
    let result = stitch(&board, Some(&surface()));
    assert!(result.complete() && result.added_via_count == 3, "{:?}", result.pending_pads);
    assert!(result.board.vias[1..].iter().all(|v| v.position.y_nm < 7_000_000), "{:?}", result.board.vias);
    assert_eq!(result.board.vias[..1], board.vias[..]);
}

#[test]
fn equal_cost_passive_returns_prefer_departure_away_from_signal_land() {
    for rotation in [0, 90, 180, 270] {
        let (scene, _) = local_ground_scene(0, "U");
        let passive = footprint(
            "passive",
            vec![pad("1", mm(0.0, 0.775), size(0.85, 0.95)), pad("2", mm(0.0, -0.775), size(0.85, 0.95))],
            size(1.46, 2.96),
        );
        let pose = place_rotated("PASSIVE", "passive", mm(10.0, 10.0), rotation);
        let start = transformed_local_point(&pose, passive.pads[1].position);
        let mut board = PhysicalBoard {
            placements: vec![pose.clone()],
            nets: vec![net("GND", &[("PASSIVE", "2")]), net("INPUT", &[("PASSIVE", "1")])],
            zones: scene.zones[..1].to_vec(),
            vias: Vec::new(),
            ..scene
        };
        board.footprints.clear();
        add_footprint(&mut board, passive);
        let result = stitch(&board, None);
        assert!(result.complete() && result.added_via_count == 1, "{rotation}");
        let outward = transformed_local_point(&pose, mm(0.0, -1.775));
        assert_eq!(result.board.vias[0].position, outward, "{rotation}");
        assert_eq!(result.board.tracks[0].start, start, "{rotation}");
    }
}

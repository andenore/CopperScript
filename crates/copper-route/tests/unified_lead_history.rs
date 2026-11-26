//! Exact off-grid conflict prices must survive both lattice implementations.
mod support_routers;

use copper_pcb::board::Stackup;
use copper_physical::{CopperLayer, DesignRules};
use copper_route::RoutingClearanceIndex;
use copper_route::unified_router::{UnifiedRouterOptions, router::Router};
use indexmap::IndexSet;
use support_routers::*;

fn check_exact_lead_history(fine_regions: &str, nested: bool) {
    let mut board = board(
        "lead-history",
        20.0,
        20.0,
        vec![ball_array("array", 5, 0.75, 0.35)],
        vec![placement("U", "array", 10.0, 10.0)],
        Vec::new(),
    );
    board.stackup = Stackup::new(layers(4));
    board.rules = DesignRules {
        minimum_clearance_nm: mm(0.1),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.1),
        default_via_size_nm: mm(0.4),
        default_via_drill_nm: mm(0.2),
        minimum_hole_clearance_nm: mm(0.15),
        ..DesignRules::default()
    };
    let original = board.clone();
    let options = UnifiedRouterOptions { fine_regions: fine_regions.to_owned(), ..Default::default() };
    let mut router = Router::new(
        &board,
        &options,
        &[],
        IndexSet::new(),
        None,
        RoutingClearanceIndex::new(&board).unwrap(),
    )
    .unwrap();
    assert_eq!(router.lattice.is_nested(), nested);
    let lead = vec![track("A", (6.01, 7.02), (7.61, 7.02), 0.1, CopperLayer::Front)];
    let alternative = vec![track("A", (6.01, 6.52), (7.61, 6.52), 0.1, CopperLayer::Front)];
    router.occupancy.history.fill(2.0);
    router.occupancy.occ.fill(1);
    let history = router.occupancy.history.clone();
    let occupancy = router.occupancy.occ.clone();
    let baseline = router.lead_penalty(&lead, 0.6);
    let alternative_baseline = router.lead_penalty(&alternative, 0.6);
    assert!(baseline > 0.0, "exercise sampled-node costs, not the empty-node fallback");

    // This is the exact proposal-specific price accumulated by conflict feedback.
    // It must add to, rather than replace or disappear behind, raster prices.
    router.lead_history.insert(lead[0].clone(), 7.0);
    let priced = router.lead_penalty(&lead, 0.6);
    assert!((priced - baseline - 7.0).abs() < 1e-10, "exact price lost: baseline={baseline}, priced={priced}");
    assert_eq!(router.lead_penalty(&alternative, 0.6), alternative_baseline);
    assert_eq!(router.occupancy.history, history);
    assert_eq!(router.occupancy.occ, occupancy);
    assert_eq!(board, original, "cost evaluation must not change source copper or rules");
}

#[test]
fn dense_lattice_consumes_exact_off_grid_lead_history() {
    check_exact_lead_history("dense", true);
}

#[test]
fn uniform_lattice_keeps_exact_off_grid_lead_history() {
    check_exact_lead_history("off", false);
}

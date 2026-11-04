//! The critical pair state cap (ported from the former
//! `tests/test_unified_pair_budget.py`). The forwarding test spied on
//! `flow.route_critical_nets` and is skipped (see `PORTED-routers.md`).

mod support_routers;

use copper_route::flow::{RoutingPipelineOptions, run_routing_pipeline};
use copper_route::unified_router::UnifiedRouterOptions;
use support_routers::*;

#[test]
fn negative_stage_cap_is_rejected() {
    let board = board(
        "budget",
        20.0,
        12.0,
        vec![footprint("one", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 1.0, 1.0)],
        vec![placement("A", "one", 3.0, 6.0), placement("B", "one", 17.0, 6.0)],
        vec![net("S", &[("A", "1"), ("B", "1")])],
    );
    let options = RoutingPipelineOptions { critical_pair_state_limit: -1, ..RoutingPipelineOptions::default() };
    let error = run_routing_pipeline(board, options, None).expect_err("negative cap");
    assert!(error.to_string().contains("critical pair state limit"), "{error}");
}

/// `True` and `2.5` are JSON overrides here (the typed option holds an integer).
#[test]
fn invalid_pair_caps_are_rejected() {
    for limit in [0, -1] {
        let options =
            UnifiedRouterOptions { critical_pair_state_limit: Some(limit), ..UnifiedRouterOptions::default() };
        let error = options.validated().expect_err("invalid cap");
        assert!(error.to_string().contains("critical pair state limit"), "{limit}: {error}");
    }
    for value in ["true", "2.5"] {
        let overrides = [("critical_pair_state_limit".to_owned(), copper_json::parse(value).expect("json"))];
        let error = UnifiedRouterOptions::default().with_overrides(&overrides).expect_err("invalid cap");
        assert!(error.to_string().contains("critical pair state limit"), "{value}: {error}");
    }
}

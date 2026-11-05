//! Unified router option defaults and validation, and the fanout-access
//! guard of `route_unified` (ported from the former
//! `tests/test_unified_router_options.py`). The failed-net retry and
//! via-budget tests replaced router internals with scripted fakes and are
//! skipped (see `PORTED-routers.md`); `_open_count` is a unit test in
//! `src/unified_router/tests.rs`.

mod support_routers;

use copper_json::Value;
use copper_physical::CopperLayer;
use copper_route::pin_escape::RoutingAccess;
use copper_route::unified_router::UnifiedRouterOptions;
use copper_route::unified_router::route::route_unified;
use indexmap::IndexMap;
use support_routers::routes::guides;
use support_routers::*;

#[test]
fn tunable_defaults_equal_the_former_literals() {
    let o = UnifiedRouterOptions::default();
    assert_eq!((o.failed_retry_every, o.failed_retry_limit), (3, 0));
    assert!(o.failed_retry_before_exit);
    assert_eq!(o.outward_axis_threshold_nm, 300_000);
    assert_eq!(o.attach_extension_steps_nm, [300_000, 500_000, 750_000, 1_000_000, 1_500_000]);
    assert_eq!(o.attach_extension_reach_nm, 600_000);
    assert_eq!((o.local_escape_detour_nm, o.local_escape_states), (500_000, 20_000));
    assert_eq!(o.open_flood_limit, 600);
    assert_eq!(o.open_space_area_nm2, 2_000_000i64.pow(2));
    assert_eq!((o.plane_piece_area_nm2, o.plane_piece_fraction), (800_000i64.pow(2), 0.25));
    assert_eq!(o.repair_neighbour_reach_nm, 1_000_000);
    assert_eq!((o.via_budget_factor, o.via_overrun_factor), (4.0, 10.0));
    assert_eq!(o.clone().validated().expect("valid"), o);
}

fn json(text: &str) -> Value {
    copper_json::parse(text).expect("json")
}

/// Adapted: `bench --set` passes JSON values; a JSON list of steps becomes
/// the step list (Python's tuple/hash check has no Rust counterpart).
#[test]
fn tunables_are_validated_from_json_overrides() {
    let base = UnifiedRouterOptions::default();
    let set = |name: &str, value: &str| base.with_overrides(&[(name.to_owned(), json(value))]);
    assert_eq!(
        set("attach_extension_steps_nm", "[200000, 400000]").expect("steps").attach_extension_steps_nm,
        [200_000, 400_000]
    );
    for (name, value) in [
        ("failed_retry_every", "-1"),
        ("failed_retry_limit", "-1"),
        ("attach_extension_steps_nm", "[]"),
        ("attach_extension_steps_nm", "[0.3]"),
        ("attach_extension_steps_nm", "[-1]"),
        ("local_escape_states", "0"),
        ("open_flood_limit", "0"),
        ("plane_piece_fraction", "0"),
        ("plane_piece_fraction", "1.5"),
        ("via_budget_factor", "0"),
        ("via_overrun_factor", "0.5"),
        ("repair_neighbour_reach_nm", "-1"),
        ("open_space_area_nm2", "-1"),
    ] {
        let error = set(name, value).expect_err(name);
        assert!(matches!(error, copper_route::unified_error::Error::Value(_)), "{name}={value}: {error:?}");
    }
}

#[test]
fn unconsumed_fanout_accesses_are_rejected() {
    let board = pad_board(&[("A", &[(3.0, 4.0), (17.0, 4.0)])], 2, vec![]);
    let global_route = guides(&board);
    let options = UnifiedRouterOptions::default();
    let launch = pt(6.0, 7.0);
    let access = RoutingAccess::new(
        launch,
        CopperLayer::Front,
        launch,
        vec![track("A", (6.0, 7.0), (6.0, 7.0), 0.2, CopperLayer::Front)],
    )
    .expect("access");
    let fanout: IndexMap<_, _> = [(copper_pcb::board::PadReference::new("A_0", "1"), access)].into_iter().collect();
    let error = route_unified(&board, &global_route, &options, Some(&fanout), None, None, None, None).unwrap_err();
    assert!(error.to_string().contains("does not consume verified fanout accesses"), "{error}");
    // None and an empty mapping are the pipeline's unified-path inputs.
    for accesses in [None, Some(IndexMap::new())] {
        let result =
            route_unified(&board, &global_route, &options, accesses.as_ref(), None, None, None, None).expect("route");
        assert!(result.nets.iter().all(|net| net.connected));
    }
}

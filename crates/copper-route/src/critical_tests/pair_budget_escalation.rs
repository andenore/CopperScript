//! `tests/test_pair_budget_escalation.py`: an exhausted bounded pair search
//! escalates its state budget before giving up.

use copper_pcb::board::NetRoutingRule;

use crate::critical::{CriticalNetResult, CriticalOptions, route_critical_nets};
use crate::test_support_critical::*;
use crate::unified_router::UnifiedRouterOptions;

fn route<'a>(limit: i64, change: impl FnOnce(&mut CriticalOptions<'a>)) -> crate::Result<CriticalNetResult> {
    let board = super::critical_diagnostics::bent_pair_board(|r| NetRoutingRule { max_skew_nm: Some(5 * MM), ..r });
    let guides = global_route(&board, crate::routing::GlobalRouterOptions::default().tile_size_nm);
    let mut options = CriticalOptions { pair_state_limit: Some(limit), ..CriticalOptions::default() };
    change(&mut options);
    Ok(route_critical_nets(board, &guides, &options, None)?.nets.remove(0))
}

#[test]
fn an_exhausted_pair_search_escalates_once_and_records_the_bound() {
    // The bent pair needs about 100 states; a 20-state tier is exhausted with
    // no candidate. The escalated tier (10 x 20) connects it with the same
    // ports and gates, and the result records the bound it ended under.
    let bounded = route(20, |o| o.pair_state_escalation = Some(1)).expect("routed");
    assert!(!bounded.connected && bounded.pair_budget_exhausted && bounded.pair_state_limit == Some(20), "{bounded:?}");
    let escalated = route(20, |o| o.pair_state_escalation = Some(10)).expect("routed");
    assert!(escalated.connected && escalated.pair_state_limit == Some(200), "{escalated:?}");
    assert!(20 < escalated.search_states && escalated.search_states <= 200 && !escalated.pair_budget_exhausted);
}

#[test]
fn escalation_comes_from_the_unified_options() {
    // The unified router's option drives it (default 10), also when the
    // caller passes no options.
    assert_eq!(UnifiedRouterOptions::default().critical_pair_state_escalation, 10);
    let unified = |escalation: i64| UnifiedRouterOptions {
        critical_pair_state_escalation: escalation,
        ..UnifiedRouterOptions::default()
    };
    let (ten, one) = (unified(10), unified(1));
    assert!(route(20, |o| o.unified_options = Some(&ten)).expect("routed").connected);
    assert!(!route(20, |o| o.unified_options = Some(&one)).expect("routed").connected);
    assert!(route(20, |_| {}).expect("routed").connected);
}

#[test]
fn a_pair_within_its_tier_never_escalates() {
    let plain = route(2000, |o| o.pair_state_escalation = Some(10)).expect("routed");
    assert!(plain.connected && plain.pair_state_limit == Some(2000), "{plain:?}");
}

#[test]
fn escalation_must_be_a_positive_factor() {
    let zero = UnifiedRouterOptions { critical_pair_state_escalation: 0, ..UnifiedRouterOptions::default() };
    assert!(zero.validated().is_err());
    assert!(route(20, |o| o.pair_state_escalation = Some(0)).is_err());
}

//! Geometric cost regressions from the six-layer routing review (ported from
//! the former `tests/test_routing_review_regressions.py`).

use copper_physical::Point;
use copper_route::routing_costs::{length_cost, preference_cost};

#[test]
fn costs_are_physical_and_invariant_to_edge_splitting() {
    let (a, b, c) = (Point::new(0, 0), Point::new(300_000, 0), Point::new(1_000_000, 0));
    let cost = |first, second| length_cost(first, second, 10) + preference_cost(first, second, 2, 4, 2, false);
    assert_eq!(cost(a, c), cost(a, b) + cost(b, c));
    assert_eq!(cost(a, c), 20_000_000);
}

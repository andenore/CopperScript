//! Ordinary escape proposals cannot invalidate a complete bypass cohort;
//! ported from `tests/test_crosslayer_fanout_budgets.py`.

mod support_planes;

use copper_route::bypass_links::verified_bypass_links;
use copper_route::bypass_reservations::verified_bypass_reservations;
use copper_route::fanout::route_fanout;
use indexmap::IndexSet;
use support_planes::decoupling::*;
use support_planes::*;

#[test]
fn ordinary_escape_cannot_spend_prior_crosslayer_supply_via_budget() {
    for renamed in [false, true] {
        let (mut source, link) = crosslayer(renamed, true);
        // A second supply land of the chip, with one via allowed for the net.
        source.footprints.get_mut("chip").unwrap().pads.push(pad("X", mm(1.0, -0.8), size(0.4, 0.4)));
        let extra = pad_ref(&link.target.component, "X");
        for item in source.nets.iter_mut().filter(|n| n.name == link.net) {
            item.pads.push(extra.clone());
        }
        for item in source.net_routing_rules.iter_mut().filter(|r| r.net == link.net) {
            item.max_vias = Some(1);
        }
        let result = route_fanout(&source, None, None).unwrap();
        assert!(result.pending_pads.contains(&extra), "{renamed}");
        assert!(result.decoupling_pending_links.is_empty(), "{renamed}: {:?}", result.decoupling_pending_links);
        assert_eq!(result.created_vias.iter().filter(|v| v.net == link.net).count(), 1, "{renamed}");
        let only: IndexSet<String> = IndexSet::from([link.net.clone()]);
        assert_eq!(
            verified_bypass_links(&result.board, &only, None, None, &[]).unwrap(),
            IndexSet::from([link.clone()])
        );
        assert_eq!(verified_bypass_reservations(&source, &reservation(&result)).unwrap().2, None, "{renamed}");
        assert_eq!(result.created_vias, result.decoupling_vias, "{renamed}");
        assert_eq!(result.created_tracks, result.decoupling_tracks, "{renamed}");
    }
}

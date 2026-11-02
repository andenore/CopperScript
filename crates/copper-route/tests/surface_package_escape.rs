//! A pin whose net may not change layer escapes on its own surface (ported
//! from the former `tests/test_surface_package_escape.py`). Python's patched
//! `_surface_layer` check is replaced by a determinism check, and the
//! `pin_analysis` assertion is dropped (Rust's `FanoutResult` does not model
//! pin analysis).

mod support_routers;

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, Stackup};
use copper_physical::{CopperKeepout, CopperLayer, DesignRules};
use copper_route::fanout::route_fanout;
use support_routers::*;

fn pin() -> PadReference {
    PadReference::new("U", "1")
}

/// A 4 mm, 0.5 mm-pitch QFN-24; pin 1 is next to a package corner.
fn qfn() -> copper_physical::footprint::PhysicalFootprint {
    let mut lands = Vec::new();
    for side in 0..4 {
        for i in 0..6 {
            let t = (i as f64 - 2.5) * 0.5;
            let (x, y, w, h) =
                [(-1.9, t, 0.7, 0.25), (t, 1.9, 0.25, 0.7), (1.9, -t, 0.7, 0.25), (-t, -1.9, 0.25, 0.7)][side];
            lands.push(pad(&(lands.len() + 1).to_string(), x, y, w, h));
        }
    }
    lands.push(pad("25", 0.0, 0.0, 2.4, 2.4));
    footprint("qfn24", lands, 4.0, 4.0)
}

/// Crystal-like net X1 from pin 1 to Y; pins 2-7 carry ordinary two-layer nets.
fn surface_board(allowed: Vec<CopperLayer>) -> PhysicalBoard {
    let one = footprint("one", vec![pad("1", 0.0, 0.0, 0.6, 0.6)], 1.0, 1.0);
    let mut poses = vec![placement("U", "qfn24", 12.0, 10.0), placement("Y", "one", 6.0, 7.0)];
    let mut nets = vec![net("X1", &[("U", "1"), ("Y", "1")])];
    for (k, number) in (2..8).enumerate() {
        let (j, n) = (format!("J{number}"), number.to_string());
        poses.push(placement(&j, "one", 4.0 + 1.5 * k as f64, 16.0));
        nets.push(net(&format!("N{number}"), &[("U", n.as_str()), (j.as_str(), "1")]));
    }
    let mut result = board("surface escape", 24.0, 20.0, vec![qfn(), one], poses, nets);
    result.net_routing_rules = vec![NetRoutingRule { allowed_layers: allowed, ..rule("X1") }];
    result.stackup = Stackup::new(layers(4));
    result.rules = DesignRules {
        minimum_clearance_nm: mm(0.1),
        minimum_track_width_nm: mm(0.1),
        default_track_width_nm: mm(0.15),
        default_via_size_nm: mm(0.45),
        default_via_drill_nm: mm(0.2),
        minimum_hole_clearance_nm: mm(0.2),
        ..DesignRules::default()
    };
    result
}

#[test]
fn nets_that_may_change_layer_keep_their_dogbones() {
    for allowed in [layers(4), vec![CopperLayer::Front, CopperLayer::Back]] {
        let source = surface_board(allowed);
        let result = route_fanout(&source, None, None).expect("fanout");
        assert!(result.surface_accesses.is_none());
        assert!(result.accesses.contains_key(&pin()) && result.created_vias.iter().any(|v| v.net == "X1"));
        let again = route_fanout(&source, None, None).expect("fanout");
        assert_eq!(format!("{again:?}"), format!("{result:?}"));
    }
}

#[test]
fn blocked_surface_pin_stays_pending_without_a_via() {
    // An F.Cu track keepout ring across every collar port; vias stay legal.
    let wall = |name: &str, left: f64, top: f64, right: f64, bottom: f64| CopperKeepout {
        block_vias: false,
        block_zones: false,
        ..copper_keepout(name, &[CopperLayer::Front], rect(left, top, right, bottom))
    };
    let mut source = surface_board(vec![CopperLayer::Front]);
    source.copper_keepouts = vec![
        wall("left", 8.8, 6.8, 9.3, 13.2),
        wall("right", 14.7, 6.8, 15.2, 13.2),
        wall("top", 8.8, 6.8, 15.2, 7.3),
        wall("bottom", 8.8, 12.7, 15.2, 13.2),
    ];
    let result = route_fanout(&source, None, None).expect("fanout");
    assert!(result.pending_pads.contains(&pin()));
    let escaped = result.accesses.contains_key(&pin())
        || result.surface_accesses.as_ref().is_some_and(|surface| surface.contains_key(&pin()));
    assert!(!escaped);
    assert!(!result.board.vias.iter().any(|v| v.net == "X1"));
}

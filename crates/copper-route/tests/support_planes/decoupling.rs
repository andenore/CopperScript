//! Cross-surface bypass scenes: `fixture` of
//! `tests/test_decoupling_crosslayer_paths.py`, `scene` of
//! `tests/test_decoupling_existing_launches.py` and `incumbent_scene` of
//! `tests/test_fanout_late_bypass_completion.py`.

use copper_pcb::board::{DecouplingLink, NetRoutingRule, PhysicalBoard, Stackup};
use copper_physical::{BoardSide, CopperLayer, DesignRules, Point};
use copper_route::fanout::FanoutResult;
use indexmap::IndexMap;

use super::*;

/// A chip (supply pad S, return pad G) on the front and a back-side
/// capacitor whose pads 1/2 bypass S to G, on six (or two) layers.
pub fn crosslayer(renamed: bool, six: bool) -> (PhysicalBoard, DecouplingLink) {
    let (u, c, p, g) = if renamed { ("IC_A", "BYPASS_B", "RAIL_C", "RETURN_D") } else { ("U", "C", "P", "R") };
    let mut board = board("crosslayer", 25.0, 20.0);
    add_footprint(
        &mut board,
        footprint(
            "chip",
            vec![pad("S", mm(1.0, 0.0), size(0.4, 0.4)), pad("G", mm(1.0, 2.0), size(0.4, 0.4))],
            size(2.0, 3.0),
        ),
    );
    add_footprint(
        &mut board,
        footprint(
            "cap",
            vec![pad("1", mm(-0.5, 0.0), size(0.4, 0.4)), pad("2", mm(0.5, 0.0), size(0.4, 0.4))],
            size(1.5, 1.0),
        ),
    );
    let link = DecouplingLink {
        capacitor: pad_ref(c, "1"),
        target: pad_ref(u, "S"),
        return_pad: pad_ref(c, "2"),
        net: p.to_owned(),
        return_net: g.to_owned(),
    };
    let mut capacitor = place(c, "cap", mm(13.0, 9.0));
    capacitor.side = BoardSide::Back;
    board.placements = vec![place(u, "chip", mm(8.0, 8.0)), capacitor];
    board.nets = vec![net(p, &[(u, "S"), (c, "1")]), net(g, &[(u, "G"), (c, "2")])];
    board.rules = DesignRules {
        minimum_track_width_nm: 100_000,
        minimum_clearance_nm: 100_000,
        default_track_width_nm: 150_000,
        default_via_size_nm: 450_000,
        default_via_drill_nm: 200_000,
        minimum_hole_clearance_nm: 200_000,
        ..DesignRules::default()
    };
    if six {
        board.stackup = Stackup::new(vec![
            CopperLayer::Front,
            CopperLayer::In1,
            CopperLayer::In2,
            CopperLayer::In3,
            CopperLayer::In4,
            CopperLayer::Back,
        ]);
    }
    board.decoupling_links = vec![link.clone()];
    board.net_routing_rules = [p, g]
        .iter()
        .map(|name| NetRoutingRule {
            width_nm: Some(150_000),
            allowed_layers: vec![CopperLayer::Front, CopperLayer::Back],
            max_vias: Some(2),
            max_length_nm: Some(20_000_000),
            ..rule(name)
        })
        .collect();
    (board, link)
}

/// `scene`: the fixture with ordinary front launches of both chip pads to
/// through vias at (11, 7.5) and (11, 10.5) mm, and one via per net allowed.
pub fn existing_launches(renamed: bool) -> (PhysicalBoard, DecouplingLink) {
    let (mut board, link) = crosslayer(renamed, true);
    let chain = |net: &str, points: &[(f64, f64)]| -> Vec<copper_pcb::board::TrackSegment> {
        points
            .windows(2)
            .map(|pair| track(net, mm(pair[0].0, pair[0].1), mm(pair[1].0, pair[1].1), 150_000, CopperLayer::Front))
            .collect()
    };
    board.tracks = chain(&link.net, &[(9.0, 8.0), (9.5, 8.0), (9.5, 7.5), (11.0, 7.5)]);
    board.tracks.extend(chain(&link.return_net, &[(9.0, 10.0), (9.5, 10.0), (9.5, 10.5), (11.0, 10.5)]));
    board.vias = [(&link.net, 7.5), (&link.return_net, 10.5)]
        .iter()
        .map(|(net, y)| via(net, mm(11.0, *y), 450_000, 200_000))
        .collect();
    for rule in &mut board.net_routing_rules {
        rule.max_vias = Some(1);
    }
    (board, link)
}

pub const EARLY_UNAVAILABLE: &str = "early source launch unavailable";

/// `incumbent_scene`: the source board without copper, the link, an
/// incumbent fanout holding the scene's ordinary launches as via ports with
/// the bypass pending, and the original pending obligations.
pub fn incumbent_scene() -> (PhysicalBoard, DecouplingLink, FanoutResult, Vec<(DecouplingLink, String)>) {
    let (board, link) = existing_launches(false);
    let mut source = board.clone();
    source.tracks.clear();
    source.vias.clear();
    let ports: IndexMap<copper_pcb::board::PadReference, Point> =
        [(link.target.clone(), board.vias[0].position), (pad_ref(&link.target.component, "G"), board.vias[1].position)]
            .into_iter()
            .collect();
    let pending = vec![(link.clone(), EARLY_UNAVAILABLE.to_owned())];
    let incumbent = FanoutResult {
        accesses: ports,
        pending_pads: vec![link.capacitor.clone(), link.return_pad.clone()],
        added_track_count: board.tracks.len() as i64,
        added_via_count: board.vias.len() as i64,
        created_tracks: board.tracks.clone(),
        created_vias: board.vias.clone(),
        assignment: None,
        surface_accesses: None,
        decoupling_tracks: Vec::new(),
        decoupling_vias: Vec::new(),
        decoupling_witnesses: Vec::new(),
        decoupling_pending_links: pending.clone(),
        board,
    };
    (source, link, incumbent, pending)
}

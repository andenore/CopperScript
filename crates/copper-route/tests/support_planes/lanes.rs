//! `_lanes_board` of `tests/test_length_match_tuning.py`: one straight
//! differential pair per lane, all lanes in one length-match group.

use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::{CopperKeepout, CopperLayer, DesignRules};

use super::*;

pub const WIDTH: i64 = 200_000;
pub const GAP: i64 = 200_000;
pub const SPACING: i64 = WIDTH + GAP;
pub const AMPLITUDE: i64 = 1_000_000;
pub const PAIR_SKEW: i64 = 50_000;

/// Lanes are (name, y, x of the package lands, x of the connector lands) in mm.
/// LANE_A is 28 mm long, LANE_B 20 mm and LANE_C 24 mm.
pub type Lane = (&'static str, f64, f64, f64);
pub const THREE_LANES: [Lane; 3] =
    [("LANE_A", 6.0, 5.0, 33.0), ("LANE_B", 14.0, 5.0, 25.0), ("LANE_C", 22.0, 5.0, 29.0)];
pub const TWO_LANES: [Lane; 2] = [("LANE_A", 6.0, 5.0, 33.0), ("LANE_B", 14.0, 5.0, 29.0)];

pub fn lane_keepout(name: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> CopperKeepout {
    keepout(name, &[CopperLayer::Front, CopperLayer::Back], rect(x0, y0, x1, y1))
}

pub struct Lanes<'a> {
    pub lanes: &'a [Lane],
    pub groups: Option<Vec<(&'a str, Vec<&'a str>)>>,
    pub max_skew: f64,
    pub amplitude: Option<i64>,
    pub uncoupled: Option<f64>,
    pub keepouts: Vec<CopperKeepout>,
}

impl<'a> Lanes<'a> {
    pub fn new(lanes: &'a [Lane]) -> Self {
        Lanes { lanes, groups: None, max_skew: 0.1, amplitude: Some(AMPLITUDE), uncoupled: None, keepouts: Vec::new() }
    }

    pub fn keepouts(mut self, keepouts: Vec<CopperKeepout>) -> Self {
        self.keepouts = keepouts;
        self
    }

    /// `_lanes_board`: one straight 0.2/0.2 mm differential pair per lane,
    /// each between its own two parts; all lanes form the group `lanes`
    /// unless `groups` names others.
    pub fn build(self) -> PhysicalBoard {
        let mut board = board("LengthMatchLanes", 40.0, 30.0);
        add_footprint(
            &mut board,
            footprint(
                "test/pair-lands",
                vec![pad("1", mm(0.0, -0.2), size(0.6, 0.2)), pad("2", mm(0.0, 0.2), size(0.6, 0.2))],
                size(1.0, 1.0),
            ),
        );
        for (index, (name, y, start, end)) in self.lanes.iter().enumerate() {
            let (source, sink) = (format!("U{}", index + 1), format!("J{}", index + 1));
            board.placements.push(place(&source, "test/pair-lands", mm(*start, *y)));
            board.placements.push(place(&sink, "test/pair-lands", mm(*end, *y)));
            board.nets.push(net(&format!("{name}_N"), &[(&source, "1"), (&sink, "1")]));
            board.nets.push(net(&format!("{name}_P"), &[(&source, "2"), (&sink, "2")]));
            for (member, partner) in [("P", "N"), ("N", "P")] {
                board.net_routing_rules.push(NetRoutingRule {
                    kind: RouteKind::Differential,
                    differential_partner: Some(format!("{name}_{partner}")),
                    priority: 100,
                    width_nm: Some(WIDTH),
                    pair_gap_nm: Some(GAP),
                    max_skew_nm: Some(PAIR_SKEW),
                    tuning_amplitude_limit_nm: self.amplitude,
                    maximum_uncoupled_length_nm: self.uncoupled.map(nm),
                    ..rule(&format!("{name}_{member}"))
                });
            }
        }
        board.rules = DesignRules {
            minimum_clearance_nm: nm(0.1),
            minimum_track_width_nm: nm(0.1),
            default_track_width_nm: WIDTH,
            ..DesignRules::default()
        };
        board.copper_keepouts = self.keepouts;
        let groups = self.groups.unwrap_or_else(|| vec![("lanes", self.lanes.iter().map(|lane| lane.0).collect())]);
        board.match_groups = groups
            .into_iter()
            .map(|(id, names)| NetMatchGroup {
                id: id.to_owned(),
                nets: names.iter().flat_map(|name| [format!("{name}_N"), format!("{name}_P")]).collect(),
                max_skew_nm: nm(self.max_skew),
            })
            .collect();
        board
    }
}

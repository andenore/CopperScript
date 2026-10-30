//! `_group_board` of `tests/test_group_tuning.py`: a bundle of three pairs
//! in one tuning group plus a longer pair, all in one length-match group.

use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind, TuningStyle};
use copper_physical::{CopperKeepout, DesignRules};

use super::lanes::{GAP, WIDTH, lane_keepout};
use super::*;

pub const NAMES: [&str; 3] = ["LANE_A", "LANE_B", "LANE_C"];
/// Pairs at a 0.9 mm pitch leave 0.3 mm between neighbouring lanes: at the
/// board's 0.1 mm clearance, too little for a pair bump's coupled corners.
pub const LANES: [&str; 6] = ["LANE_A_N", "LANE_A_P", "LANE_B_N", "LANE_B_P", "LANE_C_N", "LANE_C_P"];

/// A wall 0.25 mm above LANE_A hems the bundle in from above.
pub fn wall_above() -> CopperKeepout {
    lane_keepout("above", 6.0, 12.5, 24.0, 13.55)
}

pub struct Group {
    pub run: f64,
    pub amplitude: f64,
    pub keepouts: Vec<CopperKeepout>,
    pub group: Option<&'static str>,
    pub style: TuningStyle,
    pub longest: f64,
    pub max_skew: f64,
}

impl Default for Group {
    fn default() -> Self {
        Group {
            run: 20.0,
            amplitude: 2.0,
            keepouts: vec![wall_above()],
            group: Some("bundle"),
            style: TuningStyle::Bumps,
            longest: 24.0,
            max_skew: 0.1,
        }
    }
}

impl Group {
    /// Three straight 0.2/0.2 mm pairs from U1 to J1 at a 0.9 mm pitch, and a
    /// longer pair LANE_L. The bundle runs along y = 14.1, 15 and 15.9 mm for
    /// `run` mm; LANE_L runs `longest` mm along y = 5 mm and routes first.
    /// All eight nets form the length-match group `lanes`; the bundle's pairs
    /// the tuning `group`.
    pub fn build(self) -> PhysicalBoard {
        let mut board = board("GroupLanes", 40.0, 30.0);
        let row = (0..3)
            .flat_map(|index| {
                (0..2).map(move |member| {
                    let y = 0.9 * (index as f64 - 1.0) + 0.4 * member as f64 - 0.2;
                    pad(&(2 * index + member + 1).to_string(), mm(0.0, y), size(0.6, 0.2))
                })
            })
            .collect();
        add_footprint(&mut board, footprint("test/pair-row", row, size(1.0, 3.0)));
        add_footprint(
            &mut board,
            footprint(
                "test/pair-lands",
                vec![pad("1", mm(0.0, -0.2), size(0.6, 0.2)), pad("2", mm(0.0, 0.2), size(0.6, 0.2))],
                size(1.0, 1.0),
            ),
        );
        board.placements = vec![
            place("U1", "test/pair-row", mm(5.0, 15.0)),
            place("J1", "test/pair-row", mm(5.0 + self.run, 15.0)),
            place("U2", "test/pair-lands", mm(5.0, 5.0)),
            place("J2", "test/pair-lands", mm(5.0 + self.longest, 5.0)),
        ];
        for (index, name) in NAMES.iter().chain(&["LANE_L"]).enumerate() {
            let lone = *name == "LANE_L";
            let (source, sink) = if lone { ("U2", "J2") } else { ("U1", "J1") };
            let (n, p) = if lone {
                ("1".to_owned(), "2".to_owned())
            } else {
                ((2 * index + 1).to_string(), (2 * index + 2).to_string())
            };
            board.nets.push(net(&format!("{name}_N"), &[(source, &n), (sink, &n)]));
            board.nets.push(net(&format!("{name}_P"), &[(source, &p), (sink, &p)]));
            for (member, partner) in [("P", "N"), ("N", "P")] {
                board.net_routing_rules.push(NetRoutingRule {
                    kind: RouteKind::Differential,
                    differential_partner: Some(format!("{name}_{partner}")),
                    priority: if lone { 200 } else { 100 },
                    width_nm: Some(WIDTH),
                    pair_gap_nm: Some(GAP),
                    max_skew_nm: Some(nm(0.05)),
                    tuning_amplitude_limit_nm: Some(nm(self.amplitude)),
                    tuning_style: self.style,
                    tuning_group: if lone { None } else { self.group.map(str::to_owned) },
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
        board.match_groups = vec![NetMatchGroup {
            id: "lanes".to_owned(),
            nets: board.nets.iter().map(|net| net.name.clone()).collect(),
            max_skew_nm: nm(self.max_skew),
        }];
        board
    }
}

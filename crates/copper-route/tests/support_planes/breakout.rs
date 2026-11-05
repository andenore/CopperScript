//! `_row_board` of `tests/test_breakout_regions.py`: a fine land row joined
//! to an equal row by one pair whose breakout values apply near the lands.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_physical::footprint::PadShape;
use copper_physical::{DesignRules, Point};

use super::*;

pub const ROWS: [f64; 6] = [-1.25, -0.75, -0.25, 0.25, 0.75, 1.25];

/// A test point beside the straight channel, 0.3 mm from the P lane's edge:
/// closer than the 0.5 mm channel clearance, farther than the 0.15 mm
/// breakout clearance.
pub fn test_point() -> Point {
    mm(15.0, 9.175)
}

pub struct Row {
    pub clearance: f64,
    /// Breakout length and breakout clearance, in mm.
    pub breakout: Option<(f64, f64)>,
    pub neck: bool,
    pub obstacle: bool,
}

impl Default for Row {
    fn default() -> Self {
        Row { clearance: 0.5, breakout: Some((1.0, 0.15)), neck: false, obstacle: true }
    }
}

impl Row {
    /// The pair profile: 0.15 mm width, 0.25 mm gap and `clearance`;
    /// `breakout` adds a breakout length and clearance, `neck` a 0.12 mm
    /// breakout width and 0.2 mm breakout gap.
    pub fn rule(&self, net: &str, partner: &str) -> NetRoutingRule {
        NetRoutingRule {
            kind: RouteKind::Differential,
            differential_partner: Some(partner.to_owned()),
            priority: 100,
            width_nm: Some(nm(0.15)),
            pair_gap_nm: Some(nm(0.25)),
            clearance_nm: Some(nm(self.clearance)),
            breakout_length_nm: self.breakout.map(|(length, _)| nm(length)),
            breakout_clearance_nm: self.breakout.map(|(_, clearance)| nm(clearance)),
            breakout_width_nm: self.neck.then(|| nm(0.12)),
            breakout_gap_nm: self.neck.then(|| nm(0.2)),
            ..rule(net)
        }
    }

    /// A 0.5 mm-pitch land row (U1) joined to an equal row (J1) by one pair.
    /// Pads 3 and 4 of each row carry LANE_P/LANE_N; the other lands are
    /// unconnected neighbours 0.25 mm away.
    pub fn build(self) -> PhysicalBoard {
        let mut board = board("BreakoutRow", 30.0, 20.0);
        let lands = |x: f64| {
            ROWS.iter()
                .enumerate()
                .map(|(index, y)| pad(&(index + 1).to_string(), mm(x, *y), size(0.6, 0.25)))
                .collect()
        };
        add_footprint(&mut board, footprint("test/fine-row", lands(1.4), size(3.0, 4.0)));
        add_footprint(&mut board, footprint("test/fine-connector", lands(-1.4), size(3.0, 4.0)));
        let mut point = pad("1", Point::new(0, 0), size(0.5, 0.5));
        point.shape = PadShape::Circle;
        add_footprint(&mut board, footprint("test/test-point", vec![point], size(1.0, 1.0)));
        board.placements =
            vec![place("U1", "test/fine-row", mm(8.0, 10.0)), place("J1", "test/fine-connector", mm(22.0, 10.0))];
        if self.obstacle {
            board.placements.push(place("TP1", "test/test-point", test_point()));
        }
        board.nets = vec![
            net("LANE_P", &[("U1", "3"), ("J1", "3")]),
            net("LANE_N", &[("U1", "4"), ("J1", "4")]),
            net("AUX", &[]),
        ];
        board.rules = DesignRules {
            minimum_clearance_nm: nm(0.1),
            minimum_track_width_nm: nm(0.1),
            default_track_width_nm: nm(0.15),
            ..DesignRules::default()
        };
        board.net_routing_rules = vec![self.rule("LANE_P", "LANE_N"), self.rule("LANE_N", "LANE_P")];
        board
    }
}

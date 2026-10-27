//! Breakout regions around a net's terminal lands (`pcbir.breakout`), as
//! physical DRC reads them: region membership and region-aware spacing.
//!
//! The router-side helpers (`split`, `neck_down`, ...) belong with the router.

use indexmap::{IndexMap, IndexSet};
use num_rational::BigRational;
use num_traits::Zero;

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, RouteKind};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::{Bounds, rational, shape_distance_squared};
use copper_physical::{Point, RoundedConvexShape};

/// `BreakoutLand`: one terminal land of a breakout net.
#[derive(Clone, Debug)]
pub struct BreakoutLand {
    pub identity: String,
    pub net: String,
    pub shape: RoundedConvexShape,
    pub breakout_length_nm: i64,
    pub bounds: Bounds,
}

impl BreakoutLand {
    /// Squared spine distance of `point` when it is inside the region.
    pub fn distance_squared(&self, point: Point) -> Option<BigRational> {
        let b = self.bounds;
        if !(b.min_x <= point.x_nm && point.x_nm <= b.max_x && b.min_y <= point.y_nm && point.y_nm <= b.max_y) {
            return None;
        }
        let reach = i128::from(self.shape.radius_nm) + i128::from(self.breakout_length_nm);
        let probe = RoundedConvexShape::new(vec![point], 0).expect("point shape");
        let distance = shape_distance_squared(&probe, &self.shape);
        (distance <= rational(reach * reach)).then_some(distance)
    }
}

/// `BreakoutRegions`: terminal-land regions of every net declaring `breakout_length`.
pub struct BreakoutRegions {
    pub minimum_clearance_nm: i64,
    pub rules: IndexMap<String, NetRoutingRule>,
    pub declared: IndexSet<String>,
    pub ordinary: IndexSet<String>,
    lands: IndexMap<String, Vec<BreakoutLand>>,
}

impl BreakoutRegions {
    pub fn new(board: &PhysicalBoard) -> Self {
        let rules: IndexMap<String, NetRoutingRule> =
            board.net_routing_rules.iter().map(|r| (r.net.clone(), r.clone())).collect();
        let declared: IndexSet<String> =
            rules.iter().filter(|(_, r)| r.breakout_length_nm.is_some()).map(|(n, _)| n.clone()).collect();
        let ordinary = declared.iter().filter(|n| rules[*n].kind == RouteKind::General).cloned().collect();
        let mut lands: IndexMap<String, Vec<BreakoutLand>> = IndexMap::new();
        if !declared.is_empty() {
            let mut assigned: IndexMap<&PadReference, &str> = IndexMap::new();
            for net in board.nets.iter().filter(|n| declared.contains(&n.name)) {
                for pad in &net.pads {
                    assigned.insert(pad, net.name.as_str());
                }
            }
            let mut placements: Vec<_> = board.placements.iter().collect();
            placements.sort_by(|a, b| a.reference.cmp(&b.reference));
            for placement in placements {
                for (index, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
                    let Some(net) = assigned.get(&PadReference::new(&placement.reference, &pad.number)) else {
                        continue;
                    };
                    if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                        continue;
                    }
                    let length = rules[*net].breakout_length_nm.expect("declared breakout");
                    let shape = placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement);
                    let bounds = shape.bounds.expanded(length);
                    lands.entry((*net).to_owned()).or_default().push(BreakoutLand {
                        identity: format!("pad:{}.{}:{index}", placement.reference, pad.number),
                        net: (*net).to_owned(),
                        shape,
                        breakout_length_nm: length,
                        bounds,
                    });
                }
            }
        }
        BreakoutRegions { minimum_clearance_nm: board.rules.minimum_clearance_nm, rules, declared, ordinary, lands }
    }

    /// `__bool__`.
    pub fn any(&self) -> bool {
        !self.declared.is_empty()
    }

    /// `region`: the land whose region holds every point; nearest first, then first in order.
    pub fn region(&self, net: &str, spine: &[Point]) -> Option<String> {
        let lands = self.lands.get(net)?;
        let mut best: Option<(BigRational, &str)> = None;
        'lands: for land in lands {
            let mut farthest = BigRational::zero();
            for point in spine {
                let Some(distance) = land.distance_squared(*point) else { continue 'lands };
                if distance > farthest {
                    farthest = distance;
                }
            }
            if best.as_ref().is_none_or(|(distance, _)| farthest < *distance) {
                let zero = farthest.is_zero();
                best = Some((farthest, &land.identity));
                if zero {
                    break;
                }
            }
        }
        best.map(|(_, identity)| identity.to_owned())
    }

    /// `pair`: two nets are the members of a pair that declares breakout properties.
    pub fn pair(&self, first: &str, second: &str) -> bool {
        let (Some(a), Some(b)) = (self.rules.get(first), self.rules.get(second)) else { return false };
        (a.differential_partner.as_deref() == Some(second) || b.differential_partner.as_deref() == Some(first))
            && (self.declared.contains(first) || self.declared.contains(second))
            && a.pair_gap_nm.is_some()
            && b.pair_gap_nm.is_some()
    }

    /// `spacing_nm`: required spacing, each net's value at its own copper.
    pub fn spacing_nm(&self, first: &str, first_land: Option<&str>, second: &str, second_land: Option<&str>) -> i64 {
        let (a, b) = (self.rules.get(first), self.rules.get(second));
        if self.pair(first, second) {
            let (a, b) = (a.expect("pair rule"), b.expect("pair rule"));
            return self.minimum_clearance_nm.max(gap(a, first_land)).max(gap(b, second_land));
        }
        self.minimum_clearance_nm.max(clearance(a, first_land)).max(clearance(b, second_land))
    }
}

fn clearance(rule: Option<&NetRoutingRule>, land: Option<&str>) -> i64 {
    let Some(rule) = rule else { return 0 };
    if land.is_some()
        && let Some(value) = rule.breakout_clearance_nm
    {
        return value;
    }
    rule.clearance_nm.unwrap_or(0)
}

fn gap(rule: &NetRoutingRule, land: Option<&str>) -> i64 {
    if land.is_some()
        && let Some(value) = rule.breakout_gap_nm
    {
        return value;
    }
    rule.pair_gap_nm.unwrap_or(0)
}

/// `_clearance`: the larger of the board minimum and both nets' routing clearances.
pub fn plain_clearance(board: &PhysicalBoard, first: Option<&NetRoutingRule>, second: Option<&NetRoutingRule>) -> i64 {
    board
        .rules
        .minimum_clearance_nm
        .max(first.and_then(|r| r.clearance_nm).unwrap_or(0))
        .max(second.and_then(|r| r.clearance_nm).unwrap_or(0))
}

/// `_BreakoutSpacing`: region-aware copper spacing for physical DRC.
///
/// Relaxation records (the report's `breakout_relaxations`) are not kept;
/// they never change a finding.
pub struct BreakoutSpacing<'a> {
    board: &'a PhysicalBoard,
    pub rules: IndexMap<String, NetRoutingRule>,
    pub regions: BreakoutRegions,
}

impl<'a> BreakoutSpacing<'a> {
    pub fn new(board: &'a PhysicalBoard) -> Self {
        let regions = BreakoutRegions::new(board);
        BreakoutSpacing { board, rules: regions.rules.clone(), regions }
    }

    /// `land`: the terminal land whose breakout region holds this copper of `net`.
    pub fn land(&self, net: &str, spine: &[Point]) -> Option<String> {
        if self.regions.declared.contains(net) { self.regions.region(net, spine) } else { None }
    }

    /// `between`: required spacing with breakout regions applied, and without them.
    pub fn between(
        &self,
        first: &str,
        first_land: Option<&str>,
        second: &str,
        second_land: Option<&str>,
    ) -> (i64, i64) {
        if !self.regions.any() {
            let value = plain_clearance(self.board, self.rules.get(first), self.rules.get(second));
            return (value, value);
        }
        (
            self.regions.spacing_nm(first, first_land, second, second_land),
            self.regions.spacing_nm(first, None, second, None),
        )
    }

    /// `subject`: what a spacing finding between the nets measures.
    pub fn subject(&self, first: &str, second: &str) -> &'static str {
        if self.regions.any() && self.regions.pair(first, second) { "pair gap" } else { "copper spacing" }
    }
}

//! Breakout regions around a net's terminal lands (`pcbir.breakout`), as
//! physical DRC reads them: region membership and region-aware spacing.
//!
//! The router-side helpers (`split`, `neck_down`, ...) cut copper at region
//! boundaries for the routers.

use indexmap::{IndexMap, IndexSet};
use num_rational::BigRational;
use num_traits::Zero;

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, RouteKind, TrackSegment};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::CopperLayer;
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
    pub default_track_width_nm: i64,
    pub rules: IndexMap<String, NetRoutingRule>,
    pub declared: IndexSet<String>,
    pub ordinary: IndexSet<String>,
    pub lands: IndexMap<String, Vec<BreakoutLand>>,
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
        BreakoutRegions {
            minimum_clearance_nm: board.rules.minimum_clearance_nm,
            default_track_width_nm: board.rules.default_track_width_nm,
            rules,
            declared,
            ordinary,
            lands,
        }
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

/// `DrcBreakoutRelaxation`: a width, clearance or pair-gap check passed only
/// under breakout values. `required_nm` is the normal value the copper does
/// not meet and `relaxed_nm` the breakout value it does meet; `regions`
/// names, for each net whose breakout value applied, the terminal land whose
/// region holds its copper and the net's breakout length.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BreakoutRelaxation {
    /// "track_width", "clearance" or "pair_gap".
    pub check: String,
    pub objects: Vec<String>,
    pub nets: Vec<String>,
    pub layers: Vec<String>,
    pub required_nm: i64,
    pub relaxed_nm: i64,
    pub measured_nm: Option<i64>,
    pub regions: Vec<(String, String, i64)>,
}

/// `_BreakoutSpacing`: region-aware copper spacing for physical DRC, with
/// the relaxation records of the checks that passed only under breakout
/// values (they never change a finding).
pub struct BreakoutSpacing<'a> {
    board: &'a PhysicalBoard,
    pub rules: IndexMap<String, NetRoutingRule>,
    pub regions: BreakoutRegions,
    pub relaxations: std::cell::RefCell<Vec<BreakoutRelaxation>>,
}

impl<'a> BreakoutSpacing<'a> {
    pub fn new(board: &'a PhysicalBoard) -> Self {
        let regions = BreakoutRegions::new(board);
        BreakoutSpacing { board, rules: regions.rules.clone(), regions, relaxations: Default::default() }
    }

    /// `check`: the breakout value a spacing between the nets relaxes.
    pub fn check(&self, first: &str, second: &str) -> &'static str {
        if self.regions.any() && self.regions.pair(first, second) { "pair_gap" } else { "clearance" }
    }

    /// `note`: record a relaxation when copper meets `relaxed` but not
    /// `required`. `distance_squared` is between the spines and
    /// `radii_twice` the sum of both copper widths; `copper` pairs each net
    /// with its region.
    #[allow(clippy::too_many_arguments)]
    pub fn note(
        &self,
        distance_squared: &num_rational::BigRational,
        radii_twice: i64,
        required: i64,
        relaxed: i64,
        objects: [String; 2],
        copper: [(&str, Option<&str>); 2],
        layers: Vec<String>,
    ) {
        use num_bigint::BigInt;
        let needed = BigInt::from(radii_twice + 2 * required);
        let four = num_rational::BigRational::from_integer(BigInt::from(4));
        let scaled = &four * distance_squared;
        if scaled >= num_rational::BigRational::from_integer(&needed * &needed) {
            return;
        }
        let floor = crate::fraction_sqrt_floor(&scaled);
        let measured = (floor - radii_twice).div_euclid(2);
        let check = self.check(copper[0].0, copper[1].0);
        self.record(check, objects.to_vec(), &copper, layers, required, relaxed, Some(measured));
    }

    /// `record`: one relaxation with the regions that justified it.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        check: &str,
        objects: Vec<String>,
        copper: &[(&str, Option<&str>)],
        layers: Vec<String>,
        required: i64,
        relaxed: i64,
        measured: Option<i64>,
    ) {
        let mut regions: Vec<(String, String, i64)> = copper
            .iter()
            .filter_map(|(net, land)| {
                let land = (*land)?;
                self.regions
                    .relaxes(net, Some(land), check)
                    .then(|| ((*net).to_owned(), land.to_owned(), self.rules[*net].breakout_length_nm.unwrap_or(0)))
            })
            .collect();
        regions.sort();
        regions.dedup();
        let mut nets: Vec<String> = copper.iter().map(|(net, _)| (*net).to_owned()).collect();
        nets.sort();
        nets.dedup();
        self.relaxations.borrow_mut().push(BreakoutRelaxation {
            check: check.to_owned(),
            objects,
            nets,
            layers,
            required_nm: required,
            relaxed_nm: relaxed,
            measured_nm: measured,
            regions,
        });
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

impl BreakoutLand {
    /// `contains`: whether `point` lies in this land's region.
    pub fn contains(&self, point: Point) -> bool {
        self.distance_squared(point).is_some()
    }
}

impl BreakoutRegions {
    /// `inside_any`: whether `point` is inside the region of any net's terminal land.
    pub fn inside_any(&self, point: Point) -> bool {
        self.lands.values().flatten().any(|land| land.contains(point))
    }

    /// `split`: cut one track centreline at the region boundaries of `net`.
    ///
    /// Returns `(start, end, land)` pieces in order. Only octilinear segments
    /// are cut; a segment inside one region, or with no region at either end,
    /// is one piece, as is a segment of any other direction.
    pub fn split(&self, net: &str, start: Point, end: Point) -> Vec<(Point, Point, Option<String>)> {
        let whole = vec![(start, end, self.region(net, &[start, end]))];
        let Some(lands) = self.lands.get(net).filter(|lands| !lands.is_empty()) else { return whole };
        let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
        if start == end || whole[0].2.is_some() || (dx != 0 && dy != 0 && dx.abs() != dy.abs()) {
            return whole;
        }
        let steps = dx.abs().max(dy.abs());
        let (ux, uy) = (dx.signum(), dy.signum());
        let at = |step: i64| Point::new(start.x_nm + ux * step, start.y_nm + uy * step);
        // The farthest step toward `toward` that stays in a region holding
        // `origin`; the steps between are inside because regions are convex.
        let extent = |origin: i64, toward: i64| -> i64 {
            let mut reached = origin;
            for land in lands {
                if !land.contains(at(origin)) {
                    continue;
                }
                let (mut inside, mut outside) = (origin, toward);
                if land.contains(at(outside)) {
                    inside = outside;
                } else {
                    while (outside - inside).abs() > 1 {
                        let middle = (inside + outside).div_euclid(2);
                        if land.contains(at(middle)) {
                            inside = middle;
                        } else {
                            outside = middle;
                        }
                    }
                }
                reached = if toward > origin { reached.max(inside) } else { reached.min(inside) };
            }
            reached
        };
        let (head, tail) = (extent(0, steps), extent(steps, 0));
        let candidates: Vec<i64> = if head < tail { vec![head, tail] } else { vec![head] };
        let cuts: Vec<i64> = candidates.into_iter().filter(|step| 0 < *step && *step < steps).collect();
        if cuts.is_empty() {
            return whole;
        }
        let mut marks = vec![0];
        marks.extend(cuts);
        marks.push(steps);
        marks
            .windows(2)
            .filter(|pair| pair[0] != pair[1])
            .map(|pair| (at(pair[0]), at(pair[1]), self.region(net, &[at(pair[0]), at(pair[1])])))
            .collect()
    }

    /// `split_tracks`: cut tracks at region boundaries; pieces inside get the breakout width.
    pub fn split_tracks(&self, tracks: &[TrackSegment], width_nm: Option<i64>) -> Vec<TrackSegment> {
        let mut result = Vec::new();
        for track in tracks {
            if !self.declared.contains(&track.net) {
                result.push(track.clone());
                continue;
            }
            let outside = width_nm.unwrap_or(track.width_nm);
            for (start, end, land) in self.split(&track.net, track.start, track.end) {
                result.push(TrackSegment {
                    net: track.net.clone(),
                    start,
                    end,
                    width_nm: self.track_width_nm(&track.net, land.as_deref(), outside),
                    layer: track.layer,
                });
            }
        }
        result
    }

    /// `track_width_nm`: the breakout width inside a region, else `width_nm`.
    pub fn track_width_nm(&self, net: &str, land: Option<&str>, width_nm: i64) -> i64 {
        match (land, self.rules.get(net).and_then(|rule| rule.breakout_width_nm)) {
            (Some(_), Some(width)) => width,
            _ => width_nm,
        }
    }

    /// `width_nm`: the profile width of `net` outside its regions.
    pub fn width_nm(&self, net: &str) -> i64 {
        match self.rules.get(net).and_then(|rule| rule.width_nm) {
            Some(width) if width != 0 => width,
            _ => self.default_track_width_nm,
        }
    }

    /// `ordinary_pieces`: the minimum-width fallback copper for one ordinary centreline.
    pub fn ordinary_pieces(
        &self,
        net: &str,
        start: Point,
        end: Point,
        width_nm: i64,
        layer: CopperLayer,
    ) -> Vec<TrackSegment> {
        let track = TrackSegment { net: net.to_owned(), start, end, width_nm, layer };
        if self.ordinary.contains(net) { self.split_tracks(&[track], Some(width_nm)) } else { vec![track] }
    }

    /// `neck_down`: re-cut ordinary copper after a geometry change such as pruning.
    pub fn neck_down(&self, tracks: &[TrackSegment]) -> Vec<TrackSegment> {
        let mut result = Vec::new();
        for track in tracks {
            if self.ordinary.contains(&track.net) {
                let profile = self.width_nm(&track.net);
                for (start, end, land) in self.split(&track.net, track.start, track.end) {
                    let width = track.width_nm.max(self.track_width_nm(&track.net, land.as_deref(), profile));
                    result.push(TrackSegment {
                        net: track.net.clone(),
                        start,
                        end,
                        width_nm: width,
                        layer: track.layer,
                    });
                }
            } else {
                result.push(track.clone());
            }
        }
        result
    }

    /// `required_width_nm`: the least width of verified ordinary copper.
    pub fn required_width_nm(&self, track: &TrackSegment, width_nm: i64) -> i64 {
        if !self.ordinary.contains(&track.net) {
            return width_nm;
        }
        self.track_width_nm(&track.net, self.region(&track.net, &[track.start, track.end]).as_deref(), width_nm)
    }

    /// `relaxes`: whether `net` declares the breakout value of `check` and its copper is inside.
    pub fn relaxes(&self, net: &str, land: Option<&str>, check: &str) -> bool {
        let (Some(_), Some(rule)) = (land, self.rules.get(net)) else { return false };
        match check {
            "clearance" => rule.breakout_clearance_nm.is_some(),
            "pair_gap" => rule.breakout_gap_nm.is_some(),
            "track_width" => rule.breakout_width_nm.is_some(),
            other => panic!("unknown breakout check {other}"),
        }
    }
}

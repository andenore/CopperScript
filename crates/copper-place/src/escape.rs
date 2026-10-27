//! Board-bound, directional escape-channel estimates (`pcbir.placement_escape`).
//!
//! A conservative soft objective, not a pin-access or DRC certificate. The
//! caches keep each reference's and pair's most recent pose only; every
//! result equals a fresh computation.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, Placement, RelativePlacementKind};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, Bounds, CopperLayer, Point};
use copper_units::Decimal;

use crate::scene::{Poses, Scene};

/// `EscapeSide`, in its enum order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EscapeSide {
    North,
    East,
    South,
    West,
}

impl EscapeSide {
    pub const ALL: [EscapeSide; 4] = [EscapeSide::North, EscapeSide::East, EscapeSide::South, EscapeSide::West];

    pub fn value(self) -> &'static str {
        match self {
            EscapeSide::North => "north",
            EscapeSide::East => "east",
            EscapeSide::South => "south",
            EscapeSide::West => "west",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// `EscapeDemand`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EscapeDemand {
    pub pad_count: i64,
    pub via_pad_count: i64,
    pub bank_rows: i64,
    pub depth_nm: i64,
    pub width_nm: i64,
    pub clearance_nm: i64,
}

/// `PackageEscapeProfile`: the pose-local envelope and per-side demand.
#[derive(Clone, Debug, PartialEq)]
pub struct EscapeProfile {
    pub bounds: Bounds,
    pub sides: [EscapeDemand; 4],
}

/// `EscapeChannel`: a facing pair with too little escape room.
#[derive(Clone, Debug, PartialEq)]
pub struct EscapeChannel {
    pub left: String,
    pub right: String,
    pub axis: &'static str,
    pub left_side: EscapeSide,
    pub right_side: EscapeSide,
    pub gap_nm: i64,
    pub required_gap_nm: i64,
    pub left_depth_nm: i64,
    pub right_depth_nm: i64,
    pub left_rows: i64,
    pub right_rows: i64,
    pub pad_count: i64,
}

impl EscapeChannel {
    pub fn deficit_nm(&self) -> i64 {
        (self.required_gap_nm - self.gap_nm).max(0)
    }
}

/// A transitive rigid/proximity unit: a sorted set of references.
pub type Unit = Rc<BTreeSet<String>>;

/// `placement_units`: rigid clusters and maximum-distance groups, closed transitively.
pub fn placement_units(
    board: &PhysicalBoard,
    rules: &[&copper_pcb::board::RelativePlacementRule],
) -> IndexMap<String, Unit> {
    let references: IndexSet<&str> = board.placements.iter().map(|p| p.reference.as_str()).collect();
    let mut parent: Vec<usize> = (0..references.len()).collect();
    fn find(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let mut join = |members: Vec<&str>| {
        let indexes: Vec<usize> = members.iter().filter_map(|m| references.get_index_of(m)).collect();
        for pair in indexes.windows(2) {
            let (a, b) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            if a != b {
                parent[a] = b;
            }
        }
    };
    for cluster in &board.rigid_clusters {
        join(cluster.members.iter().map(|m| m.reference.as_str()).collect());
    }
    for rule in rules.iter().filter(|r| r.kind == RelativePlacementKind::MaxDistance) {
        join(rule.targets.iter().map(|t| t.reference.as_str()).collect());
    }
    let mut groups: IndexMap<usize, BTreeSet<String>> = IndexMap::new();
    for (index, reference) in references.iter().enumerate() {
        groups.entry(find(&mut parent, index)).or_default().insert((*reference).to_owned());
    }
    let shared: IndexMap<usize, Unit> = groups.into_iter().map(|(root, set)| (root, Rc::new(set))).collect();
    references
        .iter()
        .enumerate()
        .map(|(index, reference)| ((*reference).to_owned(), shared[&find(&mut parent, index)].clone()))
        .collect()
}

type PoseKey = (String, BoardSide, Decimal, Point);

struct Positioned {
    key: PoseKey,
    profile: Rc<EscapeProfile>,
    bounds: Bounds,
}

#[derive(Default)]
struct Caches {
    profiles: IndexMap<(String, String, BoardSide, Decimal), Rc<EscapeProfile>>,
    positioned: IndexMap<String, Rc<Positioned>>,
    pairs: IndexMap<(String, String, bool), (PoseKey, PoseKey, Rc<Vec<EscapeChannel>>)>,
    deficit_poses: Option<Poses>,
    deficit_by_pair: IndexMap<(String, String), i64>,
    deficit_total: i64,
}

/// `EscapeSpacingModel`: reusable geometry/demand model for one source board.
pub struct EscapeSpacingModel<'a> {
    scene: &'a Scene<'a>,
    pub margin_nm: i64,
    pub transit_lanes: i64,
    pub units: IndexMap<String, Unit>,
    rules: IndexMap<&'a str, &'a NetRoutingRule>,
    net_by_pad: IndexMap<(String, String), String>,
    layers: IndexMap<String, Vec<CopperLayer>>,
    pad_counts: IndexMap<String, usize>,
    rigid_pairs: IndexSet<(String, String)>,
    caches: RefCell<Caches>,
}

fn sorted_pair(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.to_owned(), b.to_owned()) } else { (b.to_owned(), a.to_owned()) }
}

impl<'a> EscapeSpacingModel<'a> {
    pub fn new(scene: &'a Scene<'a>, margin_nm: i64, transit_lanes: i64) -> copper_pcb::Result<Self> {
        if margin_nm < 0 || transit_lanes < 0 {
            return Err(copper_pcb::Error::Value("escape margin and transit lanes must be non-negative".to_owned()));
        }
        let board = scene.board;
        let rules: IndexMap<&str, &NetRoutingRule> =
            board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
        let mut net_by_pad = IndexMap::new();
        for net in &board.nets {
            if net.pads.len() >= 2 || scene.plane_nets.contains(&net.name) {
                for pad in &net.pads {
                    net_by_pad.insert((pad.component.clone(), pad.pad.clone()), net.name.clone());
                }
            }
        }
        let layers = board
            .nets
            .iter()
            .map(|n| {
                (n.name.clone(), copper_pcb::zones::routing_layers(board, &n.name, rules.get(n.name.as_str()).copied()))
            })
            .collect();
        let pad_counts = board
            .placements
            .iter()
            .map(|p| {
                let numbers: IndexSet<&str> = board.footprints[&p.footprint]
                    .pads
                    .iter()
                    .filter(|l| {
                        !l.number.is_empty() && !matches!(l.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
                    })
                    .map(|l| l.number.as_str())
                    .collect();
                (p.reference.clone(), numbers.len())
            })
            .collect();
        let mut rigid_pairs = IndexSet::new();
        for cluster in &board.rigid_clusters {
            for a in &cluster.members {
                for b in &cluster.members {
                    if a.reference != b.reference {
                        rigid_pairs.insert(sorted_pair(&a.reference, &b.reference));
                    }
                }
            }
        }
        Ok(EscapeSpacingModel {
            scene,
            margin_nm,
            transit_lanes,
            units: placement_units(board, &scene.relative),
            rules,
            net_by_pad,
            layers,
            pad_counts,
            rigid_pairs,
            caches: RefCell::new(Caches::default()),
        })
    }

    /// `deficit_nm`: the exact channel deficit, updating only pairs a changed pose touches.
    pub fn deficit_nm(&self, placements: &Poses) -> i64 {
        let previous = self.caches.borrow_mut().deficit_poses.take();
        let same_keys = previous
            .as_ref()
            .is_some_and(|p| p.len() == placements.len() && placements.keys().all(|k| p.contains_key(k)));
        if !same_keys {
            let mut by_pair: IndexMap<(String, String), i64> = IndexMap::new();
            for channel in self.channels(placements, false) {
                *by_pair.entry((channel.left.clone(), channel.right.clone())).or_insert(0) += channel.deficit_nm();
            }
            let mut caches = self.caches.borrow_mut();
            caches.deficit_total = by_pair.values().sum();
            caches.deficit_by_pair = by_pair;
        } else {
            let previous = previous.as_ref().expect("previous poses");
            let changed: Vec<&String> =
                placements.iter().filter(|(r, pose)| previous[*r] != **pose).map(|(r, _)| r).collect();
            let mut touched: BTreeSet<(String, String)> = BTreeSet::new();
            for left in &changed {
                for right in placements.keys() {
                    if *left != right {
                        touched.insert(sorted_pair(left, right));
                    }
                }
            }
            for (left, right) in touched {
                let mut pair = Poses::new();
                pair.insert(left.clone(), placements[&left].clone());
                pair.insert(right.clone(), placements[&right].clone());
                let value: i64 = self.channels(&pair, false).iter().map(EscapeChannel::deficit_nm).sum();
                let mut caches = self.caches.borrow_mut();
                let key = (left, right);
                let old = caches.deficit_by_pair.get(&key).copied().unwrap_or(0);
                caches.deficit_total += value - old;
                if value != 0 {
                    caches.deficit_by_pair.insert(key, value);
                } else {
                    caches.deficit_by_pair.shift_remove(&key);
                }
            }
        }
        let mut caches = self.caches.borrow_mut();
        caches.deficit_poses = Some(placements.clone());
        caches.deficit_total
    }

    /// `profile`: the pose-local land envelope and each side's escape demand.
    pub fn profile(&self, placement: &Placement) -> Rc<EscapeProfile> {
        let key = (
            placement.reference.clone(),
            placement.footprint.clone(),
            placement.side,
            placement.rotation_degrees.clone(),
        );
        if let Some(profile) = self.caches.borrow().profiles.get(&key) {
            return profile.clone();
        }
        let board = self.scene.board;
        let rules = &board.rules;
        let local = Placement { position: Point::new(0, 0), ..placement.clone() };
        let footprint = &board.footprints[&placement.footprint];
        let lands: Vec<(&copper_physical::footprint::FootprintPad, Bounds)> = footprint
            .pads
            .iter()
            .filter(|pad| {
                !pad.number.is_empty() && !matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole)
            })
            .map(|pad| (pad, placed_pad_shape(transformed_local_point(&local, pad.position), pad, &local).bounds))
            .collect();
        let outline = self.scene.polygon(&local);
        let mut extent = Bounds {
            min_x: outline.iter().map(|p| p.x_nm).min().expect("outline"),
            min_y: outline.iter().map(|p| p.y_nm).min().expect("outline"),
            max_x: outline.iter().map(|p| p.x_nm).max().expect("outline"),
            max_y: outline.iter().map(|p| p.y_nm).max().expect("outline"),
        };
        for (_, b) in &lands {
            extent.min_x = extent.min_x.min(b.min_x);
            extent.min_y = extent.min_y.min(b.min_y);
            extent.max_x = extent.max_x.max(b.max_x);
            extent.max_y = extent.max_y.max(b.max_y);
        }
        let mut active: [Vec<(Bounds, bool, i64, i64)>; 4] = Default::default();
        let surface = if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
        for (pad, bounds) in &lands {
            let Some(net) = self.net_by_pad.get(&(placement.reference.clone(), pad.number.clone())) else { continue };
            if pad.kind != PadKind::Smd {
                continue;
            }
            // Two-terminal passives block a neighbour's access but demand no via banks.
            if self.pad_counts[&placement.reference] <= 2 {
                continue;
            }
            let distances = [
                bounds.min_y - extent.min_y,
                extent.max_x - bounds.max_x,
                extent.max_y - bounds.max_y,
                bounds.min_x - extent.min_x,
            ];
            let side = EscapeSide::ALL
                .iter()
                .copied()
                .min_by_key(|s| (distances[s.index()], active[s.index()].len(), s.value()))
                .expect("four sides");
            let rule = self.rules.get(net.as_str()).copied();
            let width = rules
                .minimum_track_width_nm
                .max(rule.and_then(|r| r.width_nm).filter(|w| *w != 0).unwrap_or(rules.default_track_width_nm));
            let clearance =
                rules.minimum_clearance_nm.max(rule.and_then(|r| r.clearance_nm).filter(|c| *c != 0).unwrap_or(0));
            let allowed = &self.layers[net];
            let via = rule.is_none_or(|r| r.max_vias != Some(0)) && allowed.iter().any(|layer| *layer != surface);
            active[side.index()].push((*bounds, via, width, clearance));
        }
        let mut sides = [EscapeDemand::default(); 4];
        for side in EscapeSide::ALL {
            let pads = &active[side.index()];
            if pads.is_empty() {
                continue;
            }
            let width = pads.iter().map(|p| p.2).max().expect("pads");
            let clearance = pads.iter().map(|p| p.3).max().expect("pads");
            let via_pads: Vec<Bounds> = pads.iter().filter(|p| p.1).map(|p| p.0).collect();
            let mut rows = 0;
            let depth = if !via_pads.is_empty() {
                let horizontal = matches!(side, EscapeSide::North | EscapeSide::South);
                let span = if horizontal {
                    via_pads.iter().map(|b| b.max_x).max().expect("via pads")
                        - via_pads.iter().map(|b| b.min_x).min().expect("via pads")
                } else {
                    via_pads.iter().map(|b| b.max_y).max().expect("via pads")
                        - via_pads.iter().map(|b| b.min_y).min().expect("via pads")
                };
                let via_pitch = (rules.default_via_size_nm + clearance)
                    .max(rules.default_via_drill_nm + rules.minimum_hole_clearance_nm);
                let facade = if horizontal { extent.max_x - extent.min_x } else { extent.max_y - extent.min_y };
                // Bounded lateral staggering beyond the pad window, never beyond the package side.
                let span = facade.min(span + 2 * via_pitch);
                let slots = 1i64.max(1 + 0i64.max(span - rules.default_via_size_nm).div_euclid(via_pitch));
                rows = (via_pads.len() as i64 + slots - 1).div_euclid(slots);
                rules.default_via_size_nm + (rows - 1) * via_pitch
            } else {
                width // Surface-only signals still need a launch lane.
            };
            sides[side.index()] = EscapeDemand {
                pad_count: pads.len() as i64,
                via_pad_count: via_pads.len() as i64,
                bank_rows: rows,
                depth_nm: depth + clearance + self.margin_nm,
                width_nm: width,
                clearance_nm: clearance,
            };
        }
        let profile = Rc::new(EscapeProfile { bounds: extent, sides });
        self.caches.borrow_mut().profiles.insert(key, profile.clone());
        profile
    }

    fn positioned(&self, placement: &Placement) -> Rc<Positioned> {
        let key = (placement.footprint.clone(), placement.side, placement.rotation_degrees.clone(), placement.position);
        if let Some(cached) = self.caches.borrow().positioned.get(&placement.reference)
            && cached.key == key
        {
            return cached.clone();
        }
        let profile = self.profile(placement);
        let (b, p) = (profile.bounds, placement.position);
        let bounds = Bounds {
            min_x: b.min_x + p.x_nm,
            min_y: b.min_y + p.y_nm,
            max_x: b.max_x + p.x_nm,
            max_y: b.max_y + p.y_nm,
        };
        let positioned = Rc::new(Positioned { key, profile, bounds });
        self.caches.borrow_mut().positioned.insert(placement.reference.clone(), positioned.clone());
        positioned
    }

    /// `channels`: every facing pair with an escape deficit (or every facing pair).
    pub fn channels(&self, placements: &Poses, include_clear: bool) -> Vec<EscapeChannel> {
        let mut references: Vec<&String> = placements.keys().collect();
        references.sort();
        let positioned: Vec<(&Placement, Rc<Positioned>)> =
            references.iter().map(|r| (&placements[*r], self.positioned(&placements[*r]))).collect();
        let mut result = Vec::new();
        for (index, (left, lp)) in positioned.iter().enumerate() {
            for (right, rp) in &positioned[index + 1..] {
                let pair_key = (left.reference.clone(), right.reference.clone(), include_clear);
                if let Some((lk, rk, cached)) = self.caches.borrow().pairs.get(&pair_key)
                    && *lk == lp.key
                    && *rk == rp.key
                {
                    result.extend(cached.iter().cloned());
                    continue;
                }
                let pair = self.pair_channels(left, right, lp, rp, include_clear);
                result.extend(pair.iter().cloned());
                self.caches.borrow_mut().pairs.insert(pair_key, (lp.key.clone(), rp.key.clone(), Rc::new(pair)));
            }
        }
        result
    }

    fn pair_channels(
        &self,
        left: &Placement,
        right: &Placement,
        lp: &Positioned,
        rp: &Positioned,
        include_clear: bool,
    ) -> Vec<EscapeChannel> {
        let mut pair = Vec::new();
        if left.side != right.side || self.rigid_pairs.contains(&sorted_pair(&left.reference, &right.reference)) {
            return pair;
        }
        if self.units[&left.reference].contains(&right.reference)
            && self.pad_counts[&left.reference].min(self.pad_counts[&right.reference]) <= 2
        {
            return pair; // A close two-terminal companion travels with its owner.
        }
        let (lb, rb) = (lp.bounds, rp.bounds);
        if lb.max_y.min(rb.max_y) > lb.min_y.max(rb.min_y) {
            let east = right.position.x_nm >= left.position.x_nm;
            let (ls, rs) =
                if east { (EscapeSide::East, EscapeSide::West) } else { (EscapeSide::West, EscapeSide::East) };
            let gap = if east { rb.min_x - lb.max_x } else { lb.min_x - rb.max_x };
            pair.extend(self.channel(left, right, lp, rp, "x", ls, rs, gap, include_clear));
        }
        if lb.max_x.min(rb.max_x) > lb.min_x.max(rb.min_x) {
            let south = right.position.y_nm >= left.position.y_nm;
            let (ls, rs) =
                if south { (EscapeSide::South, EscapeSide::North) } else { (EscapeSide::North, EscapeSide::South) };
            let gap = if south { rb.min_y - lb.max_y } else { lb.min_y - rb.max_y };
            pair.extend(self.channel(left, right, lp, rp, "y", ls, rs, gap, include_clear));
        }
        pair
    }

    #[allow(clippy::too_many_arguments)]
    fn channel(
        &self,
        left: &Placement,
        right: &Placement,
        lp: &Positioned,
        rp: &Positioned,
        axis: &'static str,
        ls: EscapeSide,
        rs: EscapeSide,
        gap: i64,
        include_clear: bool,
    ) -> Option<EscapeChannel> {
        let rules = &self.scene.board.rules;
        let (a, b) = (lp.profile.sides[ls.index()], rp.profile.sides[rs.index()]);
        if a.pad_count == 0 && b.pad_count == 0 {
            return None;
        }
        let width = a.width_nm.max(b.width_nm).max(rules.default_track_width_nm);
        let clearance = a.clearance_nm.max(b.clearance_nm).max(rules.minimum_clearance_nm);
        let transit = if self.transit_lanes != 0 {
            self.transit_lanes * width + (self.transit_lanes + 1) * clearance
        } else if a.depth_nm != 0 && b.depth_nm != 0 {
            clearance
        } else {
            0
        };
        let required = a.depth_nm + b.depth_nm + transit;
        if !include_clear && gap >= required {
            return None;
        }
        Some(EscapeChannel {
            left: left.reference.clone(),
            right: right.reference.clone(),
            axis,
            left_side: ls,
            right_side: rs,
            gap_nm: gap,
            required_gap_nm: required,
            left_depth_nm: a.depth_nm,
            right_depth_nm: b.depth_nm,
            left_rows: a.bank_rows,
            right_rows: b.bank_rows,
            pad_count: a.pad_count + b.pad_count,
        })
    }
}

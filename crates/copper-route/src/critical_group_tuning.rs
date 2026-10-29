//! Group serpentines: adjacent units of a `length_match` group tuned together
//! (`pcbir.critical_group_tuning`, D-PHY plan R14).
//!
//! Units (pairs or single nets) of one `length_match` group whose routing
//! rules name the same `tuning_group` are bent together, like the lanes of a
//! DDR byte lane: every lane follows the same bumps or serpentine, keeps its
//! spacing to the next lane through every bend, and the group uses the free
//! room beside it. A *run* is a stretch along one axis and layer where every
//! net of the group has exactly one straight line and each unit's lanes are
//! adjacent; every lane rises the same height off its own line, with the
//! N-lane corner offsets of `lane_corners`, so all lanes gain the same length.
//! The critical router owns the targets, the order and the atomic validation.

use std::cell::RefCell;
use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PhysicalBoard, TrackSegment, TuningStyle};
use copper_physical::{Bounds, CopperLayer, Point, RoundedConvexShape};

use crate::critical_tuning::{
    CHAMFER_STEPS, Features, TunerOptions, UnitTuning, area_clear, axial, chamfer_offset, diagonal, double,
    feature_legs, horizontal, levelled, near, normal, parts, remoteness, rounded, spliced, straight_lines, tallest,
    terminal_ends, union,
};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::{Error, Result};

/// `_LANE_TOLERANCE_NM`: pair members count as adjacent lanes at their spacing within this tolerance.
const LANE_TOLERANCE_NM: i64 = 4;

/// `_CORNER_SLACK_NM`: nanometres by which `lane_corners` may lower a lane's corner offsets.
pub const CORNER_SLACK_NM: i64 = 8;

/// `_GRID_SHIFTS`: bound on the shifted bump grids of one run (`_bump_tops`).
const GRID_SHIFTS: i64 = 8;

/// `_Lane`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Lane {
    /// The lane's straight line in `GroupTuner.segments`.
    lead: usize,
    net: String,
    /// Normal coordinate of the line.
    offset: i64,
}

/// `_Run`: a stretch where every lane of the group runs straight, side by side.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GroupRun {
    layer: CopperLayer,
    horizontal: bool,
    /// By ascending offset.
    lanes: Vec<Lane>,
    low: i64,
    high: i64,
    /// Widest lane.
    width: i64,
}

impl GroupRun {
    /// `span`.
    fn span(&self) -> i64 {
        self.lanes[self.lanes.len() - 1].offset - self.lanes[0].offset
    }

    /// `gaps`.
    fn gaps(&self) -> Vec<i64> {
        self.lanes.windows(2).map(|pair| pair[1].offset - pair[0].offset).collect()
    }

    /// `offset_sum`: D, the corner offset of the lane farthest from the inside of a turn.
    fn offset_sum(&self) -> i64 {
        self.gaps().into_iter().map(chamfer_offset).sum()
    }
}

/// `_Top`: one bump, or one serpentine top, of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Top {
    run: usize,
    /// +1 or -1 along the normal.
    side: i64,
    /// The bulge lane's interval along the axis.
    start: i64,
    end: i64,
    /// The inner lane's interval width.
    inner: i64,
}

/// `_Feature`: a bump (one top) or a serpentine (consecutive tops on alternating sides).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Feature {
    tops: Vec<Top>,
    rooms: Vec<i64>,
}

impl Feature {
    fn single(top: Top, room: i64) -> Self {
        Feature { tops: vec![top], rooms: vec![room] }
    }
}

/// `_diagonal_cost`: length a 45-degree chamfer with leg `leg` saves, 2k - round(k sqrt 2).
fn diagonal_cost(leg: i64) -> i64 {
    2 * leg - diagonal(leg)
}

/// `lane_corners`: chamfer legs `(lower, upper)` of every lane at each leg, or None.
///
/// `gaps` are the distances between adjacent lanes (lane 1 first). Lane N is
/// inside the turn at the lower end of a leg and takes `chamfer`; lane k takes
/// `chamfer` + U'_k, with U'_k - U'_{k+1} between 0 and d(gap); likewise V'
/// from lane 1 at the upper end. Offsets at most `slack` below the natural
/// ones are chosen, least in total and then by lane, so that every lane's
/// rounded loss is equal. None when no such offsets exist.
pub fn lane_corners(gaps: &[i64], chamfer: i64, slack: i64) -> Option<Vec<(i64, i64)>> {
    let steps: Vec<i64> = gaps.iter().copied().map(chamfer_offset).collect();
    let count = steps.len() + 1;
    let lower: Vec<i64> = (0..count).map(|lane| steps[lane..].iter().sum()).collect();
    let upper: Vec<i64> = (0..count).map(|lane| steps[..lane].iter().sum()).collect();
    let loss = |lane: usize, low: i64, up: i64| {
        diagonal_cost(chamfer + lower[lane] - low) + diagonal_cost(chamfer + upper[lane] - up)
    };
    type Entry = (i64, Vec<(i64, i64)>);
    let mut best: Option<Entry> = None;
    for first in 0..=slack.min(lower[0]) {
        let target = loss(0, first, 0);
        // Lowered amounts (lower, upper) of the lane so far -> least total and path.
        let mut states: BTreeMap<(i64, i64), Entry> = BTreeMap::new();
        states.insert((first, 0), (first, vec![(first, 0)]));
        for lane in 1..count {
            let step = steps[lane - 1];
            let mut following: BTreeMap<(i64, i64), Entry> = BTreeMap::new();
            for (&(low, up), (total, path)) in &states {
                for next_low in 0.max(low - step)..=low {
                    for next_up in up..=slack.min(up + step) {
                        if loss(lane, next_low, next_up) != target {
                            continue;
                        }
                        let mut next_path = path.clone();
                        next_path.push((next_low, next_up));
                        let entry = (total + next_low + next_up, next_path);
                        match following.get(&(next_low, next_up)) {
                            Some(existing) if *existing <= entry => {}
                            _ => {
                                following.insert((next_low, next_up), entry);
                            }
                        }
                    }
                }
            }
            states = following;
        }
        for (&(low, _), entry) in &states {
            if low == 0 && best.as_ref().is_none_or(|best| entry < best) {
                best = Some(entry.clone());
            }
        }
    }
    let (_, path) = best?;
    Some(
        path.iter()
            .enumerate()
            .map(|(lane, &(low, up))| (chamfer + lower[lane] - low, chamfer + upper[lane] - up))
            .collect(),
    )
}

/// `group_path`: centre-line corners `(axial, rise)` of one lane of a group feature, in axial order.
///
/// `legs` are the axial positions of each leg's first lane: lane N (the last
/// of `offsets`) on a leg rising toward +, lane 1 on a falling one. `rises`
/// are the levels before, between and after the legs. `lower`/`upper` are the
/// lane's chamfer legs at the lower and upper end of every leg.
pub fn group_path(
    legs: &[i64],
    rises: &[i64],
    offsets: &[i64],
    lane: usize,
    lower: i64,
    upper: i64,
) -> Vec<(i64, i64)> {
    let mut corners = Vec::new();
    for (leg, &position) in legs.iter().enumerate() {
        let (before, after) = (rises[leg], rises[leg + 1]);
        if after > before {
            let along = position + offsets[offsets.len() - 1] - offsets[lane];
            corners.extend([
                (along - lower, before),
                (along, before + lower),
                (along, after - upper),
                (along + upper, after),
            ]);
        } else {
            let along = position + offsets[lane] - offsets[0];
            corners.extend([
                (along - upper, before),
                (along, before - upper),
                (along, after + lower),
                (along + lower, after),
            ]);
        }
    }
    corners
}

/// `GroupTuner`: bumps and serpentines that bend every lane of a tuning group together.
///
/// `tracks` is the copper of every unit of the group and `obstacles` indexes
/// all other copper. `units` are the group's units (each one net or a pair)
/// and `spacings` each unit's lane spacing (0 for a single net). Rooms are
/// measured once, so several length targets cost one survey.
pub struct GroupTuner {
    tracks: Vec<TrackSegment>,
    nets: Vec<String>,
    limit: i64,
    clearance_nm: i64,
    bump_limit: usize,
    leg_limit: usize,
    segments: BTreeMap<usize, TrackSegment>,
    lead_of: IndexMap<usize, usize>,
    runs: Vec<GroupRun>,
    corners: RefCell<IndexMap<(Vec<i64>, i64), Option<Vec<(i64, i64)>>>>,
    ends: Vec<Point>,
    rooms: IndexMap<Top, i64>,
    ordered: Vec<Top>,
    windows: Vec<Feature>,
}

impl GroupTuner {
    /// `GroupTuner.__init__`: find the group's runs and survey every top's room.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        board: &PhysicalBoard,
        obstacles: &RoutingClearanceIndex<'_>,
        tracks: Vec<TrackSegment>,
        units: &[Vec<String>],
        spacings: &[i64],
        amplitude_limit_nm: i64,
        clearance_nm: i64,
        options: TunerOptions,
    ) -> Result<Self> {
        let lines = straight_lines(&tracks, &options.vias);
        let segments: BTreeMap<usize, TrackSegment> =
            lines.iter().map(|(lead, (_, line))| (*lead, line.clone())).collect();
        let mut lead_of = IndexMap::new();
        for (lead, (indices, _)) in &lines {
            for index in indices {
                lead_of.insert(*index, *lead);
            }
        }
        let runs = group_runs(&segments, units, spacings);
        let ends = terminal_ends(&tracks);
        let mut tuner = GroupTuner {
            tracks,
            nets: units.iter().flatten().cloned().collect(),
            limit: amplitude_limit_nm,
            clearance_nm,
            bump_limit: options.bump_limit,
            leg_limit: options.leg_limit,
            segments,
            lead_of,
            runs,
            corners: RefCell::new(IndexMap::new()),
            ends,
            rooms: IndexMap::new(),
            ordered: Vec::new(),
            windows: Vec::new(),
        };
        let mut rooms = IndexMap::new();
        for (index, run) in tuner.runs.iter().enumerate() {
            for top in bump_tops(index, run) {
                if !rooms.contains_key(&top) {
                    let room = tuner.room(board, obstacles, &top)?;
                    rooms.insert(top, room);
                }
            }
        }
        let mut ordered: Vec<Top> = rooms.iter().filter(|(_, room)| **room > 0).map(|(top, _)| *top).collect();
        ordered.sort_by_cached_key(|top| (-rooms[top], tuner.order(top)));
        tuner.rooms = rooms;
        tuner.ordered = ordered;
        if options.style == TuningStyle::Serpentine {
            let mut windows = Vec::new();
            for (index, run) in tuner.runs.iter().enumerate() {
                let gap = options.leg_gap_nm.unwrap_or_else(|| (3 * run.width).max(clearance_nm));
                let tops = serpentine_tops(index, run, gap);
                let mut rooms = IndexMap::new();
                for top in &tops {
                    rooms.insert(*top, tuner.room(board, obstacles, top)?);
                }
                windows.extend(group_windows(&tops, &rooms));
            }
            windows.sort_by_cached_key(|window| (-tuner.capacity(window, &window.rooms), tuner.order(&span(window))));
            tuner.windows = windows;
        }
        Ok(tuner)
    }

    /// `remoteness`: squared distance of a top's bulge-lane centre from the nearest terminal.
    fn remoteness(&self, top: &Top) -> i128 {
        let run = &self.runs[top.run];
        let bulge = if top.side > 0 { &run.lanes[run.lanes.len() - 1] } else { &run.lanes[0] };
        let middle = (top.start + top.end).div_euclid(2);
        let point = if run.horizontal { Point::new(middle, bulge.offset) } else { Point::new(bulge.offset, middle) };
        remoteness(&self.ends, point)
    }

    /// `order`: farthest from the terminals, then by position.
    fn order(&self, top: &Top) -> (i128, &'static str, bool, i64, i64, i64) {
        let run = &self.runs[top.run];
        (-self.remoteness(top), run.layer.value(), !run.horizontal, run.lanes[0].offset, top.start, top.side)
    }

    /// `plan`: add `added_nm` (even) to every lane, serpentines first where declared, else bumps.
    pub fn plan(&self, added_nm: i64) -> Result<UnitTuning> {
        if added_nm <= 0 {
            return Ok(UnitTuning::failed("no length to add"));
        }
        if self.runs.is_empty() {
            return Ok(UnitTuning::failed(format!(
                "no straight run on which all {} lanes run side by side",
                self.nets.len()
            )));
        }
        if self.windows.is_empty() {
            return self.bumps(added_nm);
        }
        let serpentine = self.serpentines(added_nm)?;
        if serpentine.tracks.is_some() {
            return Ok(serpentine);
        }
        let mut bumps = self.bumps(added_nm)?;
        if bumps.tracks.is_none() {
            bumps.reason = format!("{}; serpentine: {}", bumps.reason, serpentine.reason);
        }
        Ok(bumps)
    }

    /// `_bumps`.
    fn bumps(&self, added_nm: i64) -> Result<UnitTuning> {
        let mut chosen: Vec<Top> = Vec::new();
        let mut capacity = 0;
        for top in &self.ordered {
            if capacity >= added_nm || chosen.len() >= self.bump_limit {
                break;
            }
            if chosen.iter().any(|other| self.conflict(top, other)) {
                continue;
            }
            chosen.push(*top);
            let single = Feature::single(*top, self.rooms[top]);
            capacity += self.capacity(&single, &single.rooms);
        }
        if capacity < added_nm {
            let limit = if chosen.len() >= self.bump_limit {
                format!(", the limit of {} bumps", self.bump_limit)
            } else {
                String::new()
            };
            return Ok(UnitTuning::failed(format!(
                "insufficient tuning room: {capacity} of {added_nm} nm in {} group bump(s){limit}; {}",
                chosen.len(),
                self.diagnosis()
            )));
        }
        let features: Vec<Feature> = chosen.iter().map(|top| Feature::single(*top, self.rooms[top])).collect();
        self.build(&features, added_nm)
    }

    /// `_serpentines`.
    fn serpentines(&self, added_nm: i64) -> Result<UnitTuning> {
        let mut chosen: Vec<Feature> = Vec::new();
        let (mut legs, mut capacity) = (0usize, 0i64);
        for window in &self.windows {
            if capacity >= added_nm || legs + 3 > self.leg_limit {
                break;
            }
            if chosen.iter().any(|other| {
                window.tops[0].run == other.tops[0].run
                    || near(self.bounds(window), self.bounds(other), self.margin(window, other))
            }) {
                continue;
            }
            let window = self.trimmed(window, added_nm - capacity, self.leg_limit - legs);
            legs += window.tops.len() + 1;
            capacity += self.capacity(&window, &window.rooms);
            chosen.push(window);
        }
        let mut bumps: Vec<Top> = Vec::new();
        let runs: IndexSet<usize> = chosen.iter().map(|window| window.tops[0].run).collect();
        for top in &self.ordered {
            if chosen.is_empty() || capacity >= added_nm || legs + 2 > self.leg_limit {
                break;
            }
            let single = Feature::single(*top, self.rooms[top]);
            if runs.contains(&top.run)
                || chosen
                    .iter()
                    .any(|window| near(self.bounds(&single), self.bounds(window), self.margin(&single, window)))
                || bumps.iter().any(|other| self.conflict(top, other))
            {
                continue;
            }
            bumps.push(*top);
            legs += 2;
            capacity += self.capacity(&single, &single.rooms);
        }
        if chosen.is_empty() || capacity < added_nm {
            let limit = if legs + 2 > self.leg_limit {
                format!(", the limit of {} legs", self.leg_limit)
            } else {
                String::new()
            };
            return Ok(UnitTuning::failed(format!(
                "insufficient tuning room: {capacity} of {added_nm} nm in {legs} group leg(s){limit}"
            )));
        }
        chosen.extend(bumps.iter().map(|top| Feature::single(*top, self.rooms[top])));
        self.build(&chosen, added_nm)
    }

    /// `_trimmed`: the fewest consecutive tops of `window` that add `need`, within `legs` legs.
    fn trimmed(&self, window: &Feature, need: i64, legs: usize) -> Feature {
        let most = window.tops.len().min(legs.saturating_sub(1));
        for count in 2..=most {
            let mut best: Option<((i64, i128, i64), usize)> = None;
            for first in 0..=window.tops.len() - count {
                let part = cut(window, first, count);
                let key = (self.capacity(&part, &part.rooms), self.remoteness(&span(&part)), -(first as i64));
                if best.as_ref().is_none_or(|(best, _)| key > *best) {
                    best = Some((key, first));
                }
            }
            let part = cut(window, best.map_or(0, |(_, first)| first), count);
            if count == most || self.capacity(&part, &part.rooms) >= need {
                return part;
            }
        }
        window.clone()
    }

    /// `_build`: the features at levelled heights, with one chamfer leg for every leg, adding exactly `added_nm`.
    fn build(&self, features: &[Feature], added_nm: i64) -> Result<UnitTuning> {
        let rooms: Vec<i64> = features.iter().flat_map(|feature| feature.rooms.iter().copied()).collect();
        let sizes = || features.iter().map(|feature| feature.tops.len());
        let mut chamfer = features.iter().map(|feature| self.chamfer(feature, &feature.rooms)).min().unwrap_or(0);
        let mut heights = rooms.clone();
        let mut exhausted = true;
        for _ in 0..CHAMFER_STEPS {
            if chamfer < 1 {
                exhausted = false;
                break;
            }
            let losses: Vec<Option<i64>> = features.iter().map(|feature| self.leg_loss(feature, chamfer)).collect();
            if losses.contains(&None) {
                chamfer -= 1;
                continue;
            }
            let total = added_nm
                + features
                    .iter()
                    .zip(&losses)
                    .map(|(feature, loss)| (feature.tops.len() as i64 + 1) * loss.unwrap_or(0))
                    .sum::<i64>();
            if total.rem_euclid(2) != 0 {
                chamfer -= 1;
                continue;
            }
            heights = levelled(&rooms, total, &|_, height| double(height));
            let fitting = features
                .iter()
                .zip(parts(sizes(), &heights))
                .map(|(feature, part)| self.chamfer(feature, &part))
                .min()
                .unwrap_or(0);
            if fitting >= chamfer {
                exhausted = false;
                break;
            }
            chamfer = fitting;
        }
        if exhausted {
            chamfer = 0;
        }
        if chamfer < 1 {
            let run = &self.runs[features[0].tops[0].run];
            return Ok(UnitTuning::failed(format!(
                "{} mm does not split into group tops of at least {} mm, the least with 45-degree corners",
                mm(added_nm),
                mm(floor(run))
            )));
        }
        let mut lines = Features::new();
        let mut amplitudes: Vec<i64> = Vec::new();
        let (mut bumps, mut legs) = (0i64, 0i64);
        for (feature, part) in features.iter().zip(parts(sizes(), &heights)) {
            let run = &self.runs[feature.tops[0].run];
            let Some(corners) = self.lane_corners(run, chamfer) else {
                return Err(Error::Value("group tuning lost its lane corners".to_owned()));
            };
            let mut positions: Vec<i64> = feature.tops.iter().map(|top| top.start).collect();
            positions.push(feature.tops[feature.tops.len() - 1].end - run.span());
            let mut rises = vec![0];
            rises.extend(feature.tops.iter().zip(&part).map(|(top, height)| top.side * height));
            rises.push(0);
            let offsets: Vec<i64> = run.lanes.iter().map(|lane| lane.offset).collect();
            for (index, lane) in run.lanes.iter().enumerate() {
                let (lower, upper) = corners[index];
                lines.entry(lane.lead).or_default().push(group_path(&positions, &rises, &offsets, index, lower, upper));
            }
            amplitudes.extend(&part);
            if feature.tops.len() == 1 {
                bumps += 1;
            } else {
                legs += feature.tops.len() as i64 + 1;
            }
        }
        let lanes = self.runs[features[0].tops[0].run].lanes.iter().map(|lane| lane.net.clone()).collect();
        Ok(UnitTuning {
            tracks: Some(spliced(&self.tracks, &self.lead_of, &self.segments, &lines)?),
            bumps,
            legs,
            amplitudes_nm: amplitudes,
            reason: String::new(),
            lanes,
        })
    }

    /// `_lane_corners`: `lane_corners` of a run's gaps, cached.
    fn lane_corners(&self, run: &GroupRun, chamfer: i64) -> Option<Vec<(i64, i64)>> {
        let key = (run.gaps(), chamfer);
        if let Some(found) = self.corners.borrow().get(&key) {
            return found.clone();
        }
        let corners = lane_corners(&key.0, chamfer, CORNER_SLACK_NM);
        self.corners.borrow_mut().insert(key, corners.clone());
        corners
    }

    /// `_leg_loss`: length every lane loses at the two corners of one leg of `feature`.
    fn leg_loss(&self, feature: &Feature, chamfer: i64) -> Option<i64> {
        let corners = self.lane_corners(&self.runs[feature.tops[0].run], chamfer)?;
        Some(diagonal_cost(corners[0].0) + diagonal_cost(corners[0].1))
    }

    /// `_chamfer`: the largest chamfer leg every leg of `feature` allows at `heights`.
    ///
    /// R9's rule on each leg of length L: at most the width, L // 4 and
    /// (L - D) // 2, and small enough to leave the inner lane a straight top.
    fn chamfer(&self, feature: &Feature, heights: &[i64]) -> i64 {
        let run = &self.runs[feature.tops[0].run];
        let offset = run.offset_sum();
        feature_legs(heights)
            .into_iter()
            .map(|leg| leg.div_euclid(4).min((leg - offset).div_euclid(2)))
            .fold(run.width.min((feature.tops[0].inner - 1).div_euclid(2)), i64::min)
    }

    /// `_capacity`: length a feature at `heights` adds to every lane (0 if it has no corners).
    fn capacity(&self, feature: &Feature, heights: &[i64]) -> i64 {
        let mut chamfer = self.chamfer(feature, heights);
        while chamfer >= 1 {
            if let Some(loss) = self.leg_loss(feature, chamfer) {
                return 2 * heights.iter().sum::<i64>() - (heights.len() as i64 + 1) * loss;
            }
            chamfer -= 1;
        }
        0
    }

    /// `_room`: tallest clear height of `top` in floor..limit, else 0.
    fn room(&self, board: &PhysicalBoard, obstacles: &RoutingClearanceIndex<'_>, top: &Top) -> Result<i64> {
        let run = &self.runs[top.run];
        tallest(floor(run), self.limit, |height| {
            area_clear(board, obstacles, &self.nets, &swept(run, top, height), run.layer)
        })
    }

    /// `_bounds`.
    fn bounds(&self, feature: &Feature) -> Bounds {
        union(feature.tops.iter().zip(&feature.rooms).map(|(top, &room)| swept(&self.runs[top.run], top, room).bounds))
    }

    /// `_margin`.
    fn margin(&self, first: &Feature, second: &Feature) -> i64 {
        self.runs[first.tops[0].run].width.div_euclid(2)
            + self.runs[second.tops[0].run].width.div_euclid(2)
            + self.clearance_nm
    }

    /// `_conflict`: two bumps cannot both be used, too close on one run or their swept areas near.
    fn conflict(&self, top: &Top, other: &Top) -> bool {
        let run = &self.runs[top.run];
        if top.run == other.run {
            let pitch = 3 * run.width;
            return top.start < other.end + pitch && other.start < top.end + pitch;
        }
        if run.layer != self.runs[other.run].layer {
            return false;
        }
        let first = Feature::single(*top, self.rooms[top]);
        let second = Feature::single(*other, self.rooms[other]);
        near(self.bounds(&first), self.bounds(&second), self.margin(&first, &second))
    }

    /// `_diagnosis`: why the group's bumps fall short, run length, amplitude or room per side.
    fn diagnosis(&self) -> String {
        let length = |run: &GroupRun| run.high - run.low;
        let mut longest = &self.runs[0];
        for run in &self.runs[1..] {
            if length(run) > length(longest) {
                longest = run;
            }
        }
        let (inner, margin) = (3 * longest.width, 3 * longest.width);
        let needed = inner + 2 * longest.span() + 2 * margin;
        if self.runs.iter().all(|run| length(run) < 3 * run.width + 2 * run.span() + 6 * run.width) {
            return format!(
                "the longest straight run of all {} lanes is {} mm, a one-sided group bump needs {} mm: the inner \
                 lane's {} mm plus twice the {} mm lane span, and {} mm at each end",
                longest.lanes.len(),
                mm(length(longest)),
                mm(needed),
                mm(inner),
                mm(longest.span()),
                mm(margin)
            );
        }
        if self.limit < floor(longest) {
            return format!(
                "tuning_amplitude_limit {} mm is below the least group bump height {} mm (45-degree corners across \
                 the {} mm lane span)",
                mm(self.limit),
                mm(floor(longest)),
                mm(longest.span())
            );
        }
        let mut sides: BTreeMap<String, i64> = BTreeMap::new();
        for (top, &room) in &self.rooms {
            let run = &self.runs[top.run];
            let label = format!("{}{}", if top.side > 0 { "+" } else { "-" }, if run.horizontal { "y" } else { "x" });
            let entry = sides.entry(label).or_insert(0);
            *entry = (*entry).max(room);
        }
        let rooms =
            sides.iter().map(|(label, room)| format!("{label} {} mm", mm(*room))).collect::<Vec<_>>().join(", ");
        format!("room beside the lanes: {rooms} (a group bump needs at least {} mm)", mm(floor(longest)))
    }
}

/// `_mm`.
fn mm(value: i64) -> String {
    format!("{:.3}", value as f64 / 1e6)
}

/// `_floor`: least height of a top whose legs have room for 45-degree corners, c >= 1.
fn floor(run: &GroupRun) -> i64 {
    4.max(run.offset_sum() + 2)
}

/// `_runs`: stretches where every net of the group has one straight line, side by side.
///
/// Along each axis and layer, the lines are cut at all their ends; a stretch
/// where each net has exactly one line covering it, and each unit's lanes are
/// adjacent, is a run, joined with the next stretch when the lines are the same.
fn group_runs(segments: &BTreeMap<usize, TrackSegment>, units: &[Vec<String>], spacings: &[i64]) -> Vec<GroupRun> {
    let mut nets: Vec<&str> = units.iter().flatten().map(String::as_str).collect();
    nets.sort();
    let mut axes: BTreeMap<(&'static str, bool), Vec<usize>> = BTreeMap::new();
    for (&lead, line) in segments {
        let Some(h) = horizontal(line) else { continue };
        if nets.contains(&line.net.as_str()) && line.start != line.end {
            axes.entry((line.layer.value(), h)).or_default().push(lead);
        }
    }
    let mut runs = Vec::new();
    for ((_, h), leads) in &axes {
        let h = *h;
        let spans: IndexMap<usize, (i64, i64)> = leads
            .iter()
            .map(|&lead| {
                let (a, b) = (axial(segments[&lead].start, h), axial(segments[&lead].end, h));
                (lead, (a.min(b), a.max(b)))
            })
            .collect();
        let cuts: Vec<i64> =
            spans.values().flat_map(|&(a, b)| [a, b]).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let mut current: Option<(Vec<Lane>, i64, i64)> = None;
        for pair in cuts.windows(2) {
            let (low, high) = (pair[0], pair[1]);
            let covering: Vec<usize> =
                leads.iter().copied().filter(|lead| spans[lead].0 <= low && spans[lead].1 >= high).collect();
            let mut lanes = None;
            let mut covered: Vec<&str> = covering.iter().map(|lead| segments[lead].net.as_str()).collect();
            covered.sort();
            if covered == nets {
                let mut found: Vec<Lane> = covering
                    .iter()
                    .map(|&lead| Lane {
                        lead,
                        net: segments[&lead].net.clone(),
                        offset: normal(segments[&lead].start, h),
                    })
                    .collect();
                found.sort_by(|a, b| (a.offset, &a.net).cmp(&(b.offset, &b.net)));
                if adjacent(&found, units, spacings) {
                    lanes = Some(found);
                }
            }
            if let (Some(found), Some(open)) = (&lanes, &mut current)
                && open.0 == *found
                && open.2 == low
            {
                open.2 = high;
                continue;
            }
            if let Some((lanes, low, high)) = current.take() {
                runs.push(group_run(segments, h, lanes, low, high));
            }
            current = lanes.map(|lanes| (lanes, low, high));
        }
        if let Some((lanes, low, high)) = current {
            runs.push(group_run(segments, h, lanes, low, high));
        }
    }
    runs
}

/// `_run`.
fn group_run(segments: &BTreeMap<usize, TrackSegment>, h: bool, lanes: Vec<Lane>, low: i64, high: i64) -> GroupRun {
    let width = lanes.iter().map(|lane| segments[&lane.lead].width_nm).max().unwrap_or(0);
    GroupRun { layer: segments[&lanes[0].lead].layer, horizontal: h, lanes, low, high, width }
}

/// `_adjacent`: every unit's lanes are neighbours (a pair's at its spacing), all at distinct offsets.
fn adjacent(lanes: &[Lane], units: &[Vec<String>], spacings: &[i64]) -> bool {
    if lanes.windows(2).any(|pair| pair[0].offset == pair[1].offset) {
        return false;
    }
    let position: IndexMap<&str, usize> =
        lanes.iter().enumerate().map(|(index, lane)| (lane.net.as_str(), index)).collect();
    for (unit, &spacing) in units.iter().zip(spacings) {
        let mut indices: Vec<usize> = unit.iter().map(|net| position[net.as_str()]).collect();
        indices.sort_unstable();
        if indices[indices.len() - 1] - indices[0] != indices.len() - 1 {
            return false;
        }
        if unit.len() == 2 && (lanes[indices[1]].offset - lanes[indices[0]].offset - spacing).abs() > LANE_TOLERANCE_NM
        {
            return false;
        }
    }
    true
}

/// `_bump_tops`: one-sided bump slots of a run on both sides.
///
/// Bumps lie on the densest grid (bump, then 3 x width, repeated), centred in
/// the run, and on grids shifted from one end to the other at steps of at
/// least 3 x width (at most `_GRID_SHIFTS` + 1 of them).
fn bump_tops(index: usize, run: &GroupRun) -> Vec<Top> {
    let (width, span) = (run.width, run.span());
    let pitch = 3 * width;
    let bump = pitch + 2 * span;
    let usable = run.high - run.low - 2 * pitch;
    let count = if usable >= bump { (usable + pitch).div_euclid(bump + pitch) } else { 0 };
    if count == 0 {
        return Vec::new();
    }
    let slack = usable - count * bump - (count - 1) * pitch;
    let step = pitch.max(slack.div_euclid(GRID_SHIFTS)).max(1);
    let mut shifts: std::collections::BTreeSet<i64> = (0..=slack).step_by(step as usize).collect();
    shifts.insert(slack.div_euclid(2));
    shifts.insert(slack);
    let mut tops = Vec::new();
    for shift in shifts {
        for position in 0..count {
            let start = run.low + pitch + shift + position * (bump + pitch);
            for side in [1, -1] {
                tops.push(Top { run: index, side, start, end: start + bump, inner: pitch });
            }
        }
    }
    tops
}

/// `_serpentine_tops`: legs a pitch of W + width + gap apart, on a grid centred in the run.
fn serpentine_tops(index: usize, run: &GroupRun, gap: i64) -> Vec<Top> {
    let (width, span) = (run.width, run.span());
    let pitch = span + width + gap;
    let usable = run.high - run.low - 6 * width;
    let count = (usable - span).div_euclid(pitch);
    if count < 2 {
        return Vec::new();
    }
    let first = run.low + 3 * width + (usable - count * pitch - span).div_euclid(2);
    let mut tops = Vec::new();
    for position in 0..count {
        for side in [1, -1] {
            tops.push(Top {
                run: index,
                side,
                start: first + position * pitch,
                end: first + (position + 1) * pitch + span,
                inner: width + gap,
            });
        }
    }
    tops
}

/// `_windows`: for each phase, maximal stretches of two or more consecutive tops with room on alternating sides.
fn group_windows(tops: &[Top], rooms: &IndexMap<Top, i64>) -> Vec<Feature> {
    let count = tops.len() / 2;
    let by_position: IndexMap<(usize, i64), Top> =
        tops.iter().enumerate().map(|(index, top)| ((index / 2, top.side), *top)).collect();
    let mut windows = Vec::new();
    for phase in [1, -1] {
        let sides: Vec<i64> = (0..count).map(|position| if position % 2 == 0 { phase } else { -phase }).collect();
        let mut position = 0;
        while position < count {
            let mut end = position;
            while end < count && rooms[&by_position[&(end, sides[end])]] > 0 {
                end += 1;
            }
            if end - position >= 2 {
                let chosen: Vec<Top> = (position..end).map(|index| by_position[&(index, sides[index])]).collect();
                let rooms = chosen.iter().map(|top| rooms[top]).collect();
                windows.push(Feature { tops: chosen, rooms });
            }
            position = end + 1;
        }
    }
    windows
}

/// `_cut`.
fn cut(window: &Feature, first: usize, count: usize) -> Feature {
    Feature { tops: window.tops[first..first + count].to_vec(), rooms: window.rooms[first..first + count].to_vec() }
}

/// `_span`: a top over the whole feature, for its distance from the terminals.
fn span(window: &Feature) -> Top {
    Top { end: window.tops[window.tops.len() - 1].end, ..window.tops[0] }
}

/// `_swept`: area covered by every lane's copper of `top` at `height`.
///
/// The convex outline of the bulge lane's chamfered top, at the largest foot
/// chamfer (the width) and the least top chamfer any step can use, and of the
/// inner lane's feet at their largest chamfer.
fn swept(run: &GroupRun, top: &Top, height: i64) -> RoundedConvexShape {
    let (span, offset) = (run.span(), run.offset_sum());
    let bulge = if top.side > 0 { &run.lanes[run.lanes.len() - 1] } else { &run.lanes[0] };
    let (base, far) = (bulge.offset, bulge.offset - top.side * span);
    let (foot, crown, far_foot) = (run.width, 1.max(1 + offset - CORNER_SLACK_NM), run.width + offset);
    let rise = base + top.side * height;
    let corners = [
        (top.start + span - far_foot, far),
        (top.start - foot, base),
        (top.start, rise - top.side * crown),
        (top.start + crown, rise),
        (top.end - crown, rise),
        (top.end, rise - top.side * crown),
        (top.end + foot, base),
        (top.end - span + far_foot, far),
    ];
    let mut spine: Vec<Point> = Vec::new();
    for (axial, normal) in corners {
        let point = if run.horizontal { Point::new(axial, normal) } else { Point::new(normal, axial) };
        if !spine.contains(&point) {
            spine.push(point);
        }
    }
    rounded(spine, run.width.div_euclid(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values come from running `pcbir.critical_group_tuning` on the same inputs.

    fn line(net: &str, start: (i64, i64), end: (i64, i64), width: i64) -> TrackSegment {
        TrackSegment {
            net: net.to_owned(),
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            width_nm: width,
            layer: CopperLayer::Front,
        }
    }

    #[test]
    fn corners_of_lanes() {
        assert_eq!(
            lane_corners(&[200_000, 150_000], 100_000, CORNER_SLACK_NM),
            Some(vec![(305024, 100000), (187867, 217157), (100000, 305024)])
        );
        assert_eq!(lane_corners(&[], 50, CORNER_SLACK_NM), Some(vec![(50, 50)]));
        assert_eq!(
            lane_corners(&[101, 57, 233], 20, CORNER_SLACK_NM),
            Some(vec![(248, 20), (189, 79), (156, 112), (20, 248)])
        );
    }

    #[test]
    fn lane_path() {
        assert_eq!(
            group_path(&[0, 1500, 3000], &[0, 700, -700, 0], &[0, 200, 500], 1, 40, 90),
            [
                (260, 0),
                (300, 40),
                (300, 610),
                (390, 700),
                (1610, 700),
                (1700, 610),
                (1700, -660),
                (1740, -700),
                (3260, -700),
                (3300, -660),
                (3300, -90),
                (3390, 0)
            ]
        );
    }

    #[test]
    fn runs_tops_and_outlines() {
        let segments: BTreeMap<usize, TrackSegment> = [
            line("P", (0, 0), (10_000_000, 0), 100_000),
            line("N", (1_000_000, 200_000), (10_000_000, 200_000), 100_000),
            line("S", (0, 800_000), (6_000_000, 800_000), 120_000),
            line("S", (6_000_000, 800_000), (9_000_000, 800_000), 120_000),
        ]
        .into_iter()
        .enumerate()
        .collect();
        let units = [vec!["N".to_owned(), "P".to_owned()], vec!["S".to_owned()]];
        let runs = group_runs(&segments, &units, &[200_000, 0]);
        let brief = |run: &GroupRun| {
            (
                run.lanes.iter().map(|lane| (lane.lead, lane.net.clone(), lane.offset)).collect::<Vec<_>>(),
                run.low,
                run.high,
                run.width,
                run.span(),
                run.gaps(),
                run.offset_sum(),
                floor(run),
            )
        };
        let lanes =
            |last: usize| vec![(0, "P".to_owned(), 0), (1, "N".to_owned(), 200_000), (last, "S".to_owned(), 800_000)];
        assert_eq!(
            runs.iter().map(brief).collect::<Vec<_>>(),
            [
                (lanes(2), 1_000_000, 6_000_000, 120_000, 800_000, vec![200_000, 600_000], 468628, 468630),
                (lanes(3), 6_000_000, 9_000_000, 120_000, 800_000, vec![200_000, 600_000], 468628, 468630),
            ]
        );
        assert!(runs.iter().all(|run| run.horizontal));
        let tops = bump_tops(0, &runs[0]);
        assert_eq!(
            tops.iter().map(|t| (t.side, t.start, t.end, t.inner)).collect::<Vec<_>>(),
            [
                (1, 1_360_000, 3_320_000, 360_000),
                (-1, 1_360_000, 3_320_000, 360_000),
                (1, 3_680_000, 5_640_000, 360_000),
                (-1, 3_680_000, 5_640_000, 360_000)
            ]
        );
        let serpentine = serpentine_tops(0, &runs[0], 400_000);
        assert_eq!(serpentine.len(), 4);
        assert_eq!(serpentine[0], Top { run: 0, side: 1, start: 1_780_000, end: 3_900_000, inner: 520_000 });
        let spine =
            |top: &Top| swept(&runs[0], top, 500_000).spine.iter().map(|p| (p.x_nm, p.y_nm)).collect::<Vec<_>>();
        assert_eq!(
            spine(&tops[0]),
            [
                (1571372, 0),
                (1240000, 800000),
                (1360000, 831379),
                (1828621, 1300000),
                (2851379, 1300000),
                (3320000, 831379),
                (3440000, 800000),
                (3108628, 0)
            ]
        );
        assert_eq!(
            spine(&tops[1]),
            [
                (1571372, 800000),
                (1240000, 0),
                (1360000, -31379),
                (1828621, -500000),
                (2851379, -500000),
                (3320000, -31379),
                (3440000, 0),
                (3108628, 800000)
            ]
        );
        // A pair off its spacing is no run.
        assert!(group_runs(&segments, &units, &[150_000, 0]).is_empty());
    }

    #[test]
    fn group_tuner_end_to_end() {
        use crate::critical_tuning::tests::{board, track};
        // Expected plans from `GroupTuner` on the same board in Python.
        let board = board();
        let obstacle = track("X", (8_000_000, 8_000_000), (12_000_000, 8_000_000), 200_000);
        let obstacles_board = PhysicalBoard { tracks: vec![obstacle], ..board.clone() };
        let index = RoutingClearanceIndex::new(&obstacles_board).expect("index");
        let tracks = vec![
            track("S", (3_000_000, 6_000_000), (10_000_000, 6_000_000), 200_000),
            track("S", (10_000_000, 6_000_000), (17_000_000, 6_000_000), 200_000),
            track("T", (4_000_000, 6_600_000), (16_000_000, 6_600_000), 200_000),
        ];
        let checksum = |tracks: &[TrackSegment]| -> i64 {
            tracks.iter().map(|t| t.start.x_nm * 3 + t.start.y_nm * 7 + t.end.x_nm * 11 + t.end.y_nm * 13).sum()
        };
        let bumps_short = "insufficient tuning room: 12478392 of 40000000 nm in 4 group bump(s); room beside the \
                           lanes: +y 2.000 mm, -y 2.000 mm (a group bump needs at least 0.351 mm)";
        let serpentine_short =
            format!("{bumps_short}; serpentine: insufficient tuning room: 20478392 of 40000000 nm in 8 group leg(s)");
        let expected = [
            (TuningStyle::Bumps, 1_000_000, 1, 0, vec![940201], String::new(), Some((18, 4674367840))),
            (TuningStyle::Bumps, 3_000_000, 1, 0, vec![1940201], String::new(), Some((18, 4514367840))),
            (TuningStyle::Bumps, 40_000_000, 0, 0, vec![], bumps_short.to_owned(), None),
            (TuningStyle::Serpentine, 1_000_000, 1, 0, vec![940201], String::new(), Some((18, 4674367840))),
            (TuningStyle::Serpentine, 3_000_000, 0, 3, vec![1080149, 1080148], String::new(), Some((26, 8195999840))),
            (TuningStyle::Serpentine, 40_000_000, 0, 0, vec![], serpentine_short, None),
        ];
        let units = [vec!["S".to_owned()], vec!["T".to_owned()]];
        for (style, target, bumps, legs, amplitudes, reason, copper) in expected {
            let options = TunerOptions { style, ..TunerOptions::default() };
            let tuner = GroupTuner::new(&board, &index, tracks.clone(), &units, &[0, 0], 2_000_000, 200_000, options)
                .expect("tuner");
            let plan = tuner.plan(target).expect("plan");
            assert_eq!(
                (plan.bumps, plan.legs, plan.amplitudes_nm.clone(), plan.reason.clone()),
                (bumps, legs, amplitudes, reason),
                "{} {target}",
                style.value()
            );
            assert_eq!(plan.tracks.as_deref().map(|t| (t.len(), checksum(t))), copper, "{} {target}", style.value());
            if plan.tracks.is_some() {
                assert_eq!(plan.lanes, ["S", "T"]);
            }
        }
    }
}

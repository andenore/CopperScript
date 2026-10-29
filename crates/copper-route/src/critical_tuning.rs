//! Length-match tuning of accepted critical copper (`pcbir.critical_tuning`,
//! D-PHY plan R3, R9 and R10).
//!
//! After every critical group is accepted, the critical router lengthens the
//! shorter members of each `length_match` group whose skew exceeds its
//! `max_skew`. Tuning works on *units*: one accepted critical group each. A
//! differential pair is one unit and both members receive the same bumps,
//! bent together at the pair's own spacing; a single-ended net is a unit of
//! its own.
//!
//! Every bump corner is a 45-degree chamfer (`bump_chamfers`), so a bump adds
//! exactly `bump_gain` to each member. Candidate slots lie on straight
//! axis-aligned runs (a pair's parallel members, up to three lane spacings
//! apart for twin bumps, or any axis-aligned piece of a single net); slots
//! outside breakout regions come first, then most room, then farthest from the
//! terminals. With `tuning_style = "serpentine"` a unit snakes about its line
//! instead (`serpentine_path`), falling back to bumps. Both are tried outside
//! breakout regions first, then anywhere. The critical router owns the order,
//! the length targets and the atomic validation.

use std::collections::BTreeMap;

use indexmap::{IndexMap, IndexSet};

use copper_drc::legality::shape_in_board;
use copper_json::{Object, Value};
use copper_pcb::board::{PhysicalBoard, TrackSegment, TuningStyle, Via};
use copper_physical::pyfloat::{hypot, round_int};
use copper_physical::{Bounds, CopperLayer, Point, RoundedConvexShape};

use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_metrics::isqrt;
use crate::{Error, Result};

/// `MATCH_TUNING_BUMP_LIMIT`: bound on bumps per tuned unit.
pub const MATCH_TUNING_BUMP_LIMIT: usize = 16;

/// `MATCH_TUNING_LEG_LIMIT`: bound on serpentine legs per tuned unit (plan
/// R10), as many as 16 bumps have.
pub const MATCH_TUNING_LEG_LIMIT: usize = 2 * MATCH_TUNING_BUMP_LIMIT;

/// `_LANE_TOLERANCE_NM`: pair lanes count as coupled within this tolerance.
pub(crate) const LANE_TOLERANCE_NM: i64 = 4;

/// `_WIDE_LANES`: parallel pair members at most this many lane spacings
/// apart still form a run (`_runs`).
const WIDE_LANES: i64 = 3;

/// `_CHAMFER_STEPS`: bound on the chamfer steps of one serpentine plan.
pub(crate) const CHAMFER_STEPS: usize = 64;

// ---------------------------------------------------------------------------
// Reports

/// `MatchTuningMember`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchTuningMember {
    pub net: String,
    pub length_before_nm: i64,
    pub length_after_nm: i64,
}

impl MatchTuningMember {
    /// `added_length_nm`.
    pub fn added_length_nm(&self) -> i64 {
        self.length_after_nm - self.length_before_nm
    }
}

/// `MatchTuningUnit`: one tuning step of one unit (plan R10/R12).
///
/// `stage` is `routing` (tuned as soon as accepted, R12) or `final`; `style`
/// is `serpentine` or `bumps`. A tuning-group step (R14) names its `group`
/// (else empty) and its `lanes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchTuningUnit {
    pub nets: Vec<String>,
    pub stage: String,
    pub style: String,
    pub added_length_nm: i64,
    pub bumps: i64,
    pub legs: i64,
    pub amplitudes_nm: Vec<i64>,
    pub group: String,
    pub lanes: Vec<String>,
}

/// `TuningGroupOutcome`: outcome of one tuning group (plan R14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuningGroupOutcome {
    pub name: String,
    pub nets: Vec<String>,
    pub status: String,
    pub reason: String,
}

/// `MatchTuningResult`: outcome of tuning one `length_match` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchTuningResult {
    pub id: String,
    pub max_skew_nm: i64,
    pub status: String,
    pub skew_before_nm: Option<i64>,
    pub skew_after_nm: Option<i64>,
    pub members: Vec<MatchTuningMember>,
    pub bumps: i64,
    pub reason: String,
    pub units: Vec<MatchTuningUnit>,
    pub tuning_groups: Vec<TuningGroupOutcome>,
}

impl MatchTuningResult {
    /// `legs`: serpentine legs over every step.
    pub fn legs(&self) -> i64 {
        self.units.iter().map(|unit| unit.legs).sum()
    }

    /// `reported_units`: a step used a serpentine, ran while routing or tuned a group.
    pub fn reported_units(&self) -> bool {
        self.units.iter().any(|unit| unit.stage == "routing" || unit.legs != 0 || !unit.group.is_empty())
    }
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

fn texts(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| Value::from(item.as_str())).collect())
}

/// `match_tuning_document`: JSON-ready form of one group's tuning outcome.
pub fn match_tuning_document(item: &MatchTuningResult) -> Value {
    let mut document = vec![
        ("id", Value::from(item.id.as_str())),
        ("max_skew_nm", Value::from(item.max_skew_nm)),
        ("status", Value::from(item.status.as_str())),
        ("skew_before_nm", item.skew_before_nm.map_or(Value::Null, Value::from)),
        ("skew_after_nm", item.skew_after_nm.map_or(Value::Null, Value::from)),
        (
            "members",
            Value::Array(
                item.members
                    .iter()
                    .map(|member| {
                        object(vec![
                            ("net", Value::from(member.net.as_str())),
                            ("length_before_nm", Value::from(member.length_before_nm)),
                            ("length_after_nm", Value::from(member.length_after_nm)),
                            ("added_length_nm", Value::from(member.added_length_nm())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("bumps", Value::from(item.bumps)),
        ("reason", Value::from(item.reason.as_str())),
    ];
    if item.reported_units() {
        document.push(("legs", Value::from(item.legs())));
        let units = item
            .units
            .iter()
            .map(|unit| {
                let mut entries = vec![
                    ("nets", texts(&unit.nets)),
                    ("stage", Value::from(unit.stage.as_str())),
                    ("style", Value::from(unit.style.as_str())),
                    ("added_length_nm", Value::from(unit.added_length_nm)),
                    ("bumps", Value::from(unit.bumps)),
                    ("legs", Value::from(unit.legs)),
                    ("amplitudes_nm", Value::Array(unit.amplitudes_nm.iter().map(|a| Value::from(*a)).collect())),
                ];
                if !unit.group.is_empty() {
                    entries.push(("group", Value::from(unit.group.as_str())));
                    entries.push(("lanes", texts(&unit.lanes)));
                }
                object(entries)
            })
            .collect();
        document.push(("units", Value::Array(units)));
    }
    if !item.tuning_groups.is_empty() {
        let groups = item
            .tuning_groups
            .iter()
            .map(|group| {
                object(vec![
                    ("name", Value::from(group.name.as_str())),
                    ("nets", texts(&group.nets)),
                    ("status", Value::from(group.status.as_str())),
                    ("reason", Value::from(group.reason.as_str())),
                ])
            })
            .collect();
        document.push(("tuning_groups", Value::Array(groups)));
    }
    object(document)
}

/// `mm` of `match_tuning_line`: `f"{value / 1e6:.3f}"`, `n/a` for None.
fn mm(value: Option<i64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{:.3}", value as f64 / 1e6))
}

/// `match_tuning_line`: one console line per group, as the critical preflight prints it.
pub fn match_tuning_line(item: &MatchTuningResult) -> String {
    let added = item
        .members
        .iter()
        .filter(|member| member.added_length_nm() != 0)
        .map(|member| format!("{} +{}", member.net, mm(Some(member.added_length_nm()))))
        .collect::<Vec<_>>()
        .join(", ");
    let mut steps = String::new();
    if item.reported_units() {
        let parts = item
            .units
            .iter()
            .map(|unit| {
                let mut text = if unit.group.is_empty() {
                    unit.nets.join("/")
                } else {
                    format!("group {} {}", unit.group, unit.lanes.join("/"))
                };
                text.push_str(&format!(" {}", unit.style));
                if unit.legs != 0 {
                    text.push_str(&format!(" {} legs", unit.legs));
                }
                if unit.bumps != 0 {
                    text.push_str(&format!(" {} bumps", unit.bumps));
                }
                text.push_str(&format!(
                    " up to {} mm",
                    mm(Some(unit.amplitudes_nm.iter().copied().max().unwrap_or(0)))
                ));
                if unit.stage == "routing" {
                    text.push_str(" while routing");
                }
                text
            })
            .collect::<Vec<_>>();
        steps = format!("; {}", parts.join(", "));
    }
    let groups: String = item
        .tuning_groups
        .iter()
        .filter(|group| group.status != "tuned")
        .map(|group| {
            let reason = if group.reason.is_empty() { String::new() } else { format!(" ({})", group.reason) };
            format!("; tuning group {} {}{}", group.name, group.status, reason)
        })
        .collect();
    let mut line = format!(
        "match group {}: {}, skew {} -> {} mm (max {} mm)",
        item.id,
        item.status,
        mm(item.skew_before_nm),
        mm(item.skew_after_nm),
        mm(Some(item.max_skew_nm))
    );
    if !added.is_empty() {
        line.push_str(&format!("; added {added} mm"));
    }
    if item.bumps != 0 && steps.is_empty() {
        line.push_str(&format!("; {} bumps", item.bumps));
    }
    line.push_str(&steps);
    line.push_str(&groups);
    if !item.reason.is_empty() {
        line.push_str(&format!(": {}", item.reason));
    }
    line
}

// ---------------------------------------------------------------------------
// Slots and plans

/// `_Slot`: one candidate bump of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Slot {
    /// First member's line, partner's (None for one net).
    pub run: (usize, Option<usize>),
    /// Grid index inside the run.
    pub position: i64,
    pub layer: CopperLayer,
    /// The run is along x.
    pub horizontal: bool,
    /// +1 or -1 along the normal axis.
    pub side: i64,
    /// Line of the member on the bulge side.
    pub outer: usize,
    /// Line of the other pair member.
    pub inner: Option<usize>,
    /// Normal coordinate of the outer member's line.
    pub base: i64,
    /// Outer member's bump interval along the axis.
    pub start: i64,
    pub end: i64,
    /// Lane distance of a pair, 0 for a single net.
    pub spacing: i64,
    pub width: i64,
    /// A twin bump: the inner member bulges from this normal coordinate toward -side.
    pub twin_base: Option<i64>,
}

/// `UnitTuning`: one unit's tuning, the tuned copper or `None` and the reason.
///
/// `legs` counts serpentine legs and `amplitudes_nm` every serpentine top's
/// and bump's height, in placement order; `lanes` a tuning group's lanes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitTuning {
    pub tracks: Option<Vec<TrackSegment>>,
    pub bumps: i64,
    pub legs: i64,
    pub amplitudes_nm: Vec<i64>,
    pub reason: String,
    pub lanes: Vec<String>,
}

impl UnitTuning {
    /// `UnitTuning(None, reason=...)`.
    pub fn failed(reason: impl Into<String>) -> Self {
        UnitTuning { reason: reason.into(), ..UnitTuning::default() }
    }

    /// `style`: `serpentine` when the step has legs, else `bumps`.
    pub fn style(&self) -> &'static str {
        if self.legs != 0 { "serpentine" } else { "bumps" }
    }
}

/// `_Window`: consecutive serpentine tops of one run on alternating sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    /// Each top as the bump over its two legs on its side.
    pub tops: Vec<Slot>,
    pub rooms: Vec<i64>,
    /// Leg pitch along the run.
    pub pitch: i64,
    /// Least edge gap between adjacent legs.
    pub gap: i64,
}

/// The keyword arguments of `UnitTuner` (and `GroupTuner`) after the required ones.
#[derive(Clone, Debug)]
pub struct TunerOptions {
    pub bump_limit: usize,
    pub style: TuningStyle,
    /// The declared `tuning_spacing` (None for the default).
    pub leg_gap_nm: Option<i64>,
    pub leg_limit: usize,
    pub vias: Vec<Via>,
}

impl Default for TunerOptions {
    fn default() -> Self {
        TunerOptions {
            bump_limit: MATCH_TUNING_BUMP_LIMIT,
            style: TuningStyle::Bumps,
            leg_gap_nm: None,
            leg_limit: MATCH_TUNING_LEG_LIMIT,
            vias: Vec::new(),
        }
    }
}

/// Features (corner lists) to splice into each line, by line index.
pub(crate) type Features = BTreeMap<usize, Vec<Vec<(i64, i64)>>>;

/// `UnitTuner`: candidate bump slots of one unit, with their room, in placement order.
///
/// `tracks` is the unit's accepted copper and `obstacles` indexes every other
/// copper object. `nets` has one net, or both pair members, whose lanes are
/// `spacing_nm` apart. Rooms are measured once, so several length targets
/// cost one survey.
pub struct UnitTuner {
    tracks: Vec<TrackSegment>,
    clearance_nm: i64,
    bump_limit: usize,
    leg_limit: usize,
    segments: BTreeMap<usize, TrackSegment>,
    lead_of: IndexMap<usize, usize>,
    rooms: IndexMap<Slot, i64>,
    ends: Vec<Point>,
    /// Slots (with room, and window tops) whose centre lies in a breakout region.
    inside: IndexSet<Slot>,
    ordered: Vec<Slot>,
    windows: Vec<Window>,
}

/// `centre` of `UnitTuner`: the middle of a slot on its outer member's line.
fn centre(slot: &Slot) -> Point {
    let middle = (slot.start + slot.end).div_euclid(2);
    if slot.horizontal { Point::new(middle, slot.base) } else { Point::new(slot.base, middle) }
}

/// Least squared distance of `point` from the terminal `ends` (0 without ends).
pub(crate) fn remoteness(ends: &[Point], point: Point) -> i128 {
    ends.iter()
        .map(|end| {
            let (dx, dy) = ((point.x_nm - end.x_nm) as i128, (point.y_nm - end.y_nm) as i128);
            dx * dx + dy * dy
        })
        .min()
        .unwrap_or(0)
}

impl UnitTuner {
    /// `UnitTuner.__init__`: survey every slot's room and, for serpentines, every window.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        board: &PhysicalBoard,
        obstacles: &RoutingClearanceIndex<'_>,
        tracks: Vec<TrackSegment>,
        nets: &[String],
        spacing_nm: i64,
        amplitude_limit_nm: i64,
        clearance_nm: i64,
        options: TunerOptions,
    ) -> Result<Self> {
        let serpentine = options.style == TuningStyle::Serpentine;
        let mut segments: BTreeMap<usize, TrackSegment> = tracks.iter().cloned().enumerate().collect();
        let mut lead_of = IndexMap::new();
        if serpentine {
            let lines = straight_lines(&tracks, &options.vias);
            segments = lines.iter().map(|(lead, (_, line))| (*lead, line.clone())).collect();
            for (lead, (indices, _)) in &lines {
                for index in indices {
                    lead_of.insert(*index, *lead);
                }
            }
        }
        let runs = if amplitude_limit_nm > 0 { unit_runs(&segments, nets, spacing_nm) } else { Vec::new() };
        let slots = slots(&segments, &runs, spacing_nm);
        let mut rooms = IndexMap::new();
        for slot in &slots {
            rooms.insert(*slot, room(board, obstacles, nets, slot, amplitude_limit_nm, 1, None)?);
        }
        let ends = terminal_ends(&tracks);
        let breakout = &obstacles.breakout;
        let declared = !breakout.declared.is_empty();
        let is_inside = |slot: &Slot| declared && breakout.inside_any(centre(slot));
        let mut inside = IndexSet::new();
        let mut ordered: Vec<Slot> = slots.iter().copied().filter(|slot| rooms[slot] > 0).collect();
        for slot in &ordered {
            if is_inside(slot) {
                inside.insert(*slot);
            }
        }
        ordered.sort_by_cached_key(|slot| {
            (
                inside.contains(slot),
                -rooms[slot],
                -remoteness(&ends, centre(slot)),
                slot.layer.value(),
                !slot.horizontal,
                slot.base,
                slot.start,
                slot.side,
            )
        });
        let mut windows = Vec::new();
        if serpentine {
            for run in &runs {
                if run.second.is_some() && run.distance > spacing_nm + LANE_TOLERANCE_NM {
                    continue; // Wide runs take twin bumps only (`_slots`).
                }
                windows.extend(unit_windows(
                    board,
                    obstacles,
                    &segments,
                    nets,
                    run,
                    amplitude_limit_nm,
                    options.leg_gap_nm,
                    clearance_nm,
                )?);
            }
            for window in &windows {
                for top in &window.tops {
                    if is_inside(top) {
                        inside.insert(*top);
                    }
                }
            }
            windows.sort_by_cached_key(|window| {
                let top = &window.tops[0];
                (
                    window.tops.iter().any(|top| inside.contains(top)),
                    -window_capacity(window, &window.rooms),
                    -remoteness(&ends, centre(&span(window))),
                    top.layer.value(),
                    !top.horizontal,
                    top.base,
                    top.start,
                    top.side,
                )
            });
        }
        Ok(UnitTuner {
            tracks,
            clearance_nm,
            bump_limit: options.bump_limit,
            leg_limit: options.leg_limit,
            segments,
            lead_of,
            rooms,
            ends,
            inside,
            ordered,
            windows,
        })
    }

    fn is_inside(&self, slot: &Slot) -> bool {
        self.inside.contains(slot)
    }

    fn remoteness(&self, slot: &Slot) -> i128 {
        remoteness(&self.ends, centre(slot))
    }

    /// `tune`: add `added_nm` (even) to every net of the unit.
    ///
    /// Returns the tuned copper in the original track order (None when the
    /// length does not fit), the bump count and the reason.
    pub fn tune(&self, added_nm: i64) -> Result<(Option<Vec<TrackSegment>>, i64, String)> {
        let result = self.plan(added_nm)?;
        Ok((result.tracks, result.bumps, result.reason))
    }

    /// `plan`: add `added_nm` to every net of the unit, serpentines first, else bumps alone.
    ///
    /// Both are tried outside breakout regions first, then anywhere.
    pub fn plan(&self, added_nm: i64) -> Result<UnitTuning> {
        if added_nm <= 0 {
            return Ok(UnitTuning::failed("no length to add"));
        }
        let outside = !self.ordered.iter().any(|slot| self.is_inside(slot))
            && !self.windows.iter().any(|window| window.tops.iter().any(|top| self.is_inside(top)));
        let passes: &[bool] = if outside { &[true] } else { &[false, true] };
        let mut serpentine = UnitTuning::default();
        let mut bumps = UnitTuning::default();
        for &anywhere in passes {
            if !self.windows.is_empty() {
                serpentine = self.serpentines(added_nm, anywhere)?;
                if serpentine.tracks.is_some() {
                    return Ok(serpentine);
                }
            }
            bumps = self.bumps(added_nm, anywhere)?;
            if bumps.tracks.is_some() {
                return Ok(bumps);
            }
        }
        if self.windows.is_empty() {
            return Ok(bumps);
        }
        bumps.reason = format!("{}; serpentine: {}", bumps.reason, serpentine.reason);
        Ok(bumps)
    }

    /// `_bumps`: one-sided bumps only (plan R3), most room first.
    fn bumps(&self, added_nm: i64, anywhere: bool) -> Result<UnitTuning> {
        let mut chosen: Vec<Slot> = Vec::new();
        let mut capacity = 0;
        for slot in &self.ordered {
            if !anywhere && self.is_inside(slot) {
                continue;
            }
            if capacity >= added_nm || chosen.len() >= self.bump_limit {
                break;
            }
            if chosen.iter().any(|other| conflict(slot, other, self.rooms[slot], self.rooms[other], self.clearance_nm))
            {
                continue;
            }
            chosen.push(*slot);
            capacity += slot_gain(slot, self.rooms[slot]);
        }
        if capacity < added_nm {
            let limit = if chosen.len() >= self.bump_limit {
                format!(", the limit of {} bumps", self.bump_limit)
            } else {
                String::new()
            };
            return Ok(UnitTuning::failed(format!(
                "insufficient tuning room: {capacity} of {added_nm} nm in {} bump(s){limit}",
                chosen.len()
            )));
        }
        let rooms: Vec<i64> = chosen.iter().map(|slot| self.rooms[slot]).collect();
        let heights = levelled(&rooms, added_nm, &|index, height| slot_gain(&chosen[index], height));
        let mut features = Features::new();
        let used = add_bumps(&mut features, &chosen, &heights);
        Ok(UnitTuning {
            tracks: Some(self.spliced(&features)?),
            bumps: used.len() as i64,
            legs: 0,
            amplitudes_nm: used,
            ..UnitTuning::default()
        })
    }

    /// `_serpentines`: serpentine windows, most capacity first, then bumps on the other runs.
    fn serpentines(&self, added_nm: i64, anywhere: bool) -> Result<UnitTuning> {
        let mut chosen: Vec<Window> = Vec::new();
        let (mut legs, mut capacity) = (0usize, 0i64);
        for window in &self.windows {
            if capacity >= added_nm || legs + 3 > self.leg_limit {
                break;
            }
            if !anywhere && window.tops.iter().any(|top| self.is_inside(top)) {
                continue;
            }
            if chosen.iter().any(|other| {
                window.tops[0].run == other.tops[0].run
                    || near(
                        window_bounds(window),
                        window_bounds(other),
                        window.tops[0].width.div_euclid(2) + other.tops[0].width.div_euclid(2) + self.clearance_nm,
                    )
            }) {
                continue;
            }
            let window = self.trimmed(window, added_nm - capacity, self.leg_limit - legs);
            legs += window.tops.len() + 1;
            capacity += window_capacity(&window, &window.rooms);
            chosen.push(window);
        }
        let mut bumps: Vec<Slot> = Vec::new();
        let runs: IndexSet<(usize, Option<usize>)> = chosen.iter().map(|window| window.tops[0].run).collect();
        for slot in &self.ordered {
            if chosen.is_empty() || capacity >= added_nm || legs + 2 > self.leg_limit {
                break;
            }
            let bounds = slot_bounds(slot, self.rooms[slot]);
            if runs.contains(&slot.run)
                || !anywhere && self.is_inside(slot)
                || chosen.iter().any(|window| {
                    near(
                        bounds,
                        window_bounds(window),
                        slot.width.div_euclid(2) + window.tops[0].width.div_euclid(2) + self.clearance_nm,
                    )
                })
                || bumps
                    .iter()
                    .any(|other| conflict(slot, other, self.rooms[slot], self.rooms[other], self.clearance_nm))
            {
                continue;
            }
            bumps.push(*slot);
            legs += 2;
            capacity += slot_gain(slot, self.rooms[slot]);
        }
        if chosen.is_empty() || capacity < added_nm {
            let limit = if legs + 2 > self.leg_limit {
                format!(", the limit of {} legs", self.leg_limit)
            } else {
                String::new()
            };
            return Ok(UnitTuning::failed(format!(
                "insufficient tuning room: {capacity} of {added_nm} nm in {legs} leg(s){limit}"
            )));
        }
        // One chamfer leg for every serpentine, lowered until it suits every
        // leg of the levelled heights and makes the added length even.
        let mut rooms: Vec<i64> = chosen.iter().flat_map(|window| window.rooms.iter().copied()).collect();
        rooms.extend(bumps.iter().map(|slot| self.rooms[slot]));
        let serpentine_tops = rooms.len() - bumps.len();
        let gain = |index: usize, height: i64| {
            if index < serpentine_tops { double(height) } else { slot_gain(&bumps[index - serpentine_tops], height) }
        };
        let mut chamfer = chosen.iter().map(|window| window_chamfer(window, &window.rooms)).min().unwrap_or(0);
        let mut heights = rooms.clone();
        let mut exhausted = true;
        for _ in 0..CHAMFER_STEPS {
            if chamfer < 1 {
                exhausted = false;
                break;
            }
            let total = added_nm
                + chosen
                    .iter()
                    .map(|window| (window.tops.len() as i64 + 1) * leg_loss(chamfer, window.tops[0].spacing))
                    .sum::<i64>();
            if total.rem_euclid(2) != 0 {
                chamfer -= 1;
                continue;
            }
            heights = levelled(&rooms, total, &gain);
            let fitting = chosen
                .iter()
                .zip(parts(chosen.iter().map(|window| window.tops.len()), &heights))
                .map(|(window, part)| window_chamfer(window, &part))
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
            return Ok(UnitTuning::failed("serpentine tops too low for 45-degree corners"));
        }
        let mut features = Features::new();
        let mut amplitudes: Vec<i64> = Vec::new();
        for (window, part) in chosen.iter().zip(parts(chosen.iter().map(|window| window.tops.len()), &heights)) {
            add_serpentine(&mut features, &self.segments, window, &part, chamfer);
            amplitudes.extend(part);
        }
        let used = add_bumps(&mut features, &bumps, &heights[heights.len() - bumps.len()..]);
        let bump_count = used.len() as i64;
        amplitudes.extend(used);
        Ok(UnitTuning {
            tracks: Some(self.spliced(&features)?),
            bumps: bump_count,
            legs: chosen.iter().map(|window| window.tops.len() as i64 + 1).sum(),
            amplitudes_nm: amplitudes,
            ..UnitTuning::default()
        })
    }

    /// `_trimmed`: the fewest consecutive tops of `window` that add `need`, within `legs` legs.
    ///
    /// Of the windows of that length, the one with the most capacity is kept,
    /// then the one farthest from the terminals, then the first.
    fn trimmed(&self, window: &Window, need: i64, legs: usize) -> Window {
        let most = window.tops.len().min(legs.saturating_sub(1));
        for count in 2..=most {
            let mut best: Option<((i64, i128, i64), usize)> = None;
            for first in 0..=window.tops.len() - count {
                let key = (
                    window_capacity(window, &window.rooms[first..first + count]),
                    self.remoteness(&span(&cut(window, first, count))),
                    -(first as i64),
                );
                if best.as_ref().is_none_or(|(best, _)| key > *best) {
                    best = Some((key, first));
                }
            }
            let first = best.map_or(0, |(_, first)| first);
            let part = cut(window, first, count);
            if count == most || window_capacity(&part, &part.rooms) >= need {
                return part;
            }
        }
        window.clone()
    }

    /// `_spliced`: the unit's copper in the original track order, each tuned line replacing its pieces.
    fn spliced(&self, features: &Features) -> Result<Vec<TrackSegment>> {
        spliced(&self.tracks, &self.lead_of, &self.segments, features)
    }
}

/// `_spliced` of `UnitTuner` and `GroupTuner`.
pub(crate) fn spliced(
    tracks: &[TrackSegment],
    lead_of: &IndexMap<usize, usize>,
    segments: &BTreeMap<usize, TrackSegment>,
    features: &Features,
) -> Result<Vec<TrackSegment>> {
    let mut tuned = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let lead = lead_of.get(&index).copied().unwrap_or(index);
        match features.get(&lead) {
            None => tuned.push(track.clone()),
            Some(items) if index == lead => tuned.extend(bumped(&segments[&lead], items)?),
            Some(_) => {}
        }
    }
    Ok(tuned)
}

/// `_double`.
pub(crate) fn double(height: i64) -> i64 {
    2 * height
}

/// `_add_bumps`: add each bump's corners to its members; the heights of the bumps that add length.
fn add_bumps(features: &mut Features, slots: &[Slot], heights: &[i64]) -> Vec<i64> {
    let mut used = Vec::new();
    for (slot, &height) in slots.iter().zip(heights) {
        if slot_gain(slot, height) == 0 {
            continue;
        }
        used.push(height);
        // The bulge-side member is inside the turn at the run corners, the
        // other member at the top corners.
        let (inside, outside) = bump_chamfers(height, slot.width, slot.spacing);
        let path = bump_path(slot.start, slot.end, height, inside, outside);
        features.entry(slot.outer).or_default().push(path.iter().map(|&(a, r)| (a, slot.side * r)).collect());
        if slot.twin_base.is_some() {
            let inner = slot.inner.expect("a twin bump has an inner member");
            features.entry(inner).or_default().push(path.iter().map(|&(a, r)| (a, -slot.side * r)).collect());
        } else if let Some(inner) = slot.inner {
            let path = bump_path(slot.start + slot.spacing, slot.end - slot.spacing, height, outside, inside);
            features.entry(inner).or_default().push(path.iter().map(|&(a, r)| (a, slot.side * r)).collect());
        }
    }
    used
}

/// `bump_chamfers`: chamfer legs `(inside, outside)` of a bump's corners (plan R9).
///
/// At each corner the member inside the turn takes `inside` and its pair
/// partner `inside + d`, d = floor((2 - sqrt 2) x `spacing`) (0 for a single
/// net). A bump too low for `inside >= 1` has one 45-degree ramp per side:
/// `inside + outside == height`.
pub fn bump_chamfers(height: i64, width: i64, spacing: i64) -> (i64, i64) {
    let offset = chamfer_offset(spacing);
    let inside = width.min(height.div_euclid(4)).min((height - offset).div_euclid(2));
    if inside < 1 {
        let inside = -(offset - height).div_euclid(2);
        return (inside, height - inside);
    }
    (inside, inside + offset)
}

/// `_chamfer_offset`: d = floor((2 - sqrt 2) x `spacing`), 0 for a single net (spacing 0).
pub(crate) fn chamfer_offset(spacing: i64) -> i64 {
    if spacing > 0 { 2 * spacing - isqrt(2 * spacing as i128 * spacing as i128) - 1 } else { 0 }
}

/// `round(hypot(leg, leg))`: the rounded length of a 45-degree piece.
pub(crate) fn diagonal(leg: i64) -> i64 {
    round_int(hypot(leg as f64, leg as f64))
}

/// `bump_gain`: length a member gains from a bump, as critical lengths measure it.
///
/// `base`/`top` are the member's chamfer legs at the run and top corners
/// (`bump_chamfers`); every piece of `bump_path` is rounded on its own.
pub fn bump_gain(height: i64, base: i64, top: i64) -> i64 {
    if base.min(top) < 1 {
        return 2 * (diagonal(height) - height);
    }
    2 * height + 2 * (diagonal(base) + diagonal(top)) - 4 * (base + top)
}

/// `bump_path`: centre-line corners `(axial, rise)` of one member's bump, in travel order.
///
/// `entry`/`leave` are the axial positions of the legs; a leg below 1 makes
/// each side one ramp.
pub fn bump_path(entry: i64, leave: i64, height: i64, base: i64, top: i64) -> Vec<(i64, i64)> {
    let sign = if leave > entry { 1 } else { -1 };
    if base.min(top) < 1 {
        return vec![
            (entry - sign * base, 0),
            (entry + sign * top, height),
            (leave - sign * top, height),
            (leave + sign * base, 0),
        ];
    }
    vec![
        (entry - sign * base, 0),
        (entry, base),
        (entry, height - top),
        (entry + sign * top, height),
        (leave - sign * top, height),
        (leave, height - top),
        (leave, base),
        (leave + sign * base, 0),
    ]
}

/// `_slot_gain`: length each member gains from a bump of `height` in `slot`.
fn slot_gain(slot: &Slot, height: i64) -> i64 {
    let (base, top) = bump_chamfers(height, slot.width, slot.spacing);
    bump_gain(height, base, top)
}

/// `_horizontal`: Some(true) along x, Some(false) along y, None for a diagonal.
pub(crate) fn horizontal(track: &TrackSegment) -> Option<bool> {
    if track.start.y_nm == track.end.y_nm {
        Some(true)
    } else if track.start.x_nm == track.end.x_nm {
        Some(false)
    } else {
        None
    }
}

/// `_axial`.
pub(crate) fn axial(point: Point, horizontal: bool) -> i64 {
    if horizontal { point.x_nm } else { point.y_nm }
}

/// `_normal`.
pub(crate) fn normal(point: Point, horizontal: bool) -> i64 {
    if horizontal { point.y_nm } else { point.x_nm }
}

/// `_straight_lines`: maximal straight lines of the axis-aligned pieces, by their first index.
///
/// Collinear pieces of one net, layer and width join where they meet end to
/// end at a point no other piece or via of the net touches. Each line is one
/// segment in its first piece's direction; the value holds the joined
/// piece indices (ascending) and the line.
pub(crate) fn straight_lines(tracks: &[TrackSegment], vias: &[Via]) -> BTreeMap<usize, (Vec<usize>, TrackSegment)> {
    let mut ends: IndexMap<(&str, CopperLayer, Point), i64> = IndexMap::new();
    for track in tracks {
        for point in [track.start, track.end] {
            *ends.entry((track.net.as_str(), track.layer, point)).or_insert(0) += 1;
        }
    }
    let blocked: IndexSet<(&str, Point)> = vias.iter().map(|via| (via.net.as_str(), via.position)).collect();
    let mut indices: Vec<usize> = (0..tracks.len())
        .filter(|&index| tracks[index].start != tracks[index].end && horizontal(&tracks[index]).is_some())
        .collect();
    indices.sort_by_cached_key(|&index| {
        let track = &tracks[index];
        let h = horizontal(track).unwrap_or(true);
        (
            track.net.clone(),
            track.layer.value(),
            h,
            normal(track.start, h),
            axial(track.start, h).min(axial(track.end, h)),
            index,
        )
    });
    let lowest = |a: Point, b: Point, h: bool| if axial(b, h) < axial(a, h) { b } else { a };
    let highest = |a: Point, b: Point, h: bool| if axial(b, h) > axial(a, h) { b } else { a };
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        let track = &tracks[index];
        let h = horizontal(track).unwrap_or(true);
        if let Some(chain) = chains.last_mut() {
            let last = &tracks[*chain.last().expect("chains are never empty")];
            let joint = lowest(track.start, track.end, h);
            if last.net == track.net
                && last.layer == track.layer
                && horizontal(last) == Some(h)
                && last.width_nm == track.width_nm
                && joint == highest(last.start, last.end, h)
                && normal(last.start, h) == normal(track.start, h)
                && ends[&(track.net.as_str(), track.layer, joint)] == 2
                && !blocked.contains(&(track.net.as_str(), joint))
            {
                chain.push(index);
                continue;
            }
        }
        chains.push(vec![index]);
    }
    let mut lines = BTreeMap::new();
    for mut chain in chains {
        chain.sort_unstable();
        let lead = chain[0];
        let h = horizontal(&tracks[lead]).unwrap_or(true);
        let points: std::collections::BTreeSet<(i64, Point)> = chain
            .iter()
            .flat_map(|&index| [tracks[index].start, tracks[index].end])
            .map(|point| (axial(point, h), point))
            .collect();
        let (mut first, mut last) =
            (points.first().expect("a line has points").1, points.last().expect("a line has points").1);
        if axial(tracks[lead].end, h) < axial(tracks[lead].start, h) {
            std::mem::swap(&mut first, &mut last);
        }
        let line = if chain.len() == 1 {
            tracks[lead].clone()
        } else {
            TrackSegment { start: first, end: last, ..tracks[lead].clone() }
        };
        lines.insert(lead, (chain, line));
    }
    lines
}

/// `_Run`: `(first, second, horizontal, low, high, distance)`, `second` None for one net.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnitRun {
    pub first: usize,
    pub second: Option<usize>,
    pub horizontal: bool,
    pub low: i64,
    pub high: i64,
    pub distance: i64,
}

/// `_runs`: straight runs of the unit.
///
/// For a pair, the parallel `segments` of both members on one layer over
/// their common interval, `spacing_nm` apart or wider up to `_WIDE_LANES`
/// times it. For a single net every axis-aligned segment.
pub(crate) fn unit_runs(segments: &BTreeMap<usize, TrackSegment>, nets: &[String], spacing_nm: i64) -> Vec<UnitRun> {
    let mut runs = Vec::new();
    for (&first, track) in segments {
        let Some(h) = horizontal(track) else { continue };
        if track.net != nets[0] {
            continue;
        }
        let (a, b) = (axial(track.start, h), axial(track.end, h));
        let (low, high) = (a.min(b), a.max(b));
        if nets.len() == 1 {
            runs.push(UnitRun { first, second: None, horizontal: h, low, high, distance: 0 });
            continue;
        }
        for (&second, partner) in segments {
            if partner.net != nets[1] || partner.layer != track.layer || horizontal(partner) != Some(h) {
                continue;
            }
            let distance = (normal(track.start, h) - normal(partner.start, h)).abs();
            if !(spacing_nm - LANE_TOLERANCE_NM <= distance && distance <= WIDE_LANES * spacing_nm) {
                continue;
            }
            let (c, d) = (axial(partner.start, h), axial(partner.end, h));
            let (other_low, other_high) = (c.min(d), c.max(d));
            if low.max(other_low) < high.min(other_high) {
                runs.push(UnitRun {
                    first,
                    second: Some(second),
                    horizontal: h,
                    low: low.max(other_low),
                    high: high.min(other_high),
                    distance,
                });
            }
        }
    }
    runs
}

/// `_run_slot`: the bump of `run` over the outer member's interval `start..end` toward `side`.
fn run_slot(
    tracks: &BTreeMap<usize, TrackSegment>,
    run: &UnitRun,
    position: i64,
    side: i64,
    start: i64,
    end: i64,
    width: i64,
) -> Slot {
    let h = run.horizontal;
    let (mut outer, mut inner) = (run.first, None);
    if let Some(second) = run.second {
        let lane = normal(tracks[&run.first].start, h) - normal(tracks[&second].start, h);
        (outer, inner) = if side * lane > 0 { (run.first, Some(second)) } else { (second, Some(run.first)) };
    }
    Slot {
        run: (run.first, run.second),
        position,
        layer: tracks[&run.first].layer,
        horizontal: h,
        side,
        outer,
        inner,
        base: normal(tracks[&outer].start, h),
        start,
        end,
        spacing: run.distance,
        width,
        twin_base: None,
    }
}

/// The widest member of a run.
fn run_width(tracks: &BTreeMap<usize, TrackSegment>, run: &UnitRun) -> i64 {
    tracks[&run.first].width_nm.max(run.second.map_or(0, |second| tracks[&second].width_nm))
}

/// `_slots`: every candidate bump slot of the unit, on both sides of each straight run.
///
/// A run whose members lie wider apart than `spacing_nm` takes twin bumps:
/// both members bulge apart over the same interval, each as a single net.
pub(crate) fn slots(tracks: &BTreeMap<usize, TrackSegment>, runs: &[UnitRun], spacing_nm: i64) -> Vec<Slot> {
    let mut slots = Vec::new();
    for run in runs {
        let h = run.horizontal;
        let width = run_width(tracks, run);
        let pitch = 3 * width;
        let twin = run.second.is_some() && run.distance > spacing_nm + LANE_TOLERANCE_NM;
        let bump = if twin { pitch } else { pitch + 2 * run.distance };
        let usable = run.high - run.low - 2 * pitch;
        let count = if usable >= bump { (usable + pitch).div_euclid(bump + pitch) } else { 0 };
        let offset = run.low + pitch + (usable - count * bump - (count - 1) * pitch).div_euclid(2);
        for position in 0..count {
            let start = offset + position * (bump + pitch);
            if twin {
                let second = run.second.expect("a twin run is a pair's");
                let (outer, inner) = if normal(tracks[&run.first].start, h) >= normal(tracks[&second].start, h) {
                    (run.first, second)
                } else {
                    (second, run.first)
                };
                slots.push(Slot {
                    run: (run.first, run.second),
                    position,
                    layer: tracks[&run.first].layer,
                    horizontal: h,
                    side: 1,
                    outer,
                    inner: Some(inner),
                    base: normal(tracks[&outer].start, h),
                    start,
                    end: start + bump,
                    spacing: 0,
                    width,
                    twin_base: Some(normal(tracks[&inner].start, h)),
                });
                continue;
            }
            for side in [1, -1] {
                slots.push(run_slot(tracks, run, position, side, start, start + bump, width));
            }
        }
    }
    slots
}

/// `_windows`: the serpentine windows of one run (plan R10).
///
/// Legs are a pitch of the lane width plus the leg gap apart, on a grid
/// centred in the run 3 x width from its ends. For each phase a window is a
/// maximal stretch of at least two tops with room on their sides.
#[allow(clippy::too_many_arguments)]
fn unit_windows(
    board: &PhysicalBoard,
    obstacles: &RoutingClearanceIndex<'_>,
    tracks: &BTreeMap<usize, TrackSegment>,
    nets: &[String],
    run: &UnitRun,
    limit: i64,
    gap: Option<i64>,
    clearance_nm: i64,
) -> Result<Vec<Window>> {
    let width = run_width(tracks, run);
    let distance = run.distance;
    let gap = gap.unwrap_or_else(|| (3 * width).max(clearance_nm));
    let pitch = distance + width + gap;
    let usable = run.high - run.low - 6 * width;
    let count = (usable - distance).div_euclid(pitch);
    if count < 2 {
        return Ok(Vec::new());
    }
    let offset = run.low + 3 * width + (usable - count * pitch - distance).div_euclid(2);
    let floor = 4.max(chamfer_offset(distance) + 2);
    let chamfers = (width, 1 + chamfer_offset(distance));
    let mut tops: IndexMap<(i64, i64), Slot> = IndexMap::new();
    let mut rooms: IndexMap<(i64, i64), i64> = IndexMap::new();
    for position in 0..count {
        for side in [1, -1] {
            let slot = run_slot(
                tracks,
                run,
                position,
                side,
                offset + position * pitch,
                offset + (position + 1) * pitch + distance,
                width,
            );
            rooms.insert((position, side), room(board, obstacles, nets, &slot, limit, floor, Some(chamfers))?);
            tops.insert((position, side), slot);
        }
    }
    let mut windows = Vec::new();
    for phase in [1, -1] {
        let sides: Vec<i64> = (0..count).map(|position| if position % 2 == 0 { phase } else { -phase }).collect();
        let mut position = 0;
        while position < count {
            let mut end = position;
            while end < count && rooms[&(end, sides[end as usize])] > 0 {
                end += 1;
            }
            if end - position >= 2 {
                windows.push(Window {
                    tops: (position..end).map(|index| tops[&(index, sides[index as usize])]).collect(),
                    rooms: (position..end).map(|index| rooms[&(index, sides[index as usize])]).collect(),
                    pitch,
                    gap,
                });
            }
            position = end + 1;
        }
    }
    Ok(windows)
}

/// `_cut`.
fn cut(window: &Window, first: usize, count: usize) -> Window {
    Window {
        tops: window.tops[first..first + count].to_vec(),
        rooms: window.rooms[first..first + count].to_vec(),
        pitch: window.pitch,
        gap: window.gap,
    }
}

/// `_span`: a slot over the whole window, for its distance from the terminals.
fn span(window: &Window) -> Slot {
    Slot { end: window.tops[window.tops.len() - 1].end, ..window.tops[0] }
}

/// `_parts`: `heights` split into each feature's tops (bump heights follow).
pub(crate) fn parts(sizes: impl IntoIterator<Item = usize>, heights: &[i64]) -> Vec<Vec<i64>> {
    let mut parts = Vec::new();
    let mut used = 0;
    for size in sizes {
        let end = (used + size).min(heights.len());
        parts.push(heights[used.min(end)..end].to_vec());
        used += size;
    }
    parts
}

/// `_leg_loss`: length each member loses at the two corners of one serpentine leg.
///
/// The member is inside one corner (chamfer leg c) and outside the other
/// (c + d); each 45-degree piece replaces 2k of straight track with
/// round(k sqrt 2).
pub(crate) fn leg_loss(chamfer: i64, spacing: i64) -> i64 {
    [chamfer, chamfer + chamfer_offset(spacing)].iter().map(|&leg| 2 * leg - diagonal(leg)).sum()
}

/// The legs of a feature of `heights`: half legs as long as their top's
/// height, the others the two adjacent heights together.
pub(crate) fn feature_legs(heights: &[i64]) -> Vec<i64> {
    let mut legs = vec![heights[0]];
    legs.extend(heights.windows(2).map(|pair| pair[0] + pair[1]));
    legs.push(heights[heights.len() - 1]);
    legs
}

/// `_window_chamfer`: the largest chamfer leg every leg of the window allows at `heights`.
///
/// R9's rule for a leg of length L: at most the width, L // 4 and
/// (L - d) // 2; and at most (w + gap - 1) // 2, so the inner member's top
/// keeps a straight piece. Below 1, no serpentine fits.
fn window_chamfer(window: &Window, heights: &[i64]) -> i64 {
    let (width, offset) = (window.tops[0].width, chamfer_offset(window.tops[0].spacing));
    feature_legs(heights)
        .into_iter()
        .map(|leg| leg.div_euclid(4).min((leg - offset).div_euclid(2)))
        .fold(width.min((width + window.gap - 1).div_euclid(2)), i64::min)
}

/// `_window_capacity`: length a serpentine of `heights` adds to each member (0 if it has no corners).
fn window_capacity(window: &Window, heights: &[i64]) -> i64 {
    let chamfer = window_chamfer(window, heights);
    if chamfer < 1 {
        return 0;
    }
    2 * heights.iter().sum::<i64>() - (heights.len() as i64 + 1) * leg_loss(chamfer, window.tops[0].spacing)
}

/// The bounding box of several bounds.
pub(crate) fn union(areas: impl IntoIterator<Item = Bounds>) -> Bounds {
    areas
        .into_iter()
        .reduce(|a, b| Bounds {
            min_x: a.min_x.min(b.min_x),
            min_y: a.min_y.min(b.min_y),
            max_x: a.max_x.max(b.max_x),
            max_y: a.max_y.max(b.max_y),
        })
        .expect("a feature has at least one top")
}

/// `_window_bounds`: bounds of a window's tops at full room, on both sides of its run.
fn window_bounds(window: &Window) -> Bounds {
    union(window.tops.iter().zip(&window.rooms).map(|(top, &room)| swept(top, room, None).bounds))
}

/// `_near`.
pub(crate) fn near(first: Bounds, second: Bounds, margin: i64) -> bool {
    first.expanded(margin).intersects(second)
}

/// `serpentine_path`: centre-line corners `(axial, offset)` of one member's serpentine, in axial order.
///
/// The pair's legs are `pitch` apart from `first_leg`; top i lies toward
/// `sides[i]`, `heights[i]` off each member's own lane. `member` is +1 for
/// the member whose lane lies toward +1 (or a single net, with `spacing` 0),
/// else -1. Inside corners take `chamfer` and outside ones `chamfer` + d.
pub fn serpentine_path(
    first_leg: i64,
    pitch: i64,
    spacing: i64,
    sides: &[i64],
    heights: &[i64],
    member: i64,
    chamfer: i64,
) -> Vec<(i64, i64)> {
    let offset = chamfer_offset(spacing);
    let mut rises = vec![0];
    rises.extend(sides.iter().zip(heights).map(|(side, height)| side * height));
    rises.push(0);
    let mut corners = Vec::new();
    for leg in 0..=heights.len() {
        let (before, after) = (rises[leg], rises[leg + 1]);
        let direction = if after > before { 1 } else { -1 };
        let along = first_leg + leg as i64 * pitch + if member == direction { 0 } else { spacing };
        let (entry, leave) =
            if member == direction { (chamfer, chamfer + offset) } else { (chamfer + offset, chamfer) };
        corners.extend([
            (along - entry, before),
            (along, before + direction * entry),
            (along, after - direction * leave),
            (along + leave, after),
        ]);
    }
    corners
}

/// `_add_serpentine`: add a window's serpentine at `heights` to each member of its run.
fn add_serpentine(
    features: &mut Features,
    tracks: &BTreeMap<usize, TrackSegment>,
    window: &Window,
    heights: &[i64],
    chamfer: i64,
) {
    let (first, second) = window.tops[0].run;
    let sides: Vec<i64> = window.tops.iter().map(|top| top.side).collect();
    let h = window.tops[0].horizontal;
    for (index, other) in [(Some(first), second), (second, Some(first))] {
        let Some(index) = index else { continue };
        let member = match other {
            None => 1,
            Some(other) if normal(tracks[&index].start, h) > normal(tracks[&other].start, h) => 1,
            Some(_) => -1,
        };
        features.entry(index).or_default().push(serpentine_path(
            window.tops[0].start,
            window.pitch,
            window.tops[0].spacing,
            &sides,
            heights,
            member,
            chamfer,
        ));
    }
}

/// A rounded convex shape whose spine is known to be non-empty.
pub(crate) fn rounded(spine: Vec<Point>, radius_nm: i64) -> RoundedConvexShape {
    RoundedConvexShape::new(spine, radius_nm.max(0)).expect("tuning outlines have a spine")
}

/// `_swept`: area covered by a bump's copper (every member) of `height`.
///
/// It is the convex outline of the bulge-side member's chamfered bump: its
/// run chamfers are bridged by the straight edge from the foot to the leg's
/// top end. `chamfers` (base, top) replace the bump's own, as for a
/// serpentine top.
fn swept(slot: &Slot, height: i64, chamfers: Option<(i64, i64)>) -> RoundedConvexShape {
    let (base, top) = chamfers.unwrap_or_else(|| bump_chamfers(height, slot.width, slot.spacing));
    let mut corners = bump_path(slot.start, slot.end, height, base, top);
    if corners.len() == 8 {
        corners = [&corners[..1], &corners[2..6], &corners[7..]].concat();
    }
    let point = |(axial, rise): (i64, i64)| {
        let normal = slot.base + slot.side * rise;
        if slot.horizontal { Point::new(axial, normal) } else { Point::new(normal, axial) }
    };
    rounded(corners.into_iter().map(point).collect(), slot.width.div_euclid(2))
}

/// `_swept_areas`: `_swept`, plus the inner member's own bulge for a twin bump.
fn swept_areas(slot: &Slot, height: i64, chamfers: Option<(i64, i64)>) -> Vec<RoundedConvexShape> {
    let Some(twin_base) = slot.twin_base else { return vec![swept(slot, height, chamfers)] };
    let inner = Slot { base: twin_base, side: -slot.side, twin_base: None, ..*slot };
    vec![swept(slot, height, chamfers), swept(&inner, height, chamfers)]
}

/// `_slot_bounds`.
fn slot_bounds(slot: &Slot, height: i64) -> Bounds {
    union(swept_areas(slot, height, None).into_iter().map(|shape| shape.bounds))
}

/// The shape clears the board edge, holes and every net's foreign copper on `layer`.
pub(crate) fn area_clear(
    board: &PhysicalBoard,
    obstacles: &RoutingClearanceIndex<'_>,
    nets: &[String],
    shape: &RoundedConvexShape,
    layer: CopperLayer,
) -> Result<bool> {
    // Each member is checked as its own net, so the other member's lands
    // count as foreign copper too.
    if !shape_in_board(board, shape, board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm)? {
        return Ok(false);
    }
    for net in nets {
        if !obstacles.can_area(net, shape, layer)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The tallest height in `floor..=limit` for which `clear` holds, else 0 (the swept area only grows).
pub(crate) fn tallest(floor: i64, limit: i64, mut clear: impl FnMut(i64) -> Result<bool>) -> Result<i64> {
    if floor > limit || !clear(floor)? {
        return Ok(0);
    }
    if clear(limit)? {
        return Ok(limit);
    }
    let (mut low, mut high) = (floor, limit);
    while high - low > 1 {
        let middle = (low + high).div_euclid(2);
        if clear(middle)? {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

/// `_room`: tallest clear bump height in `floor..limit`, else 0.
fn room(
    board: &PhysicalBoard,
    obstacles: &RoutingClearanceIndex<'_>,
    nets: &[String],
    slot: &Slot,
    limit: i64,
    floor: i64,
    chamfers: Option<(i64, i64)>,
) -> Result<i64> {
    tallest(floor, limit, |height| {
        for shape in swept_areas(slot, height, chamfers) {
            if !area_clear(board, obstacles, nets, &shape, slot.layer)? {
                return Ok(false);
            }
        }
        Ok(true)
    })
}

/// `_conflict`: two slots cannot both be used.
///
/// Slots of one run are spaced by construction; only one side of each grid
/// position can be used. Slots of different runs must keep their swept
/// areas (at full room) apart by the clearance.
fn conflict(slot: &Slot, other: &Slot, room: i64, other_room: i64, clearance_nm: i64) -> bool {
    if slot.run == other.run {
        return slot.position == other.position;
    }
    if slot.layer != other.layer {
        return false;
    }
    near(
        slot_bounds(slot, room),
        slot_bounds(other, other_room),
        slot.width.div_euclid(2) + other.width.div_euclid(2) + clearance_nm,
    )
}

/// `_terminal_ends`: track endpoints used once per net, the unit's terminals and stubs.
pub(crate) fn terminal_ends(tracks: &[TrackSegment]) -> Vec<Point> {
    let mut counts: IndexMap<(&str, Point), i64> = IndexMap::new();
    for track in tracks {
        for point in [track.start, track.end] {
            *counts.entry((track.net.as_str(), point)).or_insert(0) += 1;
        }
    }
    let ends: std::collections::BTreeSet<Point> =
        counts.into_iter().filter(|(_, count)| *count == 1).map(|((_, point), _)| point).collect();
    ends.into_iter().collect()
}

/// `_levelled`: heights within each room whose gains sum to `total`, as level as possible.
///
/// `gain(i, height)` is the length bump `i` adds at `height`; one nanometre
/// more height changes it by -2, 0 or +2. The lowest common level `L` whose
/// gains reach `total` is used; the excess is taken from the last slots at
/// that level whose gain drops by 2 one nanometre lower.
pub(crate) fn levelled(rooms: &[i64], total: i64, gain: &dyn Fn(usize, i64) -> i64) -> Vec<i64> {
    let reached =
        |level: i64| -> i64 { rooms.iter().enumerate().map(|(index, &room)| gain(index, room.min(level))).sum() };
    let (mut low, mut high) = (0, rooms.iter().copied().max().unwrap_or(0));
    while high - low > 1 {
        let middle = (low + high).div_euclid(2);
        if reached(middle) < total {
            low = middle;
        } else {
            high = middle;
        }
    }
    let level = high;
    let mut heights: Vec<i64> = rooms.iter().map(|&room| room.min(level)).collect();
    let mut excess = reached(level) - total;
    for index in (0..heights.len()).rev() {
        if excess < 2 {
            break;
        }
        if heights[index] == level && gain(index, level) - gain(index, level - 1) == 2 {
            heights[index] -= 1;
            excess -= 2;
        }
    }
    heights
}

/// `_bumped`: replace one axis-aligned segment with chamfered bumps or serpentines.
///
/// Each feature is its centre-line corners `(axial, offset)` in axial order,
/// `offset` along the normal from the segment's own line. Features are
/// spliced in travel order.
pub(crate) fn bumped(track: &TrackSegment, features: &[Vec<(i64, i64)>]) -> Result<Vec<TrackSegment>> {
    let Some(h) = horizontal(track) else {
        return Err(Error::Value("tuning features need an axis-aligned segment".to_owned()));
    };
    let base = normal(track.start, h);
    let sign = if axial(track.end, h) > axial(track.start, h) { 1 } else { -1 };
    let point = |(along, offset): (i64, i64)| {
        if h { Point::new(along, base + offset) } else { Point::new(base + offset, along) }
    };
    let mut ordered: Vec<&Vec<(i64, i64)>> = features.iter().collect();
    ordered.sort_by_key(|corners| sign * corners[0].0);
    let mut points = vec![track.start];
    for corners in ordered {
        if sign > 0 {
            points.extend(corners.iter().copied().map(point));
        } else {
            points.extend(corners.iter().rev().copied().map(point));
        }
    }
    points.push(track.end);
    Ok(points
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .map(|pair| TrackSegment {
            net: track.net.clone(),
            start: pair[0],
            end: pair[1],
            width_nm: track.width_nm,
            layer: track.layer,
        })
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Expected values come from running `pcbir.critical_tuning` on the same inputs.

    pub(crate) fn track(net: &str, start: (i64, i64), end: (i64, i64), width: i64) -> TrackSegment {
        TrackSegment {
            net: net.to_owned(),
            start: Point::new(start.0, start.1),
            end: Point::new(end.0, end.1),
            width_nm: width,
            layer: CopperLayer::Front,
        }
    }

    fn sample() -> Vec<TrackSegment> {
        vec![
            track("A", (0, 0), (5_000_000, 0), 150_000),
            track("A", (5_000_000, 0), (9_000_000, 0), 150_000),
            track("A", (9_000_000, 0), (10_000_000, 1_000_000), 150_000),
            track("B", (500_000, 300_000), (9_500_000, 300_000), 150_000),
            track("B", (9_500_000, 300_000), (9_500_000, 900_000), 150_000),
        ]
    }

    #[test]
    fn chamfers_and_gains() {
        let cases = [(1_000_000, 0), (1_000_000, 200_000), (3, 0), (10, 200_000), (60_000, 200_000), (2, 0)];
        let got: Vec<(i64, i64)> = cases.iter().map(|&(h, s)| bump_chamfers(h, 150_000, s)).collect();
        assert_eq!(got, [(150000, 150000), (150000, 267157), (2, 1), (-58573, 58583), (-28578, 88578), (1, 1)]);
        let gain = |h: i64, s: i64| {
            let (base, top) = bump_chamfers(h, 150_000, s);
            bump_gain(h, base, top)
        };
        assert_eq!(
            (gain(1_000_000, 0), gain(1_000_000, 200_000), gain(3, 0), gain(10, 200_000)),
            (1648528, 1511270, 2, 8)
        );
        assert_eq!([0, 1, 100, 200_000, 123_457].map(chamfer_offset), [0, 0, 58, 117157, 72319]);
        assert_eq!([(1, 0), (50, 0), (150_000, 200_000), (7, 13)].map(|(c, s)| leg_loss(c, s)), [2, 58, 244365, 12]);
    }

    #[test]
    fn paths() {
        assert_eq!(
            bump_path(0, 1000, 400, 100, 180),
            [(-100, 0), (0, 100), (0, 220), (180, 400), (820, 400), (1000, 220), (1000, 100), (1100, 0)]
        );
        assert_eq!(bump_path(1000, 0, 10, -2, 12), [(998, 0), (988, 10), (12, 10), (2, 0)]);
        assert_eq!(
            serpentine_path(100, 900, 200, &[1, -1, 1], &[500, 600, 400], -1, 50),
            [
                (133, 0),
                (300, 167),
                (300, 450),
                (350, 500),
                (950, 500),
                (1000, 450),
                (1000, -433),
                (1167, -600),
                (1933, -600),
                (2100, -433),
                (2100, 350),
                (2150, 400),
                (2750, 400),
                (2800, 350),
                (2800, 167),
                (2967, 0)
            ]
        );
        assert_eq!(
            serpentine_path(0, 700, 0, &[-1, 1], &[300, 300], 1, 40),
            [
                (-40, 0),
                (0, -40),
                (0, -260),
                (40, -300),
                (660, -300),
                (700, -260),
                (700, 260),
                (740, 300),
                (1360, 300),
                (1400, 260),
                (1400, 40),
                (1440, 0)
            ]
        );
    }

    #[test]
    fn levelled_heights() {
        assert_eq!(levelled(&[500, 800, 300], 2400, &|_, h| double(h)), [450, 450, 300]);
        assert_eq!(levelled(&[500, 800, 300], 2401, &|_, h| double(h)), [451, 450, 300]);
    }

    #[test]
    fn lines_runs_and_slots() {
        let tracks = sample();
        let lines = straight_lines(&tracks, &[]);
        let summary: Vec<(usize, Vec<usize>, Point, Point)> =
            lines.iter().map(|(lead, (indices, line))| (*lead, indices.clone(), line.start, line.end)).collect();
        assert_eq!(
            summary,
            [
                (0, vec![0, 1], Point::new(0, 0), Point::new(9_000_000, 0)),
                (3, vec![3], Point::new(500_000, 300_000), Point::new(9_500_000, 300_000)),
                (4, vec![4], Point::new(9_500_000, 300_000), Point::new(9_500_000, 900_000)),
            ]
        );
        let via = Via {
            net: "A".to_owned(),
            position: Point::new(5_000_000, 0),
            size_nm: 400_000,
            drill_nm: 200_000,
            from_layer: CopperLayer::Front,
            to_layer: CopperLayer::Back,
            technology: None,
            finish: "standard".to_owned(),
        };
        assert_eq!(straight_lines(&tracks, &[via]).keys().copied().collect::<Vec<_>>(), [0, 1, 3, 4]);
        let segments: BTreeMap<usize, TrackSegment> =
            lines.iter().map(|(lead, (_, line))| (*lead, line.clone())).collect();
        let nets = ["A".to_owned(), "B".to_owned()];
        let runs = unit_runs(&segments, &nets, 300_000);
        assert_eq!(
            runs,
            [UnitRun { first: 0, second: Some(3), horizontal: true, low: 500_000, high: 9_000_000, distance: 300_000 }]
        );
        let found = slots(&segments, &runs, 300_000);
        assert_eq!(found.len(), 10);
        let brief =
            |s: &Slot| (s.position, s.side, s.outer, s.inner, s.base, s.start, s.end, s.spacing, s.width, s.twin_base);
        assert_eq!(
            found[..3].iter().map(brief).collect::<Vec<_>>(),
            [
                (0, 1, 3, Some(0), 300000, 1225000, 2275000, 300000, 150000, None),
                (0, -1, 0, Some(3), 0, 1225000, 2275000, 300000, 150000, None),
                (1, 1, 3, Some(0), 300000, 2725000, 3775000, 300000, 150000, None),
            ]
        );
        // Wider than the spacing: twin bumps.
        let twins = slots(&segments, &unit_runs(&segments, &nets, 150_000), 150_000);
        assert_eq!(twins.len(), 8);
        assert_eq!(
            twins[..2].iter().map(brief).collect::<Vec<_>>(),
            [
                (0, 1, 3, Some(0), 300000, 1375000, 1825000, 0, 150000, Some(0)),
                (1, 1, 3, Some(0), 300000, 2275000, 2725000, 0, 150000, Some(0)),
            ]
        );
        let pieces: BTreeMap<usize, TrackSegment> = tracks.iter().cloned().enumerate().collect();
        let single = unit_runs(&pieces, &nets[..1], 0);
        assert_eq!(
            single,
            [
                UnitRun { first: 0, second: None, horizontal: true, low: 0, high: 5_000_000, distance: 0 },
                UnitRun { first: 1, second: None, horizontal: true, low: 5_000_000, high: 9_000_000, distance: 0 },
            ]
        );
        let found = slots(&pieces, &single, 0);
        assert_eq!(found.len(), 16);
        assert_eq!((found[0].run, found[0].side, found[0].start, found[0].end), ((0, None), 1, 475000, 925000));
        assert_eq!(found[1].side, -1);
        assert_eq!(
            terminal_ends(&tracks),
            [
                Point::new(0, 0),
                Point::new(500_000, 300_000),
                Point::new(9_500_000, 900_000),
                Point::new(10_000_000, 1_000_000)
            ]
        );
    }

    #[test]
    fn bumped_segments() {
        let tracks = sample();
        let features = [bump_path(3_000_000, 3_600_000, 500, 150, 150), bump_path(1_000_000, 1_600_000, 400, 100, 100)];
        let points: Vec<(i64, i64)> =
            bumped(&tracks[0], &features).unwrap().iter().map(|t| (t.start.x_nm, t.start.y_nm)).collect();
        assert_eq!(
            points,
            [
                (0, 0),
                (999900, 0),
                (1000000, 100),
                (1000000, 300),
                (1000100, 400),
                (1599900, 400),
                (1600000, 300),
                (1600000, 100),
                (1600100, 0),
                (2999850, 0),
                (3000000, 150),
                (3000000, 350),
                (3000150, 500),
                (3599850, 500),
                (3600000, 350),
                (3600000, 150),
                (3600150, 0)
            ]
        );
        let down = track("A", (0, 5000), (0, 0), 100);
        let pieces = bumped(&down, &[vec![(4000, 0), (3500, 20), (3000, 0)]]).unwrap();
        let ends: Vec<((i64, i64), (i64, i64))> =
            pieces.iter().map(|t| ((t.start.x_nm, t.start.y_nm), (t.end.x_nm, t.end.y_nm))).collect();
        assert_eq!(
            ends,
            [((0, 5000), (0, 3000)), ((0, 3000), (20, 3500)), ((20, 3500), (0, 4000)), ((0, 4000), (0, 0))]
        );
    }

    #[test]
    fn report_line_and_document() {
        let item = MatchTuningResult {
            id: "dsi".to_owned(),
            max_skew_nm: 100_000,
            status: "tuned".to_owned(),
            skew_before_nm: Some(1_234_567),
            skew_after_nm: Some(2),
            members: vec![
                MatchTuningMember { net: "A".to_owned(), length_before_nm: 10, length_after_nm: 1_234_577 },
                MatchTuningMember { net: "B".to_owned(), length_before_nm: 5, length_after_nm: 5 },
            ],
            bumps: 3,
            reason: String::new(),
            units: vec![MatchTuningUnit {
                nets: vec!["A".to_owned()],
                stage: "routing".to_owned(),
                style: "serpentine".to_owned(),
                added_length_nm: 1_234_567,
                bumps: 1,
                legs: 4,
                amplitudes_nm: vec![300_000, 450_500],
                group: String::new(),
                lanes: Vec::new(),
            }],
            tuning_groups: vec![TuningGroupOutcome {
                name: "g".to_owned(),
                nets: vec!["A".to_owned()],
                status: "failed".to_owned(),
                reason: "no room".to_owned(),
            }],
        };
        assert_eq!(
            match_tuning_line(&item),
            "match group dsi: tuned, skew 1.235 -> 0.000 mm (max 0.100 mm); added A +1.235 mm; A serpentine 4 legs \
             1 bumps up to 0.451 mm while routing; tuning group g failed (no room)"
        );
        let Value::Object(document) = match_tuning_document(&item) else { panic!("an object") };
        assert_eq!(
            document.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "id",
                "max_skew_nm",
                "status",
                "skew_before_nm",
                "skew_after_nm",
                "members",
                "bumps",
                "reason",
                "legs",
                "units",
                "tuning_groups"
            ]
        );
    }

    /// `tests/test_route_cleanup.py::board` plus net X: one-pad J1 and J2 joined by net S.
    pub(crate) fn board() -> PhysicalBoard {
        use copper_pcb::board::{PadReference, PhysicalNet, Placement, Stackup};
        use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
        use copper_physical::{BoardOutline, DesignRules};
        use copper_units::Decimal;
        use indexmap::IndexMap;
        const MM: i64 = 1_000_000;
        let pad = FootprintPad {
            number: "1".to_owned(),
            position: Point { x_nm: 0, y_nm: 0 },
            size: Size { width_nm: 600_000, height_nm: 600_000 },
            kind: PadKind::Smd,
            shape: PadShape::Roundrect,
            rotation_degrees: Decimal::from_i64(0),
            drill: None,
            roundrect_ratio_ppm: 250_000,
            has_solder_mask: true,
            has_solder_paste: true,
            zone_connection: None,
            heatsink: false,
            remove_unused_layers: false,
            connector_contact: false,
        };
        let footprint = PhysicalFootprint {
            name: "one-pad".to_owned(),
            pads: vec![pad],
            body_size: Size { width_nm: MM, height_nm: MM },
            source_library_id: None,
            graphics: Vec::new(),
            keepouts: Vec::new(),
            metadata: IndexMap::new(),
            courtyard: Vec::new(),
            height_nm: None,
            clearance_nm: None,
            exclude_from_bom: false,
            exclude_from_pos_files: false,
            internal_pad_groups: Vec::new(),
        };
        let zero = Decimal::from_i64(0);
        PhysicalBoard {
            name: "cleanup".to_owned(),
            outline: BoardOutline::rectangle(20 * MM, 12 * MM, None).expect("outline"),
            footprints: IndexMap::from([(footprint.name.clone(), footprint)]),
            placements: vec![
                Placement::new("J1", "one-pad", Point::new(3 * MM, 6 * MM), &zero).expect("J1"),
                Placement::new("J2", "one-pad", Point::new(17 * MM, 6 * MM), &zero).expect("J2"),
            ],
            nets: vec![
                PhysicalNet {
                    name: "S".to_owned(),
                    pads: vec![PadReference::new("J1", "1"), PadReference::new("J2", "1")],
                },
                PhysicalNet { name: "X".to_owned(), pads: Vec::new() },
            ],
            stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
            rules: DesignRules::default(),
            tracks: Vec::new(),
            vias: Vec::new(),
            metadata: IndexMap::new(),
            regions: Vec::new(),
            keepouts: Vec::new(),
            placement_rules: Vec::new(),
            relative_rules: Vec::new(),
            placement_groups: Vec::new(),
            power_domains: Vec::new(),
            net_routing_rules: Vec::new(),
            zones: Vec::new(),
            polygons: Vec::new(),
            copper_keepouts: Vec::new(),
            rigid_clusters: Vec::new(),
            hard_macros: Vec::new(),
            materialized_macros: Vec::new(),
            via_in_pad_rules: Vec::new(),
            mechanical_holes: Vec::new(),
            mechanical_slots: Vec::new(),
            datums: Vec::new(),
            boundary_edges: Vec::new(),
            attachments: Vec::new(),
            component_heights: Vec::new(),
            body_overhangs: Vec::new(),
            assembly_envelopes: Vec::new(),
            assembly_access: Vec::new(),
            match_groups: Vec::new(),
            component_hole_clearances: Vec::new(),
            decoupling_links: Vec::new(),
            silkscreen: Vec::new(),
            power_feeds: Vec::new(),
            mechanical_references: Vec::new(),
        }
    }

    #[test]
    fn unit_tuner_end_to_end() {
        // Expected plans from `UnitTuner` on the same board in Python.
        let board = board();
        let obstacle = track("X", (8_000_000, 7_000_000), (12_000_000, 7_000_000), 200_000);
        let obstacles_board = PhysicalBoard { tracks: vec![obstacle], ..board.clone() };
        let index = RoutingClearanceIndex::new(&obstacles_board).expect("index");
        let tracks = vec![
            track("S", (3_000_000, 6_000_000), (10_000_000, 6_000_000), 200_000),
            track("S", (10_000_000, 6_000_000), (17_000_000, 6_000_000), 200_000),
        ];
        let checksum = |tracks: &[TrackSegment]| -> i64 {
            tracks.iter().map(|t| t.start.x_nm * 3 + t.start.y_nm * 7 + t.end.x_nm * 11 + t.end.y_nm * 13).sum()
        };
        type Expected = (i64, i64, i64, Vec<i64>, &'static str, Option<(usize, i64)>);
        let expected: [(TuningStyle, Vec<Expected>); 2] = [
            (
                TuningStyle::Bumps,
                vec![
                    (1_000_000, 1, 0, vec![707106], "", Some((10, 2476231520))),
                    (3_000_000, 1, 0, vec![1734314], "", Some((10, 2394054880))),
                    (6_000_001, 2, 0, vec![1734315, 1734314], "", Some((18, 4458509680))),
                    (40_000_000, 0, 0, vec![], "insufficient tuning room: 35313720 of 40000000 nm in 10 bump(s)", None),
                ],
            ),
            (
                TuningStyle::Serpentine,
                vec![
                    (1_000_000, 0, 3, vec![320377, 320377], "", Some((13, 2898400000))),
                    (3_000_000, 0, 3, vec![925736, 925735], "", Some((13, 2898400080))),
                    (6_000_001, 2, 0, vec![1734315, 1734314], "", Some((17, 4064109680))),
                    (
                        40_000_000,
                        0,
                        15,
                        vec![
                            1899864, 1899864, 1899863, 1899863, 600000, 1899863, 600000, 1899863, 600000, 1899863,
                            958723, 1899863, 1899863, 1899863,
                        ],
                        "",
                        Some((61, 15528741680)),
                    ),
                ],
            ),
        ];
        for (style, cases) in expected {
            let options = TunerOptions { style, ..TunerOptions::default() };
            let tuner =
                UnitTuner::new(&board, &index, tracks.clone(), &["S".to_owned()], 0, 2_000_000, 200_000, options)
                    .expect("tuner");
            for (target, bumps, legs, amplitudes, reason, copper) in cases {
                let plan = tuner.plan(target).expect("plan");
                assert_eq!(
                    (plan.bumps, plan.legs, plan.amplitudes_nm.clone(), plan.reason.as_str()),
                    (bumps, legs, amplitudes, reason),
                    "{} {target}",
                    style.value()
                );
                assert_eq!(
                    plan.tracks.as_deref().map(|t| (t.len(), checksum(t))),
                    copper,
                    "{} {target}",
                    style.value()
                );
            }
        }
    }

    #[test]
    fn pair_tuner_end_to_end() {
        // Expected plans from `UnitTuner` on the same board in Python: a pair at
        // its spacing (bumps, serpentines) and a wide pair (twin bumps).
        let board = board();
        let obstacle = track("X", (8_000_000, 7_000_000), (12_000_000, 7_000_000), 200_000);
        let obstacles_board = PhysicalBoard { tracks: vec![obstacle], ..board.clone() };
        let index = RoutingClearanceIndex::new(&obstacles_board).expect("index");
        let tracks = vec![
            track("S", (3_000_000, 6_000_000), (10_000_000, 6_000_000), 200_000),
            track("S", (10_000_000, 6_000_000), (17_000_000, 6_000_000), 200_000),
            track("N", (4_000_000, 6_300_000), (16_000_000, 6_300_000), 200_000),
        ];
        let checksum = |tracks: &[TrackSegment]| -> i64 {
            tracks.iter().map(|t| t.start.x_nm * 3 + t.start.y_nm * 7 + t.end.x_nm * 11 + t.end.y_nm * 13).sum()
        };
        let expected = [
            (300_000, TuningStyle::Bumps, 1_000_000, 1, 0, vec![837257], String::new(), Some((19, 4695238880))),
            (300_000, TuningStyle::Bumps, 3_000_000, 1, 0, vec![1837257], String::new(), Some((19, 4535238880))),
            (
                300_000,
                TuningStyle::Bumps,
                40_000_000,
                0,
                0,
                vec![],
                "insufficient tuning room: 19952916 of 40000000 nm in 6 bump(s)".to_owned(),
                None,
            ),
            (300_000, TuningStyle::Serpentine, 1_000_000, 0, 3, vec![419319, 419318], String::new(), Some((26, 7865999840))),
            (300_000, TuningStyle::Serpentine, 3_000_000, 0, 3, vec![1002939, 1002939], String::new(), Some((26, 7866000000))),
            (
                300_000,
                TuningStyle::Serpentine,
                40_000_000,
                0,
                0,
                vec![],
                "insufficient tuning room: 19952916 of 40000000 nm in 6 bump(s); serpentine: insufficient tuning room: \
                 25827430 of 40000000 nm in 10 leg(s)"
                    .to_owned(),
                None,
            ),
            (150_000, TuningStyle::Bumps, 1_000_000, 1, 0, vec![707106], String::new(), Some((19, 4291600000))),
            (150_000, TuningStyle::Bumps, 3_000_000, 1, 0, vec![1734314], String::new(), Some((19, 4291600000))),
            (
                150_000,
                TuningStyle::Bumps,
                40_000_000,
                0,
                0,
                vec![],
                "insufficient tuning room: 15953108 of 40000000 nm in 8 bump(s)".to_owned(),
                None,
            ),
        ];
        let nets = ["S".to_owned(), "N".to_owned()];
        for (spacing, style, target, bumps, legs, amplitudes, reason, copper) in expected {
            let options = TunerOptions { style, ..TunerOptions::default() };
            let tuner = UnitTuner::new(&board, &index, tracks.clone(), &nets, spacing, 2_000_000, 200_000, options)
                .expect("tuner");
            let plan = tuner.plan(target).expect("plan");
            assert_eq!(
                (plan.bumps, plan.legs, plan.amplitudes_nm.clone(), plan.reason.clone()),
                (bumps, legs, amplitudes, reason),
                "{spacing} {} {target}",
                style.value()
            );
            assert_eq!(
                plan.tracks.as_deref().map(|t| (t.len(), checksum(t))),
                copper,
                "{spacing} {} {target}",
                style.value()
            );
        }
    }
}

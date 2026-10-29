//! Length-match tuning of accepted critical groups (`pcbir.critical`
//! `_tune_match_groups`, `_tune_bundle_pairs` and their units, plan R3, R12
//! and R14).

use std::collections::BTreeMap;

use copper_drc::breakout::BreakoutRegions;
use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, TrackSegment, TuningStyle, Via};
use copper_pcb::{Error, Result};
use indexmap::{IndexMap, IndexSet};

use crate::critical::{
    CriticalNetResult, board_with, budget_diagnostics, budget_warnings, coupled_length, is_pair_kind, rejection_gate,
    track_length, validate_candidate,
};
use crate::critical_group_tuning::GroupTuner;
use crate::critical_tuning::{
    MatchTuningMember, MatchTuningResult, MatchTuningUnit, TunerOptions, TuningGroupOutcome, UnitTuner, UnitTuning,
};
use crate::pair_tuning::BREAKOUT_TUNING_MARGIN_NM;
use crate::routing_clearance::RoutingClearanceIndex;

/// Copper accepted by the critical stage, per group in acceptance order.
pub(crate) type Committed = Vec<(Vec<String>, Vec<TrackSegment>, Vec<Via>)>;

/// The rules of the stage, by net.
pub(crate) type Rules<'a> = IndexMap<&'a str, &'a NetRoutingRule>;

/// What tuning while routing (plan R12) has done so far: the steps, the
/// members' lengths as routed, the target at which a unit last failed, each
/// tuned unit's copper and result as routed, and why each tuning group last
/// failed to tune.
#[derive(Clone, Default)]
pub(crate) struct RoutingTuning {
    pub early: Vec<MatchTuningUnit>,
    pub routed: IndexMap<String, i64>,
    pub failed_targets: IndexMap<Vec<String>, i64>,
    pub as_routed: IndexMap<Vec<String>, (Vec<TrackSegment>, CriticalNetResult)>,
    pub group_failures: IndexMap<String, String>,
}

fn net_length(copper: &[&TrackSegment], net: &str) -> i64 {
    let items: Vec<&TrackSegment> = copper.iter().copied().filter(|t| t.net == net).collect();
    track_length(&items)
}

fn copper_of<'c>(base_tracks: &'c [TrackSegment], committed: &'c Committed) -> Vec<&'c TrackSegment> {
    base_tracks.iter().chain(committed.iter().flat_map(|(_, items, _)| items.iter())).collect()
}

fn width_or_default(board: &PhysicalBoard, rule: &NetRoutingRule) -> i64 {
    rule.width_nm.filter(|width| *width != 0).unwrap_or(board.rules.default_track_width_nm)
}

fn disjoint(first: &[String], second: &[String]) -> bool {
    !first.iter().any(|net| second.contains(net))
}

/// Python's `dict.fromkeys((a, b))`: both targets, once each, in order.
fn targets(full: i64, least: i64) -> Vec<i64> {
    if full == least { vec![full] } else { vec![full, least] }
}

/// `_tuning_groups`: the units of each tuning group among `nets`, by name.
pub(crate) fn tuning_groups(rules: &Rules, nets: &[String]) -> Vec<(String, Vec<Vec<String>>)> {
    let mut groups: BTreeMap<String, IndexSet<Vec<String>>> = BTreeMap::new();
    for net in nets {
        let Some(rule) = rules.get(net.as_str()) else { continue };
        let Some(name) = &rule.tuning_group else { continue };
        let unit = match (&rule.differential_partner, is_pair_kind(rule.kind)) {
            (Some(partner), true) if !partner.is_empty() => {
                let mut unit = vec![net.clone(), partner.clone()];
                unit.sort();
                unit
            }
            _ => vec![net.clone()],
        };
        groups.entry(name.clone()).or_default().insert(unit);
    }
    groups
        .into_iter()
        .map(|(name, units)| {
            let mut units: Vec<Vec<String>> = units.into_iter().collect();
            units.sort();
            (name, units)
        })
        .collect()
}

/// `_group_outcomes`: each tuning group of `group`, tuned by a kept step,
/// else why it was not.
fn group_outcomes(
    rules: &Rules,
    group: &NetMatchGroup,
    steps: &[MatchTuningUnit],
    attempts: &IndexMap<String, TuningGroupOutcome>,
    failures: &IndexMap<String, String>,
    incomplete: bool,
) -> Vec<TuningGroupOutcome> {
    let mut outcomes = Vec::new();
    for (name, units) in tuning_groups(rules, &group.nets) {
        let nets: Vec<String> = units.into_iter().flatten().collect();
        let attempt = attempts.get(&name);
        let outcome = |status: &str, reason: String| TuningGroupOutcome {
            name: name.clone(),
            nets: nets.clone(),
            status: status.to_owned(),
            reason,
        };
        if steps.iter().any(|step| step.group == name) {
            outcomes.push(outcome("tuned", String::new()));
        } else if let Some(attempt) = attempt.filter(|attempt| attempt.status == "incomplete") {
            outcomes.push(attempt.clone());
        } else if let Some(failure) = failures.get(&name) {
            let reason = match attempt {
                Some(attempt) if attempt.status == "failed" && attempt.reason != *failure => {
                    format!("while routing: {failure}; final pass: {}", attempt.reason)
                }
                _ => failure.clone(),
            };
            outcomes.push(outcome("failed", reason));
        } else if let Some(attempt) = attempt {
            outcomes.push(attempt.clone());
        } else {
            outcomes.push(outcome(if incomplete { "incomplete" } else { "within_limit" }, String::new()));
        }
    }
    outcomes
}

fn members_of(
    group: &NetMatchGroup,
    before: &IndexMap<String, i64>,
    after: &IndexMap<String, i64>,
) -> Vec<MatchTuningMember> {
    group
        .nets
        .iter()
        .map(|net| MatchTuningMember {
            net: net.clone(),
            length_before_nm: before[net.as_str()],
            length_after_nm: after[net.as_str()],
        })
        .collect()
}

fn spread(lengths: &IndexMap<String, i64>) -> i64 {
    lengths.values().max().copied().unwrap_or(0) - lengths.values().min().copied().unwrap_or(0)
}

/// `_tune_match_groups`: tune each `length_match` group over its `max_skew`
/// once every critical group is accepted, group by group in declaration
/// order. A tuned group's copper and results replace the entries in
/// `committed` and `results` only when the whole group ends within its
/// limit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tune_match_groups(
    board: &PhysicalBoard,
    rules: &Rules,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &mut Committed,
    results: &mut [CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    tuning: &RoutingTuning,
) -> Result<Vec<MatchTuningResult>> {
    if board.match_groups.is_empty() {
        return Ok(Vec::new());
    }
    let tracks: Vec<TrackSegment> = copper_of(base_tracks, committed).into_iter().cloned().collect();
    let vias: Vec<Via> =
        base_vias.iter().cloned().chain(committed.iter().flat_map(|(_, _, items)| items.iter().cloned())).collect();
    let current = board_with(board, &tracks, &vias);
    let checks = copper_drc::signal_integrity::verify_match_groups(&current)?;
    let mut outcomes = Vec::new();
    for (check, group) in checks.iter().zip(&board.match_groups) {
        let lengths: IndexMap<String, i64> =
            check.members.iter().map(|(net, length, _)| (net.clone(), *length)).collect();
        let before: IndexMap<String, i64> = group
            .nets
            .iter()
            .map(|net| (net.clone(), tuning.routed.get(net).copied().unwrap_or(lengths[net.as_str()])))
            .collect();
        let steps: Vec<MatchTuningUnit> =
            tuning.early.iter().filter(|step| !disjoint(&step.nets, &group.nets)).cloned().collect();
        let skew = if steps.is_empty() || check.skew_nm.is_none() { check.skew_nm } else { Some(spread(&before)) };
        let bumps: i64 = steps.iter().map(|step| step.bumps).sum();
        if check.status != "fail" {
            let status = if check.status == "pass" { "within_limit" } else { "incomplete" };
            let reported = if !steps.is_empty() && status == "within_limit" { "tuned" } else { status };
            let tuning_groups =
                group_outcomes(rules, group, &steps, &IndexMap::new(), &tuning.group_failures, status == "incomplete");
            outcomes.push(MatchTuningResult {
                id: group.id.clone(),
                max_skew_nm: group.max_skew_nm,
                status: reported.to_owned(),
                skew_before_nm: skew,
                skew_after_nm: check.skew_nm,
                members: members_of(group, &before, &lengths),
                bumps,
                reason: String::new(),
                units: steps,
                tuning_groups,
            });
            continue;
        }
        let (mut trial, tuned, final_steps, mut reason, mut attempts) = tune_match_group(
            board,
            rules,
            base_tracks,
            base_vias,
            committed,
            results,
            result_index,
            &group.nets,
            group.max_skew_nm,
            &lengths,
        )?;
        let mut after = lengths.clone();
        if let Some(candidate) = &trial {
            let copper = copper_of(base_tracks, candidate);
            for net in &group.nets {
                after.insert(net.clone(), net_length(&copper, net));
            }
            let skew_after = spread(&after);
            if skew_after > group.max_skew_nm {
                trial = None;
                after = lengths.clone();
                reason = format!("skew is still {skew_after} nm after tuning");
            }
        }
        let Some(trial) = trial else {
            for item in attempts.values_mut() {
                if item.status == "tuned" {
                    item.status = "failed".to_owned();
                    item.reason = "tuned, but the length_match group was not brought within max_skew".to_owned();
                }
            }
            let tuning_groups = group_outcomes(rules, group, &steps, &attempts, &tuning.group_failures, false);
            outcomes.push(MatchTuningResult {
                id: group.id.clone(),
                max_skew_nm: group.max_skew_nm,
                status: "failed".to_owned(),
                skew_before_nm: skew,
                skew_after_nm: check.skew_nm,
                members: members_of(group, &before, &lengths),
                bumps,
                reason,
                units: steps,
                tuning_groups,
            });
            continue;
        };
        *committed = trial;
        for (unit, result) in tuned {
            results[result_index[&unit]] = result;
        }
        let units: Vec<MatchTuningUnit> = steps.iter().cloned().chain(final_steps.iter().cloned()).collect();
        let tuning_groups = group_outcomes(rules, group, &units, &attempts, &tuning.group_failures, false);
        outcomes.push(MatchTuningResult {
            id: group.id.clone(),
            max_skew_nm: group.max_skew_nm,
            status: "tuned".to_owned(),
            skew_before_nm: skew,
            skew_after_nm: Some(spread(&after)),
            members: members_of(group, &before, &after),
            bumps: bumps + final_steps.iter().map(|step| step.bumps).sum::<i64>(),
            reason: String::new(),
            units,
            tuning_groups,
        });
    }
    Ok(outcomes)
}

/// `_drop_group_steps`: undo the tuning-group steps made while routing that
/// bent any of `entries`; the group's other units go back to their copper as
/// routed. Returns whether copper changed.
pub(crate) fn drop_group_steps(
    entries: &[Vec<String>],
    committed: &mut Committed,
    results: &mut [CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    tuning: &mut RoutingTuning,
) -> bool {
    let nets: IndexSet<String> = entries.iter().flatten().cloned().collect();
    let nets_list: Vec<String> = nets.iter().cloned().collect();
    let steps: Vec<&MatchTuningUnit> =
        tuning.early.iter().filter(|step| !step.group.is_empty() && !disjoint(&nets_list, &step.nets)).collect();
    if steps.is_empty() {
        return false;
    }
    let others: IndexSet<String> =
        steps.iter().flat_map(|step| step.nets.iter()).filter(|net| !nets.contains(*net)).cloned().collect();
    let others_list: Vec<String> = others.iter().cloned().collect();
    let mut changed = false;
    for item in committed.iter_mut() {
        let entry = item.0.clone();
        if !entries.contains(&entry)
            && !disjoint(&others_list, &entry)
            && let Some((entry_tracks, entry_result)) = tuning.as_routed.shift_remove(&entry)
        {
            item.1 = entry_tracks;
            results[result_index[&entry]] = entry_result;
            changed = true;
        }
    }
    let restored: Vec<String> = nets.iter().chain(others.iter()).cloned().collect();
    tuning.early.retain(|step| disjoint(&restored, &step.nets));
    tuning.failed_targets.retain(|key, _| disjoint(&restored, key));
    for net in &others {
        tuning.routed.shift_remove(net);
    }
    changed
}

/// `_tune_bundle_pairs`: tune a bundle's accepted pairs toward their group's
/// longest accepted member each time a bundle pair of a `length_match` group
/// is accepted, so tuning claims room while the inner neighbours are
/// unrouted. Tuning groups wait until every unit is accepted in the bundle
/// and are bent together first. Returns whether copper changed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tune_bundle_pairs(
    board: &PhysicalBoard,
    rules: &Rules,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &mut Committed,
    results: &mut [CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    bundle: &IndexSet<Vec<String>>,
    group: &NetMatchGroup,
    tuning: &mut RoutingTuning,
) -> Result<bool> {
    let members: IndexSet<String> = group.nets.iter().cloned().collect();
    let mut lengths: IndexMap<String, i64> = IndexMap::new();
    {
        let copper = copper_of(base_tracks, committed);
        for net in &group.nets {
            if copper.iter().any(|t| t.net == *net) {
                lengths.insert(net.clone(), net_length(&copper, net));
            }
        }
    }
    let Some(longest) = lengths.values().max().copied() else {
        return Err(Error::Value("max() iterable argument is empty".to_owned()));
    };
    let breakout = BreakoutRegions::new(board);
    let mut changed = false;
    let mut waiting: IndexSet<Vec<String>> = IndexSet::new();
    for (name, units) in tuning_groups(rules, &group.nets) {
        let positions: Vec<usize> = (0..committed.len())
            .filter(|index| units.contains(&committed[*index].0) && !committed[*index].1.is_empty())
            .collect();
        let key: Vec<String> = units.iter().flatten().cloned().collect();
        if positions.len() < units.len() || !units.iter().all(|unit| bundle.contains(unit)) {
            waiting.extend(units.iter().cloned());
            continue;
        }
        if tuning.failed_targets.get(&key).copied().unwrap_or(longest + 1) <= longest {
            continue;
        }
        let (outcome, reason) = tune_group(
            board,
            rules,
            base_tracks,
            base_vias,
            committed,
            &positions,
            results,
            result_index,
            &lengths,
            &members,
            longest,
            group.max_skew_nm,
            &breakout,
            &name,
        )?;
        let Some((proposed, candidates, plan, added)) = outcome else {
            if !reason.is_empty() {
                tuning.failed_targets.insert(key, longest);
                tuning.group_failures.insert(name, reason);
            }
            continue;
        };
        for index in &positions {
            let unit = committed[*index].0.clone();
            for net in &unit {
                tuning.routed.entry(net.clone()).or_insert(lengths[net.as_str()]);
            }
            let as_routed = (committed[*index].1.clone(), results[result_index[&unit]].clone());
            tuning.as_routed.entry(unit.clone()).or_insert(as_routed);
            committed[*index].1 = proposed[index].clone();
            let candidate = candidates[&unit].clone();
            for (net, length) in candidate.nets.iter().zip(&candidate.lengths_nm) {
                lengths.insert(net.clone(), *length);
            }
            results[result_index[&unit]] = candidate;
        }
        tuning.early.push(step(key, "routing", &plan, added, &name));
        tuning.group_failures.shift_remove(&name);
        changed = true;
    }
    for index in 0..committed.len() {
        let unit = committed[index].0.clone();
        if !bundle.contains(&unit)
            || committed[index].1.is_empty()
            || !unit.iter().all(|net| members.contains(net))
            || waiting.contains(&unit)
            || tuning.failed_targets.get(&unit).copied().unwrap_or(longest + 1) <= longest
        {
            continue;
        }
        let result = results[result_index[&unit]].clone();
        let (outcome, reason) = tune_unit(
            board,
            rules,
            base_tracks,
            base_vias,
            committed,
            index,
            &result,
            &lengths,
            &members,
            longest,
            group.max_skew_nm,
            &breakout,
        )?;
        let Some((proposed, candidate, plan, added)) = outcome else {
            if !reason.is_empty() {
                tuning.failed_targets.insert(unit, longest);
            }
            continue;
        };
        for net in &unit {
            tuning.routed.entry(net.clone()).or_insert(lengths[net.as_str()]);
        }
        tuning.as_routed.entry(unit.clone()).or_insert((committed[index].1.clone(), result));
        committed[index].1 = proposed;
        for (net, length) in candidate.nets.iter().zip(&candidate.lengths_nm) {
            lengths.insert(net.clone(), *length);
        }
        results[result_index[&unit]] = candidate;
        tuning.early.push(step(unit, "routing", &plan, added, ""));
        changed = true;
    }
    Ok(changed)
}

fn step(nets: Vec<String>, stage: &str, plan: &UnitTuning, added: i64, group: &str) -> MatchTuningUnit {
    MatchTuningUnit {
        nets,
        stage: stage.to_owned(),
        style: plan.style().to_owned(),
        added_length_nm: added,
        bumps: plan.bumps,
        legs: plan.legs,
        amplitudes_nm: plan.amplitudes_nm.clone(),
        group: group.to_owned(),
        lanes: if group.is_empty() { Vec::new() } else { plan.lanes.clone() },
    }
}

type MatchGroupOutcome = (
    Option<Committed>,
    IndexMap<Vec<String>, CriticalNetResult>,
    Vec<MatchTuningUnit>,
    String,
    IndexMap<String, TuningGroupOutcome>,
);

/// `_tune_match_group`: lengthen every unit of one group toward its longest
/// member; tuning groups are bent together first. Returns the trial copper,
/// the tuned results, the steps and, on failure, `None` and the reason, and
/// each tuning group's outcome. Nothing is changed here.
#[allow(clippy::too_many_arguments)]
fn tune_match_group(
    board: &PhysicalBoard,
    rules: &Rules,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &Committed,
    results: &[CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    nets: &[String],
    max_skew_nm: i64,
    lengths: &IndexMap<String, i64>,
) -> Result<MatchGroupOutcome> {
    let members: IndexSet<String> = nets.iter().cloned().collect();
    let longest = lengths.values().max().copied().expect("a failed group has members");
    let units: Vec<usize> = (0..committed.len())
        .filter(|index| {
            let (unit, items, _) = &committed[*index];
            !items.is_empty() && unit.iter().any(|net| members.contains(net))
        })
        .collect();
    let covered: IndexSet<&String> = units.iter().flat_map(|index| committed[*index].0.iter()).collect();
    if let Some(fixed) = nets.iter().find(|net| !covered.contains(net) && lengths[net.as_str()] < longest - max_skew_nm)
    {
        let reason = format!("{fixed} has no accepted critical copper to tune");
        return Ok((None, IndexMap::new(), Vec::new(), reason, IndexMap::new()));
    }
    let mut trial = committed.clone();
    let mut tuned: IndexMap<Vec<String>, CriticalNetResult> = IndexMap::new();
    let mut steps: Vec<MatchTuningUnit> = Vec::new();
    let mut attempts: IndexMap<String, TuningGroupOutcome> = IndexMap::new();
    let breakout = BreakoutRegions::new(board);
    let mut lengths = lengths.clone();
    for (name, group_units) in tuning_groups(rules, nets) {
        let positions: Vec<usize> =
            units.iter().copied().filter(|index| group_units.contains(&trial[*index].0)).collect();
        let key: Vec<String> = group_units.iter().flatten().cloned().collect();
        let outcome = |status: &str, reason: String| TuningGroupOutcome {
            name: name.clone(),
            nets: key.clone(),
            status: status.to_owned(),
            reason,
        };
        if positions.len() < group_units.len() {
            attempts.insert(name.clone(), outcome("incomplete", "a unit has no accepted critical copper".to_owned()));
            continue;
        }
        let (result, reason) = tune_group(
            board,
            rules,
            base_tracks,
            base_vias,
            &trial,
            &positions,
            results,
            result_index,
            &lengths,
            &members,
            longest,
            max_skew_nm,
            &breakout,
            &name,
        )?;
        let Some((proposed, candidates, plan, added)) = result else {
            let status = if reason.is_empty() { "within_limit" } else { "failed" };
            attempts.insert(name.clone(), outcome(status, reason));
            continue;
        };
        for index in &positions {
            let unit = trial[*index].0.clone();
            trial[*index].1 = proposed[index].clone();
            let candidate = candidates[&unit].clone();
            for (net, length) in candidate.nets.iter().zip(&candidate.lengths_nm) {
                lengths.insert(net.clone(), *length);
            }
            tuned.insert(unit, candidate);
        }
        steps.push(step(key.clone(), "final", &plan, added, &name));
        attempts.insert(name.clone(), outcome("tuned", String::new()));
    }
    for index in units {
        let unit = trial[index].0.clone();
        let result = tuned.get(&unit).cloned().unwrap_or_else(|| results[result_index[&unit]].clone());
        let (outcome, reason) = tune_unit(
            board,
            rules,
            base_tracks,
            base_vias,
            &trial,
            index,
            &result,
            &lengths,
            &members,
            longest,
            max_skew_nm,
            &breakout,
        )?;
        if !reason.is_empty() {
            return Ok((None, IndexMap::new(), Vec::new(), reason, attempts));
        }
        let Some((proposed, candidate, plan, added)) = outcome else { continue };
        trial[index].1 = proposed;
        tuned.insert(unit.clone(), candidate);
        steps.push(step(unit, "final", &plan, added, ""));
    }
    Ok((Some(trial), tuned, steps, String::new(), attempts))
}

type UnitOutcome = (Option<(Vec<TrackSegment>, CriticalNetResult, UnitTuning, i64)>, String);

fn others(
    committed: &Committed,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    skip: &[usize],
) -> (Vec<TrackSegment>, Vec<Via>) {
    let tracks = base_tracks
        .iter()
        .cloned()
        .chain(
            committed
                .iter()
                .enumerate()
                .filter(|(position, _)| !skip.contains(position))
                .flat_map(|(_, (_, items, _))| items.iter().cloned()),
        )
        .collect();
    let vias = base_vias
        .iter()
        .cloned()
        .chain(
            committed
                .iter()
                .enumerate()
                .filter(|(position, _)| !skip.contains(position))
                .flat_map(|(_, (_, _, items))| items.iter().cloned()),
        )
        .collect();
    (tracks, vias)
}

fn split_by_rule(
    board: &PhysicalBoard,
    rules: &Rules,
    breakout: &BreakoutRegions,
    proposed: Vec<TrackSegment>,
) -> Vec<TrackSegment> {
    if !breakout.any() {
        return proposed;
    }
    // Guard: copper keeps the breakout width only inside a terminal land's
    // region, and the net's own width outside it (plan R1).
    proposed
        .into_iter()
        .flat_map(|track| {
            let width = width_or_default(board, rules[track.net.as_str()]);
            breakout.split_tracks(std::slice::from_ref(&track), Some(width))
        })
        .collect()
}

/// `_tune_unit`: tune the unit `committed[index]` toward `longest`, against
/// all other copper: first matched to `longest`, else lengthened by the
/// least that brings it within `max_skew`. Returns the tuned copper, its
/// result, the plan and the added length; `None` when the unit is already
/// within the limit; or `None` and the reason it cannot be tuned.
#[allow(clippy::too_many_arguments)]
fn tune_unit(
    board: &PhysicalBoard,
    rules: &Rules,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &Committed,
    index: usize,
    result: &CriticalNetResult,
    lengths: &IndexMap<String, i64>,
    members: &IndexSet<String>,
    longest: i64,
    max_skew_nm: i64,
    breakout: &BreakoutRegions,
) -> Result<UnitOutcome> {
    let (unit, unit_tracks, unit_vias) = &committed[index];
    let label = unit.join("/");
    let own: Vec<i64> = unit.iter().filter(|net| members.contains(*net)).map(|net| lengths[net.as_str()]).collect();
    let (own_min, own_max) = (*own.iter().min().expect("a unit member"), *own.iter().max().expect("a unit member"));
    let (need, aim) = (longest - max_skew_nm - own_min, longest - own_max);
    if need <= 0 {
        return Ok((None, String::new()));
    }
    if aim < need {
        return Ok((
            None,
            format!(
                "{label}: members differ by {} nm, more than max_skew; a pair is tuned as one unit",
                own_max - own_min
            ),
        ));
    }
    let amplitudes: Vec<i64> = unit.iter().filter_map(|net| rules[net.as_str()].tuning_amplitude_limit_nm).collect();
    let Some(amplitude) = amplitudes.iter().min().copied() else {
        return Ok((None, format!("{label}: no tuning_amplitude_limit is declared")));
    };
    let mut spacing = 0;
    if unit.len() == 2 {
        let (first, second) = (rules[unit[0].as_str()], rules[unit[1].as_str()]);
        spacing = width_or_default(board, first)
            + first.pair_gap_nm.filter(|gap| *gap != 0).or(second.pair_gap_nm).unwrap_or(0);
    }
    let clearance = unit
        .iter()
        .map(|net| rules[net.as_str()].clearance_nm.unwrap_or(0))
        .fold(board.rules.minimum_clearance_nm, i64::max);
    let leg_gap = unit.iter().filter_map(|net| rules[net.as_str()].tuning_spacing_nm).max();
    let (other_tracks, other_vias) = others(committed, base_tracks, base_vias, &[index]);
    let obstacles_vias: Vec<Via> = other_vias.iter().chain(unit_vias).cloned().collect();
    let obstacles = board_with(board, &other_tracks, &obstacles_vias);
    let index_obstacles = RoutingClearanceIndex::new(&obstacles)?;
    let tuner = UnitTuner::new(
        board,
        &index_obstacles,
        unit_tracks.clone(),
        unit,
        spacing,
        amplitude,
        clearance,
        TunerOptions {
            style: rules[unit[0].as_str()].tuning_style,
            leg_gap_nm: leg_gap,
            vias: unit_vias.clone(),
            ..TunerOptions::default()
        },
    )?;
    // Bumps add an even length: match the longest member (one nanometre
    // over it only when an odd exact target leaves no choice), else add the
    // least that brings the unit within max_skew (with breakout regions,
    // the margin inside it).
    let full = if aim - aim.rem_euclid(2) >= need { aim - aim.rem_euclid(2) } else { aim + 1 };
    let mut least = need + if breakout.any() { BREAKOUT_TUNING_MARGIN_NM.min(max_skew_nm) } else { 0 };
    least = full.min(least + least.rem_euclid(2));
    let mut reason = String::new();
    for target in targets(full, least) {
        let plan = tuner.plan(target)?;
        reason = plan.reason.clone();
        let Some(proposed) = plan.tracks.clone() else { continue };
        let proposed = split_by_rule(board, rules, breakout, proposed);
        let mut candidate = retuned_result(board, rules, result, &proposed, unit_vias);
        let mut proposed = proposed;
        if candidate.diagnostics.is_empty() {
            (candidate, proposed, _) =
                validate_candidate(board, candidate, proposed, unit_vias.clone(), &other_tracks, &other_vias, None)?;
        }
        if candidate.connected && candidate.diagnostics.is_empty() {
            return Ok((Some((proposed, candidate, plan, target)), String::new()));
        }
        reason = rejection_gate(&candidate.diagnostics).1;
    }
    Ok((None, format!("{label}: {reason}")))
}

type GroupOutcome =
    (Option<(IndexMap<usize, Vec<TrackSegment>>, IndexMap<Vec<String>, CriticalNetResult>, UnitTuning, i64)>, String);

/// `_tune_group`: bend the units `committed[positions]` of tuning group
/// `name` together: every lane gains the same length. Returns the tuned
/// copper by position, the units' results, the plan and the added length;
/// `None` when no unit needs length; or `None` and the reason.
#[allow(clippy::too_many_arguments)]
fn tune_group(
    board: &PhysicalBoard,
    rules: &Rules,
    base_tracks: &[TrackSegment],
    base_vias: &[Via],
    committed: &Committed,
    positions: &[usize],
    results: &[CriticalNetResult],
    result_index: &IndexMap<Vec<String>, usize>,
    lengths: &IndexMap<String, i64>,
    members: &IndexSet<String>,
    longest: i64,
    max_skew_nm: i64,
    breakout: &BreakoutRegions,
    name: &str,
) -> Result<GroupOutcome> {
    let units: Vec<Vec<String>> = positions.iter().map(|position| committed[*position].0.clone()).collect();
    let nets: Vec<String> = units.iter().flatten().cloned().collect();
    let label = format!("tuning group {name}");
    let own: Vec<Vec<i64>> = units
        .iter()
        .map(|unit| unit.iter().filter(|net| members.contains(*net)).map(|net| lengths[net.as_str()]).collect())
        .collect();
    let needs: Vec<i64> =
        own.iter().map(|values| longest - max_skew_nm - values.iter().min().expect("a unit member")).collect();
    let aims: Vec<i64> = own.iter().map(|values| longest - values.iter().max().expect("a unit member")).collect();
    if needs.iter().max().is_none_or(|need| *need <= 0) {
        return Ok((None, String::new()));
    }
    let min_aim = *aims.iter().min().expect("a unit");
    if min_aim < 2 {
        let short = &units[aims.iter().position(|aim| *aim == min_aim).expect("the minimum")];
        return Ok((
            None,
            format!(
                "{label}: {} is as long as the longest member, and a group adds the same length to every lane",
                short.join("/")
            ),
        ));
    }
    let amplitudes: Vec<i64> = nets.iter().filter_map(|net| rules[net.as_str()].tuning_amplitude_limit_nm).collect();
    let Some(amplitude) = amplitudes.iter().min().copied() else {
        return Ok((None, format!("{label}: no tuning_amplitude_limit is declared")));
    };
    let spacings: Vec<i64> = units
        .iter()
        .map(|unit| {
            if unit.len() == 2 {
                let (first, second) = (rules[unit[0].as_str()], rules[unit[1].as_str()]);
                width_or_default(board, first)
                    + first.pair_gap_nm.filter(|gap| *gap != 0).or(second.pair_gap_nm).unwrap_or(0)
            } else {
                0
            }
        })
        .collect();
    let clearance = nets
        .iter()
        .map(|net| rules[net.as_str()].clearance_nm.unwrap_or(0))
        .fold(board.rules.minimum_clearance_nm, i64::max);
    let leg_gap = nets.iter().filter_map(|net| rules[net.as_str()].tuning_spacing_nm).max();
    let style = if nets.iter().all(|net| rules[net.as_str()].tuning_style == TuningStyle::Serpentine) {
        TuningStyle::Serpentine
    } else {
        TuningStyle::Bumps
    };
    let group_vias: Vec<Via> = positions.iter().flat_map(|position| committed[*position].2.iter().cloned()).collect();
    let (other_tracks, other_vias) = others(committed, base_tracks, base_vias, positions);
    let obstacles_vias: Vec<Via> = other_vias.iter().chain(&group_vias).cloned().collect();
    let obstacles = board_with(board, &other_tracks, &obstacles_vias);
    let index_obstacles = RoutingClearanceIndex::new(&obstacles)?;
    let group_tracks: Vec<TrackSegment> =
        positions.iter().flat_map(|position| committed[*position].1.iter().cloned()).collect();
    let tuner = GroupTuner::new(
        board,
        &index_obstacles,
        group_tracks,
        &units,
        &spacings,
        amplitude,
        clearance,
        TunerOptions { style, leg_gap_nm: leg_gap, vias: group_vias.clone(), ..TunerOptions::default() },
    )?;
    // Every lane gains the same even length, never past the longest member.
    let full = min_aim - min_aim.rem_euclid(2);
    let least_need = needs.iter().copied().filter(|need| *need > 0).min().expect("a unit needs length");
    let mut least = least_need + if breakout.any() { BREAKOUT_TUNING_MARGIN_NM.min(max_skew_nm) } else { 0 };
    least = full.min(least + least.rem_euclid(2));
    let mut reason = String::new();
    for target in targets(full, least) {
        let plan = tuner.plan(target)?;
        let Some(planned) = plan.tracks.clone() else {
            reason = plan.reason.clone();
            continue;
        };
        let mut proposed: IndexMap<usize, Vec<TrackSegment>> = IndexMap::new();
        for position in positions {
            let unit = &committed[*position].0;
            let items: Vec<TrackSegment> = planned.iter().filter(|t| unit.contains(&t.net)).cloned().collect();
            proposed.insert(*position, split_by_rule(board, rules, breakout, items));
        }
        let mut candidates: IndexMap<Vec<String>, CriticalNetResult> = IndexMap::new();
        for (position, unit) in positions.iter().zip(&units) {
            let original = &results[result_index[unit]];
            candidates.insert(
                unit.clone(),
                retuned_result(board, rules, original, &proposed[position], &committed[*position].2),
            );
        }
        let mut rejected = candidates.values().find(|item| !item.diagnostics.is_empty()).cloned();
        if rejected.is_none() {
            let mut joint = candidates[&units[0]].clone();
            joint.nets = nets.clone();
            let all: Vec<TrackSegment> =
                positions.iter().flat_map(|position| proposed[position].iter().cloned()).collect();
            let (checked, _, _) =
                validate_candidate(board, joint, all, group_vias.clone(), &other_tracks, &other_vias, None)?;
            if checked.connected && checked.diagnostics.is_empty() {
                return Ok((Some((proposed, candidates, plan, target)), String::new()));
            }
            rejected = Some(checked);
        }
        reason = rejection_gate(&rejected.expect("a rejection").diagnostics).1;
    }
    Ok((None, format!("{label}: {reason}")))
}

/// `_retuned_result`: an accepted group's metrics and profile gates for new
/// copper.
fn retuned_result(
    board: &PhysicalBoard,
    rules: &Rules,
    original: &CriticalNetResult,
    tracks: &[TrackSegment],
    vias: &[Via],
) -> CriticalNetResult {
    let per_net: Vec<Vec<&TrackSegment>> =
        original.nets.iter().map(|net| tracks.iter().filter(|t| t.net == *net).collect()).collect();
    let lengths: Vec<i64> = per_net.iter().map(|items| track_length(items)).collect();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (net, items) in original.nets.iter().zip(&per_net) {
        let net_vias: Vec<&Via> = vias.iter().filter(|v| v.net == *net).collect();
        diagnostics.extend(budget_diagnostics(rules[net.as_str()], &net_vias));
        warnings.extend(budget_warnings(rules[net.as_str()], items));
    }
    let mut result = original.clone();
    result.track_count = tracks.len() as i64;
    result.lengths_nm = lengths.clone();
    if original.nets.len() != 2 {
        result.diagnostics = diagnostics;
        result.warnings = warnings;
        return result;
    }
    let (first, second) = (rules[original.nets[0].as_str()], rules[original.nets[1].as_str()]);
    let skew = (lengths[0] - lengths[1]).abs();
    let skews: Vec<i64> = [first.max_skew_nm, second.max_skew_nm].into_iter().flatten().collect();
    if let Some(limit) = skews.iter().min()
        && skew > *limit
    {
        warnings.push(format!("pair skew {skew} nm exceeds {limit} nm"));
    }
    let spacing =
        width_or_default(board, first) + first.pair_gap_nm.filter(|gap| *gap != 0).or(second.pair_gap_nm).unwrap_or(0);
    let first_tracks: Vec<TrackSegment> = per_net[0].iter().map(|t| (*t).clone()).collect();
    let second_tracks: Vec<TrackSegment> = per_net[1].iter().map(|t| (*t).clone()).collect();
    let coupled = coupled_length(&first_tracks, &second_tracks, spacing);
    let uncoupled: Vec<i64> = lengths.iter().map(|length| (length - coupled).max(0)).collect();
    let limits: Vec<i64> =
        [first.maximum_uncoupled_length_nm, second.maximum_uncoupled_length_nm].into_iter().flatten().collect();
    if let Some(limit) = limits.iter().min() {
        let worst = *uncoupled.iter().max().expect("two members");
        if worst > *limit {
            diagnostics.push(format!("pair uncoupled length {worst} nm exceeds {limit} nm"));
        }
    }
    result.skew_nm = skew;
    result.coupled_length_nm = coupled;
    result.uncoupled_lengths_nm = uncoupled;
    result.diagnostics = diagnostics;
    result.warnings = warnings;
    result
}

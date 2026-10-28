//! Bounded shared-spine shortcuts (`pcbir.pair_refine`); the critical owner
//! validates each proposal.

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_physical::Point;
use copper_physical::pyfloat::{hypot, round_int};

use crate::pair_search::{PairSearchCandidate, heading, lane_paths, legal, paths, tracks};
use crate::routing_clearance::RoutingClearanceIndex;
use crate::{Error, Result};

/// `PairRefinementStats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PairRefinementStats {
    pub attempts: i64,
    pub candidates: i64,
}

/// A pair of lanes: the first and second member's tracks.
pub type Lanes = (Vec<TrackSegment>, Vec<TrackSegment>);

/// `_compact`: remove only exact, forward collinear vertices; keep both ends.
fn compact(points: &[Point]) -> Vec<Point> {
    let mut result: Vec<Point> = Vec::new();
    for point in points {
        if result.last() == Some(point) {
            continue;
        }
        while result.len() > 1 {
            let (a, b) = (result[result.len() - 2], result[result.len() - 1]);
            let (ax, ay) = (i128::from(b.x_nm - a.x_nm), i128::from(b.y_nm - a.y_nm));
            let (bx, by) = (i128::from(point.x_nm - b.x_nm), i128::from(point.y_nm - b.y_nm));
            if ax * by != ay * bx || ax * bx + ay * by <= 0 {
                break;
            }
            result.pop();
        }
        result.push(*point);
    }
    result
}

fn length(lane: &[TrackSegment]) -> i64 {
    lane.iter().map(|t| round_int(hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64))).sum()
}

fn reversed(lane: &[TrackSegment]) -> Vec<TrackSegment> {
    lane.iter().rev().map(|t| TrackSegment { start: t.end, end: t.start, ..t.clone() }).collect()
}

/// `paired_shortcuts`: joint octilinear shortcuts that keep the ports and
/// input copper, each improving on the last; the caller accepts or rejects.
pub fn paired_shortcuts(
    board: &PhysicalBoard,
    candidate: &PairSearchCandidate,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    maximum_attempts: i64,
    stats: &mut PairRefinementStats,
) -> Result<Vec<Lanes>> {
    if maximum_attempts <= 0 {
        return Err(Error::Value("pair refinement bound must be positive".to_owned()));
    }
    let mut proposals = Vec::new();
    if !candidate.via_pairs.is_empty() || !candidate.return_vias.is_empty() {
        return Ok(proposals); // Surface-spine compaction cannot own multilayer transitions.
    }
    let (Some(start), Some(end)) = (&candidate.start_port, &candidate.end_port) else { return Ok(proposals) };
    let spine = &candidate.spine;
    if spine.is_empty() || spine[0] != start.center || spine[spine.len() - 1] != end.center {
        return Ok(proposals);
    }
    let default_width = board.rules.default_track_width_nm;
    let width = first_rule.width_nm.filter(|w| *w != 0).unwrap_or(default_width);
    let Some(gap) = first_rule.pair_gap_nm else { return Ok(proposals) };
    if second_rule.pair_gap_nm != Some(gap)
        || second_rule.width_nm.filter(|w| *w != 0).unwrap_or(default_width) != width
    {
        return Ok(proposals);
    }
    let escaped: Vec<TrackSegment> =
        start.first.iter().chain(&start.second).chain(&end.first).chain(&end.second).cloned().collect();
    if escaped.is_empty() || escaped.iter().any(|t| t.layer != escaped[0].layer) || start.sign != -end.sign {
        return Ok(proposals);
    }
    let lanes = [(&candidate.first, &start.first, &end.first), (&candidate.second, &start.second, &end.second)];
    for (lane, lead, tail) in lanes {
        if lead.is_empty()
            || tail.is_empty()
            || lane.len() < lead.len()
            || lane.len() < tail.len()
            || lane[..lead.len()] != lead[..]
            || lane[lane.len() - tail.len()..] != reversed(tail)[..]
        {
            return Ok(proposals);
        }
    }
    let layer = escaped[0].layer;
    let offset = (width + gap + 1).div_euclid(2);
    let clearance = board
        .rules
        .minimum_clearance_nm
        .max(first_rule.clearance_nm.unwrap_or(0))
        .max(second_rule.clearance_nm.unwrap_or(0));
    let mut held = board.clone();
    held.tracks.extend(escaped.iter().cloned());
    let index = RoutingClearanceIndex::new(&held)?;
    let inbound = (end.heading + 4) % 8;
    let materialize = |points: &[Point]| -> Result<Option<Lanes>> {
        let n = points.len();
        if n < 2
            || heading(points[0], points[1]) != Some(start.heading)
            || heading(points[n - 2], points[n - 1]) != Some(inbound)
        {
            return Ok(None);
        }
        let Some((first_points, second_points)) = lane_paths(points, start.heading, inbound, offset, start.sign) else {
            return Ok(None);
        };
        let a = tracks(&first_rule.net, &first_points, width, layer, Some(&index.breakout));
        let b = tracks(&second_rule.net, &second_points, width, layer, Some(&index.breakout));
        if !legal(board, &index, &a, &b, clearance)? {
            return Ok(None);
        }
        let first: Vec<TrackSegment> = start.first.iter().cloned().chain(a).chain(reversed(&end.first)).collect();
        let second: Vec<TrackSegment> = start.second.iter().cloned().chain(b).chain(reversed(&end.second)).collect();
        if !legal(board, &index, &first, &second, clearance)? {
            return Ok(None);
        }
        Ok(Some((first, second)))
    };
    let improves = |proposal: &Lanes, incumbent: &Lanes| {
        let (before, after) =
            ([length(&incumbent.0), length(&incumbent.1)], [length(&proposal.0), length(&proposal.1)]);
        after[0] <= before[0]
            && after[1] <= before[1]
            && (after[0] + after[1], proposal.0.len() + proposal.1.len())
                < (before[0] + before[1], incumbent.0.len() + incumbent.1.len())
    };
    let mut incumbent: Lanes = (candidate.first.clone(), candidate.second.clone());
    // Provenance is structural: a stale or unrelated spine must never delete
    // accepted copper outside its own lanes.
    if materialize(spine)?.as_ref() != Some(&incumbent) {
        return Ok(proposals);
    }
    let mut current = spine.clone();
    let compacted = compact(&current);
    if compacted != current && stats.attempts < maximum_attempts {
        stats.attempts += 1;
        if let Some(proposal) = materialize(&compacted)?
            && improves(&proposal, &incumbent)
        {
            incumbent = proposal.clone();
            stats.candidates += 1;
            proposals.push(proposal);
        }
        // Exact forward-collinear compaction is geometrically equivalent even
        // if per-segment integer length rounding rejects its metric score.
        current = compacted;
    }
    while stats.attempts < maximum_attempts {
        let mut changed = false;
        'search: for first in 0..current.len().saturating_sub(2) {
            for last in (first + 2..current.len()).rev() {
                for bridge in paths(current[first], current[last]) {
                    let joined: Vec<Point> =
                        current[..first].iter().chain(&bridge).chain(&current[last + 1..]).copied().collect();
                    let points = compact(&joined);
                    if points == current {
                        continue;
                    }
                    if stats.attempts >= maximum_attempts {
                        return Ok(proposals);
                    }
                    stats.attempts += 1;
                    let Some(proposal) = materialize(&points)? else { continue };
                    if !improves(&proposal, &incumbent) {
                        continue;
                    }
                    current = points;
                    incumbent = proposal.clone();
                    stats.candidates += 1;
                    proposals.push(proposal);
                    changed = true;
                    break 'search;
                }
            }
        }
        if !changed {
            return Ok(proposals);
        }
    }
    Ok(proposals)
}

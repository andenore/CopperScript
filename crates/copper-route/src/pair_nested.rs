//! Coupled L-shaped candidates for a pair that wraps a package corner
//! (`pcbir.pair_search.nested_exit_candidates`, plan R7).

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_pcb::{Error, Result};
use copper_physical::Point;
use indexmap::IndexMap;

use crate::pair_search::{
    HEADS, PORT_LENGTHS_NM, PairSearchCandidate, Port, PortRequest, lane_paths, legal, nested_turn_chamfer,
    pair_profile, tracks, turn,
};
use crate::routing::GlobalNetRoute;
use crate::routing_clearance::RoutingClearanceIndex;

/// One nested candidate: its turn distance, its diagonal's axial leg and the
/// candidate.
pub type NestedCandidate = (i64, i64, PairSearchCandidate);

fn reversed(items: &[TrackSegment]) -> Vec<TrackSegment> {
    items
        .iter()
        .rev()
        .map(|t| TrackSegment { net: t.net.clone(), start: t.end, end: t.start, width_nm: t.width_nm, layer: t.layer })
        .collect()
}

/// `nested_exit_candidates`: the pair leaves its lands at `component` along
/// `direction` through its shortest legal port and turns 90 degrees onto its
/// run along `travel` at each distance in `turns`, with one 45-degree
/// diagonal, longest first, down in `chamfer_step` steps to one pair pitch.
/// It enters the far lands through their shortest legal port; a run beyond
/// their column jogs back just before it. Only candidates whose lanes clear
/// the board's copper reach `accept`, which returns whether to stop.
#[allow(clippy::too_many_arguments)]
pub fn nested_exit_candidates(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    first_guide: &GlobalNetRoute,
    second_guide: &GlobalNetRoute,
    component: &str,
    direction: (i64, i64),
    travel: (i64, i64),
    turns: impl IntoIterator<Item = i64>,
    chamfer_step: i64,
    accept: &mut dyn FnMut(NestedCandidate) -> Result<bool>,
) -> Result<()> {
    if chamfer_step <= 0 {
        return Err(Error::Value("nested exit chamfer step must be positive".to_owned()));
    }
    let Some((width, offset, clearance, layer)) =
        pair_profile(board, first_rule, second_rule, first_guide, second_guide)?
    else {
        return Ok(());
    };
    let partners = crate::pair_terminals::terminal_partners(first_guide, second_guide).expect("profiled partners");
    let even = |axis: (i64, i64)| HEADS.iter().step_by(2).any(|head| *head == axis);
    if !even(direction) || !even(travel) {
        return Ok(());
    }
    let outward = HEADS.iter().position(|head| *head == direction).expect("an even heading");
    let inward = HEADS.iter().position(|head| *head == travel).expect("an even heading");
    if turn(outward, inward) != 2 || !partners.contains_key(&format!("component:{component}")) {
        return Ok(());
    }
    let index = RoutingClearanceIndex::new(board)?;
    let mut ports: IndexMap<String, Vec<Port>> = IndexMap::new();
    for access in &first_guide.accesses {
        let request = PortRequest {
            board,
            index: &index,
            first_name: &first_rule.net,
            second_name: &second_rule.net,
            first: access.pad_position,
            second: partners[&format!("pad:{}", access.pad)].pad_position,
            component: &access.pad.component,
            width,
            offset,
            clearance,
            layer,
            lengths: &PORT_LENGTHS_NM,
            allow_local_fallback: true,
        };
        ports.insert(access.pad.component.clone(), request.ports()?);
    }
    let start = ports.get(component).and_then(|items| items.iter().find(|port| port.heading == outward)).cloned();
    let Some(start) = start else { return Ok(()) };
    let end = ports
        .iter()
        .filter(|(name, _)| name.as_str() != component)
        .flat_map(|(_, items)| items.iter())
        .find(|port| port.heading == (inward + 4) % 8 && port.sign == -start.sign)
        .cloned();
    let Some(end) = end else { return Ok(()) };
    let first_escape: Vec<TrackSegment> = start.first.iter().chain(&end.first).cloned().collect();
    let second_escape: Vec<TrackSegment> = start.second.iter().chain(&end.second).cloned().collect();
    if !legal(board, &index, &first_escape, &second_escape, clearance)? {
        return Ok(());
    }
    drop(index);
    let mut escaped_board = board.clone();
    escaped_board.tracks.extend(first_escape.iter().cloned());
    escaped_board.tracks.extend(second_escape.iter().cloned());
    let index = RoutingClearanceIndex::new(&escaped_board)?;
    let first_land = first_guide
        .accesses
        .iter()
        .find(|access| access.pad.component == component)
        .map(|access| access.pad_position)
        .expect("the component is a terminal");
    let second_land = partners[&format!("component:{component}")].pad_position;
    let middle = Point::new(
        (first_land.x_nm + second_land.x_nm).div_euclid(2),
        (first_land.y_nm + second_land.y_nm).div_euclid(2),
    );
    let at = |across: i64, along: i64| {
        Point::new(
            middle.x_nm + direction.0 * across + travel.0 * along,
            middle.y_nm + direction.1 * across + travel.1 * along,
        )
    };
    let project =
        |point: Point, axis: (i64, i64)| (point.x_nm - middle.x_nm) * axis.0 + (point.y_nm - middle.y_nm) * axis.1;
    let (port, column, reach) =
        (project(start.center, direction), project(end.center, direction), project(end.center, travel));
    let pair_gap = first_rule.pair_gap_nm.expect("profiled pairs declare a gap");
    let least = nested_turn_chamfer(width, pair_gap);
    for turn in turns {
        let jog = turn - column;
        // The diagonal may start one lane offset after the port, where the
        // inner lane's miter still leaves it a straight piece.
        let longest = (turn - port - offset).min((if jog != 0 { reach - jog - least } else { reach }) - least);
        if jog < 0 || longest < least {
            continue;
        }
        let mut chamfers: Vec<i64> = Vec::new();
        let mut chamfer = longest;
        while chamfer > least {
            chamfers.push(chamfer);
            chamfer -= chamfer_step;
        }
        chamfers.push(least);
        for chamfer in chamfers {
            let mut points = vec![start.center, at(turn - chamfer, 0), at(turn, chamfer)];
            if jog != 0 {
                points.push(at(turn, reach - jog - least));
                points.push(at(column, reach - least));
            }
            points.push(end.center);
            let Some((first_lane, second_lane)) = lane_paths(&points, outward, inward, offset, start.sign) else {
                continue;
            };
            let a = tracks(&first_rule.net, &first_lane, width, layer, Some(&index.breakout));
            let b = tracks(&second_rule.net, &second_lane, width, layer, Some(&index.breakout));
            if !legal(&escaped_board, &index, &a, &b, clearance)? {
                continue;
            }
            let candidate = PairSearchCandidate {
                first: start.first.iter().cloned().chain(a).chain(reversed(&end.first)).collect(),
                second: start.second.iter().cloned().chain(b).chain(reversed(&end.second)).collect(),
                expanded_states: 0,
                search_index: 0,
                spine: points,
                start_port: Some(start.clone()),
                end_port: Some(end.clone()),
                via_pairs: Vec::new(),
                return_vias: Vec::new(),
            };
            if accept((turn, chamfer, candidate))? {
                return Ok(());
            }
        }
    }
    Ok(())
}

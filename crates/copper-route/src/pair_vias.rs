//! Bounded paired terminal transitions plus a joint single-layer middle
//! route (`pcbir.pair_vias`).
//!
//! Short surface collars widen to manufacturing-legal matched through-vias;
//! the other signal layer tapers back to the declared pair spacing. Both
//! lanes are searched together. This is not arbitrary 3-D maze routing or SI
//! signoff. Return vias (`require_return_vias`) report not implemented.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{NetRoutingRule, PadReference, PhysicalBoard, TrackSegment, Via};
use copper_physical::footprint::PadKind;
use copper_physical::geometry::shapes_clear;
use copper_physical::pyfloat::hypot;
use copper_physical::{CopperLayer, Point, RoundedConvexShape};

use crate::Result;
use crate::pair_search::{
    Continuation, HEADS, PORT_LENGTHS_NM, PairSearchCandidate, PairSearchStats, PairTransitionExample,
    PairTransitionInventory, Port, PortRequest, SearchBounds, SpineSearch, can_depart, legal, normal, remaining_states,
    shift, tracks,
};
use crate::pair_terminals::{terminal_copper_layers, terminal_partners};
use crate::pin_access::{LocalAccessBounds, LocalAccessStats, local_access_path};
use crate::routing::GlobalNetRoute;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_layers::{routing_layers, signal_layer_preferences};
use crate::surface_path::via_inside_board;

/// `_Transition`: a collar, its signal vias, return vias and site rank.
#[derive(Clone, Debug)]
pub struct Transition {
    pub port: Port,
    pub vias: Vec<Via>,
    pub returns: Vec<Via>,
    pub rank: i64,
    pub outer: bool,
}

/// `_TransitionTrace`: finite diagnostic samples, with no influence on admission.
#[derive(Default)]
pub struct TransitionTrace {
    pub counts: IndexMap<String, i64>,
    pub examples: Vec<PairTransitionExample>,
}

impl TransitionTrace {
    fn count(&mut self, stage: &str, by: i64) {
        *self.counts.entry(stage.to_owned()).or_default() += by;
    }

    /// `record`: count a stage and keep at most two examples of it (16 in all).
    fn record(
        &mut self,
        stage: &str,
        positions: &[Point],
        index: Option<&RoutingClearanceIndex<'_>>,
        vias: &[Via],
        tracks: &[TrackSegment],
        local: Option<&LocalAccessStats>,
    ) -> Result<()> {
        self.count(stage, 1);
        if self.examples.len() >= 16 || self.examples.iter().filter(|item| item.stage == stage).count() >= 2 {
            return Ok(());
        }
        let mut blockers: IndexMap<(String, String), crate::routing_clearance::RoutingBlocker> = IndexMap::new();
        if let Some(index) = index {
            for via in vias {
                for blocker in index.via_blocker_records(via)? {
                    blockers.entry((blocker.identity.clone(), blocker.occurrence.clone())).or_insert(blocker);
                }
            }
            for track in tracks {
                for blocker in index.track_blocker_records(track)? {
                    blockers.entry((blocker.identity.clone(), blocker.occurrence.clone())).or_insert(blocker);
                }
            }
        }
        self.examples.push(PairTransitionExample {
            stage: stage.to_owned(),
            positions: positions.to_vec(),
            blockers: blockers.into_values().take(8).collect(),
            local_expanded_states: local.map_or(0, |stats| stats.expanded_states),
            local_discovered_nodes: local.map_or(0, |stats| stats.discovered_nodes),
        });
        Ok(())
    }

    /// `tuple(sorted(trace.counts.items()))`.
    fn sorted_counts(&self) -> Vec<(String, i64)> {
        let mut counts: Vec<(String, i64)> = self.counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
        counts.sort();
        counts
    }
}

fn clearance_of(board: &PhysicalBoard, first: &NetRoutingRule, second: &NetRoutingRule) -> i64 {
    board.rules.minimum_clearance_nm.max(first.clearance_nm.unwrap_or(0)).max(second.clearance_nm.unwrap_or(0))
}

fn width_of(board: &PhysicalBoard, rule: &NetRoutingRule) -> i64 {
    rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm)
}

/// `transition_spacing`: via pads and drills need wider spacing than the
/// coupled trace pitch (a breakout pair's vias keep its pair gap).
pub fn transition_spacing(board: &PhysicalBoard, first: &NetRoutingRule, second: &NetRoutingRule) -> i64 {
    let mut clearance = clearance_of(board, first, second);
    if (first.breakout_length_nm.is_some() || second.breakout_length_nm.is_some())
        && let (Some(a), Some(b)) = (first.pair_gap_nm, second.pair_gap_nm)
    {
        clearance = clearance.max(a).max(b);
    }
    let width = width_of(board, first);
    (width + first.pair_gap_nm.unwrap_or(0))
        .max(board.rules.default_via_size_nm + clearance + 4)
        .max(board.rules.default_via_drill_nm + board.rules.minimum_hole_clearance_nm + 4)
}

/// `_reserve_vias`: each via inside the board and clear, then locked.
fn reserve_vias(board: &PhysicalBoard, index: &mut RoutingClearanceIndex<'_>, vias: &[Via]) -> Result<bool> {
    for via in vias {
        if !via_inside_board(board, via.position, via.size_nm)?
            || !index.can_via(
                &via.net,
                via.position,
                via.size_nm,
                via.from_layer,
                via.to_layer,
                Some(via.drill_nm),
                false,
                None,
            )?
        {
            return Ok(false);
        }
        index.add_via(via.clone(), true, None)?;
    }
    Ok(true)
}

/// `_signal_drills_outside_smd`: no open signal drill in a solderable SMD land of its net.
fn signal_drills_outside_smd(board: &PhysicalBoard, pair: &[Via]) -> Result<bool> {
    let mut assigned: IndexMap<PadReference, &str> = IndexMap::new();
    for net in &board.nets {
        for pad in &net.pads {
            assigned.insert(pad.clone(), &net.name);
        }
    }
    for placement in &board.placements {
        for pad in &board.footprints[&placement.footprint].pads {
            if pad.kind != PadKind::Smd {
                continue;
            }
            let net = assigned.get(&PadReference::new(&placement.reference, &pad.number)).copied();
            for via in pair {
                if net != Some(via.net.as_str()) {
                    continue;
                }
                let position = copper_pcb::transform::transformed_local_point(placement, pad.position);
                let shape = copper_pcb::transform::placed_pad_shape(position, pad, placement);
                let drill = RoundedConvexShape::new(vec![via.position], via.drill_nm.div_euclid(2))?;
                if !shapes_clear(&shape, &drill, 0) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn via(board: &PhysicalBoard, net: &str, position: Point, span: (CopperLayer, CopperLayer, Option<String>)) -> Via {
    Via {
        net: net.to_owned(),
        position,
        size_nm: board.rules.default_via_size_nm,
        drill_nm: board.rules.default_via_drill_nm,
        from_layer: span.0,
        to_layer: span.1,
        technology: span.2,
        finish: "standard".to_owned(),
    }
}

fn with_copper(board: &PhysicalBoard, tracks: &[TrackSegment], vias: &[Via]) -> PhysicalBoard {
    let mut copy = board.clone();
    copy.tracks.extend(tracks.iter().cloned());
    copy.vias.extend(vias.iter().cloned());
    copy
}

/// `_return_via`: the return vias of a paired transition (none required here).
fn return_via(first: &NetRoutingRule, second: &NetRoutingRule) -> Result<Option<Vec<Via>>> {
    if !(first.require_return_vias || second.require_return_vias) {
        return Ok(Some(Vec::new()));
    }
    // PENDING: return via sites (_return_positions, return_paths.shared_reference_plane).
    copper_pcb::not_implemented("critical paired transitions with return vias")
}

/// `_local_via_ports`: the finite recessed-land fallback choosing legal vias
/// before searching collars.
#[allow(clippy::too_many_arguments)]
fn local_via_ports(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    a: Point,
    b: Point,
    component: &str,
    surface: CopperLayer,
    layer: CopperLayer,
    trace: &mut TransitionTrace,
) -> Result<Vec<Port>> {
    let Some(span) = crate::routing_vias::physical_via_span(board, surface, layer) else { return Ok(Vec::new()) };
    let width = width_of(board, first);
    let clearance = clearance_of(board, first, second);
    let offset = (transition_spacing(board, first, second) + 1).div_euclid(2);
    let middle = Point::new((a.x_nm + b.x_nm).div_euclid(2), (a.y_nm + b.y_nm).div_euclid(2));
    let pitch = (a.x_nm - b.x_nm).abs().max((a.y_nm - b.y_nm).abs());
    let pose = board.placements.iter().find(|p| p.reference == component).expect("a placed component");
    let outward = Point::new(middle.x_nm - pose.position.x_nm, middle.y_nm - pose.position.y_nm);
    let mut headings = vec![0usize, 2, 4, 6];
    headings.sort_by_key(|h| (-(HEADS[*h].0 * outward.x_nm + HEADS[*h].1 * outward.y_nm), *h));
    let base = RoutingClearanceIndex::new(board)?;
    let mut results = Vec::new();
    let (mut sites, mut calls) = (0, 0);
    let lengths =
        [500_000, 750_000, 1_000_000, 1_500_000, 2_000_000, 3_000_000, 4_000_000, 4_500_000, 5_000_000, 6_000_000];
    let transverses = [0, -pitch.div_euclid(2), pitch.div_euclid(2), -pitch, pitch];
    'product: for &heading_index in &headings {
        for &distance in &lengths {
            for &transverse in &transverses {
                for sign in [-1, 1] {
                    if sites >= 16 || calls >= 32 || results.len() >= 8 {
                        break 'product;
                    }
                    let (dx, dy) = HEADS[heading_index];
                    let center = Point::new(
                        middle.x_nm + dx * distance - dy * transverse,
                        middle.y_nm + dy * distance + dx * transverse,
                    );
                    let lane_normal = normal(heading_index, offset);
                    let targets = [shift(center, lane_normal, sign), shift(center, lane_normal, -sign)];
                    let pair = vec![
                        via(board, &first.net, targets[0], span.clone()),
                        via(board, &second.net, targets[1], span.clone()),
                    ];
                    trace.count("local_sites_sampled", 1);
                    if !signal_drills_outside_smd(board, &pair)? {
                        trace.record("local_signal_drill_in_smd", &targets, None, &[], &[], None)?;
                        continue;
                    }
                    let mut reserved = RoutingClearanceIndex::new(board)?;
                    if !reserve_vias(board, &mut reserved, &pair)? {
                        trace.record("local_via_clearance", &targets, Some(&base), &pair, &[], None)?;
                        continue;
                    }
                    sites += 1;
                    trace.count("local_legal_via_sites", 1);
                    let approaches = targets.map(|p| Point::new(p.x_nm - dx * 125_000, p.y_nm - dy * 125_000));
                    let tails = [
                        tracks(&first.net, &[approaches[0], targets[0]], width, surface, Some(&base.breakout)),
                        tracks(&second.net, &[approaches[1], targets[1]], width, surface, Some(&base.breakout)),
                    ];
                    if !legal(board, &base, &tails[0], &tails[1], clearance)? {
                        let mut both = tails[0].clone();
                        both.extend(tails[1].iter().cloned());
                        trace.record("local_tail_clearance", &targets, Some(&base), &[], &both, None)?;
                        continue;
                    }
                    for order in [[0usize, 1], [1, 0]] {
                        let mut copper = tails[0].clone();
                        copper.extend(tails[1].iter().cloned());
                        let held = with_copper(board, &copper, &pair);
                        let mut index = RoutingClearanceIndex::new(&held)?;
                        index.pair_access = true;
                        let mut lanes: [Option<Vec<TrackSegment>>; 2] = [None, None];
                        for member in order {
                            if calls >= 32 {
                                break;
                            }
                            calls += 1;
                            let mut local =
                                LocalAccessStats { outcome: "not_started".to_owned(), ..Default::default() };
                            let (name, land) = if member == 0 { (&first.net, a) } else { (&second.net, b) };
                            let bounds =
                                LocalAccessBounds { step_nm: 25_000, detour_nm: 500_000, maximum_states: 2400 };
                            let path = local_access_path(
                                &held,
                                &index,
                                name,
                                land,
                                approaches[member],
                                surface,
                                width,
                                bounds,
                                Some(&mut local),
                                None,
                            )?;
                            trace.record(
                                &format!("local_path_{}", local.outcome),
                                &[land, approaches[member]],
                                None,
                                &[],
                                &[],
                                Some(&local),
                            )?;
                            let sign_text = sign.to_string();
                            trace.count(
                                &format!(
                                    "local_{}_member{member}_heading{heading_index}_sign{sign_text}",
                                    local.outcome
                                ),
                                1,
                            );
                            let Some(path) = path else { break };
                            let mut lane = path.clone();
                            lane.extend(tails[member].iter().cloned());
                            lanes[member] = Some(lane);
                            for track in path {
                                index.add_track(track, true, None)?;
                            }
                        }
                        if let [Some(first_lane), Some(second_lane)] = &lanes {
                            let check = RoutingClearanceIndex::new(&held)?;
                            if legal(board, &check, first_lane, second_lane, clearance)? {
                                results.push(Port {
                                    center,
                                    heading: heading_index,
                                    sign,
                                    first: first_lane.clone(),
                                    second: second_lane.clone(),
                                    preference: 8,
                                });
                                trace.count("local_collars", 1);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    trace.count("local_path_calls", calls);
    trace.count("local_site_limit_reached", i64::from(sites >= 16));
    trace.count("local_call_limit_reached", i64::from(calls >= 32));
    Ok(results)
}

/// `_interleave`: stable family diversity inside a finite allowance.
fn interleave<T: Clone>(groups: &[Vec<T>], limit: usize) -> Vec<T> {
    let mut cursors: Vec<(usize, usize)> = (0..groups.len()).map(|g| (g, 0)).collect();
    let mut emitted = Vec::new();
    while !cursors.is_empty() && emitted.len() < limit {
        let mut remaining = Vec::new();
        for (group, position) in cursors {
            let Some(item) = groups[group].get(position) else { continue };
            emitted.push(item.clone());
            if emitted.len() == limit {
                return emitted;
            }
            remaining.push((group, position + 1));
        }
        cursors = remaining;
    }
    emitted
}

#[allow(clippy::too_many_arguments)]
fn port_request<'a>(
    board: &'a PhysicalBoard,
    index: &'a RoutingClearanceIndex<'a>,
    first: &'a NetRoutingRule,
    second: &'a NetRoutingRule,
    positions: (Point, Point),
    component: &'a str,
    offset: i64,
    layer: CopperLayer,
    lengths: &'a [i64],
    allow_local_fallback: bool,
) -> PortRequest<'a> {
    PortRequest {
        board,
        index,
        first_name: &first.net,
        second_name: &second.net,
        first: positions.0,
        second: positions.1,
        component,
        width: width_of(board, first),
        offset,
        clearance: clearance_of(board, first, second),
        layer,
        lengths,
        allow_local_fallback,
    }
}

const OUTER_LENGTHS_NM: [i64; 4] = [4_000_000, 4_500_000, 5_000_000, 6_000_000];

/// Python's `sum(hypot(...) for s in segments)` (compensated, as Python 3.12+ sums floats).
fn track_sum(port: &Port) -> f64 {
    copper_physical::pyfloat::sum(
        port.first
            .iter()
            .chain(&port.second)
            .map(|s| hypot((s.end.x_nm - s.start.x_nm) as f64, (s.end.y_nm - s.start.y_nm) as f64)),
    )
}

/// Keep at most 16 transitions per (heading, sign) domain, short and outer interleaved.
fn select_domains(ordered: Vec<Transition>) -> Vec<Transition> {
    let mut domains: IndexMap<(usize, i64), [Vec<usize>; 2]> = IndexMap::new();
    for (position, item) in ordered.iter().enumerate() {
        domains.entry((item.port.heading, item.port.sign)).or_default()[usize::from(item.outer)].push(position);
    }
    let mut selected: IndexSet<usize> = IndexSet::new();
    for families in domains.values() {
        selected.extend(interleave(families, 16));
    }
    ordered.into_iter().enumerate().filter(|(position, _)| selected.contains(position)).map(|(_, item)| item).collect()
}

/// `_transitions`: the exact paired transitions from `surface` to `layer`
/// at one terminal pair.
#[allow(clippy::too_many_arguments)]
fn transitions(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    positions: (Point, Point),
    component: &str,
    surface: CopperLayer,
    layer: CopperLayer,
    include_outer: bool,
    trace: &mut TransitionTrace,
    stats: &mut PairSearchStats,
) -> Result<Vec<Transition>> {
    let width = width_of(board, first);
    let gap = first.pair_gap_nm.expect("profiled gap");
    let offset = (width + gap + 1).div_euclid(2);
    let index = RoutingClearanceIndex::new(board)?;
    if surface == layer {
        let mut results = Vec::new();
        for family in 0..(if include_outer { 2 } else { 1 }) {
            let (lengths, local): (&[i64], bool) =
                if family == 1 { (&OUTER_LENGTHS_NM, false) } else { (&PORT_LENGTHS_NM, true) };
            let request =
                port_request(board, &index, first, second, positions, component, offset, surface, lengths, local);
            results.extend(request.ports()?.into_iter().map(|port| Transition {
                port,
                vias: Vec::new(),
                returns: Vec::new(),
                rank: 0,
                outer: family == 1,
            }));
        }
        return Ok(select_domains(results));
    }
    let Some(span) = crate::routing_vias::physical_via_span(board, surface, layer) else { return Ok(Vec::new()) };
    let wide_offset = (transition_spacing(board, first, second) + 1).div_euclid(2);
    let mut wide =
        port_request(board, &index, first, second, positions, component, wide_offset, surface, &PORT_LENGTHS_NM, true)
            .ports()?;
    let mut results: Vec<Transition> = Vec::new();
    let mut rejections: IndexMap<&str, i64> = IndexMap::new();
    for family in 0..3 {
        if family == 1 {
            if !results.is_empty() && !include_outer {
                break;
            }
            wide = port_request(
                board,
                &index,
                first,
                second,
                positions,
                component,
                wide_offset,
                surface,
                &OUTER_LENGTHS_NM,
                false,
            )
            .ports()?;
        } else if family == 2 {
            if !results.is_empty() {
                break;
            }
            wide = local_via_ports(board, first, second, positions.0, positions.1, component, surface, layer, trace)?;
        }
        trace.count(&format!("{}_surface_collars", ["short", "outer", "local"][family]), wide.len() as i64);
        for collar in &wide {
            *rejections.entry("collars").or_default() += 1;
            let pair = vec![
                via(board, &first.net, collar.first.last().expect("collar copper").end, span.clone()),
                via(board, &second.net, collar.second.last().expect("collar copper").end, span.clone()),
            ];
            let sites: Vec<Point> = pair.iter().map(|v| v.position).collect();
            if !signal_drills_outside_smd(board, &pair)? {
                trace.record("signal_drill_in_smd", &sites, None, &[], &[], None)?;
                *rejections.entry("drill_over_smd").or_default() += 1;
                continue;
            }
            let mut collar_copper = collar.first.clone();
            collar_copper.extend(collar.second.iter().cloned());
            let escaped = with_copper(board, &collar_copper, &[]);
            let mut reserved = RoutingClearanceIndex::new(&escaped)?;
            if !reserve_vias(board, &mut reserved, &pair)? {
                let fresh = RoutingClearanceIndex::new(&escaped)?;
                trace.record("signal_via_clearance", &sites, Some(&fresh), &pair, &[], None)?;
                *rejections.entry("signal_vias_blocked").or_default() += 1;
                continue;
            }
            let Some(returns) = return_via(first, second)? else {
                trace.record("return_unavailable", &sites, None, &[], &[], None)?;
                *rejections.entry("return_via_blocked").or_default() += 1;
                continue;
            };
            let mut transition_vias = pair.clone();
            transition_vias.extend(returns.iter().cloned());
            let mut held = escaped.clone();
            held.vias = board.vias.iter().cloned().chain(transition_vias.iter().cloned()).collect();
            let held_index = RoutingClearanceIndex::new(&held)?;
            let inner_ports = port_request(
                &held,
                &held_index,
                first,
                second,
                (pair[0].position, pair[1].position),
                component,
                offset,
                layer,
                &PORT_LENGTHS_NM,
                true,
            )
            .ports()?;
            trace.count("middle_layer_collars", inner_ports.len() as i64);
            if inner_ports.is_empty() {
                trace.record("middle_layer_blocked", &sites, None, &[], &[], None)?;
                *rejections.entry("onward_collars_blocked").or_default() += 1;
            }
            for inner in inner_ports {
                let mut first_lane = collar.first.clone();
                first_lane.extend(inner.first.iter().cloned());
                let mut second_lane = collar.second.clone();
                second_lane.extend(inner.second.iter().cloned());
                results.push(Transition {
                    port: Port {
                        center: inner.center,
                        heading: inner.heading,
                        sign: inner.sign,
                        first: first_lane,
                        second: second_lane,
                        preference: collar.preference + inner.preference,
                    },
                    vias: pair.clone(),
                    returns: returns.clone(),
                    rank: 0,
                    outer: family != 0,
                });
            }
        }
    }
    if results.is_empty() {
        let mut counts: Vec<(&&str, &i64)> = rejections.iter().collect();
        counts.sort();
        let listed: Vec<String> = counts.iter().map(|(key, count)| format!("{key}={count}")).collect();
        let message =
            format!("{component} {} to {}: no exact transition; {}", surface.value(), layer.value(), listed.join(", "));
        if !stats.terminal_port_rejections.contains(&message) {
            stats.terminal_port_rejections.push(message);
        }
        stats.terminal_port_rejections.truncate(4);
    }
    let mut ordered = results;
    ordered.sort_by(|a, b| {
        let key =
            |t: &Transition| (t.rank, t.port.preference, track_sum(&t.port), t.port.center.x_nm, t.port.center.y_nm);
        key(a).partial_cmp(&key(b)).expect("finite keys")
    });
    Ok(select_domains(ordered))
}

/// `_facing_turns`: how far each collar turns from facing the other.
fn facing_turns(first: &Port, second: &Port) -> usize {
    let turns = |port: &Port, target: &Port| {
        let (dx, dy) = (target.center.x_nm - port.center.x_nm, target.center.y_nm - port.center.y_nm);
        let mut desired = 0usize;
        let mut best = f64::NEG_INFINITY;
        for (h, (hx, hy)) in HEADS.iter().enumerate() {
            let value = (hx * dx + hy * dy) as f64 / hypot(*hx as f64, *hy as f64);
            if value > best {
                best = value;
                desired = h;
            }
        }
        ((port.heading + 8 - desired) % 8).min((desired + 8 - port.heading) % 8)
    };
    turns(first, second) + turns(second, first)
}

/// `_transition_pairs`: compatible transition pairs, family by family, ranked.
fn transition_pairs(
    groups: &[Vec<Transition>],
    departure_rank: &mut dyn FnMut(&Transition) -> Result<i64>,
) -> Result<(usize, Vec<(Transition, Transition)>)> {
    let keys = [(false, false), (true, true), (false, true), (true, false)];
    let mut families: Vec<Vec<(Transition, Transition)>> = vec![Vec::new(); 4];
    for a in &groups[0] {
        for b in &groups[1] {
            if a.port.sign == -b.port.sign {
                let family = keys.iter().position(|key| *key == (a.outer, b.outer)).expect("a family");
                families[family].push((a.clone(), b.clone()));
            }
        }
    }
    let mut ordered = Vec::new();
    for family in families {
        let mut ranked = Vec::new();
        for (a, b) in family {
            let departure = departure_rank(&a)? + departure_rank(&b)?;
            let key = (
                a.rank + b.rank,
                departure,
                facing_turns(&a.port, &b.port) as i64,
                a.port.preference + b.port.preference,
                hypot(
                    (a.port.center.x_nm - b.port.center.x_nm) as f64,
                    (a.port.center.y_nm - b.port.center.y_nm) as f64,
                ),
                a.port.center.x_nm,
                a.port.center.y_nm,
                b.port.center.x_nm,
                b.port.center.y_nm,
            );
            ranked.push((key, (a, b)));
        }
        ranked.sort_by(|x, y| x.0.partial_cmp(&y.0).expect("finite keys"));
        ordered.push(ranked.into_iter().map(|(_, pair)| pair).collect::<Vec<_>>());
    }
    let total = ordered.iter().map(Vec::len).sum();
    Ok((total, interleave(&ordered, 128)))
}

/// `paired_via_candidates`: matched terminal transitions and the joint
/// middle-layer search; the owner checks DRC and profiles.
#[allow(clippy::too_many_arguments)]
pub fn paired_via_candidates(
    board: &PhysicalBoard,
    first: &NetRoutingRule,
    second: &NetRoutingRule,
    first_guide: &GlobalNetRoute,
    second_guide: &GlobalNetRoute,
    bounds: SearchBounds,
    stats: &mut PairSearchStats,
    accept: &mut dyn FnMut(PairSearchCandidate, &mut PairSearchStats) -> Result<bool>,
) -> Result<bool> {
    if bounds.maximum_searches.min(bounds.maximum_states).min(bounds.pitch_nm) <= 0 {
        return Err(crate::Error::Value("paired via search bounds must be positive".to_owned()));
    }
    if bounds.maximum_total_states.is_some_and(|total| total <= 0) {
        return Err(crate::Error::Value("paired via aggregate state bound must be positive".to_owned()));
    }
    if bounds.maximum_total_states.is_some_and(|total| stats.expanded_states >= total) {
        return Ok(false);
    }
    let width = width_of(board, first);
    if width_of(board, second) != width
        || first.pair_gap_nm.is_none()
        || second.pair_gap_nm != first.pair_gap_nm
        || first_guide.accesses.len() != 2
        || second_guide.accesses.len() != 2
    {
        return Ok(false);
    }
    let Some(partners) = terminal_partners(first_guide, second_guide) else { return Ok(false) };
    let mut accesses: Vec<_> = first_guide.accesses.iter().collect();
    accesses.sort_by(|a, b| a.pad.component.cmp(&b.pad.component));
    let second_layers = routing_layers(board, &second.net, Some(second));
    let allowed: IndexSet<CopperLayer> = routing_layers(board, &first.net, Some(first))
        .into_iter()
        .filter(|layer| second_layers.contains(layer))
        .collect();
    let mut domains: Vec<IndexSet<CopperLayer>> = Vec::new();
    for access in &accesses {
        let partner = &partners[&format!("pad:{}", access.pad)];
        let theirs = terminal_copper_layers(board, &partner.pad)?;
        domains.push(
            terminal_copper_layers(board, &access.pad)?
                .into_iter()
                .filter(|layer| theirs.contains(layer) && allowed.contains(layer))
                .collect(),
        );
    }
    if domains.iter().any(IndexSet::is_empty) {
        return Ok(false);
    }
    let (ranks, _) = signal_layer_preferences(board);
    let order = &board.stackup.copper_layers;
    let common: Vec<CopperLayer> = allowed.iter().copied().filter(|l| domains.iter().all(|d| d.contains(l))).collect();
    let primary = common.iter().copied().min_by_key(|layer| crate::layer_index(order, *layer));
    let mut targets: Vec<CopperLayer> = allowed
        .iter()
        .copied()
        .filter(|layer| Some(*layer) != primary)
        .filter(|layer| {
            let needed = domains.iter().filter(|domain| !domain.contains(layer)).count() as i64;
            [first, second].iter().all(|rule| rule.max_vias.is_none_or(|limit| limit >= needed))
        })
        .collect();
    targets.sort_by_key(|layer| {
        (
            domains.iter().filter(|domain| !domain.contains(layer)).count(),
            ranks[layer],
            crate::layer_index(order, *layer),
        )
    });
    let clearance = clearance_of(board, first, second);
    let offset = (width + first.pair_gap_nm.expect("gap") + 1).div_euclid(2);
    for layer in targets {
        if remaining_states(stats, bounds.maximum_states, bounds.maximum_total_states) == 0 {
            break;
        }
        let surfaces: Vec<CopperLayer> = domains
            .iter()
            .map(|domain| {
                if domain.contains(&layer) {
                    layer
                } else {
                    *domain.iter().min_by_key(|l| crate::layer_index(order, **l)).expect("a domain")
                }
            })
            .collect();
        let mut traces: Vec<TransitionTrace> = Vec::new();
        let mut groups: Vec<Vec<Transition>> = Vec::new();
        for (access, surface) in accesses.iter().zip(&surfaces) {
            let mut trace = TransitionTrace::default();
            let partner = &partners[&format!("pad:{}", access.pad)];
            groups.push(transitions(
                board,
                first,
                second,
                (access.pad_position, partner.pad_position),
                &access.pad.component,
                *surface,
                layer,
                true,
                &mut trace,
                stats,
            )?);
            traces.push(trace);
        }
        for (((access, surface), group), trace) in accesses.iter().zip(&surfaces).zip(&groups).zip(&traces) {
            if stats.transition_inventory.len() < 24 {
                stats.transition_inventory.push(PairTransitionInventory {
                    component: access.pad.component.clone(),
                    surface: *surface,
                    middle_layer: layer,
                    pitch_nm: bounds.pitch_nm,
                    transitions: group.len() as i64,
                    direct_contacts: group.iter().filter(|item| item.vias.is_empty()).count() as i64,
                    stage_counts: trace.sorted_counts(),
                    examples: trace.examples.clone(),
                });
            }
        }
        let mut departure_cache: Vec<(Transition, i64)> = Vec::new();
        let mut departure_rank = |item: &Transition| -> Result<i64> {
            if let Some((_, rank)) = departure_cache.iter().find(|(known, _)| {
                known.port == item.port
                    && known.vias == item.vias
                    && known.returns == item.returns
                    && known.outer == item.outer
            }) {
                return Ok(*rank);
            }
            let mut copper = item.port.first.clone();
            copper.extend(item.port.second.iter().cloned());
            let mut extra = item.vias.clone();
            extra.extend(item.returns.iter().cloned());
            let held = with_copper(board, &copper, &extra);
            let index = RoutingClearanceIndex::new(&held)?;
            let rank = i64::from(!can_depart(
                &held,
                &index,
                &first.net,
                &second.net,
                &item.port,
                width,
                offset,
                clearance,
                layer,
                bounds.pitch_nm,
            )?);
            departure_cache.push((item.clone(), rank));
            Ok(rank)
        };
        let (total_pairs, combinations) = transition_pairs(&groups, &mut departure_rank)?;
        if total_pairs == 0 {
            let parts: Vec<String> = accesses
                .iter()
                .zip(&surfaces)
                .zip(&groups)
                .map(|((access, surface), group)| {
                    let mut signs: Vec<i64> =
                        group.iter().map(|t| t.port.sign).collect::<IndexSet<i64>>().into_iter().collect();
                    signs.sort_unstable();
                    let orders = if signs.is_empty() {
                        "none".to_owned()
                    } else {
                        signs.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
                    };
                    format!(
                        "{} {} to {}: {} exact transitions, lane orders {orders}",
                        access.pad.component,
                        surface.value(),
                        layer.value(),
                        group.len()
                    )
                })
                .collect();
            let message = parts.join("; ");
            if !stats.terminal_port_rejections.contains(&message) {
                stats.terminal_port_rejections.push(message);
            }
            stats.terminal_port_rejections.truncate(4);
        }
        stats.port_pairs += total_pairs as i64;
        let searches_at_entry = stats.searches;
        for (ordinal, (a, b)) in combinations.iter().take(128).enumerate() {
            if stats.searches - searches_at_entry >= bounds.maximum_searches {
                break;
            }
            let budget = remaining_states(stats, bounds.maximum_states, bounds.maximum_total_states);
            if budget == 0 {
                break;
            }
            let (start, end) = (&a.port, &b.port);
            let mut first_escapes = start.first.clone();
            first_escapes.extend(end.first.iter().cloned());
            let mut second_escapes = start.second.clone();
            second_escapes.extend(end.second.iter().cloned());
            let plain = RoutingClearanceIndex::new(board)?;
            if !legal(board, &plain, &first_escapes, &second_escapes, clearance)? {
                continue;
            }
            let mut escape_copper = first_escapes.clone();
            escape_copper.extend(second_escapes.iter().cloned());
            let escaped_tracks = with_copper(board, &escape_copper, &[]);
            let mut transition_vias: Vec<Via> = a.vias.iter().chain(&b.vias).cloned().collect();
            transition_vias.extend(a.returns.iter().chain(&b.returns).cloned());
            let mut reserved = RoutingClearanceIndex::new(&escaped_tracks)?;
            if !reserve_vias(board, &mut reserved, &transition_vias)? {
                continue;
            }
            let mut escaped = escaped_tracks.clone();
            escaped.vias = board.vias.iter().cloned().chain(transition_vias.iter().cloned()).collect();
            let escaped_index = RoutingClearanceIndex::new(&escaped)?;
            if !can_depart(
                &escaped,
                &escaped_index,
                &first.net,
                &second.net,
                start,
                width,
                offset,
                clearance,
                layer,
                bounds.pitch_nm,
            )? {
                let (dx, dy) = HEADS[start.heading];
                let ahead =
                    Point::new(start.center.x_nm + dx * bounds.pitch_nm, start.center.y_nm + dy * bounds.pitch_nm);
                let lane_normal = normal(start.heading, offset);
                let mut launch = tracks(
                    &first.net,
                    &[shift(start.center, lane_normal, start.sign), shift(ahead, lane_normal, start.sign)],
                    width,
                    layer,
                    Some(&escaped_index.breakout),
                );
                launch.extend(tracks(
                    &second.net,
                    &[shift(start.center, lane_normal, -start.sign), shift(ahead, lane_normal, -start.sign)],
                    width,
                    layer,
                    Some(&escaped_index.breakout),
                ));
                stats.reject_launch(&escaped_index, &launch)?;
                continue;
            }
            stats.searches += 1;
            let via_pairs: Vec<(Via, Via)> = [&a.vias, &b.vias]
                .iter()
                .filter(|pair| !pair.is_empty())
                .map(|pair| (pair[0].clone(), pair[1].clone()))
                .collect();
            let return_vias: Vec<Via> = a.returns.iter().chain(&b.returns).cloned().collect();
            let mut continuation = Continuation {
                start: start.clone(),
                end: end.clone(),
                layer,
                pitch: bounds.pitch_nm,
                search_index: stats.searches,
                search: Some(SpineSearch::new(
                    &first.net,
                    &second.net,
                    start,
                    end,
                    width,
                    offset,
                    clearance,
                    bounds.pitch_nm,
                )),
                registered: false,
                via_pairs: via_pairs.clone(),
                return_vias: return_vias.clone(),
                active: true,
                resumed: false,
                expanded_states: 0,
                legal_advances: 0,
                minimum_goal_distance_nm: f64::INFINITY,
                board: escaped,
            };
            let found = continuation.advance(budget, stats)?;
            if bounds.maximum_total_states.is_some() {
                continuation.registered = true;
                stats.continuations.push(continuation);
            }
            let Some(found) = found else { continue };
            stats.candidates += 1;
            let mut first_lane = start.first.clone();
            first_lane.extend(found.first);
            first_lane.extend(end.first.iter().rev().map(|t| TrackSegment {
                net: t.net.clone(),
                start: t.end,
                end: t.start,
                width_nm: t.width_nm,
                layer: t.layer,
            }));
            let mut second_lane = start.second.clone();
            second_lane.extend(found.second);
            second_lane.extend(end.second.iter().rev().map(|t| TrackSegment {
                net: t.net.clone(),
                start: t.end,
                end: t.start,
                width_nm: t.width_nm,
                layer: t.layer,
            }));
            let candidate = PairSearchCandidate {
                first: first_lane,
                second: second_lane,
                expanded_states: found.expanded_states,
                search_index: ordinal as i64 + 1,
                spine: Vec::new(),
                start_port: None,
                end_port: None,
                via_pairs,
                return_vias,
            };
            if accept(candidate, stats)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

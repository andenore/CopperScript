//! `route_unified`: every ordinary net routed with one negotiated-congestion
//! loop, then rip-in repair rounds, smoothing, budgets and the congestion
//! export.

use indexmap::{IndexMap, IndexSet};

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_json::{Object, Value};
use copper_pcb::board::{PadReference, PhysicalBoard, PhysicalNet, RouteKind, TrackSegment, Via};
use copper_pcb::keepouts::placement_map;
use copper_pcb::transform::transformed_local_point;
use copper_pcb::zones::{deferred_zone_nets, routing_layers};
use copper_physical::Point;
use copper_physical::pyfloat::{hypot, round_int};

use super::negotiate::{Accepted, Event};
use super::options::UnifiedRouterOptions;
use super::rasters::FREE;
use super::router::Router;
use crate::congestion_feedback::CongestionMap;
use crate::detailed::{
    DetailedNetResult, DetailedRoutingMetrics, DetailedRoutingResult, DetailedRoutingStatus, fingerprint,
};
use crate::pin_escape::RoutingAccess;
use crate::progress::Progress;
use crate::routing::GlobalRoutingResult;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::unified_error::{Result, fail, not_implemented};

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn emit(on_progress: &mut Option<&mut dyn Progress>, phase: &str, event: &str, details: Vec<(&'static str, Value)>) {
    if let Some(callback) = on_progress {
        callback.observe(phase, event, details);
    }
}

fn forward(on_progress: &mut Option<&mut dyn Progress>, events: &mut Vec<Event>) {
    for (phase, event, details) in events.drain(..) {
        emit(on_progress, phase, event, details);
    }
}

/// `_with`: `board` plus the accepted copper of every net, by name.
fn with_copper(board: &PhysicalBoard, accepted: &Accepted) -> PhysicalBoard {
    let mut names: Vec<&String> = accepted.keys().collect();
    names.sort();
    let mut result = board.clone();
    for name in &names {
        result.tracks.extend(accepted[*name].0.iter().cloned());
    }
    for name in &names {
        result.vias.extend(accepted[*name].1.iter().cloned());
    }
    result
}

/// `_net_track_length`: routed length as DRC measures it.
fn net_track_length(board: &PhysicalBoard, name: &str, new_tracks: &[TrackSegment]) -> i64 {
    board
        .tracks
        .iter()
        .filter(|t| t.net == name)
        .chain(new_tracks)
        .map(|t| round_int(hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64)))
        .sum()
}

/// `_open_count`: (open signal connections, incomplete plane nets).
fn open_count(
    board: &PhysicalBoard,
    nets: &[&PhysicalNet],
    accepted: &Accepted,
    plane_nets: &IndexSet<String>,
    complete: &IndexSet<String>,
) -> Result<(i64, i64)> {
    let names: IndexSet<String> = nets.iter().map(|n| n.name.clone()).collect();
    let candidate = with_copper(board, accepted);
    let graph =
        explicit_copper_connectivity(&candidate, Candidate { only_nets: Some(&names), ..Candidate::default() })?;
    let mut opens = 0;
    for net in nets {
        if plane_nets.contains(&net.name) {
            continue;
        }
        let mut roots: IndexSet<&String> = IndexSet::new();
        let mut isolated = 0;
        for pad in &net.pads {
            match graph.pad_nodes.get(pad).and_then(|nodes| nodes.first()) {
                Some(node) => {
                    roots.insert(&graph.roots[node]);
                }
                None => isolated += 1,
            }
        }
        opens += 0.max(roots.len() as i64 + isolated - 1);
    }
    let planes = nets.iter().filter(|n| plane_nets.contains(&n.name) && !complete.contains(&n.name)).count() as i64;
    Ok((opens, planes))
}

/// `_neighbour_nets`: nets with accepted new copper within `reach` of the given nets' pads.
fn neighbour_nets(board: &PhysicalBoard, nets: &[&PhysicalNet], accepted: &Accepted, reach: i64) -> IndexSet<String> {
    let poses = placement_map(board);
    let names: IndexSet<&str> = nets.iter().map(|n| n.name.as_str()).collect();
    let mut points = Vec::new();
    for net in nets {
        for pad in &net.pads {
            let pose = &poses[pad.component.as_str()];
            for land in &board.footprints[&pose.footprint].pads {
                if land.number == pad.pad {
                    points.push(transformed_local_point(pose, land.position));
                }
            }
        }
    }
    let mut result = IndexSet::new();
    for (name, (tracks, vias)) in accepted {
        if names.contains(name.as_str()) {
            continue;
        }
        let ends: Vec<Point> =
            tracks.iter().flat_map(|t| [t.start, t.end]).chain(vias.iter().map(|v| v.position)).collect();
        if ends
            .iter()
            .any(|a| points.iter().any(|b| (a.x_nm - b.x_nm).abs() <= reach && (a.y_nm - b.y_nm).abs() <= reach))
        {
            result.insert(name.clone());
        }
    }
    result
}

/// `_SharedIndex`: one exact index of a base board plus a chosen part of this call's copper.
struct Shared {
    held: IndexMap<String, (Vec<TrackSegment>, Vec<Via>)>,
    frozen: IndexSet<String>,
}

impl Shared {
    fn added(&mut self, more: &Accepted) {
        for (name, (tracks, vias)) in more {
            let entry = self.held.entry(name.clone()).or_default();
            entry.0.extend(tracks.iter().cloned());
            entry.1.extend(vias.iter().cloned());
        }
    }

    /// `freeze`: the copper held now becomes part of the base.
    fn freeze(&mut self) {
        let names: Vec<String> = self.held.keys().cloned().collect();
        self.frozen.extend(names);
        self.held.clear();
    }

    fn hold(&mut self, index: &mut RoutingClearanceIndex<'_>, target: &Accepted) -> Result<()> {
        let mut names: Vec<String> =
            self.held.keys().chain(target.keys()).cloned().collect::<IndexSet<_>>().into_iter().collect();
        names.sort();
        let empty = (Vec::new(), Vec::new());
        for name in names {
            let old = self.held.get(&name).unwrap_or(&empty);
            let new = target.get(&name).unwrap_or(&empty);
            if old == new {
                continue;
            }
            if self.frozen.contains(&name) {
                return Err(crate::unified_error::Error::Crash(
                    "AssertionError".to_owned(),
                    format!("net {name} has frozen copper in the shared index"),
                ));
            }
            index.remove(&name);
            if index.owned_count(&name) != 0 {
                return Err(crate::unified_error::Error::Crash(
                    "AssertionError".to_owned(),
                    format!("ripped net {name} keeps mutable copper in the shared index"),
                ));
            }
            for track in &new.0 {
                index.add_track(track.clone(), false, None)?;
            }
            for via in &new.1 {
                index.add_via(via.clone(), false, None)?;
            }
        }
        self.held = target
            .iter()
            .filter(|(_, (t, v))| !t.is_empty() || !v.is_empty())
            .map(|(n, c)| (n.clone(), c.clone()))
            .collect();
        Ok(())
    }
}

/// `_rejection_text`: where the exact commit check rejected a net.
fn rejection_text(router: &Router<'_, '_>, name: &str) -> String {
    let Some(details) = router.rejections.get(name).filter(|d| !d.is_empty()) else {
        return "unified: rejected by exact commit check".to_owned();
    };
    let (at, layer, blockers, fixed, reason) = &details[0];
    let by = if !blockers.is_empty() {
        blockers.join(", ")
    } else if *fixed {
        "fixed copper".to_owned()
    } else {
        "board geometry".to_owned()
    };
    let reason = if reason.is_empty() { String::new() } else { format!(" [{reason}]") };
    let more = if details.len() > 1 { format!(" (+{} more)", details.len() - 1) } else { String::new() };
    format!(
        "unified: rejected by exact commit check: blocked by {by} at ({:.2},{:.2}) {layer}{reason}{more}",
        at.x_nm as f64 / 1e6,
        at.y_nm as f64 / 1e6
    )
}

/// `_failed_points`: where each failed net was blocked or could not be reached.
fn failed_points(router: &Router<'_, '_>, failed: &[String]) -> Vec<(String, Point)> {
    let mut names: Vec<&String> = failed.iter().collect();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        let mut points: Vec<Point> = router.rejections.get(name).into_iter().flatten().map(|record| record.0).collect();
        let spec = router.specs.iter().find(|s| &s.name == name);
        let route = router.routes.get(name);
        if let Some(spec) = spec {
            let disconnected = route.is_none_or(|r| !r.connected);
            let empty = IndexSet::new();
            let touched = route.map_or(&empty, |r| &r.nodes);
            for (component, anchors) in spec.components.iter().zip(&spec.anchors) {
                if !component.is_empty() && !disconnected {
                    continue;
                }
                if !component.is_empty()
                    && (component.iter().any(|(node, _, _)| touched.contains(node))
                        || component.iter().any(|(node, _, _)| spec.plane.contains(node)))
                {
                    continue;
                }
                points.extend(anchors.iter().copied());
            }
        }
        let unique: IndexSet<Point> = points.into_iter().collect();
        result.extend(unique.into_iter().map(|p| (name.clone(), p)));
    }
    result
}

/// `_congestion_map`: per-tile contention of the router's final state.
fn congestion_map(router: &mut Router<'_, '_>, failed: &[String], tile_nm: i64) -> CongestionMap {
    let (nl, plane) = (router.lattice.nl as usize, router.lattice.plane as usize);
    let mut node_demand = vec![0.0f64; plane];
    for (column, demand) in node_demand.iter_mut().enumerate() {
        let mut total: i64 = 0;
        for layer in 0..nl {
            total += 0.max(router.occupancy.occ[layer * plane + column] as i64 - 1);
        }
        *demand = total as f64;
        *demand += 0.max(router.occupancy.via_occ[column] as i64 - 1) as f64;
    }
    let increment = router.options.history_increment;
    if increment > 0.0 {
        let mut history = vec![0.0f64; plane];
        for layer in 0..nl {
            for (column, value) in history.iter_mut().enumerate() {
                *value += router.occupancy.history[layer * plane + column];
            }
        }
        for (_, columns, cost) in &router.seeded_history {
            for &column in columns {
                history[column as usize] -= cost;
            }
        }
        for (column, value) in history.iter_mut().enumerate() {
            *value += router.occupancy.via_history[column];
        }
        for (demand, value) in node_demand.iter_mut().zip(&history) {
            *demand += value.max(0.0) / increment;
        }
    }
    let g = &router.lattice;
    let tx: Vec<i64> = (0..g.nx).map(|i| (i * g.pitch).div_euclid(tile_nm)).collect();
    let ty: Vec<i64> = (0..g.ny).map(|i| (i * g.pitch).div_euclid(tile_nm)).collect();
    let (tiles_x, tiles_y) = (*tx.last().expect("nx") as usize + 1, *ty.last().expect("ny") as usize + 1);
    let mut tile_ids = Vec::with_capacity(plane);
    for &y in &ty {
        for &x in &tx {
            tile_ids.push(y as usize * tiles_x + x as usize);
        }
    }
    let count = tiles_x * tiles_y;
    let mut demand = vec![0.0f64; count];
    for (id, value) in tile_ids.iter().zip(&node_demand) {
        demand[*id] += value;
    }
    let width = router.default_width;
    let clearance = router.board.rules.minimum_clearance_nm;
    let signal: IndexSet<copper_physical::CopperLayer> = routing_layers(router.board, "", None).into_iter().collect();
    let layers: Vec<usize> =
        router.lattice.layers.iter().enumerate().filter(|(_, l)| signal.contains(*l)).map(|(i, _)| i).collect();
    let pitch = router.lattice.pitch as f64;
    let owner = router.rasters.track(width, clearance).0;
    let lane = (width + clearance) as f64 / pitch;
    let mut capacity = vec![0.0f64; count];
    for (column, id) in tile_ids.iter().enumerate() {
        let free = layers.iter().filter(|&&layer| owner[layer * plane + column] == FREE).count();
        capacity[*id] += free as f64 / lane;
    }
    let rows = |values: Vec<f64>| values.chunks(tiles_x).map(<[f64]>::to_vec).collect::<Vec<_>>();
    let g = &router.lattice;
    CongestionMap {
        tile_nm,
        x0_nm: g.x0,
        y0_nm: g.y0,
        nx: tiles_x,
        ny: tiles_y,
        demand: rows(demand),
        capacity: rows(capacity),
        source: "unified_router".to_owned(),
        failed_points: failed_points(router, failed),
    }
}

/// `route_unified` without a bus phase, plane nets or an escape seeder.
#[allow(clippy::too_many_arguments)]
pub fn route_unified(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &UnifiedRouterOptions,
    fanout_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    only_nets: Option<&IndexSet<String>>,
    required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    on_progress: Option<&mut dyn Progress>,
) -> Result<DetailedRoutingResult> {
    route_unified_seeded(
        board,
        global_route,
        options,
        fanout_accesses,
        only_nets,
        required_accesses,
        on_progress,
        false,
    )
}

/// `route_unified(..., escape_seeder=...)` where only the seeder's presence
/// is known: the seeded repair attempts are not rewritten, so a run that
/// reaches a repair round with a seeder reports not implemented.
// PENDING: owned by rust-m4-unified (the escape_seeder repair attempts).
#[allow(clippy::too_many_arguments)]
pub fn route_unified_seeded(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &UnifiedRouterOptions,
    fanout_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    only_nets: Option<&IndexSet<String>>,
    required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    mut on_progress: Option<&mut dyn Progress>,
    escape_seeder: bool,
) -> Result<DetailedRoutingResult> {
    if let Some(fanout) = fanout_accesses
        && fanout.iter().any(|(pad, access)| required_accesses.is_none_or(|r| r.get(pad) != Some(access)))
    {
        return fail("unified router does not consume verified fanout accesses; pass required_accesses");
    }
    if options.time_budget_s.is_some_and(|budget| budget != 0.0) {
        // A wall-clock deadline makes the result depend on timing.
        return not_implemented("unified router time_budget_s (a wall-clock deadline)");
    }
    if options.save_inputs_path.is_some() {
        return not_implemented("unified router save_inputs_path");
    }
    if !board.hard_macros.is_empty() {
        let materialized: IndexSet<&String> = board.materialized_macros.iter().collect();
        let clusters: IndexSet<&String> = board.hard_macros.iter().map(|m| &m.cluster).collect();
        if materialized != clusters {
            return fail("materialize hard-macro copper before detailed routing");
        }
    }
    let rules: IndexMap<&str, &copper_pcb::board::NetRoutingRule> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let zone_nets = deferred_zone_nets(board)?;
    let connectivity = explicit_copper_connectivity(board, Candidate::default())?;
    let plane_nets: IndexSet<String> = board.zones.iter().map(|z| z.net.clone()).collect();
    let selected = |net: &PhysicalNet| {
        only_nets.is_none_or(|only| only.contains(&net.name))
            && (options.direct || rules.get(net.name.as_str()).is_none_or(|r| r.kind == RouteKind::General))
    };
    let mut nets: Vec<&PhysicalNet> = Vec::new();
    let mut already: Vec<&PhysicalNet> = Vec::new();
    for net in &board.nets {
        let connected = connectivity.net_connected(net);
        if (net.pads.len() >= 2 || plane_nets.contains(&net.name))
            && (!zone_nets.contains(&net.name) || plane_nets.contains(&net.name))
            && selected(net)
            && !connected
        {
            nets.push(net);
        }
        if net.pads.len() >= 2 && !zone_nets.contains(&net.name) && selected(net) && connected {
            already.push(net);
        }
    }
    let original_board = board;
    let all_nets = nets.clone();
    let bus_names: IndexSet<&str> = board
        .match_groups
        .iter()
        .flat_map(|g| g.nets.iter().map(String::as_str))
        .filter(|name| nets.iter().any(|n| n.name == *name))
        .collect();
    if options.buses_first && options.bus_bundles && !bus_names.is_empty() {
        return not_implemented("unified router bus bundles");
    }
    if !options.share_commit_index {
        return not_implemented("unified router without share_commit_index");
    }
    let mut events: Vec<Event> = Vec::new();
    let mut shared = Shared { held: IndexMap::new(), frozen: IndexSet::new() };
    let mut bus_accepted: Accepted = IndexMap::new();
    let mut bus_complete: IndexSet<String> = IndexSet::new();
    let mut bus_skew_notes: Vec<String> = Vec::new();
    let mut index = RoutingClearanceIndex::new(board)?;
    let bus_board;
    let mut board: &PhysicalBoard = original_board;
    if options.buses_first && !bus_names.is_empty() && bus_names.len() < nets.len() {
        let mut bus_options = options.clone();
        if options.bus_length_budget {
            bus_options.length_priority = true;
        }
        let bus_nets: Vec<&PhysicalNet> =
            nets.iter().copied().filter(|n| bus_names.contains(n.name.as_str())).collect();
        let mut bus_router = Router::new(board, &bus_options, &bus_nets, IndexSet::new(), required_accesses, index)?;
        let (bus_routes, bus_conflicts, used) = bus_router.negotiate(None, &mut events)?;
        forward(&mut on_progress, &mut events);
        let (accepted_bus, _, complete_bus) = bus_router.commit(&bus_routes, &bus_conflicts)?;
        index = bus_router.clearance.take().expect("index");
        drop(bus_router);
        bus_accepted = accepted_bus;
        bus_complete = complete_bus;
        shared.added(&bus_accepted);
        let mut over_length: IndexSet<String> = IndexSet::new();
        if options.bus_length_budget {
            for name in &bus_complete {
                if let Some(rule) = rules.get(name.as_str())
                    && let Some(max_length) = rule.max_length_nm
                {
                    let tracks = bus_accepted.get(name).map_or(&[][..], |c| c.0.as_slice());
                    if net_track_length(board, name, tracks) > max_length {
                        over_length.insert(name.clone());
                    }
                }
            }
            bus_complete.retain(|name| !over_length.contains(name));
        }
        bus_accepted.retain(|name, _| bus_complete.contains(name));
        let mut skew_notes: Vec<String> = Vec::new();
        if options.bus_skew_tuning && !bus_accepted.is_empty() {
            if options.route_smoothing {
                bus_accepted = smoothed(board, &bus_accepted)?;
            }
            skew_notes = tune_bus_skew(board, &bus_accepted, &rules)?;
        }
        shared.hold(&mut index, &bus_accepted)?;
        shared.freeze();
        emit(
            &mut on_progress,
            "unified_buses",
            "finished",
            vec![
                ("nets", int(bus_names.len() as i64)),
                ("complete", int(bus_complete.len() as i64)),
                ("over_length", int(over_length.len() as i64)),
                ("iterations", int(used)),
                ("skew_notes", Value::Array(skew_notes.iter().map(|n| Value::from(n.as_str())).collect())),
            ],
        );
        bus_skew_notes = skew_notes;
        if !bus_accepted.is_empty() {
            bus_board = with_copper(board, &bus_accepted);
            board = &bus_board;
            nets.retain(|n| !bus_complete.contains(&n.name));
        }
    }
    emit(&mut on_progress, "unified_setup", "started", vec![("nets", int(nets.len() as i64))]);
    let mut router = Router::new(board, options, &nets, IndexSet::new(), required_accesses, index)?;
    if options.export_congestion && options.congestion_tile_nm < router.pitch {
        return fail("congestion_tile_nm must be at least the router pitch");
    }
    let lattice = &router.lattice;
    emit(
        &mut on_progress,
        "unified_setup",
        "finished",
        vec![
            ("nets", int(router.specs.len() as i64)),
            ("grid", Value::Array(vec![int(lattice.nx), int(lattice.ny), int(lattice.nl)])),
            ("seconds", Value::Float(0.0)),
        ],
    );
    let (routes, conflicts, mut iterations) = router.negotiate(None, &mut events)?;
    forward(&mut on_progress, &mut events);
    let (mut accepted, mut rejected, mut complete) = router.commit(&routes, &conflicts)?;
    shared.added(&accepted);
    emit(
        &mut on_progress,
        "unified_commit",
        "finished",
        vec![
            ("round", int(1)),
            ("accepted", int(accepted.len() as i64)),
            ("rejected", int(rejected.len() as i64)),
            ("conflicted", int(conflicts.len() as i64)),
            ("seconds", Value::Float(0.0)),
        ],
    );
    for round_index in 1..options.commit_rounds {
        if nets.iter().all(|net| complete.contains(&net.name)) {
            break;
        }
        if escape_seeder {
            return not_implemented("unified repair rounds with an escape seeder (escape_seeder)");
        }
        let baseline = open_count(board, &nets, &accepted, &plane_nets, &complete)?;
        // repair_attempt(seed=False)
        let mut work: Accepted = accepted.clone();
        let mut done = complete.clone();
        let remaining: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| !done.contains(&n.name)).collect();
        let mut crowd = neighbour_nets(board, &remaining, &work, options.repair_neighbour_reach_nm);
        let remaining_names: IndexSet<String> = remaining.iter().map(|n| n.name.clone()).collect();
        for name in &remaining_names {
            for (_, _, blockers, _, _) in router.rejections.get(name).into_iter().flatten() {
                for blocker in blockers {
                    if work.contains_key(blocker) && !plane_nets.contains(blocker) {
                        crowd.insert(blocker.clone());
                    }
                }
            }
        }
        for name in &crowd {
            work.shift_remove(name);
            done.shift_remove(name);
        }
        let remaining: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| !done.contains(&n.name)).collect();
        let partial = with_copper(board, &work);
        let mut index = router.clearance.take().expect("index");
        shared.hold(&mut index, &work)?;
        let fixed = explicit_copper_connectivity(&partial, Candidate::default())?;
        let remaining: Vec<&PhysicalNet> =
            remaining.into_iter().filter(|net| !fixed.net_connected(net) || plane_nets.contains(&net.name)).collect();
        let mut repair = Router::new(&partial, options, &remaining, IndexSet::new(), required_accesses, index)?;
        repair.history_seed(&router);
        if options.rejection_marker_cost != 0.0 {
            let mut names: Vec<&String> = remaining_names.iter().collect();
            names.sort();
            for name in names {
                let points: Vec<Point> = router.rejections.get(name).into_iter().flatten().map(|r| r.0).collect();
                if !points.is_empty() {
                    repair.penalise_points(&points, options.rejection_marker_cost);
                }
            }
        }
        let (more_routes, more_conflicts, used) = repair.negotiate(None, &mut events)?;
        forward(&mut on_progress, &mut events);
        let (more, more_rejected, more_complete) = repair.commit(&more_routes, &more_conflicts)?;
        shared.added(&more);
        for (name, (tracks, vias)) in &more {
            let entry = work.entry(name.clone()).or_default();
            entry.0.extend(tracks.iter().cloned());
            entry.1.extend(vias.iter().cloned());
        }
        done.extend(more_complete.iter().cloned());
        for net in &remaining {
            if !more_routes.contains_key(&net.name) && fixed.net_connected(net) && !plane_nets.contains(&net.name) {
                done.insert(net.name.clone());
            }
        }
        let still: IndexMap<String, Vec<Point>> =
            more_rejected.into_iter().filter(|(name, _)| !done.contains(name)).collect();
        let repair_rejections = repair.rejections.clone();
        let mut index = repair.clearance.take().expect("index");
        drop(repair);
        iterations += used;
        let score = open_count(board, &nets, &work, &plane_nets, &done)?;
        let improved = score < baseline;
        if improved {
            accepted = work;
            complete = done;
            rejected = still;
            for (name, records) in repair_rejections {
                router.rejections.insert(name, records);
            }
            for (name, route) in more_routes {
                router.routes.insert(name, route);
            }
        }
        shared.hold(&mut index, &accepted)?;
        router.clearance = Some(index);
        emit(
            &mut on_progress,
            "unified_commit",
            "finished",
            vec![
                ("round", int(round_index + 1)),
                ("adopted", Value::Bool(improved)),
                ("seeded", Value::Bool(false)),
                ("seconds", Value::Float(0.0)),
            ],
        );
        if !improved {
            break;
        }
    }
    if options.pour_recheck && !plane_nets.is_empty() {
        let mut slot = router.clearance.take();
        let (adopted, adopted_complete, report) = recheck_pours(
            board,
            &nets,
            &plane_nets,
            &accepted,
            complete.clone(),
            options,
            required_accesses,
            &mut shared,
            &mut slot,
            &mut events,
        )?;
        router.clearance = slot;
        forward(&mut on_progress, &mut events);
        accepted = adopted;
        complete = adopted_complete;
        iterations += report.0;
        emit(&mut on_progress, "unified_pour_recheck", "finished", report.1);
    }
    if !bus_accepted.is_empty() {
        accepted = merge(&bus_accepted, &accepted);
        complete.extend(bus_complete.iter().cloned());
        board = original_board;
        nets = all_nets.clone();
    }
    let mut spec_names: IndexSet<String> = router.specs.iter().map(|s| s.name.clone()).collect();
    spec_names.extend(bus_complete.iter().cloned());
    if options.route_smoothing && !accepted.is_empty() {
        let held: Accepted = if options.bus_skew_tuning {
            accepted.iter().filter(|(n, _)| bus_complete.contains(*n)).map(|(n, c)| (n.clone(), c.clone())).collect()
        } else {
            IndexMap::new()
        };
        let rest: Accepted =
            accepted.iter().filter(|(n, _)| !held.contains_key(*n)).map(|(n, c)| (n.clone(), c.clone())).collect();
        let mut merged = if rest.is_empty() { IndexMap::new() } else { smoothed(&with_copper(board, &held), &rest)? };
        for (name, copper) in held {
            merged.insert(name, copper);
        }
        accepted = merged;
    }
    let mut budget_errors: IndexMap<String, String> = IndexMap::new();
    let mut budget_warnings: IndexMap<String, String> = IndexMap::new();
    for net in nets.iter().chain(&already) {
        let Some(rule) = rules.get(net.name.as_str()) else { continue };
        if rule.max_length_nm.is_none() && rule.max_vias.is_none() {
            continue;
        }
        let (tracks, vias) = accepted.get(&net.name).cloned().unwrap_or_default();
        let count = board.vias.iter().filter(|v| v.net == net.name).count() as i64 + vias.len() as i64;
        if let Some(max_vias) = rule.max_vias
            && count > max_vias
        {
            budget_errors.insert(net.name.clone(), format!("via count {count} exceeds {max_vias}"));
            accepted.shift_remove(&net.name);
            continue;
        }
        let length = net_track_length(board, &net.name, &tracks);
        if let Some(max_length) = rule.max_length_nm
            && length > max_length
        {
            budget_warnings.insert(net.name.clone(), format!("route length {length} nm exceeds {max_length} nm"));
        }
    }
    let mut names: Vec<&String> = accepted.keys().collect();
    names.sort();
    let new_tracks: Vec<TrackSegment> = names.iter().flat_map(|n| accepted[*n].0.iter().cloned()).collect();
    let new_vias: Vec<Via> = names.iter().flat_map(|n| accepted[*n].1.iter().cloned()).collect();
    let mut routed_names: IndexSet<String> = complete.iter().cloned().collect();
    routed_names.extend(already.iter().map(|n| n.name.clone()));
    routed_names.retain(|n| !budget_errors.contains_key(n));
    let all: Vec<&PhysicalNet> = nets.iter().chain(&already).copied().collect();
    let unrouted: Vec<String> = all.iter().map(|n| n.name.clone()).filter(|n| !routed_names.contains(n)).collect();
    let success = unrouted.is_empty();
    let congestion = if options.export_congestion {
        let map = congestion_map(&mut router, &unrouted, options.congestion_tile_nm);
        let hot = map
            .demand
            .iter()
            .flatten()
            .zip(map.capacity.iter().flatten())
            .filter(|(demand, capacity)| **demand > 0.0 && (**capacity <= 0.0 || **demand / **capacity > 1.0))
            .count();
        emit(
            &mut on_progress,
            "unified_congestion",
            "finished",
            vec![
                ("tiles", int((map.nx * map.ny) as i64)),
                ("hot_tiles", int(hot as i64)),
                ("failed_points", int(map.failed_points.len() as i64)),
                ("seconds", Value::Float(0.0)),
            ],
        );
        Some(map)
    } else {
        None
    };
    let mut routed = board.clone();
    routed.tracks.extend(new_tracks.iter().cloned());
    routed.vias.extend(new_vias.iter().cloned());
    for (key, value) in [
        ("detailed_routing", if success { "complete" } else { "partial" }),
        ("detailed_router", "unified"),
        ("global_routing_fingerprint", global_route.routing_fingerprint.as_str()),
        ("fabrication_ready", "false"),
    ] {
        routed.metadata.insert(key.to_owned(), value.to_owned());
    }
    if !bus_skew_notes.is_empty() {
        routed.metadata.insert("bus_skew".to_owned(), bus_skew_notes.join(" | "));
    }
    let mut results = Vec::new();
    for net in &all {
        let (tracks, vias) = accepted.get(&net.name).cloned().unwrap_or_default();
        let routed_net = routed_names.contains(&net.name);
        let diagnostics = if let Some(error) = budget_errors.get(&net.name) {
            vec![error.clone()]
        } else if !routed_net {
            let last = router.routes.get(&net.name);
            if rejected.contains_key(&net.name) {
                vec![rejection_text(&router, &net.name)]
            } else if !spec_names.contains(&net.name) {
                vec!["unified: no routable layer".to_owned()]
            } else {
                match last {
                    Some(last) if !last.reason.is_empty() => vec![format!("unified: {}", last.reason)],
                    _ => vec!["unified: unresolved congestion".to_owned()],
                }
            }
        } else {
            Vec::new()
        };
        let length = copper_physical::pyfloat::sum(
            tracks.iter().map(|t| hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64)),
        );
        let warnings =
            if routed_net { budget_warnings.get(&net.name).cloned().into_iter().collect() } else { Vec::new() };
        results.push(DetailedNetResult {
            net: net.name.clone(),
            connected: routed_net,
            track_count: tracks.len() as i64,
            via_count: vias.len() as i64,
            length_nm: length as i64,
            guide_deviation_count: 0,
            diagnostics,
            orthogonal_mode_used: false,
            resumed_branch_count: 0,
            search_budget_exhausted: false,
            retry_heuristic_weight_percent: None,
            warnings,
        });
    }
    let metrics = DetailedRoutingMetrics {
        routed_net_count: results.iter().filter(|r| r.connected).count() as i64,
        unrouted_net_count: results.iter().filter(|r| !r.connected).count() as i64,
        conflict_resource_count: 0,
        total_conflict_overflow: 0,
        track_count: new_tracks.len() as i64,
        via_count: new_vias.len() as i64,
        total_length_nm: results.iter().map(|r| r.length_nm).sum(),
        passes: iterations,
    };
    emit(
        &mut on_progress,
        "unified_route",
        "finished",
        vec![
            ("routed", int(accepted.len() as i64)),
            ("unrouted", int(unrouted.len() as i64)),
            ("seconds", Value::Float(0.0)),
        ],
    );
    let routing_fingerprint = fingerprint(&global_route.routing_fingerprint, &routed, &metrics);
    Ok(DetailedRoutingResult {
        status: if success { DetailedRoutingStatus::Success } else { DetailedRoutingStatus::Partial },
        nets: results,
        metrics,
        locked_track_count: board.tracks.len() as i64,
        locked_via_count: board.vias.len() as i64,
        global_routing_fingerprint: global_route.routing_fingerprint.clone(),
        routing_fingerprint,
        congestion,
        board: routed,
    })
}

/// `_smoothed`: `accepted` with its tracks straightened against the final board.
fn smoothed(board: &PhysicalBoard, accepted: &Accepted) -> Result<Accepted> {
    let mut names: Vec<&String> = accepted.keys().collect();
    names.sort();
    let owned: Vec<TrackSegment> = names.iter().flat_map(|n| accepted[*n].0.iter().cloned()).collect();
    let mut vias: Vec<Via> = board.vias.clone();
    vias.extend(names.iter().flat_map(|n| accepted[*n].1.iter().cloned()));
    let mut index_board = board.clone();
    index_board.tracks.extend(owned.iter().cloned());
    index_board.vias = vias.clone();
    let mut clearance = RoutingClearanceIndex::new(&index_board)?;
    let mut smooth_board = board.clone();
    smooth_board.vias = vias;
    let tracks = crate::route_smoothing::smooth_owned_tracks(&smooth_board, &owned, &mut clearance, 2)?;
    let mut by_net: IndexMap<&String, Vec<TrackSegment>> = accepted.keys().map(|n| (n, Vec::new())).collect();
    for track in tracks {
        by_net.get_mut(&track.net).expect("accepted net").push(track);
    }
    Ok(accepted.iter().map(|(name, (_, vias))| (name.clone(), (by_net[name].clone(), vias.clone()))).collect())
}

/// `_merge`.
fn merge(accepted: &Accepted, more: &Accepted) -> Accepted {
    let mut merged = accepted.clone();
    for (name, (tracks, vias)) in more {
        let entry = merged.entry(name.clone()).or_default();
        entry.0.extend(tracks.iter().cloned());
        entry.1.extend(vias.iter().cloned());
    }
    merged
}

fn pour_estimate(board: &PhysicalBoard, accepted: &Accepted) -> Result<crate::plane_estimate::PlaneEstimate> {
    crate::plane_estimate::estimate_plane_connectivity(&with_copper(board, accepted))
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(v.as_str())).collect())
}

/// `_recheck_pours`: reconnect plane pads committed signal copper walled off
/// from their pour. Returns (accepted, complete, (iterations, report)).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn recheck_pours<'b>(
    board: &PhysicalBoard,
    nets: &[&PhysicalNet],
    plane_nets: &IndexSet<String>,
    accepted: &Accepted,
    mut complete: IndexSet<String>,
    options: &UnifiedRouterOptions,
    required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    shared: &mut Shared,
    slot: &mut Option<RoutingClearanceIndex<'b>>,
    events: &mut Vec<Event>,
) -> Result<(Accepted, IndexSet<String>, (i64, Vec<(&'static str, Value)>))> {
    let before = pour_estimate(board, accepted)?;
    let routed_planes: IndexSet<&str> =
        nets.iter().filter(|n| plane_nets.contains(&n.name)).map(|n| n.name.as_str()).collect();
    let cut: IndexMap<&String, &Vec<String>> =
        before.open_pads.iter().filter(|(name, _)| routed_planes.contains(name.as_str())).collect();
    let mut cut_pads: Vec<String> = cut.values().flat_map(|p| p.iter().cloned()).collect();
    cut_pads.sort();
    cut_pads.truncate(12);
    let mut iterations = 0;
    let mut steps: Vec<String> = Vec::new();
    let cut_off = cut.values().map(|p| p.len() as i64).sum::<i64>();
    if cut.is_empty() {
        return Ok((
            accepted.clone(),
            complete,
            (0, vec![("cut_off", int(cut_off)), ("pads", strings(&cut_pads)), ("steps", strings(&steps))]),
        ));
    }
    let signal_before = open_count(board, nets, accepted, plane_nets, &complete)?.0;
    let plane_list: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| cut.contains_key(&n.name)).collect();
    let mut current = (accepted.clone(), before);
    let mut adopted = false;
    // Step 1: plane nets only, everything else fixed.
    shared.hold(slot.as_mut().expect("index"), &current.0)?;
    let partial = with_copper(board, &current.0);
    let taken = slot.take().expect("index");
    let mut step = Router::new(&partial, options, &plane_list, IndexSet::new(), required_accesses, taken)?;
    let (routes, conflicts, used) = step.negotiate(None, events)?;
    iterations += used;
    let (more, _, _) = step.commit(&routes, &conflicts)?;
    *slot = step.clearance.take();
    drop(step);
    shared.added(&more);
    let candidate = merge(&current.0, &more);
    let estimate = pour_estimate(board, &candidate)?;
    let signal = open_count(board, nets, &candidate, plane_nets, &complete)?.0;
    let ok = estimate.opens < current.1.opens && signal <= signal_before;
    steps.push(format!(
        "plane reroute: pour opens {}->{}, signal opens {signal_before}->{signal}{}",
        current.1.opens,
        estimate.opens,
        if ok { "" } else { " (not kept)" }
    ));
    if ok {
        current = (candidate, estimate);
        adopted = true;
    }
    let still: IndexMap<String, Vec<String>> = current
        .1
        .open_pads
        .iter()
        .filter(|(name, _)| routed_planes.contains(name.as_str()))
        .map(|(n, p)| (n.clone(), p.clone()))
        .collect();
    let mut explicit_report: Option<Value> = None;
    if !still.is_empty() {
        let pad_of = |text: &String| {
            let (component, pad) = text.rsplit_once('.').expect("REF.PAD");
            PadReference::new(component, pad)
        };
        let pads: IndexSet<PadReference> = still.values().flatten().map(pad_of).collect();
        let mut names: Vec<&String> = still.keys().collect();
        names.sort();
        let focus: Vec<PhysicalNet> = names
            .iter()
            .map(|name| PhysicalNet { name: (*name).clone(), pads: still[*name].iter().map(pad_of).collect() })
            .collect();
        let focus_refs: Vec<&PhysicalNet> = focus.iter().collect();
        let crowd: IndexSet<String> = neighbour_nets(board, &focus_refs, &current.0, options.repair_neighbour_reach_nm)
            .into_iter()
            .filter(|name| !plane_nets.contains(name))
            .collect();
        let ripped: Accepted =
            current.0.iter().filter(|(name, _)| !crowd.contains(*name)).map(|(n, c)| (n.clone(), c.clone())).collect();
        let still_nets: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| still.contains_key(&n.name)).collect();
        shared.hold(slot.as_mut().expect("index"), &ripped)?;
        let partial = with_copper(board, &ripped);
        let taken = slot.take().expect("index");
        let mut first = Router::new(&partial, options, &still_nets, pads, required_accesses, taken)?;
        let (routes, conflicts, used) = first.negotiate(None, events)?;
        iterations += used;
        let (plane_more, plane_rejected, _) = first.commit(&routes, &conflicts)?;
        let mut explicit = Object::new();
        for name in still.keys() {
            let copper = plane_more.get(name).map_or((0, 0), |(t, v)| (t.len(), v.len()));
            let reason = routes.get(name).map_or("not routed".to_owned(), |r| r.reason.clone());
            let rejected = if plane_rejected.contains_key(name) { rejection_text(&first, name) } else { String::new() };
            explicit.insert(
                name.clone(),
                Value::Object(Object::from([
                    ("copper".to_owned(), Value::Array(vec![int(copper.0 as i64), int(copper.1 as i64)])),
                    ("reason".to_owned(), Value::from(reason.as_str())),
                    ("rejected".to_owned(), Value::from(rejected.as_str())),
                ])),
            );
        }
        explicit_report = Some(Value::Object(explicit));
        *slot = first.clearance.take();
        drop(first);
        shared.added(&plane_more);
        let candidate = merge(&ripped, &plane_more);
        let crowd_nets: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| crowd.contains(&n.name)).collect();
        shared.hold(slot.as_mut().expect("index"), &candidate)?;
        let partial = with_copper(board, &candidate);
        let taken = slot.take().expect("index");
        let mut second = Router::new(&partial, options, &crowd_nets, IndexSet::new(), required_accesses, taken)?;
        let (routes, conflicts, used) = second.negotiate(None, events)?;
        iterations += used;
        let (signal_more, _, signal_complete) = second.commit(&routes, &conflicts)?;
        *slot = second.clearance.take();
        drop(second);
        shared.added(&signal_more);
        let candidate = merge(&candidate, &signal_more);
        let estimate = pour_estimate(board, &candidate)?;
        let mut candidate_complete: IndexSet<String> =
            complete.iter().filter(|n| !crowd.contains(*n)).cloned().collect();
        candidate_complete.extend(signal_complete);
        let signal = open_count(board, nets, &candidate, plane_nets, &candidate_complete)?.0;
        let ok = estimate.opens < current.1.opens && signal <= signal_before;
        steps.push(format!(
            "explicit copper, {} net(s) ripped up: pour opens {}->{}, signal opens {signal_before}->{signal}{}",
            crowd.len(),
            current.1.opens,
            estimate.opens,
            if ok { "" } else { " (not kept)" }
        ));
        if ok {
            current = (candidate, estimate);
            complete = candidate_complete;
            adopted = true;
        }
    }
    shared.hold(slot.as_mut().expect("index"), &current.0)?;
    let mut still_cut: Vec<String> = current
        .1
        .open_pads
        .iter()
        .filter(|(name, _)| routed_planes.contains(name.as_str()))
        .flat_map(|(_, pads)| pads.iter().cloned())
        .collect();
    still_cut.sort();
    still_cut.truncate(12);
    let mut report = vec![("cut_off", int(cut_off)), ("pads", strings(&cut_pads)), ("steps", strings(&steps))];
    if let Some(explicit) = explicit_report {
        report.push(("explicit_route", explicit));
    }
    report.push(("adopted", Value::Bool(adopted)));
    report.push(("still_cut_off", strings(&still_cut)));
    Ok((current.0, complete, (iterations, report)))
}

/// `_tune_bus_skew` up to the tuner: the notes for members left over each
/// group's `max_skew`. Lengthening a member (`critical_tuning.UnitTuner`)
/// is not rewritten yet.
fn tune_bus_skew(
    board: &PhysicalBoard,
    accepted: &Accepted,
    rules: &IndexMap<&str, &copper_pcb::board::NetRoutingRule>,
) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    for group in &board.match_groups {
        let max_skew = group.max_skew_nm;
        let lengths: IndexMap<&str, i64> = group
            .nets
            .iter()
            .map(|name| {
                let tracks = accepted.get(name).map_or(&[][..], |c| c.0.as_slice());
                (name.as_str(), net_track_length(board, name, tracks))
            })
            .collect();
        if lengths.values().any(|length| *length == 0) {
            continue;
        }
        let longest = *lengths.values().max().expect("members");
        let mut members: Vec<&str> = group.nets.iter().map(String::as_str).collect();
        members.sort_by_key(|name| (lengths[name], name.to_string()));
        for name in members {
            let need = longest - max_skew - lengths[name];
            if need <= 0 {
                continue;
            }
            if !accepted.contains_key(name) {
                notes.push(format!("{}: {name} is {need} nm short; its copper is not the bus router's", group.id));
                continue;
            }
            let rule = rules.get(name);
            if rule.is_none_or(|r| r.tuning_amplitude_limit_nm.is_none()) {
                notes.push(format!("{}: {name} is {need} nm short; no tuning_amplitude_limit is declared", group.id));
                continue;
            }
            return not_implemented("unified router bus skew tuning (critical_tuning.UnitTuner)");
        }
    }
    Ok(notes)
}

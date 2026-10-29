//! `route_unified`: every ordinary net routed with one negotiated-congestion
//! loop, then rip-in repair rounds, smoothing, budgets and the congestion
//! export.

use std::time::{Duration, Instant};

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
use crate::unified_error::{Result, fail};

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

/// Runs `$body` with `$router`, a router over `$partial`: on the shared index
/// in `$slot` (given back afterwards), or, without `share_commit_index`, on a
/// fresh index of `$partial`.
macro_rules! with_router {
    ($shared:expr, $slot:expr, $partial:expr, $options:expr, $nets:expr, $pads:expr, $required:expr,
     |$router:ident| $body:block) => {
        if $shared.enabled {
            let taken = $slot.take().expect("index");
            let mut $router = Router::new($partial, $options, $nets, $pads, $required, taken)?;
            $router.set_guides($shared.guides);
            let outcome = $body;
            *$slot = $router.clearance.take();
            outcome
        } else {
            let fresh = RoutingClearanceIndex::new($partial)?;
            let mut $router = Router::new($partial, $options, $nets, $pads, $required, fresh)?;
            $router.set_guides($shared.guides);
            $body
        }
    };
}

/// `_SharedIndex`: one exact index of a base board plus a chosen part of this call's copper.
/// Without `share_commit_index` (`enabled` false) it holds nothing and every
/// router builds a fresh index of its own board.
struct Shared<'g> {
    enabled: bool,
    /// The global route every router of the call follows (`guides`).
    guides: Option<&'g GlobalRoutingResult>,
    held: IndexMap<String, (Vec<TrackSegment>, Vec<Via>)>,
    frozen: IndexSet<String>,
}

impl Shared<'_> {
    fn added(&mut self, more: &Accepted) {
        if !self.enabled {
            return;
        }
        for (name, (tracks, vias)) in more {
            let entry = self.held.entry(name.clone()).or_default();
            entry.0.extend(tracks.iter().cloned());
            entry.1.extend(vias.iter().cloned());
        }
    }

    /// `freeze`: the copper held now becomes part of the base.
    fn freeze(&mut self) {
        if !self.enabled {
            return;
        }
        let names: Vec<String> = self.held.keys().cloned().collect();
        self.frozen.extend(names);
        self.held.clear();
    }

    fn hold(&mut self, index: &mut RoutingClearanceIndex<'_>, target: &Accepted) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
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
    let mut weight: Option<Vec<f64>> = None;
    let (tiles_x, tiles_y, tile_ids) = match g.as_nested() {
        Some(nested) => {
            let tx: Vec<i64> = nested.column_x.iter().map(|x| (x - nested.x0).div_euclid(tile_nm)).collect();
            let ty: Vec<i64> = nested.column_y.iter().map(|y| (y - nested.y0).div_euclid(tile_nm)).collect();
            let tiles_x = *tx.iter().max().expect("columns") as usize + 1;
            let tiles_y = *ty.iter().max().expect("columns") as usize + 1;
            let ids = tx.iter().zip(&ty).map(|(&x, &y)| y as usize * tiles_x + x as usize).collect();
            let column_weight = nested.column_weight();
            for (demand, w) in node_demand.iter_mut().zip(&column_weight) {
                *demand *= w;
            }
            weight = Some(column_weight);
            (tiles_x, tiles_y, ids)
        }
        None => {
            let tx: Vec<i64> = (0..g.nx).map(|i| (i * g.pitch).div_euclid(tile_nm)).collect();
            let ty: Vec<i64> = (0..g.ny).map(|i| (i * g.pitch).div_euclid(tile_nm)).collect();
            let (tiles_x, tiles_y) = (*tx.last().expect("nx") as usize + 1, *ty.last().expect("ny") as usize + 1);
            let mut ids = Vec::with_capacity(plane);
            for &y in &ty {
                for &x in &tx {
                    ids.push(y as usize * tiles_x + x as usize);
                }
            }
            (tiles_x, tiles_y, ids)
        }
    };
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
        let lanes = free as f64 / lane;
        capacity[*id] += match &weight {
            Some(weight) => lanes * weight[column],
            None => lanes,
        };
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

/// `escape_seeder(board, net_names)`: a board with checked pad escapes for
/// the named failed nets; the seeded repair attempts route from it.
pub type EscapeSeeder<'a> = dyn FnMut(&PhysicalBoard, &IndexSet<String>) -> Result<PhysicalBoard> + 'a;

/// One rip-in repair's outcome (`repair_attempt`'s tuple).
struct RepairAttempt {
    work: Accepted,
    done: IndexSet<String>,
    still: IndexMap<String, Vec<Point>>,
    routes: IndexMap<String, super::router::NetRoute>,
    rejections: IndexMap<String, Vec<(Point, String, Vec<String>, bool, String)>>,
    used: i64,
}

/// `Counter(after) - Counter(before)`, elements in `after`'s first-seen order.
fn added_items<T: Clone + Eq + std::hash::Hash>(after: &[T], before: &[T]) -> Vec<T> {
    let mut counts: IndexMap<&T, i64> = IndexMap::new();
    for item in after {
        *counts.entry(item).or_default() += 1;
    }
    for item in before {
        if let Some(count) = counts.get_mut(item) {
            *count -= 1;
        }
    }
    counts.into_iter().flat_map(|(item, count)| std::iter::repeat_n(item.clone(), count.max(0) as usize)).collect()
}

/// `_same_but_copper`: the boards differ at most in their tracks and vias.
fn same_but_copper(board: &PhysicalBoard, other: &PhysicalBoard) -> bool {
    let mut copy = other.clone();
    copy.tracks = board.tracks.clone();
    copy.vias = board.vias.clone();
    &copy == board
}

/// `repair_attempt(seed)`: one rip-in repair from the adopted copper, or
/// None for a seeded attempt with no failed signal net to seed. `index` is
/// the shared commit index; it stays held at this attempt's copper.
#[allow(clippy::too_many_arguments)]
fn repair_attempt(
    board: &PhysicalBoard,
    options: &UnifiedRouterOptions,
    nets: &[&PhysicalNet],
    plane_nets: &IndexSet<String>,
    accepted: &Accepted,
    complete: &IndexSet<String>,
    router: &Router<'_, '_>,
    index: &mut Option<RoutingClearanceIndex<'_>>,
    shared: &mut Shared<'_>,
    seeder: Option<&mut EscapeSeeder<'_>>,
    required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    deadline: Option<Instant>,
    events: &mut Vec<Event>,
) -> Result<Option<RepairAttempt>> {
    let mut work: Accepted = accepted.clone();
    let mut done = complete.clone();
    let remaining: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| !done.contains(&n.name)).collect();
    // Rip up neighbours whose new copper crowds the remaining nets' pads,
    // so they renegotiate together; everything else stays fixed.
    let mut crowd = neighbour_nets(board, &remaining, &work, options.repair_neighbour_reach_nm);
    // The exact check names what blocked a rejected path: rip those too.
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
    let mut partial = with_copper(board, &work);
    // The shared index rips the crowd out instead of being rebuilt.
    let mut share = shared.enabled;
    if share {
        shared.hold(index.as_mut().expect("index"), &work)?;
    }
    if let Some(seeder) = seeder {
        let failed_signal: IndexSet<String> = remaining
            .iter()
            .filter(|net| !plane_nets.contains(&net.name) && !crowd.contains(&net.name))
            .map(|net| net.name.clone())
            .collect();
        if failed_signal.is_empty() {
            return Ok(None);
        }
        let seeded = seeder(&partial, &failed_signal)?;
        for track in added_items(&seeded.tracks, &partial.tracks) {
            work.entry(track.net.clone()).or_default().0.push(track);
        }
        for via in added_items(&seeded.vias, &partial.vias) {
            work.entry(via.net.clone()).or_default().1.push(via);
        }
        // Seeded escapes join the shared index only if the seeder just added
        // copper; any other change gets a fresh index of its board.
        share = share
            && same_but_copper(&seeded, &partial)
            && added_items(&partial.tracks, &seeded.tracks).is_empty()
            && added_items(&partial.vias, &seeded.vias).is_empty();
        if share {
            shared.hold(index.as_mut().expect("index"), &work)?;
        }
        partial = seeded;
    }
    let fixed = explicit_copper_connectivity(&partial, Candidate::default())?;
    let remaining: Vec<&PhysicalNet> =
        remaining.into_iter().filter(|net| !fixed.net_connected(net) || plane_nets.contains(&net.name)).collect();
    let penalised = |repair: &mut Router<'_, '_>| {
        repair.history_seed(router);
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
    };
    let (more_routes, more, more_rejected, more_complete, rejections, used) = if share {
        let taken = index.take().expect("index");
        let mut repair = Router::new(&partial, options, &remaining, IndexSet::new(), required_accesses, taken)?;
        repair.set_guides(shared.guides);
        penalised(&mut repair);
        let (more_routes, more_conflicts, used) = repair.negotiate(deadline, None, events)?;
        let (more, more_rejected, more_complete) = repair.commit(&more_routes, &more_conflicts)?;
        *index = repair.clearance.take();
        shared.added(&more);
        (more_routes, more, more_rejected, more_complete, repair.rejections.clone(), used)
    } else {
        let fresh = RoutingClearanceIndex::new(&partial)?;
        let mut repair = Router::new(&partial, options, &remaining, IndexSet::new(), required_accesses, fresh)?;
        repair.set_guides(shared.guides);
        penalised(&mut repair);
        let (more_routes, more_conflicts, used) = repair.negotiate(deadline, None, events)?;
        let (more, more_rejected, more_complete) = repair.commit(&more_routes, &more_conflicts)?;
        (more_routes, more, more_rejected, more_complete, repair.rejections.clone(), used)
    };
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
    Ok(Some(RepairAttempt { work, done, still, routes: more_routes, rejections, used }))
}

/// `save_state`'s write: the parent directories (`mkdir`), then the text
/// (`write_text`); a failure is the uncaught `OSError`.
fn save_inputs(path: &str, text: &str) -> Result<()> {
    use copper_lang::paths::{python_os_call_error, python_os_error};
    let crash = |error: &std::io::Error, message: String| {
        let class = match error.kind() {
            std::io::ErrorKind::AlreadyExists => "FileExistsError",
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::IsADirectory => "IsADirectoryError",
            std::io::ErrorKind::NotADirectory => "NotADirectoryError",
            _ => "OSError",
        };
        crate::unified_error::Error::Crash(class.to_owned(), message)
    };
    let target = std::path::Path::new(path);
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        let shown = crate::pypath::display(&parent.to_string_lossy());
        std::fs::create_dir_all(parent).map_err(|e| crash(&e, python_os_call_error(&e, Some(&shown))))?;
    }
    let shown = crate::pypath::display(path);
    std::fs::write(target, text).map_err(|e| crash(&e, python_os_error(&e, Some(&shown))))
}

/// `route_unified`: every ordinary net in one negotiated-congestion loop.
#[allow(clippy::too_many_arguments)]
pub fn route_unified(
    board: &PhysicalBoard,
    global_route: &GlobalRoutingResult,
    options: &UnifiedRouterOptions,
    fanout_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    only_nets: Option<&IndexSet<String>>,
    required_accesses: Option<&IndexMap<PadReference, RoutingAccess>>,
    mut escape_seeder: Option<&mut EscapeSeeder<'_>>,
    mut on_progress: Option<&mut dyn Progress>,
) -> Result<DetailedRoutingResult> {
    if let Some(fanout) = fanout_accesses
        && fanout.iter().any(|(pad, access)| required_accesses.is_none_or(|r| r.get(pad) != Some(access)))
    {
        return fail("unified router does not consume verified fanout accesses; pass required_accesses");
    }
    if let Some(path) = &options.save_inputs_path
        && !path.is_empty()
    {
        // A replayable input state for iterating on the router alone
        // (see replay_unified); a working artifact, not evidence.
        let text = crate::state_codec::inputs_state_text(
            board,
            global_route,
            options,
            only_nets,
            fanout_accesses.is_some() || required_accesses.is_some(),
        )?;
        save_inputs(path, &text)?;
    }
    // A soft wall-clock deadline (D6: a result reached before it does not
    // depend on timing; one cut short can differ from Python's).
    let deadline = options
        .time_budget_s
        .filter(|budget| *budget != 0.0)
        .map(|budget| Instant::now() + Duration::from_secs_f64(budget.max(0.0)));
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
    let mut bus_names: IndexSet<&str> = board
        .match_groups
        .iter()
        .flat_map(|g| g.nets.iter().map(String::as_str))
        .filter(|name| nets.iter().any(|n| n.name == *name))
        .collect();
    if options.buses_first && options.bus_bundles && !bus_names.is_empty() {
        // A bus bundle: every net that lies on the parts a bus joins.
        let pads_of: IndexMap<&str, &Vec<PadReference>> =
            board.nets.iter().map(|net| (net.name.as_str(), &net.pads)).collect();
        for group in &board.match_groups {
            let parts: IndexSet<&str> = group
                .nets
                .iter()
                .flat_map(|name| pads_of.get(name.as_str()).into_iter().flat_map(|pads| pads.iter()))
                .map(|pad| pad.component.as_str())
                .collect();
            for net in &nets {
                if net.pads.len() >= 2 && net.pads.iter().all(|pad| parts.contains(pad.component.as_str())) {
                    bus_names.insert(net.name.as_str());
                }
            }
        }
    }
    let mut events: Vec<Event> = Vec::new();
    let mut shared = Shared {
        enabled: options.share_commit_index,
        guides: Some(global_route),
        held: IndexMap::new(),
        frozen: IndexSet::new(),
    };
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
        bus_router.set_guides(shared.guides);
        let (bus_routes, bus_conflicts, used) = bus_router.negotiate(deadline, None, &mut events)?;
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
            (bus_accepted, skew_notes) = tune_bus_skew(board, &bus_accepted, &rules)?;
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
    let index = if shared.enabled { index } else { RoutingClearanceIndex::new(board)? };
    let mut router = Router::new(board, options, &nets, IndexSet::new(), required_accesses, index)?;
    router.set_guides(shared.guides);
    if options.export_congestion && options.congestion_tile_nm < router.pitch {
        return fail("congestion_tile_nm must be at least the router pitch");
    }
    // The base block's shape; on a nested lattice also the patches and node counts.
    let lattice = &router.lattice;
    let mut setup = vec![
        ("nets", int(router.specs.len() as i64)),
        ("grid", Value::Array(vec![int(lattice.nx), int(lattice.ny), int(lattice.nl)])),
    ];
    if let Some(nested) = lattice.as_nested() {
        let patches = nested.blocks[1..]
            .iter()
            .map(|block| {
                Value::Object(Object::from([
                    ("x0".to_owned(), Value::Float(block.x0 as f64 / 1e6)),
                    ("y0".to_owned(), Value::Float(block.y0 as f64 / 1e6)),
                    ("x1".to_owned(), Value::Float(block.x1 as f64 / 1e6)),
                    ("y1".to_owned(), Value::Float(block.y1 as f64 / 1e6)),
                    ("factor".to_owned(), int(block.factor)),
                    ("nodes".to_owned(), int(block.plane * nested.nl)),
                ]))
            })
            .collect();
        setup.push(("patches", Value::Array(patches)));
        setup.push(("nodes", int(nested.size)));
        let finest = router.fine_plan.as_ref().expect("fine plan").uniform_nodes_at_finest;
        setup.push(("uniform_nodes_at_finest", int(finest)));
    }
    setup.push(("seconds", Value::Float(0.0)));
    emit(&mut on_progress, "unified_setup", "finished", setup);
    let (routes, conflicts, mut iterations) = router.negotiate(deadline, None, &mut events)?;
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
        if nets.iter().all(|net| complete.contains(&net.name)) || deadline.is_some_and(|d| Instant::now() > d) {
            break;
        }
        let baseline = open_count(board, &nets, &accepted, &plane_nets, &complete)?;
        let mut index = router.clearance.take();
        let mut best: Option<((i64, i64), RepairAttempt, bool)> = None;
        let seeds: &[bool] = if escape_seeder.is_some() { &[false, true] } else { &[false] };
        for &seed in seeds {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                emit(
                    &mut on_progress,
                    "unified_repair",
                    "skipped",
                    vec![
                        ("round", int(round_index + 1)),
                        ("seeded", Value::Bool(seed)),
                        ("reason", Value::Str("time_budget".to_owned())),
                    ],
                );
                break;
            }
            let attempt = repair_attempt(
                board,
                options,
                &nets,
                &plane_nets,
                &accepted,
                &complete,
                &router,
                &mut index,
                &mut shared,
                if seed { escape_seeder.as_deref_mut() } else { None },
                required_accesses,
                deadline,
                &mut events,
            )?;
            forward(&mut on_progress, &mut events);
            let Some(attempt) = attempt else { continue };
            iterations += attempt.used;
            let score = open_count(board, &nets, &attempt.work, &plane_nets, &attempt.done)?;
            if score < baseline && best.as_ref().is_none_or(|(least, _, _)| score < *least) {
                best = Some((score, attempt, seed));
            }
            if best.as_ref().is_some_and(|(least, _, _)| least.0 == 0) {
                break;
            }
        }
        let improved = best.is_some();
        let mut seeded = false;
        if let Some((_, attempt, seed)) = best {
            accepted = attempt.work;
            complete = attempt.done;
            rejected = attempt.still;
            seeded = seed;
            for (name, records) in attempt.rejections {
                router.rejections.insert(name, records);
            }
            for (name, route) in attempt.routes {
                router.routes.insert(name, route);
            }
        }
        // Back to the adopted copper (attempts left their own in the index).
        if shared.enabled {
            shared.hold(index.as_mut().expect("index"), &accepted)?;
        }
        router.clearance = index;
        emit(
            &mut on_progress,
            "unified_commit",
            "finished",
            vec![
                ("round", int(round_index + 1)),
                ("adopted", Value::Bool(improved)),
                ("seeded", Value::Bool(improved && seeded)),
                ("seconds", Value::Float(0.0)),
            ],
        );
        if !improved {
            break;
        }
    }
    if options.pour_recheck && !plane_nets.is_empty() && deadline.is_none_or(|d| Instant::now() <= d) {
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
            deadline,
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
    shared: &mut Shared<'_>,
    slot: &mut Option<RoutingClearanceIndex<'b>>,
    deadline: Option<Instant>,
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
    if shared.enabled {
        shared.hold(slot.as_mut().expect("index"), &current.0)?;
    }
    let partial = with_copper(board, &current.0);
    let (more, used) =
        with_router!(shared, slot, &partial, options, &plane_list, IndexSet::new(), required_accesses, |step| {
            let (routes, conflicts, used) = step.negotiate(deadline, None, events)?;
            let (more, _, _) = step.commit(&routes, &conflicts)?;
            (more, used)
        });
    iterations += used;
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
    if !still.is_empty() && deadline.is_none_or(|d| Instant::now() <= d) {
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
        if shared.enabled {
            shared.hold(slot.as_mut().expect("index"), &ripped)?;
        }
        let partial = with_copper(board, &ripped);
        let (plane_more, used, explicit) =
            with_router!(shared, slot, &partial, options, &still_nets, pads, required_accesses, |first| {
                let (routes, conflicts, used) = first.negotiate(deadline, None, events)?;
                let (plane_more, plane_rejected, _) = first.commit(&routes, &conflicts)?;
                let mut explicit = Object::new();
                for name in still.keys() {
                    let copper = plane_more.get(name).map_or((0, 0), |(t, v)| (t.len(), v.len()));
                    let reason = routes.get(name).map_or("not routed".to_owned(), |r| r.reason.clone());
                    let rejected =
                        if plane_rejected.contains_key(name) { rejection_text(&first, name) } else { String::new() };
                    explicit.insert(
                        name.clone(),
                        Value::Object(Object::from([
                            ("copper".to_owned(), Value::Array(vec![int(copper.0 as i64), int(copper.1 as i64)])),
                            ("reason".to_owned(), Value::from(reason.as_str())),
                            ("rejected".to_owned(), Value::from(rejected.as_str())),
                        ])),
                    );
                }
                (plane_more, used, explicit)
            });
        iterations += used;
        explicit_report = Some(Value::Object(explicit));
        shared.added(&plane_more);
        let candidate = merge(&ripped, &plane_more);
        let crowd_nets: Vec<&PhysicalNet> = nets.iter().copied().filter(|n| crowd.contains(&n.name)).collect();
        if shared.enabled {
            shared.hold(slot.as_mut().expect("index"), &candidate)?;
        }
        let partial = with_copper(board, &candidate);
        let (signal_more, signal_complete, used) =
            with_router!(shared, slot, &partial, options, &crowd_nets, IndexSet::new(), required_accesses, |second| {
                let (routes, conflicts, used) = second.negotiate(deadline, None, events)?;
                let (signal_more, _, signal_complete) = second.commit(&routes, &conflicts)?;
                (signal_more, signal_complete, used)
            });
        iterations += used;
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
    if shared.enabled {
        shared.hold(slot.as_mut().expect("index"), &current.0)?;
    }
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

/// `_tune_bus_skew`: lengthen short bus members so each length-match group
/// meets its `max_skew`, with checked bumps or serpentines
/// (`critical_tuning.UnitTuner`) against all other copper: to the longest
/// when that fits, else by the least that brings it within the limit, never
/// past its own `max_length`. Only a net that declares
/// `tuning_amplitude_limit` is tuned; every member left over the limit is
/// reported with its reason.
fn tune_bus_skew(
    board: &PhysicalBoard,
    accepted: &Accepted,
    rules: &IndexMap<&str, &copper_pcb::board::NetRoutingRule>,
) -> Result<(Accepted, Vec<String>)> {
    let mut accepted = accepted.clone();
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
            let Some(rule) = rules.get(name).filter(|r| r.tuning_amplitude_limit_nm.is_some()) else {
                notes.push(format!("{}: {name} is {need} nm short; no tuning_amplitude_limit is declared", group.id));
                continue;
            };
            let (tracks, vias) = accepted[name].clone();
            let mut others: Vec<TrackSegment> = board.tracks.clone();
            let mut other_vias: Vec<Via> = board.vias.clone();
            for (other, (items, other_items)) in &accepted {
                if other != name {
                    others.extend(items.iter().cloned());
                    other_vias.extend(other_items.iter().cloned());
                }
            }
            other_vias.extend(vias.iter().cloned());
            let mut obstacles = board.clone();
            obstacles.tracks = others;
            obstacles.vias = other_vias;
            let index = RoutingClearanceIndex::new(&obstacles)?;
            let clearance = board.rules.minimum_clearance_nm.max(rule.clearance_nm.unwrap_or(0));
            let nets = [name.to_owned()];
            let tuner = crate::critical_tuning::UnitTuner::new(
                board,
                &index,
                tracks.clone(),
                &nets,
                0,
                rule.tuning_amplitude_limit_nm.expect("declared"),
                clearance,
                crate::critical_tuning::TunerOptions {
                    style: rule.tuning_style,
                    leg_gap_nm: rule.tuning_spacing_nm,
                    vias: vias.clone(),
                    ..Default::default()
                },
            )?;
            let room = rule.max_length_nm.map(|limit| limit - lengths[name]);
            let (full, least) = (longest - lengths[name], need + need.rem_euclid(2));
            let mut targets: Vec<i64> = Vec::new();
            for target in if full == least { vec![full] } else { vec![full, least] } {
                let even = target - target.rem_euclid(2);
                if even >= need && room.is_none_or(|room| even <= room) {
                    targets.push(even);
                }
            }
            let mut reason = "max_length leaves no room".to_owned();
            let mut tuned = false;
            for target in &targets {
                let plan = tuner.plan(*target)?;
                if let Some(planned) = &plan.tracks {
                    let width = rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
                    let split = index.breakout.split_tracks(planned, Some(width));
                    accepted.insert(name.to_owned(), (split, vias.clone()));
                    tuned = true;
                    break;
                }
                reason = plan.reason.clone();
            }
            if !tuned {
                notes.push(format!("{}: {name} is {need} nm short; {reason}", group.id));
            }
        }
    }
    Ok((accepted, notes))
}

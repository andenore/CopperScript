//! Crossing-aware layer assignment of ordinary global guide runs
//! (`pcbir.layer_assignment`).
//!
//! The sequential global search prices each net against earlier edge usage
//! only, so two guides on one layer can pass straight through the same tile
//! in perpendicular directions. This post-pass keeps every net's tile path,
//! via positions, via count and access pads; it only relabels the layer of an
//! ordinary (`GENERAL`) net's via-bounded run when that lowers its soft cost.

use indexmap::{IndexMap, IndexSet};

use copper_pcb::Result;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind};
use copper_pcb::zones::routing_layers;

use crate::routing::{
    AccessOptions, GlobalNetRoute, GlobalRouteSegment, GlobalRouterOptions, GlobalViaProposal, Graph, GridNode,
    PinAccess, Resource, edge_key, graph_preference_cost, net_demand, route_resource_demands,
};
use crate::routing_costs::{COST_UNIT, demand_cost};

type Edge = (GridNode, GridNode);
type Pass = (i64, i64, i64, &'static str);

/// `_Run`: one via-bounded planar run of a route.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Run {
    route: usize,
    net: String,
    layer: i64,
    edges: Vec<Edge>,
    straight: Vec<(i64, i64, &'static str)>,
    /// (route via index, other layer index)
    vias: Vec<(usize, i64)>,
    accesses: Vec<usize>,
    candidates: Vec<i64>,
    demand: i64,
    length_nm: i64,
    tiles: IndexSet<(i64, i64)>,
}

/// `assign_ordinary_layers`: routes with ordinary runs relabelled; unchanged routes are reused.
pub fn assign_ordinary_layers(
    board: &PhysicalBoard,
    graph: &Graph,
    routes: &[GlobalNetRoute],
    rules: &IndexMap<String, NetRoutingRule>,
    access_options: &AccessOptions,
    options: &GlobalRouterOptions,
) -> Result<Vec<GlobalNetRoute>> {
    let planar: IndexMap<&str, &Resource> = graph
        .resources
        .values()
        .filter(|item| item.first.layer_index == item.second.layer_index)
        .map(|item| (item.identifier.as_str(), item))
        .collect();
    let mut usage: IndexMap<String, i64> = IndexMap::new();
    let mut passes: IndexMap<Pass, i64> = IndexMap::new();
    let mut demands = Vec::new();
    let mut edges_by_route: Vec<Vec<Edge>> = Vec::new();
    for route in routes {
        let demand = net_demand(board, rules.get(&route.net), options);
        demands.push(demand);
        let mut edges: Vec<Edge> = route
            .segments
            .iter()
            .filter_map(|item| planar.get(item.resource_id.as_str()).map(|r| (r.first, r.second)))
            .collect();
        edges.sort();
        edges_by_route.push(edges);
        if !route.connected {
            continue;
        }
        for (identifier, amount) in route_resource_demands(route, demand, None) {
            *usage.entry(identifier).or_insert(0) += amount;
        }
        for key in straight_passes(edges_by_route.last().expect("edges")) {
            *passes.entry(key).or_insert(0) += 1;
        }
    }
    let mut runs: Vec<Run> = Vec::new();
    for (index, route) in routes.iter().enumerate() {
        runs.extend(find_runs(
            board,
            graph,
            index,
            route,
            &edges_by_route[index],
            rules.get(&route.net),
            access_options,
            demands[index],
        ));
    }
    runs.sort_by(|a, b| (-a.length_nm, &a.net, a.layer, &a.edges).cmp(&(-b.length_nm, &b.net, b.layer, &b.edges)));
    let mut layers: Vec<i64> = runs.iter().map(|run| run.layer).collect();
    let mut by_net: IndexMap<&str, Vec<usize>> = IndexMap::new();
    for (index, run) in runs.iter().enumerate() {
        by_net.entry(run.net.as_str()).or_default().push(index);
    }
    let crossing = 2 * options.via_cost * COST_UNIT;
    for _ in 0..options.layer_assignment_passes {
        let mut changed = false;
        for (index, run) in runs.iter().enumerate() {
            let current = layers[index];
            apply(graph, run, current, &mut usage, &mut passes, -1);
            let taken: IndexSet<i64> = by_net[run.net.as_str()]
                .iter()
                .filter(|other| **other != index && runs[**other].tiles.iter().any(|tile| run.tiles.contains(tile)))
                .map(|other| layers[*other])
                .collect();
            let mut best: Option<(i64, bool, i64)> = None;
            for layer in &run.candidates {
                if *layer != current && taken.contains(layer) {
                    continue;
                }
                let cost = run_cost(graph, run, *layer, current, &usage, &passes, crossing, options);
                if let Some(cost) = cost {
                    let key = (cost, *layer != current, *layer);
                    if best.is_none_or(|known| key < known) {
                        best = Some(key);
                    }
                }
            }
            let (_, _, chosen) = best.expect("the current layer is always a candidate");
            layers[index] = chosen;
            changed |= chosen != current;
            apply(graph, run, chosen, &mut usage, &mut passes, 1);
        }
        if !changed {
            break;
        }
    }
    let mut moved: IndexMap<usize, Vec<(&Run, i64)>> = IndexMap::new();
    for (index, run) in runs.iter().enumerate() {
        if layers[index] != run.layer {
            moved.entry(run.route).or_default().push((run, layers[index]));
        }
    }
    routes
        .iter()
        .enumerate()
        .map(|(index, route)| match moved.get(&index) {
            Some(moves) => relabel(graph, route, moves, access_options),
            None => Ok(route.clone()),
        })
        .collect()
}

/// `_straight_passes`: tiles a run passes straight through, with the axis.
fn straight_passes(edges: &[Edge]) -> Vec<Pass> {
    let mut ports: IndexMap<(i64, i64, i64), IndexSet<(i64, i64)>> = IndexMap::new();
    for (first, second) in edges {
        let (dx, dy) = (second.x_index - first.x_index, second.y_index - first.y_index);
        ports.entry((first.layer_index, first.x_index, first.y_index)).or_default().insert((dx, dy));
        ports.entry((second.layer_index, second.x_index, second.y_index)).or_default().insert((-dx, -dy));
    }
    let mut result = Vec::new();
    for ((layer, x, y), directions) in ports {
        if directions.contains(&(1, 0)) && directions.contains(&(-1, 0)) {
            result.push((layer, x, y, "h"));
        }
        if directions.contains(&(0, 1)) && directions.contains(&(0, -1)) {
            result.push((layer, x, y, "n"));
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn find_runs(
    board: &PhysicalBoard,
    graph: &Graph,
    index: usize,
    route: &GlobalNetRoute,
    edges: &[Edge],
    rule: Option<&NetRoutingRule>,
    access_options: &AccessOptions,
    demand: i64,
) -> Vec<Run> {
    if !route.connected
        || route.deferred_to_zone
        || edges.is_empty()
        || rule.is_some_and(|r| r.kind != RouteKind::General)
    {
        return Vec::new();
    }
    let mut allowed: Vec<i64> =
        routing_layers(board, &route.net, rule).iter().filter_map(|layer| graph.layer_position(*layer)).collect();
    allowed.sort_unstable();
    if allowed.len() < 2 {
        return Vec::new();
    }
    let surfaces = [graph.layers[0], graph.layers[graph.layers.len() - 1]];
    let mut parent: IndexMap<GridNode, GridNode> = IndexMap::new();
    fn find(parent: &mut IndexMap<GridNode, GridNode>, mut node: GridNode) -> GridNode {
        loop {
            let up = *parent.entry(node).or_insert(node);
            if up == node {
                return node;
            }
            let grand = *parent.entry(up).or_insert(up);
            parent.insert(node, grand);
            node = grand;
        }
    }
    for (first, second) in edges {
        let (a, b) = (find(&mut parent, *first), find(&mut parent, *second));
        parent.insert(a, b);
    }
    let mut groups: IndexMap<GridNode, Vec<Edge>> = IndexMap::new();
    for edge in edges {
        let root = find(&mut parent, edge.0);
        groups.entry(root).or_default().push(*edge);
    }
    let access_vias: Vec<(copper_physical::Point, copper_physical::CopperLayer, copper_physical::CopperLayer)> = route
        .accesses
        .iter()
        .filter_map(|item| item.via.as_ref().map(|via| (via.position, via.from_layer, via.to_layer)))
        .collect();
    // xy -> same-net layers, for merge avoidance
    let mut occupied: IndexMap<(i64, i64), IndexSet<i64>> = IndexMap::new();
    for (first, second) in edges {
        for node in [first, second] {
            occupied.entry((node.x_index, node.y_index)).or_default().insert(node.layer_index);
        }
    }
    for access in &route.accesses {
        occupied.entry((access.node.x_index, access.node.y_index)).or_default().insert(access.node.layer_index);
    }
    let mut result = Vec::new();
    for members in groups.values() {
        let layer = members[0].0.layer_index;
        let mut nodes: IndexSet<GridNode> = IndexSet::new();
        for (a, b) in members {
            nodes.insert(*a);
            nodes.insert(*b);
        }
        let xy: IndexSet<(i64, i64)> = nodes.iter().map(|node| (node.x_index, node.y_index)).collect();
        let mut vias: Vec<(usize, i64)> = Vec::new();
        let mut excluded: IndexSet<i64> = IndexSet::new();
        for (via_index, via) in route.vias.iter().enumerate() {
            if access_vias.contains(&(via.position, via.from_layer, via.to_layer)) {
                continue;
            }
            let pair = (
                graph.layer_position(via.from_layer).expect("layer"),
                graph.layer_position(via.to_layer).expect("layer"),
            );
            let node = nodes.iter().find(|n| graph.point(**n) == via.position);
            if node.is_some() && (pair.0 == layer || pair.1 == layer) {
                let other = if pair.0 == layer { pair.1 } else { pair.0 };
                vias.push((via_index, other));
                excluded.insert(other);
            }
        }
        let accesses: Vec<usize> = route
            .accesses
            .iter()
            .enumerate()
            .filter(|(_, access)| nodes.contains(&access.node))
            .map(|(i, _)| i)
            .collect();
        // Keep inner-to-inner transitions and non-via pin accesses as built:
        // relabelling both sides of one transition could merge or orphan it.
        if accesses.iter().any(|i| route.accesses[*i].via.is_none())
            || vias.iter().any(|(_, other)| !surfaces.contains(&graph.layers[*other as usize]))
        {
            continue;
        }
        for tile in &xy {
            excluded.extend(occupied[tile].iter().copied().filter(|l| *l != layer));
        }
        let mut candidates = vec![layer];
        for other in &allowed {
            if excluded.contains(other) || *other == layer {
                continue;
            }
            if !members.iter().all(|(a, b)| graph.resources.contains_key(&edge_key(on(*a, *other), on(*b, *other)))) {
                continue;
            }
            if !vias.iter().all(|(i, o)| {
                graph
                    .resources
                    .contains_key(&edge_key(at(graph, &route.vias[*i], *o), at(graph, &route.vias[*i], *other)))
            }) {
                continue;
            }
            if !accesses.iter().all(|i| access_on(&route.accesses[*i], *other, &route.net, access_options).is_some()) {
                continue;
            }
            candidates.push(*other);
        }
        if candidates.len() < 2 {
            continue;
        }
        let length: i64 = members
            .iter()
            .map(|(a, b)| {
                (graph.xs[a.x_index as usize] - graph.xs[b.x_index as usize]).abs()
                    + (graph.ys[a.y_index as usize] - graph.ys[b.y_index as usize]).abs()
            })
            .sum();
        let mut straight: Vec<(i64, i64, &'static str)> =
            straight_passes(members).into_iter().map(|(_, x, y, axis)| (x, y, axis)).collect();
        straight.sort();
        let mut sorted_members = members.clone();
        sorted_members.sort();
        candidates.sort_unstable();
        result.push(Run {
            route: index,
            net: route.net.clone(),
            layer,
            edges: sorted_members,
            straight,
            vias,
            accesses,
            candidates,
            demand,
            length_nm: length,
            tiles: xy,
        });
    }
    result
}

fn on(node: GridNode, layer: i64) -> GridNode {
    GridNode::new(layer, node.x_index, node.y_index)
}

fn at(graph: &Graph, via: &GlobalViaProposal, layer: i64) -> GridNode {
    let x = graph.xs.iter().position(|x| *x == via.position.x_nm).expect("via on a tile centre") as i64;
    let y = graph.ys.iter().position(|y| *y == via.position.y_nm).expect("via on a tile centre") as i64;
    GridNode::new(layer, x, y)
}

/// `_access_on`: the same pad, physical via and tile on another layer, if legal.
fn access_on<'a>(
    access: &PinAccess,
    layer: i64,
    net: &str,
    access_options: &'a AccessOptions,
) -> Option<&'a PinAccess> {
    let via = access.via.as_ref()?;
    access_options.get(&(net.to_owned(), access.pad.clone()))?.iter().find(|item| {
        item.via.as_ref().is_some_and(|other| other.position == via.position) && item.node == on(access.node, layer)
    })
}

fn apply(
    graph: &Graph,
    run: &Run,
    layer: i64,
    usage: &mut IndexMap<String, i64>,
    passes: &mut IndexMap<Pass, i64>,
    sign: i64,
) {
    for (a, b) in &run.edges {
        let identifier = &graph.resources[&edge_key(on(*a, layer), on(*b, layer))].identifier;
        *usage.entry(identifier.clone()).or_insert(0) += sign * run.demand;
    }
    for (x, y, axis) in &run.straight {
        *passes.entry((layer, *x, *y, axis)).or_insert(0) += sign;
    }
}

#[allow(clippy::too_many_arguments)]
fn run_cost(
    graph: &Graph,
    run: &Run,
    layer: i64,
    current: i64,
    usage: &IndexMap<String, i64>,
    passes: &IndexMap<Pass, i64>,
    crossing: i64,
    options: &GlobalRouterOptions,
) -> Option<i64> {
    let mut cost = 0;
    for (a, b) in &run.edges {
        let (first, second) = (on(*a, layer), on(*b, layer));
        let resource = &graph.resources[&edge_key(first, second)];
        let used = usage.get(&resource.identifier).copied().unwrap_or(0);
        if layer != current && used + run.demand > resource.capacity {
            return None;
        }
        let capacity =
            if graph.demand_lanes != 0 { resource.capacity.min(graph.demand_lanes) } else { resource.capacity };
        cost += graph_preference_cost(graph, first, second, options)
            + demand_cost(graph.point(first), graph.point(second), options.local_demand_cost, used, capacity);
    }
    for (x, y, axis) in &run.straight {
        let other = if *axis == "h" { "n" } else { "h" };
        cost += crossing * passes.get(&(layer, *x, *y, other)).copied().unwrap_or(0);
    }
    Some(cost)
}

fn relabel(
    graph: &Graph,
    route: &GlobalNetRoute,
    moves: &[(&Run, i64)],
    access_options: &AccessOptions,
) -> Result<GlobalNetRoute> {
    let mut segment_layer: IndexMap<Edge, i64> = IndexMap::new();
    let mut vias: IndexMap<usize, GlobalViaProposal> = IndexMap::new();
    let mut accesses: IndexMap<usize, PinAccess> = IndexMap::new();
    for (run, layer) in moves {
        for edge in &run.edges {
            segment_layer.insert(*edge, *layer);
        }
        for (index, other) in &run.vias {
            let via = &route.vias[*index];
            let resource = &graph.resources[&edge_key(at(graph, via, *other), at(graph, via, *layer))];
            let (low, high) = ((*other).min(*layer), (*other).max(*layer));
            vias.insert(
                *index,
                GlobalViaProposal {
                    from_layer: graph.layers[low as usize],
                    to_layer: graph.layers[high as usize],
                    resource_id: resource.identifier.clone(),
                    ..via.clone()
                },
            );
        }
        for index in &run.accesses {
            let replacement =
                access_on(&route.accesses[*index], *layer, &route.net, access_options).expect("checked candidate");
            accesses.insert(*index, replacement.clone());
        }
    }
    let planar: IndexMap<&str, &Resource> = graph
        .resources
        .values()
        .filter(|item| item.first.layer_index == item.second.layer_index)
        .map(|item| (item.identifier.as_str(), item))
        .collect();
    let mut segments: Vec<GlobalRouteSegment> = Vec::new();
    for segment in &route.segments {
        let edge = planar.get(segment.resource_id.as_str()).map(|resource| (resource.first, resource.second));
        let moved = edge.and_then(|edge| segment_layer.get(&edge).map(|layer| (edge, *layer)));
        match moved {
            Some((edge, layer)) => {
                let resource = &graph.resources[&edge_key(on(edge.0, layer), on(edge.1, layer))];
                segments.push(GlobalRouteSegment {
                    net: segment.net.clone(),
                    layer: graph.layers[layer as usize],
                    start: segment.start,
                    end: segment.end,
                    guide_half_width_nm: segment.guide_half_width_nm,
                    resource_id: resource.identifier.clone(),
                });
            }
            None => segments.push(segment.clone()),
        }
    }
    segments.sort_by(|a, b| a.resource_id.cmp(&b.resource_id));
    // A replacement access keeps its physical via and tile, so its span-based
    // via proposal is unchanged; only path transitions change logical layers.
    let mut unique_vias: Vec<GlobalViaProposal> = Vec::new();
    for (index, via) in route.vias.iter().enumerate() {
        let via = vias.get(&index).cloned().unwrap_or_else(|| via.clone());
        if !unique_vias.contains(&via) {
            unique_vias.push(via);
        }
    }
    Ok(GlobalNetRoute {
        accesses: route
            .accesses
            .iter()
            .enumerate()
            .map(|(index, access)| accesses.get(&index).cloned().unwrap_or_else(|| access.clone()))
            .collect(),
        segments,
        vias: unique_vias,
        ..route.clone()
    })
}

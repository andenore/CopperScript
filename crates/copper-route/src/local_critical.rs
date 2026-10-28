//! Bounded terminal-tree alternatives to expensive single-net global guides
//! (`pcbir.local_critical`).
//!
//! A small legal-path graph, not a Steiner solver or RF reference-copper
//! transplant: cheap direct and one-corner legal paths between the actual
//! physical lands, joined by Kruskal's minimum spanning tree. Native critical
//! acceptance still checks the complete candidate. The optional bounded
//! surface maze fallback reports not implemented.

use indexmap::IndexSet;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment};
use copper_pcb::transform::transformed_local_point;
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::Result;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::routing_layers::routing_layers;
use crate::surface_path::surface_path;

fn octilinear(path: &Option<Vec<TrackSegment>>) -> bool {
    match path {
        Some(tracks) if !tracks.is_empty() => tracks.iter().all(|t| {
            t.start.x_nm == t.end.x_nm
                || t.start.y_nm == t.end.y_nm
                || (t.start.x_nm - t.end.x_nm).abs() == (t.start.y_nm - t.end.y_nm).abs()
        }),
        _ => false,
    }
}

/// `local_surface_candidates(board, rule, maze=...)`: every deterministic,
/// via-free octilinear tree for two to eight lands, one per permitted layer
/// that joins them all.
pub fn local_surface_candidates(
    board: &PhysicalBoard,
    rule: &NetRoutingRule,
    maze: bool,
) -> Result<Vec<Vec<TrackSegment>>> {
    if !matches!(rule.kind, RouteKind::Critical | RouteKind::Clock | RouteKind::RfFeed) {
        return Ok(Vec::new());
    }
    let Some(net) = board.nets.iter().find(|net| net.name == rule.net) else { return Ok(Vec::new()) };
    if !(2..=8).contains(&net.pads.len()) {
        return Ok(Vec::new());
    }
    let rf_tree = rule.kind == RouteKind::RfFeed && rule.topology == "tree" && rule.max_vias == Some(0);
    let critical_tree = rule.kind == RouteKind::Critical && rule.topology == "tree" && rule.max_vias == Some(0);
    let point_to_point = matches!(rule.kind, RouteKind::Clock | RouteKind::RfFeed);
    if point_to_point && net.pads.len() != 2 && !rf_tree {
        return Ok(Vec::new());
    }
    if net.pads.len() > 2 && rule.topology != "tree" {
        return Ok(Vec::new());
    }
    let mut layers: IndexSet<CopperLayer> = routing_layers(board, &rule.net, Some(rule)).into_iter().collect();
    let mut terminals: Vec<Point> = Vec::new();
    for reference in &net.pads {
        let pose = board.placements.iter().find(|pose| pose.reference == reference.component).expect("a placed pad");
        let pads: Vec<_> =
            board.footprints[&pose.footprint].pads.iter().filter(|pad| pad.number == reference.pad).collect();
        if pads.is_empty() || pads.iter().any(|pad| !matches!(pad.kind, PadKind::Smd | PadKind::ThroughHole)) {
            return Ok(Vec::new());
        }
        for pad in pads {
            if pad.kind == PadKind::Smd {
                let surface = if pose.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back };
                layers.retain(|layer| *layer == surface);
            }
            terminals.push(transformed_local_point(pose, pad.position));
            if terminals.len() > 8 {
                return Ok(Vec::new());
            }
        }
    }
    if !(2..=8).contains(&terminals.len()) {
        return Ok(Vec::new());
    }
    if point_to_point && terminals.len() != 2 && !rf_tree {
        return Ok(Vec::new());
    }
    let mut unique: Vec<Point> = terminals.iter().copied().collect::<IndexSet<Point>>().into_iter().collect();
    unique.sort_by_key(|point| (point.x_nm, point.y_nm));
    let terminals = unique;
    if terminals.len() < 2 {
        return Ok(Vec::new());
    }
    let width = rule.width_nm.filter(|w| *w != 0).unwrap_or(board.rules.default_track_width_nm);
    let clearance = RoutingClearanceIndex::new(board)?;
    let mut candidates = Vec::new();
    for layer in routing_layers(board, &rule.net, Some(rule)) {
        if !layers.contains(&layer) {
            continue;
        }
        let mut edges: Vec<(i64, usize, usize, Vec<TrackSegment>)> = Vec::new();
        for first in 0..terminals.len() {
            for second in first + 1..terminals.len() {
                let path = surface_path(
                    board,
                    &clearance,
                    &rule.net,
                    terminals[first],
                    terminals[second],
                    width,
                    layer,
                    &board.tracks,
                )?;
                if !octilinear(&path)
                    && maze
                    && (terminals.len() == 2 || rf_tree || critical_tree)
                    && rule.max_vias == Some(0)
                {
                    // PENDING: _bounded_surface_maze (surface_path_between).
                    return copper_pcb::not_implemented("critical bounded surface maze");
                }
                if !octilinear(&path) {
                    continue;
                }
                let path = path.expect("octilinear");
                let refs: Vec<&TrackSegment> = path.iter().collect();
                edges.push((crate::critical::track_length(&refs), first, second, path));
            }
        }
        let mut parent: Vec<usize> = (0..terminals.len()).collect();
        let root = |parent: &Vec<usize>, mut node: usize| {
            while parent[node] != node {
                node = parent[node];
            }
            node
        };
        edges.sort_by_key(|edge| (edge.0, edge.1, edge.2));
        let mut tracks: Vec<TrackSegment> = Vec::new();
        let mut joined = 0;
        for (_, first, second, path) in edges {
            let (a, b) = (root(&parent, first), root(&parent, second));
            if a == b {
                continue;
            }
            parent[b] = a;
            tracks.extend(path);
            joined += 1;
            if joined == terminals.len() - 1 {
                let mut unique: Vec<TrackSegment> = Vec::new();
                for track in tracks.drain(..) {
                    if !unique.contains(&track) {
                        unique.push(track);
                    }
                }
                candidates.push(unique);
                break;
            }
        }
    }
    Ok(candidates)
}

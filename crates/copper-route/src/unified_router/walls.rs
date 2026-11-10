//! Report-only: what bounds the region a failed connection search reached.
//!
//! A search that ends with an exhausted frontier has flooded a closed region
//! of the lattice. The legality rasters give the owner of every blocked node
//! (a net id, or `FOREIGN` for the board outline, keep-outs, netless copper and
//! overlaps), so one extra flood over the same admissibility rules names the
//! copper that forms the region's wall. It runs only when the final
//! diagnostics are assembled, for connections that stay unresolved, and never
//! touches routing state.

use indexmap::{IndexMap, IndexSet};

use super::rasters::{FREE, WallCause};
use super::router::{NetSpec, Router};
use crate::routing_lattice::Lattice;

/// Entries listed in the diagnostic before the rest is summarised.
pub(crate) const WALL_LIST_LIMIT: usize = 5;
/// Blocked nodes whose `FOREIGN` owner is traced to obstacles; more are
/// counted as `unclassified` (the trace is linear in the obstacle count).
const FOREIGN_TRACE_LIMIT: usize = 4000;

const NODE: u8 = 0;
const CELL: u8 = 1;

const MOVES: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WallSide {
    /// The tree the search grew from is the enclosed region.
    Source,
    /// The remaining pad groups sit in the enclosed region.
    Target,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WallReport {
    pub side: WallSide,
    /// Lattice nodes in the enclosed region.
    pub reached: usize,
    /// Distinct blocked nodes (and blocked cell centres) on its boundary.
    pub boundary: usize,
    /// What the boundary consists of, largest first: net names, `board_edge`,
    /// `unconnected_pad`, `keepout_or_netless_copper` (the obstacle type does
    /// not tell a keep-out from a netless track or via), `other`.
    pub shares: Vec<(String, f64)>,
    /// Columns where the via rules refuse a layer change to an otherwise open
    /// node. Counted apart: they are not copper and not boundary nodes.
    pub via_columns: usize,
    /// Layers the net may use and layers of the board, when the net is restricted.
    pub layers: Option<(usize, usize)>,
    /// Bounding box of the region in nm: x0, y0, x1, y1.
    pub bounds_nm: (i64, i64, i64, i64),
}

/// `weights` largest first; ties by label so the order is deterministic.
pub(crate) fn rank_shares(weights: IndexMap<String, f64>) -> Vec<(String, f64)> {
    let mut ranked: Vec<(String, f64)> = weights.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

impl WallReport {
    /// The diagnostic fields appended after the existing ones.
    pub(crate) fn describe(&self) -> String {
        let total: f64 = self.shares.iter().map(|(_, weight)| weight).sum();
        let mut items: Vec<String> = self
            .shares
            .iter()
            .take(WALL_LIST_LIMIT)
            .map(|(label, weight)| format!("{label}:{}%", (100.0 * weight / total).round() as i64))
            .collect();
        if self.shares.len() > WALL_LIST_LIMIT {
            items.push(format!("+{} more", self.shares.len() - WALL_LIST_LIMIT));
        }
        let side = match self.side {
            WallSide::Source => "source",
            WallSide::Target => "target",
        };
        let (x0, y0, x1, y1) = self.bounds_nm;
        let via =
            if self.via_columns > 0 { format!("; via_blocked_columns={}", self.via_columns) } else { String::new() };
        let layers = self.layers.map(|(allowed, all)| format!("; layers_allowed={allowed}/{all}")).unwrap_or_default();
        format!(
            "; walled_in={side}; reached_nodes={}; region_nm=({x0},{y0})-({x1},{y1}); boundary_nodes={}; bounded_by=[{}]{via}{layers}",
            self.reached,
            self.boundary,
            items.join(",")
        )
    }
}

struct Tables<'a> {
    owner: &'a [i32],
    mid_owner: &'a [i32],
    via_owner: Option<&'a [i32]>,
    nid: i32,
    layers: &'a [usize],
    plane: i64,
    /// Row length of a uniform lattice (diagonal squeezes are checked there only).
    nx: Option<i64>,
}

struct Flood {
    /// Columns whose via the rules refuse although another layer's node is open.
    via_columns: usize,
    order: Vec<i64>,
    /// `(node or cell, owner, NODE/CELL)` per distinct blocked node or cell centre.
    blocked: Vec<(i64, i32, u8)>,
    reached_other: bool,
}

impl Router<'_, '_> {
    fn flood(&self, t: &Tables<'_>, start: &[i64], other: &[bool], cap: usize) -> Option<Flood> {
        let g = &self.lattice;
        let mut visited = vec![false; g.size as usize];
        let mut order: Vec<i64> = Vec::new();
        let mut keys: IndexSet<i64> = IndexSet::new();
        let mut blocked: Vec<(i64, i32, u8)> = Vec::new();
        let mut via_columns = 0;
        for &node in start {
            if !visited[node as usize] {
                visited[node as usize] = true;
                order.push(node);
            }
        }
        let free = |owner: i32| owner == FREE || owner == t.nid;
        let mut next = 0;
        while next < order.len() {
            let node = order[next];
            next += 1;
            if order.len() > cap {
                return None;
            }
            if other[node as usize] {
                return Some(Flood { via_columns, order, blocked, reached_other: true });
            }
            for (n2, _) in g.neighbours(node, true) {
                if visited[n2 as usize] {
                    continue;
                }
                let o = t.owner[n2 as usize];
                if !free(o) {
                    if keys.insert(n2) {
                        blocked.push((n2, o, NODE));
                    }
                    continue;
                }
                if let Some(nx) = t.nx {
                    let d = n2 - node;
                    if let Some(&(dx, dy)) = MOVES.iter().find(|(dx, dy)| dx != &0 && dy != &0 && dy * nx + dx == d) {
                        let cell = node + if dy > 0 { 0 } else { -nx } + if dx > 0 { 0 } else { -1 };
                        let m = t.mid_owner[cell as usize];
                        if !free(m) {
                            if keys.insert(-cell - 1) {
                                blocked.push((cell, m, CELL));
                            }
                            continue;
                        }
                    }
                }
                visited[n2 as usize] = true;
                order.push(n2);
            }
            let Some(via_owner) = t.via_owner else { continue };
            let (layer, column) = (node.div_euclid(t.plane), node.rem_euclid(t.plane));
            let vo = via_owner[column as usize];
            if !free(vo) {
                // A via rule is a wall only where it keeps the search off a layer
                // whose node is otherwise open; elsewhere the planar copper explains it.
                let useful = t.layers.iter().any(|&layer2| {
                    let n2 = layer2 as i64 * t.plane + column;
                    layer2 as i64 != layer && !visited[n2 as usize] && free(t.owner[n2 as usize])
                });
                if useful && keys.insert(-(g.size + column) - 1) {
                    via_columns += 1;
                }
                continue;
            }
            for &layer2 in t.layers {
                let n2 = layer2 as i64 * t.plane + column;
                if layer2 as i64 == layer || visited[n2 as usize] {
                    continue;
                }
                let o = t.owner[n2 as usize];
                if !free(o) {
                    if keys.insert(n2) {
                        blocked.push((n2, o, NODE));
                    }
                    continue;
                }
                visited[n2 as usize] = true;
                order.push(n2);
            }
        }
        Some(Flood { via_columns, order, blocked, reached_other: false })
    }

    /// What bounds the region a failed connection of `spec` reached: the
    /// smaller of the region flooded from `sources` and the one flooded from
    /// `targets`. `None` when the legality rasters are not built, or when the
    /// flood disagrees with the search (it reaches the other side, which the
    /// kernel's neck and target-layer rules can allow).
    pub(crate) fn wall_report(&self, spec: &NetSpec, sources: &[i64], targets: &[i64]) -> Option<WallReport> {
        if sources.is_empty() || targets.is_empty() {
            return None;
        }
        let g = &self.lattice;
        let (owner, mid_owner) = self.rasters.track_cached(spec.width, spec.clearance)?;
        let via_owner = if self.board.stackup.via_technologies.is_empty() {
            self.rasters.via_cached(spec.via_size, spec.via_drill, spec.clearance)
        } else {
            None
        };
        let tables = Tables {
            owner,
            mid_owner,
            via_owner,
            nid: spec.nid,
            layers: &spec.layers,
            plane: g.plane,
            nx: (!g.is_nested()).then_some(g.nx),
        };
        let mask = |nodes: &[i64]| {
            let mut mask = vec![false; g.size as usize];
            for &node in nodes {
                mask[node as usize] = true;
            }
            mask
        };
        let from_sources = self.flood(&tables, sources, &mask(targets), usize::MAX)?;
        if from_sources.reached_other {
            return None;
        }
        let (side, flood) = match self.flood(&tables, targets, &mask(sources), from_sources.order.len()) {
            Some(from_targets) if from_targets.reached_other => return None,
            Some(from_targets) if from_targets.order.len() < from_sources.order.len() => {
                (WallSide::Target, from_targets)
            }
            _ => (WallSide::Source, from_sources),
        };
        if flood.blocked.is_empty() && flood.via_columns == 0 {
            return None;
        }
        let names: IndexMap<i32, &String> = self.net_ids.iter().map(|(name, &id)| (id, name)).collect();
        let net = |id: i32| names.get(&id).map_or_else(|| format!("net#{id}"), |name| (*name).clone());
        let half_width = spec.width as f64 / 2.0;
        let mut weights: IndexMap<String, f64> = IndexMap::new();
        let mut traced = 0;
        for &(node, owner, kind) in &flood.blocked {
            let cell = kind == CELL;
            if owner >= 0 {
                *weights.entry(net(owner)).or_default() += 1.0;
                continue;
            }
            if traced >= FOREIGN_TRACE_LIMIT {
                *weights.entry("other".to_owned()).or_default() += 1.0;
                continue;
            }
            traced += 1;
            if self.rasters.edge_blocks(g.column(node) as usize, half_width, cell) == Some(true) {
                *weights.entry("board_edge".to_owned()).or_default() += 1.0;
                continue;
            }
            // A cell centre lies half a pitch from its corner node.
            let (at, shift) = (g.point(node), if cell { g.pitch_nm as f64 / 2.0 } else { 0.0 });
            let mut causes = self.rasters.foreign_causes(
                at.x_nm as f64 + shift,
                at.y_nm as f64 + shift,
                g.layer_of(node) as usize,
                half_width,
                spec.clearance,
            );
            // The net's own copper never blocks it: where its clearance overlaps
            // another net's, only the other net is the wall.
            causes.retain(|&cause| cause != WallCause::Net(spec.nid));
            if causes.is_empty() {
                *weights.entry("other".to_owned()).or_default() += 1.0;
            }
            for &cause in &causes {
                let label = match cause {
                    WallCause::Net(id) => net(id),
                    WallCause::UnconnectedPad => "unconnected_pad".to_owned(),
                    WallCause::NetlessObstacle => "keepout_or_netless_copper".to_owned(),
                };
                *weights.entry(label).or_default() += 1.0 / causes.len() as f64;
            }
        }
        let points = flood.order.iter().map(|&node| g.point(node));
        let mut bounds = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for p in points {
            bounds = (bounds.0.min(p.x_nm), bounds.1.min(p.y_nm), bounds.2.max(p.x_nm), bounds.3.max(p.y_nm));
        }
        Some(WallReport {
            side,
            reached: flood.order.len(),
            boundary: flood.blocked.len(),
            shares: rank_shares(weights),
            via_columns: flood.via_columns,
            layers: (spec.layers.len() < g.nl as usize).then_some((spec.layers.len(), g.nl as usize)),
            bounds_nm: bounds,
        })
    }
}

//! A cheap proof that a connection cannot succeed, before A* is run.
//!
//! The kernel moves only onto nodes whose owner is free or the net's own,
//! except that a net with a neck may also step onto its `neck` nodes (and over
//! `neck_mid` cells). A small flood over those same rules, started from one
//! side of the connection, therefore closes (runs dry before touching the
//! other side) exactly when the copper around that side is a wall. Every
//! restriction the kernel adds on top is ignored here, so the flood is at
//! least as permissive as the kernel everywhere:
//!
//! - the search window (the check uses the whole lattice),
//! - target-only layers and the "region only" rule of a neck node,
//! - the owner of a start node and of any source or target it touches (a
//!   touched source or target counts as reached whatever its owner),
//! - via technologies and their per-transition masks (every via is allowed
//!   when technologies exist, or when the via raster is not built),
//! - costs, turn and guide terms, history and the expansion limit, which
//!   decide only which path is found.
//!
//! Hence a flood that closes without touching the other side proves the
//! kernel's frontier would exhaust too. The check changes no routing result:
//! it only skips searches that must fail. It is symmetric: it may start from
//! the targets or from the sources.

use indexmap::IndexSet;

use super::rasters::FREE;
use super::router::{NetSpec, Router};
use crate::routing_lattice::KernelTables;

/// Nodes a closure flood may visit before it gives up and calls the region open.
pub(crate) const CLOSURE_CAP: usize = 512;

/// Which side of a connection sits in the closed region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureSide {
    Source,
    Target,
}

impl ClosureSide {
    pub(crate) fn word(self) -> &'static str {
        match self {
            ClosureSide::Source => "source",
            ClosureSide::Target => "target",
        }
    }
}

/// Result of one flood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flood {
    /// The region closed after visiting this many nodes without touching the goal.
    Closed(usize),
    /// The goal was touched.
    Touched,
    /// The cap was reached first; nothing is known.
    Capped,
}

/// Reusable stamps, so a flood costs only the nodes it visits.
#[derive(Default)]
pub struct ClosureScratch {
    visited: Vec<u32>,
    goal: Vec<u32>,
    epoch: u32,
    stack: Vec<i64>,
}

enum Geometry<'a> {
    Uniform { nx: i64, ny: i64 },
    Nested(&'a KernelTables),
}

struct Grid<'a> {
    owner: &'a [i32],
    mid_owner: &'a [i32],
    /// `None`: every via is allowed.
    via_owner: Option<&'a [i32]>,
    nid: i32,
    layers: &'a [usize],
    plane: i64,
    neck: &'a IndexSet<i64>,
    neck_mid: &'a IndexSet<i64>,
    geometry: Geometry<'a>,
}

const DX: [i64; 8] = [1, -1, 0, 0, 1, 1, -1, -1];
const DY: [i64; 8] = [0, 0, 1, -1, 1, -1, 1, -1];

impl Grid<'_> {
    fn free(&self, owner: i32) -> bool {
        owner == FREE || owner == self.nid
    }

    /// A node the kernel can enter: free, or one of the net's neck nodes.
    fn enterable(&self, node: i64) -> bool {
        self.free(self.owner[node as usize]) || self.neck.contains(&node)
    }

    /// Calls `visit` for each planar neighbour `u` of `v` (and the portal twin
    /// of a nested lattice) whose kernel move `u -> v` a diagonal squeeze does
    /// not rule out.
    fn for_each_neighbour(&self, v: i64, mut visit: impl FnMut(i64)) {
        let rest = v % self.plane;
        // `row` is the row length of the node's block; `(x, y)` its position there.
        let (row, x, y, bounds) = match &self.geometry {
            Geometry::Uniform { nx, ny } => (*nx, rest % nx, rest / nx, (*nx, *ny)),
            Geometry::Nested((blocks, block_of, ..)) => {
                let b = block_of[rest as usize] as usize;
                let local = rest - blocks[3 * b] as i64;
                let (nx, ny) = (blocks[3 * b + 1] as i64, blocks[3 * b + 2] as i64);
                (nx, local % nx, local / nx, (nx, ny))
            }
        };
        for m in 0..8 {
            let (x2, y2) = (x + DX[m], y + DY[m]);
            if x2 < 0 || x2 >= bounds.0 || y2 < 0 || y2 >= bounds.1 {
                continue;
            }
            let u = v + DY[m] * row + DX[m];
            if DX[m] != 0 && DY[m] != 0 {
                // The kernel move is u -> v, in the opposite direction.
                let (fx, fy) = (-DX[m], -DY[m]);
                let cell = u + if fy > 0 { 0 } else { -row } + if fx > 0 { 0 } else { -1 };
                if !self.free(self.mid_owner[cell as usize]) && !self.neck_mid.contains(&cell) {
                    continue;
                }
            }
            visit(u);
        }
        if let Geometry::Nested((_, _, _, _, twins, _)) = &self.geometry {
            let twin = twins[rest as usize];
            if twin >= 0 {
                visit(v - rest + twin as i64);
            }
        }
    }
}

impl ClosureScratch {
    fn flood(&mut self, grid: &Grid<'_>, size: usize, start: &[i64], goal: &[i64], cap: usize) -> Flood {
        if self.visited.len() != size {
            self.visited = vec![0; size];
            self.goal = vec![0; size];
            self.epoch = 0;
        }
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.visited.fill(0);
            self.goal.fill(0);
            self.epoch = 1;
        }
        let epoch = self.epoch;
        for &node in goal {
            self.goal[node as usize] = epoch;
        }
        self.stack.clear();
        for &node in start {
            if self.goal[node as usize] == epoch {
                return Flood::Touched;
            }
            if self.visited[node as usize] != epoch {
                self.visited[node as usize] = epoch;
                self.stack.push(node);
            }
        }
        let mut count = 0;
        while let Some(v) = self.stack.pop() {
            count += 1;
            if count > cap {
                return Flood::Capped;
            }
            let (visited, goal, stack) = (&mut self.visited, &self.goal, &mut self.stack);
            let mut touched = false;
            let mut reach = |u: i64| {
                if goal[u as usize] == epoch {
                    touched = true;
                } else if visited[u as usize] != epoch && grid.enterable(u) {
                    visited[u as usize] = epoch;
                    stack.push(u);
                }
            };
            grid.for_each_neighbour(v, &mut reach);
            // A layer change at this column: the kernel move u -> v.
            let (layer, column) = (v.div_euclid(grid.plane), v.rem_euclid(grid.plane));
            if grid.via_owner.is_none_or(|owner| grid.free(owner[column as usize])) {
                for &layer2 in grid.layers {
                    if layer2 as i64 != layer {
                        reach(layer2 as i64 * grid.plane + column);
                    }
                }
            }
            if touched {
                return Flood::Touched;
            }
        }
        Flood::Closed(count)
    }
}

impl Router<'_, '_> {
    /// Whether `targets` can never be reached from `sources` for this net.
    /// `Some(side)` names the side whose flood closed; `None`: not proven.
    pub(crate) fn closed_connection(
        &mut self,
        index: usize,
        sources: &[i64],
        targets: &[i64],
        cap: usize,
    ) -> (Option<ClosureSide>, usize) {
        if sources.is_empty() || targets.is_empty() {
            return (None, 0);
        }
        if self.lattice.is_nested() && self.kernel_tables.is_none() {
            let Ok(tables) = self.lattice.as_nested().expect("nested lattice").kernel_tables() else {
                return (None, 0);
            };
            self.kernel_tables = Some(tables);
        }
        let (width, clearance) = (self.specs[index].width, self.specs[index].clearance);
        self.rasters.track(width, clearance);
        let spec: &NetSpec = &self.specs[index];
        let Some((owner, mid_owner)) = self.rasters.track_cached(width, clearance) else { return (None, 0) };
        let via_owner = if self.board.stackup.via_technologies.is_empty() {
            self.rasters.via_cached(spec.via_size, spec.via_drill, spec.clearance)
        } else {
            None
        };
        let geometry = match &self.kernel_tables {
            Some(tables) if self.lattice.is_nested() => Geometry::Nested(tables),
            _ => Geometry::Uniform { nx: self.lattice.nx, ny: self.lattice.ny },
        };
        let grid = Grid {
            owner,
            mid_owner,
            via_owner,
            nid: spec.nid,
            layers: &spec.layers,
            plane: self.lattice.plane,
            neck: &spec.neck,
            neck_mid: &spec.neck_mid,
            geometry,
        };
        let size = self.lattice.size as usize;
        let mut visits = 0;
        for (side, start, goal) in [(ClosureSide::Target, targets, sources), (ClosureSide::Source, sources, targets)] {
            let result = self.closure.flood(&grid, size, start, goal, cap);
            visits += match result {
                Flood::Closed(n) => n,
                Flood::Capped => cap,
                Flood::Touched => 0,
            };
            if let Flood::Closed(_) = result {
                return (Some(side), visits);
            }
        }
        (None, visits)
    }
}

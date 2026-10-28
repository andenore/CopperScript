//! Legality rasters of the unified router (`_Obstacle`, `_BlockRaster`,
//! `_Rasters`): per width/clearance class, the owner of every lattice node
//! (and cell centre): free, one net, or foreign.

use indexmap::IndexMap;

use copper_pcb::board::PhysicalBoard;
use copper_physical::Point;

use super::geometry::{polygon_distance, polygon_inside, segment_distance, spine_distance};
use crate::routing_lattice::LatticeBlock;

/// An obstacle owned by another net, by several nets or by none.
pub const FOREIGN: i32 = -2;
pub const FREE: i32 = -1;

/// `_Obstacle`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obstacle {
    /// Net id, or [`FOREIGN`] for netless obstacles.
    pub owner: i32,
    pub layers: Vec<usize>,
    pub spine: Vec<Point>,
    pub radius: i64,
    /// Object-specific clearance floor.
    pub clearance: i64,
    pub is_pad: bool,
    pub blocks_tracks: bool,
    pub blocks_vias: bool,
    /// The spine is a simple polygon, not a convex hull.
    pub polygon: bool,
}

/// A drilled hole: `(owner, x, y, radius)`.
pub type Hole = (i32, f64, f64, f64);

/// `_BlockRaster`: node and cell-centre coordinates and board material of
/// one lattice block, row-major `(ny, nx)`.
#[derive(Clone, Debug)]
pub struct BlockRaster {
    pub block: LatticeBlock,
    pub margin: i64,
    pub px: Vec<f64>,
    pub py: Vec<f64>,
    pub mx: Vec<f64>,
    pub my: Vec<f64>,
    pub inside_nodes: Vec<bool>,
    pub inside_cells: Vec<bool>,
    pub edge_nodes: Vec<f64>,
    pub edge_cells: Vec<f64>,
}

/// A via raster key: `(size, drill, clearance, span)`.
type ViaKey = (i64, i64, i64, (usize, usize));

/// `_Rasters`: lazily built legality rasters keyed by width/clearance class.
pub struct Rasters {
    pub minimum_clearance_nm: i64,
    pub minimum_hole_clearance_nm: i64,
    pub nl: usize,
    /// Shadowed base columns and cells of a nested lattice (empty otherwise).
    pub shadowed: Vec<usize>,
    pub shadowed_cells: Vec<usize>,
    pub nested: bool,
    pub obstacles: Vec<Obstacle>,
    pub holes: Vec<Hole>,
    pub margin: i64,
    pub exact_boxes: Vec<(i64, i64, i64, i64)>,
    track: IndexMap<(i64, i64), (Vec<i32>, Vec<i32>)>,
    via: IndexMap<ViaKey, Vec<i32>>,
    /// False forbids a new via there: `(ny, nx)` row-major on a uniform
    /// lattice, one entry per column on a nested one.
    pub via_mask: Option<Vec<bool>>,
    pub views: Vec<BlockRaster>,
}

fn material(outline: &[Point], cutouts: &[Vec<Point>], px: &[f64], py: &[f64]) -> Vec<bool> {
    px.iter()
        .zip(py)
        .map(|(&x, &y)| polygon_inside(outline, x, y) && cutouts.iter().all(|ring| !polygon_inside(ring, x, y)))
        .collect()
}

fn edge_distance(outline: &[Point], cutouts: &[Vec<Point>], px: &[f64], py: &[f64]) -> Vec<f64> {
    px.iter()
        .zip(py)
        .map(|(&x, &y)| {
            let mut best = f64::INFINITY;
            for ring in std::iter::once(outline).chain(cutouts.iter().map(Vec::as_slice)) {
                let n = ring.len();
                for i in 0..n {
                    let (a, b) = (ring[i], ring[(i + 1) % n]);
                    let d = segment_distance(x, y, a.x_nm as f64, a.y_nm as f64, b.x_nm as f64, b.y_nm as f64);
                    best = super::geometry::minimum(best, d);
                }
            }
            best
        })
        .collect()
}

/// `np.meshgrid(xs, ys)`: row-major `(len(ys), len(xs))` coordinate grids.
fn meshgrid(xs: &[f64], ys: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut gx = Vec::with_capacity(xs.len() * ys.len());
    let mut gy = Vec::with_capacity(xs.len() * ys.len());
    for &y in ys {
        for &x in xs {
            gx.push(x);
            gy.push(y);
        }
    }
    (gx, gy)
}

impl Rasters {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        board: &PhysicalBoard,
        blocks: &[LatticeBlock],
        nl: usize,
        nested: bool,
        obstacles: Vec<Obstacle>,
        holes: Vec<Hole>,
        margins: Vec<i64>,
        exact_boxes: Vec<(i64, i64, i64, i64)>,
    ) -> Rasters {
        let outline = &board.outline.vertices;
        let cutouts: Vec<Vec<Point>> = board.outline.cutouts.iter().map(|c| c.vertices.clone()).collect();
        let views = blocks
            .iter()
            .zip(&margins)
            .map(|(block, &margin)| {
                let (px, py) = meshgrid(&block.xs, &block.ys);
                let half = block.pitch as f64 / 2.0;
                let cx: Vec<f64> = block.xs.iter().map(|x| x + half).collect();
                let cy: Vec<f64> = block.ys.iter().map(|y| y + half).collect();
                let (mx, my) = meshgrid(&cx, &cy);
                BlockRaster {
                    block: block.clone(),
                    margin,
                    inside_nodes: material(outline, &cutouts, &px, &py),
                    inside_cells: material(outline, &cutouts, &mx, &my),
                    edge_nodes: edge_distance(outline, &cutouts, &px, &py),
                    edge_cells: edge_distance(outline, &cutouts, &mx, &my),
                    px,
                    py,
                    mx,
                    my,
                }
            })
            .collect();
        Rasters {
            minimum_clearance_nm: board.rules.minimum_clearance_nm,
            minimum_hole_clearance_nm: board.rules.minimum_hole_clearance_nm,
            nl,
            shadowed: Vec::new(),
            shadowed_cells: Vec::new(),
            nested,
            obstacles,
            holes,
            margin: margins[0],
            exact_boxes,
            track: IndexMap::new(),
            via: IndexMap::new(),
            via_mask: None,
            views,
        }
    }

    /// `_flatten`: per-block `(nl, ny, nx)` owner arrays as one flat node-id array.
    fn flatten(&self, parts: Vec<Vec<i32>>) -> Vec<i32> {
        if parts.len() == 1 {
            return parts.into_iter().next().expect("one block");
        }
        let nl = self.nl;
        let mut flat = Vec::new();
        for layer in 0..nl {
            for part in &parts {
                let per_layer = part.len() / nl;
                flat.extend_from_slice(&part[layer * per_layer..(layer + 1) * per_layer]);
            }
        }
        flat
    }

    /// `track(width, clearance)`: owners of nodes and cell centres.
    pub fn track(&mut self, width: i64, clearance: i64) -> (&[i32], &[i32]) {
        let key = (width, clearance);
        if !self.track.contains_key(&key) {
            let half = width as f64 / 2.0;
            let nodes = self.build(half, clearance, false, false, None);
            let nodes = self.flatten(nodes);
            let cells = self.build(half, clearance, true, false, None);
            let mut cells = self.flatten(cells);
            let mut nodes = nodes;
            if self.nested {
                let plane = nodes.len() / self.nl;
                for layer in 0..self.nl {
                    for &column in &self.shadowed {
                        nodes[layer * plane + column] = FOREIGN;
                    }
                    for &cell in &self.shadowed_cells {
                        cells[layer * plane + cell] = FOREIGN;
                    }
                }
            }
            self.track.insert(key, (nodes, cells));
        }
        let (nodes, cells) = &self.track[&key];
        (nodes, cells)
    }

    /// `via(size, drill, clearance, span)`: owners for a physical via span.
    pub fn via(&mut self, size: i64, drill: i64, clearance: i64, span: Option<(usize, usize)>) -> &[i32] {
        let span = span.unwrap_or((0, self.nl - 1));
        let key = (size, drill, clearance, span);
        if !self.via.contains_key(&key) {
            let flat = self.build_via(size, drill, clearance, span);
            self.via.insert(key, flat);
        }
        &self.via[&key]
    }

    fn build_via(&self, size: i64, drill: i64, clearance: i64, (low, high): (usize, usize)) -> Vec<i32> {
        let half_size = size as f64 / 2.0;
        let half = half_size.max(drill as f64 / 2.0 + self.minimum_hole_clearance_nm as f64 - clearance as f64);
        let hole_clear = self.minimum_hole_clearance_nm;
        let mut owners = Vec::new();
        let stacks = self.build(half, clearance, false, true, Some(half_size));
        for (view, stacked) in self.views.iter().zip(stacks) {
            let per_layer = stacked.len() / self.nl;
            let mut owner: Vec<i32> = stacked[low * per_layer..(low + 1) * per_layer].to_vec();
            for layer in low + 1..=high {
                let other = &stacked[layer * per_layer..(layer + 1) * per_layer];
                for (o, &v) in owner.iter_mut().zip(other) {
                    let both = *o != v;
                    if both && *o == FREE {
                        *o = v;
                    }
                    if both && v != FREE && *o != v {
                        *o = FOREIGN;
                    }
                }
            }
            let nx = view.block.nx as usize;
            for &(_, x, y, radius) in &self.holes {
                let reach =
                    drill as f64 / 2.0 + radius + hole_clear as f64 + self.margin_at(&[x], &[y], view.margin) as f64;
                let (ix0, iy0, ix1, iy1) = view.block.window(x - reach, y - reach, x + reach, y + reach);
                if ix0 > ix1 || iy0 > iy1 {
                    continue;
                }
                for iy in iy0 as usize..=iy1 as usize {
                    for ix in ix0 as usize..=ix1 as usize {
                        let at = iy * nx + ix;
                        if (view.px[at] - x).hypot(view.py[at] - y) < reach {
                            owner[at] = FOREIGN;
                        }
                    }
                }
            }
            owners.push(owner);
        }
        let mut owner: Vec<i32> = owners.concat();
        if let Some(mask) = &self.via_mask {
            for (o, &allowed) in owner.iter_mut().zip(mask) {
                if !allowed {
                    *o = FOREIGN;
                }
            }
        }
        if owners.len() > 1 {
            for &column in &self.shadowed {
                owner[column] = FOREIGN;
            }
        }
        owner
    }

    /// `_margin_at`: no margin for an obstacle inside an exact box.
    fn margin_at(&self, xs: &[f64], ys: &[f64], margin: i64) -> i64 {
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        for &(x0, y0, x1, y1) in &self.exact_boxes {
            if x0 as f64 <= min(xs) && max(xs) <= x1 as f64 && y0 as f64 <= min(ys) && max(ys) <= y1 as f64 {
                return 0;
            }
        }
        margin
    }

    fn build(
        &self,
        half_width: f64,
        clearance: i64,
        cells: bool,
        for_via: bool,
        via_annulus_half_width: Option<f64>,
    ) -> Vec<Vec<i32>> {
        self.views
            .iter()
            .map(|view| self.build_block(view, half_width, clearance, cells, for_via, via_annulus_half_width))
            .collect()
    }

    /// `_build_block`: owner array `(nl, ny, nx)` of one block.
    fn build_block(
        &self,
        view: &BlockRaster,
        half_width: f64,
        clearance: i64,
        cells: bool,
        for_via: bool,
        via_annulus_half_width: Option<f64>,
    ) -> Vec<i32> {
        let block = &view.block;
        let margin = view.margin;
        let (px, py) = if cells { (&view.mx, &view.my) } else { (&view.px, &view.py) };
        let (inside, edge) =
            if cells { (&view.inside_cells, &view.edge_cells) } else { (&view.inside_nodes, &view.edge_nodes) };
        let edge_clear = self.minimum_clearance_nm;
        let threshold = half_width + edge_clear as f64 + margin as f64;
        let per_layer = (block.nx * block.ny) as usize;
        let nx = block.nx as usize;
        let mut owner = vec![FREE; self.nl * per_layer];
        for layer in 0..self.nl {
            for at in 0..per_layer {
                if !(inside[at] && edge[at] >= threshold) {
                    owner[layer * per_layer + at] = FOREIGN;
                }
            }
        }
        for item in &self.obstacles {
            if (for_via && !item.blocks_vias) || (!for_via && !item.blocks_tracks) {
                continue;
            }
            let mut gap = clearance.max(item.clearance);
            if for_via && item.is_pad {
                gap = gap.max(1);
            }
            let xs: Vec<f64> = item.spine.iter().map(|p| p.x_nm as f64).collect();
            let ys: Vec<f64> = item.spine.iter().map(|p| p.y_nm as f64).collect();
            let reach = (item.radius + gap) as f64 + half_width + self.margin_at(&xs, &ys, margin) as f64;
            let min_x = item.spine.iter().map(|p| p.x_nm).min().expect("spine") as f64;
            let min_y = item.spine.iter().map(|p| p.y_nm).min().expect("spine") as f64;
            let max_x = item.spine.iter().map(|p| p.x_nm).max().expect("spine") as f64;
            let max_y = item.spine.iter().map(|p| p.y_nm).max().expect("spine") as f64;
            let (ix0, iy0, ix1, iy1) = block.window(min_x - reach, min_y - reach, max_x + reach, max_y + reach);
            if ix0 > ix1 || iy0 > iy1 {
                continue;
            }
            let mut window = Vec::new();
            for iy in iy0 as usize..=iy1 as usize {
                for ix in ix0 as usize..=ix1 as usize {
                    let at = iy * nx + ix;
                    let distance = if item.polygon {
                        polygon_distance(&item.spine, px[at], py[at])
                    } else {
                        spine_distance(&item.spine, px[at], py[at])
                    };
                    window.push((at, distance));
                }
            }
            if !window.iter().any(|&(_, distance)| distance < reach) {
                continue;
            }
            let annulus = via_annulus_half_width.unwrap_or(half_width);
            let contact_reach = item.radius as f64 + annulus + 1.0 + margin as f64;
            for &layer in &item.layers {
                let base = layer * per_layer;
                for &(at, distance) in &window {
                    let hit = distance < reach;
                    let cell = &mut owner[base + at];
                    if item.owner == FOREIGN {
                        if hit {
                            *cell = FOREIGN;
                        }
                        continue;
                    }
                    if hit && *cell == FREE {
                        *cell = item.owner;
                    } else if hit && *cell != item.owner {
                        *cell = FOREIGN;
                    }
                    if for_via && item.is_pad && distance < contact_reach {
                        *cell = FOREIGN;
                    }
                }
            }
        }
        owner
    }
}

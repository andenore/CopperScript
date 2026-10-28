//! Search lattice of the unified router (`pcbir.routing_lattice`).
//!
//! Node ids are dense: `layer * plane + column`, where a column is the planar
//! id shared by every layer at one lattice point. A lattice is a sequence of
//! [`LatticeBlock`]s, each a dense regular grid with its own origin and pitch;
//! a block's columns are one contiguous run of the column space (block 0 is
//! the base lattice). [`UniformLattice`] is one block; [`NestedLattice`] adds
//! fine patches at base pitch / k with shadowed base nodes and portal twins.
//! numpy arrays are `Vec`s in the same order; float operations follow Python
//! (correctly rounded `int / int`) and numpy (`np.hypot` is the C library's).

use copper_pcb::board::PhysicalBoard;
use copper_pcb::{Error, Result};
use copper_physical::pyfloat::{big_ratio_to_f64, hypot as py_hypot, round_int};
use copper_physical::{CopperLayer, Point};
use num_bigint::BigInt;

use crate::fine_regions::{FinePatch, FineRegionPlan};

const SQRT2: f64 = std::f64::consts::SQRT_2;

/// The 8 planar moves `(dx, dy, length in pitches)`; the order is part of the
/// search contract (move tables, turn table, native kernel).
pub const MOVES: [(i64, i64, f64); 8] = [
    (1, 0, 1.0),
    (-1, 0, 1.0),
    (0, 1, 1.0),
    (0, -1, 1.0),
    (1, 1, SQRT2),
    (1, -1, SQRT2),
    (-1, 1, SQRT2),
    (-1, -1, SQRT2),
];

/// `(dx, dy, column delta, diagonal cell delta, length in base pitches)`.
pub type Move = (i64, i64, i64, i64, f64);
/// An inclusive index box `(ix0, iy0, ix1, iy1)`.
pub type IndexBox = (i64, i64, i64, i64);

fn fail<T>(message: &str) -> Result<T> {
    Err(Error::Value(message.to_owned()))
}

/// Python's `int / int`: the correctly rounded quotient.
pub fn true_div(numerator: i128, denominator: i128) -> f64 {
    const EXACT: i128 = 1 << 53;
    if numerator.abs() <= EXACT && denominator.abs() <= EXACT {
        return numerator as f64 / denominator as f64;
    }
    big_ratio_to_f64(&BigInt::from(numerator), &BigInt::from(denominator))
}

fn floor_int(value: f64) -> i64 {
    value.floor() as i64
}

fn ceil_int(value: f64) -> i64 {
    value.ceil() as i64
}

/// Python's `max(0.0, min(1.0, value))`.
fn clamp_unit(value: f64) -> f64 {
    let low = if value < 1.0 { value } else { 1.0 };
    if low > 0.0 { low } else { 0.0 }
}

/// `np.clip(value, 0.0, 1.0)`.
fn clip_unit(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// A row-major mask over an index box (`rows = y`), as `np.nonzero` reads it.
pub struct Mask<'m> {
    pub width: usize,
    pub values: &'m [bool],
}

/// The (iy, ix) of an index box, row-major, optionally where `mask` is set.
fn box_cells(window: IndexBox, mask: Option<&Mask<'_>>) -> Vec<(i64, i64)> {
    let (ix0, iy0, ix1, iy1) = window;
    if ix0 > ix1 || iy0 > iy1 {
        return Vec::new();
    }
    match mask {
        None => (iy0..=iy1).flat_map(|iy| (ix0..=ix1).map(move |ix| (iy, ix))).collect(),
        Some(mask) => mask
            .values
            .iter()
            .enumerate()
            .filter(|(_, set)| **set)
            .map(|(index, _)| ((index / mask.width) as i64 + iy0, (index % mask.width) as i64 + ix0))
            .collect(),
    }
}

/// One dense regular block of a lattice: origin, pitch and shape.
///
/// Block-local index boxes are inclusive; a block's columns are
/// `column_offset + iy * nx + ix`. `live` (a copy of the lattice's, indexed by
/// column) is set on a block whose nodes may be shadowed (a nested lattice's
/// base); column queries then drop disabled columns.
#[derive(Clone, Debug, PartialEq)]
pub struct LatticeBlock {
    pub index: usize,
    pub x0: i64,
    pub y0: i64,
    pub pitch: i64,
    pub factor: i64,
    pub nx: i64,
    pub ny: i64,
    pub plane: i64,
    pub column_offset: i64,
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    pub x1: i64,
    pub y1: i64,
    pub live: Option<Vec<bool>>,
}

impl LatticeBlock {
    #[allow(clippy::too_many_arguments)]
    pub fn new(index: usize, x0: i64, y0: i64, pitch: i64, factor: i64, nx: i64, ny: i64, column_offset: i64) -> Self {
        LatticeBlock {
            index,
            x0,
            y0,
            pitch,
            factor,
            nx,
            ny,
            plane: nx * ny,
            column_offset,
            xs: (0..nx).map(|i| (x0 as f64) + (pitch as f64) * (i as f64)).collect(),
            ys: (0..ny).map(|i| (y0 as f64) + (pitch as f64) * (i as f64)).collect(),
            x1: x0 + (nx - 1) * pitch,
            y1: y0 + (ny - 1) * pitch,
            live: None,
        }
    }

    /// Block index box covering a nanometre box (clipped; may be empty).
    pub fn window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        let p = self.pitch as f64;
        (
            0.max(floor_int((min_x - self.x0 as f64) / p)),
            0.max(floor_int((min_y - self.y0 as f64) / p)),
            (self.nx - 1).min(ceil_int((max_x - self.x0 as f64) / p)),
            (self.ny - 1).min(ceil_int((max_y - self.y0 as f64) / p)),
        )
    }

    /// Block index box of the nodes inside a nanometre box (clipped; may be empty).
    pub fn inner_window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        let p = self.pitch as f64;
        (
            0.max(ceil_int((min_x - self.x0 as f64) / p)),
            0.max(ceil_int((min_y - self.y0 as f64) / p)),
            (self.nx - 1).min(floor_int((max_x - self.x0 as f64) / p)),
            (self.ny - 1).min(floor_int((max_y - self.y0 as f64) / p)),
        )
    }

    /// Live columns of a block index box (row-major), optionally where `mask` is set.
    pub fn window_columns(&self, window: IndexBox, mask: Option<&Mask<'_>>) -> Vec<i64> {
        let columns = box_cells(window, mask).into_iter().map(|(iy, ix)| iy * self.nx + ix + self.column_offset);
        match &self.live {
            Some(live) => columns.filter(|c| live[*c as usize]).collect(),
            None => columns.collect(),
        }
    }

    /// Whether a point lies in the block's closed rectangle.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.x0 as f64 <= x && x <= self.x1 as f64 && self.y0 as f64 <= y && y <= self.y1 as f64
    }

    /// Per move of [`MOVES`]: (dx, dy, column delta, diagonal cell delta, length in base pitches).
    pub fn moves(&self) -> Vec<Move> {
        let (nx, k) = (self.nx, self.factor);
        MOVES
            .iter()
            .map(|&(dx, dy, step)| {
                let cell = (if dy > 0 { 0 } else { -1 }) * nx + (if dx > 0 { 0 } else { -1 });
                (dx, dy, dy * nx + dx, cell, if k == 1 { step } else { step / k as f64 })
            })
            .collect()
    }
}

/// What the router needs from a search lattice.
pub trait Lattice {
    fn pitch_nm(&self) -> i64;
    fn size(&self) -> i64;
    fn layers(&self) -> &[CopperLayer];
    fn nl(&self) -> i64;
    fn plane(&self) -> i64;
    fn point(&self, node: i64) -> Point;
    fn layer_of(&self, node: i64) -> i64 {
        node.div_euclid(self.plane())
    }
    fn column(&self, node: i64) -> i64 {
        node.rem_euclid(self.plane())
    }
    fn column_nodes(&self, column: i64) -> Vec<i64> {
        (0..self.nl()).map(|layer| layer * self.plane() + column).collect()
    }
    /// Per layer, the id offset of its nodes.
    fn layer_bases(&self) -> Vec<i64> {
        (0..self.nl()).map(|layer| layer * self.plane()).collect()
    }
    fn layer_node(&self, column: i64, layer: i64) -> i64 {
        layer * self.plane() + column
    }
    fn node_at(&self, layer: i64, point: Point) -> Option<i64>;
    fn nearest(&self, layer: i64, point: Point) -> i64;
    fn nodes_in(&self, layer: i64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<i64>;
    fn neighbours(&self, node: i64, diagonals: bool) -> Vec<(i64, f64)>;
    fn step_nm(&self, node: i64) -> i64;
}

/// Bounds of an offsets table: `(min dx, max dx, min dy, max dy)`.
fn offset_bounds(offsets: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    if offsets.is_empty() {
        return (0, 0, 0, 0);
    }
    let xs = offsets.iter().map(|o| o.0);
    let ys = offsets.iter().map(|o| o.1);
    (xs.clone().min().unwrap(), xs.max().unwrap(), ys.clone().min().unwrap(), ys.max().unwrap())
}

/// In-bounds offset ids of a block-local position, in `offsets` order.
fn offset_ids(nx: i64, ny: i64, ix: i64, iy: i64, base: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
    let (lo_x, hi_x, lo_y, hi_y) = offset_bounds(offsets);
    if 0 <= ix + lo_x && ix + hi_x < nx && 0 <= iy + lo_y && iy + hi_y < ny {
        return offsets.iter().map(|(ox, oy)| oy * nx + ox + (base + iy * nx + ix)).collect();
    }
    offsets
        .iter()
        .map(|(ox, oy)| (ox + ix, oy + iy))
        .filter(|(x, y)| *x >= 0 && *x < nx && *y >= 0 && *y < ny)
        .map(|(x, y)| y * nx + x + base)
        .collect()
}

/// One pitch over the board outline's bounding box (or a search window).
#[derive(Clone, Debug, PartialEq)]
pub struct UniformLattice {
    pub pitch: i64,
    pub pitch_nm: i64,
    pub x0: i64,
    pub y0: i64,
    pub nx: i64,
    pub ny: i64,
    pub layers: Vec<CopperLayer>,
    pub nl: i64,
    pub plane: i64,
    pub size: i64,
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    pub base_pitch_nm: i64,
    pub patches: Vec<FinePatch>,
    pub blocks: Vec<LatticeBlock>,
    moves: Vec<Move>,
}

impl UniformLattice {
    pub fn new(board: &PhysicalBoard, pitch_nm: i64, window: Option<(i64, i64, i64, i64)>) -> Result<Self> {
        let pitch = pitch_nm;
        let vertices = &board.outline.vertices;
        let min_x = vertices.iter().map(|p| p.x_nm).min().expect("outline vertices");
        let min_y = vertices.iter().map(|p| p.y_nm).min().expect("outline vertices");
        let max_x = vertices.iter().map(|p| p.x_nm).max().expect("outline vertices");
        let max_y = vertices.iter().map(|p| p.y_nm).max().expect("outline vertices");
        let mut x0 = floor_int(true_div(min_x as i128, pitch as i128)) * pitch;
        let mut y0 = floor_int(true_div(min_y as i128, pitch as i128)) * pitch;
        let mut nx = (max_x - x0).div_euclid(pitch) + 2;
        let mut ny = (max_y - y0).div_euclid(pitch) + 2;
        if let Some((wx0, wy0, wx1, wy1)) = window {
            // Retain the full-board lattice phase. A window only discards
            // search nodes; it never translates or clips any source object.
            let ix0 = -(x0 - min_x.max(wx0)).div_euclid(pitch);
            let iy0 = -(y0 - min_y.max(wy0)).div_euclid(pitch);
            let ix1 = (max_x.min(wx1) - x0).div_euclid(pitch);
            let iy1 = (max_y.min(wy1) - y0).div_euclid(pitch);
            x0 += ix0 * pitch;
            y0 += iy0 * pitch;
            nx = ix1 - ix0 + 1;
            ny = iy1 - iy0 + 1;
            if nx < 2 || ny < 2 {
                return fail("grid_window_nm must contain at least two board grid nodes per axis");
            }
        }
        let layers = board.stackup.copper_layers.clone();
        let nl = layers.len() as i64;
        let plane = nx * ny;
        let block = LatticeBlock::new(0, x0, y0, pitch, 1, nx, ny, 0);
        let moves = block.moves();
        Ok(UniformLattice {
            pitch,
            pitch_nm: pitch,
            x0,
            y0,
            nx,
            ny,
            layers,
            nl,
            plane,
            size: plane * nl,
            xs: block.xs.clone(),
            ys: block.ys.clone(),
            base_pitch_nm: pitch,
            patches: Vec::new(),
            blocks: vec![block],
            moves,
        })
    }

    /// (pitch, x0, y0, nx, ny, nl): equal signatures share node ids.
    pub fn signature(&self) -> (i64, i64, i64, i64, i64, i64) {
        (self.pitch, self.x0, self.y0, self.nx, self.ny, self.nl)
    }

    pub fn index(&self, layer: i64, ix: i64, iy: i64) -> i64 {
        layer * self.plane + iy * self.nx + ix
    }

    /// `(layer, ix, iy)`.
    pub fn unpack(&self, node: i64) -> (i64, i64, i64) {
        let (layer, rest) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        (layer, rest.rem_euclid(self.nx), rest.div_euclid(self.nx))
    }

    /// `(layer, column, ix, iy)` of a node in one call.
    pub fn decode(&self, node: i64) -> (i64, i64, i64, i64) {
        let (layer, rest) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        (layer, rest, rest.rem_euclid(self.nx), rest.div_euclid(self.nx))
    }

    /// Index box covering a nanometre box (clipped to the grid).
    pub fn window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        self.blocks[0].window(min_x, min_y, max_x, max_y)
    }

    pub fn column_point(&self, column: i64) -> Point {
        let (iy, ix) = (column.div_euclid(self.nx), column.rem_euclid(self.nx));
        Point { x_nm: self.x0 + ix * self.pitch, y_nm: self.y0 + iy * self.pitch }
    }

    /// Python `round()` of the float ratio: half-way points go to the even index.
    fn rounded(&self, point: Point) -> (i64, i64) {
        (
            round_int(true_div((point.x_nm - self.x0) as i128, self.pitch as i128)),
            round_int(true_div((point.y_nm - self.y0) as i128, self.pitch as i128)),
        )
    }

    /// The node at the rounded lattice position of `point`; None outside the lattice.
    pub fn snap(&self, layer: i64, point: Point) -> Option<i64> {
        let (ix, iy) = self.rounded(point);
        if !(0 <= ix && ix < self.nx && 0 <= iy && iy < self.ny) {
            return None;
        }
        Some(self.index(layer, ix, iy))
    }

    /// (column, x_nm, y_nm) of every node of the covering box, x-major.
    pub fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)> {
        let (ix0, iy0, ix1, iy1) = self.window(min_x, min_y, max_x, max_y);
        let (nx, x0, y0, p) = (self.nx, self.x0, self.y0, self.pitch);
        (ix0..=ix1).flat_map(|ix| (iy0..=iy1).map(move |iy| (iy * nx + ix, x0 + ix * p, y0 + iy * p))).collect()
    }

    /// Columns of an index box (row-major), optionally where `mask` is set.
    pub fn window_columns(&self, window: IndexBox, mask: Option<&Mask<'_>>) -> Vec<i64> {
        box_cells(window, mask).into_iter().map(|(iy, ix)| iy * self.nx + ix).collect()
    }

    /// `window_columns` on `layer` as node ids.
    pub fn window_nodes(&self, layer: i64, window: IndexBox, mask: Option<&Mask<'_>>) -> Vec<i64> {
        let base = layer * self.plane;
        self.window_columns(window, mask).into_iter().map(|c| c + base).collect()
    }

    /// (node, distance) for nodes on `layer` strictly within `radius_nm` of a segment (row-major).
    pub fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)> {
        let (ix0, iy0, ix1, iy1) = self.window(
            start.x_nm.min(end.x_nm) as f64 - radius_nm,
            start.y_nm.min(end.y_nm) as f64 - radius_nm,
            start.x_nm.max(end.x_nm) as f64 + radius_nm,
            start.y_nm.max(end.y_nm) as f64 + radius_nm,
        );
        let (ax, ay, bx, by) = (start.x_nm as i128, start.y_nm as i128, end.x_nm as i128, end.y_nm as i128);
        let (dx, dy) = (bx - ax, by - ay);
        let length2 = dx * dx + dy * dy;
        let (base, nx, x0, y0, p) = (layer * self.plane, self.nx, self.x0, self.y0, self.pitch);
        let mut result = Vec::new();
        for iy in iy0..=iy1 {
            let py = (y0 + iy * p) as i128;
            for ix in ix0..=ix1 {
                let px = (x0 + ix * p) as i128;
                let t = if length2 == 0 { 0.0 } else { clamp_unit(true_div((px - ax) * dx + (py - ay) * dy, length2)) };
                let distance =
                    py_hypot(px as f64 - (ax as f64 + t * dx as f64), py as f64 - (ay as f64 + t * dy as f64));
                if distance < radius_nm {
                    result.push((base + iy * nx + ix, distance));
                }
            }
        }
        result
    }

    /// Per move of [`MOVES`]: (dx, dy, node-id delta, diagonal cell delta, length).
    pub fn move_deltas(&self) -> &[Move] {
        &self.moves
    }

    /// Planar lattice step (dx, dy) from node `a` to node `b`.
    pub fn delta(&self, a: i64, b: i64) -> (i64, i64) {
        let (_, xa, ya) = self.unpack(a.rem_euclid(self.plane));
        let (_, xb, yb) = self.unpack(b.rem_euclid(self.plane));
        (xb - xa, yb - ya)
    }

    /// Cell id (its lower-left node) crossed by the diagonal step a-b, or -1.
    pub fn diagonal_cell(&self, a: i64, b: i64) -> i64 {
        let (la, xa, ya) = self.unpack(a);
        let (_, xb, yb) = self.unpack(b);
        if xa == xb || ya == yb {
            return -1;
        }
        la * self.plane + ya.min(yb) * self.nx + xa.min(xb)
    }

    /// Ids of the in-bounds planar offsets of `node` on its layer, in `offsets` order.
    pub fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (layer, ix, iy) = self.unpack(node);
        offset_ids(self.nx, self.ny, ix, iy, layer * self.plane, offsets)
    }

    /// Columns of the in-bounds planar offsets of `column`, in `offsets` order.
    pub fn offset_columns(&self, column: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (iy, ix) = (column.div_euclid(self.nx), column.rem_euclid(self.nx));
        offset_ids(self.nx, self.ny, ix, iy, 0, offsets)
    }

    /// Columns of the in-bounds offsets around `point`'s rounded lattice position.
    pub fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (ix, iy) = self.rounded(point);
        offset_ids(self.nx, self.ny, ix, iy, 0, offsets)
    }

    pub fn full_box(&self) -> IndexBox {
        (0, 0, self.nx - 1, self.ny - 1)
    }

    /// Bounding index box of nodes (any layer); nodes must not be empty.
    pub fn box_of(&self, nodes: &[i64]) -> Result<IndexBox> {
        let cells: Vec<(i64, i64)> = nodes
            .iter()
            .map(|node| {
                let rest = node.rem_euclid(self.plane);
                (rest.rem_euclid(self.nx), rest.div_euclid(self.nx))
            })
            .collect();
        if cells.is_empty() {
            return fail("box_of needs at least one node");
        }
        Ok((
            cells.iter().map(|c| c.0).min().unwrap(),
            cells.iter().map(|c| c.1).min().unwrap(),
            cells.iter().map(|c| c.0).max().unwrap(),
            cells.iter().map(|c| c.1).max().unwrap(),
        ))
    }

    /// `box` widened by `margin` lattice steps per side, clipped to the lattice.
    pub fn grow_box(&self, index_box: IndexBox, margin: i64) -> IndexBox {
        let (x0, y0, x1, y1) = index_box;
        (0.max(x0 - margin), 0.max(y0 - margin), (self.nx - 1).min(x1 + margin), (self.ny - 1).min(y1 + margin))
    }
}

impl Lattice for UniformLattice {
    fn pitch_nm(&self) -> i64 {
        self.pitch_nm
    }
    fn size(&self) -> i64 {
        self.size
    }
    fn layers(&self) -> &[CopperLayer] {
        &self.layers
    }
    fn nl(&self) -> i64 {
        self.nl
    }
    fn plane(&self) -> i64 {
        self.plane
    }
    fn point(&self, node: i64) -> Point {
        let (_, ix, iy) = self.unpack(node);
        Point { x_nm: self.x0 + ix * self.pitch, y_nm: self.y0 + iy * self.pitch }
    }
    /// The node exactly at `point` on `layer`, or None off the lattice.
    fn node_at(&self, layer: i64, point: Point) -> Option<i64> {
        let (dx, dy) = (point.x_nm - self.x0, point.y_nm - self.y0);
        if dx.rem_euclid(self.pitch) != 0 || dy.rem_euclid(self.pitch) != 0 {
            return None;
        }
        let (ix, iy) = (dx.div_euclid(self.pitch), dy.div_euclid(self.pitch));
        if !(0 <= ix && ix < self.nx && 0 <= iy && iy < self.ny && 0 <= layer && layer < self.nl) {
            return None;
        }
        Some(self.index(layer, ix, iy))
    }
    /// The node nearest `point` on `layer` (rounded as `snap`, clamped).
    fn nearest(&self, layer: i64, point: Point) -> i64 {
        let (ix, iy) = self.rounded(point);
        self.index(layer, ix.max(0).min(self.nx - 1), iy.max(0).min(self.ny - 1))
    }
    /// Ids of the nodes on `layer` with x0 <= x <= x1 and y0 <= y <= y1 (row-major).
    fn nodes_in(&self, layer: i64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<i64> {
        let window = self.blocks[0].inner_window(x0, y0, x1, y1);
        self.window_nodes(layer, window, None)
    }
    /// In-bounds planar moves from `node` in [`MOVES`] order.
    fn neighbours(&self, node: i64, diagonals: bool) -> Vec<(i64, f64)> {
        let (_, ix, iy) = self.unpack(node);
        let moves = if diagonals { &self.moves[..] } else { &self.moves[..4] };
        moves
            .iter()
            .filter(|(dx, dy, ..)| 0 <= ix + dx && ix + dx < self.nx && 0 <= iy + dy && iy + dy < self.ny)
            .map(|(_, _, dn, _, step)| (node + dn, *step))
            .collect()
    }
    fn step_nm(&self, _node: i64) -> i64 {
        self.pitch
    }
}

/// A base lattice plus dense fine blocks over the patches of a fine-region plan.
#[derive(Clone, Debug, PartialEq)]
pub struct NestedLattice {
    pub base: UniformLattice,
    pub plan: FineRegionPlan,
    pub patches: Vec<FinePatch>,
    pub pitch: i64,
    pub base_pitch_nm: i64,
    pub x0: i64,
    pub y0: i64,
    pub nx: i64,
    pub ny: i64,
    pub layers: Vec<CopperLayer>,
    pub nl: i64,
    pub blocks: Vec<LatticeBlock>,
    rects: Vec<IndexBox>,
    pub plane: i64,
    pub size: i64,
    pub pitch_nm: i64,
    offsets_list: Vec<i64>,
    first_fine: i64,
    pub block_of: Vec<u8>,
    pub column_x: Vec<i64>,
    pub column_y: Vec<i64>,
    pub live: Vec<bool>,
    pub live_cells: Vec<bool>,
    pub twins: Vec<i64>,
    pub shadowed: Vec<i64>,
    pub shadowed_cells: Vec<i64>,
    /// (base columns, fine columns).
    pub portal_pairs: (Vec<i64>, Vec<i64>),
    moves: Vec<Vec<Move>>,
}

/// `search_tables`: per column, (x, y) in base pitches, block and twin.
pub type SearchTables = (Vec<f64>, Vec<f64>, Vec<u8>, Vec<i64>);
/// `kernel_tables`: blocks (first column, nx, ny), block of every column, x, y,
/// twins and per block the 8 move lengths.
pub type KernelTables = (Vec<i32>, Vec<u8>, Vec<f64>, Vec<f64>, Vec<i32>, Vec<f64>);

impl NestedLattice {
    pub fn new(
        board: &PhysicalBoard,
        base_pitch_nm: i64,
        plan: &FineRegionPlan,
        window: Option<(i64, i64, i64, i64)>,
    ) -> Result<Self> {
        if window.is_some() {
            return fail("grid_window_nm is not supported with fine_regions");
        }
        if plan.base_pitch_nm != base_pitch_nm {
            return fail("fine-region plan was made for another base pitch");
        }
        let base = UniformLattice::new(board, base_pitch_nm, None)?;
        let bp = base_pitch_nm;
        let mut blocks = vec![LatticeBlock::new(0, base.x0, base.y0, bp, 1, base.nx, base.ny, 0)];
        let mut offset = base.plane;
        let mut rects: Vec<IndexBox> = Vec::new();
        for patch in &plan.patches {
            let k = patch.factor;
            if k < 2 || bp.rem_euclid(k) != 0 {
                return Err(Error::Value(format!("fine patch factor {k} does not divide the base pitch {bp}")));
            }
            for (value, origin) in
                [(patch.x0_nm, base.x0), (patch.x1_nm, base.x0), (patch.y0_nm, base.y0), (patch.y1_nm, base.y0)]
            {
                if (value - origin).rem_euclid(bp) != 0 {
                    return fail("fine patch corners must lie on the base lattice");
                }
            }
            let (i0, j0) = ((patch.x0_nm - base.x0).div_euclid(bp), (patch.y0_nm - base.y0).div_euclid(bp));
            let (i1, j1) = ((patch.x1_nm - base.x0).div_euclid(bp), (patch.y1_nm - base.y0).div_euclid(bp));
            if !(0 <= i0 && 0 <= j0 && i1 < base.nx && j1 < base.ny) {
                return fail("fine patch lies outside the base lattice");
            }
            if i1 - i0 < 2 || j1 - j0 < 2 {
                return fail("fine patch must span at least two base cells per axis");
            }
            for other in &rects {
                if !(i1 < other.0 || other.2 < i0 || j1 < other.1 || other.3 < j0) {
                    return fail("fine patches must not overlap or touch");
                }
            }
            rects.push((i0, j0, i1, j1));
            let (cx, cy) = (i1 - i0, j1 - j0);
            let block = LatticeBlock::new(
                blocks.len(),
                patch.x0_nm,
                patch.y0_nm,
                bp.div_euclid(k),
                k,
                cx * k + 1,
                cy * k + 1,
                offset,
            );
            offset += block.plane;
            blocks.push(block);
        }
        let plane = offset;
        let offsets_list: Vec<i64> = blocks.iter().map(|b| b.column_offset).collect();
        let first_fine = if blocks.len() > 1 { offsets_list[1] } else { offset };
        let size = plane as usize;
        let mut block_of = vec![0u8; size];
        let (mut column_x, mut column_y) = (Vec::with_capacity(size), Vec::with_capacity(size));
        for block in &blocks {
            let start = block.column_offset as usize;
            block_of[start..start + block.plane as usize].fill(block.index as u8);
            for gy in 0..block.ny {
                for gx in 0..block.nx {
                    column_x.push(block.x0 + gx * block.pitch);
                    column_y.push(block.y0 + gy * block.pitch);
                }
            }
        }
        let mut live = vec![true; size];
        let mut live_cells = vec![true; size];
        let mut twins = vec![-1i64; size];
        let nx = base.nx;
        for (&(i0, j0, i1, j1), block) in rects.iter().zip(&blocks[1..]) {
            for iy in j0 + 1..j1 {
                for ix in i0 + 1..i1 {
                    live[(iy * nx + ix) as usize] = false;
                }
            }
            for iy in j0..j1 {
                for ix in i0..i1 {
                    live_cells[(iy * nx + ix) as usize] = false;
                }
            }
            let k = block.factor;
            let mut fy = 0;
            while fy < block.ny {
                let mut fx = 0;
                while fx < block.nx {
                    if fx == 0 || fx == block.nx - 1 || fy == 0 || fy == block.ny - 1 {
                        let fine = block.column_offset + fy * block.nx + fx;
                        let coarse = (fy.div_euclid(k) + j0) * nx + (fx.div_euclid(k) + i0);
                        twins[fine as usize] = coarse;
                        twins[coarse as usize] = fine;
                    }
                    fx += k;
                }
                fy += k;
            }
        }
        blocks[0].live = Some(live.clone());
        let shadowed = (0..plane).filter(|c| !live[*c as usize]).collect();
        let shadowed_cells = (0..plane).filter(|c| !live_cells[*c as usize]).collect();
        let pairs: Vec<i64> = (0..base.plane).filter(|c| twins[*c as usize] >= 0).collect();
        let fine_pairs = pairs.iter().map(|c| twins[*c as usize]).collect();
        let moves = blocks.iter().map(LatticeBlock::moves).collect();
        Ok(NestedLattice {
            plan: plan.clone(),
            patches: plan.patches.clone(),
            pitch: bp,
            base_pitch_nm: bp,
            x0: base.x0,
            y0: base.y0,
            nx: base.nx,
            ny: base.ny,
            layers: base.layers.clone(),
            nl: base.nl,
            pitch_nm: blocks.iter().map(|b| b.pitch).min().expect("base block"),
            blocks,
            rects,
            plane,
            size: plane * base.nl,
            offsets_list,
            first_fine,
            block_of,
            column_x,
            column_y,
            live,
            live_cells,
            twins,
            shadowed,
            shadowed_cells,
            portal_pairs: (pairs, fine_pairs),
            moves,
            base,
        })
    }

    /// Base signature plus every patch: equal signatures share node ids.
    pub fn signature(&self) -> ((i64, i64, i64, i64, i64, i64), Vec<(i64, i64, i64, i64, i64)>) {
        (self.base.signature(), self.patches.iter().map(|p| (p.x0_nm, p.y0_nm, p.x1_nm, p.y1_nm, p.factor)).collect())
    }

    /// Distinct live lattice sites, all layers (twins once, shadowed nodes not at all).
    pub fn node_count(&self) -> i64 {
        (self.live.iter().filter(|l| **l).count() as i64 - self.portal_pairs.0.len() as i64) * self.nl
    }

    /// Index of the block holding `column`.
    pub fn block_index(&self, column: i64) -> usize {
        if column < self.first_fine {
            return 0;
        }
        self.offsets_list.partition_point(|offset| *offset <= column) - 1
    }

    /// The fine block whose closed rectangle holds `point`, else the base.
    pub fn block_at(&self, point: Point) -> &LatticeBlock {
        self.blocks[1..]
            .iter()
            .find(|block| block.contains(point.x_nm as f64, point.y_nm as f64))
            .unwrap_or(&self.blocks[0])
    }

    /// Per column, its area in base cells: `(step / base pitch) ** 2`.
    pub fn column_weight(&self) -> Vec<f64> {
        let mut weights = vec![1.0; self.plane as usize];
        for block in &self.blocks[1..] {
            let start = block.column_offset as usize;
            weights[start..start + block.plane as usize].fill(1.0 / (block.factor * block.factor) as f64);
        }
        weights
    }

    pub fn column_point(&self, column: i64) -> Point {
        Point { x_nm: self.column_x[column as usize], y_nm: self.column_y[column as usize] }
    }

    /// (block, ix, iy) of a column.
    pub fn local(&self, column: i64) -> (&LatticeBlock, i64, i64) {
        let block = &self.blocks[self.block_index(column)];
        let rest = column - block.column_offset;
        (block, rest.rem_euclid(block.nx), rest.div_euclid(block.nx))
    }

    /// (layer, column, block-local ix, block-local iy).
    pub fn decode(&self, node: i64) -> (i64, i64, i64, i64) {
        let (layer, column) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        let (_, ix, iy) = self.local(column);
        (layer, column, ix, iy)
    }

    /// The portal twin of `node` (same layer), or -1.
    pub fn twin(&self, node: i64) -> i64 {
        let (layer, column) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        let other = self.twins[column as usize];
        if other < 0 { -1 } else { layer * self.plane + other }
    }

    /// Node ids of `nodes` plus the twins of those that have one.
    pub fn with_twins(&self, nodes: &[i64]) -> Vec<i64> {
        let mut result = nodes.to_vec();
        for node in nodes {
            let column = node.rem_euclid(self.plane);
            let other = self.twins[column as usize];
            if other >= 0 {
                result.push(node - column + other);
            }
        }
        result
    }

    /// Columns plus the twins of those that have one.
    pub fn columns_with_twins(&self, columns: &[i64]) -> Vec<i64> {
        let mut result = columns.to_vec();
        result.extend(columns.iter().map(|c| self.twins[*c as usize]).filter(|o| *o >= 0));
        result
    }

    fn rounded(block: &LatticeBlock, point: Point) -> (i64, i64) {
        (
            round_int(true_div((point.x_nm - block.x0) as i128, block.pitch as i128)),
            round_int(true_div((point.y_nm - block.y0) as i128, block.pitch as i128)),
        )
    }

    /// The node at the rounded position of `point` in its block; None off the lattice.
    pub fn snap(&self, layer: i64, point: Point) -> Option<i64> {
        let block = self.block_at(point);
        let (ix, iy) = Self::rounded(block, point);
        if !(0 <= ix && ix < block.nx && 0 <= iy && iy < block.ny) {
            return None;
        }
        Some(layer * self.plane + block.column_offset + iy * block.nx + ix)
    }

    fn overlapping(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<&LatticeBlock> {
        self.blocks
            .iter()
            .filter(|b| !(x1 < b.x0 as f64 || (b.x1 as f64) < x0 || y1 < b.y0 as f64 || (b.y1 as f64) < y0))
            .collect()
    }

    /// (column, x_nm, y_nm) of the live columns of every block's covering box,
    /// x-major. Of a twin pair only the fine column is listed.
    pub fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)> {
        let blocks = self.overlapping(min_x, min_y, max_x, max_y);
        let parts: Vec<Vec<i64>> =
            blocks.iter().map(|b| b.window_columns(b.window(min_x, min_y, max_x, max_y), None)).collect();
        let mut columns: Vec<i64> = parts.concat();
        if columns.is_empty() {
            return Vec::new();
        }
        if blocks[0].index == 0 && parts.len() > 1 && !parts[0].is_empty() {
            let (base, fine) = columns.split_at(parts[0].len());
            let mut kept: Vec<i64> = base
                .iter()
                .copied()
                .filter(|c| {
                    let other = self.twins[*c as usize];
                    !(other >= 0 && fine.contains(&other))
                })
                .collect();
            kept.extend_from_slice(fine);
            columns = kept;
        }
        let mut order: Vec<usize> = (0..columns.len()).collect();
        order.sort_by_key(|i| {
            let c = columns[*i] as usize;
            (self.column_x[c], self.column_y[c])
        });
        order
            .into_iter()
            .map(|i| {
                let c = columns[i];
                (c, self.column_x[c as usize], self.column_y[c as usize])
            })
            .collect()
    }

    /// Live columns strictly within `radius_nm` of `point` (block by block).
    pub fn columns_within(&self, point: Point, radius_nm: f64) -> Vec<i64> {
        let (x, y) = (point.x_nm as f64, point.y_nm as f64);
        let mut result = Vec::new();
        for block in self.overlapping(x - radius_nm, y - radius_nm, x + radius_nm, y + radius_nm) {
            let window = block.inner_window(x - radius_nm, y - radius_nm, x + radius_nm, y + radius_nm);
            for column in block.window_columns(window, None) {
                let c = column as usize;
                // numpy: int64 differences, then the C library's hypot.
                let d = ((self.column_x[c] - point.x_nm) as f64).hypot((self.column_y[c] - point.y_nm) as f64);
                if d < radius_nm {
                    result.push(column);
                }
            }
        }
        result
    }

    /// Live nodes on `layer` strictly within `radius_nm` of `point`.
    pub fn nodes_within(&self, layer: i64, point: Point, radius_nm: f64) -> Vec<i64> {
        let base = layer * self.plane;
        self.columns_within(point, radius_nm).into_iter().map(|c| c + base).collect()
    }

    fn segment_columns(
        &self,
        start: Point,
        end: Point,
        radius_of: impl Fn(&LatticeBlock) -> f64,
    ) -> Vec<(&LatticeBlock, Vec<i64>, Vec<f64>)> {
        let (ax, ay, bx, by) = (start.x_nm, start.y_nm, end.x_nm, end.y_nm);
        let reach = self.blocks.iter().map(&radius_of).fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a });
        let (lo_x, lo_y, hi_x, hi_y) = (ax.min(bx) as f64, ay.min(by) as f64, ax.max(bx) as f64, ay.max(by) as f64);
        let (dx, dy) = ((bx - ax) as f64, (by - ay) as f64);
        let length2 = dx * dx + dy * dy;
        let (fax, fay) = (ax as f64, ay as f64);
        let mut result = Vec::new();
        for block in self.overlapping(lo_x - reach, lo_y - reach, hi_x + reach, hi_y + reach) {
            let r = radius_of(block);
            let columns = block.window_columns(block.window(lo_x - r, lo_y - r, hi_x + r, hi_y + r), None);
            if columns.is_empty() {
                continue;
            }
            let (mut kept, mut distances) = (Vec::new(), Vec::new());
            for column in columns {
                let px = self.column_x[column as usize] as f64;
                let py = self.column_y[column as usize] as f64;
                let t = if length2 != 0.0 { clip_unit(((px - fax) * dx + (py - fay) * dy) / length2) } else { 0.0 };
                let d = (px - (fax + t * dx)).hypot(py - (fay + t * dy));
                if d < r {
                    kept.push(column);
                    distances.push(d);
                }
            }
            result.push((block, kept, distances));
        }
        result
    }

    /// (node, distance) for live nodes on `layer` strictly within `radius_nm` of a segment.
    pub fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)> {
        let base = layer * self.plane;
        self.segment_columns(start, end, |_| radius_nm)
            .into_iter()
            .flat_map(|(_, columns, distances)| columns.into_iter().zip(distances).map(move |(c, d)| (c + base, d)))
            .collect()
    }

    /// (node, distance, weight) for live nodes on `layer` strictly within
    /// `half_steps` of their own block's pitch (plus `extra_nm`) of a segment;
    /// `weight` is that pitch in base pitches.
    pub fn segment_nodes_local(
        &self,
        layer: i64,
        start: Point,
        end: Point,
        half_steps: f64,
        extra_nm: f64,
    ) -> Vec<(i64, f64, f64)> {
        let base = layer * self.plane;
        let pitch = self.pitch;
        self.segment_columns(start, end, |block| half_steps * block.pitch as f64 + extra_nm)
            .into_iter()
            .flat_map(|(block, columns, distances)| {
                let weight = true_div(block.pitch as i128, pitch as i128);
                columns.into_iter().zip(distances).map(move |(c, d)| (c + base, d, weight))
            })
            .collect()
    }

    /// Whether a nanometre box lies wholly in `block`'s own region.
    pub fn owns(&self, block: &LatticeBlock, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
        if block.index != 0 {
            return block.x0 as f64 <= x0 && x1 <= block.x1 as f64 && block.y0 as f64 <= y0 && y1 <= block.y1 as f64;
        }
        !self.blocks[1..]
            .iter()
            .any(|b| !(x1 < b.x0 as f64 || (b.x1 as f64) < x0 || y1 < b.y0 as f64 || (b.y1 as f64) < y0))
    }

    /// `LatticeBlock::moves` of block `index` (lengths in base pitches).
    pub fn block_moves(&self, index: usize) -> &[Move] {
        &self.moves[index]
    }

    /// The base block's moves.
    pub fn move_deltas(&self) -> &[Move] {
        &self.moves[0]
    }

    /// Direction (sign x, sign y) from node `a` to `b`: the lattice step
    /// within a block, (0, 0) across a portal.
    pub fn delta(&self, a: i64, b: i64) -> (i64, i64) {
        let (ca, cb) = (a.rem_euclid(self.plane) as usize, b.rem_euclid(self.plane) as usize);
        let dx = self.column_x[cb] - self.column_x[ca];
        let dy = self.column_y[cb] - self.column_y[ca];
        (dx.signum(), dy.signum())
    }

    /// Cell id (its lower-left node) crossed by the in-block diagonal step a-b, or -1.
    pub fn diagonal_cell(&self, a: i64, b: i64) -> i64 {
        let (layer, ca) = (a.div_euclid(self.plane), a.rem_euclid(self.plane));
        let (block, xa, ya) = self.local(ca);
        let (other, xb, yb) = self.local(b.rem_euclid(self.plane));
        if other.index != block.index || xa == xb || ya == yb {
            return -1;
        }
        layer * self.plane + block.column_offset + ya.min(yb) * block.nx + xa.min(xb)
    }

    fn offset_ids(block: &LatticeBlock, ix: i64, iy: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let ids = offset_ids(block.nx, block.ny, ix, iy, block.column_offset, offsets);
        match &block.live {
            Some(live) => ids.into_iter().filter(|id| live[*id as usize]).collect(),
            None => ids,
        }
    }

    /// Live columns of the in-block offsets (in the block's own pitch) of `column`.
    pub fn offset_columns(&self, column: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (block, ix, iy) = self.local(column);
        Self::offset_ids(block, ix, iy, offsets)
    }

    /// `offset_columns` of `node`'s column, on its layer.
    pub fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (layer, column) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        let base = layer * self.plane;
        self.offset_columns(column, offsets).into_iter().map(|c| c + base).collect()
    }

    /// Live in-block offsets around `point`'s rounded position in its block.
    pub fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64> {
        let block = self.block_at(point);
        let (ix, iy) = Self::rounded(block, point);
        Self::offset_ids(block, ix, iy, offsets)
    }

    pub fn full_box(&self) -> IndexBox {
        (0, 0, self.nx - 1, self.ny - 1)
    }

    /// Base-lattice index box covering nodes (any layer, any block); not empty.
    pub fn box_of(&self, nodes: &[i64]) -> Result<IndexBox> {
        if nodes.is_empty() {
            return fail("box_of needs at least one node");
        }
        let columns: Vec<usize> = nodes.iter().map(|n| n.rem_euclid(self.plane) as usize).collect();
        let xs = columns.iter().map(|c| self.column_x[*c]);
        let ys = columns.iter().map(|c| self.column_y[*c]);
        let p = self.pitch;
        let (min_x, max_x) = (xs.clone().min().unwrap(), xs.max().unwrap());
        let (min_y, max_y) = (ys.clone().min().unwrap(), ys.max().unwrap());
        Ok((
            (min_x - self.x0).div_euclid(p),
            (min_y - self.y0).div_euclid(p),
            -(self.x0 - max_x).div_euclid(p),
            -(self.y0 - max_y).div_euclid(p),
        ))
    }

    /// `box` widened by `margin` base steps per side, clipped to the lattice.
    pub fn grow_box(&self, index_box: IndexBox, margin: i64) -> IndexBox {
        let (x0, y0, x1, y1) = index_box;
        (0.max(x0 - margin), 0.max(y0 - margin), (self.nx - 1).min(x1 + margin), (self.ny - 1).min(y1 + margin))
    }

    /// (x, y) of every column in base pitches from the base origin, the block
    /// of every column and the twin column of every column (-1 for none).
    pub fn search_tables(&self) -> SearchTables {
        let p = self.pitch as f64;
        (
            self.column_x.iter().map(|x| (x - self.x0) as f64 / p).collect(),
            self.column_y.iter().map(|y| (y - self.y0) as f64 / p).collect(),
            self.block_of.clone(),
            self.twins.clone(),
        )
    }

    /// The block tables of `search_tables` for the native kernel.
    pub fn kernel_tables(&self) -> Result<KernelTables> {
        if self.blocks.len() > 256 || self.size >= 1 << 31 {
            return fail("nested lattice exceeds the native kernel's block or node limits");
        }
        let (xs, ys, block_of, twins) = self.search_tables();
        let blocks = self.blocks.iter().flat_map(|b| [b.column_offset as i32, b.nx as i32, b.ny as i32]).collect();
        let steps = self.moves.iter().flat_map(|moves| moves.iter().map(|m| m.4)).collect();
        Ok((blocks, block_of, xs, ys, twins.into_iter().map(|t| t as i32).collect(), steps))
    }
}

impl Lattice for NestedLattice {
    fn pitch_nm(&self) -> i64 {
        self.pitch_nm
    }
    fn size(&self) -> i64 {
        self.size
    }
    fn layers(&self) -> &[CopperLayer] {
        &self.layers
    }
    fn nl(&self) -> i64 {
        self.nl
    }
    fn plane(&self) -> i64 {
        self.plane
    }
    fn point(&self, node: i64) -> Point {
        self.column_point(node.rem_euclid(self.plane))
    }
    /// The live node exactly at `point` (the fine one of a twin pair), or None.
    fn node_at(&self, layer: i64, point: Point) -> Option<i64> {
        if !(0 <= layer && layer < self.nl) {
            return None;
        }
        let block = self.block_at(point);
        let (dx, dy) = (point.x_nm - block.x0, point.y_nm - block.y0);
        if dx.rem_euclid(block.pitch) != 0 || dy.rem_euclid(block.pitch) != 0 {
            return None;
        }
        let (ix, iy) = (dx.div_euclid(block.pitch), dy.div_euclid(block.pitch));
        if !(0 <= ix && ix < block.nx && 0 <= iy && iy < block.ny) {
            return None;
        }
        Some(layer * self.plane + block.column_offset + iy * block.nx + ix)
    }
    /// The live node nearest `point` on `layer` (rounded in its block, clamped).
    fn nearest(&self, layer: i64, point: Point) -> i64 {
        let block = self.block_at(point);
        let (ix, iy) = Self::rounded(block, point);
        let (ix, iy) = (ix.max(0).min(block.nx - 1), iy.max(0).min(block.ny - 1));
        layer * self.plane + block.column_offset + iy * block.nx + ix
    }
    /// Ids of the live nodes on `layer` inside a box (block by block, row-major).
    fn nodes_in(&self, layer: i64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<i64> {
        let base = layer * self.plane;
        self.overlapping(x0, y0, x1, y1)
            .into_iter()
            .flat_map(|block| block.window_columns(block.inner_window(x0, y0, x1, y1), None))
            .map(|c| c + base)
            .collect()
    }
    /// In-block planar moves to live nodes in [`MOVES`] order, then the portal twin at length 0.
    fn neighbours(&self, node: i64, diagonals: bool) -> Vec<(i64, f64)> {
        let (layer, column) = (node.div_euclid(self.plane), node.rem_euclid(self.plane));
        let (block, ix, iy) = self.local(column);
        let moves = &self.moves[block.index];
        let moves = if diagonals { &moves[..] } else { &moves[..4] };
        let mut result = Vec::new();
        for &(dx, dy, dn, dcell, step) in moves {
            if !(0 <= ix + dx && ix + dx < block.nx && 0 <= iy + dy && iy + dy < block.ny) {
                continue;
            }
            if !self.live[(column + dn) as usize] || (dx != 0 && dy != 0 && !self.live_cells[(column + dcell) as usize])
            {
                continue;
            }
            result.push((node + dn, step));
        }
        let twin = self.twins[column as usize];
        if twin >= 0 {
            result.push((layer * self.plane + twin, 0.0));
        }
        result
    }
    fn step_nm(&self, node: i64) -> i64 {
        self.blocks[self.block_index(node.rem_euclid(self.plane))].pitch
    }
}

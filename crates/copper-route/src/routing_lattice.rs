//! Search lattice of the unified router (`pcbir.routing_lattice`).
//!
//! Node ids are dense: `layer * plane + column`, a column being the planar
//! id shared by every layer at one lattice point. [`UniformLattice`] is one
//! pitch over the board outline's bounding box (or a search window), one
//! [`LatticeBlock`]. The nested lattice of fine-region patches is not
//! rewritten yet (`NestedLattice` reports not implemented).
//!
//! PENDING: owned by `rust-m4-global`; ported here because the unified
//! router needs it.

use num_integer::Integer;

use copper_pcb::board::PhysicalBoard;
use copper_physical::pyfloat::round_int;
use copper_physical::{CopperLayer, Point};

const SQRT2: f64 = std::f64::consts::SQRT_2;

/// The 8 planar moves `(dx, dy, length in pitches)`; the order is part of the
/// search contract (move tables, turn table, kernel).
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

/// A move: `(dx, dy, node delta, diagonal cell delta, length)`.
pub type Move = (i64, i64, i64, i64, f64);

/// An inclusive index box `(ix0, iy0, ix1, iy1)`.
pub type IndexBox = (i64, i64, i64, i64);

/// Python's `int / int`: the correctly rounded quotient.
pub fn true_div(numerator: i128, denominator: i128) -> f64 {
    const EXACT: i128 = 1 << 53;
    if numerator.abs() <= EXACT && denominator.abs() <= EXACT {
        return numerator as f64 / denominator as f64;
    }
    copper_physical::pyfloat::big_ratio_to_f64(&numerator.into(), &denominator.into())
}

fn moves_of(nx: i64, factor: i64) -> [Move; 8] {
    MOVES.map(|(dx, dy, step)| {
        let cell = (if dy > 0 { 0 } else { -1 }) * nx + if dx > 0 { 0 } else { -1 };
        (dx, dy, dy * nx + dx, cell, if factor == 1 { step } else { step / factor as f64 })
    })
}

/// `LatticeBlock`: one dense regular block (origin, pitch, shape).
#[derive(Clone, Debug)]
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
    /// Live columns of a block whose nodes may be shadowed (nested base).
    pub live: Option<Vec<bool>>,
}

impl LatticeBlock {
    #[allow(clippy::too_many_arguments)]
    pub fn new(index: usize, x0: i64, y0: i64, pitch: i64, factor: i64, nx: i64, ny: i64, column_offset: i64) -> Self {
        let xs = (0..nx).map(|i| x0 as f64 + (pitch * i) as f64).collect();
        let ys = (0..ny).map(|i| y0 as f64 + (pitch * i) as f64).collect();
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
            xs,
            ys,
            x1: x0 + (nx - 1) * pitch,
            y1: y0 + (ny - 1) * pitch,
            live: None,
        }
    }

    /// Block index box covering a nanometre box (clipped; may be empty).
    pub fn window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        let p = self.pitch as f64;
        (
            0.max(((min_x - self.x0 as f64) / p).floor() as i64),
            0.max(((min_y - self.y0 as f64) / p).floor() as i64),
            (self.nx - 1).min(((max_x - self.x0 as f64) / p).ceil() as i64),
            (self.ny - 1).min(((max_y - self.y0 as f64) / p).ceil() as i64),
        )
    }

    /// Block index box of the nodes inside a nanometre box.
    pub fn inner_window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        let p = self.pitch as f64;
        (
            0.max(((min_x - self.x0 as f64) / p).ceil() as i64),
            0.max(((min_y - self.y0 as f64) / p).ceil() as i64),
            (self.nx - 1).min(((max_x - self.x0 as f64) / p).floor() as i64),
            (self.ny - 1).min(((max_y - self.y0 as f64) / p).floor() as i64),
        )
    }

    /// Live columns of an index box (row-major), optionally where `mask`
    /// (row-major over the box) is true.
    pub fn window_columns(&self, window: IndexBox, mask: Option<&[bool]>) -> Vec<i64> {
        let (ix0, iy0, ix1, iy1) = window;
        if ix0 > ix1 || iy0 > iy1 {
            return Vec::new();
        }
        let width = (ix1 - ix0 + 1) as usize;
        let mut columns = Vec::new();
        for iy in iy0..=iy1 {
            for ix in ix0..=ix1 {
                if let Some(mask) = mask
                    && !mask[(iy - iy0) as usize * width + (ix - ix0) as usize]
                {
                    continue;
                }
                let column = iy * self.nx + ix + self.column_offset;
                if self.live.as_ref().is_none_or(|live| live[column as usize]) {
                    columns.push(column);
                }
            }
        }
        columns
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.x0 as f64 <= x && x <= self.x1 as f64 && self.y0 as f64 <= y && y <= self.y1 as f64
    }

    pub fn moves(&self) -> [Move; 8] {
        moves_of(self.nx, self.factor)
    }
}

/// `UniformLattice`: one pitch over the outline's bounding box or a window.
#[derive(Clone, Debug)]
pub struct UniformLattice {
    pub pitch: i64,
    pub pitch_nm: i64,
    pub base_pitch_nm: i64,
    pub x0: i64,
    pub y0: i64,
    pub nx: i64,
    pub ny: i64,
    pub layers: Vec<CopperLayer>,
    pub nl: i64,
    pub plane: i64,
    pub size: i64,
    pub blocks: Vec<LatticeBlock>,
    moves: [Move; 8],
}

impl UniformLattice {
    pub fn new(
        board: &PhysicalBoard,
        pitch: i64,
        window: Option<(i64, i64, i64, i64)>,
    ) -> Result<UniformLattice, String> {
        let vertices = &board.outline.vertices;
        let min_x = vertices.iter().map(|p| p.x_nm).min().expect("outline");
        let min_y = vertices.iter().map(|p| p.y_nm).min().expect("outline");
        let max_x = vertices.iter().map(|p| p.x_nm).max().expect("outline");
        let max_y = vertices.iter().map(|p| p.y_nm).max().expect("outline");
        let mut x0 = (min_x as f64 / pitch as f64).floor() as i64 * pitch;
        let mut y0 = (min_y as f64 / pitch as f64).floor() as i64 * pitch;
        let mut nx = Integer::div_floor(&(max_x - x0), &pitch) + 2;
        let mut ny = Integer::div_floor(&(max_y - y0), &pitch) + 2;
        if let Some((wx0, wy0, wx1, wy1)) = window {
            let ix0 = -Integer::div_floor(&(x0 - min_x.max(wx0)), &pitch);
            let iy0 = -Integer::div_floor(&(y0 - min_y.max(wy0)), &pitch);
            let ix1 = Integer::div_floor(&(max_x.min(wx1) - x0), &pitch);
            let iy1 = Integer::div_floor(&(max_y.min(wy1) - y0), &pitch);
            x0 += ix0 * pitch;
            y0 += iy0 * pitch;
            nx = ix1 - ix0 + 1;
            ny = iy1 - iy0 + 1;
            if nx < 2 || ny < 2 {
                return Err("grid_window_nm must contain at least two board grid nodes per axis".to_owned());
            }
        }
        let layers = board.stackup.copper_layers.clone();
        let nl = layers.len() as i64;
        Ok(UniformLattice {
            pitch,
            pitch_nm: pitch,
            base_pitch_nm: pitch,
            x0,
            y0,
            nx,
            ny,
            layers,
            nl,
            plane: nx * ny,
            size: nx * ny * nl,
            blocks: vec![LatticeBlock::new(0, x0, y0, pitch, 1, nx, ny, 0)],
            moves: moves_of(nx, 1),
        })
    }

    pub fn xs(&self) -> &[f64] {
        &self.blocks[0].xs
    }

    pub fn ys(&self) -> &[f64] {
        &self.blocks[0].ys
    }

    /// `(pitch, x0, y0, nx, ny, nl)`: equal signatures share node ids.
    pub fn signature(&self) -> (i64, i64, i64, i64, i64, i64) {
        (self.pitch, self.x0, self.y0, self.nx, self.ny, self.nl)
    }

    pub fn step_nm(&self, _node: i64) -> i64 {
        self.pitch
    }

    pub fn index(&self, layer: i64, ix: i64, iy: i64) -> i64 {
        layer * self.plane + iy * self.nx + ix
    }

    /// `(layer, ix, iy)`.
    pub fn unpack(&self, node: i64) -> (i64, i64, i64) {
        let (layer, rest) = node.div_mod_floor(&self.plane);
        let (iy, ix) = rest.div_mod_floor(&self.nx);
        (layer, ix, iy)
    }

    /// `(layer, column, ix, iy)`.
    pub fn decode(&self, node: i64) -> (i64, i64, i64, i64) {
        let (layer, rest) = node.div_mod_floor(&self.plane);
        let (iy, ix) = rest.div_mod_floor(&self.nx);
        (layer, rest, ix, iy)
    }

    pub fn point(&self, node: i64) -> Point {
        let (_, ix, iy) = self.unpack(node);
        Point::new(self.x0 + ix * self.pitch, self.y0 + iy * self.pitch)
    }

    pub fn window(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> IndexBox {
        self.blocks[0].window(min_x, min_y, max_x, max_y)
    }

    pub fn layer_of(&self, node: i64) -> i64 {
        Integer::div_floor(&node, &self.plane)
    }

    pub fn column(&self, node: i64) -> i64 {
        node.mod_floor(&self.plane)
    }

    pub fn column_nodes(&self, column: i64) -> Vec<i64> {
        (0..self.nl).map(|layer| layer * self.plane + column).collect()
    }

    pub fn layer_bases(&self) -> Vec<i64> {
        (0..self.nl).map(|layer| layer * self.plane).collect()
    }

    pub fn layer_node(&self, column: i64, layer: i64) -> i64 {
        layer * self.plane + column
    }

    pub fn column_point(&self, column: i64) -> Point {
        let (iy, ix) = column.div_mod_floor(&self.nx);
        Point::new(self.x0 + ix * self.pitch, self.y0 + iy * self.pitch)
    }

    fn rounded(&self, point: Point) -> (i64, i64) {
        let p = self.pitch as f64;
        (round_int((point.x_nm - self.x0) as f64 / p), round_int((point.y_nm - self.y0) as f64 / p))
    }

    /// The node exactly at `point` on `layer`, or `None` off the lattice.
    pub fn node_at(&self, layer: i64, point: Point) -> Option<i64> {
        let (dx, dy) = (point.x_nm - self.x0, point.y_nm - self.y0);
        if dx.mod_floor(&self.pitch) != 0 || dy.mod_floor(&self.pitch) != 0 {
            return None;
        }
        let (ix, iy) = (Integer::div_floor(&dx, &self.pitch), Integer::div_floor(&dy, &self.pitch));
        if !(0 <= ix && ix < self.nx && 0 <= iy && iy < self.ny && 0 <= layer && layer < self.nl) {
            return None;
        }
        Some(self.index(layer, ix, iy))
    }

    /// The node at the rounded position of `point`; `None` outside.
    pub fn snap(&self, layer: i64, point: Point) -> Option<i64> {
        let (ix, iy) = self.rounded(point);
        if !(0 <= ix && ix < self.nx && 0 <= iy && iy < self.ny) {
            return None;
        }
        Some(self.index(layer, ix, iy))
    }

    /// The node nearest `point` on `layer` (rounded, clamped).
    pub fn nearest(&self, layer: i64, point: Point) -> i64 {
        let (ix, iy) = self.rounded(point);
        self.index(layer, ix.clamp(0, self.nx - 1), iy.clamp(0, self.ny - 1))
    }

    /// Ids of the nodes on `layer` inside a closed box (row-major).
    pub fn nodes_in(&self, layer: i64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<i64> {
        self.window_nodes(layer, self.blocks[0].inner_window(x0, y0, x1, y1), None)
    }

    /// `(column, x_nm, y_nm)` of every node of the covering box, x-major.
    pub fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)> {
        let (ix0, iy0, ix1, iy1) = self.window(min_x, min_y, max_x, max_y);
        let mut result = Vec::new();
        for ix in ix0..=ix1 {
            for iy in iy0..=iy1 {
                result.push((iy * self.nx + ix, self.x0 + ix * self.pitch, self.y0 + iy * self.pitch));
            }
        }
        result
    }

    pub fn window_columns(&self, window: IndexBox, mask: Option<&[bool]>) -> Vec<i64> {
        self.blocks[0].window_columns(window, mask)
    }

    pub fn window_nodes(&self, layer: i64, window: IndexBox, mask: Option<&[bool]>) -> Vec<i64> {
        self.window_columns(window, mask).into_iter().map(|column| column + layer * self.plane).collect()
    }

    /// `(node, distance)` strictly within `radius_nm` of a segment (row-major).
    pub fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)> {
        let (ix0, iy0, ix1, iy1) = self.window(
            start.x_nm.min(end.x_nm) as f64 - radius_nm,
            start.y_nm.min(end.y_nm) as f64 - radius_nm,
            start.x_nm.max(end.x_nm) as f64 + radius_nm,
            start.y_nm.max(end.y_nm) as f64 + radius_nm,
        );
        let (ax, ay, bx, by) = (start.x_nm, start.y_nm, end.x_nm, end.y_nm);
        let (dx, dy) = (bx - ax, by - ay);
        let length2 = dx * dx + dy * dy;
        let base = layer * self.plane;
        let mut result = Vec::new();
        for iy in iy0..=iy1 {
            let py = self.y0 + iy * self.pitch;
            for ix in ix0..=ix1 {
                let px = self.x0 + ix * self.pitch;
                let t = if length2 == 0 {
                    0.0
                } else {
                    true_div(((px - ax) * dx + (py - ay) * dy) as i128, length2 as i128).clamp(0.0, 1.0)
                };
                let distance = copper_physical::pyfloat::hypot(
                    px as f64 - (ax as f64 + t * dx as f64),
                    py as f64 - (ay as f64 + t * dy as f64),
                );
                if distance < radius_nm {
                    result.push((base + iy * self.nx + ix, distance));
                }
            }
        }
        result
    }

    pub fn move_deltas(&self) -> &[Move; 8] {
        &self.moves
    }

    /// In-bounds planar moves from `node` in `MOVES` order.
    pub fn neighbours(&self, node: i64, diagonals: bool) -> Vec<(i64, f64)> {
        let (_, ix, iy) = self.unpack(node);
        let moves = if diagonals { &self.moves[..] } else { &self.moves[..4] };
        moves
            .iter()
            .filter(|(dx, dy, ..)| 0 <= ix + dx && ix + dx < self.nx && 0 <= iy + dy && iy + dy < self.ny)
            .map(|(_, _, dn, _, step)| (node + dn, *step))
            .collect()
    }

    pub fn delta(&self, a: i64, b: i64) -> (i64, i64) {
        let (ya, xa) = a.mod_floor(&self.plane).div_mod_floor(&self.nx);
        let (yb, xb) = b.mod_floor(&self.plane).div_mod_floor(&self.nx);
        (xb - xa, yb - ya)
    }

    /// Cell id (lower-left node) crossed by the diagonal step `a`-`b`, or -1.
    pub fn diagonal_cell(&self, a: i64, b: i64) -> i64 {
        let (la, xa, ya) = self.unpack(a);
        let (_, xb, yb) = self.unpack(b);
        if xa == xb || ya == yb {
            return -1;
        }
        la * self.plane + ya.min(yb) * self.nx + xa.min(xb)
    }

    fn offset_ids(&self, ix: i64, iy: i64, base: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        offsets
            .iter()
            .map(|&(ox, oy)| (ix + ox, iy + oy))
            .filter(|&(x, y)| 0 <= x && x < self.nx && 0 <= y && y < self.ny)
            .map(|(x, y)| y * self.nx + x + base)
            .collect()
    }

    /// In-bounds planar offsets of `node` on its layer, in `offsets` order.
    pub fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (layer, rest) = node.div_mod_floor(&self.plane);
        let (iy, ix) = rest.div_mod_floor(&self.nx);
        self.offset_ids(ix, iy, layer * self.plane, offsets)
    }

    pub fn offset_columns(&self, column: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (iy, ix) = column.div_mod_floor(&self.nx);
        self.offset_ids(ix, iy, 0, offsets)
    }

    pub fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64> {
        let (ix, iy) = self.rounded(point);
        self.offset_ids(ix, iy, 0, offsets)
    }

    pub fn full_box(&self) -> IndexBox {
        (0, 0, self.nx - 1, self.ny - 1)
    }

    /// Bounding index box of nodes (any layer); `None` when empty.
    pub fn box_of(&self, nodes: impl IntoIterator<Item = i64>) -> Option<IndexBox> {
        let mut found: Option<IndexBox> = None;
        for node in nodes {
            let (y, x) = node.mod_floor(&self.plane).div_mod_floor(&self.nx);
            found = Some(match found {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        found
    }

    pub fn grow_box(&self, (x0, y0, x1, y1): IndexBox, margin: i64) -> IndexBox {
        (0.max(x0 - margin), 0.max(y0 - margin), (self.nx - 1).min(x1 + margin), (self.ny - 1).min(y1 + margin))
    }
}

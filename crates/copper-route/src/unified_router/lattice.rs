//! The router's lattice: the uniform lattice, or the nested one of a
//! fine-region plan (`self.lattice` is either in `_Router`). The shared
//! queries dispatch to the concrete lattice; `nested()` gives the nested
//! lattice where the router branches on `self.nested`.

use copper_physical::{CopperLayer, Point};

use crate::routing_lattice::{IndexBox, Lattice, LatticeBlock, NestedLattice, UniformLattice};
use copper_pcb::Result;

#[derive(Clone, Debug, PartialEq)]
pub enum LatticeKind {
    Uniform(Box<UniformLattice>),
    Nested(Box<NestedLattice>),
}

/// `signature()` of either lattice (compared by `history_seed`).
#[derive(Clone, Debug, PartialEq)]
pub enum Signature {
    Uniform((i64, i64, i64, i64, i64, i64)),
    Nested(((i64, i64, i64, i64, i64, i64), Vec<(i64, i64, i64, i64, i64)>)),
}

/// Either lattice, with the shape fields both share (`nx`, `ny`, `x0`, `y0`
/// are the base block's on a nested lattice).
#[derive(Clone, Debug, PartialEq)]
pub struct RouterLattice {
    pub kind: LatticeKind,
    pub layers: Vec<CopperLayer>,
    pub nl: i64,
    pub plane: i64,
    pub size: i64,
    /// The base pitch.
    pub pitch: i64,
    pub pitch_nm: i64,
    pub nx: i64,
    pub ny: i64,
    pub x0: i64,
    pub y0: i64,
}

macro_rules! dispatch {
    ($self:ident, $g:ident => $body:expr) => {
        match &$self.kind {
            LatticeKind::Uniform($g) => $body,
            LatticeKind::Nested($g) => $body,
        }
    };
}

impl RouterLattice {
    pub fn uniform(lattice: UniformLattice) -> RouterLattice {
        RouterLattice {
            layers: lattice.layers.clone(),
            nl: lattice.nl,
            plane: lattice.plane,
            size: lattice.size,
            pitch: lattice.pitch,
            pitch_nm: lattice.pitch_nm,
            nx: lattice.nx,
            ny: lattice.ny,
            x0: lattice.x0,
            y0: lattice.y0,
            kind: LatticeKind::Uniform(Box::new(lattice)),
        }
    }

    pub fn nested(lattice: NestedLattice) -> RouterLattice {
        RouterLattice {
            layers: lattice.layers.clone(),
            nl: lattice.nl,
            plane: lattice.plane,
            size: lattice.size,
            pitch: lattice.pitch,
            pitch_nm: lattice.pitch_nm,
            nx: lattice.nx,
            ny: lattice.ny,
            x0: lattice.x0,
            y0: lattice.y0,
            kind: LatticeKind::Nested(Box::new(lattice)),
        }
    }

    /// The nested lattice (`self.nested`), or `None` on a uniform one.
    pub fn as_nested(&self) -> Option<&NestedLattice> {
        match &self.kind {
            LatticeKind::Nested(g) => Some(g),
            LatticeKind::Uniform(_) => None,
        }
    }

    pub fn is_nested(&self) -> bool {
        matches!(self.kind, LatticeKind::Nested(_))
    }

    pub fn blocks(&self) -> &[LatticeBlock] {
        dispatch!(self, g => &g.blocks)
    }

    pub fn signature(&self) -> Signature {
        match &self.kind {
            LatticeKind::Uniform(g) => Signature::Uniform(g.signature()),
            LatticeKind::Nested(g) => Signature::Nested(g.signature()),
        }
    }

    pub fn column_point(&self, column: i64) -> Point {
        dispatch!(self, g => g.column_point(column))
    }

    pub fn snap(&self, layer: i64, point: Point) -> Option<i64> {
        dispatch!(self, g => g.snap(layer, point))
    }

    pub fn cover(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(i64, i64, i64)> {
        dispatch!(self, g => g.cover(min_x, min_y, max_x, max_y))
    }

    pub fn segment_nodes(&self, layer: i64, start: Point, end: Point, radius_nm: f64) -> Vec<(i64, f64)> {
        dispatch!(self, g => g.segment_nodes(layer, start, end, radius_nm))
    }

    pub fn delta(&self, a: i64, b: i64) -> (i64, i64) {
        dispatch!(self, g => g.delta(a, b))
    }

    pub fn diagonal_cell(&self, a: i64, b: i64) -> i64 {
        dispatch!(self, g => g.diagonal_cell(a, b))
    }

    pub fn offset_nodes(&self, node: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        dispatch!(self, g => g.offset_nodes(node, offsets))
    }

    pub fn offset_columns(&self, column: i64, offsets: &[(i64, i64)]) -> Vec<i64> {
        dispatch!(self, g => g.offset_columns(column, offsets))
    }

    pub fn columns_around(&self, point: Point, offsets: &[(i64, i64)]) -> Vec<i64> {
        dispatch!(self, g => g.columns_around(point, offsets))
    }

    pub fn full_box(&self) -> IndexBox {
        dispatch!(self, g => g.full_box())
    }

    pub fn box_of(&self, nodes: &[i64]) -> Result<IndexBox> {
        dispatch!(self, g => g.box_of(nodes))
    }

    pub fn grow_box(&self, index_box: IndexBox, margin: i64) -> IndexBox {
        dispatch!(self, g => g.grow_box(index_box, margin))
    }
}

impl Lattice for RouterLattice {
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
        dispatch!(self, g => g.point(node))
    }
    fn node_at(&self, layer: i64, point: Point) -> Option<i64> {
        dispatch!(self, g => g.node_at(layer, point))
    }
    fn nearest(&self, layer: i64, point: Point) -> i64 {
        dispatch!(self, g => g.nearest(layer, point))
    }
    fn nodes_in(&self, layer: i64, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<i64> {
        dispatch!(self, g => g.nodes_in(layer, x0, y0, x1, y1))
    }
    fn neighbours(&self, node: i64, diagonals: bool) -> Vec<(i64, f64)> {
        dispatch!(self, g => g.neighbours(node, diagonals))
    }
    fn step_nm(&self, node: i64) -> i64 {
        dispatch!(self, g => g.step_nm(node))
    }
}

//! The geometry an extraction takes in: copper per layer and net as integer-nanometre polygon sets, vias, and the
//! terminals, shorts and ports that define what is being extracted.
//!
//! Coordinates are board nanometres (x right, y as the board file has it); heights are metres from the first
//! copper layer's centre downwards (`z <= 0`), the convention of `copper_fullwave::stack`.

pub use copper_cam::poly::Shapes;

/// Resistivity of annealed copper at 20 C, ohm m (the value `copper_emi::inductance` uses).
pub const COPPER_RESISTIVITY: f64 = copper_emi::inductance::COPPER_RESISTIVITY;

/// Copper of one net on one layer. Several conductors of a net may overlap; the mesher takes their union.
#[derive(Clone, Debug)]
pub struct Conductor {
    pub net: String,
    pub shapes: Shapes,
    /// Copper of the loop being extracted (a loop track, a loop pad, a bar that stands for a component): a narrow
    /// shape of it is cut into at least `MeshSpec::across` cells across its width whatever the pitch. Other narrow
    /// copper is cut at the pitch.
    pub on_loop: bool,
}

/// A request for a finer mesh in a rectangle `[x0, y0, x1, y1]` (nm) of one layer: lines at `cell_x_nm` pitch over
/// the rectangle's x range and at `cell_y_nm` over its y range (0 asks for nothing on that axis, so a strip along a
/// trace asks for fine lines across the trace only).
#[derive(Clone, Copy, Debug)]
pub struct Hint {
    pub rect: [i64; 4],
    pub cell_x_nm: i64,
    pub cell_y_nm: i64,
}

impl Hint {
    /// Fine in both axes.
    pub fn square(rect: [i64; 4], cell_nm: i64) -> Hint {
        Hint { rect, cell_x_nm: cell_nm, cell_y_nm: cell_nm }
    }
}

/// One copper layer.
#[derive(Clone, Debug)]
pub struct LayerGeom {
    pub name: String,
    /// Height of the layer's centre, metres.
    pub z_m: f64,
    pub thickness_m: f64,
    pub conductors: Vec<Conductor>,
    pub hints: Vec<Hint>,
}

/// A vertical conductor: a via, or the barrel of a through-hole pad, between layers `first_layer ..= last_layer`
/// (indices into [`Layout::layers`], in stack order). It joins the copper of its net on every layer it passes
/// that has some at its position.
#[derive(Clone, Debug)]
pub struct ViaGeom {
    pub name: String,
    pub x: i64,
    pub y: i64,
    pub net: String,
    pub first_layer: usize,
    pub last_layer: usize,
    pub drill_nm: i64,
    pub plating_nm: i64,
    /// Layers on which the via joins the copper of another net than `net` (a via that deliberately joins two nets,
    /// as the shorting wall of a test line does); empty for an ordinary via.
    pub layer_nets: Vec<(usize, String)>,
}

/// A terminal: the copper of `net` on `layer` inside `region` (a pad outline). All its nodes are one equipotential
/// terminal.
#[derive(Clone, Debug)]
pub struct Terminal {
    pub name: String,
    pub layer: usize,
    pub net: String,
    pub region: Shapes,
}

/// A port between two terminals (indices into [`Layout::terminals`]).
#[derive(Clone, Debug)]
pub struct PortDef {
    pub name: String,
    pub plus: usize,
    pub minus: usize,
}

/// Everything the mesher needs.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The window `[x0, y0, x1, y1]` (nm); copper outside it is cut off.
    pub window: [i64; 4],
    pub layers: Vec<LayerGeom>,
    pub vias: Vec<ViaGeom>,
    pub terminals: Vec<Terminal>,
    /// Sets of terminals joined by a perfect conductor (a component on the loop path closed by a short).
    pub shorts: Vec<Vec<usize>>,
    pub ports: Vec<PortDef>,
    pub resistivity: f64,
}

impl Layout {
    pub fn new(window: [i64; 4]) -> Layout {
        Layout {
            window,
            layers: Vec::new(),
            vias: Vec::new(),
            terminals: Vec::new(),
            shorts: Vec::new(),
            ports: Vec::new(),
            resistivity: COPPER_RESISTIVITY,
        }
    }

    /// Add a layer and return its index.
    pub fn add_layer(&mut self, name: &str, z_m: f64, thickness_m: f64) -> usize {
        self.layers.push(LayerGeom {
            name: name.to_owned(),
            z_m,
            thickness_m,
            conductors: Vec::new(),
            hints: Vec::new(),
        });
        self.layers.len() - 1
    }

    /// Add a polygon (nm, counter-clockwise or not) of `net` on `layer`.
    pub fn add_polygon(&mut self, layer: usize, net: &str, points: &[(f64, f64)]) {
        self.layers[layer].conductors.push(Conductor {
            net: net.to_owned(),
            shapes: copper_cam::poly::polygon(points),
            on_loop: true,
        });
    }

    /// Add an axis-aligned rectangle `[x0, y0, x1, y1]` (nm).
    pub fn add_rect(&mut self, layer: usize, net: &str, rect: [i64; 4]) {
        self.add_polygon(
            layer,
            net,
            &[
                (rect[0] as f64, rect[1] as f64),
                (rect[2] as f64, rect[1] as f64),
                (rect[2] as f64, rect[3] as f64),
                (rect[0] as f64, rect[3] as f64),
            ],
        );
    }

    /// Add a terminal over a rectangle and return its index.
    pub fn add_terminal_rect(&mut self, name: &str, layer: usize, net: &str, rect: [i64; 4]) -> usize {
        let region = copper_cam::poly::polygon(&[
            (rect[0] as f64, rect[1] as f64),
            (rect[2] as f64, rect[1] as f64),
            (rect[2] as f64, rect[3] as f64),
            (rect[0] as f64, rect[3] as f64),
        ]);
        self.terminals.push(Terminal { name: name.to_owned(), layer, net: net.to_owned(), region });
        self.terminals.len() - 1
    }

    /// Add a port between two terminals.
    pub fn add_port(&mut self, name: &str, plus: usize, minus: usize) {
        self.ports.push(PortDef { name: name.to_owned(), plus, minus });
    }
}

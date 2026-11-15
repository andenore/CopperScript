//! An independent reference solver for the DC limit of a [`Layout`], from the same physical geometry as the mesher
//! and sharing none of its code (nothing of `mesh.rs`, `network.rs`, `partial.rs`): a uniform raster of every layer's
//! copper, flood-fill connectivity, finite-difference sheet conduction and the DC inductance from the energy of the
//! resulting current distribution, evaluated with FFT convolutions. Only the polygon library, the radix-2 FFT of
//! `copper-emi` and the [`Layout`] type are shared.
//!
//! **What it computes.** For every port (1 A into the plus terminal, out of the minus terminal, every terminal an
//! equipotential, shorts joining terminals):
//! * the topology: copper pixels connected by 4-neighbours, vias (the pixel under the via on each layer it joins),
//!   terminals and shorts; the number of connected components and whether each port is open;
//! * `R_ij`, the potential difference at port `i` per ampere into port `j` (DC);
//! * `L_ij = (mu0 / 4 pi) sum I_a I_b / (I_i I_j) * (1 / h^2) * double integral of 1 / |r - r'|` over the current
//!   cells of the DC current distribution (current on the edges between neighbouring pixels, each edge a cell of
//!   `h x h` carrying a uniform sheet current `I / h`), the vertical current of the via barrels as filaments.
//!
//! **Modelling choices, stated.** Each layer is a sheet at the layer's height: the thickness enters the conductance
//! (`sigma t`) and nothing else (a sheet of width `w` has about `ln((w + t) / w)` less inductance per length than the
//! bar of thickness `t`: 1 % to 3 % for `t / w` of 0.03 to 0.1). The same-layer self term of a cell is the exact
//! average of `1 / r` over a square pair (`2.9732 h`); neighbouring and next cells use a Gauss-Legendre quadrature of
//! the double integral; farther cells the point formula. Via barrels are tubes of the drill radius plus plating, their
//! inductance the exact filament-pair integral with the tube radius as the distance for the self term. This is the DC
//! limit (current distributed by resistance): it does not know skin effect, proximity effect or the inductive
//! redistribution of current at frequency.

// Index loops over the raster and the pixel grid read better than iterator chains here.
#![allow(clippy::needless_range_loop, clippy::type_complexity)]

use std::time::Instant;

use copper_cam::poly::{self, Contour, Shapes};
use copper_emi::spectrum::{Complex, fft, ifft};

use crate::layout::{COPPER_RESISTIVITY, Layout};

const MU0_OVER_4PI: f64 = 1e-7;
const NM: f64 = 1e-9;

/// A connected component of this many pixels or fewer is not a piece of copper a raster resolves.
pub const FRAGMENT_PIXELS: usize = 4;

/// What the caller chooses.
#[derive(Clone, Debug)]
pub struct ReferenceOptions {
    /// The pixel pitch, nm; `None` chooses it from `max_cells`.
    pub pitch_nm: Option<i64>,
    /// The most pixels over all layers when the pitch is chosen (the cost is a few FFTs of the grid and one CG solve
    /// per port).
    pub max_cells: usize,
}

impl Default for ReferenceOptions {
    fn default() -> Self {
        ReferenceOptions { pitch_nm: None, max_cells: 1_600_000 }
    }
}

/// The result.
#[derive(Clone, Debug)]
pub struct ReferenceResult {
    pub pitch_nm: i64,
    /// The grid: pixels in x and y and layers.
    pub grid: (usize, usize, usize),
    /// Copper pixels over all layers.
    pub cells: usize,
    /// Connected components of the copper after vias, terminals and shorts, counted over the pixels.
    pub components: usize,
    /// Components a raster cannot resolve and that are not counted as pieces of copper in the comparison with the
    /// mesher: those of `FRAGMENT_PIXELS` pixels or fewer (a corner touch between two polygons, a vertex of a
    /// shape) and those all of whose pixels are in the outermost row or column of the window (slivers at the window's
    /// edge, which the mesher clips away).
    pub edge_slivers: usize,
    /// Whether each port's two terminals are in different components (no current can flow: R infinite).
    pub port_open: Vec<bool>,
    /// `R_ij`, ohm; NaN in a row or column of an open port.
    pub resistance_ohm: Vec<Vec<f64>>,
    /// `L_ij`, nH; NaN in a row or column of an open port.
    pub inductance_nh: Vec<Vec<f64>>,
    /// Terminals with no pixel centre inside their region, taken at the nearest copper pixel of their net.
    pub terminals_snapped: Vec<String>,
    pub notes: Vec<String>,
    pub seconds: f64,
}

/// The pixel grid of a layout.
struct Grid {
    nx: usize,
    ny: usize,
    h: f64,  // metres
    x0: f64, // nm
    y0: f64,
    h_nm: f64,
}

/// Union-find with path halving.
struct Dsu(Vec<usize>);

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Fill the pixels (even-odd, pixel centres) of `shapes` with `value` into `layer` (`nx * ny`); returns pixels set.
fn fill(layer: &mut [u16], grid: &Grid, shapes: &Shapes, value: u16, overlaps: &mut usize) -> usize {
    let mut set = 0;
    let mut xs: Vec<f64> = Vec::new();
    for shape in shapes {
        let rings: Vec<&Contour> = shape.iter().collect();
        let (mut ymin, mut ymax) = (i64::MAX, i64::MIN);
        for ring in &rings {
            for p in ring.iter() {
                ymin = ymin.min(p.y);
                ymax = ymax.max(p.y);
            }
        }
        if ymin > ymax {
            continue;
        }
        let j0 = (((ymin as f64 - grid.y0) / grid.h_nm - 0.5).floor().max(0.0)) as usize;
        let j1 = ((((ymax as f64 - grid.y0) / grid.h_nm + 0.5).ceil()).max(0.0) as usize).min(grid.ny);
        for j in j0..j1 {
            let yc = grid.y0 + (j as f64 + 0.5) * grid.h_nm;
            xs.clear();
            for ring in &rings {
                let n = ring.len();
                for k in 0..n {
                    let (p, q) = (&ring[k], &ring[(k + 1) % n]);
                    let (py, qy) = (p.y as f64, q.y as f64);
                    if (py <= yc && qy > yc) || (qy <= yc && py > yc) {
                        let t = (yc - py) / (qy - py);
                        xs.push(p.x as f64 + t * (q.x - p.x) as f64);
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for pair in xs.chunks(2) {
                if pair.len() < 2 {
                    continue;
                }
                let i0 = (((pair[0] - grid.x0) / grid.h_nm - 0.5).ceil().max(0.0)) as usize;
                let i1 = (((pair[1] - grid.x0) / grid.h_nm - 0.5).ceil().max(0.0) as usize).min(grid.nx);
                for i in i0..i1 {
                    let cell = &mut layer[j * grid.nx + i];
                    if *cell == 0 {
                        *cell = value;
                        set += 1;
                    } else if *cell != value {
                        *overlaps += 1;
                    }
                }
            }
        }
    }
    set
}

/// The mean of `1 / r` over a pair of unit squares (`2.9732`), by the exact polar integral.
fn unit_square_mean_inverse_distance() -> f64 {
    8.0 * (0.5 * (1.0 + 2f64.sqrt()).ln() - (2f64.sqrt() - 1.0) / 6.0)
}

/// Gauss-Legendre nodes and weights on [-1, 1].
fn gauss(n: usize) -> (&'static [f64], &'static [f64]) {
    match n {
        4 => (
            &[-0.861_136_311_594_052_6, -0.339_981_043_584_856_3, 0.339_981_043_584_856_3, 0.861_136_311_594_052_6],
            &[0.347_854_845_137_453_9, 0.652_145_154_862_546_1, 0.652_145_154_862_546_1, 0.347_854_845_137_453_9],
        ),
        _ => (
            &[
                -0.932_469_514_203_152,
                -0.661_209_386_466_264_5,
                -0.238_619_186_083_196_9,
                0.238_619_186_083_196_9,
                0.661_209_386_466_264_5,
                0.932_469_514_203_152,
            ],
            &[
                0.171_324_492_379_170_3,
                0.360_761_573_048_138_6,
                0.467_913_934_572_691,
                0.467_913_934_572_691,
                0.360_761_573_048_138_6,
                0.171_324_492_379_170_3,
            ],
        ),
    }
}

/// The double integral of `1 / r` over two squares of side `h` whose centres are `(m h, n h)` apart in the plane and
/// `dz` apart in height, m^3.
fn pair_integral(m: i64, n: i64, dz: f64, h: f64, c0: f64) -> f64 {
    if m == 0 && n == 0 && dz == 0.0 {
        return c0 * h * h * h;
    }
    let near = m.abs() <= 1 && n.abs() <= 1;
    if m.abs() > 3 || n.abs() > 3 {
        let d = ((m as f64 * h).powi(2) + (n as f64 * h).powi(2) + dz * dz).sqrt();
        return h.powi(4) / d;
    }
    let (x, w) = gauss(if near { 6 } else { 4 });
    let mut sum = 0.0;
    for (ia, &xa) in x.iter().enumerate() {
        for (ja, &ya) in x.iter().enumerate() {
            let wa = w[ia] * w[ja];
            for (ib, &xb) in x.iter().enumerate() {
                for (jb, &yb) in x.iter().enumerate() {
                    let dx = m as f64 * h + (xb - xa) * h / 2.0;
                    let dy = n as f64 * h + (yb - ya) * h / 2.0;
                    sum += wa * w[ib] * w[jb] / (dx * dx + dy * dy + dz * dz).sqrt();
                }
            }
        }
    }
    sum * (h / 2.0).powi(4)
}

/// The integral of `1 / sqrt(d^2 + (z - z')^2)` over two collinear ranges `[a1, a2]`, `[b1, b2]` of parallel filaments
/// a distance `d` apart.
fn parallel_filaments(a1: f64, a2: f64, b1: f64, b2: f64, d: f64) -> f64 {
    let phi = |u: f64| u * (u / d).asinh() - (u * u + d * d).sqrt();
    phi(a2 - b1) - phi(a2 - b2) - phi(a1 - b1) + phi(a1 - b2)
}

/// A via barrel between two attached pixels.
struct Barrel {
    x: f64,
    y: f64,
    z_a: f64,
    z_b: f64,
    conductance: f64,
    pixel_a: usize,
    pixel_b: usize,
    radius: f64,
}

fn next_pow2(n: usize) -> usize {
    n.next_power_of_two().max(2)
}

/// 2-D transform of a row-major `nx x ny` array (in place), forward or inverse.
fn fft2(data: &mut [Complex], nx: usize, ny: usize, inverse: bool) {
    let run = |d: &mut [Complex]| {
        if inverse {
            ifft(d).expect("power of two");
        } else {
            fft(d).expect("power of two");
        }
    };
    for row in data.chunks_mut(nx) {
        run(row);
    }
    let mut column = vec![Complex::ZERO; ny];
    for i in 0..nx {
        for j in 0..ny {
            column[j] = data[j * nx + i];
        }
        run(&mut column);
        for j in 0..ny {
            data[j * nx + i] = column[j];
        }
    }
}

/// Solve a layout's DC limit. See the module documentation.
pub fn solve(layout: &Layout, options: &ReferenceOptions) -> Result<ReferenceResult, String> {
    let started = Instant::now();
    let window = layout.window;
    let (wx, wy) = ((window[2] - window[0]) as f64, (window[3] - window[1]) as f64);
    if wx <= 0.0 || wy <= 0.0 || layout.layers.is_empty() {
        return Err("the layout has no window or no layers".to_owned());
    }
    let layers = layout.layers.len();
    let h_nm = match options.pitch_nm {
        Some(p) if p > 0 => p as f64,
        _ => (wx * wy * layers as f64 / options.max_cells.max(1) as f64).sqrt().max(1000.0),
    };
    let (nx, ny) = ((wx / h_nm).ceil() as usize, (wy / h_nm).ceil() as usize);
    let grid = Grid { nx, ny, h: h_nm * NM, x0: window[0] as f64, y0: window[1] as f64, h_nm };
    let per_layer = nx * ny;
    let mut notes: Vec<String> = Vec::new();

    // Raster: net ids per pixel (0 = no copper).
    let mut nets: Vec<String> = Vec::new();
    let net_id = |nets: &mut Vec<String>, name: &str| -> u16 {
        match nets.iter().position(|n| n == name) {
            Some(i) => (i + 1) as u16,
            None => {
                nets.push(name.to_owned());
                nets.len() as u16
            }
        }
    };
    // Rasters, one per *slot*: a layer's copper of one net or of several nets that do not overlap. Copper of two nets
    // that overlaps in a layer of the layout (a bar over a pour) needs a second slot at the same height.
    let mut raster: Vec<Vec<u16>> = Vec::new();
    let mut slot_z: Vec<f64> = Vec::new();
    let mut slot_t: Vec<f64> = Vec::new();
    let mut slots_of_layer: Vec<Vec<usize>> = vec![Vec::new(); layers];
    let mut net_slot: std::collections::BTreeMap<(usize, u16), usize> = std::collections::BTreeMap::new();
    let mut overlays = 0usize;
    for (li, layer) in layout.layers.iter().enumerate() {
        let mut by_net: Vec<(String, Shapes)> = Vec::new();
        for c in &layer.conductors {
            match by_net.iter_mut().find(|(n, _)| *n == c.net) {
                Some((_, s)) => s.extend(c.shapes.iter().cloned()),
                None => by_net.push((c.net.clone(), c.shapes.clone())),
            }
        }
        for (net, shapes) in by_net {
            let id = net_id(&mut nets, &net);
            let mut scratch = vec![0u16; per_layer];
            let mut ignored = 0usize;
            fill(&mut scratch, &grid, &poly::union(&shapes), id, &mut ignored);
            let free = |slot: &Vec<u16>| scratch.iter().zip(slot).all(|(a, b)| *a == 0 || *b == 0);
            let chosen = slots_of_layer[li].iter().copied().find(|&sl| free(&raster[sl]));
            let slot = match chosen {
                Some(sl) => sl,
                None => {
                    if !slots_of_layer[li].is_empty() {
                        overlays += 1;
                    }
                    raster.push(vec![0u16; per_layer]);
                    slot_z.push(layer.z_m);
                    slot_t.push(layer.thickness_m);
                    slots_of_layer[li].push(raster.len() - 1);
                    raster.len() - 1
                }
            };
            for (k, &v) in scratch.iter().enumerate() {
                if v != 0 {
                    raster[slot][k] = v;
                }
            }
            net_slot.insert((li, id), slot);
        }
    }
    if overlays > 0 {
        notes.push(format!(
            "{overlays} net(s) overlap copper of another net in their layer (a bar over a pour): each is rastered in a slot of its own at the same height"
        ));
    }
    let slots = raster.len();
    let pixel = |l: usize, i: usize, j: usize| l * per_layer + j * nx + i;
    let total = slots * per_layer;
    let cells: usize = raster.iter().map(|r| r.iter().filter(|&&v| v != 0).count()).sum();

    // Connectivity inside each slot: 4-neighbours of one net.
    let mut connectivity = Dsu::new(total);
    for l in 0..slots {
        for j in 0..ny {
            for i in 0..nx {
                let v = raster[l][j * nx + i];
                if v == 0 {
                    continue;
                }
                if i + 1 < nx && raster[l][j * nx + i + 1] == v {
                    connectivity.union(pixel(l, i, j), pixel(l, i + 1, j));
                }
                if j + 1 < ny && raster[l][(j + 1) * nx + i] == v {
                    connectivity.union(pixel(l, i, j), pixel(l, i, j + 1));
                }
            }
        }
    }
    let mut equi = Dsu::new(total);

    // Terminals: the pixels of the terminal's net under its region.
    let mut terminal_pixels: Vec<Vec<usize>> = Vec::new();
    let mut snapped: Vec<String> = Vec::new();
    for t in &layout.terminals {
        let id = nets.iter().position(|n| *n == t.net).map(|i| (i + 1) as u16);
        let mut region = vec![0u16; per_layer];
        let mut ov = 0;
        fill(&mut region, &grid, &t.region, 1, &mut ov);
        let mut pixels: Vec<usize> = Vec::new();
        let slot = id.and_then(|id| net_slot.get(&(t.layer, id)).copied());
        if let (Some(id), Some(slot)) = (id, slot) {
            let layer = &raster[slot];
            for (k, &r) in region.iter().enumerate() {
                if r == 1 && layer[k] == id {
                    pixels.push(slot * per_layer + k);
                }
            }
            if pixels.is_empty() {
                // The region is narrower than a pixel (a sliver at the end of a trace): the pixels of the net whose
                // centres are within one pixel of the region's bounding box.
                let bounds = t
                    .region
                    .iter()
                    .map(|s| poly::bounds(s))
                    .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
                if let Some(bb) = bounds {
                    // The pixels of the net whose centres are within one pixel of the region's bounding box; two or
                    // three pixels when none are (a sliver of copper thinner than a pixel).
                    for reach in [1.0, 2.0, 3.0] {
                        for (k, &v) in layer.iter().enumerate() {
                            if v != id {
                                continue;
                            }
                            let (i, j) = (k % nx, k / nx);
                            let (cx, cy) = (grid.x0 + (i as f64 + 0.5) * h_nm, grid.y0 + (j as f64 + 0.5) * h_nm);
                            if cx > bb.0 as f64 - reach * h_nm
                                && cx < bb.2 as f64 + reach * h_nm
                                && cy > bb.1 as f64 - reach * h_nm
                                && cy < bb.3 as f64 + reach * h_nm
                            {
                                pixels.push(slot * per_layer + k);
                            }
                        }
                        if !pixels.is_empty() {
                            break;
                        }
                    }
                    if !pixels.is_empty() {
                        // A snapped terminal touches one piece of copper only: the piece of the pixel nearest the
                        // region's centre (pixels of a neighbouring island within reach are not contacted).
                        let (mx, my) = ((bb.0 + bb.2) as f64 / 2.0, (bb.1 + bb.3) as f64 / 2.0);
                        let distance = |p: usize| {
                            let k = p - slot * per_layer;
                            (grid.x0 + ((k % nx) as f64 + 0.5) * h_nm - mx)
                                .hypot(grid.y0 + ((k / nx) as f64 + 0.5) * h_nm - my)
                        };
                        let mut nearest = pixels[0];
                        for &p in &pixels {
                            if distance(p) < distance(nearest) {
                                nearest = p;
                            }
                        }
                        let piece = connectivity.find(nearest);
                        pixels.retain(|&p| connectivity.find(p) == piece);
                        snapped.push(t.name.clone());
                    }
                }
            }
        }
        if pixels.is_empty() {
            let count = id.zip(slot).map(|(id, sl)| raster[sl].iter().filter(|&&v| v == id).count()).unwrap_or(0);
            let bounds = t
                .region
                .iter()
                .map(|s| poly::bounds(s))
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
            return Err(format!(
                "terminal {}: no copper of net {} under its region on layer {} ({count} pixel(s) of the net there; region {bounds:?} nm; window {window:?} nm; pitch {h_nm} nm)",
                t.name, t.net, t.layer
            ));
        }
        terminal_pixels.push(pixels);
    }

    // Vias: the pixel under the via (or the nearest of its net within the contact circle or one pixel) on each layer.
    let mut barrels: Vec<Barrel> = Vec::new();
    for via in &layout.vias {
        let mut attached: Vec<(usize, usize)> = Vec::new(); // (layer, pixel)
        let r_out_contact = via.drill_nm as f64 / 2.0 + via.plating_nm as f64;
        let r_contact = r_out_contact.max(0.75 * h_nm);
        for lay in via.first_layer..=via.last_layer.min(layers - 1) {
            let net = via.layer_nets.iter().find(|(k, _)| *k == lay).map_or(&via.net, |(_, n)| n);
            let Some(id) = nets.iter().position(|n| n == net).map(|i| (i + 1) as u16) else { continue };
            let Some(&l) = net_slot.get(&(lay, id)) else { continue };
            let (ci, cj) = (
                ((via.x as f64 - grid.x0) / h_nm - 0.5).round() as i64,
                ((via.y as f64 - grid.y0) / h_nm - 0.5).round() as i64,
            );
            // Every pixel of the net whose centre is inside the contact circle is on the barrel (an equipotential: a
            // single pixel would give a spreading resistance that grows with the logarithm of the pitch); the nearest
            // one within three quarters of a pixel when the circle holds no centre.
            let reach = (r_out_contact / h_nm).ceil() as i64 + 2;
            let mut inside: Vec<(f64, usize)> = Vec::new();
            let mut best: Option<(f64, usize)> = None;
            for dj in -reach..=reach {
                for di in -reach..=reach {
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i >= nx as i64 || j >= ny as i64 {
                        continue;
                    }
                    let k = j as usize * nx + i as usize;
                    if raster[l][k] != id {
                        continue;
                    }
                    let d = (grid.x0 + (i as f64 + 0.5) * h_nm - via.x as f64)
                        .hypot(grid.y0 + (j as f64 + 0.5) * h_nm - via.y as f64);
                    if d <= r_out_contact {
                        inside.push((d, l * per_layer + k));
                    }
                    if d <= r_contact && best.is_none_or(|b| d < b.0) {
                        best = Some((d, l * per_layer + k));
                    }
                }
            }
            if let Some((_, k)) = best {
                for &(_, other) in &inside {
                    equi.union(k, other);
                    connectivity.union(k, other);
                }
                attached.push((l, k));
            }
        }
        if attached.len() < 2 {
            notes.push(format!("via {} joins fewer than two layers and is left out", via.name));
            continue;
        }
        let r_out = via.drill_nm as f64 / 2.0 + via.plating_nm as f64;
        let r_in = via.drill_nm as f64 / 2.0;
        let area = std::f64::consts::PI * (r_out * r_out - r_in * r_in) * NM * NM;
        for w in attached.windows(2) {
            let ((la, pa), (lb, pb)) = (w[0], w[1]);
            let (za, zb) = (slot_z[la], slot_z[lb]);
            let length = (za - zb).abs();
            if length <= 0.0 {
                continue;
            }
            connectivity.union(pa, pb);
            barrels.push(Barrel {
                x: via.x as f64 * NM,
                y: via.y as f64 * NM,
                z_a: za,
                z_b: zb,
                conductance: area / (layout.resistivity * length),
                pixel_a: pa,
                pixel_b: pb,
                radius: r_out * NM,
            });
        }
    }

    // Equipotentials (terminals and shorts) and connectivity through them.
    for pixels in &terminal_pixels {
        for w in pixels.windows(2) {
            equi.union(w[0], w[1]);
            connectivity.union(w[0], w[1]);
        }
    }
    for set in &layout.shorts {
        let mut all: Vec<usize> = Vec::new();
        for &t in set {
            if let Some(p) = terminal_pixels.get(t) {
                all.extend(p.iter().copied());
            }
        }
        for w in all.windows(2) {
            equi.union(w[0], w[1]);
            connectivity.union(w[0], w[1]);
        }
    }
    let mut roots: Vec<usize> = Vec::new();
    for l in 0..slots {
        for k in 0..per_layer {
            if raster[l][k] != 0 {
                roots.push(connectivity.find(l * per_layer + k));
            }
        }
    }
    // The components, described (pixels, layers, nets, bounding box in mm), the smallest first, for the notes; those
    // whose pixels all lie in the outermost row or column of the window are slivers of copper at its edge.
    let edge_slivers = {
        let mut by_root: Vec<(usize, usize, Vec<usize>, Vec<u16>, [f64; 4], bool)> = Vec::new();
        for l in 0..slots {
            for k in 0..per_layer {
                let v = raster[l][k];
                if v == 0 {
                    continue;
                }
                let r = connectivity.find(l * per_layer + k);
                let (i, j) = (k % nx, k / nx);
                let on_edge = i == 0 || j == 0 || i + 1 == nx || j + 1 == ny;
                let (x, y) = ((grid.x0 + (i as f64 + 0.5) * h_nm) * 1e-6, (grid.y0 + (j as f64 + 0.5) * h_nm) * 1e-6);
                match by_root.iter_mut().find(|c| c.0 == r) {
                    Some(c) => {
                        c.1 += 1;
                        if !c.2.contains(&l) {
                            c.2.push(l);
                        }
                        if !c.3.contains(&v) {
                            c.3.push(v);
                        }
                        c.4 = [c.4[0].min(x), c.4[1].min(y), c.4[2].max(x), c.4[3].max(y)];
                        c.5 &= on_edge;
                    }
                    None => by_root.push((r, 1, vec![l], vec![v], [x, y, x, y], on_edge)),
                }
            }
        }
        by_root.sort_by_key(|c| c.1);
        let slivers = by_root.iter().filter(|c| c.5 || c.1 <= FRAGMENT_PIXELS).count();
        for c in by_root.iter().take(8) {
            notes.push(format!(
                "component of {} pixel(s), layers {:?}, nets {:?}, from ({:.2}, {:.2}) to ({:.2}, {:.2}) mm",
                c.1,
                c.2,
                c.3.iter().map(|&v| nets[v as usize - 1].clone()).collect::<Vec<_>>(),
                c.4[0],
                c.4[1],
                c.4[2],
                c.4[3]
            ));
        }
        slivers
    };
    roots.sort_unstable();
    roots.dedup();
    let components = roots.len();
    let port_open: Vec<bool> = layout
        .ports
        .iter()
        .map(|p| connectivity.find(terminal_pixels[p.plus][0]) != connectivity.find(terminal_pixels[p.minus][0]))
        .collect();

    // Unknowns: the equipotential classes of copper pixels.
    let mut class_of = vec![usize::MAX; total];
    let mut classes = 0usize;
    let mut class_root: Vec<usize> = Vec::new();
    for l in 0..slots {
        for k in 0..per_layer {
            if raster[l][k] != 0 {
                let p = l * per_layer + k;
                let r = equi.find(p);
                if class_of[r] == usize::MAX {
                    class_of[r] = classes;
                    class_root.push(r);
                    classes += 1;
                }
                class_of[p] = class_of[r];
            }
        }
    }
    // Conductances between classes: neighbouring pixels of one net on one layer, and via barrels.
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    let mut diagonal = vec![0.0f64; classes];
    let mut add = |a: usize, b: usize, g: f64, edges: &mut Vec<(usize, usize, f64)>| {
        if a != b {
            edges.push((a, b, g));
            diagonal[a] += g;
            diagonal[b] += g;
        }
    };
    for l in 0..slots {
        let g = slot_t[l] / layout.resistivity;
        for j in 0..ny {
            for i in 0..nx {
                let v = raster[l][j * nx + i];
                if v == 0 {
                    continue;
                }
                if i + 1 < nx && raster[l][j * nx + i + 1] == v {
                    add(class_of[pixel(l, i, j)], class_of[pixel(l, i + 1, j)], g, &mut edges);
                }
                if j + 1 < ny && raster[l][(j + 1) * nx + i] == v {
                    add(class_of[pixel(l, i, j)], class_of[pixel(l, i, j + 1)], g, &mut edges);
                }
            }
        }
    }
    for b in &barrels {
        add(class_of[b.pixel_a], class_of[b.pixel_b], b.conductance, &mut edges);
    }
    let np = layout.ports.len();
    let mut potentials: Vec<Vec<f64>> = Vec::new();
    let mut resistance = vec![vec![f64::NAN; np]; np];
    for (pi, port) in layout.ports.iter().enumerate() {
        if port_open[pi] {
            potentials.push(Vec::new());
            continue;
        }
        let (cp, cm) = (class_of[terminal_pixels[port.plus][0]], class_of[terminal_pixels[port.minus][0]]);
        let mut b = vec![0.0; classes];
        b[cp] += 1.0;
        b[cm] -= 1.0;
        let Some(v) = conjugate_gradient(&edges, &diagonal, &b) else {
            return Err(format!(
                "the conduction solve of port {pi} did not converge ({classes} unknowns): no reference at this pitch"
            ));
        };
        potentials.push(v);
    }
    for i in 0..np {
        for j in 0..np {
            if port_open[i] || port_open[j] {
                continue;
            }
            let v = &potentials[j];
            let (cp, cm) = (
                class_of[terminal_pixels[layout.ports[i].plus][0]],
                class_of[terminal_pixels[layout.ports[i].minus][0]],
            );
            resistance[i][j] = v[cp] - v[cm];
        }
    }

    // Currents: per port, per layer, x-edge and y-edge cell arrays (padded for the convolution), and barrel currents.
    let (px, py) = (next_pow2(2 * nx), next_pow2(2 * ny));
    let live: Vec<usize> = (0..np).filter(|&p| !port_open[p]).collect();
    // currents[p][l][o]: real arrays of nx * ny (o = 0: x edges, 1: y edges); padded only for the transforms.
    let mut currents: Vec<Vec<[Vec<f64>; 2]>> = Vec::new();
    let mut barrel_currents: Vec<Vec<f64>> = Vec::new();
    for &p in &live {
        let v = &potentials[p];
        let mut per_l: Vec<[Vec<f64>; 2]> = Vec::new();
        for l in 0..slots {
            let g = slot_t[l] / layout.resistivity;
            let mut ax = vec![0.0f64; per_layer];
            let mut ay = vec![0.0f64; per_layer];
            for j in 0..ny {
                for i in 0..nx {
                    let val = raster[l][j * nx + i];
                    if val == 0 {
                        continue;
                    }
                    let me = v[class_of[pixel(l, i, j)]];
                    if i + 1 < nx && raster[l][j * nx + i + 1] == val {
                        ax[j * nx + i] = g * (me - v[class_of[pixel(l, i + 1, j)]]);
                    }
                    if j + 1 < ny && raster[l][(j + 1) * nx + i] == val {
                        ay[j * nx + i] = g * (me - v[class_of[pixel(l, i, j + 1)]]);
                    }
                }
            }
            per_l.push([ax, ay]);
        }
        currents.push(per_l);
        barrel_currents.push(
            barrels
                .iter()
                .map(|b| {
                    let i_ab = b.conductance * (v[class_of[b.pixel_a]] - v[class_of[b.pixel_b]]);
                    i_ab * (b.z_b - b.z_a).signum()
                })
                .collect(),
        );
    }
    let mut inductance = vec![vec![f64::NAN; np]; np];
    if !live.is_empty() {
        let c0 = unit_square_mean_inverse_distance();
        // Forward transforms of one port's current arrays (one port at a time: the padded spectra are the memory).
        let spectra_of = |per_l: &Vec<[Vec<f64>; 2]>| -> Vec<[Vec<Complex>; 2]> {
            per_l
                .iter()
                .map(|arrays| {
                    let padded = |a: &Vec<f64>| {
                        let mut out = vec![Complex::ZERO; px * py];
                        for j in 0..ny {
                            for i in 0..nx {
                                out[j * px + i] = Complex::new(a[j * nx + i], 0.0);
                            }
                        }
                        fft2(&mut out, px, py, false);
                        out
                    };
                    [padded(&arrays[0]), padded(&arrays[1])]
                })
                .collect()
        };
        // Kernels by height difference; U_b = sum_a K_ab * I_a for every port, then L_ij += I_b^j . U_b^i.
        let mut kernels: Vec<(f64, Vec<Complex>)> = Vec::new();
        let kernel = |dz: f64, kernels: &mut Vec<(f64, Vec<Complex>)>| -> usize {
            if let Some(k) = kernels.iter().position(|(d, _)| (d - dz).abs() < 1e-12) {
                return k;
            }
            let mut k = vec![Complex::ZERO; px * py];
            for n in 0..py {
                for m in 0..px {
                    let (mm, nn) = (
                        if m <= px / 2 { m as i64 } else { m as i64 - px as i64 },
                        if n <= py / 2 { n as i64 } else { n as i64 - py as i64 },
                    );
                    if mm.unsigned_abs() as usize > nx || nn.unsigned_abs() as usize > ny {
                        continue;
                    }
                    k[n * px + m] = Complex::new(pair_integral(mm, nn, dz, grid.h, c0), 0.0);
                }
            }
            fft2(&mut k, px, py, false);
            kernels.push((dz, k));
            kernels.len() - 1
        };
        let mut l_plane = vec![vec![0.0f64; np]; np];
        for (a, &pa) in live.iter().enumerate() {
            let spectra = spectra_of(&currents[a]);
            // U^a_b for every layer b and orientation.
            for b in 0..slots {
                let mut u = [vec![Complex::ZERO; px * py], vec![Complex::ZERO; px * py]];
                for l in 0..slots {
                    let dz = (slot_z[l] - slot_z[b]).abs();
                    let kidx = kernel(dz, &mut kernels);
                    let k = &kernels[kidx].1;
                    for o in 0..2 {
                        for (idx, target) in u[o].iter_mut().enumerate() {
                            *target = *target + spectra[l][o][idx] * k[idx];
                        }
                    }
                }
                for o in 0..2 {
                    fft2(&mut u[o], px, py, true);
                }
                for (c, &pc) in live.iter().enumerate() {
                    let mut sum = 0.0;
                    for o in 0..2 {
                        for j in 0..ny {
                            for i in 0..nx {
                                sum += currents[c][b][o][j * nx + i] * u[o][j * px + i].re;
                            }
                        }
                    }
                    l_plane[pc][pa] += sum;
                }
            }
        }
        // Via barrels: filament pairs.
        let mut l_via = vec![vec![0.0f64; np]; np];
        for (a, &pa) in live.iter().enumerate() {
            for (c, &pc) in live.iter().enumerate() {
                let mut sum = 0.0;
                for (ib, b1) in barrels.iter().enumerate() {
                    let i1 = barrel_currents[a][ib];
                    if i1 == 0.0 {
                        continue;
                    }
                    for (jb, b2) in barrels.iter().enumerate() {
                        let i2 = barrel_currents[c][jb];
                        if i2 == 0.0 {
                            continue;
                        }
                        let d = if ib == jb {
                            b1.radius
                        } else {
                            ((b1.x - b2.x).powi(2) + (b1.y - b2.y).powi(2)).sqrt().max(b1.radius.min(b2.radius))
                        };
                        let (a1, a2) = (b1.z_a.min(b1.z_b), b1.z_a.max(b1.z_b));
                        let (c1, c2) = (b2.z_a.min(b2.z_b), b2.z_a.max(b2.z_b));
                        sum += i1 * i2 * parallel_filaments(a1, a2, c1, c2, d);
                    }
                }
                l_via[pc][pa] = sum;
            }
        }
        for &pa in &live {
            for &pc in &live {
                // (mu0 / 4 pi) times the double integral; the plane part carries the 1 / h^2 of the sheet currents.
                inductance[pc][pa] = MU0_OVER_4PI * (l_plane[pc][pa] / (grid.h * grid.h) + l_via[pc][pa]) * 1e9;
            }
        }
    }
    Ok(ReferenceResult {
        pitch_nm: h_nm.round() as i64,
        grid: (nx, ny, slots),
        cells,
        components,
        edge_slivers,
        port_open,
        resistance_ohm: resistance,
        inductance_nh: inductance,
        terminals_snapped: snapped,
        notes,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// Jacobi-preconditioned conjugate gradient for the graph Laplacian of `edges` with `diagonal` its diagonal.
/// `None` when the residual does not fall to `1e-10` of the right-hand side within the iteration limit.
fn conjugate_gradient(edges: &[(usize, usize, f64)], diagonal: &[f64], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let matvec = |x: &[f64], y: &mut [f64]| {
        for (yi, (&xi, &d)) in y.iter_mut().zip(x.iter().zip(diagonal)) {
            *yi = d * xi;
        }
        for &(a, c, g) in edges {
            y[a] -= g * x[c];
            y[c] -= g * x[a];
        }
    };
    let precondition = |r: &[f64]| -> Vec<f64> {
        r.iter().zip(diagonal).map(|(&ri, &d)| if d > 0.0 { ri / d } else { 0.0 }).collect()
    };
    let mut x = vec![0.0; n];
    let mut r = b.to_vec();
    let mut z = precondition(&r);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let norm_b: f64 = b.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut ap = vec![0.0; n];
    for _ in 0..(40 * (n as f64).sqrt() as usize + 2000) {
        matvec(&p, &mut ap);
        let denom: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if denom <= 0.0 {
            return None;
        }
        let alpha = rz / denom;
        for k in 0..n {
            x[k] += alpha * p[k];
            r[k] -= alpha * ap[k];
        }
        let norm_r: f64 = r.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm_r <= 1e-10 * norm_b.max(1e-300) {
            return Some(x);
        }
        z = precondition(&r);
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_new / rz;
        rz = rz_new;
        for k in 0..n {
            p[k] = z[k] + beta * p[k];
        }
    }
    None
}

/// The resistivity the solver uses unless the layout states one.
pub const DEFAULT_RESISTIVITY: f64 = COPPER_RESISTIVITY;

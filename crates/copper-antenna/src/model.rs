//! The board-level full-wave model of one antenna site, and its evaluation.
//!
//! The model holds what [`copper_pcb::antenna::site_digest`] covers and
//! nothing else (any change to how it is simulated bumps
//! [`copper_pcb::antenna::MODEL_REVISION`]): the board outline extruded through every declared
//! dielectric (permittivity and loss tangent), the ground net's copper zones
//! on their layers as zero-thickness PEC sheets clipped around the antenna
//! region and the feed's clearance slot, the radiator and the feed land, and a
//! 50 ohm lumped port at the feed. Ground layers are assumed stitched by vias
//! at a tenth of the guided wavelength, which the model adds; without them two
//! ground planes form a parallel-plate cavity the port would drive.
//! Components, other nets' copper, zone holes, the enclosure and any cable are
//! not modelled.
//!
//! Ground is clipped geometrically rather than covered by a higher-priority
//! dielectric: the solver keeps every edge of a PEC sheet metal whatever
//! material lies over it (openEMS's priority rule for thin metal applies only
//! to openEMS model files).

use std::time::Instant;

use ems::csx::primitives::{BoxShape, LinPoly, Polygon};
use ems::csx::{ContinuousStructure, Material, Primitive, Property, PropertyId};
use ems::engine::StopReason;
use ems::prelude::*;

use copper_pcb::antenna::{AntennaSite, Axis as FrameAxis, Layout};
use copper_pcb::board::PhysicalBoard;
use copper_physical::geometry::point_in_polygon;
use copper_physical::{CopperLayer, Point};

/// The solver identity recorded in every report.
pub const SOLVER: &str = "ems 0.0.5 (pure-Rust FDTD, openEMS-compatible)";
const C0_MM_PER_S: f64 = 299_792_458_000.0;
const EPS0: f64 = 8.854_187_812_8e-12;
const PML_CELLS: usize = 8;

fn mm(nm: i64) -> f64 {
    nm as f64 * 1e-6
}

/// One dielectric slab, `top > bottom`, in millimetres (`F.Cu` at 0).
#[derive(Clone, Debug)]
pub struct Slab {
    pub id: String,
    pub top: f64,
    pub bottom: f64,
    pub er: f64,
    pub loss_tangent: f64,
}

/// Copper-layer heights and dielectric slabs, top down.
#[derive(Clone, Debug)]
pub struct Stack {
    pub copper: Vec<(CopperLayer, f64)>,
    pub slabs: Vec<Slab>,
}

impl Stack {
    pub fn z(&self, layer: CopperLayer) -> Option<f64> {
        self.copper.iter().find(|(l, _)| *l == layer).map(|(_, z)| *z)
    }

    pub fn max_er(&self) -> f64 {
        self.slabs.iter().map(|s| s.er).fold(1.0, f64::max)
    }
}

/// The declared stack-up as heights; copper thickness is neglected.
pub fn stack(board: &PhysicalBoard) -> Result<Stack, String> {
    let layers = &board.stackup.physical_layers;
    if layers.is_empty() {
        return Err("antenna synthesis requires a mechanical stackup with dielectric permittivities".to_owned());
    }
    let (mut z, mut copper, mut slabs) = (0.0_f64, Vec::new(), Vec::new());
    for layer in layers {
        match layer.copper_layer {
            Some(copper_layer) => copper.push((copper_layer, z)),
            None => {
                let Some(er) = &layer.relative_permittivity else {
                    return Err(format!("stackup dielectric {} declares no er", layer.id));
                };
                let er = copper_pcb::antenna::decimal_f64(er);
                let loss_tangent = layer.loss_tangent.as_ref().map_or(0.0, copper_pcb::antenna::decimal_f64);
                let bottom = z - mm(layer.thickness_nm);
                slabs.push(Slab { id: layer.id.clone(), top: z, bottom, er, loss_tangent });
                z = bottom;
            }
        }
    }
    if copper.first().map(|(l, _)| *l) != Some(CopperLayer::Front) {
        return Err("antenna synthesis requires F.Cu at the top of the stackup".to_owned());
    }
    Ok(Stack { copper, slabs })
}

/// How finely and how long one evaluation runs.
#[derive(Clone, Debug)]
pub struct Fidelity {
    pub name: &'static str,
    /// The cell size over the antenna region, in millimetres.
    pub cell: f64,
    /// Stop when the field energy has decayed this far (dB, negative).
    pub decay_db: f64,
    /// Simulated-time cap, in nanoseconds.
    pub max_time_ns: f64,
    /// Record a near-to-far-field box for radiation efficiency.
    pub efficiency: bool,
    /// Solver threads; 0 picks [`auto_threads`] for each mesh.
    pub threads: usize,
}

/// The solver thread count for a mesh of `nodes` lines per axis: one per
/// physical core, at most one per five x-planes and per 4096 cells (the
/// solver's own rule). The time loop is memory-bandwidth bound, so a second
/// hardware thread per core only adds synchronisation: on the 6-core,
/// 12-thread laptop of `docs/antenna-synthesis-performance.md`, 6 threads
/// ran 1.74x faster than 12. The solver counts cores itself on Linux; on
/// Windows, where it cannot, half the logical processors is assumed (SMT
/// is near universal on current x86 parts, and a bandwidth-bound loop loses
/// little if a part without it gets fewer threads); Apple silicon has no
/// SMT. `--threads` overrides it.
pub fn auto_threads(nodes: [usize; 3]) -> usize {
    let solver = ems::engine::default_threads_for(nodes);
    if cfg!(windows) {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        solver.min((logical / 2).max(1))
    } else {
        solver
    }
}

impl Fidelity {
    /// Two cells across the track: the search fidelity.
    pub fn coarse(site: &AntennaSite, threads: usize) -> Fidelity {
        let cell = (mm(site.width) / 2.0).clamp(0.2, 0.75);
        Fidelity { name: "coarse", cell, decay_db: -25.0, max_time_ns: 25.0, efficiency: false, threads }
    }

    /// Four cells across the track, longer ring-down and far field: the verification fidelity.
    pub fn fine(site: &AntennaSite, threads: usize) -> Fidelity {
        let cell = (mm(site.width) / 4.0).clamp(0.1, 0.4);
        Fidelity { name: "fine", cell, decay_db: -35.0, max_time_ns: 40.0, efficiency: true, threads }
    }
}

type Rect = [f64; 4];
type Ring = Vec<(f64, f64)>;

/// How the port reaches ground.
#[derive(Clone, Debug, PartialEq)]
pub enum PortKind {
    /// In the plane of `F.Cu`, across the feed slot from the land to the
    /// top-layer ground, as a coax shield soldered beside the land.
    Lateral,
    /// From the land down to the first ground layer under it.
    Vertical { layer: CopperLayer, z: f64 },
}

/// The fixed part of one site's model: stack, clipped ground, stitching,
/// port and analysis window.
pub struct Problem<'a> {
    pub board: &'a PhysicalBoard,
    pub site: &'a AntennaSite,
    pub stack: Stack,
    /// Ground sheets as `(z, ring)`, clipped around the region and slot.
    pub ground: Vec<(f64, Ring)>,
    /// Stitching vias as `(x, y, z_top, z_bottom)`.
    pub stitching: Vec<(f64, f64, f64, f64)>,
    pub stitch_pitch: f64,
    pub port: PortKind,
    /// The feed's clearance slot on F.Cu.
    pub slot: Rect,
    /// The modelled gap from the feed land to the top-layer ground below it,
    /// in nanometres: the board clearance, widened to one coarse cell so the
    /// port spans the mesh the same way in every fidelity.
    pub slot_gap: i64,
    /// Analysis frequencies ([`sweep_frequencies`]): from below the lowest
    /// band to above the highest, at least 0.8 to 1.2 times the centre.
    pub freqs: Vec<f64>,
}

/// Sutherland-Hodgman clip of `ring` to the half plane `coordinate(axis) <= value`
/// (`keep_below`) or `>= value`.
fn clip(ring: &Ring, axis: usize, value: f64, keep_below: bool) -> Ring {
    let at = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
    let inside = |p: (f64, f64)| if keep_below { at(p) <= value } else { at(p) >= value };
    let mut out = Vec::new();
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        if inside(a) {
            out.push(a);
        }
        if inside(a) != inside(b) {
            let t = (value - at(a)) / (at(b) - at(a));
            out.push((a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1)));
        }
    }
    out
}

fn area(ring: &Ring) -> f64 {
    (0..ring.len())
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

/// `ring` minus the rectangle `r`, as up to four pieces: left, right, and the
/// strips below and above `r` between its sides.
fn subtract(ring: &Ring, r: Rect) -> Vec<Ring> {
    let strip = |ring: &Ring| clip(&clip(ring, 0, r[0], false), 0, r[2], true);
    let pieces = [
        clip(ring, 0, r[0], true),
        clip(ring, 0, r[2], false),
        clip(&strip(ring), 1, r[1], true),
        clip(&strip(ring), 1, r[3], false),
    ];
    pieces.into_iter().filter(|p| p.len() >= 3 && area(p) > 1e-6).collect()
}

fn board_rect(site: &AntennaSite, u0: i64, v0: i64, u1: i64, v1: i64) -> Rect {
    let (a, b) = (site.frame.to_board(u0, v0), site.frame.to_board(u1, v1));
    [mm(a.x_nm.min(b.x_nm)), mm(a.y_nm.min(b.y_nm)), mm(a.x_nm.max(b.x_nm)), mm(a.y_nm.max(b.y_nm))]
}

fn land_halves(site: &AntennaSite) -> (i64, i64) {
    let land = &site.feed_land;
    match site.frame.axis {
        FrameAxis::X => (land.size.width_nm / 2, land.size.height_nm / 2),
        FrameAxis::Y => (land.size.height_nm / 2, land.size.width_nm / 2),
    }
}

impl<'a> Problem<'a> {
    pub fn new(board: &'a PhysicalBoard, site: &'a AntennaSite) -> Result<Problem<'a>, String> {
        let stack = stack(board)?;
        let feed = site.frame.to_board(site.feed_u, site.feed_v);
        let zones: Vec<_> = board.zones.iter().filter(|z| z.net == site.ground).collect();
        if zones.is_empty() {
            return Err(format!(
                "antenna synthesis needs a copper_zone on the ground net {} to model the ground plane",
                site.ground
            ));
        }
        let (along, across) = land_halves(site);
        let reach = (site.width / 2).max(along) + site.clearance;
        let coarse_cell = (Fidelity::coarse(site, 1).cell * 1e6).round() as i64;
        let slot_gap = site.clearance.max(coarse_cell);
        let slot = board_rect(site, site.feed_u - reach, site.feed_v - across - slot_gap, site.feed_u + reach, 0);
        let region = [mm(site.area.min_x), mm(site.area.min_y), mm(site.area.max_x), mm(site.area.max_y)];
        let covers = |layer: CopperLayer, point: Point| {
            zones.iter().any(|zone| zone.layers.contains(&layer) && point_in_polygon(point, &zone.outer))
        };
        let top_ground = covers(CopperLayer::Front, feed);
        let port = if top_ground {
            PortKind::Lateral
        } else {
            let below = stack.copper.iter().skip(1).find(|(layer, _)| covers(*layer, feed)).copied();
            let Some((layer, z)) = below else {
                return Err(format!(
                    "antenna synthesis needs ground net {} zoned at the feed pad, on F.Cu or a layer below it, as \
                     the port's return",
                    site.ground
                ));
            };
            PortKind::Vertical { layer, z }
        };
        let mut ground = Vec::new();
        for zone in &zones {
            let outer: Ring = zone.outer.iter().map(|p| (mm(p.x_nm), mm(p.y_nm))).collect();
            for layer in &zone.layers {
                let Some(z) = stack.z(*layer) else { continue };
                let mut pieces = subtract(&outer, region);
                if *layer == CopperLayer::Front {
                    pieces = pieces.iter().flat_map(|piece| subtract(piece, slot)).collect();
                }
                ground.extend(pieces.into_iter().map(|piece| (z, piece)));
            }
        }
        let centre = site.centre_hz();
        let bands: Vec<(f64, f64)> = site
            .bands
            .iter()
            .map(|b| (copper_pcb::antenna::decimal_f64(&b.low_hz), copper_pcb::antenna::decimal_f64(&b.high_hz)))
            .collect();
        let freqs = sweep_frequencies(&bands);
        let f_top = freqs.last().copied().unwrap_or(centre * 1.2);
        let guided = C0_MM_PER_S / f_top / stack.max_er().sqrt();
        let stitch_pitch = (guided / 10.0).min(5.0);
        let ground_layers: Vec<(CopperLayer, f64)> = stack
            .copper
            .iter()
            .copied()
            .filter(|(layer, _)| zones.iter().any(|zone| zone.layers.contains(layer)))
            .collect();
        let mut stitching = Vec::new();
        if ground_layers.len() >= 2 {
            let xs: Vec<f64> = zones.iter().flat_map(|z| z.outer.iter().map(|p| mm(p.x_nm))).collect();
            let ys: Vec<f64> = zones.iter().flat_map(|z| z.outer.iter().map(|p| mm(p.y_nm))).collect();
            let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let keep_clear =
                |r: Rect, x: f64, y: f64| x > r[0] - 1.0 && x < r[2] + 1.0 && y > r[1] - 1.0 && y < r[3] + 1.0;
            let (x0, y0) = (lo(&xs) + stitch_pitch / 2.0, lo(&ys) + stitch_pitch / 2.0);
            let mut y = y0;
            while y < hi(&ys) {
                let mut x = x0;
                while x < hi(&xs) {
                    let point = Point::new((x * 1e6) as i64, (y * 1e6) as i64);
                    let layers: Vec<f64> =
                        ground_layers.iter().filter(|(layer, _)| covers(*layer, point)).map(|(_, z)| *z).collect();
                    if layers.len() >= 2 && !keep_clear(region, x, y) && !keep_clear(slot, x, y) {
                        let (top, bottom) = (
                            layers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                            layers.iter().copied().fold(f64::INFINITY, f64::min),
                        );
                        stitching.push((x, y, top, bottom));
                    }
                    x += stitch_pitch;
                }
                y += stitch_pitch;
            }
        }
        Ok(Problem { board, site, stack, ground, stitching, stitch_pitch, port, slot, slot_gap, freqs })
    }

    /// A one-line description of the port for reports.
    pub fn port_description(&self) -> String {
        match &self.port {
            PortKind::Lateral => format!(
                "Lumped 50 ohm port on F.Cu across the feed slot, from the feed land to the top-layer ground (a coax \
                 shield soldered beside the land); the slot below the land is modelled {:.2} mm wide",
                self.slot_gap as f64 * 1e-6
            ),
            PortKind::Vertical { layer, z } => {
                format!("Lumped 50 ohm port from the feed land down to {} at {:.3} mm depth", layer.value(), -z)
            }
        }
    }
}

/// `sweep_frequencies`: the analysis frequencies for `bands` (`(low, high)`
/// in hertz). The sweep spans at least 0.8 to 1.2 times the combined centre
/// and always reaches 10 % beyond the lowest and highest band limits, so
/// every requested band lies inside the simulated range; its step is at most
/// 0.2 % of the centre.
pub fn sweep_frequencies(bands: &[(f64, f64)]) -> Vec<f64> {
    let low = bands.iter().map(|b| b.0).fold(f64::INFINITY, f64::min);
    let high = bands.iter().map(|b| b.1).fold(f64::NEG_INFINITY, f64::max);
    if !(low.is_finite() && high.is_finite() && low > 0.0 && high >= low) {
        return Vec::new();
    }
    let centre = (low + high) / 2.0;
    let (start, stop) = ((0.8 * centre).min(0.9 * low), (1.2 * centre).max(1.1 * high));
    let steps = ((stop - start) / (0.002 * centre) - 1e-6).ceil().max(200.0) as u32;
    (0..=steps).map(|i| start + (stop - start) * f64::from(i) / f64::from(steps)).collect()
}

/// `excitation`: the Gaussian pulse `(f0, fc)` (20 dB half-bandwidth `fc`)
/// for a sweep over `freqs`: centred on the sweep with twice its half-span,
/// kept below `f0` so the pulse carries little DC.
fn excitation(freqs: &[f64]) -> (f64, f64) {
    let (start, stop) = (freqs[0], freqs[freqs.len() - 1]);
    let (f0, half) = ((start + stop) / 2.0, (stop - start) / 2.0);
    (f0, (2.0 * half).min(0.95 * f0).max(half))
}

/// One full-wave evaluation.
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub fidelity: &'static str,
    pub freqs: Vec<f64>,
    pub z: Vec<Complex64>,
    pub s11_db: Vec<f64>,
    /// `(frequency, radiation efficiency)` at each band centre, when recorded.
    pub efficiency: Vec<(f64, f64)>,
    pub cells: usize,
    pub steps: u64,
    pub dt_ps: f64,
    pub seconds: f64,
    /// Whether the energy decayed to the fidelity's threshold before the time cap.
    pub converged: bool,
    pub warnings: Vec<String>,
}

/// How a candidate mesh line ranks when two lie closer than the snap distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LineRank {
    /// Ground vertices and the board outline: their sheets snap onto a nearby line.
    Anchor,
    /// Edges of the slot, region and the region's padding.
    Edge,
    /// Edges of the radiator, feed land, short and via: what the search
    /// moves, so a sub-cell change of a parameter never moves them a cell.
    Copper,
    /// The port's terminals and the line between them: never moved.
    Port,
}

/// One axis of an evaluation's mesh, from which [`axis_lines`] builds every line.
#[derive(Clone, Debug)]
struct AxisPlan {
    lines: Vec<(f64, LineRank)>,
    /// `(lo, hi, limit)`: no cell between `lo` and `hi` wider than `limit`.
    zones: Vec<(f64, f64, f64)>,
    /// Lines closer than this merge into the higher-ranked one (port lines never merge).
    snap: f64,
    /// The domain's ends (air margin and PML included).
    lo: f64,
    hi: f64,
    /// The largest cell anywhere, and the growth ratio of neighbouring cells.
    max_cell: f64,
    ratio: f64,
}

/// The mesh lines of one axis: every line is chosen here, none is detected
/// from the model's primitives.
///
/// 1. Candidate lines merge when closer than `snap`, or half the limit of a
///    finer zone they lie in (the lower rank moves onto the higher; port
///    lines all stay), so no incidental pair of edges
///    leaves a sliver: the smallest cell sets the FDTD time step.
/// 2. Between the outermost zone bounds, each gap is split into the fewest
///    equal cells no wider than the tightest zone limit over it.
/// 3. Outside, `smooth_lines` grades from the outer zone cells to the domain
///    ends. It runs only there: inside the zones its ratio rule would split
///    a 0.3 mm cell next to a 0.5 mm one into slivers, which is how the
///    csx-built mesh lost half its time step.
fn axis_lines(plan: &AxisPlan) -> Result<Vec<f64>, String> {
    let mut candidates = plan.lines.clone();
    for (lo, hi, _) in &plan.zones {
        candidates.extend([(*lo, LineRank::Anchor), (*hi, LineRank::Anchor)]);
    }
    candidates.retain(|(v, _)| v.is_finite() && *v > plan.lo && *v < plan.hi);
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Inside a zone finer than the default, edges keep half its limit apart.
    let snap_at = |x: f64| {
        plan.zones
            .iter()
            .filter(|(lo, hi, _)| x > *lo && x < *hi)
            .map(|(_, _, limit)| limit / 2.0)
            .fold(plan.snap, f64::min)
    };
    let mut kept: Vec<(f64, LineRank)> = Vec::new();
    for (v, rank) in candidates {
        match kept.last().copied() {
            Some((last, _)) if v - last < 1e-9 => {
                let top = kept.last_mut().expect("a line");
                top.1 = top.1.max(rank);
            }
            Some((last, last_rank)) if v - last < snap_at((last + v) / 2.0) => match (last_rank, rank) {
                (LineRank::Port, LineRank::Port) => kept.push((v, rank)),
                (_, r) if r > last_rank => {
                    kept.pop();
                    kept.push((v, rank));
                }
                _ => {}
            },
            _ => kept.push((v, rank)),
        }
    }
    let kept: Vec<f64> = kept.into_iter().map(|(v, _)| v).collect();
    let zone_lo = plan.zones.iter().map(|z| z.0).fold(f64::INFINITY, f64::min);
    let zone_hi = plan.zones.iter().map(|z| z.1).fold(f64::NEG_INFINITY, f64::max);
    let inner_lines: Vec<f64> =
        kept.iter().copied().filter(|v| *v >= zone_lo - plan.snap && *v <= zone_hi + plan.snap).collect();
    if inner_lines.len() < 2 {
        let mut all = vec![plan.lo];
        all.extend(kept);
        all.push(plan.hi);
        return ems::csx::grid::smooth_lines(&all, plan.max_cell, plan.ratio).map_err(|e| e.to_string());
    }
    // No zone may ask for cells above the frequency limit.
    let coarsest = plan.zones.iter().map(|z| z.2).fold(0.0, f64::max).min(plan.max_cell);
    let mut inner = vec![inner_lines[0]];
    for pair in inner_lines.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let middle = (a + b) / 2.0;
        let limit = plan
            .zones
            .iter()
            .filter(|(lo, hi, _)| middle > *lo && middle < *hi)
            .map(|(_, _, limit)| limit.min(plan.max_cell))
            .fold(coarsest, f64::min);
        let n = ((b - a) / limit - 1e-9).ceil().max(1.0) as usize;
        inner.extend((1..=n).map(|i| a + (b - a) * i as f64 / n as f64));
    }
    let (first, last) = (inner[0], inner[inner.len() - 1]);
    let (c_first, c_last) = (inner[1] - inner[0], inner[inner.len() - 1] - inner[inner.len() - 2]);
    // Each side is graded from a copy of the zone's outer cell, unless an
    // anchor already stands within one and a half of it.
    let side = |anchors: Vec<f64>, end: f64, edge: f64, cell: f64| -> Result<Vec<f64>, String> {
        let mut fixed = anchors;
        let helper = if end < edge { edge - cell } else { edge + cell };
        if fixed.iter().all(|v| (v - edge).abs() > 1.5 * cell) && (helper - end).abs() > cell {
            fixed.push(helper);
        }
        fixed.extend([end, edge]);
        fixed.sort_by(f64::total_cmp);
        ems::csx::grid::smooth_lines(&fixed, plan.max_cell, plan.ratio).map_err(|e| e.to_string())
    };
    let below = side(kept.iter().copied().filter(|v| *v < first).collect(), plan.lo, first, c_first)?;
    let above = side(kept.iter().copied().filter(|v| *v > last).collect(), plan.hi, last, c_last)?;
    let mut lines: Vec<f64> = below.into_iter().filter(|v| *v < first).collect();
    lines.extend(inner);
    lines.extend(above.into_iter().filter(|v| *v > last));
    Ok(lines)
}

/// The radiator's copper as board rectangles: each track segment widened to
/// the track width with square ends, plus the tiles and the feed land.
pub fn copper_rects(site: &AntennaSite, layout: &Layout) -> Vec<Rect> {
    let half = site.width / 2;
    let mut rects: Vec<Rect> = layout
        .path
        .windows(2)
        .map(|pair| {
            let ((ua, va), (ub, vb)) = (pair[0], pair[1]);
            board_rect(site, ua.min(ub) - half, va.min(vb) - half, ua.max(ub) + half, va.max(vb) + half)
        })
        .collect();
    rects.extend(layout.tiles.iter().map(|[u0, v0, u1, v1]| board_rect(site, *u0, *v0, *u1, *v1)));
    let (along, across) = land_halves(site);
    rects.push(board_rect(site, site.feed_u - along, site.feed_v - across, site.feed_u + along, site.feed_v + across));
    // The inverted-F short: its leg, the link bridged as one conductor from
    // its feed land to its ground land, and the ground stub to the via.
    if let (Some(link), Some(short)) = (&site.short, &layout.short) {
        rects.extend(short.leg.windows(2).map(|pair| {
            let ((ua, va), (ub, vb)) = (pair[0], pair[1]);
            board_rect(site, ua.min(ub) - half, va.min(vb) - half, ua.max(ub) + half, va.max(vb) + half)
        }));
        let (fu, fv) = short.feed_pad;
        let (gu, gv) = short.ground_pad;
        let reach = half.max(link.feed_pad.size.width_nm.min(link.feed_pad.size.height_nm) / 2);
        rects.push(board_rect(site, fu.min(gu) - reach, fv.min(gv) - reach, fu.max(gu) + reach, fv.max(gv) + reach));
        let (vu, vv) = short.via;
        rects.push(board_rect(site, gu.min(vu) - half, gv.min(vv) - half, gu.max(vu) + half, gv.max(vv) + half));
        let r = link.via_size / 2;
        rects.push(board_rect(site, vu - r, vv - r, vu + r, vv + r));
    }
    rects
}

/// The z of a via's far end when it leaves F.Cu, in millimetres.
fn via_bottom(stack: &Stack, to: CopperLayer) -> Option<f64> {
    stack.z(to).filter(|z| *z < 0.0)
}

fn sheet(model: &mut ContinuousStructure, id: PropertyId, r: Rect, z: f64) -> Result<(), String> {
    model
        .add_primitive(id, Primitive::new(BoxShape::new([r[0], r[1], z], [r[2], r[3], z])).with_priority(30))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Build and run one evaluation of `layout`.
pub fn evaluate(problem: &Problem, layout: &Layout, fidelity: &Fidelity) -> Result<Evaluation, String> {
    let (board, site, stack) = (problem.board, problem.site, &problem.stack);
    if problem.freqs.len() < 2 {
        return Err("antenna bands give no analysis frequencies".to_owned());
    }
    let started = Instant::now();
    let centre = site.centre_hz();
    let mut model = ContinuousStructure::new();
    model.grid.delta_unit = 1e-3;
    let err = |e: ems::csx::Error| e.to_string();
    let outline: Ring = board.outline.vertices.iter().map(|p| (mm(p.x_nm), mm(p.y_nm))).collect();

    for slab in &stack.slabs {
        let kappa = 2.0 * std::f64::consts::PI * centre * EPS0 * slab.er * slab.loss_tangent;
        let id = model.add_property(Property::material(format!("slab-{}", slab.id), Material::new(slab.er, kappa)));
        model
            .add_primitive(id, Primitive::new(LinPoly::new(2, slab.bottom, slab.top - slab.bottom, outline.clone())))
            .map_err(err)?;
    }
    let ground = model.add_property(Property::metal("ground"));
    for (z, ring) in &problem.ground {
        model
            .add_primitive(ground, Primitive::new(Polygon::new(2, *z, ring.clone())).with_priority(10))
            .map_err(err)?;
    }
    let antenna = model.add_property(Property::metal("antenna"));
    let copper = copper_rects(site, layout);
    for rect in &copper {
        sheet(&mut model, antenna, *rect, 0.0)?;
    }
    // The short's ground via is a barrel over the span the board gets, F.Cu
    // to `via_to`, stub below the last ground plane included: a flat square
    // alone leaves the short open when ground is zoned only below F.Cu.
    let mut via_rect = None;
    if let (Some(link), Some(short)) = (&site.short, &layout.short) {
        let (vu, vv) = short.via;
        let r = link.via_size / 2;
        let via = board_rect(site, vu - r, vv - r, vu + r, vv + r);
        via_rect = Some(via);
        if let Some(bottom) = via_bottom(stack, link.via_to) {
            model
                .add_primitive(
                    ground,
                    Primitive::new(BoxShape::new([via[0], via[1], bottom], [via[2], via[3], 0.0])).with_priority(10),
                )
                .map_err(err)?;
        }
    }

    // The port.
    let (along, across) = land_halves(site);
    let a = site.width.min(2 * along) / 2;
    let port_box = match &problem.port {
        PortKind::Lateral => {
            let r = board_rect(
                site,
                site.feed_u - a,
                site.feed_v - across - problem.slot_gap,
                site.feed_u + a,
                site.feed_v - across,
            );
            ([r[0], r[1], 0.0], [r[2], r[3], 0.0])
        }
        PortKind::Vertical { z, .. } => {
            let r = board_rect(site, site.feed_u - a, site.feed_v - a, site.feed_u + a, site.feed_v + a);
            ([r[0], r[1], *z], [r[2], r[3], 0.0])
        }
    };
    let direction = match (&problem.port, site.frame.axis) {
        (PortKind::Vertical { .. }, _) => Axis::Z,
        (PortKind::Lateral, FrameAxis::X) => Axis::Y,
        (PortKind::Lateral, FrameAxis::Y) => Axis::X,
    };
    let port = LumpedPort::new(1, 50.0, port_box.0, port_box.1, direction)
        .excite(1.0)
        .add_to(&mut model)
        .map_err(|e| e.to_string())?;

    // Mesh: every line is chosen by `axis_lines`; csx detects none from the
    // primitives, so no stitching via or polygon vertex adds a sliver.
    // COPPER_ANTENNA_FINE_CELL overrides the fine fidelity's cell, for
    // convergence references (docs/antenna-synthesis-performance.md).
    let f_high = problem.freqs.last().copied().unwrap_or(centre);
    let max_cell = (C0_MM_PER_S / f_high / stack.max_er().sqrt() / 15.0).min(3.0);
    // The fidelity's cell, never coarser than the frequency limit: the feed
    // and short zones refine from it, so they stay finer than the rest.
    let cell = std::env::var("COPPER_ANTENNA_FINE_CELL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|c| fidelity.name == "fine" && *c > 0.0)
        .unwrap_or(fidelity.cell)
        .min(max_cell);
    let region = [mm(site.area.min_x), mm(site.area.min_y), mm(site.area.max_x), mm(site.area.max_y)];
    let pad = 2.0 * cell;
    let port_rect = [port_box.0[0], port_box.0[1], port_box.1[0], port_box.1[1]];
    // The port's terminals must stay on distinct lines however close they
    // are, with a line between them along the port: a one-cell gap whose two
    // nodes both touch metal discretizes as a PEC short.
    let mid = |a: f64, b: f64| (a + b) / 2.0;
    let along_port = |k: usize| direction.index() == k;
    let keep = |k: usize| {
        let (a, b) = (port_box.0[k], port_box.1[k]);
        if along_port(k) { vec![a, mid(a, b), b] } else { vec![a, b] }
    };
    let mut planned: [Vec<(f64, LineRank)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (k, lines) in planned.iter_mut().enumerate().take(2) {
        lines.extend(keep(k).into_iter().map(|v| (v, LineRank::Port)));
        lines.extend([(region[k] - pad, LineRank::Edge), (region[k + 2] + pad, LineRank::Edge)]);
        for r in [&problem.slot, &region, &port_rect] {
            lines.extend([(r[k], LineRank::Edge), (r[k + 2], LineRank::Edge)]);
        }
        for r in copper.iter().chain(via_rect.as_ref()) {
            lines.extend([(r[k], LineRank::Copper), (r[k + 2], LineRank::Copper)]);
        }
    }
    // The antenna's own cells span every feature line; around the port, a
    // finer limit resolves the feed gap at every fidelity (at two coarse
    // cells across the gap, the result depended on incidental lines).
    let feature_span = |k: usize| {
        let v = planned[k].iter().map(|(v, _)| *v);
        (v.clone().fold(f64::INFINITY, f64::min), v.fold(f64::NEG_INFINITY, f64::max))
    };
    let feed_zone = |k: usize| (port_rect[k] - 1.0, port_rect[k + 2] + 1.0, 0.75 * cell);
    // An inverted-F's short (leg, link and ground via) sets its impedance as
    // the feed gap does, and gets the same resolution.
    let short_box = layout.short.as_ref().map(|short| {
        let points: Vec<Point> = short
            .leg
            .iter()
            .chain([&short.centre, &short.feed_pad, &short.ground_pad, &short.via])
            .map(|(u, v)| site.frame.to_board(*u, *v))
            .collect();
        let (xs, ys) = (points.iter().map(|p| mm(p.x_nm)), points.iter().map(|p| mm(p.y_nm)));
        [
            xs.clone().fold(f64::INFINITY, f64::min) - 1.0,
            ys.clone().fold(f64::INFINITY, f64::min) - 1.0,
            xs.fold(f64::NEG_INFINITY, f64::max) + 1.0,
            ys.fold(f64::NEG_INFINITY, f64::max) + 1.0,
        ]
    });
    let zones_xy: [Vec<(f64, f64, f64)>; 2] = [0, 1].map(|k| {
        let (a, b) = feature_span(k);
        let mut zones = vec![(a, b, cell), feed_zone(k)];
        zones.extend(short_box.map(|r| (r[k], r[k + 2], 0.75 * cell)));
        zones
    });
    for (x, y) in outline.iter().chain(problem.ground.iter().flat_map(|(_, ring)| ring.iter())) {
        planned[0].push((*x, LineRank::Anchor));
        planned[1].push((*y, LineRank::Anchor));
    }
    let z_cell = cell.max(0.15);
    let keep_z: Vec<f64> = if port_box.0[2] == port_box.1[2] { vec![] } else { keep(2) };
    planned[2].extend(stack.copper.iter().map(|(_, z)| (*z, LineRank::Edge)));
    planned[2].extend(stack.slabs.iter().flat_map(|s| [(s.top, LineRank::Edge), (s.bottom, LineRank::Edge)]));
    planned[2].extend(keep_z.iter().map(|z| (*z, LineRank::Port)));
    let z_bottom = planned[2].iter().map(|(z, _)| *z).fold(0.0, f64::min);
    let zone_z = (z_bottom, 4.0 * cell, z_cell);
    // An eighth of a wavelength of air before the PML: S11 and the far-field
    // box are unchanged from a quarter (docs/antenna-synthesis-performance.md).
    let margin = C0_MM_PER_S / centre / 8.0 + PML_CELLS as f64 * max_cell;
    let span = |k: usize, extra: &[f64]| {
        let v = planned[k].iter().map(|(v, _)| *v).chain(extra.iter().copied());
        (v.clone().fold(f64::INFINITY, f64::min) - margin, v.fold(f64::NEG_INFINITY, f64::max) + margin)
    };
    for k in 0..3 {
        let (zones, snap, extra) = if k < 2 {
            (zones_xy[k].clone(), cell / 2.0, vec![])
        } else {
            (vec![zone_z], z_cell / 2.0, vec![zone_z.0, zone_z.1])
        };
        let (lo, hi) = span(k, &extra);
        let plan = AxisPlan { lines: planned[k].clone(), zones, snap, lo, hi, max_cell, ratio: 1.3 };
        model.grid.set_lines(k, axis_lines(&plan)?).map_err(err)?;
    }
    // Stitching vias as PEC wires on the nearest mesh lines: they add none.
    let nearest = |k: usize, v: f64| {
        model.grid.lines(k).iter().copied().min_by(|a, b| (a - v).abs().total_cmp(&(b - v).abs())).unwrap_or(v)
    };
    let stitches: Vec<[f64; 4]> =
        problem.stitching.iter().map(|(x, y, top, bottom)| [nearest(0, *x), nearest(1, *y), *top, *bottom]).collect();
    for [x, y, top, bottom] in stitches {
        model
            .add_primitive(ground, Primitive::new(BoxShape::new([x, y, bottom], [x, y, top])).with_priority(10))
            .map_err(err)?;
    }
    if std::env::var_os("COPPER_ANTENNA_DEBUG").is_some() {
        for k in 0..3 {
            let (a, b) = (port_box.0[k].min(port_box.1[k]) - 1.0, port_box.0[k].max(port_box.1[k]) + 1.0);
            let near: Vec<String> =
                model.grid.lines(k).iter().filter(|v| (a..=b).contains(*v)).map(|v| format!("{v:.4}")).collect();
            eprintln!("debug axis {k}: port {:.4}..{:.4}, lines {}", port_box.0[k], port_box.1[k], near.join(" "));
        }
    }
    let n = model.grid.num_lines();
    let cells = n[0].saturating_sub(1) * n[1].saturating_sub(1) * n[2].saturating_sub(1);
    let threads = if fidelity.threads == 0 { auto_threads(n) } else { fidelity.threads };
    let profile = std::env::var_os("COPPER_ANTENNA_PROFILE").is_some();
    let meshed = started.elapsed().as_secs_f64();
    if profile {
        mesh_census(&model, problem, &outline, stack, cell, max_cell, margin);
    }

    let fdtd = Fdtd::new()
        .waveform({
            let (f0, fc) = excitation(&problem.freqs);
            Waveform::gaussian(f0, fc)
        })
        .boundaries(Boundaries::all(Boundary::pml(PML_CELLS)))
        .end_criteria(EndCriteria::energy_decay_db(fidelity.decay_db))
        .max_time(fidelity.max_time_ns * 1e-9)
        .threads(threads);
    let mut sim = Simulation::new(&model, &fdtd).map_err(|e| e.to_string())?;
    let dt_ps = sim.engine().dt() * 1e12;
    let centres: Vec<f64> = site
        .bands
        .iter()
        .map(|b| (copper_pcb::antenna::decimal_f64(&b.low_hz) + copper_pcb::antenna::decimal_f64(&b.high_hz)) / 2.0)
        .collect();
    let nf2ff = if fidelity.efficiency {
        // A box around the board, halfway out into the air margin.
        let bx: Vec<f64> = outline.iter().map(|p| p.0).collect();
        let by: Vec<f64> = outline.iter().map(|p| p.1).collect();
        let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let gap = (margin - PML_CELLS as f64 * max_cell) / 2.0;
        let bottom = stack.slabs.last().map_or(0.0, |s| s.bottom);
        let nf = Nf2ffBox::new(
            [lo(&bx) - gap, lo(&by) - gap, bottom - gap],
            [hi(&bx) + gap, hi(&by) + gap, gap],
            centres.clone(),
        );
        Some(sim.add_nf2ff(&nf).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let warnings: Vec<String> = sim.warnings().to_vec();
    let compiled = started.elapsed().as_secs_f64();
    if std::env::var("COPPER_ANTENNA_PROFILE").is_ok_and(|v| v.contains("mesh")) {
        eprintln!("profile {}: cells {cells} dt {dt_ps:.4} ps (mesh-only run)", fidelity.name);
        return Err("COPPER_ANTENNA_PROFILE=mesh: stopped after compiling".to_owned());
    }
    let results = sim.run().map_err(|e| e.to_string())?;
    let ran = started.elapsed().as_secs_f64();
    let outcome = results.outcome().cloned();
    let converged = matches!(outcome.as_ref().map(|o| &o.reason), Some(StopReason::EnergyDecay { .. }));
    let steps = outcome.as_ref().map_or(0, |o| o.steps);
    if profile {
        let run = ran - compiled;
        let updates = cells as f64 * steps as f64;
        eprintln!(
            "profile {}: threads {} cells {cells} steps {steps} dt {dt_ps:.4} ps simulated {:.2} ns converged {converged}; \
             mesh {meshed:.1} s, compile {:.1} s, run {run:.1} s; {:.0} Mcell-updates/s, {:.0} us/step",
            fidelity.name,
            threads,
            steps as f64 * dt_ps * 1e-3,
            compiled - meshed,
            updates / run / 1e6,
            run / steps.max(1) as f64 * 1e6,
        );
    }
    let analysed = port
        .analyze(&results, &problem.freqs)
        .map_err(|e| format!("{e}; solver setup warnings: {}", warnings.join(" | ")))?;
    let s11_db = analysed.s11().iter().map(|s| 20.0 * s.norm().max(1e-12).log10()).collect();
    let z = analysed.z_in();
    let mut efficiency = Vec::new();
    if let Some(handle) = nf2ff {
        let far = sim.nf2ff(&handle);
        let accepted = port.analyze(&results, &centres).map_err(|e| e.to_string())?.accepted_power();
        for (k, f) in centres.iter().enumerate() {
            let radiated = far.radiated_power(k);
            if accepted[k] > 0.0 {
                efficiency.push((*f, radiated / accepted[k]));
            }
        }
    }
    Ok(Evaluation {
        fidelity: fidelity.name,
        freqs: problem.freqs.clone(),
        z,
        s11_db,
        efficiency,
        cells,
        steps,
        dt_ps,
        seconds: started.elapsed().as_secs_f64(),
        converged,
        warnings,
    })
}

/// `COPPER_ANTENNA_PROFILE`: where the mesh's cells go. Per axis: the line
/// count, the smallest and largest cell (and where the smallest is) and the
/// cells over the board; then the
/// cells over the board's footprint and slab range against the air around it,
/// and a histogram of in-plane cell widths.
fn mesh_census(
    model: &ContinuousStructure,
    problem: &Problem,
    outline: &Ring,
    stack: &Stack,
    cell: f64,
    max_cell: f64,
    margin: f64,
) {
    let lo = |v: &mut dyn Iterator<Item = f64>| v.fold(f64::INFINITY, f64::min);
    let hi = |v: &mut dyn Iterator<Item = f64>| v.fold(f64::NEG_INFINITY, f64::max);
    let z_lo = stack.slabs.iter().map(|s| s.bottom).fold(0.0, f64::min);
    let range = [
        (lo(&mut outline.iter().map(|p| p.0)), hi(&mut outline.iter().map(|p| p.0))),
        (lo(&mut outline.iter().map(|p| p.1)), hi(&mut outline.iter().map(|p| p.1))),
        (z_lo, 0.0),
    ];
    eprintln!(
        "profile mesh: cell {cell:.3} mm, max cell {max_cell:.3} mm, air margin {margin:.1} mm, {} stitching vias at {:.2} mm pitch",
        problem.stitching.len(),
        problem.stitch_pitch
    );
    let mut inside = [0usize; 3];
    let mut histogram = [0usize; 6];
    let bins = [0.3, 0.6, 1.2, 2.5, 5.0];
    for (k, inside) in inside.iter_mut().enumerate() {
        let lines: Vec<f64> = model.grid.lines(k).to_vec();
        let widths: Vec<(f64, f64)> = lines.windows(2).map(|w| (w[1] - w[0], (w[0] + w[1]) / 2.0)).collect();
        let (min, at) = widths.iter().copied().fold((f64::INFINITY, 0.0), |a, w| if w.0 < a.0 { w } else { a });
        let max = widths.iter().map(|w| w.0).fold(0.0, f64::max);
        let (a, b) = range[k];
        *inside = widths.iter().filter(|w| w.1 > a && w.1 < b).count();
        if k < 2 {
            for w in &widths {
                histogram[bins.iter().take_while(|edge| w.0 >= **edge).count()] += 1;
            }
        }
        eprintln!(
            "profile axis {k}: {} lines, cells {min:.3} mm (at {at:.2}) to {max:.3} mm, {} cells over the board",
            lines.len(),
            *inside
        );
        if std::env::var("COPPER_ANTENNA_PROFILE").is_ok_and(|v| v.contains("lines")) {
            let small: Vec<String> = widths
                .iter()
                .zip(&lines)
                .filter(|(w, _)| w.0 < 0.45 * cell)
                .map(|(w, l)| format!("{l:.3}+{:.3}", w.0))
                .collect();
            eprintln!("profile axis {k} cells below 0.45 x fine cell: {}", small.join(" "));
        }
    }
    let n = model.grid.num_lines();
    let total = n[0].saturating_sub(1) * n[1].saturating_sub(1) * n[2].saturating_sub(1);
    let board = inside[0] * inside[1] * inside[2];
    eprintln!(
        "profile cells: {total} total, {board} over the board footprint and slabs ({:.0} %), {} elsewhere; x/y widths <0.3 {} 0.3-0.6 {} 0.6-1.2 {} 1.2-2.5 {} 2.5-5 {} >=5 {} mm; field+coefficient memory about {:.0} MB",
        100.0 * board as f64 / total.max(1) as f64,
        total - board,
        histogram[0],
        histogram[1],
        histogram[2],
        histogram[3],
        histogram[4],
        histogram[5],
        total as f64 * 72.0 / 1e6
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_covers_every_band() {
        let bands = [(2.4e9, 2.4835e9), (5.15e9, 5.85e9)];
        let freqs = sweep_frequencies(&bands);
        let (start, stop) = (freqs[0], *freqs.last().unwrap());
        for (low, high) in bands {
            assert!(start < low && high < stop, "{start}..{stop} misses {low}..{high}");
        }
        assert!(freqs.windows(2).all(|w| w[1] > w[0]));
        assert!(freqs.windows(2).all(|w| w[1] - w[0] <= 0.002 * 4.125e9 + 1.0));
        // The pulse reaches the whole sweep at its 20 dB points.
        let (f0, fc) = excitation(&freqs);
        assert!(f0 - fc <= start && stop <= f0 + fc, "{f0} ± {fc}");
        // A single narrow band keeps the historical 0.8..1.2 sweep and pulse.
        let centre = (2.4e9 + 2.4835e9) / 2.0;
        let freqs = sweep_frequencies(&[(2.4e9, 2.4835e9)]);
        assert_eq!(freqs.len(), 201);
        assert!((freqs[0] - 0.8 * centre).abs() < 1.0 && (freqs[200] - 1.2 * centre).abs() < 1.0);
        let (f0, fc) = excitation(&freqs);
        assert!((f0 - centre).abs() < 1.0 && (fc - 0.4 * centre).abs() < 1.0);
    }

    fn plan(lines: &[(f64, LineRank)], zones: &[(f64, f64, f64)]) -> AxisPlan {
        AxisPlan {
            lines: lines.to_vec(),
            zones: zones.to_vec(),
            snap: 0.25,
            lo: -20.0,
            hi: 20.0,
            max_cell: 3.0,
            ratio: 1.3,
        }
    }

    fn cells(lines: &[f64]) -> Vec<f64> {
        lines.windows(2).map(|w| w[1] - w[0]).collect()
    }

    #[test]
    fn port_lines_survive_and_lower_ranks_snap_onto_higher_ones() {
        let lines = axis_lines(&plan(
            &[
                (0.0, LineRank::Port),
                (0.1, LineRank::Port),
                (0.2, LineRank::Edge),
                (2.0, LineRank::Anchor),
                (2.1, LineRank::Edge),
            ],
            &[(-1.0, 3.0, 0.5)],
        ))
        .expect("lines");
        assert!(lines.contains(&0.0) && lines.contains(&0.1), "{lines:?}");
        assert!(!lines.contains(&0.2) && !lines.contains(&2.0) && lines.contains(&2.1), "{lines:?}");
    }

    #[test]
    fn no_cell_exceeds_the_frequency_limit_inside_the_zones() {
        // 24 GHz in er 4: a 0.416 mm limit under a zone asking for 0.75 mm.
        let mut coarse = plan(&[(0.0, LineRank::Edge), (2.0, LineRank::Edge)], &[(-1.0, 3.0, 0.75)]);
        coarse.max_cell = 0.416;
        let lines = axis_lines(&coarse).expect("lines");
        let widest = cells(&lines).into_iter().fold(0.0, f64::max);
        assert!(widest <= 0.416 + 1e-9, "{widest} mm in {lines:?}");
    }

    #[test]
    fn zones_bound_the_cells_without_leaving_slivers() {
        let lines = axis_lines(&plan(
            &[(0.0, LineRank::Edge), (0.3, LineRank::Edge), (1.3, LineRank::Edge)],
            &[(-1.0, 3.0, 0.5), (0.0, 1.3, 0.375)],
        ))
        .expect("lines");
        let inside: Vec<f64> = lines.iter().copied().filter(|v| (-1.0..=3.0).contains(v)).collect();
        for c in cells(&inside) {
            assert!((0.25 - 1e-9..=0.5 + 1e-9).contains(&c), "{inside:?}");
        }
        for c in cells(&lines) {
            assert!(c <= 3.0 + 1e-9, "{lines:?}");
        }
        assert_eq!((lines[0], *lines.last().expect("lines")), (-20.0, 20.0));
    }

    #[test]
    fn the_short_via_barrel_spans_the_fabricated_via() {
        // F.Cu, In1 (ground under the short, say) and B.Cu: the board's via
        // runs to B.Cu, so the barrel does too, stub below In1 included.
        let stack = Stack {
            copper: vec![(CopperLayer::Front, 0.0), (CopperLayer::In1, -0.2), (CopperLayer::Back, -1.6)],
            slabs: Vec::new(),
        };
        assert_eq!(via_bottom(&stack, CopperLayer::Back), Some(-1.6));
        assert_eq!(via_bottom(&stack, CopperLayer::Front), None);
    }

    #[test]
    fn subtracting_a_rectangle_keeps_the_rest_of_the_area() {
        let board: Ring = vec![(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
        let pieces = subtract(&board, [18.0, 0.0, 40.0, 6.0]);
        let total: f64 = pieces.iter().map(area).sum();
        assert!((total - (1200.0 - 22.0 * 6.0)).abs() < 1e-9, "{total}");
        // An interior slot leaves four pieces around it.
        let pieces = subtract(&board, [10.0, 10.0, 12.0, 14.0]);
        assert_eq!(pieces.len(), 4);
        let total: f64 = pieces.iter().map(area).sum();
        assert!((total - (1200.0 - 8.0)).abs() < 1e-9, "{total}");
    }
}

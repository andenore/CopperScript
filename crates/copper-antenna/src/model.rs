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

use ems::csx::mesh::{MeshBuilder, MeshOptions};
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
/// The modelled stitching via's square side, in millimetres.
const STITCH_SIZE: f64 = 0.4;

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
    pub threads: usize,
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

/// Merge edges closer than `cell / 2`, then split every gap wider than `cell`.
/// The smallest cell sets the FDTD timestep, so slivers cost far more than
/// the accuracy they add.
/// Always keeps `keep` (the port's terminals): other edges within `cell / 2`
/// of a kept line are dropped instead.
fn region_lines_keeping(mut edges: Vec<f64>, keep: &[f64], cell: f64) -> Vec<f64> {
    edges.retain(|e| keep.iter().all(|k| (e - k).abs() >= cell / 2.0));
    edges.extend_from_slice(keep);
    edges.sort_by(f64::total_cmp);
    let mut kept: Vec<f64> = Vec::new();
    for edge in edges {
        let forced = keep.contains(&edge);
        match kept.last() {
            Some(last) if edge - last < cell / 2.0 && !forced => {}
            Some(last) if edge - last < cell / 2.0 && forced && !keep.contains(last) => {
                kept.pop();
                kept.push(edge);
            }
            _ => kept.push(edge),
        }
    }
    kept.dedup();
    let mut lines = vec![kept[0]];
    for pair in kept.windows(2) {
        let n = ((pair[1] - pair[0]) / cell).ceil().max(1.0) as usize;
        lines.extend((1..=n).map(|i| pair[0] + (pair[1] - pair[0]) * i as f64 / n as f64));
    }
    lines
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
    let half = STITCH_SIZE / 2.0;
    for (x, y, top, bottom) in &problem.stitching {
        model
            .add_primitive(
                ground,
                Primitive::new(BoxShape::new([x - half, y - half, *bottom], [x + half, y + half, *top]))
                    .with_priority(10),
            )
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
    if let (Some(link), Some(short)) = (&site.short, &layout.short) {
        let (vu, vv) = short.via;
        let r = link.via_size / 2;
        let via = board_rect(site, vu - r, vv - r, vu + r, vv + r);
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

    // Mesh: our own lines over the antenna and feed, graded outward by csx.
    let cell = fidelity.cell;
    let region = [mm(site.area.min_x), mm(site.area.min_y), mm(site.area.max_x), mm(site.area.max_y)];
    let pad = 2.0 * cell;
    let (mut xs, mut ys) = (vec![region[0] - pad, region[2] + pad], vec![region[1] - pad, region[3] + pad]);
    let port_rect = [port_box.0[0], port_box.0[1], port_box.1[0], port_box.1[1]];
    for r in copper.iter().chain([&problem.slot, &region, &port_rect]) {
        xs.extend([r[0], r[2]]);
        ys.extend([r[1], r[3]]);
    }
    let (x0, x1) =
        (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    let (y0, y1) =
        (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    xs.retain(|x| (x0..=x1).contains(x));
    ys.retain(|y| (y0..=y1).contains(y));
    let z_cell = cell.max(0.15);
    let mut zs: Vec<f64> = stack.copper.iter().map(|(_, z)| *z).collect();
    zs.extend(stack.slabs.iter().flat_map(|s| [s.top, s.bottom]));
    zs.push(4.0 * cell);
    // The port's terminals must stay on distinct lines however close they
    // are, with a line between them along the port: a one-cell gap whose two
    // nodes both touch metal discretizes as a PEC short.
    let mid = |a: f64, b: f64| (a + b) / 2.0;
    let along_port = |k: usize| direction.index() == k;
    let keep = |k: usize| {
        let (a, b) = (port_box.0[k], port_box.1[k]);
        if along_port(k) { vec![a, mid(a, b), b] } else { vec![a, b] }
    };
    let (keep_x, keep_y) = (keep(0), keep(1));
    let keep_z: Vec<f64> = if port_box.0[2] == port_box.1[2] { vec![] } else { keep(2) };
    zs.extend(&keep_z);
    let fine = [
        region_lines_keeping(xs, &keep_x, cell),
        region_lines_keeping(ys, &keep_y, cell),
        region_lines_keeping(zs, &keep_z, z_cell),
    ];
    let f_high = problem.freqs.last().copied().unwrap_or(centre);
    let max_cell = (C0_MM_PER_S / f_high / stack.max_er().sqrt() / 15.0).min(3.0);
    let margin = C0_MM_PER_S / centre / 4.0 + PML_CELLS as f64 * max_cell;
    let mut builder = MeshBuilder::new(MeshOptions::new(max_cell).with_ratio(1.3).with_margin(margin).with_min_res([
        cell * 0.45,
        cell * 0.45,
        z_cell * 0.45,
    ]));
    for (dir, lines) in fine.into_iter().enumerate() {
        builder = builder.with_lines(dir, lines);
    }
    builder.apply(&mut model).map_err(err)?;
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

    let fdtd = Fdtd::new()
        .waveform({
            let (f0, fc) = excitation(&problem.freqs);
            Waveform::gaussian(f0, fc)
        })
        .boundaries(Boundaries::all(Boundary::pml(PML_CELLS)))
        .end_criteria(EndCriteria::energy_decay_db(fidelity.decay_db))
        .max_time(fidelity.max_time_ns * 1e-9)
        .threads(fidelity.threads.max(1));
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
    let results = sim.run().map_err(|e| e.to_string())?;
    let outcome = results.outcome().cloned();
    let converged = matches!(outcome.as_ref().map(|o| &o.reason), Some(StopReason::EnergyDecay { .. }));
    let steps = outcome.as_ref().map_or(0, |o| o.steps);
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

    #[test]
    fn kept_lines_survive_merging() {
        let lines = region_lines_keeping(vec![0.0, 1.0, 1.1, 3.0], &[1.0, 1.2], 0.5);
        assert!(lines.contains(&1.0) && lines.contains(&1.2), "{lines:?}");
        assert!(!lines.contains(&1.1), "{lines:?}");
    }

    #[test]
    fn region_lines_merge_slivers_and_fill_gaps() {
        let lines = region_lines_keeping(vec![0.0, 0.05, 1.0, 1.02, 2.5], &[], 0.5);
        assert_eq!(lines.first(), Some(&0.0));
        assert!(lines.windows(2).all(|p| p[1] - p[0] >= 0.25 - 1e-12 && p[1] - p[0] <= 0.5 + 1e-12), "{lines:?}");
        assert_eq!(lines.last(), Some(&2.5));
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

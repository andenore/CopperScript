//! The region model: the board's copper, dielectric and ports inside a window
//! as solver geometry on a graded mesh.
//!
//! What is modelled (see the crate documentation for the limitations):
//! dielectric slabs between the stack-up's copper layers (relative
//! permittivity, loss tangent as a conductivity at the band centre), air above
//! and below, copper as zero-thickness PEC sheets on mesh planes, vias and
//! plated holes as vertical PEC wires, lumped ports, bridges and added traces.

use std::collections::BTreeMap;

use copper_pcb::board::{CopperZone, PhysicalBoard, Placement, TrackSegment};
use copper_physical::arcs::ArcGeometry;
use copper_physical::footprint::PadKind;
use copper_physical::{BoardSide, CopperLayer, Point};
use indexmap::IndexMap;

use crate::geom::{self, Pt, nm_to_mm};
use crate::mesh::{AxisMesh, AxisPlan, Line, Rank, axis_mesh};
use crate::spec::{
    Bridge, Error, ExtraTrace, Fidelity, NetTreatment, PortKind, PortSpec, RegionSpec, Result, Terminal,
};
use crate::stack::{Stack, stack};

pub const C0_MM_PER_S: f64 = 299_792_458_000.0;
pub const PML_CELLS: usize = 8;
const COPPER_SIGMA: f64 = 5.8e7;
const DEFAULT_COPPER_THICKNESS_M: f64 = 35e-6;

/// One copper sheet on a mesh plane.
#[derive(Clone, Debug)]
pub struct Sheet {
    /// `pad`, `track`, `arc`, `polygon`, `zone`, `bridge` or `added`.
    pub kind: &'static str,
    pub net: String,
    pub net_id: u32,
    pub layer: CopperLayer,
    pub z: f64,
    /// Millimetres; a polygon with holes is a keyhole ring.
    pub ring: Vec<Pt>,
    pub treatment: NetTreatment,
    /// Thickness for a conducting sheet, metres.
    pub thickness_m: f64,
    /// Touches the window edge (a plane-like sheet the extension may continue).
    pub touches_edge: bool,
}

/// A vertical PEC wire: a via, a plated-hole barrel.
#[derive(Clone, Debug)]
pub struct Wire {
    pub kind: &'static str,
    pub net: String,
    pub net_id: u32,
    pub x: f64,
    pub y: f64,
    pub z_top: f64,
    pub z_bottom: f64,
    /// Barrel radius (drill/2): the wire itself is a line, this is for antipads and reports.
    pub radius: f64,
}

/// A port with its box in the model, millimetres.
#[derive(Clone, Debug)]
pub struct ResolvedPort {
    pub spec: PortSpec,
    /// The `+` terminal's corner and the other corner of the box.
    pub start: [f64; 3],
    pub stop: [f64; 3],
    /// 0, 1 or 2: the direction of the port.
    pub axis: usize,
    pub description: String,
}

/// What the build did, for reports.
#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub pads: usize,
    pub tracks: usize,
    pub arcs: usize,
    pub polygons: usize,
    pub zones: usize,
    pub vias: usize,
    pub plated_holes: usize,
    pub bridges: usize,
    pub added_traces: usize,
    pub antipads: usize,
    /// Real zone fills (net and layer pieces) used in place of zone outlines.
    pub zone_fills: usize,
}

/// The mesh and what its rules did.
#[derive(Clone, Debug)]
pub struct MeshReport {
    pub lines: [usize; 3],
    pub cells: usize,
    pub smallest: [f64; 3],
    pub largest: [f64; 3],
    /// Courant estimate of the time step, seconds.
    pub dt_estimate: f64,
    pub moved_edges: usize,
    pub max_move_mm: f64,
    pub slivers: Vec<String>,
    pub max_ratio: f64,
    pub ratio_violations: usize,
    pub copper_cell_mm: f64,
}

/// The expected cost of one run, from the mesh alone.
#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    pub cells: usize,
    pub dt: f64,
    /// Simulated time the run is expected to need, seconds.
    pub sim_time: f64,
    pub steps: u64,
    pub cell_updates: f64,
    pub seconds: f64,
}

/// A region of a board as a solver model.
#[derive(Clone, Debug)]
pub struct RegionModel {
    pub spec: RegionSpec,
    pub stack: Stack,
    /// Window plus margin, millimetres.
    pub region: [f64; 4],
    pub sheets: Vec<Sheet>,
    pub wires: Vec<Wire>,
    pub ports: Vec<ResolvedPort>,
    /// The PEC straps between strap ports and their pads (see [`Strap`]).
    pub straps: Vec<Strap>,
    pub axes: [AxisMesh; 3],
    pub counts: Counts,
    pub mesh: MeshReport,
    /// Short statements of what was approximated or left out.
    pub approximations: Vec<String>,
    /// Machine-readable flags (`zone_outline_used`, `planes_truncated`, ...).
    pub flags: Vec<String>,
    /// Loss conductivity reference frequency, Hz (band centre).
    pub centre_hz: f64,
    /// Domain ends including PML, millimetres.
    pub domain: [[f64; 2]; 3],
    pub net_names: Vec<String>,
}

pub(crate) struct PlacedPad {
    pub(crate) component: String,
    pub(crate) number: String,
    pub(crate) net: Option<String>,
    pub(crate) ring: Vec<Pt>,
    pub(crate) rounded: bool,
    pub(crate) layers: Vec<CopperLayer>,
    pub(crate) drill_mm: Option<f64>,
    pub(crate) min_dimension_mm: f64,
}

fn side_layer(side: BoardSide) -> CopperLayer {
    match side {
        BoardSide::Front => CopperLayer::Front,
        BoardSide::Back => CopperLayer::Back,
    }
}

/// Placements with rigid clusters moved to the poses their anchor prescribes
/// (as `copper_drc::emi::Layout::new` does).
pub(crate) fn poses(board: &PhysicalBoard) -> IndexMap<String, Placement> {
    let mut poses: IndexMap<String, Placement> =
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    for cluster in &board.rigid_clusters {
        if let Some(anchor) = poses.get(&cluster.anchor_reference)
            && let Ok(moved) = copper_pcb::hard_macros::cluster_placements(cluster, anchor, &poses)
        {
            poses.extend(moved);
        }
    }
    poses
}

pub(crate) fn placed_pads(board: &PhysicalBoard, poses: &IndexMap<String, Placement>, stack: &Stack) -> Vec<PlacedPad> {
    let nets: BTreeMap<(&str, &str), &str> = board
        .nets
        .iter()
        .flat_map(|n| n.pads.iter().map(move |p| ((p.component.as_str(), p.pad.as_str()), n.name.as_str())))
        .collect();
    let mut out = Vec::new();
    for (reference, placement) in poses {
        let Some(footprint) = board.footprints.get(&placement.footprint) else { continue };
        for pad in &footprint.pads {
            if !pad.kind.is_electrical() {
                continue;
            }
            let at = copper_pcb::transform::transformed_local_point(placement, pad.position);
            let shape = copper_pcb::transform::placed_pad_shape(at, pad, placement);
            let (ring, rounded) = geom::pad_polygon(&shape);
            let layers = if pad.kind == PadKind::Smd {
                vec![side_layer(placement.side)]
            } else {
                stack.copper.iter().map(|(l, _)| *l).collect()
            };
            let min_dim = nm_to_mm(pad.size.width_nm.min(pad.size.height_nm));
            out.push(PlacedPad {
                component: reference.clone(),
                number: pad.number.clone(),
                net: nets.get(&(reference.as_str(), pad.number.as_str())).map(|n| (*n).to_owned()),
                ring,
                rounded,
                layers,
                drill_mm: pad.drill.map(|d| nm_to_mm(d.width_nm.min(d.height_nm))),
                min_dimension_mm: min_dim,
            });
        }
    }
    out
}

/// The edge rule with local cells: the distance by which an edge is moved into the metal is
/// `fraction` of the lateral cell at the edge plus `fraction_z` of the vertical cell at the layer. The
/// lateral cell is the general copper cell or, near a refinement box, the refined one; the vertical
/// cell is that of the dielectric slabs next to the layer (see [`slab_counts`]).
#[derive(Clone, Debug)]
pub(crate) struct InsetRule {
    pub(crate) fraction: f64,
    pub(crate) fraction_z: f64,
    pub(crate) cell: f64,
    /// Refinement intervals `(lo, hi, cell)` of x and y, millimetres.
    pub(crate) zones: [Vec<(f64, f64, f64)>; 2],
    /// Dielectric slabs `(bottom, top, cells)` as the z mesh will cut them.
    pub(crate) slabs: Vec<(f64, f64, usize)>,
}

/// The growth of the cell above the top copper, where the z mesh leaves the last slab.
const AIR_GROWTH: f64 = 1.3;

impl InsetRule {
    /// The mesher's target cell along `axis` at `v`: the general cell, or the smallest refinement cell grown by 0.3 per mm of distance.
    pub(crate) fn cell_at(&self, axis: usize, v: f64) -> f64 {
        self.zones[axis].iter().map(|(lo, hi, c)| c + 0.3 * (lo - v).max(v - hi).max(0.0)).fold(self.cell, f64::min)
    }

    /// The vertical cell at the layer plane `z`: the mean of the cells above and below it, where a
    /// side without dielectric (air above the top layer) counts as the other side's cell grown once.
    pub(crate) fn dz_at(&self, z: f64) -> f64 {
        let below = self.slabs.iter().find(|(_, top, _)| (top - z).abs() < 1e-6).map(|(b, t, n)| (t - b) / *n as f64);
        let above =
            self.slabs.iter().find(|(bottom, _, _)| (bottom - z).abs() < 1e-6).map(|(b, t, n)| (t - b) / *n as f64);
        match (below, above) {
            (Some(b), Some(a)) => 0.5 * (a + b),
            (Some(c), None) | (None, Some(c)) => 0.5 * (1.0 + AIR_GROWTH) * c,
            (None, None) => self.cell,
        }
    }

    /// The offset of an edge with midpoint `mid` and inward normal `inward` on the plane `z`.
    pub(crate) fn delta(&self, mid: Pt, inward: Pt, z: f64) -> f64 {
        let (wx, wy) = (inward.0.abs(), inward.1.abs());
        let lateral = (wx * self.cell_at(0, mid.0) + wy * self.cell_at(1, mid.1)) / (wx + wy).max(1e-12);
        self.fraction * lateral + self.fraction_z * self.dz_at(z)
    }

    pub(crate) fn is_off(&self) -> bool {
        self.fraction <= 0.0 && self.fraction_z <= 0.0
    }

    pub(crate) fn ring(&self, ring: &[Pt], fixed: [f64; 4], z: f64) -> Option<Vec<Pt>> {
        if self.is_off() {
            return Some(ring.to_vec());
        }
        geom::inset_ring_by(ring, &|m, n| self.delta(m, n, z), fixed)
    }
}

/// The cells of each dielectric slab `(bottom, top, n)`: at least two (three at fine fidelity) and at least
/// one per 1.25 times the smallest copper-region cell, not thinner than 0.45 of the slab.
pub(crate) fn slab_counts(
    stack: &Stack,
    fidelity: Fidelity,
    cell: f64,
    zones: &[Vec<(f64, f64, f64)>; 2],
) -> Vec<(f64, f64, usize)> {
    let n_min = match fidelity {
        Fidelity::Coarse => 2usize,
        Fidelity::Fine => 3usize,
    };
    let zones_cell = zones.iter().flatten().map(|z| z.2).fold(cell, f64::min);
    let min_cell = (0.4 * cell).max(0.04);
    let thinnest = stack.thinnest();
    let min_cell_z = if thinnest.is_finite() { min_cell.min(0.45 * thinnest) } else { min_cell };
    stack
        .slabs
        .iter()
        .map(|s| {
            let t = s.top - s.bottom;
            let n = (t / (1.25 * zones_cell)).ceil().max(n_min as f64) as usize;
            let n = n.min(((t / min_cell_z).floor() as usize).max(1));
            (s.bottom, s.top, n)
        })
        .collect()
}

pub(crate) fn mm_point(p: Point) -> Pt {
    (nm_to_mm(p.x_nm), nm_to_mm(p.y_nm))
}

fn octagon(centre: Pt, r: f64) -> Vec<Pt> {
    // A regular octagon whose flats stand at distance r.
    let c = r / (std::f64::consts::PI / 8.0).cos();
    (0..8)
        .map(|k| {
            let a = std::f64::consts::PI * (f64::from(k) / 4.0 + 0.125);
            (centre.0 + c * a.cos(), centre.1 + c * a.sin())
        })
        .collect()
}

pub(crate) fn overlaps(ring: &[Pt], r: [f64; 4]) -> bool {
    geom::bbox(ring).is_some_and(|b| b[0] <= r[2] && b[2] >= r[0] && b[1] <= r[3] && b[3] >= r[1])
}

struct Builder<'a> {
    spec: &'a RegionSpec,
    stack: Stack,
    region: [f64; 4],
    net_ids: BTreeMap<String, u32>,
    sheets: Vec<Sheet>,
    wires: Vec<Wire>,
    counts: Counts,
    flags: Vec<String>,
    notes: Vec<String>,
    /// The edge rule.
    inset: InsetRule,
}

impl Builder<'_> {
    fn net_id(&mut self, net: &str) -> u32 {
        let next = self.net_ids.len() as u32;
        *self.net_ids.entry(net.to_owned()).or_insert(next)
    }

    fn flag(&mut self, flag: &str) {
        if !self.flags.iter().any(|f| f == flag) {
            self.flags.push(flag.to_owned());
        }
    }

    /// The edge rule: move the ring's edges into the metal; a ring too small
    /// for it keeps its drawn edges.
    fn inset_ring(&mut self, ring: Vec<Pt>, z: f64) -> Vec<Pt> {
        if self.inset.is_off() {
            return ring;
        }
        match self.inset.ring(&ring, self.region, z) {
            Some(r) => r,
            None => {
                self.flag("edge_inset_skipped_small_feature");
                ring
            }
        }
    }

    fn thickness_m(&self, board: &PhysicalBoard, layer: CopperLayer) -> f64 {
        board
            .stackup
            .physical_layers
            .iter()
            .find(|l| l.copper_layer == Some(layer))
            .map_or(DEFAULT_COPPER_THICKNESS_M, |l| l.thickness_nm as f64 * 1e-9)
    }

    fn push_sheet(&mut self, board: &PhysicalBoard, kind: &'static str, net: &str, layer: CopperLayer, ring: Vec<Pt>) {
        let Some(z) = self.stack.z(layer) else {
            self.flag("copper_on_layer_outside_stackup_dropped");
            return;
        };
        let ring = geom::clip_rect(&ring, self.region);
        if ring.len() < 3 || geom::area(&ring) < 1e-9 {
            return;
        }
        let ring = self.inset_ring(ring, z);
        if ring.len() < 3 {
            return;
        }
        let net_id = self.net_id(net);
        let treatment = self.spec.treatment(net);
        let touches_edge = ring.iter().any(|p| {
            (p.0 - self.region[0]).abs() < 1e-9
                || (p.0 - self.region[2]).abs() < 1e-9
                || (p.1 - self.region[1]).abs() < 1e-9
                || (p.1 - self.region[3]).abs() < 1e-9
        });
        let thickness_m = self.thickness_m(board, layer);
        self.sheets.push(Sheet {
            kind,
            net: net.to_owned(),
            net_id,
            layer,
            z,
            ring,
            treatment,
            thickness_m,
            touches_edge,
        });
    }

    fn track(&mut self, board: &PhysicalBoard, t: &TrackSegment) {
        if !self.spec.includes(&t.net) {
            return;
        }
        let ring = geom::track_polygon(mm_point(t.start), mm_point(t.end), nm_to_mm(t.width_nm), true);
        if !geom::bbox(&ring).is_some_and(|_| overlaps(&ring, self.region)) {
            return;
        }
        self.counts.tracks += 1;
        self.push_sheet(board, "track", &t.net, t.layer, ring);
    }

    fn arc(&mut self, board: &PhysicalBoard, a: &copper_pcb::board::TrackArc) {
        if !self.spec.includes(&a.net) {
            return;
        }
        let Ok(geometry) = ArcGeometry::from_points(a.start, a.mid, a.end) else {
            self.flag("arc_skipped_degenerate");
            return;
        };
        let chords = geometry.chords(a.start, a.end, 20_000);
        self.counts.arcs += 1;
        for pair in chords.windows(2) {
            let ring = geom::track_polygon(mm_point(pair[0]), mm_point(pair[1]), nm_to_mm(a.width_nm), true);
            if overlaps(&ring, self.region) {
                self.push_sheet(board, "arc", &a.net, a.layer, ring);
            }
        }
        self.flag("arcs_as_chord_chains");
    }

    fn zone(
        &mut self,
        board: &PhysicalBoard,
        zone: &CopperZone,
        barrels: &[Barrel],
        foreign: &[(CopperLayer, String, Vec<Pt>)],
    ) {
        if !self.spec.includes(&zone.net) || !self.spec.include_zones {
            return;
        }
        let outer: Vec<Pt> = zone.outer.iter().map(|p| mm_point(*p)).collect();
        if !overlaps(&outer, self.region) {
            return;
        }
        self.counts.zones += 1;
        for layer in &zone.layers {
            // Where the real fill of this net on this layer is known it replaces the outline (`zone_fill`).
            if self.spec.zone_fills.as_ref().is_some_and(|f| f.covers(&zone.net, *layer)) {
                continue;
            }
            self.flag("zone_outline_used");
            let Some(z) = self.stack.z(*layer) else {
                self.flag("copper_on_layer_outside_stackup_dropped");
                continue;
            };
            let mut holes: Vec<Vec<Pt>> = zone.holes.iter().map(|h| h.iter().map(|p| mm_point(*p)).collect()).collect();
            let clearance = nm_to_mm(zone.clearance_nm.unwrap_or(board.rules.minimum_clearance_nm));
            let mut antipads = 0;
            for b in barrels {
                if b.net != zone.net && b.z_bottom - 1e-9 <= z && z <= b.z_top + 1e-9 {
                    holes.push(octagon((b.x, b.y), b.radius + clearance));
                    antipads += 1;
                }
            }
            self.counts.antipads += antipads;
            // Foreign copper on this layer (pads, tracks, bridges, legs of other nets) is cut out of the pour with
            // the zone clearance (polygon difference, `copper_cam::poly`): the outline minus what must be
            // absent, not the CAM fill (thermal reliefs, minimum widths and islands are not applied).
            let others: Vec<&Vec<Pt>> =
                foreign.iter().filter(|(l, n, _)| l == layer && *n != zone.net).map(|(_, _, r)| r).collect();
            if !others.is_empty() {
                self.flag("zone_cut_around_foreign_copper");
                for ring in self.cut_zone(&outer, &holes, &others, clearance) {
                    self.push_zone_sheet(board, &zone.net, *layer, z, ring);
                }
                continue;
            }
            let clipped_outer = geom::clip_rect(&outer, self.region);
            if clipped_outer.len() < 3 {
                continue;
            }
            let clipped_holes: Vec<Vec<Pt>> = holes
                .iter()
                .map(|h| geom::clip_rect(h, self.region))
                .filter(|h| h.len() >= 3 && geom::area(h) > 1e-9)
                .collect();
            let ring = match geom::keyhole(&clipped_outer, &clipped_holes) {
                Some(r) => r,
                None if clipped_holes.is_empty() => clipped_outer.clone(),
                None => {
                    self.flag("zone_holes_dropped");
                    clipped_outer.clone()
                }
            };
            self.push_zone_sheet(board, &zone.net, *layer, z, ring);
        }
    }

    /// The real fill of a net on a layer, clipped to the window and cut with the zone clearance around the copper the
    /// model adds to the board (`others`: bridges and added traces of other nets, which the fill does not know): one
    /// keyhole ring (millimetres) per piece that remains, as a zone sheet.
    fn zone_fill(&mut self, board: &PhysicalBoard, fill: &crate::fill::RealFill, others: &[&Vec<Pt>]) {
        use copper_cam::poly;
        if !self.spec.includes(&fill.net) || !self.spec.include_zones {
            return;
        }
        let Some(z) = self.stack.z(fill.layer) else {
            self.flag("copper_on_layer_outside_stackup_dropped");
            return;
        };
        let r = self.region;
        let window = poly::polygon(&[
            (r[0] * 1e6, r[1] * 1e6),
            (r[2] * 1e6, r[1] * 1e6),
            (r[2] * 1e6, r[3] * 1e6),
            (r[0] * 1e6, r[3] * 1e6),
        ]);
        let mut subject = poly::intersection(&fill.shapes, &window);
        if subject.is_empty() {
            return;
        }
        let clearance_nm = board
            .zones
            .iter()
            .find(|zone| zone.net == fill.net && zone.layers.contains(&fill.layer))
            .and_then(|zone| zone.clearance_nm)
            .unwrap_or(board.rules.minimum_clearance_nm);
        if !others.is_empty() {
            let mut copper: poly::Shapes = Vec::new();
            for ring in others {
                let nm: Vec<(f64, f64)> = ring.iter().map(|p| (p.0 * 1e6, p.1 * 1e6)).collect();
                copper.extend(poly::polygon(&nm));
            }
            let clip = poly::offset(&poly::union(&copper), clearance_nm, false, 5_000.0);
            subject = poly::difference(&subject, &clip);
            self.flag("zone_cut_around_foreign_copper");
        }
        self.flag("zone_fill_used");
        for shape in subject.iter().filter(|shape| poly::shape_area(shape) > 1e6) {
            let ring: Vec<Pt> = poly::fracture(shape).iter().map(|p| (p.x as f64 * 1e-6, p.y as f64 * 1e-6)).collect();
            self.counts.zone_fills += 1;
            self.push_zone_sheet(board, &fill.net, fill.layer, z, ring);
        }
    }

    /// The zone's outline minus its holes and the clearance around `others`, clipped to the window: one
    /// keyhole ring (millimetres) per piece that remains.
    fn cut_zone(&self, outer: &[Pt], holes: &[Vec<Pt>], others: &[&Vec<Pt>], clearance: f64) -> Vec<Vec<Pt>> {
        use copper_cam::poly;
        let nm = |ring: &[Pt]| -> Vec<(f64, f64)> { ring.iter().map(|p| (p.0 * 1e6, p.1 * 1e6)).collect() };
        let window = [
            (self.region[0], self.region[1]),
            (self.region[2], self.region[1]),
            (self.region[2], self.region[3]),
            (self.region[0], self.region[3]),
        ];
        let subject = poly::intersection(&poly::polygon(&nm(outer)), &poly::polygon(&nm(&window)));
        let mut clip: poly::Shapes = Vec::new();
        for h in holes {
            clip.extend(poly::polygon(&nm(h)));
        }
        let mut copper: poly::Shapes = Vec::new();
        for r in others {
            copper.extend(poly::polygon(&nm(r)));
        }
        clip.extend(poly::offset(&poly::union(&copper), (clearance * 1e6).round() as i64, false, 5_000.0));
        poly::difference(&subject, &clip)
            .iter()
            .filter(|shape| poly::shape_area(shape) > 1e6)
            .map(|shape| poly::fracture(shape).iter().map(|p| (p.x as f64 * 1e-6, p.y as f64 * 1e-6)).collect())
            .collect()
    }

    fn push_zone_sheet(&mut self, board: &PhysicalBoard, net: &str, layer: CopperLayer, z: f64, ring: Vec<Pt>) {
        let net_id = self.net_id(net);
        let treatment = self.spec.treatment(net);
        let touches_edge = ring.iter().any(|p| {
            (p.0 - self.region[0]).abs() < 1e-9
                || (p.0 - self.region[2]).abs() < 1e-9
                || (p.1 - self.region[1]).abs() < 1e-9
                || (p.1 - self.region[3]).abs() < 1e-9
        });
        let thickness_m = self.thickness_m(board, layer);
        self.sheets.push(Sheet {
            kind: "zone",
            net: net.to_owned(),
            net_id,
            layer,
            z,
            ring,
            treatment,
            thickness_m,
            touches_edge,
        });
    }
}

/// A barrel (via or plated hole) that cuts through foreign planes.
struct Barrel {
    net: String,
    x: f64,
    y: f64,
    radius: f64,
    z_top: f64,
    z_bottom: f64,
}

/// The layer, outline (moved inward by the edge rule) and net of a terminal.
fn terminal_info(
    t: &Terminal,
    pads: &[PlacedPad],
    stack: &Stack,
    inset: Option<&InsetRule>,
    region: [f64; 4],
) -> Result<(CopperLayer, Vec<Pt>, Option<String>)> {
    let shrink = |ring: Vec<Pt>, layer: CopperLayer| {
        let z = stack.z(layer).unwrap_or(0.0);
        inset.and_then(|rule| rule.ring(&ring, region, z)).unwrap_or(ring)
    };
    match t {
        Terminal::Pad { component, pad } => {
            let found = pads
                .iter()
                .find(|p| &p.component == component && &p.number == pad)
                .ok_or_else(|| Error::Invalid(format!("pad {component}.{pad} is not placed on the board")))?;
            let layer =
                *found.layers.first().ok_or_else(|| Error::Invalid(format!("pad {component}.{pad} has no layer")))?;
            Ok((layer, shrink(found.ring.clone(), layer), found.net.clone()))
        }
        Terminal::Rect { layer, rect } => {
            if stack.z(*layer).is_none() {
                return Err(Error::Invalid(format!("terminal layer {} is not in the stack-up", layer.value())));
            }
            let r = [nm_to_mm(rect[0]), nm_to_mm(rect[1]), nm_to_mm(rect[2]), nm_to_mm(rect[3])];
            Ok((*layer, shrink(vec![(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])], *layer), None))
        }
    }
}

/// A short PEC strap that joins a port that does not face its pads to them.
#[derive(Clone, Debug)]
pub struct Strap {
    pub net: String,
    pub layer: CopperLayer,
    pub z: f64,
    /// The rectangle, millimetres (already moved in by the edge rule across its width).
    pub ring: Vec<Pt>,
    /// The centre line, millimetres: for the inductance estimate.
    pub from: Pt,
    pub to: Pt,
    pub width: f64,
    /// The strap runs in the air above the copper layer (an air bridge over foreign copper such as a
    /// package's exposed pad), joined to the pads by vertical posts.
    pub in_air: bool,
}

/// How far above the top copper an air-bridge strap runs, millimetres: a lead frame inside a package.
pub const AIR_BRIDGE_HEIGHT_MM: f64 = 0.3;

/// A vertical post of an air bridge: `(x, y, z bottom, z top)` millimetres and the net.
#[derive(Clone, Debug)]
pub struct Post {
    pub net: String,
    pub at: Pt,
    pub z_bottom: f64,
    pub z_top: f64,
}

/// Whether `ring` covers more than 1 % of the rectangle `r` (a shared edge or a sliver of a slanted
/// trace passing close does not count).
fn covers(ring: &[Pt], r: [f64; 4]) -> bool {
    let clipped = geom::clip_rect(ring, r);
    clipped.len() >= 3 && geom::area(&clipped) > 0.01 * (r[2] - r[0]) * (r[3] - r[1])
}

/// The terminals of a strap port, as `resolve_port` found them.
struct StrapEnds<'a> {
    spec: &'a PortSpec,
    from: &'a Terminal,
    to: &'a Terminal,
    ring_from: &'a [Pt],
    ring_to: &'a [Pt],
    net_from: String,
    net_to: String,
    layer: CopperLayer,
    z: f64,
    obstacles: &'a [(CopperLayer, &'a str, &'a [Pt])],
}

/// A gap port that joins pads which do not face each other across free space: the port sits at the
/// middle of a PEC strap from each pad's nearest edge. Facing pads whose gap is crossed by other
/// copper get a straight strap; pads that overlap neither in x nor in y get an L.
fn strap_port(e: StrapEnds, inset: &InsetRule) -> Result<(ResolvedPort, Vec<Strap>, Vec<Post>)> {
    let StrapEnds { spec, from, to, ring_from, ring_to, net_from, net_to, layer, z, obstacles } = e;
    let (a, b) = (geom::bbox(ring_from).expect("ring"), geom::bbox(ring_to).expect("ring"));
    let eps = 1e-9;
    let x_over = (a[0].max(b[0]), a[2].min(b[2]));
    let y_over = (a[1].max(b[1]), a[3].min(b[3]));
    if x_over.1 > x_over.0 + eps && y_over.1 > y_over.0 + eps {
        return Err(Error::NotSupported(format!(
            "port {}: the terminals {} and {} overlap; a port needs separate pads",
            spec.name,
            from.label(),
            to.label()
        )));
    }
    let cell = inset.cell_at(0, (a[0] + a[2]) / 2.0).min(inset.cell_at(1, (a[1] + a[3]) / 2.0));
    let gap_len = (2.0 * cell).max(0.4);
    // Strap width: the narrowest pad dimension, 0.2 to 1 mm (the closed form's bar width).
    let dims = [a[2] - a[0], a[3] - a[1], b[2] - b[0], b[3] - b[1]];
    let narrowest = dims.iter().copied().fold(f64::INFINITY, f64::min);
    let width = narrowest.clamp(0.2, 1.0);
    let delta = inset.fraction * cell + inset.fraction_z * inset.dz_at(z);
    let into = cell.min(0.5 * narrowest);
    // The centre line from the pad A to the pad B: axis-aligned legs.
    let b_right = b[0] >= a[2] - eps;
    let b_left = a[0] >= b[2] - eps;
    let b_above = b[1] >= a[3] - eps;
    let b_below = a[1] >= b[3] - eps;
    let mut path: Vec<Pt> = Vec::new();
    if (b_right || b_left) && (b_above || b_below) {
        // Diagonal: horizontal out of A along its edge nearest B, vertical into B at its edge nearest A.
        let yh = if b_above { a[3] - width / 2.0 } else { a[1] + width / 2.0 };
        let xv = if b_right { b[0] + width / 2.0 } else { b[2] - width / 2.0 };
        let x_start = if b_right { a[2] - into } else { a[0] + into };
        let y_end = if b_above { b[1] + into } else { b[3] - into };
        path.extend([(x_start, yh), (xv, yh), (xv, y_end)]);
    } else if b_right || b_left {
        let yc = (y_over.0 + y_over.1) / 2.0;
        let (x_start, x_end) = if b_right { (a[2] - into, b[0] + into) } else { (a[0] + into, b[2] - into) };
        path.extend([(x_start, yc), (x_end, yc)]);
    } else {
        let xc = (x_over.0 + x_over.1) / 2.0;
        let (y_start, y_end) = if b_above { (a[3] - into, b[1] + into) } else { (a[1] + into, b[3] - into) };
        path.extend([(xc, y_start), (xc, y_end)]);
    }
    // The port sits in the middle of the longest leg.
    let leg_len = |i: usize| (path[i + 1].0 - path[i].0).hypot(path[i + 1].1 - path[i].1);
    let k = (0..path.len() - 1).max_by(|&i, &j| leg_len(i).total_cmp(&leg_len(j))).expect("a leg");
    let lk = leg_len(k);
    if lk < 2.0 * gap_len {
        return Err(Error::NotSupported(format!(
            "port {}: the pads {} and {} are only {lk:.2} mm apart along their strap, too close for a strap port",
            spec.name,
            from.label(),
            to.label()
        )));
    }
    let d = ((path[k + 1].0 - path[k].0) / lk, (path[k + 1].1 - path[k].1) / lk);
    // Foreign copper under the strap (the exposed pad of a package, a neighbouring pad) would be
    // shorted to the strap and would short the port: run the strap in the air instead, on posts.
    let half = width / 2.0;
    let crosses_foreign = path.windows(2).any(|seg| {
        let (lo_x, hi_x) = (seg[0].0.min(seg[1].0) - half, seg[0].0.max(seg[1].0) + half);
        let (lo_y, hi_y) = (seg[0].1.min(seg[1].1) - half, seg[0].1.max(seg[1].1) + half);
        obstacles
            .iter()
            .any(|(l, n, o)| *l == layer && *n != net_from && *n != net_to && covers(o, [lo_x, lo_y, hi_x, hi_y]))
    });
    let in_air = crosses_foreign;
    let z_strap = if in_air { z + AIR_BRIDGE_HEIGHT_MM } else { z };
    let centre = ((path[k].0 + path[k + 1].0) / 2.0, (path[k].1 + path[k + 1].1) / 2.0);
    let p_start = (centre.0 - d.0 * gap_len / 2.0, centre.1 - d.1 * gap_len / 2.0);
    let p_stop = (centre.0 + d.0 * gap_len / 2.0, centre.1 + d.1 * gap_len / 2.0);
    let axis = if d.0.abs() > 0.5 { 0 } else { 1 };
    // Strap rectangles: path[0..=k] then p_start (net A); p_stop then path[k+1..] (net B).
    let mut first: Vec<Pt> = path[..=k].to_vec();
    first.push(p_start);
    let mut second: Vec<Pt> = vec![p_stop];
    second.extend_from_slice(&path[k + 1..]);
    let w_eff = (width - 2.0 * delta).max(0.05);
    let rect = |p: Pt, q: Pt, grow_p: bool, grow_q: bool| -> Vec<Pt> {
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = dx.hypot(dy).max(1e-12);
        let (ux, uy) = (dx / len, dy / len);
        let h = w_eff / 2.0;
        let p = if grow_p { (p.0 - ux * h, p.1 - uy * h) } else { p };
        let q = if grow_q { (q.0 + ux * h, q.1 + uy * h) } else { q };
        let (nx, ny) = (-uy * h, ux * h);
        vec![(p.0 + nx, p.1 + ny), (q.0 + nx, q.1 + ny), (q.0 - nx, q.1 - ny), (p.0 - nx, p.1 - ny)]
    };
    let (first_net, second_net) = (net_from.clone(), net_to.clone());
    let mut straps = Vec::new();
    for (pts, net) in [(&first, net_from), (&second, net_to)] {
        let n = pts.len();
        for i in 0..n - 1 {
            // A leg grows by half a width at a bend so that the corner is filled; the ends stay put.
            straps.push(Strap {
                net: net.clone(),
                layer,
                z: z_strap,
                ring: rect(pts[i], pts[i + 1], i > 0, i + 2 < n),
                from: pts[i],
                to: pts[i + 1],
                width,
                in_air,
            });
        }
    }
    let (start, stop) = if axis == 0 {
        ([p_start.0, centre.1 - w_eff / 2.0, z_strap], [p_stop.0, centre.1 + w_eff / 2.0, z_strap])
    } else {
        ([centre.0 - w_eff / 2.0, p_start.1, z_strap], [centre.0 + w_eff / 2.0, p_stop.1, z_strap])
    };
    let posts = if in_air {
        vec![
            Post { net: first_net.clone(), at: path[0], z_bottom: z, z_top: z_strap },
            Post { net: second_net.clone(), at: path[path.len() - 1], z_bottom: z, z_top: z_strap },
        ]
    } else {
        Vec::new()
    };
    let strap_len: f64 = straps.iter().map(|s| (s.to.0 - s.from.0).hypot(s.to.1 - s.from.1)).sum();
    let how = if in_air {
        format!("an air bridge {AIR_BRIDGE_HEIGHT_MM} mm above the layer (foreign copper lies between the pads)")
    } else {
        "the pads' nearest edges".to_owned()
    };
    let description = format!(
        "{} Ohm lumped port {} on {}: {} to {}, a {gap_len:.3} mm gap along {} in the middle of {strap_len:.2} mm of {width:.2} mm wide strap along {how} (strap port)",
        spec.z_ref,
        spec.name,
        layer.value(),
        from.label(),
        to.label(),
        ["x", "y"][axis]
    );
    Ok((ResolvedPort { spec: spec.clone(), start, stop, axis, description }, straps, posts))
}

fn resolve_port(
    spec: &PortSpec,
    pads: &[PlacedPad],
    stack: &Stack,
    inset: &InsetRule,
    region: [f64; 4],
    obstacles: &[(CopperLayer, &str, &[Pt])],
) -> Result<(ResolvedPort, Vec<Strap>, Vec<Post>)> {
    match &spec.kind {
        PortKind::Gap { from, to } => {
            let (la, ra, na) = terminal_info(from, pads, stack, Some(inset), region)?;
            let (lb, rb, nb) = terminal_info(to, pads, stack, Some(inset), region)?;
            // A rectangle terminal has no net of its own: it takes that of the copper under its centre.
            let net_under = |ring: &[Pt], layer: CopperLayer| -> Option<String> {
                let bb = geom::bbox(ring)?;
                let centre = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0);
                obstacles
                    .iter()
                    .find(|(l, _, o)| *l == layer && geom::contains(o, centre))
                    .map(|(_, n, _)| (*n).to_owned())
            };
            let na = na.or_else(|| net_under(&ra, la));
            let nb = nb.or_else(|| net_under(&rb, lb));
            if la != lb {
                return Err(Error::NotSupported(format!(
                    "port {}: its terminals are on different layers ({} and {}); use a plane port",
                    spec.name,
                    la.value(),
                    lb.value()
                )));
            }
            let (a, b) = (geom::bbox(&ra).expect("ring"), geom::bbox(&rb).expect("ring"));
            let z = stack.z(la).expect("layer in stack");
            let eps = 1e-9;
            let x_gap = a[2] < b[0] - eps || b[2] < a[0] - eps;
            let y_gap = a[3] < b[1] - eps || b[3] < a[1] - eps;
            let y_over = (a[1].max(b[1]), a[3].min(b[3]));
            let x_over = (a[0].max(b[0]), a[2].min(b[2]));
            let describe = |axis: &str, gap: f64, width: f64| {
                format!(
                    "{} Ohm lumped port {} on {}: {} to {}, {gap:.3} mm gap along {axis}, {width:.3} mm wide",
                    spec.z_ref,
                    spec.name,
                    la.value(),
                    from.label(),
                    to.label()
                )
            };
            // A direct port needs the gap free of other copper on the layer.
            // Copper of the terminals' own nets that reaches into the gap (the square end of a leg, a bridge)
            // does not block a port; copper of any other net does.
            let (net_a, net_b) = (na.clone().unwrap_or_default(), nb.clone().unwrap_or_default());
            let free =
                |r: [f64; 4]| !obstacles.iter().any(|(l, n, o)| *l == la && *n != net_a && *n != net_b && covers(o, r));
            let strap = || {
                strap_port(
                    StrapEnds {
                        spec,
                        from,
                        to,
                        ring_from: &ra,
                        ring_to: &rb,
                        net_from: na.clone().unwrap_or_default(),
                        net_to: nb.clone().unwrap_or_default(),
                        layer: la,
                        z,
                        obstacles,
                    },
                    inset,
                )
            };
            if spec.strap {
                return strap();
            }
            if x_gap && y_over.1 > y_over.0 + eps {
                let (from_left, lo, hi) = if a[2] <= b[0] { (true, a[2], b[0]) } else { (false, b[2], a[0]) };
                if !free([lo, y_over.0, hi, y_over.1]) {
                    return strap();
                }
                let (start, stop) = if from_left {
                    ([lo, y_over.0, z], [hi, y_over.1, z])
                } else {
                    ([hi, y_over.0, z], [lo, y_over.1, z])
                };
                let description = describe("x", hi - lo, y_over.1 - y_over.0);
                Ok((ResolvedPort { spec: spec.clone(), start, stop, axis: 0, description }, Vec::new(), Vec::new()))
            } else if y_gap && x_over.1 > x_over.0 + eps {
                let (from_low, lo, hi) = if a[3] <= b[1] { (true, a[3], b[1]) } else { (false, b[3], a[1]) };
                if !free([x_over.0, lo, x_over.1, hi]) {
                    return strap();
                }
                let (start, stop) = if from_low {
                    ([x_over.0, lo, z], [x_over.1, hi, z])
                } else {
                    ([x_over.0, hi, z], [x_over.1, lo, z])
                };
                let description = describe("y", hi - lo, x_over.1 - x_over.0);
                Ok((ResolvedPort { spec: spec.clone(), start, stop, axis: 1, description }, Vec::new(), Vec::new()))
            } else {
                strap()
            }
        }
        PortKind::ToPlane { top, plane } => {
            let (layer, ring, _) = terminal_info(top, pads, stack, Some(inset), region)?;
            let z_top = stack.z(layer).expect("layer in stack");
            let z_plane = stack.z(*plane).ok_or_else(|| {
                Error::Invalid(format!("port {}: plane layer {} is not in the stack-up", spec.name, plane.value()))
            })?;
            if z_plane >= z_top {
                return Err(Error::Invalid(format!(
                    "port {}: the plane {} is not below the terminal's layer {}",
                    spec.name,
                    plane.value(),
                    layer.value()
                )));
            }
            let b = geom::bbox(&ring).expect("ring");
            Ok((
                ResolvedPort {
                    spec: spec.clone(),
                    start: [b[0], b[1], z_top],
                    stop: [b[2], b[3], z_plane],
                    axis: 2,
                    description: format!(
                        "{} Ohm lumped port {} from {} on {} down to {} ({:.3} mm), {:.3} x {:.3} mm",
                        spec.z_ref,
                        spec.name,
                        top.label(),
                        layer.value(),
                        plane.value(),
                        z_top - z_plane,
                        b[2] - b[0],
                        b[3] - b[1]
                    ),
                },
                Vec::new(),
                Vec::new(),
            ))
        }
    }
}

impl RegionModel {
    /// Translate the part of `board` inside `spec`'s window.
    pub fn build(board: &PhysicalBoard, spec: &RegionSpec) -> Result<RegionModel> {
        let stack = stack(board)?;
        let r = spec.region_nm();
        if r[2] <= r[0] || r[3] <= r[1] {
            return Err(Error::Invalid("the window is empty".to_owned()));
        }
        if !(spec.band_hz.0 > 0.0 && spec.band_hz.1 > spec.band_hz.0) {
            return Err(Error::Invalid("the frequency band must be positive and increasing".to_owned()));
        }
        if spec.ports.is_empty() {
            return Err(Error::Invalid("a model needs at least one port".to_owned()));
        }
        let region = [nm_to_mm(r[0]), nm_to_mm(r[1]), nm_to_mm(r[2]), nm_to_mm(r[3])];
        let poses = poses(board);
        let pads = placed_pads(board, &poses, &stack);
        // Copper sources of the board: its own, and the copper of hard macros not yet materialized.
        let mut tracks = board.tracks.clone();
        let arcs = board.arcs.clone();
        let mut vias = board.vias.clone();
        let mut polygons = board.polygons.clone();
        let mut zones = board.zones.clone();
        for hard in &board.hard_macros {
            let placed = board
                .rigid_clusters
                .iter()
                .find(|c| c.name == hard.cluster)
                .is_some_and(|cluster| poses.contains_key(&cluster.anchor_reference));
            if board.materialized_macros.contains(&hard.cluster) || !placed {
                continue;
            }
            use copper_pcb::hard_macros::{resolved_macro_geometry, resolved_macro_polygons, resolved_macro_zones};
            if let Ok(geometry) = resolved_macro_geometry(board, hard, &poses) {
                tracks.extend(geometry.tracks);
                vias.extend(geometry.vias);
            }
            if let Ok(p) = resolved_macro_polygons(board, hard, &poses) {
                polygons.extend(p);
            }
            if let Ok(z) = resolved_macro_zones(board, hard, &poses) {
                zones.extend(z);
            }
        }

        // Terminal pads always carry copper, whatever the net filter says.
        let terminal_pads: Vec<(String, String)> = spec
            .ports
            .iter()
            .flat_map(|p| match &p.kind {
                PortKind::Gap { from, to } => vec![from.clone(), to.clone()],
                PortKind::ToPlane { top, .. } => vec![top.clone()],
            })
            .chain(spec.bridges.iter().flat_map(|br| [br.a.clone(), br.b.clone()]))
            .filter_map(|t| match t {
                Terminal::Pad { component, pad } => Some((component, pad)),
                Terminal::Rect { .. } => None,
            })
            .collect();

        // The copper cell follows the narrowest feature that will be modelled.
        let mut narrowest = f64::INFINITY;
        for t in tracks.iter().filter(|t| spec.includes(&t.net)) {
            if overlaps(&geom::track_polygon(mm_point(t.start), mm_point(t.end), nm_to_mm(t.width_nm), true), region) {
                narrowest = narrowest.min(nm_to_mm(t.width_nm));
            }
        }
        for a in arcs.iter().filter(|a| spec.includes(&a.net)) {
            if overlaps(&[mm_point(a.start), mm_point(a.mid), mm_point(a.end)], region) {
                narrowest = narrowest.min(nm_to_mm(a.width_nm));
            }
        }
        for pad in &pads {
            let wanted = pad.net.as_ref().is_some_and(|n| spec.includes(n))
                || terminal_pads.iter().any(|(c, n)| *c == pad.component && *n == pad.number);
            if wanted && spec.include_pads && overlaps(&pad.ring, region) {
                narrowest = narrowest.min(pad.min_dimension_mm);
            }
        }
        for extra in &spec.extra_traces {
            narrowest = narrowest.min(nm_to_mm(extra.width_nm));
        }
        let base_cell = copper_cell(spec, narrowest);
        // A coarser mesh (refine_scale above 1) coarsens the general cell too; a finer one only the zones.
        let cell = base_cell * spec.refine_scale.max(1.0);
        let inset_zones = refine_zones(spec, base_cell, cell);
        let inset = InsetRule {
            fraction: spec.edge_inset.max(0.0),
            fraction_z: spec.edge_inset_z.max(0.0),
            cell,
            slabs: slab_counts(&stack, spec.fidelity, cell, &inset_zones),
            zones: inset_zones,
        };
        let mut b = Builder {
            spec,
            stack: stack.clone(),
            region,
            net_ids: BTreeMap::new(),
            sheets: Vec::new(),
            wires: Vec::new(),
            counts: Counts::default(),
            flags: Vec::new(),
            notes: Vec::new(),
            inset,
        };

        // Vias and plated holes: barrels first (zones need their antipads).
        let mut barrels: Vec<Barrel> = Vec::new();
        for via in &vias {
            if !spec.includes(&via.net) {
                continue;
            }
            let p = mm_point(via.position);
            if p.0 < region[0] || p.0 > region[2] || p.1 < region[1] || p.1 > region[3] {
                continue;
            }
            let (Some(a), Some(c)) = (b.stack.z(via.from_layer), b.stack.z(via.to_layer)) else {
                b.flag("via_outside_stackup_dropped");
                continue;
            };
            let net_id = b.net_id(&via.net);
            let (top, bottom) = (a.max(c), a.min(c));
            b.wires.push(Wire {
                kind: "via",
                net: via.net.clone(),
                net_id,
                x: p.0,
                y: p.1,
                z_top: top,
                z_bottom: bottom,
                radius: nm_to_mm(via.drill_nm) / 2.0,
            });
            barrels.push(Barrel {
                net: via.net.clone(),
                x: p.0,
                y: p.1,
                radius: nm_to_mm(via.size_nm) / 2.0,
                z_top: top,
                z_bottom: bottom,
            });
            b.counts.vias += 1;
        }
        let stack_top = b.stack.copper.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max);
        let stack_bottom = b.stack.copper.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        for pad in &pads {
            let wanted = pad.net.as_ref().is_some_and(|n| spec.includes(n))
                || terminal_pads.iter().any(|(c, n)| *c == pad.component && *n == pad.number);
            if !wanted || !spec.include_pads {
                continue;
            }
            let Some(bb) = geom::bbox(&pad.ring) else { continue };
            if !overlaps(&pad.ring, region) {
                continue;
            }
            if pad.layers.len() > 1
                && let Some(drill) = pad.drill_mm
            {
                let c = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0);
                let net = pad.net.clone().unwrap_or_default();
                let net_id = b.net_id(&net);
                b.wires.push(Wire {
                    kind: "plated_hole",
                    net: net.clone(),
                    net_id,
                    x: c.0,
                    y: c.1,
                    z_top: stack_top,
                    z_bottom: stack_bottom,
                    radius: drill / 2.0,
                });
                barrels.push(Barrel {
                    net,
                    x: c.0,
                    y: c.1,
                    radius: pad.min_dimension_mm / 2.0,
                    z_top: stack_top,
                    z_bottom: stack_bottom,
                });
                b.counts.plated_holes += 1;
            }
        }

        // Copper.
        for t in &tracks {
            b.track(board, t);
        }
        for a in &arcs {
            b.arc(board, a);
        }
        for p in &polygons {
            if !spec.includes(&p.net) {
                continue;
            }
            let ring: Vec<Pt> = p.vertices.iter().map(|v| mm_point(*v)).collect();
            if overlaps(&ring, region) {
                b.counts.polygons += 1;
                b.push_sheet(board, "polygon", &p.net, p.layer, ring);
            }
        }
        let mut rounded_pads = 0;
        for pad in &pads {
            let wanted = pad.net.as_ref().is_some_and(|n| spec.includes(n))
                || terminal_pads.iter().any(|(c, n)| *c == pad.component && *n == pad.number);
            if !wanted || !spec.include_pads || !overlaps(&pad.ring, region) {
                continue;
            }
            b.counts.pads += 1;
            rounded_pads += usize::from(pad.rounded);
            let net = pad.net.clone().unwrap_or_default();
            for layer in &pad.layers {
                b.push_sheet(board, "pad", &net, *layer, pad.ring.clone());
            }
        }
        if rounded_pads > 0 {
            b.notes.push(format!(
                "{rounded_pads} rounded pad(s) are the convex hull of 12-gons swept along the pad spine (equal area, so their corners are chamfered)"
            ));
        }
        for extra in &spec.extra_traces {
            b.add_trace(board, extra)?;
        }
        for bridge in &spec.bridges {
            b.add_bridge(board, bridge, &pads)?;
        }
        // Zones last: they are cut around the copper of other nets that exists by now.
        let foreign: Vec<(CopperLayer, String, Vec<Pt>)> =
            b.sheets.iter().filter(|s| s.kind != "zone").map(|s| (s.layer, s.net.clone(), s.ring.clone())).collect();
        if let Some(set) = spec.zone_fills.clone() {
            // The model adds copper the board does not have (the hull of a capacitor or diode, the legs of an unrouted
            // loop): the real fill is cut around it as the outline was.
            let added: Vec<(CopperLayer, String, Vec<Pt>)> = b
                .sheets
                .iter()
                .filter(|s| matches!(s.kind, "bridge" | "added"))
                .map(|s| (s.layer, s.net.clone(), s.ring.clone()))
                .collect();
            for fill in &set.fills {
                let others: Vec<&Vec<Pt>> =
                    added.iter().filter(|(l, n, _)| *l == fill.layer && *n != fill.net).map(|(_, _, r)| r).collect();
                b.zone_fill(board, fill, &others);
            }
        }
        for z in &zones {
            b.zone(board, z, &barrels, &foreign);
        }
        if !spec.include_pads && !pads.is_empty() {
            b.flag("pads_omitted");
        }

        // Ports.
        let mut ports = Vec::new();
        let mut straps: Vec<Strap> = Vec::new();
        let mut posts: Vec<Post> = Vec::new();
        // Copper a direct port would cross: every sheet but planes and zones.
        let obstacles: Vec<(CopperLayer, &str, &[Pt])> = b
            .sheets
            .iter()
            .filter(|s| s.kind != "zone" && s.kind != "polygon")
            .map(|s| (s.layer, s.net.as_str(), s.ring.as_slice()))
            .collect();
        for p in &spec.ports {
            let (resolved, port_straps, port_posts) = resolve_port(p, &pads, &b.stack, &b.inset, b.region, &obstacles)?;
            if ports.iter().any(|q: &ResolvedPort| q.spec.name == p.name) {
                return Err(Error::Invalid(format!("two ports are named {}", p.name)));
            }
            ports.push(resolved);
            straps.extend(port_straps);
            posts.extend(port_posts);
        }
        for post in &posts {
            let net_id = b.net_id(&post.net);
            b.wires.push(Wire {
                kind: "post",
                net: post.net.clone(),
                net_id,
                x: post.at.0,
                y: post.at.1,
                z_top: post.z_top,
                z_bottom: post.z_bottom,
                radius: 0.05,
            });
        }
        if straps.iter().any(|s| s.in_air) {
            b.flag("port_strap_in_air");
        }
        if !straps.is_empty() {
            b.flag("port_strap");
            for st in &straps {
                let net_id = b.net_id(&st.net);
                let thickness_m = b.thickness_m(board, st.layer);
                b.sheets.push(Sheet {
                    kind: "strap",
                    net: st.net.clone(),
                    net_id,
                    layer: st.layer,
                    z: st.z,
                    ring: st.ring.clone(),
                    treatment: NetTreatment::Pec,
                    thickness_m,
                    touches_edge: false,
                });
            }
        }
        for p in &ports {
            let (lo, hi) = (p.start[0].min(p.stop[0]), p.start[0].max(p.stop[0]));
            let (lo_y, hi_y) = (p.start[1].min(p.stop[1]), p.start[1].max(p.stop[1]));
            if lo < region[0] - 1e-9 || hi > region[2] + 1e-9 || lo_y < region[1] - 1e-9 || hi_y > region[3] + 1e-9 {
                return Err(Error::Invalid(format!("port {} lies outside the window", p.spec.name)));
            }
        }

        // Flags that state limitations.
        if !spec.extend_planes {
            let planes = b.sheets.iter().any(|s| s.touches_edge && matches!(s.kind, "zone" | "polygon"));
            if planes {
                b.flag("planes_truncated");
            }
        }
        b.flag("copper_cut_at_window");
        if !b.stack.declared {
            b.flag("assumed_stackup");
        }
        if b.sheets.iter().any(|s| s.treatment == NetTreatment::Conducting) {
            b.flag("conducting_sheet_used");
        }
        // Zone sheets that still lie over foreign same-layer copper (a pour with no cuts, or a cut that failed).
        let foreign = b.sheets.iter().filter(|z| z.kind == "zone").any(|z| {
            b.sheets.iter().any(|s| {
                s.kind != "zone"
                    && s.layer == z.layer
                    && (s.z - z.z).abs() < 1e-9
                    && s.net != z.net
                    && s.ring.iter().any(|p| geom::contains(&z.ring, *p))
            })
        });
        if foreign {
            b.flag("zone_overlaps_foreign_copper");
        }

        let inset_zones = b.inset.zones.clone();
        let Builder { sheets, wires, counts, flags, notes, net_ids, .. } = b;
        let mut net_names = vec![String::new(); net_ids.len()];
        for (n, id) in &net_ids {
            net_names[*id as usize] = n.clone();
        }
        let mut model = RegionModel {
            spec: spec.clone(),
            stack,
            region,
            sheets,
            wires,
            ports,
            straps,
            axes: [empty_axis(), empty_axis(), empty_axis()],
            counts,
            mesh: MeshReport {
                lines: [0; 3],
                cells: 0,
                smallest: [0.0; 3],
                largest: [0.0; 3],
                dt_estimate: 0.0,
                moved_edges: 0,
                max_move_mm: 0.0,
                slivers: Vec::new(),
                max_ratio: 1.0,
                ratio_violations: 0,
                copper_cell_mm: 0.0,
            },
            approximations: notes,
            flags,
            centre_hz: (spec.band_hz.0 + spec.band_hz.1) / 2.0,
            domain: [[0.0; 2]; 3],
            net_names,
        };
        model.build_mesh(cell, inset_zones)?;
        model.describe_approximations();
        Ok(model)
    }
}

/// The refinement intervals of x and y in millimetres, with the cells scaled by `refine_scale` and never above the general cell.
fn refine_zones(spec: &RegionSpec, base_cell: f64, cell: f64) -> [Vec<(f64, f64, f64)>; 2] {
    let mut zones: [Vec<(f64, f64, f64)>; 2] = [Vec::new(), Vec::new()];
    for r in &spec.refine {
        // A zone cell is the box's cell or the general one, whichever is smaller, times the scale.
        let c = (nm_to_mm(r.cell_nm).min(base_cell) * spec.refine_scale).max(0.02);
        if c >= cell {
            continue;
        }
        let rect = [nm_to_mm(r.rect_nm[0]), nm_to_mm(r.rect_nm[1]), nm_to_mm(r.rect_nm[2]), nm_to_mm(r.rect_nm[3])];
        zones[0].push((rect[0], rect[2], c));
        zones[1].push((rect[1], rect[3], c));
    }
    zones
}

/// The copper-region cell: from the override, else from the narrowest
/// modelled feature `narrowest` (mm): half of it coarse, a quarter fine.
fn copper_cell(spec: &RegionSpec, narrowest: f64) -> f64 {
    match spec.cell_nm {
        Some(c) => nm_to_mm(c),
        None => {
            let f = if narrowest.is_finite() { narrowest } else { 0.5 };
            match spec.fidelity {
                Fidelity::Coarse => (f / 2.0).clamp(0.15, 0.5),
                Fidelity::Fine => (f / 4.0).clamp(0.075, 0.25),
            }
        }
    }
}

fn empty_axis() -> AxisMesh {
    AxisMesh {
        lines: Vec::new(),
        kept: Vec::new(),
        moves: Vec::new(),
        slivers: Vec::new(),
        net_merges: 0,
        max_ratio: 1.0,
        ratio_violations: 0,
    }
}

impl Builder<'_> {
    fn add_trace(&mut self, board: &PhysicalBoard, extra: &ExtraTrace) -> Result<()> {
        if extra.points.len() < 2 {
            return Err(Error::Invalid(format!("added trace {} needs two points", extra.name)));
        }
        for pair in extra.points.windows(2) {
            let (a, c) = ((nm_to_mm(pair[0][0]), nm_to_mm(pair[0][1])), (nm_to_mm(pair[1][0]), nm_to_mm(pair[1][1])));
            let ring = geom::track_polygon(a, c, nm_to_mm(extra.width_nm), true);
            self.push_sheet(board, "added", &extra.net, extra.layer, ring);
        }
        self.counts.added_traces += 1;
        self.flag("added_traces");
        Ok(())
    }

    fn add_bridge(&mut self, board: &PhysicalBoard, bridge: &Bridge, pads: &[PlacedPad]) -> Result<()> {
        let (la, ra, na) = terminal_info(&bridge.a, pads, &self.stack, None, self.region)?;
        let (lb, rb, _) = terminal_info(&bridge.b, pads, &self.stack, None, self.region)?;
        if la != lb {
            return Err(Error::NotSupported(format!("bridge {} joins pads on different layers", bridge.name)));
        }
        let all: Vec<Pt> = ra.iter().chain(&rb).copied().collect();
        let hull = geom::convex_hull(&all);
        self.counts.bridges += 1;
        let net = na.unwrap_or_default();
        // A bridge shorts two nets by design; its sheet is exempt from the net filter.
        let z = self.stack.z(la).expect("layer in stack");
        let net_id = self.net_id(&net);
        let ring = geom::clip_rect(&hull, self.region);
        let ring = if ring.len() >= 3 { self.inset_ring(ring, z) } else { ring };
        if ring.len() >= 3 {
            let thickness_m = self.thickness_m(board, la);
            self.sheets.push(Sheet {
                kind: "bridge",
                net,
                net_id,
                layer: la,
                z,
                ring,
                treatment: NetTreatment::Pec,
                thickness_m,
                touches_edge: false,
            });
        }
        Ok(())
    }
}

impl RegionModel {
    fn build_mesh(&mut self, cell: f64, zones: [Vec<(f64, f64, f64)>; 2]) -> Result<()> {
        let spec = &self.spec;
        let f_top = spec.band_hz.1;
        let er_max = self.stack.max_er();
        let lambda_diel = C0_MM_PER_S / f_top / er_max.sqrt();
        let max_inner = (lambda_diel / 15.0).min(1.0).max(cell);
        let max_outer = (C0_MM_PER_S / f_top / 15.0).min(3.0).max(max_inner);
        let region = self.region;
        let long_side = (region[2] - region[0]).max(region[3] - region[1]);
        let air = spec.air_margin_nm.map_or((long_side / 2.0).max(8.0), nm_to_mm);
        let zones_cell = zones.iter().flatten().map(|z| z.2).fold(cell, f64::min);
        let min_cell = (0.4 * cell).max(0.04);
        let thinnest = self.stack.thinnest();
        let min_cell_z = if thinnest.is_finite() { min_cell.min(0.45 * thinnest) } else { min_cell };

        // Candidate lines.
        let mut lines: [Vec<Line>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for k in 0..2 {
            lines[k].push(Line::new(region[k], Rank::Window, None));
            lines[k].push(Line::new(region[k + 2], Rank::Window, None));
        }
        for s in &self.sheets {
            let ring = &s.ring;
            let n = ring.len();
            if let Some(bb) = geom::bbox(ring) {
                lines[0].push(Line::new(bb[0], Rank::Copper, Some(s.net_id)));
                lines[0].push(Line::new(bb[2], Rank::Copper, Some(s.net_id)));
                lines[1].push(Line::new(bb[1], Rank::Copper, Some(s.net_id)));
                lines[1].push(Line::new(bb[3], Rank::Copper, Some(s.net_id)));
            }
            for i in 0..n {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                if (a.0 - b.0).abs() < 1e-9 {
                    lines[0].push(Line::new(a.0, Rank::Copper, Some(s.net_id)));
                }
                if (a.1 - b.1).abs() < 1e-9 {
                    lines[1].push(Line::new(a.1, Rank::Copper, Some(s.net_id)));
                }
            }
        }
        for w in &self.wires {
            lines[0].push(Line::new(w.x, Rank::Via, Some(w.net_id)));
            lines[1].push(Line::new(w.y, Rank::Via, Some(w.net_id)));
        }
        for p in &self.ports {
            for (k, axis_lines) in lines.iter_mut().enumerate() {
                let (a, b) = (p.start[k], p.stop[k]);
                axis_lines.push(Line::new(a, Rank::Port, None));
                axis_lines.push(Line::new(b, Rank::Port, None));
                if k == p.axis && k < 2 {
                    axis_lines.push(Line::new((a + b) / 2.0, Rank::Port, None));
                }
            }
        }
        // z: copper layers and slab edges, and sheets in the air (air-bridge straps).
        for (_, z) in &self.stack.copper {
            lines[2].push(Line::new(*z, Rank::Copper, None));
        }
        for s in self.sheets.iter().filter(|s| s.kind == "strap") {
            lines[2].push(Line::new(s.z, Rank::Copper, None));
        }
        for s in &self.stack.slabs {
            lines[2].push(Line::new(s.top, Rank::Copper, None));
            lines[2].push(Line::new(s.bottom, Rank::Copper, None));
        }
        let gap_counts = slab_counts(&self.stack, spec.fidelity, cell, &zones);

        // The fine zone of x and y: around every non-window line.
        // With local refinement the merge distance shrinks with the cell, so the floor below which lines of
        // different nets are taken to overlap stays at 0.075 mm: the edge rule moves the edges of placed
        // straight legs and of the pads they cross by a fraction of a cell, which leaves artefact slivers
        // of that size, and a sliver sets the global time step.
        let refined = self.spec.smooth_mesh || zones.iter().any(|z| !z.is_empty());
        let net_floor_xy = if refined { (0.25 * min_cell).max(0.075) } else { 0.25 * min_cell };
        let mut plans: Vec<AxisPlan> = Vec::new();
        for (k, axis_lines) in lines.iter().enumerate() {
            let fine = {
                let v: Vec<f64> = axis_lines.iter().filter(|l| l.rank != Rank::Window).map(|l| l.pos).collect();
                if v.is_empty() {
                    None
                } else {
                    Some((
                        v.iter().copied().fold(f64::INFINITY, f64::min) - 1.0,
                        v.iter().copied().fold(f64::NEG_INFINITY, f64::max) + 1.0,
                    ))
                }
            };
            if k < 2 {
                plans.push(AxisPlan {
                    lines: axis_lines.clone(),
                    cell,
                    fine_zone: fine,
                    zones: zones[k].clone(),
                    min_cell,
                    net_floor: net_floor_xy,
                    max_inner,
                    max_outer,
                    air,
                    pml_cells: PML_CELLS,
                    gap_counts: Vec::new(),
                    smooth: refined,
                });
            } else {
                let bottom = self.stack.bottom();
                let top = self.stack.copper.iter().map(|c| c.1).fold(0.0, f64::max);
                plans.push(AxisPlan {
                    lines: axis_lines.clone(),
                    cell: zones_cell.max(0.1),
                    fine_zone: Some((bottom - 4.0 * zones_cell, top + 4.0 * zones_cell)),
                    zones: Vec::new(),
                    min_cell: min_cell_z,
                    net_floor: 0.25 * min_cell_z,
                    max_inner: max_inner.max(cell),
                    max_outer,
                    air,
                    pml_cells: PML_CELLS,
                    gap_counts: gap_counts.clone(),
                    smooth: false,
                });
            }
        }
        let mut axes = Vec::new();
        for plan in &plans {
            axes.push(axis_mesh(plan)?);
        }
        let axes: [AxisMesh; 3] = axes.try_into().map_err(|_| Error::Invalid("three axes".to_owned()))?;

        // Snap the geometry that sits on moved lines.
        let tol = min_cell;
        let snap = |axis: usize, v: f64| {
            let n = axes[axis].nearest(v);
            if (n - v).abs() <= tol { n } else { v }
        };
        for s in &mut self.sheets {
            let n = s.ring.len();
            let original = s.ring.clone();
            for i in 0..n {
                let prev = original[(i + n - 1) % n];
                let next = original[(i + 1) % n];
                let here = original[i];
                if (prev.0 - here.0).abs() < 1e-9 || (next.0 - here.0).abs() < 1e-9 {
                    s.ring[i].0 = snap(0, here.0);
                }
                if (prev.1 - here.1).abs() < 1e-9 || (next.1 - here.1).abs() < 1e-9 {
                    s.ring[i].1 = snap(1, here.1);
                }
            }
        }
        let mut max_move = 0.0_f64;
        for w in &mut self.wires {
            let (nx, ny) = (axes[0].nearest(w.x), axes[1].nearest(w.y));
            max_move = max_move.max((nx - w.x).abs()).max((ny - w.y).abs());
            w.x = nx;
            w.y = ny;
        }
        let moved_edges: usize = axes.iter().map(|a| a.moves.len()).sum();
        for a in &axes {
            for m in &a.moves {
                max_move = max_move.max((m.from - m.to).abs());
            }
        }
        let smallest = [axes[0].smallest(), axes[1].smallest(), axes[2].smallest()];
        let largest = [0, 1, 2].map(|k| axes[k].cells().into_iter().fold(0.0, f64::max));
        let nl = [axes[0].lines.len(), axes[1].lines.len(), axes[2].lines.len()];
        let cells = (nl[0] - 1) * (nl[1] - 1) * (nl[2] - 1);
        let inv: f64 = smallest.iter().map(|s| 1.0 / (s * s)).sum();
        // Courant limit of the smallest cells (cells in mm, c in mm/s); the solver's own step is a few percent lower on a graded mesh.
        let dt_estimate = 1.0 / (C0_MM_PER_S * inv.sqrt());
        let mut slivers = Vec::new();
        for (k, a) in axes.iter().enumerate() {
            for (pos, w) in &a.slivers {
                slivers.push(format!("{}={pos:.4} mm: {w:.4} mm between lines of different nets", ["x", "y", "z"][k]));
            }
        }
        self.mesh = MeshReport {
            lines: nl,
            cells,
            smallest,
            largest,
            dt_estimate,
            moved_edges,
            max_move_mm: max_move,
            slivers,
            max_ratio: axes.iter().map(|a| a.max_ratio).fold(1.0, f64::max),
            ratio_violations: axes.iter().map(|a| a.ratio_violations).sum(),
            copper_cell_mm: cell,
        };
        self.domain = [0, 1, 2].map(|k| [axes[k].lines[0], *axes[k].lines.last().expect("lines")]);
        if axes.iter().any(|a| a.net_merges > 0) {
            self.flags.push("nets_merged_below_floor".to_owned());
            self.approximations.push(format!(
                "{} pair(s) of mesh lines of different nets closer than a quarter of the minimum cell were merged (overlapping conductors, as when a straight leg of a placed loop crosses another net's pad)",
                axes.iter().map(|a| a.net_merges).sum::<usize>()
            ));
        }
        self.axes = axes;

        // Extension of plane-like copper and dielectric into the PML.
        if self.spec.extend_planes {
            let far = 1.0;
            let (r, d) = (self.region, self.domain);
            for s in &mut self.sheets {
                if !(s.touches_edge && matches!(s.kind, "zone" | "polygon")) {
                    continue;
                }
                for p in &mut s.ring {
                    if (p.0 - r[0]).abs() < 1e-9 {
                        p.0 = d[0][0] - far;
                    }
                    if (p.0 - r[2]).abs() < 1e-9 {
                        p.0 = d[0][1] + far;
                    }
                    if (p.1 - r[1]).abs() < 1e-9 {
                        p.1 = d[1][0] - far;
                    }
                    if (p.1 - r[3]).abs() < 1e-9 {
                        p.1 = d[1][1] + far;
                    }
                }
            }
        }
        Ok(())
    }

    fn describe_approximations(&mut self) {
        let mut a = vec![
            "copper is zero-thickness PEC (resistance, skin effect and thickness are not modelled) unless a net is marked conducting".to_owned(),
            "components are not modelled: capacitors and diodes are bridged by copper, the switch is a lumped port".to_owned(),
            "copper is cut at the window edge; nets outside the include list, other layers' components and the enclosure are absent".to_owned(),
        ];
        if self.flags.iter().any(|f| f == "zone_fill_used") {
            a.push(format!(
                "{} zone fill piece(s) from the native fill (clearances, thermal reliefs, openings for other nets' vias and pads, minimum width, islands removed as the zone asks) replace zone outlines where a fill exists; the fill is cut to the window and around the model's bridges and added traces",
                self.counts.zone_fills
            ));
        }
        if self.flags.iter().any(|f| f == "zone_outline_used") {
            a.push("zones are their outline minus declared holes and via antipads; fills, thermal reliefs and clearances around foreign same-layer copper are not known".to_owned());
        }
        if self.flags.iter().any(|f| f == "planes_truncated") {
            a.push("planes end at the window edge (planes_truncated): the plane's true extent is not modelled, keep the margin generous".to_owned());
        }
        if self.spec.extend_planes {
            a.push(
                "plane-like copper and the dielectric that touch the window edge continue through the air into the PML"
                    .to_owned(),
            );
        }
        if !self.stack.declared {
            a.push(format!(
                "no stack-up declared: layers spread evenly over the board thickness, FR-4 er {} loss tangent {}",
                crate::stack::DEFAULT_ER,
                crate::stack::DEFAULT_LOSS_TANGENT
            ));
        }
        if self.counts.antipads > 0 {
            a.push(format!("{} octagonal antipad(s) cut where a via or hole crosses a foreign-net plane (via radius plus the zone clearance)", self.counts.antipads));
        }
        if self.counts.vias + self.counts.plated_holes > 0 {
            a.push("vias and plated holes are one-cell PEC wires on the mesh line nearest their centre (no barrel radius, no annular rings)".to_owned());
        }
        self.approximations.extend(a);
    }

    /// The expected cost of one run from the mesh alone.
    pub fn estimate(&self) -> Estimate {
        let cells = self.mesh.cells;
        let dt = self.mesh.dt_estimate;
        let (_, fc) = self.excitation();
        // The run ends when the pulse is over (3.6 / fc, the Gaussian truncated at twice its delay) and the
        // energy has fallen 35 dB. On the validation loops over a plane the energy was already 45 to 60 dB
        // down when the pulse ended, so the pulse alone sets the end; the loop in free space, which radiates
        // and rings longer, needed 0.5 ns more (about 8 transits of the region's diagonal in air).
        let diag = ((self.region[2] - self.region[0]).powi(2) + (self.region[3] - self.region[1]).powi(2)).sqrt();
        let sim_time = 3.6 / fc + 8.0 * diag / C0_MM_PER_S;
        let steps = (sim_time / dt).ceil() as u64;
        let cell_updates = cells as f64 * steps as f64;
        let seconds = cell_updates / self.spec.budget.assumed_rate;
        Estimate { cells, dt, sim_time, steps, cell_updates, seconds }
    }

    /// The Gaussian `(f0, fc)` of the excitation: baseband with 20 dB at the band's top times `pulse_scale`.
    pub fn excitation(&self) -> (f64, f64) {
        excitation_of(self.spec.band_hz.0, self.spec.band_hz.1 * self.spec.pulse_scale.max(0.1))
    }

    /// A summary for reports: size, mesh, contents, approximations, flags.
    pub fn describe(&self) -> String {
        let e = self.estimate();
        let m = &self.mesh;
        let c = &self.counts;
        let mut s = String::new();
        s.push_str(&format!(
            "region {:.2} x {:.2} mm (window plus {:.2} mm margin), band {:.0}-{:.0} MHz, {} fidelity\n",
            self.region[2] - self.region[0],
            self.region[3] - self.region[1],
            nm_to_mm(self.spec.margin_nm),
            self.spec.band_hz.0 * 1e-6,
            self.spec.band_hz.1 * 1e-6,
            self.spec.fidelity.name()
        ));
        s.push_str(&format!(
            "stack: {} copper layers, {} dielectric slabs, er {:.2} max{}\n",
            self.stack.copper.len(),
            self.stack.slabs.len(),
            self.stack.max_er(),
            if self.stack.declared { "" } else { " (assumed)" }
        ));
        s.push_str(&format!(
            "copper: {} pads, {} tracks, {} arcs, {} polygons, {} zones, {} vias, {} plated holes, {} bridges, {} added traces ({} sheets, {} wires)\n",
            c.pads, c.tracks, c.arcs, c.polygons, c.zones, c.vias, c.plated_holes, c.bridges, c.added_traces, self.sheets.len(), self.wires.len()
        ));
        for p in &self.ports {
            s.push_str(&format!("port: {}\n", p.description));
        }
        s.push_str(&format!(
            "mesh: {} x {} x {} lines = {} cells, copper cell {:.3} mm, smallest cell {:.3} / {:.3} / {:.3} mm, largest {:.2} / {:.2} / {:.2} mm, max grading ratio {:.2} ({} pairs above 1.5)\n",
            m.lines[0], m.lines[1], m.lines[2], m.cells, m.copper_cell_mm, m.smallest[0], m.smallest[1], m.smallest[2], m.largest[0], m.largest[1], m.largest[2], m.max_ratio, m.ratio_violations
        ));
        s.push_str(&format!(
            "edge rule: copper edges moved in by {:.2} lateral cell + {:.2} vertical cell
",
            self.spec.edge_inset, self.spec.edge_inset_z
        ));
        s.push_str(&format!(
            "geometry moved by merging: {} line(s), at most {:.4} mm; {} sliver(s) kept between nets\n",
            m.moved_edges,
            m.max_move_mm,
            m.slivers.len()
        ));
        for (k, a) in self.axes.iter().enumerate() {
            let w = a.cells();
            if let Some((i, c)) = w.iter().enumerate().min_by(|x, y| x.1.total_cmp(y.1)) {
                let near: Vec<String> = a.lines[i.saturating_sub(3)..(i + 5).min(a.lines.len())]
                    .iter()
                    .map(|v| format!("{v:.3}"))
                    .collect();
                s.push_str(&format!(
                    "smallest cell {:.3} mm at {}={:.3} (lines {}), ",
                    c,
                    ["x", "y", "z"][k],
                    a.lines[i],
                    near.join(" ")
                ));
            }
            let worst = w.windows(2).enumerate().max_by(|x, y| {
                (x.1[0] / x.1[1]).max(x.1[1] / x.1[0]).total_cmp(&(y.1[0] / y.1[1]).max(y.1[1] / y.1[0]))
            });
            if let Some((i, p)) = worst {
                s.push_str(&format!(
                    ", worst ratio {:.2} ({:.3} next to {:.3}) at {:.3}
",
                    (p[0] / p[1]).max(p[1] / p[0]),
                    p[0],
                    p[1],
                    a.lines[i + 1]
                ));
            }
        }
        for (k, a) in self.axes.iter().enumerate() {
            let w = a.cells();
            let n = a.lines.len();
            for i in 0..w.len().saturating_sub(1) {
                let r = (w[i] / w[i + 1]).max(w[i + 1] / w[i]);
                if r > 1.5 + 1e-6 {
                    let at = a.lines[i + 1];
                    let place = if i < PML_CELLS || i + 1 >= n - 1 - PML_CELLS {
                        "in the PML"
                    } else if k < 2 && at >= self.region[k] && at <= self.region[k + 2] {
                        "inside the window"
                    } else {
                        "in the air"
                    };
                    s.push_str(&format!(
                        "ratio {:.2} ({:.3} next to {:.3}) at {}={:.3} mm, {place}
",
                        r,
                        w[i],
                        w[i + 1],
                        ["x", "y", "z"][k],
                        at
                    ));
                }
            }
        }
        for sliver in &m.slivers {
            s.push_str(&format!(
                "sliver: {sliver}
"
            ));
        }
        s.push_str(&format!(
            "estimate before running: dt {:.4} ps, {} steps ({:.2} ns), {:.2} G cell updates, about {:.0} s at {:.0} M/s\n",
            e.dt * 1e12,
            e.steps,
            e.sim_time * 1e9,
            e.cell_updates * 1e-9,
            e.seconds,
            self.spec.budget.assumed_rate * 1e-6
        ));
        s.push_str(&format!(
            "flags: {}\n",
            if self.flags.is_empty() { "none".to_owned() } else { self.flags.join(", ") }
        ));
        for a in &self.approximations {
            s.push_str(&format!("approximation: {a}\n"));
        }
        s
    }

    /// Conductivity of a dielectric slab at the band centre, S/m.
    pub fn slab_conductivity(&self, er: f64, loss_tangent: f64) -> f64 {
        2.0 * std::f64::consts::PI * self.centre_hz * crate::EPS0 * er * loss_tangent
    }

    pub fn conducting_sigma() -> f64 {
        COPPER_SIGMA
    }
}

/// The Gaussian `(f0, fc)` that covers `[lo, hi]` at its 20 dB points. A band
/// that starts near DC (`lo` below a quarter of `hi`) gets a baseband pulse
/// (`f0 = 0`, 20 dB at `hi`), which keeps the low frequencies an inductance
/// extraction needs from falling into the pulse's spectral hole.
pub fn excitation_of(lo: f64, hi: f64) -> (f64, f64) {
    if lo <= 0.25 * hi {
        return (0.0, hi);
    }
    let (f0, half) = ((lo + hi) / 2.0, (hi - lo) / 2.0);
    (f0, (2.0 * half).min(0.95 * f0).max(half))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{EDGE_INSET, EDGE_INSET_Z};

    fn rule(slabs: Vec<(f64, f64, usize)>) -> InsetRule {
        InsetRule { fraction: EDGE_INSET, fraction_z: EDGE_INSET_Z, cell: 0.25, zones: [Vec::new(), Vec::new()], slabs }
    }

    #[test]
    fn the_vertical_cell_is_that_of_the_slabs_next_to_the_layer() {
        // F.Cu at 0, 0.5 mm of dielectric in two cells, In1 at -0.5, 0.2 mm in two cells, B.Cu at -0.7.
        let r = rule(vec![(-0.5, 0.0, 2), (-0.7, -0.5, 2)]);
        // The top layer has dielectric below only: its 0.25 mm cell counts with the air cell above (1.3 times).
        assert!((r.dz_at(0.0) - 0.25 * 1.15).abs() < 1e-12);
        // An inner layer has both: the mean of 0.25 and 0.1.
        assert!((r.dz_at(-0.5) - 0.175).abs() < 1e-12);
        assert!((r.dz_at(-0.7) - 0.1 * 1.15).abs() < 1e-12);
        // No slab next to the plane (an air bridge): the lateral cell.
        assert!((r.dz_at(0.3) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn an_edge_moves_in_by_the_lateral_and_the_vertical_part() {
        let r = rule(vec![(-0.2, 0.0, 2)]);
        let d = r.delta((0.0, 0.0), (1.0, 0.0), 0.0);
        let expected = EDGE_INSET * 0.25 + EDGE_INSET_Z * 0.1 * 1.15;
        assert!((d - expected).abs() < 1e-12, "{d} against {expected}");
        // Both fractions zero put the edges as drawn.
        let off = InsetRule { fraction: 0.0, fraction_z: 0.0, ..rule(vec![(-0.2, 0.0, 2)]) };
        assert!(off.is_off());
        let ring = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert_eq!(off.ring(&ring, [-9.0, -9.0, 9.0, 9.0], 0.0), Some(ring));
    }

    #[test]
    fn slab_counts_follow_the_finest_cell() {
        let mut board = crate::testing::bare_board();
        crate::testing::two_layer(&mut board, 500_000, "4.3");
        let stack = crate::stack::stack(&board).expect("stack");
        let none: [Vec<(f64, f64, f64)>; 2] = [Vec::new(), Vec::new()];
        // 0.5 mm in cells of at most 1.25 x 0.25: two; with a 0.1 mm zone: four.
        assert_eq!(slab_counts(&stack, Fidelity::Coarse, 0.25, &none)[0].2, 2);
        let zones = [vec![(0.0, 1.0, 0.1)], vec![(0.0, 1.0, 0.1)]];
        assert_eq!(slab_counts(&stack, Fidelity::Coarse, 0.25, &zones)[0].2, 4);
        assert_eq!(slab_counts(&stack, Fidelity::Fine, 0.25, &none)[0].2, 3);
    }
}

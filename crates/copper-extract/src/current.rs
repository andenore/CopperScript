//! The distribution of a port's current over the mesh, for pictures: where the current flows in the copper and how
//! it divides between the vias.
//!
//! A solved [`Extraction`] already holds the complex current of every filament for a unit current into each port
//! (the loop analysis computes it); [`Extraction::current_map`] arranges one frequency of it by layer and via.
//! Filaments that share a bar (the thickness filaments) are summed. Nothing here is solved again: the numbers are
//! the ones the port impedance was made from.
//!
//! Directions: a planar bar carries its current along `+x` or `+y` (the sign of the real part says which way at
//! the instant the port current is at its peak, since the port current is the real unit current), a via segment
//! along `+z` (towards the top layer).

use std::collections::BTreeMap;

use copper_emi::spectrum::Complex;

use crate::extract::Extraction;
use crate::layout::Layout;
use crate::network::BranchKind;
use crate::partial::Axis;

const NM_TO_MM: f64 = 1e-6;

/// One bar of copper (a filament, or the filaments of one bar across the thickness) and its current.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentBar {
    /// Along `x` (else along `y`).
    pub along_x: bool,
    /// Centre of the bar, mm.
    pub centre_mm: [f64; 2],
    pub length_mm: f64,
    /// The copper cross-section's width: the strip the bar represents times the part of it that copper covers, mm.
    pub width_mm: f64,
    /// Current along `+x` (`+y`) per ampere into the port, A.
    pub current: Complex,
}

/// One copper layer's share of the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentLayer {
    pub name: String,
    /// Height of the layer's centre, mm.
    pub z_mm: f64,
    /// The copper the layer was given, as shapes (an outer contour, then its holes), mm.
    pub outlines: Vec<Vec<Vec<[f64; 2]>>>,
    pub bars: Vec<CurrentBar>,
}

/// The current of a via between two layers it joins.
#[derive(Clone, Debug, PartialEq)]
pub struct ViaSegment {
    /// Layer indices (into [`CurrentMap::layers`]), `lower` the one with the smaller z.
    pub lower: usize,
    pub upper: usize,
    /// Current along `+z` (towards the top) per ampere into the port, A.
    pub current: Complex,
}

/// A via (or the barrel of a through-hole pad) and its segments.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentVia {
    pub name: String,
    pub net: String,
    pub position_mm: [f64; 2],
    /// Outer diameter of the barrel, mm.
    pub diameter_mm: f64,
    pub segments: Vec<ViaSegment>,
}

/// The port the current was injected at.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentPort {
    pub name: String,
    /// The layer of the plus and the minus terminal, and where they are (centre of the terminal's region), mm.
    pub plus: (usize, [f64; 2]),
    pub minus: (usize, [f64; 2]),
}

/// The current distribution of one port at one frequency.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMap {
    pub frequency_hz: f64,
    /// Window `[x0, y0, x1, y1]`, mm.
    pub window_mm: [f64; 4],
    pub layers: Vec<CurrentLayer>,
    pub vias: Vec<CurrentVia>,
    pub port: CurrentPort,
}

/// The vias of one net that join the same two layers, with their shares of that transition's current.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub net: String,
    pub lower: usize,
    pub upper: usize,
    /// `(via index into CurrentMap::vias, current, share)` strongest first; `share = |I| / sum |I|` over the group.
    pub vias: Vec<(usize, Complex, f64)>,
    /// The complex sum of the currents of the group (along `+z`), A per ampere into the port. A net whose port
    /// terminals are on one layer sends as much current up as down (the sum is then zero): see `up` and `down`.
    pub total: Complex,
    /// The current the group carries towards the top layer and towards the bottom one, at the instant the port
    /// current (the real unit current) peaks, A per ampere into the port: the sums of the positive and of the
    /// negative real parts of the via currents. About 1 where the whole port current changes layer here.
    pub up: f64,
    pub down: f64,
}

impl CurrentMap {
    /// The vias grouped by net and pair of layers, in a fixed order (net, lower, upper).
    pub fn transitions(&self) -> Vec<Transition> {
        let mut groups: BTreeMap<(String, usize, usize), Vec<(usize, Complex)>> = BTreeMap::new();
        for (v, via) in self.vias.iter().enumerate() {
            for s in &via.segments {
                groups.entry((via.net.clone(), s.lower, s.upper)).or_default().push((v, s.current));
            }
        }
        groups
            .into_iter()
            .map(|((net, lower, upper), mut members)| {
                let magnitude: f64 = members.iter().map(|m| m.1.abs()).sum();
                let total = members.iter().fold(Complex::ZERO, |a, m| a + m.1);
                let up: f64 = members.iter().map(|m| m.1.re.max(0.0)).sum();
                let down: f64 = members.iter().map(|m| (-m.1.re).max(0.0)).sum();
                members.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()).then(a.0.cmp(&b.0)));
                Transition {
                    net,
                    lower,
                    upper,
                    vias: members
                        .into_iter()
                        .map(|(v, c)| (v, c, if magnitude > 0.0 { c.abs() / magnitude } else { 0.0 }))
                        .collect(),
                    total,
                    up,
                    down,
                }
            })
            .collect()
    }

    /// The largest sheet current `|I| / width` over the bars of `layer`, A/m per ampere (0 without bars).
    pub fn peak_sheet_current(&self, layer: usize) -> f64 {
        self.layers[layer]
            .bars
            .iter()
            .filter(|b| b.width_mm > 0.0)
            .map(|b| b.current.abs() / (b.width_mm * 1e-3))
            .fold(0.0, f64::max)
    }
}

impl Extraction {
    /// The current distribution for a unit current into `port` at frequency index `f`. `None` when the solution
    /// kept no branch currents (`extract(.., currents = false)`) or `port` / `f` do not exist.
    pub fn current_map(&self, layout: &Layout, f: usize, port: usize) -> Option<CurrentMap> {
        let solution = self.solutions.get(f)?;
        let currents = solution.currents.get(port)?;
        let network = &self.mesh.network;
        if currents.len() != network.branches.len() {
            return None;
        }
        let mut layers: Vec<CurrentLayer> = layout
            .layers
            .iter()
            .map(|l| CurrentLayer {
                name: l.name.clone(),
                z_mm: l.z_m * 1e3,
                outlines: l
                    .conductors
                    .iter()
                    .flat_map(|c| c.shapes.iter())
                    .map(|shape| {
                        shape
                            .iter()
                            .map(|contour| {
                                contour.iter().map(|p| [p.x as f64 * NM_TO_MM, p.y as f64 * NM_TO_MM]).collect()
                            })
                            .collect()
                    })
                    .collect(),
                bars: Vec::new(),
            })
            .collect();
        // Planar bars: the filaments of one bar (its thickness filaments) are in parallel; their currents add.
        let mut bars: BTreeMap<usize, (usize, CurrentBar)> = BTreeMap::new();
        let mut via_segments: BTreeMap<usize, Vec<ViaSegment>> = BTreeMap::new();
        for (k, branch) in network.branches.iter().enumerate() {
            match branch.kind {
                BranchKind::Plane { layer } => {
                    let entry = bars.entry(branch.group).or_insert_with(|| {
                        (
                            layer,
                            CurrentBar {
                                along_x: branch.bar.axis == Axis::X,
                                centre_mm: [branch.bar.centre[0] * 1e3, branch.bar.centre[1] * 1e3],
                                length_mm: branch.bar.length * 1e3,
                                width_mm: branch.bar.width * 1e3,
                                current: Complex::ZERO,
                            },
                        )
                    });
                    entry.1.current = entry.1.current + currents[k];
                }
                BranchKind::Barrel { via } => {
                    let (la, lb) = (self.mesh.nodes[branch.a].layer, self.mesh.nodes[branch.b].layer);
                    // Oriented along +z from node a (lower) to node b (upper).
                    let (lower, upper) =
                        if layout.layers[la].z_m <= layout.layers[lb].z_m { (la, lb) } else { (lb, la) };
                    via_segments.entry(via).or_default().push(ViaSegment { lower, upper, current: currents[k] });
                }
                BranchKind::Free => {}
            }
        }
        for (_, (layer, bar)) in bars {
            layers[layer].bars.push(bar);
        }
        let vias = layout
            .vias
            .iter()
            .enumerate()
            .filter_map(|(v, via)| {
                let segments = via_segments.remove(&v)?;
                Some(CurrentVia {
                    name: via.name.clone(),
                    net: via.net.clone(),
                    position_mm: [via.x as f64 * NM_TO_MM, via.y as f64 * NM_TO_MM],
                    diameter_mm: (via.drill_nm + 2 * via.plating_nm) as f64 * NM_TO_MM,
                    segments,
                })
            })
            .collect();
        let def = layout.ports.get(port)?;
        let at = |terminal: usize| -> Option<(usize, [f64; 2])> {
            let t = layout.terminals.get(terminal)?;
            let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
            for shape in &t.region {
                let b = copper_cam::poly::bounds(shape);
                (x0, y0, x1, y1) = (x0.min(b.0), y0.min(b.1), x1.max(b.2), y1.max(b.3));
            }
            (x0 <= x1).then(|| (t.layer, [(x0 + x1) as f64 / 2.0 * NM_TO_MM, (y0 + y1) as f64 / 2.0 * NM_TO_MM]))
        };
        Some(CurrentMap {
            frequency_hz: solution.frequency,
            window_mm: layout.window.map(|v| v as f64 * NM_TO_MM),
            layers,
            vias,
            port: CurrentPort { name: def.name.clone(), plus: at(def.plus)?, minus: at(def.minus)? },
        })
    }
}

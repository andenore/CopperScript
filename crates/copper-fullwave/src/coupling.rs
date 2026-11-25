//! Coupling between nets (`docs/coupling-query-plan.md`): the transmission from a port on one net (the
//! aggressor) to ports on others (the victims), each port a lumped element from one end of its net down to the
//! ground plane under it, terminated in the end's impedance.
//!
//! [`net_ends`] finds where a net ends: the placed pads on it, or, for copper with no pad, the free ends of its
//! tracks. [`net_geometry`] translates one net's copper with the region model's board translator (tracks at any
//! angle, arcs, vias, pads, the copper of hard macros) for a model that is not a region model, such as the
//! antenna site model. [`coupling_model`] builds the region model of an aggressor, its victims and the ground nets
//! around them (real zone fills, planes continued into the PML), and [`CouplingModel::solve`] runs one solve per
//! aggressor end.
//!
//! Accuracy (measured, `copper-extract/tests/coupled_microstrip.rs`): within 1 dB of coupled-line theory down to
//! -62 dB of coupling where the theory is exact (TEM lines), the port elements themselves 30 to 40 dB below that;
//! within 1 to 3 dB above -40 dB on FR-4 microstrip, where the static reference leaves out modal dispersion.
//! Screening evidence.

use std::collections::{BTreeMap, BTreeSet};

use copper_pcb::board::PhysicalBoard;
use copper_physical::geometry::point_in_polygon;
use copper_physical::{CopperLayer, Point};
use ems::Complex64;

use crate::geom::{Pt, bbox};
use crate::model::{RegionModel, mm_point, placed_pads, poses};
use crate::solve::RunStats;
use crate::spec::{Budget, Error, Fidelity, PortSpec, RegionSpec, Result, Terminal};
use crate::stack::{Stack, stack};

/// The termination of an end nothing else describes.
pub const DEFAULT_TERMINATION_OHMS: f64 = 50.0;

/// One end of a net: where a port goes.
#[derive(Clone, Debug, PartialEq)]
pub struct NetEnd {
    pub net: String,
    /// `U1.3` for a pad, `NET@(x,y)` (mm) for a free track end.
    pub name: String,
    pub terminal: Terminal,
    pub layer: CopperLayer,
    /// The centre of the end, mm.
    pub at: Pt,
    /// `(component, pad)` when the end is a pad.
    pub pad: Option<(String, String)>,
}

/// The termination chosen for an end, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Termination {
    pub ohms: f64,
    /// `declared output_impedance of U1.3`, `--terminate`, `default 50 ohm`, ...
    pub source: String,
}

impl Termination {
    pub fn default_ohms() -> Termination {
        Termination { ohms: DEFAULT_TERMINATION_OHMS, source: format!("default {DEFAULT_TERMINATION_OHMS} ohm") }
    }
}

/// How each end of a coupling question is terminated, in order: an override by end name (`U1.3`, or a free end's
/// `NET@(x,y)`), then the declared `output_impedance` of the signal's driving pin when the end is a pad of that
/// component on that net, then [`DEFAULT_TERMINATION_OHMS`]. A receiver's input impedance is not declared anywhere
/// in the language, so a receiving end is 50 ohm unless overridden, and says so.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terminations {
    pub overrides: BTreeMap<String, f64>,
    /// `(component, net) -> (ohms, pin)`: the drivers that declare an `output_impedance`.
    pub drivers: BTreeMap<(String, String), (f64, String)>,
}

impl Terminations {
    /// The declared driver impedances of `model`'s signals (each member net of a pair included).
    pub fn from_emi(model: &copper_lang::emi::EmiModel) -> Terminations {
        let mut drivers = BTreeMap::new();
        for source in model.signals.values() {
            let Some(z) = &source.output_impedance else { continue };
            let ohms: f64 = z.value.base_value().decimal_text().parse().unwrap_or(f64::NAN);
            if !(ohms.is_finite() && ohms > 0.0) {
                continue;
            }
            let nets = std::iter::once(source.net.clone())
                .chain(source.partner.clone())
                .chain(source.members.iter().map(|m| m.net.clone()));
            for net in nets {
                drivers.insert((z.from.component.clone(), net), (ohms, z.from.pin.clone()));
            }
        }
        Terminations { overrides: BTreeMap::new(), drivers }
    }

    /// The termination of `end`.
    pub fn choose(&self, end: &NetEnd) -> Termination {
        if let Some(ohms) = self.overrides.get(&end.name) {
            return Termination { ohms: *ohms, source: "--terminate".to_owned() };
        }
        if let Some((component, _)) = &end.pad
            && let Some((ohms, pin)) = self.drivers.get(&(component.clone(), end.net.clone()))
        {
            return Termination { ohms: *ohms, source: format!("declared output_impedance of {component}.{pin}") };
        }
        let mut t = Termination::default_ohms();
        if end.pad.is_some() {
            t.source.push_str(" (no declared impedance)");
        }
        t
    }

    /// Overrides that name no end of `ends`: an error to report, not to ignore.
    pub fn unknown_overrides(&self, ends: &[NetEnd]) -> Vec<String> {
        self.overrides.keys().filter(|name| !ends.iter().any(|e| &e.name == *name)).cloned().collect()
    }
}

/// An end with its port: the plane it returns to and its termination, and (once a model is built) its box.
#[derive(Clone, Debug)]
pub struct EndPort {
    pub end: NetEnd,
    pub plane: CopperLayer,
    pub termination: Termination,
    /// The port's box in the model, mm (`start` on the end's layer, `stop` on the plane).
    pub start: [f64; 3],
    pub stop: [f64; 3],
}

/// The ends of `net`: every placed pad on it, or, when it has none, every track end that no other track or arc of
/// the net shares. A through-hole pad is taken on its top layer.
pub fn net_ends(board: &PhysicalBoard, net: &str) -> Result<Vec<NetEnd>> {
    let stack = stack(board)?;
    let pads = placed_pads(board, &poses(board), &stack);
    let mut ends = Vec::new();
    for pad in pads.iter().filter(|p| p.net.as_deref() == Some(net)) {
        let Some(layer) = pad.layers.first().copied() else { continue };
        let Some(b) = bbox(&pad.ring) else { continue };
        ends.push(NetEnd {
            net: net.to_owned(),
            name: format!("{}.{}", pad.component, pad.number),
            terminal: Terminal::pad(&pad.component, &pad.number),
            layer,
            at: ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0),
            pad: Some((pad.component.clone(), pad.number.clone())),
        });
    }
    if !ends.is_empty() {
        return Ok(ends);
    }
    // No pad: the free ends of the net's tracks.
    let mut points: Vec<(Point, CopperLayer, i64, usize)> = Vec::new();
    let mut touch = |p: Point, layer: CopperLayer, width: i64| match points
        .iter_mut()
        .find(|(q, l, _, _)| *q == p && *l == layer)
    {
        Some(point) => point.3 += 1,
        None => points.push((p, layer, width, 1)),
    };
    for t in board.tracks.iter().filter(|t| t.net == net) {
        touch(t.start, t.layer, t.width_nm);
        touch(t.end, t.layer, t.width_nm);
    }
    for a in board.arcs.iter().filter(|a| a.net == net) {
        touch(a.start, a.layer, a.width_nm);
        touch(a.end, a.layer, a.width_nm);
    }
    let vias: Vec<Point> = board.vias.iter().filter(|v| v.net == net).map(|v| v.position).collect();
    for (p, layer, width, count) in points {
        if count != 1 || vias.contains(&p) {
            continue;
        }
        let h = width / 2;
        let at = mm_point(p);
        ends.push(NetEnd {
            net: net.to_owned(),
            name: format!("{net}@({:.2},{:.2})", at.0, at.1),
            terminal: Terminal::rect(layer, [p.x_nm - h, p.y_nm - h, p.x_nm + h, p.y_nm + h]),
            layer,
            at,
            pad: None,
        });
    }
    if ends.is_empty() {
        return Err(Error::Invalid(format!("net {net} has no placed pad and no free track end for a port")));
    }
    Ok(ends)
}

/// The first copper layer below `end`'s layer on which one of `grounds` is zoned under the end.
pub fn plane_under(
    board: &PhysicalBoard,
    stack: &Stack,
    grounds: &BTreeSet<String>,
    end: &NetEnd,
) -> Option<CopperLayer> {
    let z_end = stack.z(end.layer)?;
    let at = Point::new((end.at.0 * 1e6).round() as i64, (end.at.1 * 1e6).round() as i64);
    stack.copper.iter().filter(|(_, z)| *z < z_end).map(|(layer, _)| *layer).find(|layer| {
        board
            .zones
            .iter()
            .any(|zone| grounds.contains(&zone.net) && zone.layers.contains(layer) && point_in_polygon(at, &zone.outer))
    })
}

/// The nets that have a copper zone: the default ground nets.
pub fn zoned_nets(board: &PhysicalBoard) -> BTreeSet<String> {
    board.zones.iter().map(|z| z.net.clone()).collect()
}

/// The bounding box `[x0, y0, x1, y1]` (nm) of the copper of `nets`: tracks, arcs, vias and pads.
fn copper_bounds(board: &PhysicalBoard, nets: &BTreeSet<String>, ends: &[NetEnd]) -> Option<[i64; 4]> {
    let mut b: Option<[i64; 4]> = None;
    let mut grow = |x: i64, y: i64, r: i64| {
        let r = r.max(0);
        b = Some(match b {
            None => [x - r, y - r, x + r, y + r],
            Some(c) => [c[0].min(x - r), c[1].min(y - r), c[2].max(x + r), c[3].max(y + r)],
        });
    };
    for t in board.tracks.iter().filter(|t| nets.contains(&t.net)) {
        grow(t.start.x_nm, t.start.y_nm, t.width_nm / 2);
        grow(t.end.x_nm, t.end.y_nm, t.width_nm / 2);
    }
    for a in board.arcs.iter().filter(|a| nets.contains(&a.net)) {
        for p in [a.start, a.mid, a.end] {
            grow(p.x_nm, p.y_nm, a.width_nm / 2);
        }
    }
    for v in board.vias.iter().filter(|v| nets.contains(&v.net)) {
        grow(v.position.x_nm, v.position.y_nm, v.size_nm / 2);
    }
    for e in ends {
        grow((e.at.0 * 1e6).round() as i64, (e.at.1 * 1e6).round() as i64, 500_000);
    }
    b
}

/// A port for `end` to the plane under it.
fn end_port(
    board: &PhysicalBoard,
    stack: &Stack,
    grounds: &BTreeSet<String>,
    end: NetEnd,
    termination: Termination,
) -> Result<EndPort> {
    let plane = plane_under(board, stack, grounds, &end).ok_or_else(|| {
        Error::NotSupported(format!(
            "end {} of net {} on {}: no ground zone ({}) under it on a layer below for its port's return",
            end.name,
            end.net,
            end.layer.value(),
            grounds.iter().cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    Ok(EndPort { end, plane, termination, start: [0.0; 3], stop: [0.0; 3] })
}

fn port_spec(port: &EndPort) -> PortSpec {
    PortSpec::to_plane(&port.end.name, port.end.terminal.clone(), port.plane).with_z_ref(port.termination.ohms)
}

/// One net's copper as solver geometry, for a model that is not a region model.
#[derive(Clone, Debug)]
pub struct NetGeometry {
    pub net: String,
    /// PEC sheets `(z, ring)`, mm, as drawn (no edge rule).
    pub sheets: Vec<(f64, Vec<Pt>)>,
    /// Vertical PEC wires `[x, y, z_top, z_bottom, radius]`, mm: vias and plated holes.
    pub wires: Vec<[f64; 5]>,
    /// A port at every end, with its box.
    pub ports: Vec<EndPort>,
    /// What the translator approximated.
    pub approximations: Vec<String>,
}

/// The copper of `net` and a port at each of its ends, returning to the first of `grounds` zoned under the end;
/// `terminate` chooses each end's termination. Translated by [`RegionModel::build`] with only this net and the
/// edge rule off, so the sheets are the copper as drawn.
pub fn net_geometry(
    board: &PhysicalBoard,
    net: &str,
    grounds: &BTreeSet<String>,
    terminate: &dyn Fn(&NetEnd) -> Termination,
) -> Result<NetGeometry> {
    let stack = stack(board)?;
    let ends = net_ends(board, net)?;
    let mut ports = Vec::new();
    for end in ends {
        let termination = terminate(&end);
        ports.push(end_port(board, &stack, grounds, end, termination)?);
    }
    let nets: BTreeSet<String> = [net.to_owned()].into();
    let ends: Vec<NetEnd> = ports.iter().map(|p| p.end.clone()).collect();
    let window =
        copper_bounds(board, &nets, &ends).ok_or_else(|| Error::Invalid(format!("net {net} has no copper")))?;
    let mut spec = RegionSpec::new(window);
    spec.margin_nm = 1_000_000;
    spec.nets = Some(nets);
    spec.include_zones = false;
    spec.edge_inset = 0.0;
    spec.edge_inset_z = 0.0;
    spec.ports = ports.iter().map(port_spec).collect();
    let model = RegionModel::build(board, &spec)?;
    for (port, resolved) in ports.iter_mut().zip(&model.ports) {
        port.start = resolved.start;
        port.stop = resolved.stop;
    }
    Ok(NetGeometry {
        net: net.to_owned(),
        sheets: model.sheets.iter().filter(|s| s.net == net).map(|s| (s.z, s.ring.clone())).collect(),
        wires: model.wires.iter().filter(|w| w.net == net).map(|w| [w.x, w.y, w.z_top, w.z_bottom, w.radius]).collect(),
        ports,
        approximations: model.approximations.clone(),
    })
}

/// A coupling question for the region model.
#[derive(Clone, Debug)]
pub struct CouplingRequest {
    pub aggressor: String,
    pub victims: Vec<String>,
    /// Nets modelled as the return planes (their zones and copper); `None`: every zoned net.
    pub grounds: Option<BTreeSet<String>>,
    pub band_hz: (f64, f64),
    pub fidelity: Fidelity,
    /// Board around the copper of the aggressor and victims, nm.
    pub margin_nm: i64,
    pub budget: Budget,
    pub threads: usize,
    /// Use the native zone fills (otherwise zone outlines minus clearances).
    pub zone_fills: bool,
}

impl CouplingRequest {
    pub fn new(aggressor: &str, victims: &[String]) -> CouplingRequest {
        CouplingRequest {
            aggressor: aggressor.to_owned(),
            victims: victims.to_vec(),
            grounds: None,
            band_hz: (100e6, 3e9),
            fidelity: Fidelity::Coarse,
            margin_nm: 3_000_000,
            budget: Budget::default(),
            threads: 0,
            zone_fills: true,
        }
    }
}

/// The region model of a coupling question: the aggressor's ends first, then each victim's.
pub struct CouplingModel {
    pub model: RegionModel,
    pub ports: Vec<EndPort>,
    /// Indices into `ports` of the aggressor's ends (each is excited in turn).
    pub aggressor_ports: Vec<usize>,
    pub grounds: BTreeSet<String>,
    /// What the model left out or changed, for the report.
    pub notes: Vec<String>,
}

/// The transmission from one excited end to another end.
#[derive(Clone, Debug)]
pub struct CouplingPath {
    pub from: String,
    pub to: String,
    pub to_net: String,
    /// `S_to,from` at each frequency, power waves referenced to each port's termination.
    pub s: Vec<Complex64>,
}

impl CouplingPath {
    pub fn db(&self) -> Vec<f64> {
        self.s.iter().map(|s| 20.0 * s.norm().max(1e-15).log10()).collect()
    }
}

/// What a coupling run returns.
#[derive(Clone, Debug)]
pub struct CouplingResult {
    pub freqs: Vec<f64>,
    pub paths: Vec<CouplingPath>,
    /// `S_ii` of each excited end.
    pub reflections: Vec<(String, Vec<Complex64>)>,
    pub runs: Vec<RunStats>,
}

/// The region model of `request` on `board`; `terminate` chooses each end's termination.
pub fn coupling_model(
    board: &PhysicalBoard,
    request: &CouplingRequest,
    terminate: &dyn Fn(&NetEnd) -> Termination,
) -> Result<CouplingModel> {
    if request.victims.is_empty() {
        return Err(Error::Invalid("a coupling question needs at least one victim net".to_owned()));
    }
    let stack = stack(board)?;
    let signal: BTreeSet<String> =
        std::iter::once(request.aggressor.clone()).chain(request.victims.iter().cloned()).collect();
    let grounds: BTreeSet<String> = request
        .grounds
        .clone()
        .unwrap_or_else(|| zoned_nets(board))
        .into_iter()
        .filter(|n| !signal.contains(n))
        .collect();
    if grounds.is_empty() {
        return Err(Error::Invalid(
            "a coupling question needs a ground net with a copper zone for the ports' return".to_owned(),
        ));
    }
    let mut ports = Vec::new();
    let mut aggressor_ports = Vec::new();
    for net in std::iter::once(&request.aggressor).chain(&request.victims) {
        for end in net_ends(board, net)? {
            if *net == request.aggressor {
                aggressor_ports.push(ports.len());
            }
            let termination = terminate(&end);
            ports.push(end_port(board, &stack, &grounds, end, termination)?);
        }
    }
    let ends: Vec<NetEnd> = ports.iter().map(|p| p.end.clone()).collect();
    let window =
        copper_bounds(board, &signal, &ends).ok_or_else(|| Error::Invalid("the nets have no copper".to_owned()))?;
    // The window and its margin, clipped to the board. Planes are continued into the PML only when every edge of
    // the window is inside the board: past a board edge the continuation would put ground where there is air
    // (around an antenna, the very space it radiates into).
    let m = request.margin_nm;
    let mut region = [window[0] - m, window[1] - m, window[2] + m, window[3] + m];
    let mut notes = Vec::new();
    let outline = &board.outline.vertices;
    if !outline.is_empty() {
        let lo = |f: fn(&Point) -> i64| outline.iter().map(f).min().unwrap_or(0);
        let hi = |f: fn(&Point) -> i64| outline.iter().map(f).max().unwrap_or(0);
        let b = [lo(|p| p.x_nm), lo(|p| p.y_nm), hi(|p| p.x_nm), hi(|p| p.y_nm)];
        region = [region[0].max(b[0]), region[1].max(b[1]), region[2].min(b[2]), region[3].min(b[3])];
        let interior = region[0] > b[0] && region[1] > b[1] && region[2] < b[2] && region[3] < b[3];
        if !interior {
            notes.push(
                "the model reaches a board edge: planes end at the window instead of continuing into the PML (flag planes_truncated)"
                    .to_owned(),
            );
        }
    }
    let interior = notes.is_empty();
    let mut spec = RegionSpec::new(region);
    spec.margin_nm = 0;
    spec.nets = Some(signal.iter().chain(&grounds).cloned().collect());
    spec.band_hz = request.band_hz;
    spec.fidelity = request.fidelity;
    spec.budget = request.budget;
    spec.threads = request.threads;
    spec.extend_planes = interior;
    // A routed board's copper leaves gaps far below the cell between nets: the smoothing mesher grades into them
    // and floors the slivers that would set the time step (as for the hot-loop runs).
    spec.smooth_mesh = true;
    if request.zone_fills {
        spec.zone_fills = crate::fill::zone_fill_set(board).ok();
    }
    spec.ports = ports.iter().map(port_spec).collect();
    let model = RegionModel::build(board, &spec)?;
    for (port, resolved) in ports.iter_mut().zip(&model.ports) {
        port.start = resolved.start;
        port.stop = resolved.stop;
    }
    Ok(CouplingModel { model, ports, aggressor_ports, grounds, notes })
}

impl CouplingModel {
    /// One solve per aggressor end; every other end's transmission from it.
    pub fn solve(&self) -> Result<CouplingResult> {
        let mut paths = Vec::new();
        let mut reflections = Vec::new();
        let mut runs = Vec::new();
        let mut freqs = Vec::new();
        for &k in &self.aggressor_ports {
            let column = self.model.solve_port(k)?;
            freqs = column.freqs.clone();
            let from = &self.ports[k].end;
            for (i, port) in self.ports.iter().enumerate() {
                if i == k {
                    reflections.push((from.name.clone(), column.s[i].clone()));
                } else {
                    paths.push(CouplingPath {
                        from: from.name.clone(),
                        to: port.end.name.clone(),
                        to_net: port.end.net.clone(),
                        s: column.s[i].clone(),
                    });
                }
            }
            runs.push(column.stats);
        }
        Ok(CouplingResult { freqs, paths, reflections, runs })
    }
}

/// `|S|` in dB of the largest coupling of `path` over `lo..=hi` Hz, and the frequency.
pub fn peak_in(freqs: &[f64], s: &[Complex64], lo: f64, hi: f64) -> Option<(f64, f64)> {
    freqs
        .iter()
        .zip(s)
        .filter(|(f, _)| **f >= lo && **f <= hi)
        .map(|(f, s)| (*f, 20.0 * s.norm().max(1e-15).log10()))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::*;

    const MM: i64 = 1_000_000;

    fn two_lines() -> PhysicalBoard {
        let mut board = bare_board();
        declare_stackup(
            &mut board,
            &[
                (Some(CopperLayer::Front), 35_000, "", ""),
                (None, 500_000, "4.3", "0.0"),
                (Some(CopperLayer::Back), 35_000, "", ""),
            ],
        );
        board.tracks.push(track("A", (20 * MM, 50 * MM), (40 * MM, 50 * MM), MM, CopperLayer::Front));
        // A bent victim: a 45-degree jog, which the prototype's axis-aligned victims could not take.
        board.tracks.push(track("B", (20 * MM, 53 * MM), (30 * MM, 53 * MM), MM, CopperLayer::Front));
        board.tracks.push(track("B", (30 * MM, 53 * MM), (33 * MM, 56 * MM), MM, CopperLayer::Front));
        board.tracks.push(track("B", (33 * MM, 56 * MM), (40 * MM, 56 * MM), MM, CopperLayer::Front));
        board.zones.push(zone("GND", &[CopperLayer::Back], [0, 0, 100 * MM, 100 * MM]));
        board
    }

    #[test]
    fn terminations_prefer_an_override_then_a_declared_driver_then_fifty_ohm() {
        let end = |name: &str, pad: Option<(&str, &str)>| NetEnd {
            net: "CLK".to_owned(),
            name: name.to_owned(),
            terminal: Terminal::rect(CopperLayer::Front, [0, 0, 1, 1]),
            layer: CopperLayer::Front,
            at: (0.0, 0.0),
            pad: pad.map(|(c, p)| (c.to_owned(), p.to_owned())),
        };
        let mut t = Terminations::default();
        t.drivers.insert(("U1".to_owned(), "CLK".to_owned()), (33.0, "CLKOUT".to_owned()));
        t.overrides.insert("U2.4".to_owned(), 1000.0);
        let driver = t.choose(&end("U1.7", Some(("U1", "7"))));
        assert_eq!(driver.ohms, 33.0);
        assert_eq!(driver.source, "declared output_impedance of U1.CLKOUT");
        let overridden = t.choose(&end("U2.4", Some(("U2", "4"))));
        assert_eq!((overridden.ohms, overridden.source.as_str()), (1000.0, "--terminate"));
        let receiver = t.choose(&end("U3.1", Some(("U3", "1"))));
        assert_eq!(receiver.ohms, DEFAULT_TERMINATION_OHMS);
        assert!(receiver.source.contains("no declared impedance"), "{}", receiver.source);
        assert_eq!(t.unknown_overrides(&[end("U1.7", None)]), ["U2.4".to_owned()]);
    }

    #[test]
    fn free_track_ends_are_the_ends_of_a_net_without_pads() {
        let board = two_lines();
        let ends = net_ends(&board, "B").expect("ends");
        let names: Vec<&str> = ends.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["B@(20.00,53.00)", "B@(40.00,56.00)"]);
        let grounds = zoned_nets(&board);
        let stack = stack(&board).expect("stack");
        assert_eq!(plane_under(&board, &stack, &grounds, &ends[0]), Some(CopperLayer::Back));
    }

    #[test]
    fn net_geometry_translates_an_angled_victim_as_drawn() {
        let board = two_lines();
        let g = net_geometry(&board, "B", &zoned_nets(&board), &|_| Termination::default_ohms()).expect("geometry");
        assert_eq!(g.ports.len(), 2);
        assert!(g.sheets.len() >= 3, "{} sheets", g.sheets.len());
        assert!(g.sheets.iter().all(|(z, _)| *z == 0.0));
        // The jog is a rotated rectangle: a sheet with a vertex off the axis-aligned bounding lines.
        let rotated = g.sheets.iter().any(|(_, ring)| {
            let b = bbox(ring).expect("ring");
            ring.iter().any(|p| (p.0 - b[0]).abs() > 1e-6 && (p.0 - b[2]).abs() > 1e-6)
        });
        assert!(rotated, "no rotated sheet");
        for p in &g.ports {
            assert!((p.start[2] - 0.0).abs() < 1e-9 && p.stop[2] < -0.4, "{:?} {:?}", p.start, p.stop);
        }
    }

    #[test]
    fn a_coupling_model_ports_the_aggressor_first_and_returns_to_the_zoned_plane() {
        let board = two_lines();
        // The synthetic board declares no nets, which the KiCad export behind the native fill needs.
        let request = CouplingRequest { zone_fills: false, ..CouplingRequest::new("A", &["B".to_owned()]) };
        let terminate = |end: &NetEnd| {
            if end.name.starts_with("A@(20") {
                Termination { ohms: 25.0, source: "test".to_owned() }
            } else {
                Termination::default_ohms()
            }
        };
        let m = coupling_model(&board, &request, &terminate).expect("model");
        assert_eq!(m.aggressor_ports, vec![0, 1]);
        assert_eq!(m.ports.len(), 4);
        assert_eq!(m.grounds, ["GND".to_owned()].into());
        assert_eq!(m.model.ports[0].spec.z_ref, 25.0);
        // The synthetic board has no outline: nothing to clip, the planes continue.
        assert!(m.model.spec.extend_planes);
    }
}

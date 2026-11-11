//! The hot loop of a buck cell as a full-wave problem.
//!
//! Given an `EMI-HOT-LOOP` finding of `copper emi-check` (the loop's pads in
//! order, the nets, the closed-form inductance and, on a routed board, which
//! legs follow copper), [`hot_loop_spec`] builds the region: a window around
//! the loop, the copper of the loop nets and of every plane under it, the
//! hot capacitor and the catch diode replaced by copper (bridges), the
//! unrouted legs closed with the straight traces the closed form assumes,
//! and one lumped port where the controller's switch is.
//!
//! **Where the port goes.** The loop current runs through the controller's
//! internal switch; the pad pair that brackets it is the port. An
//! asynchronous buck (loop `C.supply, hot supply, switch, D.switch side,
//! D.rail side, C.return`): the port is across the hot-supply pad and the
//! switch pad (the high-side switch), and the capacitor and the diode are
//! shorted. A synchronous buck (`C.supply, hot supply, hot return,
//! C.return`): both switches conduct in turn through the package, and the
//! port is across the hot-supply and hot-return pads, the capacitor shorted.
//! The inductance seen at the port is then the loop's partial inductance
//! without the switch, the capacitor and the diode (their ESL and the
//! package are not included), the quantity `emi-check` computes from the
//! pad-centre polygon. Other topologies return `Error::NotSupported`.

use std::collections::BTreeSet;
use std::time::Instant;

use copper_drc::emi::EmiFinding;
use copper_json::Value;
use copper_pcb::board::PhysicalBoard;
use copper_physical::CopperLayer;

use crate::geom::{self, nm_to_mm};
use crate::model::{RegionModel, placed_pads};
use crate::spec::{Bridge, Budget, Error, ExtraTrace, Fidelity, PortSpec, RegionSpec, Result, Terminal};
use crate::stack::stack;

/// Lowest and highest frequency of the hot-loop analysis: 30 MHz to 1 GHz,
/// which spans the loop inductance's flat region and the radiated band's low end.
pub const BAND_HZ: (f64, f64) = (30e6, 1e9);
/// The 3:1 range over which the inductance is read: 50 to 150 MHz.
pub const READ_HZ: (f64, f64) = (50e6, 150e6);

/// One leg of the finding, as the report lists it for a routed board.
#[derive(Clone, Debug, PartialEq)]
pub struct LegInfo {
    pub from: String,
    pub to: String,
    pub kind: String,
}

/// What the full-wave step needs from an `EMI-HOT-LOOP` finding.
#[derive(Clone, Debug)]
pub struct HotLoopInput {
    /// The controller's reference.
    pub cell: String,
    pub topology: String,
    pub capacitor: String,
    /// The loop's pads in order, `(component, pad)`.
    pub pads: Vec<(String, String)>,
    /// The labels the finding gives them (`"hot supply U1.1"`).
    pub labels: Vec<String>,
    pub nets: Vec<String>,
    pub closed_form_nh: Option<f64>,
    pub legs: Vec<LegInfo>,
    pub trace_status: String,
    /// The loop polygon of the finding, millimetres.
    pub polygon_mm: Vec<[f64; 2]>,
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => i.to_string().parse().ok(),
        _ => None,
    }
}

impl HotLoopInput {
    /// Read the finding's measures. `topology` is the cell's topology
    /// (`buck`, or empty for an unknown one, which `emi-check` treats as a buck).
    pub fn from_finding(finding: &EmiFinding, topology: &str) -> Result<HotLoopInput> {
        let measures = &finding.measures;
        let bad = |what: &str| Error::Invalid(format!("EMI-HOT-LOOP finding: {what}"));
        let labels: Vec<String> = measures
            .get("pad_sequence")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("no pad_sequence"))?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let pads = labels
            .iter()
            .map(|label| {
                let last = label.split_whitespace().last().unwrap_or("");
                last.rsplit_once('.')
                    .map(|(c, p)| (c.to_owned(), p.to_owned()))
                    .ok_or_else(|| bad(&format!("cannot read a pad from '{label}'")))
            })
            .collect::<Result<Vec<_>>>()?;
        let capacitor = measures.get("loop_capacitor").and_then(Value::as_str).unwrap_or_default().to_owned();
        let legs = measures
            .get("legs")
            .and_then(Value::as_array)
            .map(|legs| {
                legs.iter()
                    .map(|l| LegInfo {
                        from: l.get("from").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        to: l.get("to").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        kind: l.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let polygon_mm = measures
            .get("loop_polygon_mm")
            .and_then(Value::as_array)
            .map(|p| {
                p.iter()
                    .filter_map(|pt| {
                        let a = pt.as_array()?;
                        Some([number(a.first()?)?, number(a.get(1)?)?])
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(HotLoopInput {
            cell: finding.cell.clone().unwrap_or_default(),
            topology: if topology.is_empty() { "buck".to_owned() } else { topology.to_owned() },
            capacitor,
            pads,
            labels,
            nets: finding.finding.nets.clone(),
            closed_form_nh: measures.get("loop_inductance_nh").and_then(number),
            legs,
            trace_status: measures.get("trace_status").and_then(Value::as_str).unwrap_or("no_copper").to_owned(),
            polygon_mm,
        })
    }
}

/// What the caller may choose.
#[derive(Clone, Debug)]
pub struct HotLoopOptions {
    pub fidelity: Fidelity,
    /// Window margin around the loop's bounding box, nm; at least ten dielectric heights.
    pub margin_nm: i64,
    pub budget: Budget,
    pub threads: usize,
    pub extend_planes: bool,
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            fidelity: Fidelity::Coarse,
            margin_nm: 5_000_000,
            budget: Budget::default(),
            threads: 0,
            extend_planes: false,
        }
    }
}

/// The result for a report.
#[derive(Clone, Debug)]
pub struct HotLoopResult {
    pub cell: String,
    pub inductance_nh: f64,
    pub closed_form_nh: Option<f64>,
    pub ratio: Option<f64>,
    pub band_mhz: [f64; 2],
    /// `(max - min)/mean` of `L(f)` over the band.
    pub flatness: f64,
    /// `L` read at 450-550 MHz, for the rise of `L(f)` towards the loop's resonance.
    pub inductance_500mhz_nh: Option<f64>,
    pub resonance_mhz: Option<f64>,
    pub resistance_ohm: Option<f64>,
    pub cells: usize,
    pub steps: u64,
    pub seconds: f64,
    pub converged: bool,
    pub fidelity: Fidelity,
    pub margin_mm: f64,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    pub solver: String,
    pub description: String,
    pub port: String,
}

fn invalid(m: &str) -> Error {
    Error::Invalid(m.to_owned())
}

/// The region of the hot loop: window, nets, bridges, added traces and the port.
pub fn hot_loop_spec(
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &HotLoopOptions,
) -> Result<(RegionSpec, Vec<String>)> {
    if input.topology != "buck" {
        return Err(Error::NotSupported(format!(
            "the full-wave hot-loop extraction is defined for buck cells (asynchronous and synchronous); the {} topology has no port definition",
            input.topology
        )));
    }
    let n = input.pads.len();
    // Asynchronous: 0 C.supply, 1 hot supply, 2 switch, 3 D.switch side, 4 D.rail side, 5 C.return.
    // Synchronous: 0 C.supply, 1 hot supply, 2 hot return, 3 C.return.
    type Pairs = Vec<(usize, usize)>;
    let (port, bridges, legs): ((usize, usize), Pairs, Pairs) = match n {
        6 => ((1, 2), vec![(3, 4), (5, 0)], vec![(0, 1), (2, 3), (4, 5)]),
        4 => ((1, 2), vec![(3, 0)], vec![(0, 1), (2, 3)]),
        _ => {
            return Err(Error::NotSupported(format!(
                "a hot loop of {n} pads has no port definition (a buck loop has 4 or 6)"
            )));
        }
    };
    let stack = stack(board)?;
    let all = placed_pads(board, &crate::model::poses(board), &stack);
    let find = |k: usize| {
        let (c, p) = &input.pads[k];
        all.iter()
            .find(|pad| &pad.component == c && &pad.number == p)
            .ok_or_else(|| Error::Invalid(format!("pad {c}.{p} of the loop is not placed on the board")))
    };
    let placed: Vec<_> = (0..n).map(find).collect::<Result<_>>()?;
    let layer: CopperLayer = *placed[0].layers.first().ok_or_else(|| invalid("a loop pad has no layer"))?;
    if placed.iter().any(|p| p.layers.first() != Some(&layer)) {
        return Err(Error::NotSupported(
            "the loop's pads are on both board sides; the in-plane port and bridges need one layer".to_owned(),
        ));
    }
    // Window: the pads' outlines and the loop polygon.
    let mut points: Vec<geom::Pt> = placed.iter().flat_map(|p| p.ring.iter().copied()).collect();
    points.extend(input.polygon_mm.iter().map(|p| (p[0], p[1])));
    let bb = geom::bbox(&points).ok_or_else(|| invalid("the loop has no geometry"))?;
    let to_nm = |mm: f64| (mm * 1e6).round() as i64;
    let tight = [to_nm(bb[0]), to_nm(bb[1]), to_nm(bb[2]), to_nm(bb[3])];
    // Planes: zones on other layers that overlap the loop; the nearest sets the dielectric height.
    let z_pad = stack.z(layer).unwrap_or(0.0);
    let overlaps_window = |zone: &copper_pcb::board::CopperZone| {
        let ring: Vec<geom::Pt> = zone.outer.iter().map(|p| crate::model::mm_point(*p)).collect();
        crate::model::overlaps(&ring, bb)
    };
    let mut nets: BTreeSet<String> = input.nets.iter().cloned().collect();
    let mut height = f64::INFINITY;
    for zone in board.zones.iter().filter(|z| overlaps_window(z)) {
        nets.insert(zone.net.clone());
        for l in &zone.layers {
            if *l != layer
                && let Some(z) = stack.z(*l)
            {
                height = height.min((z_pad - z).abs());
            }
        }
    }
    let height_nm = if height.is_finite() { to_nm(height) } else { to_nm(1.0) };
    let margin = options.margin_nm.max(10 * height_nm);
    let mut spec = RegionSpec::new(tight);
    spec.margin_nm = margin;
    spec.nets = Some(nets);
    spec.band_hz = BAND_HZ;
    spec.fidelity = options.fidelity;
    spec.threads = options.threads;
    spec.budget = options.budget;
    spec.extend_planes = options.extend_planes;
    let term = |k: usize| Terminal::pad(&input.pads[k].0, &input.pads[k].1);
    spec.ports = vec![PortSpec::gap(&format!("{}-switch", input.cell), term(port.0), term(port.1))];
    for (a, b) in bridges {
        let name = if a == 5 || (n == 4 && a == 3) { input.capacitor.clone() } else { input.pads[a].0.clone() };
        spec.bridges.push(Bridge { name, a: term(a), b: term(b) });
    }
    // Copper legs without routed copper get the straight trace of the closed form.
    let mut notes = Vec::new();
    for (a, b) in legs {
        let (from, to) = (&input.labels[a], &input.labels[(a + 1) % n]);
        let routed = input.legs.iter().any(|l| &l.from == from && &l.to == to && l.kind == "traced");
        if routed {
            continue;
        }
        let centre = |p: &crate::model::PlacedPad| {
            let bb = geom::bbox(&p.ring).expect("ring");
            ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0)
        };
        let (ca, cb) = (centre(placed[a]), centre(placed[b]));
        // The closed form's bar width: the smaller pad dimension clamped to 0.2 to 1 mm.
        let width = placed[a].min_dimension_mm.min(placed[b].min_dimension_mm).clamp(0.2, 1.0);
        let net = placed[a].net.clone().ok_or_else(|| invalid("a loop pad has no net"))?;
        spec.extra_traces.push(ExtraTrace {
            name: format!("{}-{}", input.pads[a].0, input.pads[b].0),
            net,
            layer,
            points: vec![[to_nm(ca.0), to_nm(ca.1)], [to_nm(cb.0), to_nm(cb.1)]],
            width_nm: to_nm(width),
        });
        notes.push(format!(
            "leg {} to {} has no routed copper: a straight {:.2} mm trace joins the pad centres, as in the closed form",
            input.labels[a], input.labels[b], width
        ));
    }
    let _ = nm_to_mm(0);
    Ok((spec, notes))
}

/// Build the region of the loop and extract its inductance with one port run.
pub fn extract_hot_loop(
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &HotLoopOptions,
) -> Result<HotLoopResult> {
    let started = Instant::now();
    let (spec, mut notes) = hot_loop_spec(board, input, options)?;
    let margin_mm = nm_to_mm(spec.margin_nm);
    let model = RegionModel::build(board, &spec)?;
    let column = model.solve_port(0).map_err(|error| match error {
        // A refused run says what the model was, so the mesh can be judged.
        Error::Budget(message) => Error::Budget(format!("{message}. Model: {}", model.describe().replace('\n', " | "))),
        other => other,
    })?;
    let (lo, hi) = READ_HZ;
    let (l, spread) = column
        .inductance_band(lo, hi)
        .ok_or_else(|| Error::Solver("no analysis frequency in the reading band".to_owned()))?;
    let l500 = column.inductance_band(450e6, 550e6).map(|v| v.0);
    let resonance = column.first_resonance_hz();
    let resistance = column.resistance_band(lo, hi);
    let mut flags = model.flags.clone();
    if !column.stats.converged {
        flags.push("energy_not_converged".to_owned());
    }
    if spread > 0.05 {
        flags.push("inductance_not_flat".to_owned());
    }
    if l <= 0.0 {
        flags.push("not_inductive".to_owned());
    }
    flags.push("components_bridged".to_owned());
    if !input.trace_status.starts_with("traced") {
        flags.push("placement_estimate".to_owned());
    }
    notes.extend(model.approximations.iter().cloned());
    // The model is PEC: Re(Z) is numerical noise unless something dissipates; report it only when it is a stable value.
    let resistance_ohm = resistance.filter(|(r, dev)| r.abs() > 1e-3 && *dev < 0.1).map(|(r, _)| r);
    Ok(HotLoopResult {
        cell: input.cell.clone(),
        inductance_nh: l * 1e9,
        closed_form_nh: input.closed_form_nh,
        ratio: input.closed_form_nh.map(|c| l * 1e9 / c),
        band_mhz: [lo * 1e-6, hi * 1e-6],
        flatness: spread,
        inductance_500mhz_nh: l500.map(|v| v * 1e9),
        resonance_mhz: resonance.map(|f| f * 1e-6),
        resistance_ohm,
        cells: model.mesh.cells,
        steps: column.stats.steps,
        seconds: started.elapsed().as_secs_f64(),
        converged: column.stats.converged,
        fidelity: options.fidelity,
        margin_mm,
        flags,
        notes,
        solver: crate::solve::SOLVER.to_owned(),
        description: model.describe(),
        port: model.ports.first().map(|p| p.description.clone()).unwrap_or_default(),
    })
}

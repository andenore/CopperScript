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
use crate::solve::PortColumn;
use crate::spec::{Bridge, Budget, Error, ExtraTrace, Fidelity, PortSpec, RefineBox, RegionSpec, Result, Terminal};
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
    /// The controller's switch-node pad nearest the hot supply pad, for a synchronous cell
    /// (the finding's pad sequence of a four-pad loop does not contain it).
    pub switch_pad: Option<(String, String)>,
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
            switch_pad: None,
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
    /// Refine the mesh around the port, the bridges and the loop's pads (default on).
    pub refine: bool,
    /// The refined cell, nm; `None` takes a quarter of the narrowest loop pad, between 0.2 and 0.25 mm.
    pub refine_cell_nm: Option<i64>,
    /// Multiplier of the refined cell (1/1.5 for the second mesh of a convergence estimate).
    pub refine_scale: f64,
    /// Run a second mesh for a convergence estimate when the budget allows (default on).
    pub convergence: bool,
    /// Scale of the excitation pulse's bandwidth (see `RegionSpec::pulse_scale`).
    pub pulse_scale: f64,
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            fidelity: Fidelity::Coarse,
            margin_nm: 5_000_000,
            budget: Budget::default(),
            threads: 0,
            extend_planes: false,
            refine: true,
            refine_cell_nm: None,
            refine_scale: 1.0,
            convergence: true,
            pulse_scale: 1.0,
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
    /// Relative difference of the inductances of the two meshes, percent; `None` for a single mesh.
    pub mesh_error_estimate_pct: Option<f64>,
    /// The meshes that were run, standard first.
    pub meshes: Vec<MeshRun>,
    /// Smallest cell (mm, over the three axes), largest ratio of neighbouring cells and the number of
    /// pairs above 1.5 on the mesh the inductance comes from.
    pub smallest_cell_mm: f64,
    pub max_grading_ratio: f64,
    pub ratio_violations: usize,
    /// The closed-form inductance of the straps that join a port to pads that do not face it, nH.
    pub port_strap_nh: Option<f64>,
    /// L(f) divided by the resonance factor, `L(f) (1 - (f/f_res)^2)`, over the band, nH: the static
    /// inductance of a loop that resonates not far above the band; `None` without a resonance below 1 GHz.
    pub inductance_static_nh: Option<f64>,
    pub flatness_static: Option<f64>,
    /// First-order extrapolation of the standard and refined meshes to a zero refinement cell,
    /// `L2 + (L2 - L1)/(r - 1)` with r = 1.5, nH; only with a refined second mesh and a falling L.
    /// Reported next to `inductance_nh`, never in its place (see the validation section).
    pub inductance_extrapolated_nh: Option<f64>,
}

/// The first-order (Richardson, p = 1) value for a zero cell from L1 on a cell h and L2 on h / ratio.
/// `None` unless L falls with the cell, the only case in which a first-order law has been observed.
pub fn extrapolate_first_order(l_standard: f64, l_refined: f64, ratio: f64) -> Option<f64> {
    (ratio > 1.0 && l_refined < l_standard && l_refined > 0.0)
        .then(|| l_refined + (l_refined - l_standard) / (ratio - 1.0))
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
    // Synchronous with a known switch pad (index 4): the port is across the high-side switch (hot supply
    // pad to switch pad), the low-side switch is a bridge from the switch pad to the hot return pad.
    type Pairs = Vec<(usize, usize)>;
    let mut pad_ids: Vec<(String, String)> = input.pads.clone();
    let (port, bridges, legs): ((usize, usize), Pairs, Pairs) = match n {
        6 => ((1, 2), vec![(3, 4), (5, 0)], vec![(0, 1), (2, 3), (4, 5)]),
        4 if input.switch_pad.is_some() => {
            pad_ids.extend(input.switch_pad.clone());
            ((1, 4), vec![(4, 2), (3, 0)], vec![(0, 1), (2, 3)])
        }
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
        let (c, p) = &pad_ids[k];
        all.iter()
            .find(|pad| &pad.component == c && &pad.number == p)
            .ok_or_else(|| Error::Invalid(format!("pad {c}.{p} of the loop is not placed on the board")))
    };
    let placed: Vec<_> = (0..pad_ids.len()).map(find).collect::<Result<_>>()?;
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
    spec.refine_scale = options.refine_scale;
    spec.smooth_mesh = true;
    spec.pulse_scale = options.pulse_scale;
    if options.refine {
        // A quarter of the narrowest loop pad, 0.2 to 0.25 mm: four cells across the pad, two across the finest leg.
        let narrowest = placed.iter().map(|p| p.min_dimension_mm).fold(f64::INFINITY, f64::min);
        let cell_nm = options.refine_cell_nm.unwrap_or_else(|| to_nm((narrowest / 4.0).clamp(0.2, 0.25)));
        let around = |name: String, rings: &[&Vec<geom::Pt>]| {
            let pts: Vec<geom::Pt> = rings.iter().flat_map(|r| r.iter().copied()).collect();
            geom::bbox(&pts).map(|b| RefineBox {
                name,
                rect_nm: [to_nm(b[0] - 0.3), to_nm(b[1] - 0.3), to_nm(b[2] + 0.3), to_nm(b[3] + 0.3)],
                cell_nm,
            })
        };
        let mut boxes: Vec<RefineBox> = Vec::new();
        // A compact loop (up to 200 mm2 of window) is refined as a whole: its legs are part of the answer.
        // A large routed loop is refined around the port, the bridges and the pads only.
        let loop_box = RefineBox {
            name: "loop".to_owned(),
            rect_nm: [tight[0] - to_nm(0.3), tight[1] - to_nm(0.3), tight[2] + to_nm(0.3), tight[3] + to_nm(0.3)],
            cell_nm,
        };
        let area_mm2 = (bb[2] - bb[0] + 0.6) * (bb[3] - bb[1] + 0.6);
        if area_mm2 <= 200.0 {
            boxes.push(loop_box);
        } else {
            boxes.extend(around(format!("{}-switch port", input.cell), &[&placed[port.0].ring, &placed[port.1].ring]));
            for &(a, b) in &bridges {
                boxes.extend(around(
                    format!("bridge {}-{}", pad_ids[a].0, pad_ids[b].0),
                    &[&placed[a].ring, &placed[b].ring],
                ));
            }
            for (k, p) in placed.iter().enumerate() {
                boxes.extend(around(format!("pad {}.{}", pad_ids[k].0, pad_ids[k].1), &[&p.ring]));
            }
        }
        spec.refine = boxes;
    }
    let term = |k: usize| Terminal::pad(&pad_ids[k].0, &pad_ids[k].1);
    spec.ports = vec![PortSpec::gap(&format!("{}-switch", input.cell), term(port.0), term(port.1))];
    for (a, b) in bridges {
        let name = if a == 5 || (n == 4 && a == 3) { input.capacitor.clone() } else { pad_ids[a].0.clone() };
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
            name: format!("{}-{}", pad_ids[a].0, pad_ids[b].0),
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

/// One solver run of the extraction: which mesh, what it cost, what it gave.
#[derive(Clone, Debug)]
pub struct MeshRun {
    /// `standard`, `refined` (the refinement cells divided by 1.5) or `coarser` (multiplied by 1.5).
    pub role: &'static str,
    /// The cell in the refinement zones, millimetres (the general cell where there are none).
    pub refine_cell_mm: f64,
    pub cells: usize,
    pub steps: u64,
    pub expected_steps: u64,
    pub seconds: f64,
    pub converged: bool,
    pub inductance_nh: f64,
}

fn run_model(model: &RegionModel) -> Result<(PortColumn, f64, f64)> {
    let column = model.solve_port(0).map_err(|error| match error {
        // A refused run says what the model was, so the mesh can be judged.
        Error::Budget(message) => Error::Budget(format!("{message}. Model: {}", model.describe().replace('\n', " | "))),
        other => other,
    })?;
    let (lo, hi) = READ_HZ;
    let (l, spread) = column
        .inductance_band(lo, hi)
        .ok_or_else(|| Error::Solver("no analysis frequency in the reading band".to_owned()))?;
    Ok((column, l, spread))
}

fn mesh_run(role: &'static str, model: &RegionModel, column: &PortColumn, l: f64) -> MeshRun {
    let base = model.mesh.copper_cell_mm / model.spec.refine_scale.max(1.0);
    let refine_cell_mm = model
        .spec
        .refine
        .iter()
        .map(|r| nm_to_mm(r.cell_nm).min(base) * model.spec.refine_scale)
        .fold(model.mesh.copper_cell_mm, f64::min);
    MeshRun {
        role,
        refine_cell_mm,
        cells: model.mesh.cells,
        steps: column.stats.steps,
        expected_steps: column.stats.expected_steps,
        seconds: column.stats.seconds,
        converged: column.stats.converged,
        inductance_nh: l * 1e9,
    }
}

/// The inductance, nH, of the port straps over the nearest plane (closed form, partial inductances with
/// the plane's image) or in free space when the region has no plane below the straps' layer.
pub fn strap_inductance_nh(board: &PhysicalBoard, model: &RegionModel) -> Option<f64> {
    use copper_emi::inductance::{Segment, loop_inductance, loop_inductance_over_plane};
    let first = model.straps.first()?;
    let z_strap = first.z;
    let mut height = f64::INFINITY;
    for zone in board.zones.iter().filter(|z| z.layers.iter().all(|l| *l != first.layer)) {
        let ring: Vec<geom::Pt> = zone.outer.iter().map(|p| crate::model::mm_point(*p)).collect();
        if crate::model::overlaps(&ring, model.region) {
            for l in &zone.layers {
                if let Some(z) = model.stack.z(*l)
                    && z < z_strap
                {
                    height = height.min(z_strap - z);
                }
            }
        }
    }
    let mm = 1e-3;
    let h = if height.is_finite() { height * mm } else { 1.0 };
    let segments: Vec<Segment> = model
        .straps
        .iter()
        .map(|s| Segment {
            start: [s.from.0 * mm, s.from.1 * mm, h],
            end: [s.to.0 * mm, s.to.1 * mm, h],
            width: s.width * mm,
            thickness: 35e-6,
        })
        .collect();
    let l = if height.is_finite() { loop_inductance_over_plane(&segments, 0.0) } else { loop_inductance(&segments) };
    l.is_finite().then_some(l * 1e9)
}

/// Build the region of the loop and extract its inductance. With `options.convergence` and
/// refinement zones, a second mesh follows when the budget allows: the refinement cells divided by
/// 1.5, or if that does not fit, multiplied by 1.5 (cheaper). The inductance reported is that of the
/// finer of the two meshes, with their relative difference as `mesh_error_estimate_pct`.
pub fn extract_hot_loop(
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &HotLoopOptions,
) -> Result<HotLoopResult> {
    let started = Instant::now();
    let (spec, mut notes) = hot_loop_spec(board, input, options)?;
    let margin_mm = nm_to_mm(spec.margin_nm);
    let model = RegionModel::build(board, &spec)?;
    let (column, l1, spread1) = run_model(&model)?;
    let mut runs = vec![mesh_run("standard", &model, &column, l1)];
    let (mut reported, mut reported_model, mut spread) = (l1, model, spread1);
    let mut reported_column = column;
    let mut error_pct: Option<f64> = None;
    let mut extrapolated: Option<f64> = None;
    let mut pair_note: Option<String> = None;
    if options.convergence && !spec.refine.is_empty() {
        let rate = {
            let r = &runs[0];
            (r.cells as f64 * r.steps as f64 / reported_column.stats.loop_seconds.max(1e-3)).max(1e6)
        };
        for (role, factor) in [("refined", 1.0 / 1.5), ("coarser", 1.5)] {
            let remaining = options.budget.max_seconds - started.elapsed().as_secs_f64();
            if remaining <= 0.0 {
                break;
            }
            let mut other = options.clone();
            other.refine_scale = options.refine_scale * factor;
            let (spec2, _) = hot_loop_spec(board, input, &other)?;
            let mut model2 = RegionModel::build(board, &spec2)?;
            // The first run measured the update rate; the second is held to what is left of the budget.
            model2.spec.budget.max_seconds = remaining;
            model2.spec.budget.assumed_rate = rate;
            if model2.mesh.lines == reported_model.mesh.lines && model2.axes[0].lines == reported_model.axes[0].lines {
                pair_note = Some(format!("the {role} mesh is the standard mesh"));
                continue;
            }
            let e2 = model2.estimate();
            if e2.seconds > remaining || e2.cell_updates > options.budget.max_cell_updates {
                pair_note = Some(format!(
                    "the {role} mesh was estimated at {:.0} s against {:.0} s left of the budget and was not run",
                    e2.seconds, remaining
                ));
                continue;
            }
            match run_model(&model2) {
                Ok((column2, l2, spread2)) => {
                    runs.push(mesh_run(role, &model2, &column2, l2));
                    if role == "refined" {
                        error_pct = Some(100.0 * (l2 - l1).abs() / l2);
                        extrapolated = extrapolate_first_order(l1, l2, 1.5);
                        (reported, reported_model, reported_column, spread) = (l2, model2, column2, spread2);
                    } else {
                        error_pct = Some(100.0 * (l1 - l2).abs() / l1);
                    }
                    pair_note = None;
                    break;
                }
                Err(Error::Budget(m)) => {
                    pair_note = Some(format!("the {role} mesh was refused: {m}"));
                }
                Err(Error::BudgetExceeded(partial)) => {
                    pair_note = Some(format!("the {role} mesh was stopped: {partial}"));
                }
                Err(other) => return Err(other),
            }
        }
    }
    let (model, column, l) = (reported_model, reported_column, reported);
    let (lo, hi) = READ_HZ;
    let l500 = column.inductance_band(450e6, 550e6).map(|v| v.0);
    let resonance = column.first_resonance_hz();
    let static_l = resonance.and_then(|f| {
        let z = &column.z_in;
        crate::solve::static_inductance(&column.freqs, z, f, lo, hi)
    });
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
    // The 3:1 reading band assumes a first resonance far above it; one under five times its top lifts L(f).
    if resonance.is_some_and(|f| f < 5.0 * hi) {
        flags.push("band_near_resonance".to_owned());
        notes.push(format!(
            "the loop resonates at {:.0} MHz, near the 50-150 MHz reading band: L(f) rises across it, and the static inductance L(f) (1 - (f/f_res)^2) is reported too",
            resonance.unwrap_or(0.0) * 1e-6
        ));
    }
    if error_pct.is_none() {
        flags.push("single_mesh".to_owned());
        if let Some(n) = &pair_note {
            notes.push(n.clone());
        } else if !options.convergence {
            notes.push("the convergence estimate was switched off: one mesh, no mesh error estimate".to_owned());
        } else if spec.refine.is_empty() {
            notes.push("no refinement zones: one mesh, no mesh error estimate".to_owned());
        }
    }
    flags.push("components_bridged".to_owned());
    if !input.trace_status.starts_with("traced") {
        flags.push("placement_estimate".to_owned());
    }
    let port_strap_nh = strap_inductance_nh(board, &model);
    if let Some(l) = port_strap_nh {
        notes.push(format!(
            "the port is joined to its pads by straps of about {l:.2} nH (closed form): a known bias that the inductance includes"
        ));
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
        mesh_error_estimate_pct: error_pct,
        meshes: runs,
        smallest_cell_mm: model.mesh.smallest.iter().copied().fold(f64::INFINITY, f64::min),
        max_grading_ratio: model.mesh.max_ratio,
        ratio_violations: model.mesh.ratio_violations,
        port_strap_nh,
        inductance_extrapolated_nh: extrapolated.map(|v| v * 1e9),
        inductance_static_nh: static_l.map(|v| v.0 * 1e9),
        flatness_static: static_l.map(|v| v.1),
    })
}

/// The controller's pad on one of `switch_nets` nearest the loop's hot supply pad: the switch-node
/// pad of a synchronous cell, whose four-pad loop does not name it.
pub fn find_switch_pad(
    board: &PhysicalBoard,
    input: &HotLoopInput,
    switch_nets: &[String],
) -> Option<(String, String)> {
    if input.pads.len() != 4 || switch_nets.is_empty() {
        return None;
    }
    let stack = stack(board).ok()?;
    let all = placed_pads(board, &crate::model::poses(board), &stack);
    let centre = |component: &str, pad: &str| -> Option<(f64, f64)> {
        let p = all.iter().find(|p| p.component == component && p.number == pad)?;
        let bb = geom::bbox(&p.ring)?;
        Some(((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0))
    };
    let supply = centre(&input.pads[1].0, &input.pads[1].1)?;
    all.iter()
        .filter(|p| p.component == input.cell && p.net.as_ref().is_some_and(|n| switch_nets.contains(n)))
        .filter_map(|p| {
            let c = centre(&p.component, &p.number)?;
            Some((p, (c.0 - supply.0).hypot(c.1 - supply.1)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| (p.component.clone(), p.number.clone()))
}

//! Interconnect impedance of validated hot-loop PCB networks, extracted from real filled copper.
//!
//! The port, the shorts and the window are those of `copper-fullwave`'s hot-loop model (see [`crate::board`]), so
//! buck numbers can be read next to the closed-form value of the finding and the full-wave one. External buck-boost
//! networks use a separate contact-resolved boundary, excluding package-gap conductors. Three meshes test every
//! R/L entry: buck keeps pitch ratio 1.5; external networks use ratio 2 and progressively finer width resolution.

use std::time::Instant;

use copper_fullwave::hotloop::HotLoopInput;
use copper_pcb::board::PhysicalBoard;

use crate::board::{BoardModel, BoardOptions, LoopPad, SynthesizedLeg, UnconnectedCapacitor, build_layout, loop_ports};
use crate::current::CurrentMap;
use crate::extract::{Extraction, Progress, extract_with_progress};
use crate::layout::Layout;
use crate::mesh::MeshSpec;
use crate::reference::{ReferenceOptions, ReferenceResult};

/// The two finest meshes of the series differing by more than this at the second frequency (10 MHz) make the result
/// `indicative`, percent.
pub const CONVERGENCE_THRESHOLD_PCT: f64 = 3.0;

/// The frequencies of the report: the partial-element model is quasi-static, so the range stays where the loop is
/// small against a wavelength.
pub const DEFAULT_FREQUENCIES_HZ: [f64; 3] = [1e6, 1e7, 1e8];

/// What the caller chooses.
#[derive(Clone, Debug)]
pub struct HotLoopOptions {
    pub mesh: MeshSpec,
    pub board: BoardOptions,
    pub frequencies_hz: Vec<f64>,
    /// Run a series of three meshes (the finest and two coarser) for the numerical sensitivity (default on).
    pub series: bool,
    /// Keep the current distribution of the hot loop's port at one frequency (10 MHz when it is among the
    /// frequencies, else the highest) in [`HotLoopExtraction::currents`] (default off: the branch currents of the
    /// finest mesh are solved anyway, so this only arranges them).
    pub current_map: bool,
    /// Also run the independent reference solver (`crate::reference`) on the same layout and compare its DC limit
    /// with the PEEC extraction at a low frequency (default off).
    pub reference: Option<ReferenceOptions>,
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            mesh: MeshSpec::default(),
            board: BoardOptions::default(),
            frequencies_hz: DEFAULT_FREQUENCIES_HZ.to_vec(),
            series: true,
            current_map: false,
            reference: None,
        }
    }
}

/// The frequency of the PEEC extraction the reference is compared with, Hz: low enough for the DC limit (the port
/// impedance is `R + j w L` with `w L` far below `R`; 100 Hz gives the same numbers).
pub const REFERENCE_COMPARISON_HZ: f64 = 1e3;

/// The largest difference, percent, between the reference and the PEEC extraction in any entry of `R` or `L` before
/// the comparison is a reason (flag `reference_disagrees`): the sheet-against-bar thickness correction (1 to 2 %), the
/// reference's pitch and the mesh sensitivity of the extraction add up to a few percent on the analytic cases.
pub const REFERENCE_LIMIT_PCT: f64 = 10.0;

/// The independent reference's DC limit beside the PEEC extraction at [`REFERENCE_COMPARISON_HZ`].
#[derive(Clone, Debug)]
pub struct ReferenceComparison {
    pub reference: ReferenceResult,
    /// The same reference at a pitch twice as coarse (a quarter of the pixels): its own pitch sensitivity.
    pub coarse_reference: Option<ReferenceResult>,
    /// The finer reference against the coarser one per entry, percent (normalised like the other differences).
    pub sensitivity_r_pct: Vec<Vec<f64>>,
    pub sensitivity_l_pct: Vec<Vec<f64>>,
    pub frequency_hz: f64,
    /// The extraction's `R` (ohm) and `L` (nH) matrices at that frequency, finest mesh.
    pub peec_r_ohm: Vec<Vec<f64>>,
    pub peec_l_nh: Vec<Vec<f64>>,
    /// Reference minus PEEC per entry, percent: a diagonal entry of the PEEC value, an off-diagonal entry of the
    /// geometric mean of the two PEEC diagonals.
    pub difference_r_pct: Vec<Vec<f64>>,
    pub difference_l_pct: Vec<Vec<f64>>,
    /// Pieces of copper of the extraction's topology check: those with mesh nodes plus those without any.
    pub peec_components: usize,
    pub limit_pct: f64,
    /// Entries beyond the limit, one line each.
    pub disagreements: Vec<String>,
    /// Topology differences: a port open for one and not for the other, different numbers of copper pieces.
    pub topology_disagreements: Vec<String>,
    /// Entries in which the reference itself moves by more than the limit between its pitch and twice it: the
    /// comparison with the extraction is then not established.
    pub reference_not_converged: Vec<String>,
}

/// Difference of `a` against `b` entry by entry, percent (see [`ReferenceComparison`]).
fn signed_differences(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    (0..b.len())
        .map(|i| {
            (0..b.len())
                .map(|j| {
                    let scale = if i == j { b[i][i].abs() } else { (b[i][i] * b[j][j]).abs().sqrt() };
                    if scale > 0.0 { 100.0 * (a[i][j] - b[i][j]) / scale } else { f64::NAN }
                })
                .collect()
        })
        .collect()
}

/// The reference's own pitch sensitivity: the coarser run against the finer one per entry of `R` and of `L`, percent
/// (normalised like the other differences), and every reason the reference is not converged with its pitch. A
/// comparison against a reference that is not converged is not established, whatever it says of the extraction.
///
/// Reasons: no coarser run; a port that is open at one pitch and connected at the other (the topology changed with the
/// pitch); different numbers of pieces of copper at the two pitches; an entry that is not a finite number (an open
/// port gives NaN, which no threshold comparison catches); an entry that moves by more than [`REFERENCE_LIMIT_PCT`].
#[allow(clippy::needless_range_loop, clippy::type_complexity)]
pub fn reference_pitch_sensitivity(
    fine: &ReferenceResult,
    coarse: Option<&ReferenceResult>,
    labels: &[String],
) -> (Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<String>) {
    let label = |i: usize| labels.get(i).map_or_else(|| format!("port {i}"), Clone::clone);
    let Some(coarse) = coarse else {
        return (
            Vec::new(),
            Vec::new(),
            vec!["the reference could not be run at twice its pitch: its own pitch sensitivity is unknown".to_owned()],
        );
    };
    let r_pct = signed_differences(&coarse.resistance_ohm, &fine.resistance_ohm);
    let l_pct = signed_differences(&coarse.inductance_nh, &fine.inductance_nh);
    let mut reasons = Vec::new();
    let says = |open: bool| if open { "open" } else { "connected" };
    for p in 0..fine.port_open.len().max(coarse.port_open.len()) {
        match (fine.port_open.get(p), coarse.port_open.get(p)) {
            (Some(&a), Some(&b)) if a == b => {}
            (Some(&a), Some(&b)) => reasons.push(format!(
                "port {}: {} for the reference at its pitch and {} at twice it: the topology changes with the pitch",
                label(p),
                says(a),
                says(b)
            )),
            _ => reasons.push(format!("port {}: missing from one of the two reference runs", label(p))),
        }
    }
    let (pieces, coarse_pieces) =
        (fine.components.saturating_sub(fine.edge_slivers), coarse.components.saturating_sub(coarse.edge_slivers));
    if pieces != coarse_pieces {
        reasons.push(format!(
            "the reference counts {pieces} piece(s) of copper at its pitch and {coarse_pieces} at twice it: the topology changes with the pitch"
        ));
    }
    let n = fine.port_open.len();
    for (name, d) in [("R", &r_pct), ("L", &l_pct)] {
        for i in 0..n {
            for j in i..n {
                let entry = if i == j { label(i) } else { format!("{} - {}", label(i), label(j)) };
                match d.get(i).and_then(|row| row.get(j)) {
                    Some(v) if v.is_finite() => {
                        if v.abs() > REFERENCE_LIMIT_PCT {
                            reasons.push(format!(
                                "{name} {entry}: the reference moves by {v:+.1} % between its pitch and twice it"
                            ));
                        }
                    }
                    _ => reasons.push(format!(
                        "{name} {entry}: no finite value at one of the two reference pitches, so its pitch sensitivity is unknown"
                    )),
                }
            }
        }
    }
    (r_pct, l_pct, reasons)
}

#[allow(clippy::needless_range_loop)]
fn compare_with_reference(
    layout: &Layout,
    base: &MeshSpec,
    options: &ReferenceOptions,
    labels: &[String],
    peec_components: usize,
) -> Result<ReferenceComparison, String> {
    let reference = crate::reference::solve(layout, options)?;
    let coarse_options = ReferenceOptions { pitch_nm: Some(reference.pitch_nm * 2), ..options.clone() };
    let coarse_reference = crate::reference::solve(layout, &coarse_options).ok();
    let x = mesh_with(layout, base, &[REFERENCE_COMPARISON_HZ], false, |_| {})?;
    let n = x.solutions[0].z.len();
    let peec_r_ohm: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| x.resistance(0, i, j)).collect()).collect();
    let peec_l_nh: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| x.inductance(0, i, j) * 1e9).collect()).collect();
    let (sensitivity_r_pct, sensitivity_l_pct, not_converged) =
        reference_pitch_sensitivity(&reference, coarse_reference.as_ref(), labels);
    let difference_r_pct = signed_differences(&reference.resistance_ohm, &peec_r_ohm);
    let difference_l_pct = signed_differences(&reference.inductance_nh, &peec_l_nh);
    let label = |i: usize| labels.get(i).map_or_else(|| format!("port {i}"), Clone::clone);
    let mut disagreements = Vec::new();
    for (name, d) in [("R", &difference_r_pct), ("L", &difference_l_pct)] {
        for i in 0..n {
            for j in i..n {
                let v = d[i][j];
                if v.is_nan() || v.abs() > REFERENCE_LIMIT_PCT {
                    disagreements.push(format!(
                        "{name} {}: the reference differs from the extraction at {:.0} Hz by {v:+.1} % (limit {REFERENCE_LIMIT_PCT} %)",
                        if i == j { label(i) } else { format!("{} - {}", label(i), label(j)) },
                        REFERENCE_COMPARISON_HZ
                    ));
                }
            }
        }
    }
    let mut topology_disagreements = Vec::new();
    for (p, open) in reference.port_open.iter().enumerate() {
        if *open {
            topology_disagreements.push(format!(
                "the reference finds port {} open (its terminals are in different pieces of copper) where the extraction solved it",
                label(p)
            ));
        }
    }
    if reference.components - reference.edge_slivers != peec_components {
        topology_disagreements.push(format!(
            "the reference counts {} connected piece(s) of copper and the extraction's topology check {} (meshed pieces and pieces without a mesh node, each counted once)",
            reference.components - reference.edge_slivers, peec_components
        ));
    }
    Ok(ReferenceComparison {
        sensitivity_r_pct,
        sensitivity_l_pct,
        coarse_reference,
        reference,
        frequency_hz: REFERENCE_COMPARISON_HZ,
        peec_r_ohm,
        peec_l_nh,
        difference_r_pct,
        difference_l_pct,
        peec_components,
        limit_pct: REFERENCE_LIMIT_PCT,
        disagreements,
        topology_disagreements,
        reference_not_converged: not_converged,
    })
}

/// One mesh that was run.
#[derive(Clone, Debug)]
pub struct MeshRun {
    pub width_cells: usize,
    pub loop_cell_nm: i64,
    pub defect_cell_nm: i64,
    pub role: &'static str,
    pub cell_nm: i64,
    pub coarse_nm: i64,
    pub filaments: usize,
    pub nodes: usize,
    pub loops: usize,
    pub barrels: usize,
    pub coarsen_steps: usize,
    /// Filament bodies dropped for too little copper, or because the copper has a gap along them.
    pub dropped_bars: usize,
    /// Of those, bodies whose copper has a gap along the axis (a slot or opening the filament would bridge).
    pub gap_bars_dropped: usize,
    /// Rounds of local refinement round gaps and cut copper.
    pub refine_rounds: usize,
    /// Refinements applied, and found but left out because the budget did not hold them.
    pub refinements_applied: usize,
    pub refinements_skipped: usize,
    /// Gaps the mesh could not resolve (see `HotLoopExtraction::unresolved_gaps`).
    pub unresolved_gap_count: usize,
    /// Copper pieces and mesh components of the topology check, and its mismatches.
    pub copper_components: usize,
    pub mesh_components: usize,
    pub topology_mismatches: usize,
    /// Inductance (nH) and resistance (ohm) at each frequency.
    pub inductance_nh: Vec<f64>,
    pub resistance_ohm: Vec<f64>,
    /// The whole resistance matrix of the ports (`Re Z`), ohm: `[frequency][row][column]`.
    pub resistance_matrix_ohm: Vec<Vec<Vec<f64>>>,
    /// The whole inductance matrix of the ports, nH: `[frequency][row][column]`.
    pub inductance_matrix_nh: Vec<Vec<Vec<f64>>>,
    pub seconds: f64,
    pub assemble_seconds: f64,
    pub solve_seconds: f64,
}

/// A via and its current.
#[derive(Clone, Debug)]
pub struct ViaCurrent {
    pub name: String,
    pub net: String,
    pub x_mm: f64,
    pub y_mm: f64,
    /// Current through the via per ampere into the port.
    pub current_a: f64,
    /// `|I| / sum |I|` over the vias of the net.
    pub share_of_net: f64,
}

/// The result for a report.
#[derive(Clone, Debug)]
pub struct HotLoopExtraction {
    pub cell: String,
    pub selected_port: usize,
    pub port_observations: Vec<PortObservations>,
    pub frequencies_hz: Vec<f64>,
    /// The numbers of the finer of the two meshes (the only one without a second).
    pub inductance_nh: Vec<f64>,
    pub resistance_ohm: Vec<f64>,
    pub meshes: Vec<MeshRun>,
    /// Relative difference of the two meshes' inductance at each frequency, percent.
    pub mesh_difference_pct: Vec<f64>,
    /// The same for the two coarser meshes of the series (empty with fewer than three).
    pub earlier_difference_pct: Vec<f64>,
    pub status: &'static str,
    pub status_reasons: Vec<String>,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    pub not_modelled: Vec<String>,
    pub nets: Vec<String>,
    pub other_nets: Vec<String>,
    pub window_mm: [f64; 4],
    pub layers: Vec<(String, f64)>,
    pub tracks: usize,
    pub pads: usize,
    pub fill_shapes: usize,
    pub vias: usize,
    pub islands_removed: usize,
    pub zone_fills: usize,
    pub fill_seconds: f64,
    /// Per frequency, the vias with at least 1 % of the port current, strongest first.
    pub via_currents: Vec<Vec<ViaCurrent>>,
    pub seconds: f64,
    pub port_description: String,
    /// The ports of the extraction: `hot loop` and the capacitors asked for (`BoardOptions::capacitor_ports`).
    pub port_labels: Vec<String>,
    /// The inductance matrix of the finest mesh, nH, `[frequency][row][column]` (1 x 1 without capacitor ports).
    pub inductance_matrix_nh: Vec<Vec<Vec<f64>>>,
    /// Largest relative change of a diagonal entry between the two finest meshes at the second frequency, percent.
    pub matrix_difference_pct: f64,
    /// A capacitor port was joined to the hot capacitor by a straight bar because no copper did (any of
    /// `synthesized_legs`).
    pub connection_legs: bool,
    /// Each straight bar added between a capacitor's pad and the hot capacitor's pad on its net because no copper
    /// joins them (the capacitor's terminal that no copper reaches, and only that one).
    pub synthesized_legs: Vec<SynthesizedLeg>,
    /// Capacitors that copper does not join to the hot capacitor: no port, no bar, their connection stays the
    /// estimate.
    pub unconnected_capacitors: Vec<UnconnectedCapacitor>,
    /// The copper's connectivity against the mesh's (finest mesh): the polygons of copper (joined by vias) and the
    /// connected components of the network, and one line for every mismatch; empty when they agree.
    pub copper_components: usize,
    pub mesh_components: usize,
    /// Polygons of copper with no mesh node (too small for the mesh).
    pub unmeshed_pieces: usize,
    pub topology_mismatches: Vec<String>,
    /// Gaps the finest mesh could not resolve (a slot or gap in the copper narrower than the smallest cell), as
    /// `layer, net, x, y: what`, at most 20 listed; the number of them. Never bridged.
    pub unresolved_gaps: Vec<String>,
    pub unresolved_gap_count: usize,
    /// The mesh series judged on every entry of the L and R matrices at every frequency.
    pub convergence: crate::convergence::Convergence,
    /// Terminals whose region overlaps several pieces of copper (equipotentials that join them): name and number of pieces.
    pub terminals_joining_pieces: Vec<(String, usize)>,
    /// The independent reference solver's DC limit beside the extraction (`HotLoopOptions::reference`).
    pub reference: Option<Box<ReferenceComparison>>,
    /// The reference solver failed (message) when it was asked for.
    pub reference_error: Option<String>,
    /// The current distribution of the hot loop's port (`HotLoopOptions::current_map`).
    pub currents: Option<Box<HotLoopCurrents>>,
}

/// Per-column observations, retained without another solve or summing excitations.
#[derive(Clone, Debug)]
pub struct PortObservations {
    pub via_currents: Vec<Vec<ViaCurrent>>,
    pub currents: Option<Box<HotLoopCurrents>>,
}

impl HotLoopExtraction {
    pub fn select_port(&self, port: usize) -> Result<Self, String> {
        if port >= self.port_labels.len() || port >= self.port_observations.len() {
            return Err("reported port is outside the impedance matrix".to_owned());
        }
        let mut result = self.clone();
        result.selected_port = port;
        for mesh in &mut result.meshes {
            mesh.inductance_nh = mesh.inductance_matrix_nh.iter().map(|m| m[port][port]).collect();
            mesh.resistance_ohm = mesh.resistance_matrix_ohm.iter().map(|m| m[port][port]).collect();
        }
        result.inductance_nh = result.meshes[0].inductance_nh.clone();
        result.resistance_ohm = result.meshes[0].resistance_ohm.clone();
        let difference = |a: usize, b: usize| {
            result.meshes[a]
                .inductance_nh
                .iter()
                .zip(&result.meshes[b].inductance_nh)
                .map(|(x, y)| 100.0 * (x - y).abs() / x.abs())
                .collect()
        };
        result.mesh_difference_pct = if result.meshes.len() >= 2 { difference(0, 1) } else { Vec::new() };
        result.earlier_difference_pct = if result.meshes.len() >= 3 { difference(1, 2) } else { Vec::new() };
        result.via_currents = self.port_observations[port].via_currents.clone();
        result.currents = self.port_observations[port].currents.clone();
        result.port_description = format!(
            "{}; column {port} of the shared native-copper matrix; all other port currents zero; declared capacitor boundary, no package termination",
            self.port_labels[port]
        );
        Ok(result)
    }
}

/// The current distribution of the finest mesh at one frequency, with the loop's pads.
#[derive(Clone, Debug)]
pub struct HotLoopCurrents {
    pub map: CurrentMap,
    /// Index into [`HotLoopExtraction::frequencies_hz`] of the frequency the map is for.
    pub frequency_index: usize,
    pub pads: Vec<LoopPad>,
}

/// Preferred frequency of the current map, Hz.
pub const CURRENT_MAP_HZ: f64 = 1e7;

fn mesh_with(
    layout: &Layout,
    spec: &MeshSpec,
    frequencies: &[f64],
    currents: bool,
    progress: impl FnMut(Progress),
) -> Result<Extraction, String> {
    extract_with_progress(layout, spec, frequencies, currents, progress).map_err(|e| e.to_string())
}

pub fn run_info(role: &'static str, x: &Extraction) -> MeshRun {
    MeshRun {
        width_cells: x.mesh.stats.width_cells,
        loop_cell_nm: x.mesh.stats.loop_cell_nm,
        defect_cell_nm: x.mesh.stats.defect_cell_nm,
        role,
        cell_nm: x.mesh.stats.cell_nm,
        coarse_nm: x.mesh.stats.coarse_nm,
        filaments: x.mesh.stats.filaments,
        nodes: x.mesh.stats.nodes,
        loops: x.loops,
        barrels: x.mesh.stats.barrels,
        coarsen_steps: x.mesh.stats.coarsen_steps,
        dropped_bars: x.mesh.stats.dropped_bars,
        gap_bars_dropped: x.mesh.stats.gap_bars_dropped,
        refine_rounds: x.mesh.stats.refine_rounds,
        refinements_applied: x.mesh.stats.refinements_applied,
        refinements_skipped: x.mesh.stats.refinements_skipped,
        unresolved_gap_count: x.mesh.stats.unresolved_gap_count,
        copper_components: x.mesh.stats.topology.copper_components,
        mesh_components: x.mesh.stats.topology.mesh_components,
        topology_mismatches: x.mesh.stats.topology.mismatches.len(),
        inductance_nh: (0..x.frequencies.len()).map(|f| x.inductance(f, 0, 0) * 1e9).collect(),
        resistance_ohm: (0..x.frequencies.len()).map(|f| x.resistance(f, 0, 0)).collect(),
        resistance_matrix_ohm: (0..x.frequencies.len())
            .map(|f| {
                let n = x.solutions[f].z.len();
                (0..n).map(|i| (0..n).map(|j| x.resistance(f, i, j)).collect()).collect()
            })
            .collect(),
        inductance_matrix_nh: (0..x.frequencies.len())
            .map(|f| {
                let n = x.solutions[f].z.len();
                (0..n).map(|i| (0..n).map(|j| x.inductance(f, i, j) * 1e9).collect()).collect()
            })
            .collect(),
        seconds: x.mesh_seconds + x.assemble_seconds + x.solve_seconds,
        assemble_seconds: x.assemble_seconds,
        solve_seconds: x.solve_seconds,
    }
}

/// Extract the hot loop of `input`.
pub fn extract_hot_loop(
    model: &BoardModel,
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &HotLoopOptions,
) -> Result<HotLoopExtraction, String> {
    extract_hot_loop_with_progress(model, board, input, options, |_, _| {})
}

/// Extract with a diagnostic observer for each completed stage of each mesh.
/// Intermediate meshes are not qualified results: convergence is assessed after the series.
pub fn extract_hot_loop_with_progress(
    model: &BoardModel,
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &HotLoopOptions,
    mut progress: impl FnMut(&'static str, Progress),
) -> Result<HotLoopExtraction, String> {
    let started = Instant::now();
    let ports = loop_ports(input)?;
    let build = |board_options: &BoardOptions| {
        build_layout(model, board, input, &ports, board_options, options.mesh.cell_nm, options.mesh.coarse_nm)
    };
    // A capacitor with a pad that no copper joins to the hot capacitor's pad gets no port and no bar (decided from the
    // copper's own connectivity, before meshing; `BoardOptions::connection_legs` asks for per-pad bars instead).
    let mut layout = build(&options.board)?;
    let unconnected = layout.unconnected.clone();
    if !unconnected.is_empty() {
        // No port and no copper for a capacitor that copper does not join: build again without it, so that the
        // window and the plane corridor are those of the capacitors that have a port.
        let ports_kept: Vec<String> = options
            .board
            .capacitor_ports
            .iter()
            .filter(|c| !unconnected.iter().any(|u| &u.capacitor == *c))
            .cloned()
            .collect();
        layout = build(&BoardOptions { capacitor_ports: ports_kept, ..options.board.clone() })?;
    }
    let freqs = &options.frequencies_hz;
    let base = MeshSpec {
        coarsen: true,
        across: if input.external_contour.is_some() { options.mesh.across.max(4) } else { options.mesh.across },
        loop_cell_nm: if input.external_contour.is_some() { 25_000 } else { options.mesh.loop_cell_nm },
        defect_cell_nm: if input.external_contour.is_some() { 25_000 } else { options.mesh.defect_cell_nm },
        ..options.mesh.clone()
    };
    let first = mesh_with(&layout.layout, &base, freqs, true, |stage| progress("standard", stage))?;
    let connection_legs = !layout.synthesized_legs.is_empty();
    let mut runs = vec![run_info("standard", &first)];
    if options.series {
        // External networks retain neighbouring copper and feature-scale mesh
        // constraints. Use octave steps to separate their adaptive meshes;
        // the original buck series keeps its 1.5 step. Density, topology and
        // every matrix entry still have to pass the same convergence checks.
        // Two coarser meshes so that the last two steps of a refinement series exist:
        // the numbers reported are the finest mesh's, the evidence of convergence is how much they move.
        let used = MeshSpec { scale: first.mesh.stats.scale, ..base.clone() };
        let step = if input.external_contour.is_some() { 2.0 } else { 1.5 };
        for (role, factor, width_factor) in [("coarser", 1.0 / step, 1.5), ("coarsest", 1.0 / (step * step), 2.25)] {
            let spec = MeshSpec {
                coarsen: false,
                across: if input.external_contour.is_some() {
                    (base.across as f64 / width_factor).ceil() as usize
                } else {
                    base.across
                },
                ..used.refined(factor)
            };
            let x = mesh_with(&layout.layout, &spec, freqs, false, |stage| progress(role, stage))?;
            runs.push(run_info(role, &x));
        }
    }
    let chosen = runs[0].clone();
    // Relative difference of the finest mesh to the next one, per frequency, and of the two coarser ones.
    let diff = |a: usize, b: usize| -> Vec<f64> {
        (0..freqs.len())
            .map(|f| 100.0 * (runs[a].inductance_nh[f] - runs[b].inductance_nh[f]).abs() / runs[a].inductance_nh[f])
            .collect()
    };
    let mesh_difference_pct: Vec<f64> = if runs.len() >= 2 { diff(0, 1) } else { Vec::new() };
    let earlier_difference_pct: Vec<f64> = if runs.len() >= 3 { diff(1, 2) } else { Vec::new() };
    let at_matrix = 1.min(freqs.len() - 1);
    let matrix_difference_pct = if runs.len() >= 2 {
        let (a, b) = (&runs[0].inductance_matrix_nh[at_matrix], &runs[1].inductance_matrix_nh[at_matrix]);
        (0..a.len()).map(|i| 100.0 * (a[i][i] - b[i][i]).abs() / a[i][i]).fold(0.0, f64::max)
    } else {
        0.0
    };
    let mut flags = layout.flags.clone();
    flags.extend(first.mesh.stats.flags.iter().cloned());
    let stats = &first.mesh.stats;
    let mut reasons = Vec::new();
    let mut notes = layout.notes.clone();
    if input.external_contour.is_some() {
        notes.push("external native loop tracks retain width-based resolution; local topology defects refine to a 25 um requested cell floor, including neighbouring copper; three pitches use 1:2:4 with decreasing width resolution (default 4/3/2 cells); all matrix convergence limits are unchanged".to_owned());
    }
    for u in &unconnected {
        notes.push(format!(
            "capacitor {}: no copper joins pad(s) {} to the hot capacitor's pad on the same net, so it gets no port and no bar: its connection inductance stays the estimate",
            u.capacitor,
            u.pads.join(", ")
        ));
    }
    if stats.coarsen_steps > 0 {
        // Terminals, vias and narrow copper keep their own resolution whatever the pitch, so coarsening is not by
        // itself a reason; the convergence evidence below is.
        notes.push(format!(
            "the pitch was coarsened {} time(s) by 1.3 to fit the {}-filament budget (pitch {:.2} mm fine, {:.2} mm coarse); terminals, vias and narrow copper keep their resolution",
            stats.coarsen_steps,
            options.mesh.max_filaments,
            stats.cell_nm as f64 * 1e-6,
            stats.coarse_nm as f64 * 1e-6
        ));
    }
    if !stats.terminals_snapped.is_empty() {
        flags.push("terminal_snapped".to_owned());
        reasons.push(format!(
            "terminal(s) without a mesh node inside the pad joined by the nearest node: {}",
            stats.terminals_snapped.join(", ")
        ));
    }
    if !stats.vias_unattached.is_empty() {
        flags.push("via_unattached".to_owned());
        reasons.push(format!("{} via(s) joined fewer than two layers and are left out", stats.vias_unattached.len()));
    }
    if stats.unresolved_gap_count > 0 {
        flags.push("unresolved_gap".to_owned());
        reasons.push(format!(
            "{} gap(s) in the copper are narrower than the smallest cell the mesher can use and were not resolved (no filament bridges them, so the copper on either side is cut there): {}",
            stats.unresolved_gap_count,
            stats.unresolved_gaps.first().map_or(String::new(), |g| format!("first: {g}"))
        ));
    }
    if !stats.topology.mismatches.is_empty() {
        flags.push("topology_mismatch".to_owned());
        reasons.push(format!(
            "the meshed network does not have the connectivity of the copper ({} mismatch(es)): {}",
            stats.topology.mismatches.len(),
            stats.topology.mismatches[0]
        ));
    }
    for (name, count) in &stats.terminals_joining_pieces {
        notes.push(format!(
            "terminal {name} overlaps {count} separate pieces of copper of its net and joins them (a terminal is an equipotential): the copper it joins is not otherwise connected there"
        ));
    }
    if stats.refine_rounds > 0 {
        notes.push(format!(
            "the mesh was refined in {} round(s) locally where a slot, an opening or two pieces of copper were closer than the cell, so that no filament bridges copper that does not touch: {} refinement(s) applied, {} left out because the filament budget did not hold them (the repairs of a topology mismatch first, then the gaps nearest to the port); {} filament bodies dropped for a gap in the copper along them",
            stats.refine_rounds, stats.refinements_applied, stats.refinements_skipped, stats.gap_bars_dropped
        ));
    }
    let convergence = crate::convergence::assess(
        &runs,
        freqs,
        &layout.port_labels,
        layout.layout.layers.first().map_or(35e-6, |l| l.thickness_m),
    );
    if runs.len() < 2 {
        reasons.push("a single mesh: no evidence of convergence".to_owned());
    } else {
        reasons.extend(convergence.status_reasons());
    }
    if !options.board.capacitor_ports.is_empty() && connection_legs {
        flags.push("connection_legs".to_owned());
    }
    flags.sort();
    flags.dedup();
    // Via currents of the standard mesh (it kept the branch currents). A buck loop reports its hot-loop port (port 0)
    // only; an external contact network reports each port, any of which `select_port` can pick.
    let observed_ports = if input.external_contour.is_some() { layout.port_labels.len() } else { 1 };
    let mut port_observations = Vec::new();
    for port in 0..observed_ports {
        let mut via_currents = Vec::new();
        let mut net_names: Vec<&str> = layout.layout.vias.iter().map(|v| v.net.as_str()).collect();
        net_names.sort_unstable();
        net_names.dedup();
        for f in 0..freqs.len() {
            let mut list = Vec::new();
            for net in &net_names {
                let ids: Vec<usize> =
                    layout.layout.vias.iter().enumerate().filter(|(_, v)| v.net == *net).map(|(i, _)| i).collect();
                for share in first.via_shares(f, port, &ids) {
                    if share.current.abs() >= 0.01 {
                        let v = &layout.layout.vias[share.via];
                        list.push(ViaCurrent {
                            name: v.name.clone(),
                            net: v.net.clone(),
                            x_mm: v.x as f64 * 1e-6,
                            y_mm: v.y as f64 * 1e-6,
                            current_a: share.current.abs(),
                            share_of_net: share.share,
                        });
                    }
                }
            }
            list.sort_by(|a, b| b.current_a.total_cmp(&a.current_a).then(a.name.cmp(&b.name)));
            via_currents.push(list);
        }
        let currents = if options.current_map {
            let index = freqs.iter().position(|f| (f - CURRENT_MAP_HZ).abs() < 1.0).unwrap_or(freqs.len() - 1);
            first
                .current_map(&layout.layout, index, port)
                .map(|map| Box::new(HotLoopCurrents { map, frequency_index: index, pads: layout.loop_pads.clone() }))
        } else {
            None
        };
        port_observations.push(PortObservations { via_currents, currents });
    }
    let via_currents = port_observations[0].via_currents.clone();
    let currents = port_observations[0].currents.clone();
    let (reference, reference_error) = match &options.reference {
        None => (None, None),
        Some(reference_options) => {
            match compare_with_reference(
                &layout.layout,
                &base,
                reference_options,
                &layout.port_labels,
                stats.topology.copper_components + stats.topology.unmeshed_pieces,
            ) {
                Ok(c) => (Some(Box::new(c)), None),
                Err(e) => (None, Some(e)),
            }
        }
    };
    if let Some(c) = &reference {
        if !c.reference_not_converged.is_empty() {
            flags.push("reference_not_converged".to_owned());
            reasons.push(format!(
                "the independent reference solver is not converged with its own pitch ({} entry(ies); first: {}): its comparison with the extraction is not established",
                c.reference_not_converged.len(),
                c.reference_not_converged[0]
            ));
        }
        if !c.disagreements.is_empty() {
            flags.push("reference_disagrees".to_owned());
            reasons.push(format!(
                "the independent reference solver (the DC limit from the same copper) disagrees with the extraction at {:.0} Hz by more than {} % in {} entry(ies): {}",
                c.frequency_hz,
                c.limit_pct,
                c.disagreements.len(),
                c.disagreements[0]
            ));
        }
        if !c.topology_disagreements.is_empty() {
            flags.push("reference_topology_disagrees".to_owned());
            reasons.push(format!(
                "the independent reference solver and the mesher disagree on the topology: {}",
                c.topology_disagreements.join("; ")
            ));
        }
    }
    if let Some(e) = &reference_error {
        reasons.push(format!("the independent reference solver failed: {e}"));
    }
    // A mesh that does not have the copper's connectivity is not a measurement of the board: status `error`; so is a
    // topology on which the independent reference and the mesher disagree.
    let reference_topology_disagrees = reference.as_ref().is_some_and(|c| !c.topology_disagreements.is_empty());
    let external_contact_short = input.external_contour.is_some() && !stats.terminals_joining_pieces.is_empty();
    if external_contact_short {
        reasons.push(
            "external contour terminal patches join disconnected copper pieces; this is not the declared board path"
                .to_owned(),
        );
    }
    let status = if !stats.topology.mismatches.is_empty() || reference_topology_disagrees || external_contact_short {
        "error"
    } else if reasons.is_empty() {
        "ok"
    } else {
        "indicative"
    };
    let mut port_description = format!(
        "{} to {}; {}",
        ports.labels[ports.port.0],
        ports.labels[ports.port.1],
        ports.closures.iter().map(|c| format!("{c} shorted")).collect::<Vec<_>>().join(", ")
    );
    if input.external_contour.as_ref().is_some_and(|c| c.grouped()) {
        port_description.push_str("; independent contact matrix; scalar and current map: port 0 only, all other port currents zero; package termination unspecified");
    }
    Ok(HotLoopExtraction {
        cell: input.cell.clone(),
        selected_port: 0,
        port_observations,
        frequencies_hz: freqs.clone(),
        inductance_nh: chosen.inductance_nh.clone(),
        resistance_ohm: chosen.resistance_ohm.clone(),
        meshes: runs,
        mesh_difference_pct,
        earlier_difference_pct,
        status,
        status_reasons: reasons,
        flags,
        notes,
        not_modelled: layout.not_modelled.clone(),
        nets: layout.nets.clone(),
        other_nets: layout.other_nets.clone(),
        window_mm: layout.window_mm,
        layers: layout.layers.clone(),
        tracks: layout.tracks,
        pads: layout.pads,
        fill_shapes: layout.fill_shapes,
        vias: layout.vias,
        islands_removed: model.islands_removed,
        zone_fills: model.zone_fills,
        fill_seconds: model.fill_seconds,
        via_currents,
        seconds: started.elapsed().as_secs_f64(),
        port_description,
        port_labels: layout.port_labels.clone(),
        inductance_matrix_nh: chosen.inductance_matrix_nh.clone(),
        matrix_difference_pct,
        connection_legs,
        synthesized_legs: layout.synthesized_legs.clone(),
        unconnected_capacitors: unconnected,
        terminals_joining_pieces: stats.terminals_joining_pieces.clone(),
        reference,
        reference_error,
        convergence,
        copper_components: stats.topology.copper_components,
        mesh_components: stats.topology.mesh_components,
        unmeshed_pieces: stats.topology.unmeshed_pieces,
        topology_mismatches: stats.topology.mismatches.clone(),
        unresolved_gaps: stats.unresolved_gaps.clone(),
        unresolved_gap_count: stats.unresolved_gap_count,
        currents,
    })
}

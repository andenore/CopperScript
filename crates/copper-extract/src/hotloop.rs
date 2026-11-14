//! The interconnect impedance of a buck cell's hot loop, extracted from the real filled copper.
//!
//! The port, the shorts and the window are those of `copper-fullwave`'s hot-loop model (see [`crate::board`]), so
//! the numbers can be read next to the closed-form value of the finding and the full-wave one. The extraction is
//! made on a series of three meshes (pitch ratio 1.5); the difference between the two finest is the numerical
//! sensitivity reported and decides the status.

use std::time::Instant;

use copper_fullwave::hotloop::HotLoopInput;
use copper_pcb::board::PhysicalBoard;

use crate::board::{BoardModel, BoardOptions, LoopPad, SynthesizedLeg, UnconnectedCapacitor, build_layout, loop_ports};
use crate::current::CurrentMap;
use crate::extract::{Extraction, extract};
use crate::layout::Layout;
use crate::mesh::MeshSpec;

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
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            mesh: MeshSpec::default(),
            board: BoardOptions::default(),
            frequencies_hz: DEFAULT_FREQUENCIES_HZ.to_vec(),
            series: true,
            current_map: false,
        }
    }
}

/// One mesh that was run.
#[derive(Clone, Debug)]
pub struct MeshRun {
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
    pub topology_mismatches: Vec<String>,
    /// Gaps the finest mesh could not resolve (a slot or gap in the copper narrower than the smallest cell), as
    /// `layer, net, x, y: what`, at most 20 listed; the number of them. Never bridged.
    pub unresolved_gaps: Vec<String>,
    pub unresolved_gap_count: usize,
    /// The current distribution of the hot loop's port (`HotLoopOptions::current_map`).
    pub currents: Option<Box<HotLoopCurrents>>,
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

fn mesh_with(layout: &Layout, spec: &MeshSpec, frequencies: &[f64], currents: bool) -> Result<Extraction, String> {
    extract(layout, spec, frequencies, currents).map_err(|e| e.to_string())
}

fn run_info(role: &'static str, x: &Extraction) -> MeshRun {
    MeshRun {
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
    let base = MeshSpec { coarsen: true, ..options.mesh.clone() };
    let first = mesh_with(&layout.layout, &base, freqs, true)?;
    let connection_legs = !layout.synthesized_legs.is_empty();
    let mut runs = vec![run_info("standard", &first)];
    if options.series {
        // Two coarser meshes (pitches times 1.5 and 2.25) so that the last two steps of a refinement series exist:
        // the numbers reported are the finest mesh's, the evidence of convergence is how much they move.
        let used = MeshSpec { scale: first.mesh.stats.scale, ..base.clone() };
        for (role, factor) in [("coarser", 1.0 / 1.5), ("coarsest", 1.0 / 2.25)] {
            let spec = MeshSpec { coarsen: false, ..used.refined(factor) };
            let x = mesh_with(&layout.layout, &spec, freqs, false)?;
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
    if stats.refine_rounds > 0 {
        notes.push(format!(
            "the mesh was refined in {} round(s) locally where a slot, an opening or two pieces of copper were closer than the cell, so that no filament bridges copper that does not touch: {} refinement(s) applied, {} left out because the filament budget did not hold them (the repairs of a topology mismatch first, then the gaps nearest to the port); {} filament bodies dropped for a gap in the copper along them",
            stats.refine_rounds, stats.refinements_applied, stats.refinements_skipped, stats.gap_bars_dropped
        ));
    }
    if mesh_difference_pct.is_empty() {
        reasons.push("a single mesh: no evidence of convergence".to_owned());
    } else {
        let at = 1.min(freqs.len() - 1);
        if mesh_difference_pct[at] > CONVERGENCE_THRESHOLD_PCT {
            reasons.push(format!(
                "the two finest meshes differ by {:.1} % at {:.0} MHz (more than {CONVERGENCE_THRESHOLD_PCT} %)",
                mesh_difference_pct[at],
                freqs[at] * 1e-6
            ));
        }
    }
    if !options.board.capacitor_ports.is_empty() {
        if connection_legs {
            flags.push("connection_legs".to_owned());
        }
        if !mesh_difference_pct.is_empty() && matrix_difference_pct > CONVERGENCE_THRESHOLD_PCT {
            reasons.push(format!(
                "a self inductance of the port matrix moves {matrix_difference_pct:.1} % between the two finest meshes (more than {CONVERGENCE_THRESHOLD_PCT} %)"
            ));
        }
    }
    flags.sort();
    flags.dedup();
    // Via currents of the standard mesh (it kept the branch currents).
    let mut via_currents = Vec::new();
    let mut net_names: Vec<&str> = layout.layout.vias.iter().map(|v| v.net.as_str()).collect();
    net_names.sort_unstable();
    net_names.dedup();
    for f in 0..freqs.len() {
        let mut list = Vec::new();
        for net in &net_names {
            let ids: Vec<usize> =
                layout.layout.vias.iter().enumerate().filter(|(_, v)| v.net == *net).map(|(i, _)| i).collect();
            for share in first.via_shares(f, 0, &ids) {
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
            .current_map(&layout.layout, index, 0)
            .map(|map| Box::new(HotLoopCurrents { map, frequency_index: index, pads: layout.loop_pads.clone() }))
    } else {
        None
    };
    // A mesh that does not have the copper's connectivity is not a measurement of the board: status `error`.
    let status = if !stats.topology.mismatches.is_empty() {
        "error"
    } else if reasons.is_empty() {
        "ok"
    } else {
        "indicative"
    };
    let port_description = format!(
        "{} to {}; {}",
        ports.labels[ports.port.0],
        ports.labels[ports.port.1],
        ports.closures.iter().map(|c| format!("{c} shorted")).collect::<Vec<_>>().join(", ")
    );
    Ok(HotLoopExtraction {
        cell: input.cell.clone(),
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
        copper_components: stats.topology.copper_components,
        mesh_components: stats.topology.mesh_components,
        topology_mismatches: stats.topology.mismatches.clone(),
        unresolved_gaps: stats.unresolved_gaps.clone(),
        unresolved_gap_count: stats.unresolved_gap_count,
        currents,
    })
}

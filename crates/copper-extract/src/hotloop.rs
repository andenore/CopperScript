//! The interconnect impedance of a buck cell's hot loop, extracted from the real filled copper.
//!
//! The port, the shorts and the window are those of `copper-fullwave`'s hot-loop model (see [`crate::board`]), so
//! the numbers can be read next to the closed-form value of the finding and the full-wave one. The extraction is
//! made on a series of three meshes (pitch ratio 1.5); the difference between the two finest is the numerical
//! sensitivity reported and decides the status.

use std::time::Instant;

use copper_fullwave::hotloop::HotLoopInput;
use copper_pcb::board::PhysicalBoard;

use crate::board::{BoardModel, BoardOptions, build_layout, loop_ports};
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
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            mesh: MeshSpec::default(),
            board: BoardOptions::default(),
            frequencies_hz: DEFAULT_FREQUENCIES_HZ.to_vec(),
            series: true,
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
    /// The capacitor ports were joined to the hot capacitor by straight bars because no copper did.
    pub connection_legs: bool,
}

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
    let mut layout = build(&options.board)?;
    let freqs = &options.frequencies_hz;
    let base = MeshSpec { coarsen: true, ..options.mesh.clone() };
    let mut connection_legs = options.board.connection_legs;
    let first = match mesh_with(&layout.layout, &base, freqs, true) {
        // A capacitor whose pads no copper joins to the hot capacitor's: the straight bars of the estimate join them.
        Err(e)
            if !options.board.capacitor_ports.is_empty()
                && !connection_legs
                && e.contains("not connected by copper") =>
        {
            connection_legs = true;
            layout = build(&BoardOptions { connection_legs: true, ..options.board.clone() })?;
            mesh_with(&layout.layout, &base, freqs, true)?
        }
        other => other?,
    };
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
    let status = if reasons.is_empty() { "ok" } else { "indicative" };
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
    })
}

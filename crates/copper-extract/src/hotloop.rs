//! The interconnect impedance of a buck cell's hot loop, extracted from the real filled copper.
//!
//! The port, the shorts and the window are those of `copper-fullwave`'s hot-loop model (see [`crate::board`]), so
//! the numbers can be read next to the closed-form value of the finding and the full-wave one. The extraction is
//! made on two meshes and the difference between them is the numerical sensitivity reported.

use std::time::Instant;

use copper_fullwave::hotloop::HotLoopInput;
use copper_pcb::board::PhysicalBoard;

use crate::board::{BoardModel, BoardOptions, build_layout, loop_ports};
use crate::extract::{Extraction, extract};
use crate::layout::Layout;
use crate::mesh::MeshSpec;

/// The frequencies of the report: the partial-element model is quasi-static, so the range stays where the loop is
/// small against a wavelength.
pub const DEFAULT_FREQUENCIES_HZ: [f64; 3] = [1e6, 1e7, 1e8];

/// What the caller chooses.
#[derive(Clone, Debug)]
pub struct HotLoopOptions {
    pub mesh: MeshSpec,
    pub board: BoardOptions,
    pub frequencies_hz: Vec<f64>,
    /// Run a second mesh for the numerical sensitivity (default on).
    pub second_mesh: bool,
}

impl Default for HotLoopOptions {
    fn default() -> Self {
        HotLoopOptions {
            mesh: MeshSpec::default(),
            board: BoardOptions::default(),
            frequencies_hz: DEFAULT_FREQUENCIES_HZ.to_vec(),
            second_mesh: true,
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
    let layout =
        build_layout(model, board, input, &ports, &options.board, options.mesh.cell_nm, options.mesh.coarse_nm)?;
    let freqs = &options.frequencies_hz;
    let base = MeshSpec { coarsen: true, ..options.mesh.clone() };
    let first = mesh_with(&layout.layout, &base, freqs, true)?;
    let mut runs = vec![run_info("standard", &first)];
    if options.second_mesh {
        // A finer mesh when it fits the budget, else a coarser one (cheaper), so there are two to compare.
        let used = MeshSpec { scale: first.mesh.stats.scale, ..base.clone() };
        let finer = MeshSpec { coarsen: false, ..used.refined(1.5) };
        match mesh_with(&layout.layout, &finer, freqs, false) {
            Ok(x) => runs.push(run_info("refined", &x)),
            Err(_) => {
                let coarser = MeshSpec { coarsen: false, ..used.refined(1.0 / 1.5) };
                let x = mesh_with(&layout.layout, &coarser, freqs, false)?;
                runs.push(run_info("coarser", &x));
            }
        }
    }
    // The finer mesh's numbers are the result.
    let primary = if runs.len() == 2 && runs[1].role == "refined" { 1 } else { 0 };
    let chosen = runs[primary].clone();
    let mesh_difference_pct: Vec<f64> = if runs.len() == 2 {
        (0..freqs.len())
            .map(|f| 100.0 * (runs[0].inductance_nh[f] - runs[1].inductance_nh[f]).abs() / chosen.inductance_nh[f])
            .collect()
    } else {
        Vec::new()
    };
    let mut flags = layout.flags.clone();
    flags.extend(first.mesh.stats.flags.iter().cloned());
    let stats = &first.mesh.stats;
    let mut reasons = Vec::new();
    if stats.coarsen_steps > 0 {
        reasons.push(format!(
            "the mesh was coarsened {} time(s) by 1.3 to fit the {}-filament budget (pitch {:.2} mm fine, {:.2} mm coarse)",
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
        reasons.push("a single mesh: no numerical sensitivity".to_owned());
    } else {
        let at = 1.min(freqs.len() - 1);
        if mesh_difference_pct[at] > 10.0 {
            reasons.push(format!(
                "the two meshes differ by {:.1} % at {:.0} MHz",
                mesh_difference_pct[at],
                freqs[at] * 1e-6
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
        status,
        status_reasons: reasons,
        flags,
        notes: layout.notes.clone(),
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
    })
}

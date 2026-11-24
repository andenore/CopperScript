//! FDTD boundary for validated external PCB networks. Uses exactly the native
//! copper and capacitor fixtures assembled by the PEEC adapter, never buck bars.
//! The source is an explicit gap between contained rectangular contact patches.
//! This source-gap voltage convention is different from PEEC partial inductance.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use copper_cam::poly;
use copper_fullwave::fill::{RealFill, ZoneFillSet};
use copper_fullwave::hotloop::HotLoopInput;
use copper_fullwave::model::RegionModel;
use copper_fullwave::solve::{PortMatrix, RunStats};
use copper_fullwave::spec::{Budget, Error, Fidelity, PortSpec, RaisedBridge, RegionSpec, Terminal};
use copper_pcb::board::{PhysicalBoard, Via};
use copper_physical::{CopperLayer, Point};

use crate::board::{BoardModel, BoardOptions, build_layout, loop_ports};

pub type SourcePatches = Vec<([i64; 4], [i64; 4])>;

#[derive(Clone, Debug)]
pub struct Options {
    pub margin_nm: i64,
    pub cell_nm: i64,
    pub band_hz: (f64, f64),
    pub meshes: usize,
    pub budget_seconds: f64,
    pub threads: usize,
    /// Explicit PEC capacitor fixture height above the front copper. No height
    /// is inferred from the footprint; None retains the checked coplanar fixture.
    pub capacitor_height_nm: Option<i64>,
    /// Explicit elevated controller source fixture. No package height is inferred.
    pub source_height_nm: Option<i64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            margin_nm: 3_000_000,
            cell_nm: 200_000,
            band_hz: (50e6, 150e6),
            meshes: 2,
            budget_seconds: 300.0,
            threads: 0,
            capacitor_height_nm: None,
            source_height_nm: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Result {
    pub frequencies_hz: Vec<f64>,
    pub labels: Vec<String>,
    /// Frequency, row, column, [real, imaginary] ohms.
    pub impedance_ohm: Vec<Vec<Vec<[f64; 2]>>>,
    pub mesh_difference_pct: Option<f64>,
    pub runs: Vec<RunStats>,
    pub status_reasons: Vec<String>,
    pub patches_nm: SourcePatches,
    pub seconds: f64,
    pub approximations: Vec<String>,
}

/// Test the Hermitian part of Z for nonnegative absorbed power for *any*
/// complex current vector. Normalize by diagonal impedance magnitudes, then
/// Cholesky-factor the equivalent real symmetric matrix with a 1 ppm shift.
fn passive(z: &[Vec<[f64; 2]>]) -> bool {
    let n = z.len();
    let mut h = vec![vec![0.0; 2 * n]; 2 * n];
    for i in 0..n {
        for j in 0..n {
            let scale = (z[i][i][0].hypot(z[i][i][1]) * z[j][j][0].hypot(z[j][j][1])).sqrt().max(1e-12);
            let re = (z[i][j][0] + z[j][i][0]) / (2.0 * scale);
            let im = (z[i][j][1] - z[j][i][1]) / (2.0 * scale);
            h[i][j] = re;
            h[i + n][j + n] = re;
            h[i + n][j] = im;
            h[i][j + n] = -im;
        }
    }
    for i in 0..2 * n {
        h[i][i] += 1e-6;
        for j in 0..=i {
            let v = h[i][j] - (0..j).map(|k| h[i][k] * h[j][k]).sum::<f64>();
            if i == j {
                if !v.is_finite() || v <= 0.0 {
                    return false;
                }
                h[i][j] = v.sqrt();
            } else {
                h[i][j] = v / h[j][j];
            }
        }
    }
    true
}

/// A contained contact patch, not the bounding box of empty space around a
/// nonrectangular thermal land. No metal is added by this selection.
fn patch(shapes: &poly::Shapes) -> std::result::Result<[i64; 4], Error> {
    let (x0, y0, x1, y1) = shapes
        .iter()
        .map(|shape| poly::bounds(shape))
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
        .ok_or_else(|| Error::Invalid("empty external contact".to_owned()))?;
    let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
    for divide in [1, 2, 4, 8] {
        let (hx, hy) = ((x1 - x0) / (2 * divide), (y1 - y0) / (2 * divide));
        if hx < 10_000 || hy < 10_000 {
            break;
        }
        let rect = [cx - hx, cy - hy, cx + hx, cy + hy];
        let area = poly::polygon(&[
            (rect[0] as f64, rect[1] as f64),
            (rect[2] as f64, rect[1] as f64),
            (rect[2] as f64, rect[3] as f64),
            (rect[0] as f64, rect[3] as f64),
        ]);
        if poly::difference(&area, shapes).is_empty() {
            return Ok(rect);
        }
    }
    Err(Error::NotSupported(
        "external contact has no contained rectangular source patch; no package strap was invented".to_owned(),
    ))
}

pub fn region(
    model: &BoardModel,
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &Options,
    scale: f64,
) -> std::result::Result<(RegionModel, SourcePatches), Error> {
    if input.external_contour.is_none() {
        return Err(Error::NotSupported("external FDTD requires a validated external network".to_owned()));
    }
    let ports = loop_ports(input).map_err(Error::Invalid)?;
    let layout = build_layout(
        model,
        board,
        input,
        &ports,
        &BoardOptions {
            margin_nm: options.margin_nm,
            switch_bar: false,
            ideal_capacitors: options.capacitor_height_nm.is_some(),
            ..BoardOptions::default()
        },
        options.cell_nm,
        options.cell_nm * 3,
    )
    .map_err(Error::Invalid)?
    .layout;
    let layers: Vec<_> = layout
        .layers
        .iter()
        .map(|l| CopperLayer::parse(&l.name).ok_or_else(|| Error::Invalid("unknown copper layer".to_owned())))
        .collect::<std::result::Result<_, _>>()?;
    let mut source = board.clone();
    source.placements.clear();
    source.footprints.clear();
    source.tracks.clear();
    source.arcs.clear();
    source.polygons.clear();
    source.zones.clear();
    source.vias.clear();
    source.hard_macros.clear();
    source.rigid_clusters.clear();
    // Every conductor is imported below as an already filled shape. No source
    // zones are refilled or expanded, and no same-net pad group is shorted.
    let mut fills = Vec::new();
    for (i, layer) in layout.layers.iter().enumerate() {
        let mut nets: BTreeMap<String, poly::Shapes> = BTreeMap::new();
        for c in &layer.conductors {
            nets.entry(c.net.clone()).or_default().extend(c.shapes.clone());
        }
        for (net, shapes) in nets {
            fills.push(RealFill { net, layer: layers[i], shapes: poly::union(&shapes) });
        }
    }
    for v in &layout.vias {
        if !v.layer_nets.is_empty() {
            return Err(Error::NotSupported("external FDTD does not support multi-net via barrels".to_owned()));
        }
        source.vias.push(Via {
            net: v.net.clone(),
            position: Point::new(v.x, v.y),
            size_nm: v.drill_nm + 2 * v.plating_nm,
            drill_nm: v.drill_nm,
            from_layer: layers[v.first_layer],
            to_layer: layers[v.last_layer],
            technology: None,
            finish: "open".to_owned(),
        });
    }
    let mut spec = RegionSpec::new(layout.window);
    spec.cell_nm = Some((options.cell_nm as f64 * scale).round() as i64);
    spec.fidelity = Fidelity::Fine;
    spec.band_hz = options.band_hz;
    spec.include_pads = false;
    spec.edge_inset = 0.0;
    spec.edge_inset_z = 0.0;
    spec.zone_fills = Some(Arc::new(ZoneFillSet { fills, ..ZoneFillSet::default() }));
    spec.budget = Budget { max_seconds: options.budget_seconds, wall_margin: 1.0, ..Budget::default() };
    spec.threads = options.threads;
    // Matrix passivity is more stringent than a fitted scalar driving-point
    // response. Use the recorded transient for every column, without separate
    // tail extrapolations that can destroy multiport consistency.
    spec.ring_fit = false;
    spec.distributed_port_voltage = true;
    spec.decay_db = -80.0;
    if let Some(height_nm) = options.capacitor_height_nm {
        for terminals in &layout.shorts {
            if terminals.len() != 2 {
                return Err(Error::NotSupported("raised capacitor fixture requires exactly two terminals".to_owned()));
            }
            let (a, b) = (&layout.terminals[terminals[0]], &layout.terminals[terminals[1]]);
            spec.raised_bridges.push(RaisedBridge {
                name: a.name.clone(),
                a: Terminal::rect(layers[a.layer], patch(&a.region)?),
                b: Terminal::rect(layers[b.layer], patch(&b.region)?),
                height_nm,
            });
        }
    }
    let mut patches = Vec::new();
    for port in &layout.ports {
        let (a, b) = (&layout.terminals[port.plus], &layout.terminals[port.minus]);
        let (pa, pb) = (patch(&a.region)?, patch(&b.region)?);
        patches.push((pa, pb));
        let mut source =
            PortSpec::gap(&port.name, Terminal::rect(layers[a.layer], pa), Terminal::rect(layers[b.layer], pb));
        source.elevation_nm = options.source_height_nm;
        spec.ports.push(source);
    }
    let region = RegionModel::build(&source, &spec)?;
    if !region.straps.is_empty() {
        return Err(Error::NotSupported(
            "external FDTD requires facing, unobstructed contact patches; automatic package/air straps are not allowed"
                .to_owned(),
        ));
    }
    let rect_shape = |a: [f64; 3], b: [f64; 3]| {
        let (x0, x1, y0, y1) = (a[0].min(b[0]) * 1e6, a[0].max(b[0]) * 1e6, a[1].min(b[1]) * 1e6, a[1].max(b[1]) * 1e6);
        poly::polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
    };
    for (i, port) in region.ports.iter().enumerate() {
        let gap = rect_shape(port.start, port.stop);
        if region.ports[..i].iter().any(|p| {
            (p.start[2] - port.start[2]).abs() < 1e-9
                && !poly::intersection(&gap, &rect_shape(p.start, p.stop)).is_empty()
        }) {
            return Err(Error::NotSupported(
                "external source volumes overlap; independent excitations would share field edges".to_owned(),
            ));
        }
        if port.spec.elevation_nm.is_some()
            && (region.component_posts.iter().any(|p| {
                p.start[2] <= port.start[2]
                    && p.stop[2] >= port.start[2]
                    && !poly::intersection(&gap, &rect_shape(p.start, p.stop)).is_empty()
            }) || region.sheets.iter().filter(|s| (s.z - port.start[2]).abs() < 1e-9).any(|s| {
                !poly::intersection(
                    &gap,
                    &poly::polygon(&s.ring.iter().map(|p| (p.0 * 1e6, p.1 * 1e6)).collect::<Vec<_>>()),
                )
                .is_empty()
            }))
        {
            return Err(Error::NotSupported(format!("elevated source {} intersects another fixture", port.spec.name)));
        }
    }
    // Imported native copper is represented as zone fills. Generic region gap
    // ports normally ignore zones, so explicitly check the source volume here.
    for (port, resolved) in layout.ports.iter().zip(&region.ports).filter(|(_, p)| p.spec.elevation_nm.is_none()) {
        let a = &layout.terminals[port.plus];
        let b = &layout.terminals[port.minus];
        let x0 = resolved.start[0].min(resolved.stop[0]) * 1e6;
        let x1 = resolved.start[0].max(resolved.stop[0]) * 1e6;
        let y0 = resolved.start[1].min(resolved.stop[1]) * 1e6;
        let y1 = resolved.start[1].max(resolved.stop[1]) * 1e6;
        let gap = poly::polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)]);
        let mut terminals = a.region.clone();
        terminals.extend(b.region.clone());
        let gap = poly::difference(&gap, &poly::union(&terminals));
        if layout.layers[a.layer].conductors.iter().any(|c| !poly::intersection(&gap, &c.shapes).is_empty()) {
            return Err(Error::NotSupported(format!(
                "source gap {} intersects native copper outside its contact patches",
                port.name
            )));
        }
    }
    Ok((region, patches))
}

pub fn extract(
    model: &BoardModel,
    board: &PhysicalBoard,
    input: &HotLoopInput,
    options: &Options,
) -> std::result::Result<Result, Error> {
    if options.meshes == 0
        || options.meshes > 2
        || !options.budget_seconds.is_finite()
        || options.budget_seconds <= 0.0
        || options.cell_nm <= 0
        || options.margin_nm <= 0
        || !options.band_hz.0.is_finite()
        || !options.band_hz.1.is_finite()
        || options.band_hz.0 <= 0.0
        || options.band_hz.1 <= options.band_hz.0
    {
        return Err(Error::Invalid(
            "external FDTD requires one or two meshes and a positive shared time budget".to_owned(),
        ));
    }
    let started = Instant::now();
    let mut matrices = Vec::new();
    let mut runs = Vec::new();
    let mut reasons = Vec::new();
    let mut patches_nm = Vec::new();
    let mut approximations = Vec::new();
    for mesh in 0..options.meshes {
        let (mut region, patches) = region(model, board, input, options, if mesh == 0 { 1.0 } else { 1.0 / 1.5 })?;
        let n = region.ports.len();
        approximations.extend(region.approximations.iter().cloned());
        approximations.extend(region.flags.iter().cloned());
        if region.flags.iter().any(|f| f == "nets_merged_below_floor") {
            reasons.push(format!("mesh {mesh}: copper edges from different nets were merged below the mesh floor; native isolation is not qualified"));
        }
        let remaining = options.budget_seconds - started.elapsed().as_secs_f64();
        if remaining <= 0.0 || region.estimate().seconds * n as f64 > remaining {
            return Err(Error::Budget(format!(
                "external FDTD shared budget exhausted or insufficient for {n} columns; {} columns completed; no partial matrix is usable",
                runs.len()
            )));
        }
        let mut columns = Vec::new();
        for column in 0..n {
            region.spec.budget.max_seconds = (options.budget_seconds - started.elapsed().as_secs_f64())
                / ((n - column) * (options.meshes - mesh)) as f64;
            if region.spec.budget.max_seconds <= 0.0 {
                return Err(Error::Budget(format!(
                    "external FDTD shared budget exhausted; {} columns completed; no partial matrix is usable",
                    runs.len()
                )));
            }
            let response = region.solve_port(column).map_err(|error| {
                Error::Solver(format!(
                    "mesh {mesh}, column {column}; {} columns completed; no partial matrix is usable: {error}",
                    runs.len()
                ))
            })?;
            if !response.stats.converged && response.fit.is_none() {
                reasons.push(format!("mesh {mesh} column {column}: energy did not decay and no accepted tail fit"));
            }
            if !response.uncorrected_ports.is_empty() {
                reasons.push(format!("mesh {mesh} column {column}: incomplete off-diagonal tail fits"));
            }
            if response
                .stats
                .warnings
                .iter()
                .any(|w| w.contains("is PEC (shorted)") || w.contains("already carries a lumped element"))
            {
                reasons.push(format!(
                    "mesh {mesh} column {column}: source edges were shorted or shared in the discretized model"
                ));
            }
            runs.push(response.stats.clone());
            columns.push(response);
        }
        let matrix = PortMatrix {
            freqs: columns[0].freqs.clone(),
            names: region.ports.iter().map(|p| p.spec.name.clone()).collect(),
            z_ref: region.ports.iter().map(|p| p.spec.z_ref).collect(),
            s: (0..n).map(|i| (0..n).map(|j| columns[j].s[i].clone()).collect()).collect(),
            stats: Vec::new(),
        };
        matrices.push(matrix);
        patches_nm = patches;
    }
    let chosen = matrices.last().expect("at least one mesh");
    let mut impedance_ohm = Vec::new();
    let mut difference: f64 = 0.0;
    for f in 0..chosen.freqs.len() {
        let z = chosen.z_at(f).ok_or_else(|| Error::Solver("singular full-wave S-to-Z conversion".to_owned()))?;
        let coarse =
            matrices[0].z_at(f).ok_or_else(|| Error::Solver("singular coarse S-to-Z conversion".to_owned()))?;
        for i in 0..z.len() {
            for j in 0..z.len() {
                if !z[i][j].re.is_finite() || !z[i][j].im.is_finite() {
                    return Err(Error::Solver("non-finite full-wave matrix".to_owned()));
                }
                let scale = (z[i][i].norm() * z[j][j].norm()).sqrt().max(1e-12);
                difference = difference.max(100.0 * (z[i][j] - coarse[i][j]).norm() / scale);
                if (z[i][j] - z[j][i]).norm() / scale > 0.01 {
                    reasons.push(format!("nonreciprocal matrix at frequency index {f}, entry {i},{j}"));
                }
            }
        }
        let values: Vec<Vec<_>> = z.iter().map(|row| row.iter().map(|v| [v.re, v.im]).collect()).collect();
        if !passive(&values) {
            reasons.push(format!("Hermitian impedance matrix is not passive within 1 ppm at frequency index {f}"));
        }
        impedance_ohm.push(values);
    }
    if options.meshes == 1 {
        reasons.push("single mesh: convergence not established".to_owned());
    } else if difference > 5.0 {
        reasons.push(format!("full matrix changes by {difference:.2}% between meshes (limit 5%)"));
    }
    reasons.sort();
    reasons.dedup();
    approximations.sort();
    approximations.dedup();
    Ok(Result {
        frequencies_hz: chosen.freqs.clone(),
        labels: chosen.names.clone(),
        impedance_ohm,
        mesh_difference_pct: (options.meshes == 2).then_some(difference),
        runs,
        status_reasons: reasons,
        patches_nm,
        seconds: started.elapsed().as_secs_f64(),
        approximations,
    })
}

#[cfg(test)]
mod tests {
    use super::passive;

    #[test]
    fn passivity_checks_all_current_combinations_including_phase() {
        assert!(passive(&[vec![[1.0, 3.0], [0.5, 2.0]], vec![[0.5, 2.0], [1.0, 4.0]]]));
        // Positive individual resistances do not make an active matrix passive.
        assert!(!passive(&[vec![[1.0, 0.0], [2.0, 0.0]], vec![[2.0, 0.0], [1.0, 0.0]]]));
        assert!(!passive(&[vec![[1.0, 0.0], [0.0, 2.0]], vec![[0.0, -2.0], [1.0, 0.0]]]));
        assert!(passive(&[vec![[0.0, 1.0]]]));
    }
}

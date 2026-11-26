//! `copper sim --layout RUN`: the routed board of a run as parasitics for the benches that say `use layout;`.
//!
//! The run's `final-board.state.json` (`route-board --checkpoint-dir`, as `emi-check --run` reads it) is checked
//! against the design (`SIM119` when it places components the design does not have or connects pads differently),
//! exported and filled in memory, and extracted by `copper_extract::parasitics`: every net that is not a supply's
//! becomes the network of its tracks, every supply net one node. The extracted pads are then named by the design's
//! component pins. Without `--layout` such a bench is refused with `SIM118`.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use copper_extract::parasitics::{ParasiticOptions, extract_parasitics};
use copper_json::text;
use copper_lang::compiler::Design;
use copper_simulate::layout::{LayoutBranch, LayoutCapacitor, LayoutInput, LayoutNet, LayoutParasitics};

/// The layout input of a run: `run` from `--layout`, only extracted when a selected bench uses layout.
pub fn input(board: &Path, design: &Design, only: Option<&str>, run: Option<&str>, out: &mut dyn Write) -> LayoutInput {
    let Some(run) = run else { return LayoutInput::Absent };
    if !copper_simulate::uses_layout(board, design, only) {
        let _ = text::print(out, &format!("layout: no bench uses layout; {run} is not read"));
        return LayoutInput::Absent;
    }
    match extract(design, run) {
        Ok(parasitics) => {
            let (r, l, c) = parasitics.element_counts();
            let _ = text::print(
                out,
                &format!(
                    "layout: {} ({r} resistors, {l} inductors, {c} capacitors; {})",
                    parasitics.source,
                    parasitics.notes.last().map(String::as_str).unwrap_or_default()
                ),
            );
            LayoutInput::Ready(Arc::new(parasitics))
        }
        Err(message) => LayoutInput::Refused(message),
    }
}

fn extract(design: &Design, run: &str) -> Result<LayoutParasitics, String> {
    let physical = crate::emi_check::routed_board(run).map_err(|message| format!("SIM118: --layout {message}"))?;
    let view = copper_lang::elaborate::elaborate(&design.electrical).map_err(|message| format!("SIM119: {message}"))?;
    let stale = |what: String| {
        format!(
            "SIM119: the routed board {run} is not this design's ({what}); route the board again, a bench that uses layout never runs on a stale layout"
        )
    };
    if let Some(extra) = crate::emi_check::unknown_placements(&physical, &view) {
        return Err(stale(format!("it places components the design does not have: {extra}")));
    }
    if let Some(differences) = crate::emi_check::net_mismatches(&physical, &view) {
        return Err(stale(differences));
    }
    let canonical = |name: &str| view.net_aliases.get(name).cloned().unwrap_or_else(|| name.to_owned());
    let supplied: std::collections::BTreeSet<String> = view.supplies.iter().map(|s| canonical(&s.net)).collect();
    let started = std::time::Instant::now();
    let model = copper_extract::board::board_model(&physical).map_err(|message| format!("SIM119: {run}: {message}"))?;
    let networked =
        physical.nets.iter().map(|n| n.name.clone()).filter(|n| !supplied.contains(&canonical(n))).collect();
    let options = ParasiticOptions { networked, ..ParasiticOptions::default() };
    let extracted = extract_parasitics(&model, &physical, &options)
        .map_err(|message| format!("SIM119: the routed board {run} cannot be extracted: {message}"))?;
    // The design's pins on each extracted net, by pad.
    let components: std::collections::BTreeMap<&str, &copper_lang::model::ComponentInstance> =
        view.components.iter().map(|c| (c.reference.as_str(), c)).collect();
    let mut nets: Vec<LayoutNet> = Vec::new();
    let mut unplaced: Vec<String> = Vec::new();
    for net in &extracted.nets {
        let name = canonical(&net.net);
        let mut pins: Vec<(String, String, usize)> = Vec::new();
        if let Some(design_net) = view.nets.iter().find(|n| canonical(&n.name) == name) {
            for endpoint in &design_net.endpoints {
                let Some(component) = components.get(endpoint.component.as_str()) else { continue };
                let Some(part) = view.library.get(&component.part) else { continue };
                let Some(number) =
                    copper_pcb::physicalize::physical_pin_number(component, part, &view.devices, &endpoint.pin)
                else {
                    continue;
                };
                let board_reference = copper_pcb::kicad_pcb::kicad_reference(&endpoint.component);
                let pad = net.pads.iter().find(|(reference, pad, _)| {
                    (reference == &endpoint.component || reference == &board_reference) && pad == &number
                });
                match pad {
                    Some((_, _, node)) => pins.push((endpoint.component.clone(), endpoint.pin.clone(), *node)),
                    None => unplaced.push(format!("{}.{}", endpoint.component, endpoint.pin)),
                }
            }
        }
        let named = pins.first().map_or(0, |p| p.2);
        nets.push(LayoutNet {
            net: view.nets.iter().find(|n| canonical(&n.name) == name).map_or(name.clone(), |n| n.name.clone()),
            nodes: net.nodes.iter().map(|n| n.label.clone()).collect(),
            named,
            pins,
            branches: net
                .branches
                .iter()
                .map(|b| LayoutBranch {
                    a: b.a,
                    b: b.b,
                    resistance: b.resistance,
                    inductance: b.inductance,
                    length_m: b.length_m,
                })
                .collect(),
            track_length_m: net.track_length_m,
            unattributed: net.unattributed,
        });
    }
    let capacitors = extracted.couplings.iter().map(|c| LayoutCapacitor { a: c.a, b: c.b, farad: c.farad }).collect();
    let mut notes = extracted.notes.clone();
    if !unplaced.is_empty() {
        notes.insert(
            0,
            format!("pins the routed board has no pad for, left on their net's node: {}", unplaced.join(", ")),
        );
    }
    let unattributed: f64 = extracted.nets.iter().map(|n| n.unattributed).sum();
    if unattributed > 0.0 {
        notes.insert(
            0,
            format!(
                "{:.3} pF in all reaches no copper within {} mm of the copper it leaves and is not inserted",
                unattributed * 1e12,
                options.cut_half_width_m * 1e3
            ),
        );
    }
    if let Some(last) = notes.last_mut() {
        last.push_str(&format!(", {:.1} s", started.elapsed().as_secs_f64()));
    }
    let source = crate::emi_check::routed_file(run).unwrap_or_else(|| run.to_owned());
    Ok(LayoutParasitics { source, nets, capacitors, flags: extracted.flags.clone(), notes })
}

//! `copper emi-check --extract --plot-current`: the current distribution of the extraction's hot-loop port as
//! the plain data of `copper_report::emi_current`. Nothing here is solved again, and nothing is written to the
//! report: the data comes from the extraction of the same invocation.

use copper_extract::hotloop::HotLoopExtraction;
use copper_report::emi_current::{Bar, CurrentPanel, LayerPanel, PadMark, PortMark, TransitionRow, ViaLayer, ViaMark};

/// The picture data of one cell's extraction, `None` when it kept no current map.
pub fn panel(cell: &str, result: &HotLoopExtraction) -> Option<CurrentPanel> {
    let currents = result.currents.as_deref()?;
    let map = &currents.map;
    let transitions = map.transitions();
    let layers = map
        .layers
        .iter()
        .map(|l| LayerPanel {
            name: l.name.clone(),
            z_mm: l.z_mm,
            outlines: l.outlines.clone(),
            bars: l
                .bars
                .iter()
                .map(|b| Bar {
                    along_x: b.along_x,
                    centre: b.centre_mm,
                    length: b.length_mm,
                    width: b.width_mm,
                    re: b.current.re,
                    im: b.current.im,
                })
                .collect(),
        })
        .collect();
    // Transitions that carry no current (a net with no copper on the far layers) are left out.
    let kept: Vec<&copper_extract::current::Transition> =
        transitions.iter().filter(|t| t.up + t.down >= 0.02).collect();
    let throughput = |t: &copper_extract::current::Transition| t.up.max(t.down);
    // A layer's panel shows a via as its share of the transition between that layer and the adjacent layer that
    // carries the net's current: of the net's transitions on the layer, the one with the largest throughput.
    let relevant = |net: &str, layer: usize| {
        kept.iter()
            .copied()
            .filter(|t| t.net == net && (t.lower == layer || t.upper == layer))
            .max_by(|a, b| throughput(a).total_cmp(&throughput(b)))
    };
    let vias = map
        .vias
        .iter()
        .enumerate()
        .map(|(v, via)| {
            let mut layer_ids: Vec<usize> = via.segments.iter().flat_map(|s| [s.lower, s.upper]).collect();
            layer_ids.sort_unstable();
            layer_ids.dedup();
            ViaMark {
                name: via.name.clone(),
                net: via.net.clone(),
                position: via.position_mm,
                diameter: via.diameter_mm,
                layers: layer_ids
                    .into_iter()
                    .map(|layer| match relevant(&via.net, layer) {
                        Some(t) => ViaLayer {
                            layer,
                            share: t.vias.iter().find(|m| m.0 == v).map_or(0.0, |m| m.2),
                            throughput: throughput(t),
                        },
                        None => ViaLayer { layer, share: 0.0, throughput: 0.0 },
                    })
                    .collect(),
            }
        })
        .collect();
    let rows = kept
        .iter()
        .map(|t| TransitionRow {
            net: t.net.clone(),
            lower: map.layers[t.lower].name.clone(),
            upper: map.layers[t.upper].name.clone(),
            count: t.vias.len(),
            up: t.up,
            down: t.down,
            top: t.vias.iter().map(|&(v, _, share)| (v, share)).collect(),
        })
        .collect();
    let impedance = result
        .frequencies_hz
        .iter()
        .zip(result.inductance_nh.iter().zip(&result.resistance_ohm))
        .map(|(f, (l, r))| (*f, *l, r * 1e3))
        .collect();
    Some(CurrentPanel {
        cell: cell.to_owned(),
        frequency_hz: map.frequency_hz,
        window: map.window_mm,
        layers,
        vias,
        pads: currents
            .pads
            .iter()
            .map(|p| PadMark { label: p.label.clone(), layer: p.layer, bounds: p.bounds_mm })
            .collect(),
        port: PortMark { plus: map.port.plus, minus: map.port.minus },
        transitions: rows,
        impedance,
        status: result.status.to_owned(),
        status_reasons: result.status_reasons.clone(),
        filaments: result.meshes.first().map_or(0, |m| m.filaments),
    })
}

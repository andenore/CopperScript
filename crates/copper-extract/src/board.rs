//! A board window as an extraction layout: the real filled copper of the nets that matter, from the native zone fill.
//!
//! **Where the copper comes from.** `copper_cam::fill` works on its own board model (`copper_cam::board::Board`, the
//! drawing model of a `.kicad_pcb`), not on a `PhysicalBoard`. The physical board is therefore exported in memory
//! with `copper_pcb::kicad_pcb::generate` (the same artifacts `export-kicad` writes), parsed with
//! `copper_cam::board::parse_board`, and its zones are filled with `copper_cam::fill::fill_zones` under the rules of
//! the exported `.kicad_pro` and `.kicad_dru`. The fills carry the zone clearances, thermal reliefs, openings for
//! other nets' vias and pads, the minimum width and the removal of islands that touch no pad (the zone's setting).
//! Tracks, arcs, pads and vias are read from the same parsed board, so one net namespace (the exported KiCad net
//! names) holds everything. Nothing is written to disk.
//!
//! **Which nets.** The nets of the loop's pads and the nets of every zone fill that reaches the window (planes).
//! Copper of other nets in the window is not modelled (listed in `not_modelled`).
//!
//! **Stack-up.** Layer heights are those of `copper_fullwave::stack` (a declared stack-up, else the board thickness
//! spread over the copper layers, flag `assumed_stackup`); the copper thickness is the stack-up's, else 35 um (flag
//! `assumed_copper_thickness`). Via plating is 25 um (flag `assumed_plating`): the export does not carry it.

use std::collections::{BTreeMap, BTreeSet};

use copper_cam::board::{Layer, Pad, PadKind, PadShape, TrackPath};
use copper_cam::geometry::flatten_arc;
use copper_cam::poly::{self, Shapes};
use copper_fullwave::hotloop::HotLoopInput;
use copper_pcb::board::PhysicalBoard;

use crate::layout::{Conductor, Hint, Layout, Terminal, ViaGeom};
use crate::mesh::DEFAULT_PLATING_M;

const NM: f64 = 1e-9;

/// The exported, parsed and filled board (the shared code of `copper_fullwave::fill`, which the full-wave region
/// model uses for its pours too).
pub use copper_fullwave::fill::FilledBoard as BoardModel;

/// Export `board` in memory, parse it and fill its zones natively.
pub fn board_model(board: &PhysicalBoard) -> Result<BoardModel, String> {
    copper_fullwave::fill::export_and_fill(board)
}

/// The pads of a hot loop and what is a port and what a short.
#[derive(Clone, Debug)]
pub struct LoopPorts {
    /// `(component, pad)` of every terminal.
    pub pads: Vec<(String, String)>,
    pub labels: Vec<String>,
    /// The two terminals of the port.
    pub port: (usize, usize),
    /// Pairs of terminals closed by a short (a capacitor or diode on the path).
    pub shorts: Vec<(usize, usize)>,
    /// What each closure stands for, for the notes.
    pub closures: Vec<String>,
    /// The copper legs of the loop as pairs of terminals (consecutive pads of one net).
    pub legs: Vec<(usize, usize)>,
}

/// The port and shorts of a buck hot loop, as `copper-fullwave` places them: the port across the high-side switch
/// (hot-supply pad to switch pad), the capacitor and the catch diode (or the low-side switch) shorted.
pub fn loop_ports(input: &HotLoopInput) -> Result<LoopPorts, String> {
    let n = input.pads.len();
    let mut pads = input.pads.clone();
    let mut labels = input.labels.clone();
    type Closing = ((usize, usize), Vec<(usize, usize)>, Vec<String>);
    let (port, shorts, closures): Closing = match n {
        6 => (
            (1, 2),
            vec![(3, 4), (5, 0)],
            vec![format!("catch diode {}", pads[3].0), format!("input capacitor {}", input.capacitor)],
        ),
        4 if input.switch_pad.is_some() => {
            let sw = input.switch_pad.clone().expect("checked");
            labels.push(format!("switch {}.{}", sw.0, sw.1));
            pads.push(sw);
            (
                (1, 4),
                vec![(4, 2), (3, 0)],
                vec![
                    "low-side switch (hot return to switch pad)".to_owned(),
                    format!("input capacitor {}", input.capacitor),
                ],
            )
        }
        4 => ((1, 2), vec![(3, 0)], vec![format!("input capacitor {}", input.capacitor)]),
        _ => return Err(format!("a hot loop of {n} pads has no port definition (a buck loop has 4 or 6)")),
    };
    let legs = if n == 6 { vec![(0, 1), (2, 3), (4, 5)] } else { vec![(0, 1), (2, 3)] };
    Ok(LoopPorts { pads, labels, port, shorts, closures, legs })
}

/// What the caller chooses.
#[derive(Clone, Debug)]
pub struct BoardOptions {
    /// Window margin around the loop, nm.
    pub margin_nm: i64,
    /// Put the port in the middle of a copper bar across the switch (pad centre to pad centre, the bar the closed
    /// form draws through the controller) instead of between the two pad centres. The port then sees the flux of
    /// the gap between the pads, as the closed form and the full-wave port do; without the bar the gap is not in the
    /// extracted path (the pure pad-to-pad interconnect).
    pub switch_bar: bool,
    /// References of further capacitors of the cell's input network (on the hot supply and return nets): each gets
    /// a port across its pads, after the hot loop's port (port 0). The hot loop's port is then oriented so that
    /// its current and every capacitor port's current leave the hot node along the V+ copper (positive mutual
    /// inductance when they share it).
    pub capacitor_ports: Vec<String>,
    /// Join the pads of the `capacitor_ports` capacitors to the hot capacitor's pads with straight bars where no
    /// copper does (the legs the closed form assumes), the connection of the estimate of section 10.
    pub connection_legs: bool,
}

impl Default for BoardOptions {
    fn default() -> Self {
        BoardOptions { margin_nm: 3_000_000, switch_bar: true, capacitor_ports: Vec::new(), connection_legs: false }
    }
}

/// A pad of the loop (or of a capacitor with a port), for pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopPad {
    /// `U1.2`, the component and pad number.
    pub label: String,
    /// Layer index into the layout's layers.
    pub layer: usize,
    pub net: String,
    /// Bounding box of the pad, mm: `[x0, y0, x1, y1]`.
    pub bounds_mm: [f64; 4],
}

/// The layout of a hot loop window and its bookkeeping.
pub struct BoardLayout {
    pub layout: Layout,
    pub flags: Vec<String>,
    pub notes: Vec<String>,
    pub not_modelled: Vec<String>,
    /// Nets modelled.
    pub nets: Vec<String>,
    /// Window in millimetres, `[x0, y0, x1, y1]`.
    pub window_mm: [f64; 4],
    /// Layer names in stack order and their heights, mm.
    pub layers: Vec<(String, f64)>,
    pub tracks: usize,
    pub pads: usize,
    pub fill_shapes: usize,
    pub vias: usize,
    /// Zone fills with an island (a piece that joins nothing in the window) are counted by the mesher; this counts
    /// the nets the layout holds but that carry no port terminal.
    pub other_nets: Vec<String>,
    /// The ports in layout order: `hot loop`, then the references of the options' `capacitor_ports`.
    pub port_labels: Vec<String>,
    /// The pads of the loop and of the capacitors that have a port.
    pub loop_pads: Vec<LoopPad>,
}

fn corner_radius(pad: &Pad) -> f64 {
    match pad.shape {
        PadShape::RoundRect(ratio) => (pad.width.min(pad.height) as f64 * ratio).round(),
        PadShape::Oval => pad.width.min(pad.height) as f64 / 2.0,
        PadShape::Circle => pad.width as f64 / 2.0,
        PadShape::Rect => 0.0,
    }
}

fn pad_shape(pad: &Pad, max_error: f64) -> Shapes {
    let (w, h) = match pad.shape {
        PadShape::Circle => (pad.width as f64, pad.width as f64),
        _ => (pad.width as f64, pad.height as f64),
    };
    poly::rounded_rect(pad.center, w, h, corner_radius(pad), pad.angle, max_error)
}

/// Whether the pad has copper on `layer` (a non-plated pad no larger than its hole has none), as the filler decides.
fn flashes(pad: &Pad, layer: usize) -> bool {
    pad.layers.has(Layer::Copper(layer))
        && !(pad.kind == PadKind::NonPlated
            && matches!(pad.shape, PadShape::Circle | PadShape::Oval)
            && pad.drill.is_some_and(|(w, h)| pad.width <= w && pad.height <= h))
}

fn bounds_of(shapes: &Shapes) -> Option<(i64, i64, i64, i64)> {
    shapes.iter().map(|s| poly::bounds(s)).reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

fn overlaps(a: (i64, i64, i64, i64), w: [i64; 4]) -> bool {
    a.0 <= w[2] && a.2 >= w[0] && a.1 <= w[3] && a.3 >= w[1]
}

fn copper_thickness_m(board: &PhysicalBoard, layer: copper_physical::CopperLayer) -> Option<f64> {
    board
        .stackup
        .physical_layers
        .iter()
        .find(|l| l.copper_layer == Some(layer))
        .map(|l| l.thickness_nm as f64 * NM)
        .filter(|t| *t > 0.0)
}

/// Build the extraction layout of the window of `ports`' loop.
pub fn build_layout(
    model: &BoardModel,
    board: &PhysicalBoard,
    input: &HotLoopInput,
    ports: &LoopPorts,
    options: &BoardOptions,
    cell_nm: i64,
    coarse_nm: i64,
) -> Result<BoardLayout, String> {
    let cam = &model.cam;
    let max_error = 5_000.0;
    let stack = copper_fullwave::stack::stack(board).map_err(|e| e.to_string())?;
    let mut flags: Vec<String> = Vec::new();
    if !stack.declared {
        flags.push("assumed_stackup".to_owned());
    }
    // Layers: the cam board's copper layers by name, with heights and thickness.
    let mut layout = Layout::new([0, 0, 1, 1]);
    let mut layer_info: Vec<(String, f64)> = Vec::new();
    let mut thickness_assumed = false;
    for name in &cam.copper_layers {
        let physical = stack
            .copper
            .iter()
            .find(|(l, _)| l.value() == name)
            .ok_or_else(|| format!("copper layer {name} of the export is not in the stack-up"))?;
        let thickness = copper_thickness_m(board, physical.0).unwrap_or_else(|| {
            thickness_assumed = true;
            35e-6
        });
        layout.add_layer(name, physical.1 * 1e-3, thickness);
        layer_info.push((name.clone(), physical.1));
    }
    if thickness_assumed {
        flags.push("assumed_copper_thickness".to_owned());
    }
    // Terminals' pads in the cam board.
    let find_pads = |component: &str, number: &str| -> Vec<(&copper_cam::board::Footprint, &Pad)> {
        let alt = copper_pcb::kicad_pcb::kicad_reference(component);
        cam.pads().filter(|(f, p)| (f.reference == component || f.reference == alt) && p.number == number).collect()
    };
    let mut terminal_pads: Vec<Vec<&Pad>> = Vec::new();
    for (component, number) in &ports.pads {
        let found = find_pads(component, number);
        if found.is_empty() {
            return Err(format!("pad {component}.{number} of the loop is not on the exported board"));
        }
        terminal_pads.push(found.into_iter().map(|(_, p)| p).collect());
    }
    // Window: the pads, the loop polygon, the margin.
    let mut bb: Option<(i64, i64, i64, i64)> = None;
    let mut grow = |b: (i64, i64, i64, i64)| {
        bb = Some(bb.map_or(b, |o| (o.0.min(b.0), o.1.min(b.1), o.2.max(b.2), o.3.max(b.3))));
    };
    for pads in &terminal_pads {
        for pad in pads {
            if let Some(b) = bounds_of(&pad_shape(pad, max_error)) {
                grow(b);
            }
        }
    }
    for p in &input.polygon_mm {
        let (x, y) = ((p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64);
        grow((x, y, x, y));
    }
    // The capacitors that get a port are inside the window too (their pads are the terminals).
    for reference in &options.capacitor_ports {
        for (_, pad) in cam.pads().filter(|(f, _)| {
            &f.reference == reference || f.reference == copper_pcb::kicad_pcb::kicad_reference(reference)
        }) {
            if let Some(b) = bounds_of(&pad_shape(pad, max_error)) {
                grow(b);
            }
        }
    }
    let bb = bb.ok_or("the loop has no geometry")?;
    let m = options.margin_nm;
    let window = [bb.0 - m, bb.1 - m, bb.2 + m, bb.3 + m];
    layout.window = window;
    // Nets: those of the loop pads and of every fill that reaches the window.
    let mut nets: BTreeSet<String> = BTreeSet::new();
    for pads in &terminal_pads {
        for pad in pads {
            if let Some(n) = &pad.net {
                nets.insert(n.clone());
            }
        }
    }
    let loop_nets = nets.clone();
    for fill in &model.fills {
        let zone = &cam.zones[fill.zone];
        let reaches = fill.shapes.iter().any(|s| overlaps(poly::bounds(s), window));
        if reaches && let Some(n) = &zone.net {
            nets.insert(n.clone());
        }
    }
    // Zone fills are cut to a corridor: the loop polygon (the traced path through the loop's pads and vias) grown by
    // the margin, so that a long loop does not mesh the whole rectangle of its bounding box. The planes under and
    // beside the loop are therefore finite; the flag `plane_corridor` says so.
    let corridor: Option<Shapes> = (input.polygon_mm.len() >= 3).then(|| {
        let points: Vec<(f64, f64)> = input.polygon_mm.iter().map(|p| (p[0] * 1e6, p[1] * 1e6)).collect();
        let mut area = poly::polygon(&points);
        for pads in &terminal_pads {
            for pad in pads {
                area.extend(pad_shape(pad, max_error));
            }
        }
        for reference in &options.capacitor_ports {
            let alt = copper_pcb::kicad_pcb::kicad_reference(reference);
            for (_, pad) in cam.pads().filter(|(f, _)| &f.reference == reference || f.reference == alt) {
                area.extend(pad_shape(pad, max_error));
            }
        }
        poly::offset(&poly::union(&area), m, true, max_error)
    });
    // Conductors.
    let mut counts = (0usize, 0usize, 0usize, 0usize);
    let mut cut_fills = false;
    for fill in &model.fills {
        let zone = &cam.zones[fill.zone];
        let Some(net) = zone.net.as_ref().filter(|n| nets.contains(*n)) else { continue };
        if fill.shapes.is_empty() {
            continue;
        }
        let shapes = match &corridor {
            Some(c) => {
                let clipped = poly::intersection(&fill.shapes, c);
                cut_fills |= clipped.iter().map(|s| poly::shape_area(s)).sum::<f64>()
                    < 0.98 * fill.shapes.iter().map(|s| poly::shape_area(s)).sum::<f64>();
                clipped
            }
            None => fill.shapes.clone(),
        };
        if shapes.is_empty() {
            continue;
        }
        counts.2 += shapes.len();
        layout.layers[fill.layer].conductors.push(Conductor { net: net.clone(), shapes, on_loop: false });
    }
    if cut_fills {
        flags.push("plane_corridor".to_owned());
    }
    let mut hints: Vec<(usize, Hint)> = Vec::new();
    let z_of = |layer: usize| layout.layers[layer].z_m;
    let z_values: Vec<f64> = (0..layout.layers.len()).map(z_of).collect();
    let track_hints = |layer: usize, a: (f64, f64), b: (f64, f64), width: f64, hints: &mut Vec<(usize, Hint)>| {
        let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
        let half = width / 2.0;
        let rect = [
            (a.0.min(b.0) - half) as i64,
            (a.1.min(b.1) - half) as i64,
            (a.0.max(b.0) + half) as i64,
            (a.1.max(b.1) + half) as i64,
        ];
        let (fine_x, fine_y) = (dy > 0.2 * dx || dx == 0.0, dx > 0.2 * dy || dy == 0.0);
        // `fine_x` means fine lines along x are wanted, i.e. the segment is not horizontal.
        for (target, &z) in z_values.iter().enumerate() {
            let dz = (z - z_values[layer]).abs();
            let (reach, pitch) = if target == layer {
                (2 * cell_nm, cell_nm)
            } else {
                let reach = ((4.0 * dz / NM) as i64).clamp(2 * cell_nm, 3_000_000);
                let pitch = ((dz / NM / 2.0) as i64).clamp(cell_nm, coarse_nm);
                (reach, pitch)
            };
            let grown = [rect[0] - reach, rect[1] - reach, rect[2] + reach, rect[3] + reach];
            hints.push((
                target,
                Hint {
                    rect: [
                        if fine_x { grown[0] } else { rect[0] },
                        if fine_y { grown[1] } else { rect[1] },
                        if fine_x { grown[2] } else { rect[2] },
                        if fine_y { grown[3] } else { rect[3] },
                    ],
                    cell_x_nm: if fine_x { pitch } else { 0 },
                    cell_y_nm: if fine_y { pitch } else { 0 },
                },
            ));
        }
    };
    for track in &cam.tracks {
        let Some(net) = track.net.as_ref().filter(|n| nets.contains(*n)) else { continue };
        let points: Vec<(f64, f64)> = match &track.path {
            TrackPath::Line { start, end } => vec![(start.x as f64, start.y as f64), (end.x as f64, end.y as f64)],
            TrackPath::Arc { start, mid, end } => flatten_arc(*start, *mid, *end, max_error),
        };
        let shapes = poly::polyline(&points, track.width as f64, false, max_error);
        if bounds_of(&shapes).is_none_or(|b| !overlaps(b, window)) {
            continue;
        }
        counts.0 += 1;
        if loop_nets.contains(net) {
            for pair in points.windows(2) {
                track_hints(track.layer, pair[0], pair[1], track.width as f64, &mut hints);
            }
        }
        layout.layers[track.layer].conductors.push(Conductor { net: net.clone(), shapes, on_loop: false });
    }
    let n_layers = cam.copper_count();
    for (_, pad) in cam.pads() {
        let Some(net) = pad.net.as_ref().filter(|n| nets.contains(*n)) else { continue };
        let shape = pad_shape(pad, max_error);
        if bounds_of(&shape).is_none_or(|b| !overlaps(b, window)) {
            continue;
        }
        counts.1 += 1;
        let mut first = usize::MAX;
        let mut last = 0usize;
        for layer in 0..n_layers {
            if flashes(pad, layer) {
                layout.layers[layer].conductors.push(Conductor {
                    net: net.clone(),
                    shapes: shape.clone(),
                    on_loop: terminal_pads.iter().flatten().any(|p| p.center == pad.center),
                });
                first = first.min(layer);
                last = last.max(layer);
            }
        }
        if pad.kind != PadKind::Smd
            && first < last
            && let Some((dw, dh)) = pad.drill
        {
            // A plated hole: a barrel through the layers the pad has copper on (non-plated holes carry no current).
            if pad.kind == PadKind::ThroughHole {
                layout.vias.push(ViaGeom {
                    name: format!("pad {net} {:.2},{:.2}", pad.center.x as f64 * 1e-6, pad.center.y as f64 * 1e-6),
                    x: pad.center.x,
                    y: pad.center.y,
                    net: net.clone(),
                    first_layer: first,
                    last_layer: last,
                    drill_nm: dw.min(dh),
                    plating_nm: (DEFAULT_PLATING_M / NM) as i64,
                    layer_nets: Vec::new(),
                });
                counts.3 += 1;
            }
        }
    }
    for via in &cam.vias {
        let Some(net) = via.net.as_ref().filter(|n| nets.contains(*n)) else { continue };
        if via.center.x < window[0] || via.center.x > window[2] || via.center.y < window[1] || via.center.y > window[3]
        {
            continue;
        }
        // The via's annular ring on every layer: the mesher joins the node nearest to its position.
        let ring = poly::circle(via.center, via.size as f64 / 2.0, max_error);
        for layer in 0..n_layers {
            layout.layers[layer].conductors.push(Conductor { net: net.clone(), shapes: ring.clone(), on_loop: false });
        }
        layout.vias.push(ViaGeom {
            name: format!("via {net} {:.2},{:.2}", via.center.x as f64 * 1e-6, via.center.y as f64 * 1e-6),
            x: via.center.x,
            y: via.center.y,
            net: net.clone(),
            first_layer: 0,
            last_layer: n_layers - 1,
            drill_nm: via.drill,
            plating_nm: (DEFAULT_PLATING_M / NM) as i64,
            layer_nets: Vec::new(),
        });
        counts.3 += 1;
    }
    flags.push("assumed_plating".to_owned());
    for (layer, hint) in hints {
        layout.layers[layer].hints.push(hint);
    }
    // The loop's pads as copper regions, on the layer of each pad's first copper layer.
    let mut pad_regions: Vec<Shapes> = Vec::new();
    let mut pad_layers: Vec<usize> = Vec::new();
    for ((component, number), pads) in ports.pads.iter().zip(&terminal_pads) {
        let first = pads.first().expect("found");
        let layer =
            (0..n_layers).find(|&l| first.layers.has(Layer::Copper(l))).ok_or("a loop pad has no copper layer")?;
        if first.net.is_none() {
            return Err(format!("loop pad {component}.{number} has no net"));
        }
        let mut region: Shapes = Vec::new();
        for pad in pads {
            region.extend(pad_shape(pad, max_error));
        }
        pad_regions.push(poly::union(&region));
        pad_layers.push(layer);
    }
    let mm_bounds = |shapes: &Shapes| -> [f64; 4] {
        bounds_of(shapes)
            .map_or([0.0; 4], |b| [b.0 as f64 * 1e-6, b.1 as f64 * 1e-6, b.2 as f64 * 1e-6, b.3 as f64 * 1e-6])
    };
    let mut loop_pads: Vec<LoopPad> = Vec::new();
    for (k, (component, number)) in ports.pads.iter().enumerate() {
        loop_pads.push(LoopPad {
            label: format!("{component}.{number}"),
            layer: pad_layers[k],
            net: terminal_pads[k][0].net.clone().unwrap_or_default(),
            bounds_mm: mm_bounds(&pad_regions[k]),
        });
    }
    let centre = |k: usize| (terminal_pads[k][0].center.x as f64, terminal_pads[k][0].center.y as f64);
    let bar_width = |a: usize, b: usize| {
        let (pa, pb) = (terminal_pads[a][0], terminal_pads[b][0]);
        (pa.width.min(pa.height).min(pb.width.min(pb.height)) as f64).clamp(200_000.0, 1_000_000.0)
    };
    let mut notes = Vec::new();
    let mut leg_hints: Vec<(usize, Hint)> = Vec::new();
    // Copper legs without routed copper get the straight bar the closed form assumes (width: the smaller pad
    // dimension clamped to 0.2 to 1 mm), as the full-wave model does. The bar is of the pads' net and joins their
    // copper, so it is part of the net's union.
    let n = input.pads.len();
    for &(a, b) in &ports.legs {
        let (from, to) = (&input.labels[a], &input.labels[(a + 1) % n]);
        let routed = input.legs.iter().any(|l| &l.from == from && &l.to == to && l.kind == "traced");
        if routed {
            continue;
        }
        let width = bar_width(a, b);
        let net = terminal_pads[a][0].net.clone().expect("checked");
        let (ca, cb) = (centre(a), centre(b));
        layout.layers[pad_layers[a]].conductors.push(Conductor {
            net,
            shapes: poly::polyline(&[ca, cb], width, false, max_error),
            on_loop: true,
        });
        track_hints(pad_layers[a], ca, cb, width, &mut leg_hints);
        notes.push(format!(
            "leg {from} to {to} has no routed copper: a straight {:.2} mm bar joins the pad centres, as in the closed form",
            width * 1e-6
        ));
    }
    // Contact patches. A component on the path (capacitor, catch diode, low-side switch) is a copper bar between the
    // two pads, from pad edge to pad edge along the line of the pad centres (the bar the closed form draws across
    // its body, without the part that lies inside the pads, which is the pads' own copper); the lumped element
    // (ESL, ESR, package, die) is left out. The bar is a conductor of its own net. Where it meets a pad, the nodes of
    // the bar's end face and the pad's copper next to that face are one equipotential contact (FastHenry's
    // `.equiv`), a patch the width of the bar and a fraction of a cell deep. Nothing overlaps: metal of two nets that
    // overlaps and is joined at a few nodes lets the surplus metal carry eddy loops through the merged nodes.
    let depth = 10_000;
    let mut link_hints: Vec<(usize, Hint)> = Vec::new();
    // `contact(pad, bar_net, bar)`: join the bar's end face to `pad` when the bar is of another net than the pad.
    let contact = |layout: &mut Layout, pad: usize, bar_net: &str, bar: &Shapes, label: &str| {
        let pad_net = terminal_pads[pad][0].net.clone().expect("checked");
        if pad_net == bar_net {
            return;
        }
        let layer = pad_layers[pad];
        let near_pad = poly::offset(&pad_regions[pad], depth, false, max_error);
        let near_bar = poly::offset(bar, depth, false, max_error);
        let on_bar = poly::intersection(bar, &near_pad);
        let on_pad = poly::intersection(&pad_regions[pad], &near_bar);
        layout.terminals.push(Terminal {
            name: format!("{label} contact (bar)"),
            layer,
            net: bar_net.to_owned(),
            region: on_bar,
        });
        let bar_t = layout.terminals.len() - 1;
        layout.terminals.push(Terminal { name: format!("{label} contact (pad)"), layer, net: pad_net, region: on_pad });
        let pad_t = layout.terminals.len() - 1;
        layout.shorts.push(vec![bar_t, pad_t]);
    };
    // The bar between two pads, clipped to the space between them.
    let between = |a: usize, b: usize| -> (Shapes, f64) {
        let width = bar_width(a, b);
        let capsule = poly::polyline(&[centre(a), centre(b)], width, false, max_error);
        let mut both = pad_regions[a].clone();
        both.extend(pad_regions[b].iter().cloned());
        (poly::difference(&capsule, &poly::union(&both)), width)
    };
    for (k, &(a, b)) in ports.shorts.iter().enumerate() {
        let label = ports.closures.get(k).map_or("component", String::as_str);
        let (bar, width) = between(a, b);
        if pad_layers[a] != pad_layers[b] || bar.is_empty() {
            return Err(format!(
                "{label}: the pads {} and {} have no copper space between them on one layer",
                ports.labels[a], ports.labels[b]
            ));
        }
        // The bar is of the net of pad a (it touches that pad, so the copper is one piece) and meets pad b at a contact.
        let net = terminal_pads[a][0].net.clone().expect("checked");
        layout.layers[pad_layers[a]].conductors.push(Conductor {
            net: net.clone(),
            shapes: bar.clone(),
            on_loop: true,
        });
        contact(&mut layout, b, &net, &bar, label);
        track_hints(pad_layers[a], centre(a), centre(b), width, &mut link_hints);
        notes.push(format!(
            "{label} is a {:.2} mm copper bar between its pads (the bar the closed form draws across its body, from pad edge to pad edge)",
            width * 1e-6
        ));
    }
    // The port. Across the switch: a bar between the two pads with a one-cell gap in the middle (so the port sees
    // the flux of the gap, as the closed form and the full-wave port do), or, without the bar, between patches of
    // the two pads facing each other.
    let (pa, pb) = ports.port;
    if options.switch_bar && pad_layers[pa] == pad_layers[pb] {
        let (bar, width) = between(pa, pb);
        if bar.is_empty() {
            return Err("the switch's pads touch: no space for the port".to_owned());
        }
        let layer = pad_layers[pa];
        let (ca, cb) = (centre(pa), centre(pb));
        let length = (cb.0 - ca.0).hypot(cb.1 - ca.1);
        let unit = ((cb.0 - ca.0) / length, (cb.1 - ca.1) / length);
        let mid = ((ca.0 + cb.0) / 2.0, (ca.1 + cb.1) / 2.0);
        let gap = (cell_nm as f64).max(100_000.0);
        // The gap: a rectangle across the bar, `gap` long along it, wider than the bar.
        let across = (-unit.1, unit.0);
        let (hl, hw) = (gap / 2.0, width);
        let corners = [(-hl, -hw), (hl, -hw), (hl, hw), (-hl, hw)]
            .map(|(u, v)| (mid.0 + unit.0 * u + across.0 * v, mid.1 + unit.1 * u + across.1 * v));
        let cut = poly::polygon(&corners);
        let halves = poly::difference(&bar, &cut);
        let (mut h0, mut h1): (Shapes, Shapes) = (Vec::new(), Vec::new());
        for shape in halves {
            let b = poly::bounds(&shape);
            let c = (((b.0 + b.2) / 2) as f64, ((b.1 + b.3) / 2) as f64);
            if (c.0 - mid.0) * unit.0 + (c.1 - mid.1) * unit.1 < 0.0 {
                h0.push(shape);
            } else {
                h1.push(shape);
            }
        }
        if h0.is_empty() || h1.is_empty() {
            return Err("the switch bar could not be split for the port".to_owned());
        }
        let mut ends = Vec::new();
        for (k, (half_shapes, pad)) in [(h0, pa), (h1, pb)].into_iter().enumerate() {
            // Each half is of the net of the pad it touches; the port is between their ends at the gap.
            let net = terminal_pads[pad][0].net.clone().expect("checked");
            layout.layers[layer].conductors.push(Conductor {
                net: net.clone(),
                shapes: half_shapes.clone(),
                on_loop: true,
            });
            let near_gap = poly::offset(&cut, depth, false, max_error);
            let region = poly::intersection(&half_shapes, &near_gap);
            layout.terminals.push(Terminal { name: format!("switch half {k} port end"), layer, net, region });
            ends.push(layout.terminals.len() - 1);
        }
        track_hints(layer, ca, cb, width, &mut link_hints);
        layout.add_port("hot loop", ends[0], ends[1]);
        notes.push(format!(
            "the switch is a {:.2} mm copper bar between its pads with the port in a {:.2} mm gap in its middle (the bar the closed form draws through the controller; the die is not modelled)",
            width * 1e-6,
            gap * 1e-6
        ));
    } else {
        // Patches of the two pads, each the part of the pad within three cells of the other pad.
        let mut terms = Vec::new();
        for (pad, other) in [(pa, pb), (pb, pa)] {
            let near = poly::offset(&pad_regions[other], (cell_nm * 3).max(300_000), false, max_error);
            let mut region = poly::intersection(&pad_regions[pad], &near);
            if region.is_empty() {
                region = pad_regions[pad].clone();
            }
            layout.terminals.push(Terminal {
                name: format!("{} port", ports.labels[pad]),
                layer: pad_layers[pad],
                net: terminal_pads[pad][0].net.clone().expect("checked"),
                region,
            });
            terms.push(layout.terminals.len() - 1);
        }
        layout.add_port("hot loop", terms[0], terms[1]);
        notes.push(
            "the port is between the facing sides of the two switch pads; the gap between the pads is not in the extracted path"
                .to_owned(),
        );
    }
    let mut port_labels = vec!["hot loop".to_owned()];
    if !options.capacitor_ports.is_empty() {
        // Port 0 the other way round: the current leaves the hot supply pad's copper into the switch, as the
        // branch currents of the conducted circuit leave the hot node.
        let p0 = layout.ports[0].clone();
        layout.ports[0] = crate::layout::PortDef { name: p0.name, plus: p0.minus, minus: p0.plus };
        let supply_net = terminal_pads[0][0].net.clone().ok_or("the hot capacitor's pad has no net")?;
        let return_net =
            terminal_pads[ports.pads.len() - 1][0].net.clone().ok_or("the hot capacitor's pad has no net")?;
        let hot_supply_pad = 0;
        let hot_return_pad = ports.pads.len() - 1;
        for reference in &options.capacitor_ports {
            let alt = copper_pcb::kicad_pcb::kicad_reference(reference);
            let pads: Vec<&Pad> =
                cam.pads().filter(|(f, _)| &f.reference == reference || f.reference == alt).map(|(_, p)| p).collect();
            let find = |net: &str| pads.iter().copied().find(|p| p.net.as_deref() == Some(net));
            let (Some(sp), Some(rp)) = (find(&supply_net), find(&return_net)) else {
                return Err(format!(
                    "capacitor {reference}: no pad on {supply_net} and on {return_net} on the exported board"
                ));
            };
            let layer = (0..n_layers)
                .find(|&l| sp.layers.has(Layer::Copper(l)) && rp.layers.has(Layer::Copper(l)))
                .ok_or_else(|| format!("capacitor {reference}: its pads share no copper layer"))?;
            if layer != pad_layers[hot_supply_pad] {
                return Err(format!("capacitor {reference} is not on the layer of the hot capacitor's pads"));
            }
            // A port across the pads: a small patch at the middle of each pad (the body of the capacitor).
            let mut terminals = Vec::new();
            for pad in [rp, sp] {
                let half = ((pad.width.min(pad.height) as f64) * 0.2).max(75_000.0) as i64;
                let (cx0, cy0) = (pad.center.x, pad.center.y);
                let region = poly::intersection(
                    &pad_shape(pad, max_error),
                    &poly::polygon(&[
                        ((cx0 - half) as f64, (cy0 - half) as f64),
                        ((cx0 + half) as f64, (cy0 - half) as f64),
                        ((cx0 + half) as f64, (cy0 + half) as f64),
                        ((cx0 - half) as f64, (cy0 + half) as f64),
                    ]),
                );
                loop_pads.push(LoopPad {
                    label: format!("{reference}.{}", pad.number),
                    layer,
                    net: pad.net.clone().unwrap_or_default(),
                    bounds_mm: mm_bounds(&pad_shape(pad, max_error)),
                });
                layout.terminals.push(Terminal {
                    name: format!("{reference} pad {}", pad.number),
                    layer,
                    net: pad.net.clone().expect("found by net"),
                    region,
                });
                terminals.push(layout.terminals.len() - 1);
                link_hints.push((
                    layer,
                    Hint::square(
                        [cx0 - 2 * half, cy0 - 2 * half, cx0 + 2 * half, cy0 + 2 * half],
                        (cell_nm / 2).max(50_000),
                    ),
                ));
            }
            layout.add_port(reference, terminals[0], terminals[1]);
            port_labels.push(reference.clone());
            if options.connection_legs {
                for (pad, hot_pad) in [(sp, hot_supply_pad), (rp, hot_return_pad)] {
                    let width = (pad
                        .width
                        .min(pad.height)
                        .min(terminal_pads[hot_pad][0].width.min(terminal_pads[hot_pad][0].height))
                        as f64)
                        .clamp(200_000.0, 1_000_000.0);
                    let (a, b) = ((pad.center.x as f64, pad.center.y as f64), centre(hot_pad));
                    let net = pad.net.clone().expect("found by net");
                    layout.layers[layer].conductors.push(Conductor {
                        net,
                        shapes: poly::polyline(&[a, b], width, false, max_error),
                        on_loop: true,
                    });
                    track_hints(layer, a, b, width, &mut link_hints);
                }
                notes.push(format!(
                    "capacitor {reference} is joined to the hot capacitor's pads by straight bars pad centre to pad centre (the connection the estimate of section 10 assumes)"
                ));
            }
            notes.push(format!(
                "port across the pads of capacitor {reference}: the capacitor itself is not in the copper"
            ));
        }
    }
    for (layer, hint) in leg_hints.into_iter().chain(link_hints) {
        layout.layers[layer].hints.push(hint);
    }
    let mut not_modelled = vec![
        "zone copper beyond the window and beyond the corridor around the loop polygon: planes and pours are finite (a finite plane raises the loop inductance a little above the infinite-plane value)".to_owned(),
        "displacement current and dielectric: the model is magneto-quasi-static (no capacitance, no radiation, no resonance)".to_owned(),
        "current distribution across the copper thickness (one filament per bar: uniform over the thickness), so skin effect is not modelled".to_owned(),
        "components: capacitor and diode are copper bars between their pad centres and the switch is the port (no ESL, ESR, package, die or bond wires)".to_owned(),
        "copper of nets other than those listed in `nets`, and everything outside the window".to_owned(),
    ];
    if !stack.declared {
        not_modelled.push("the real stack-up (undeclared: layers spread over the board thickness)".to_owned());
    }
    notes.push(format!(
        "port: {} to {} (the switch is the port)",
        ports.labels[ports.port.0], ports.labels[ports.port.1]
    ));
    let other_nets: Vec<String> = nets.difference(&loop_nets).cloned().collect();
    Ok(BoardLayout {
        layout,
        flags,
        notes,
        not_modelled,
        nets: nets.iter().cloned().collect(),
        window_mm: window.map(|v| v as f64 * 1e-6),
        layers: layer_info,
        tracks: counts.0,
        pads: counts.1,
        fill_shapes: counts.2,
        vias: counts.3,
        other_nets,
        port_labels,
        loop_pads,
    })
}

/// Helper for tests and reports: the nets of a layout by layer, as `layer -> nets`.
pub fn nets_by_layer(layout: &Layout) -> BTreeMap<String, BTreeSet<String>> {
    layout.layers.iter().map(|l| (l.name.clone(), l.conductors.iter().map(|c| c.net.clone()).collect())).collect()
}

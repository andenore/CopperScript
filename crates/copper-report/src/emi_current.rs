//! The current distribution of the quasi-static extraction as an SVG (`copper emi-check --extract --plot-current`):
//! where the hot loop's current actually flows in the copper.
//!
//! One map per copper layer that carries loop current. Each shows the sheet current `|K| = |I| / w` of every mesh
//! filament of the finest mesh, at one frequency, for the unit current into the port across the switch, as a heat
//! map in dB relative to the largest `|K|` on any layer (a 30-step viridis scale, one dB a step; the colours are
//! the `|J|` of the copper per layer, one compound path per colour); faint copper outlines under it; short arrows
//! on a coarse grid for the direction of the current at the instant the port current peaks; the vias as circles
//! whose area is their share of the current that changes layer there, with the percentage; the loop's pads with
//! their names; the port. The right-hand column lists the loop inductance at the solved frequencies and the
//! division of the current between the vias, from the same solution.
//!
//! The renderer takes plain data ([`CurrentPanel`]) made by the command from `copper_extract`'s current map; the
//! output is hand-written deterministic SVG on a white background with a fixed palette. A screening estimate, and
//! says so in one line.

use crate::emi_field::{
    BOARD, FieldPicture, INK, MUTED, Placer, centroid, halo, path_data, scale_color, stacked_svg, text_px, wrap_px,
};
use crate::emi_impedance::counted;
use crate::emi_plot::{escape, n};
use crate::scene::Point;

/// The colour scale covers this many dB below the maximum, one step each.
pub const RANGE_DB: f64 = 30.0;
const STEPS: usize = 30;
/// A layer is drawn when its peak is within this many dB of the largest on any layer.
pub const LAYER_DB: f64 = 20.0;
/// The port marker's colour (the field map's victim colour).
const PORT: &str = "#ff2bd6";
/// Vias that carry less than this share of their transition are drawn small and without a label.
const LABEL_SHARE: f64 = 0.02;
/// A layer change is listed in full and its vias labelled when it carries at least this much, A per ampere into the port.
pub const LISTED_AMPERE: f64 = 0.1;
/// A via is "near" a loop pad within this distance, mm.
const NEAR_PAD_MM: f64 = 1.5;
/// Outline colours of the via circles by net, in the order of the sorted net names.
const NET_COLOURS: [&str; 5] = ["#1f2933", "#c2410c", "#0e7490", "#7c3aed", "#a16207"];

/// A bar of copper and its current.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub along_x: bool,
    pub centre: [f64; 2],
    pub length: f64,
    pub width: f64,
    /// Current along `+x` (`+y`) per ampere into the port: real and imaginary part.
    pub re: f64,
    pub im: f64,
}

/// One layer's data.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerPanel {
    pub name: String,
    pub z_mm: f64,
    /// Copper shapes (outer contour then holes), mm.
    pub outlines: Vec<Vec<Vec<[f64; 2]>>>,
    pub bars: Vec<Bar>,
}

/// A via and its share of the current that changes layer.
#[derive(Clone, Debug, PartialEq)]
pub struct ViaMark {
    pub name: String,
    pub net: String,
    pub position: [f64; 2],
    pub diameter: f64,
    /// One entry per layer (index into [`CurrentPanel::layers`]) the via joins.
    pub layers: Vec<ViaLayer>,
}

/// A via as one layer's panel shows it: its share of the transition between that layer and the adjacent layer that
/// carries the net's current (the transition of the net with the largest throughput).
#[derive(Clone, Debug, PartialEq)]
pub struct ViaLayer {
    pub layer: usize,
    /// `|I| / sum |I|` over the vias of the net in that transition (0 when this via is not in it).
    pub share: f64,
    /// Current the transition carries, `max(up, down)`, A per ampere into the port.
    pub throughput: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PadMark {
    pub label: String,
    pub layer: usize,
    pub bounds: [f64; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortMark {
    pub plus: (usize, [f64; 2]),
    pub minus: (usize, [f64; 2]),
}

/// The division of a net's current between the vias that join two layers.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionRow {
    pub net: String,
    pub lower: String,
    pub upper: String,
    pub count: usize,
    /// The current they carry towards the top and towards the bottom layer, A per ampere into the port.
    pub up: f64,
    pub down: f64,
    /// `(via index into CurrentPanel::vias, share)`, strongest first.
    pub top: Vec<(usize, f64)>,
}

/// Everything one picture shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentPanel {
    pub cell: String,
    pub frequency_hz: f64,
    pub window: [f64; 4],
    pub layers: Vec<LayerPanel>,
    pub vias: Vec<ViaMark>,
    pub pads: Vec<PadMark>,
    pub port: PortMark,
    pub transitions: Vec<TransitionRow>,
    /// `(frequency Hz, inductance nH, resistance mohm)` of the port across the switch, finest mesh.
    pub impedance: Vec<(f64, f64, f64)>,
    /// `ok` or `indicative`, and why.
    pub status: String,
    pub status_reasons: Vec<String>,
    pub filaments: usize,
}

/// Rasterised sheet current of one layer: complex x and y components averaged over the bars that cover a pixel.
struct Raster {
    x0: f64,
    y0: f64,
    step: f64,
    nx: usize,
    ny: usize,
    /// `|K|` per pixel in A/mm, `NaN` where no bar covers; and the real parts of the two components (the direction
    /// at the instant the port current peaks).
    magnitude: Vec<f64>,
    re_x: Vec<f64>,
    re_y: Vec<f64>,
}

fn rasterise(layer: &LayerPanel, window: [f64; 4], step: f64) -> Raster {
    let nx = (((window[2] - window[0]) / step).ceil() as usize).max(1);
    let ny = (((window[3] - window[1]) / step).ceil() as usize).max(1);
    let total = nx * ny;
    // Sums of the complex sheet current per component and the number of bars that covered the pixel.
    let mut sum = [vec![(0.0, 0.0); total], vec![(0.0, 0.0); total]];
    let mut count = [vec![0u16; total], vec![0u16; total]];
    for bar in &layer.bars {
        if bar.width <= 0.0 || bar.length <= 0.0 {
            continue;
        }
        let (hx, hy) =
            if bar.along_x { (bar.length / 2.0, bar.width / 2.0) } else { (bar.width / 2.0, bar.length / 2.0) };
        let range = |c: f64, h: f64, o: f64, count: usize| -> (usize, usize) {
            let lo = ((c - h - o) / step - 0.5).ceil();
            let hi = ((c + h - o) / step - 0.5).floor();
            let (lo, hi) = if lo > hi {
                let mid = ((c - o) / step - 0.5).round();
                (mid, mid)
            } else {
                (lo, hi)
            };
            (lo.max(0.0) as usize, (hi.max(-1.0).min(count as f64 - 1.0)).max(0.0) as usize)
        };
        let (i0, i1) = range(bar.centre[0], hx, window[0], nx);
        let (j0, j1) = range(bar.centre[1], hy, window[1], ny);
        if i0 >= nx || j0 >= ny {
            continue;
        }
        let k = usize::from(!bar.along_x);
        let (kr, ki) = (bar.re / bar.width, bar.im / bar.width);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let at = j * nx + i;
                sum[k][at].0 += kr;
                sum[k][at].1 += ki;
                count[k][at] = count[k][at].saturating_add(1);
            }
        }
    }
    let mut magnitude = vec![f64::NAN; total];
    let (mut re_x, mut re_y) = (vec![0.0; total], vec![0.0; total]);
    for at in 0..total {
        if count[0][at] == 0 && count[1][at] == 0 {
            continue;
        }
        let part = |k: usize| {
            let c = f64::from(count[k][at].max(1));
            (sum[k][at].0 / c, sum[k][at].1 / c)
        };
        let (x, y) = (part(0), part(1));
        magnitude[at] = (x.0 * x.0 + x.1 * x.1 + y.0 * y.0 + y.1 * y.1).sqrt();
        re_x[at] = x.0;
        re_y[at] = y.0;
    }
    Raster { x0: window[0], y0: window[1], step, nx, ny, magnitude, re_x, re_y }
}

/// The colour step of a level in dB relative to the maximum (`None` below the scale).
fn bin(db: f64) -> Option<usize> {
    if !db.is_finite() || db < -RANGE_DB {
        return None;
    }
    Some((((db + RANGE_DB) / RANGE_DB * STEPS as f64).floor() as usize).min(STEPS - 1))
}

fn text_rows(text: &str, size: f64, width: f64) -> Vec<String> {
    wrap_px(text, size, width)
}

/// The `fraction` quantile of the covered cells of a raster, A/mm (0 without cells).
fn quantile(raster: &Raster, fraction: f64) -> f64 {
    let mut values: Vec<f64> = raster.magnitude.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
    values.sort_by(f64::total_cmp);
    values.get(((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1)).copied().unwrap_or(0.0)
}

/// Peak sheet current of a raster, A/mm.
fn peak(raster: &Raster) -> f64 {
    raster.magnitude.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max)
}

fn frequency_label(hz: f64) -> String {
    if hz >= 1e6 { format!("{} MHz", n(hz / 1e6)) } else { format!("{} kHz", n(hz / 1e3)) }
}

const LEFT: f64 = 24.0;

/// The picture of one cell.
pub fn current_picture(panel: &CurrentPanel) -> FieldPicture {
    picture(panel, false, 0)
}

/// The picture of one cell as a thumbnail for the summary page: its own title and the layer with the strongest
/// current (heat, arrows, pads, vias and the port), a one-row colour scale, a caption and the legend; without the
/// notes and the right-hand column. `id` makes the clip path unique within the page.
pub fn current_thumbnail(panel: &CurrentPanel, id: usize) -> FieldPicture {
    picture(panel, true, id)
}

fn picture(panel: &CurrentPanel, compact: bool, id: usize) -> FieldPicture {
    let full = panel.window;
    let (fw, fh) = ((full[2] - full[0]).max(1e-3), (full[3] - full[1]).max(1e-3));
    // Raster step: 0.05 mm, coarser when the window would exceed about 90 000 pixels.
    let step = (fw * fh / 90_000.0).sqrt().max(0.05);
    let rasters: Vec<Raster> = panel.layers.iter().map(|l| rasterise(l, full, step)).collect();
    // The 0 dB level: the 99th percentile of the cells of the layer with the largest peak, so that the few cells at
    // a terminal or a corner, where the filament mesh crowds the current, do not set the scale (they are drawn in the
    // top colour); the true peak is stated.
    let raw_peak = rasters.iter().map(peak).fold(0.0, f64::max);
    let reference = rasters
        .iter()
        .max_by(|a, b| peak(a).total_cmp(&peak(b)))
        .map(|r| quantile(r, 0.99))
        .filter(|v| *v > 0.0)
        .unwrap_or(raw_peak);
    let rel_db = |k: f64| if reference > 0.0 && k > 0.0 { 20.0 * (k / reference).log10() } else { f64::NEG_INFINITY };
    // A cell with a solution is never white: below the scale it takes the bottom colour. White is copper without a
    // filament.
    let cell_bin = |k: f64| if k.is_nan() { None } else { Some(bin(rel_db(k)).unwrap_or(0)) };
    // Layers drawn: peak within LAYER_DB of the 0 dB level, and the layers of the pads and the port.
    let shown: Vec<usize> = (0..panel.layers.len())
        .filter(|&l| {
            rel_db(quantile(&rasters[l], 0.9)) >= -LAYER_DB
                || panel.pads.iter().any(|p| p.layer == l)
                || panel.port.plus.0 == l
                || panel.port.minus.0 == l
        })
        .collect();
    // The thumbnail shows the one layer that carries the most current.
    let shown: Vec<usize> = if compact {
        shown
            .iter()
            .copied()
            .max_by(|&a, &b| peak(&rasters[a]).total_cmp(&peak(&rasters[b])).then(b.cmp(&a)))
            .into_iter()
            .collect()
    } else {
        shown
    };
    let hidden: Vec<String> = (0..panel.layers.len())
        .filter(|l| !shown.contains(l))
        .map(|l| {
            if rasters[l].magnitude.iter().all(|v| v.is_nan()) {
                format!("{} (no current-carrying copper in the window)", panel.layers[l].name)
            } else {
                format!(
                    "{} ({} dB at the 90th percentile of its cells)",
                    panel.layers[l].name,
                    n(rel_db(quantile(&rasters[l], 0.9)))
                )
            }
        })
        .collect();
    let nets: Vec<&str> = {
        let mut v: Vec<&str> = panel.vias.iter().map(|v| v.net.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let net_colour = |net: &str| NET_COLOURS[nets.iter().position(|n| *n == net).unwrap_or(0) % NET_COLOURS.len()];
    // The layer changes listed in full, and the vias of them that carry at least LABEL_SHARE get a number, in the
    // order of the listing; the panels label those vias with it.
    let is_listed = |t: &TransitionRow| t.up.max(t.down) >= LISTED_AMPERE;
    let mut numbers: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for t in panel.transitions.iter().filter(|t| is_listed(t)) {
        for &(via, share) in &t.top {
            if share >= LABEL_SHARE && !numbers.contains_key(&via) {
                let next = numbers.len() + 1;
                numbers.insert(via, next);
            }
        }
    }
    // The window shown: the cells within the scale, the pads and the port, and 1.5 mm round them.
    let mut crop: Option<[f64; 4]> = None;
    let mut grow = |x: f64, y: f64| {
        crop = Some(crop.map_or([x, y, x, y], |c| [c[0].min(x), c[1].min(y), c[2].max(x), c[3].max(y)]));
    };
    for &l in &shown {
        let r = &rasters[l];
        for j in 0..r.ny {
            for i in 0..r.nx {
                if rel_db(r.magnitude[j * r.nx + i]) >= -RANGE_DB {
                    let (x, y) = (r.x0 + i as f64 * r.step, r.y0 + j as f64 * r.step);
                    grow(x, y);
                    grow(x + r.step, y + r.step);
                }
            }
        }
    }
    for pad in &panel.pads {
        grow(pad.bounds[0], pad.bounds[1]);
        grow(pad.bounds[2], pad.bounds[3]);
    }
    for terminal in [panel.port.plus, panel.port.minus] {
        grow(terminal.1[0], terminal.1[1]);
    }
    let margin = 1.5;
    let [wx0, wy0, wx1, wy1] = crop.map_or(full, |c| {
        [
            (c[0] - margin).max(full[0]),
            (c[1] - margin).max(full[1]),
            (c[2] + margin).min(full[2]),
            (c[3] + margin).min(full[3]),
        ]
    });
    let (ww, wh) = ((wx1 - wx0).max(1e-3), (wy1 - wy0).max(1e-3));
    let scale = if compact { (616.0 / ww).min(270.0 / wh).min(80.0) } else { (620.0 / ww).min(400.0 / wh).min(80.0) };
    let (map_w, map_h) = (ww * scale, wh * scale);

    // Text blocks first: the picture's width depends on them.
    let side = LEFT + map_w + 30.0;
    let column = 330.0;
    let title = format!(
        "{} hot loop: current distribution in the copper, {}extraction at {}",
        panel.cell,
        if compact { "" } else { "quasi-static " },
        frequency_label(panel.frequency_hz)
    );
    let width = if compact {
        (LEFT + map_w + LEFT).max(text_px(&title, 14.0) * 1.07 + 2.0 * LEFT).max(460.0)
    } else {
        (side + column + LEFT).max(560.0)
    };
    let mut out: Vec<String> = Vec::new();
    out.push(format!(
        "<text x=\"{}\" y=\"26\" font-size=\"{}\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
        n(LEFT),
        if compact { 14 } else { 16 },
        escape(&title)
    ));
    let mut y = if compact { 34.0 } else { 46.0 };
    let notes = [format!(
        "Screening estimate from a quasi-static partial-element extraction of the filled copper ({} filaments, status {}{}): the current in every filament for one ampere into the port across the switch. Heat: sheet current |I|/w per mesh cell on a {} dB scale; arrows: direction at the instant the port current peaks. The displacement current and the skin effect are not modelled.",
        panel.filaments,
        panel.status,
        if panel.status_reasons.is_empty() { String::new() } else { format!(": {}", panel.status_reasons.join("; ")) },
        n(RANGE_DB)
    )];
    for text in notes.iter().filter(|_| !compact) {
        for line in text_rows(text, 12.0, width - 2.0 * LEFT) {
            out.push(format!(
                "<text class=\"note\" x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
                n(LEFT),
                n(y),
                escape(&line)
            ));
            y += 15.0;
        }
    }
    let mut top = y + 10.0;
    let heat_bin_max = STEPS;
    let mut defs: Vec<String> = Vec::new();
    let mut column_top = top;
    // Panels.
    for (slot, &l) in shown.iter().enumerate() {
        let layer = &panel.layers[l];
        let raster = &rasters[l];
        let (ox, oy) = (LEFT, top + 22.0);
        let px = |x: f64| ox + (x - wx0) * scale;
        let py = |yv: f64| oy + (yv - wy0) * scale;
        let bounds = [ox, oy, ox + map_w, oy + map_h];
        let layer_peak = peak(raster);
        let layer_reference = quantile(raster, 0.99);
        out.push(format!("<g class=\"layer\" data-layer=\"{}\">", escape(&layer.name)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"13\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
            n(ox),
            n(top + 14.0),
            escape(&format!(
                "{} (z = {} mm): peak {} A/mm, {}{} dB re 0 dB",
                layer.name,
                n(layer.z_mm),
                n(layer_peak),
                if rel_db(layer_peak) > 0.0 { "+" } else { "" },
                n(rel_db(layer_peak).max(-99.0))
            ))
        ));
        let clip = if compact { format!("tclip{id}") } else { format!("clip{slot}") };
        defs.push(format!(
            "<clipPath id=\"{clip}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath>",
            n(ox),
            n(oy),
            n(map_w),
            n(map_h)
        ));
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{BOARD}\"/>",
            n(ox),
            n(oy),
            n(map_w),
            n(map_h)
        ));
        out.push(format!("<g clip-path=\"url(#{clip})\">"));
        // The copper of the layer, a faint fill and a thin outline (one path per shape: overlapping conductors of
        // one net must not cancel).
        let mut fill = String::new();
        for shape in &layer.outlines {
            let mut d = String::new();
            for contour in shape {
                let pixels: Vec<Point> = contour.iter().map(|p| (px(p[0]), py(p[1]))).collect();
                d.push_str(&path_data(&pixels, true));
            }
            fill.push_str(&format!("<path d=\"{d}\"/>"));
        }
        out.push(format!(
            "<g class=\"copper\" fill=\"#ffffff\" fill-opacity=\"0.6\" fill-rule=\"evenodd\" stroke=\"{MUTED}\" stroke-opacity=\"0.55\" stroke-width=\"0.5\">{fill}</g>"
        ));
        // Heat: one compound path per colour step, runs of one step in a row as one box.
        let mut by_bin: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
        for j in 0..raster.ny {
            let row = &raster.magnitude[j * raster.nx..(j + 1) * raster.nx];
            let mut i = 0;
            while i < row.len() {
                let Some(color) = cell_bin(row[i]) else {
                    i += 1;
                    continue;
                };
                let start = i;
                while i < row.len() && cell_bin(row[i]) == Some(color) {
                    i += 1;
                }
                by_bin.entry(color).or_default().push_str(&format!("M{start} {j}h{}v1h-{}z", i - start, i - start));
            }
        }
        out.push(format!(
            "<g class=\"heat\" opacity=\"0.9\" shape-rendering=\"crispEdges\" transform=\"translate({} {}) scale({})\">",
            n(px(raster.x0)),
            n(py(raster.y0)),
            n(raster.step * scale)
        ));
        for (color, d) in &by_bin {
            let fill = scale_color((*color as f64 + 0.5) / heat_bin_max as f64);
            out.push(format!("<path fill=\"{fill}\" stroke=\"{fill}\" stroke-width=\"0.1\" d=\"{d}\"/>"));
        }
        out.push("</g>".to_owned());
        // Direction arrows on a coarse grid: the strongest cell of each block, where it is well above the floor.
        let pitch = (ww / 22.0).clamp(0.6, 1.6);
        let block = (pitch / raster.step).round().max(1.0) as usize;
        let mut casing = String::new();
        let mut arrows = String::new();
        let length = pitch * scale * 0.62;
        let (mut bj, mut arrow_count) = (0usize, 0usize);
        while bj < raster.ny {
            let mut bi = 0usize;
            while bi < raster.nx {
                let mut best: Option<(f64, usize)> = None;
                for j in bj..(bj + block).min(raster.ny) {
                    for i in bi..(bi + block).min(raster.nx) {
                        let at = j * raster.nx + i;
                        let m = raster.magnitude[at];
                        if m.is_finite() && best.is_none_or(|b| m > b.0) {
                            best = Some((m, at));
                        }
                    }
                }
                if let Some((m, at)) = best.filter(|b| b.0 >= 0.15 * layer_reference && rel_db(b.0) >= -RANGE_DB) {
                    let (dx, dy) = (raster.re_x[at], raster.re_y[at]);
                    let norm = dx.hypot(dy);
                    // Only where the real part is a good part of the magnitude: in quadrature the direction at
                    // the peak of the port current is not the direction of the flow.
                    if norm >= 0.5 * m {
                        let (ux, uy) = (dx / norm, dy / norm);
                        let (cx, cy) = (
                            px(raster.x0 + ((at % raster.nx) as f64 + 0.5) * raster.step),
                            py(raster.y0 + ((at / raster.nx) as f64 + 0.5) * raster.step),
                        );
                        let (tx, ty) = (cx + ux * length / 2.0, cy + uy * length / 2.0);
                        let (sx, sy) = (cx - ux * length / 2.0, cy - uy * length / 2.0);
                        let head = length * 0.38;
                        let wing = |sign: f64| {
                            let (rx, ry) = (-ux * 0.866 - sign * uy * 0.5, -uy * 0.866 + sign * ux * 0.5);
                            (tx + rx * head, ty + ry * head)
                        };
                        let (a, b) = (wing(1.0), wing(-1.0));
                        let d = format!(
                            "M{} {}L{} {}M{} {}L{} {}L{} {}",
                            n(sx),
                            n(sy),
                            n(tx),
                            n(ty),
                            n(a.0),
                            n(a.1),
                            n(tx),
                            n(ty),
                            n(b.0),
                            n(b.1)
                        );
                        casing.push_str(&d);
                        arrows.push_str(&d);
                        arrow_count += 1;
                    }
                }
                bi += block;
            }
            bj += block;
        }
        if arrow_count > 0 {
            out.push(format!(
                "<g class=\"arrows\" fill=\"none\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"{casing}\" stroke=\"#111827\" stroke-opacity=\"0.75\" stroke-width=\"2.8\"/><path d=\"{arrows}\" stroke=\"#ffffff\" stroke-width=\"1.2\"/></g>"
            ));
        }
        out.push("</g>".to_owned());

        // Pads, vias and the port with labels placed clear of each other.
        let mut placer = Placer { taken: Vec::new(), bounds, pad: 1.0 };
        out.push(format!(
            "<g class=\"pads\" fill=\"#ffffff\" fill-opacity=\"0.18\" stroke=\"{INK}\" stroke-width=\"1\" clip-path=\"url(#{clip})\">"
        ));
        let layer_pads: Vec<&PadMark> = panel.pads.iter().filter(|p| p.layer == l).collect();
        for pad in &layer_pads {
            let b = pad.bounds;
            out.push(format!(
                "<rect class=\"pad\" data-pad=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                escape(&pad.label),
                n(px(b[0])),
                n(py(b[1])),
                n((b[2] - b[0]) * scale),
                n((b[3] - b[1]) * scale)
            ));
        }
        out.push("</g>".to_owned());
        let on_layer: Vec<(usize, &ViaMark, &ViaLayer)> = panel
            .vias
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.layers.iter().find(|m| m.layer == l).map(|m| (i, v, m)))
            .collect();
        let radius = |share: f64| 2.2 + 9.0 * share.max(0.0).sqrt();
        out.push("<g class=\"vias\">".to_owned());
        for (_, via, mark) in &on_layer {
            let (x, y) = (px(via.position[0]), py(via.position[1]));
            if x < bounds[0] || x > bounds[2] || y < bounds[1] || y > bounds[3] {
                continue;
            }
            let share = if mark.throughput >= LISTED_AMPERE { mark.share } else { 0.0 };
            let r = radius(share).max(via.diameter * scale / 2.0);
            out.push(format!(
                "<circle class=\"via\" data-via=\"{}\" data-net=\"{}\" data-share=\"{}\" cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#ffffff\" fill-opacity=\"0.8\" stroke=\"{}\" stroke-width=\"1.6\"/>",
                escape(&via.name),
                escape(&via.net),
                n(share * 100.0),
                n(x),
                n(y),
                n(r),
                net_colour(&via.net)
            ));
            placer.reserve([x - r, y - r, x + r, y + r]);
        }
        out.push("</g>".to_owned());
        let mut labels: Vec<String> = Vec::new();
        for (index, via, mark) in &on_layer {
            let (x, y) = (px(via.position[0]), py(via.position[1]));
            let Some(number) = numbers.get(index) else { continue };
            if mark.share < LABEL_SHARE
                || mark.throughput < LISTED_AMPERE
                || x < bounds[0]
                || x > bounds[2]
                || y < bounds[1]
                || y > bounds[3]
            {
                continue;
            }
            let text = format!("{number}: {} %", n((mark.share * 100.0).round()));
            let w = text_px(&text, 10.0) * 1.05;
            let r = radius(mark.share);
            let (lx, ly) = placer.place(
                (x, y),
                w,
                10.0,
                &[
                    (r + 2.0, 4.0),
                    (-w - r - 2.0, 4.0),
                    (-w / 2.0, -r - 3.0),
                    (-w / 2.0, r + 12.0),
                    (r + 2.0, -r),
                    (-w - r - 2.0, -r),
                ],
            );
            labels.push(halo(lx, ly, 10.0, "bold", "start", "via-label", &text));
        }
        for pad in &layer_pads {
            let b = pad.bounds;
            let c: Point = centroid(&[(px(b[0]), py(b[1])), (px(b[2]), py(b[3]))]);
            let w = text_px(&pad.label, 10.0);
            let (lx, ly) = placer.place(
                c,
                w,
                10.0,
                &[
                    (-w / 2.0, 4.0),
                    (-w / 2.0, -((b[3] - b[1]) * scale / 2.0) - 4.0),
                    (-w / 2.0, ((b[3] - b[1]) * scale / 2.0) + 12.0),
                    (((b[2] - b[0]) * scale / 2.0) + 4.0, 4.0),
                    (-w - ((b[2] - b[0]) * scale / 2.0) - 4.0, 4.0),
                ],
            );
            labels.push(halo(lx, ly, 10.0, "normal", "start", "pad-label", &pad.label));
        }
        // The port: a ring at each terminal that is on this layer.
        for (terminal, name) in [(panel.port.plus, "+"), (panel.port.minus, "-")] {
            if terminal.0 != l {
                continue;
            }
            let (x, y) = (px(terminal.1[0]), py(terminal.1[1]));
            out.push(format!(
                "<g class=\"port\" data-terminal=\"{name}\"><circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"none\" stroke=\"{PORT}\" stroke-width=\"2\"/></g>",
                n(x),
                n(y)
            ));
            placer.reserve([x - 8.0, y - 8.0, x + 8.0, y + 8.0]);
        }
        if panel.port.plus.0 == l || panel.port.minus.0 == l {
            let (a, b) = (panel.port.plus.1, panel.port.minus.1);
            let at = if panel.port.plus.0 == l && panel.port.minus.0 == l {
                [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
            } else if panel.port.plus.0 == l {
                a
            } else {
                b
            };
            let (x, y) = (px(at[0]), py(at[1]));
            let text = "port (the switch)";
            let w = text_px(text, 10.0);
            let (lx, ly) = placer.place(
                (x, y),
                w,
                10.0,
                &[(-w / 2.0, -12.0), (-w / 2.0, 20.0), (9.0, 4.0), (-w - 9.0, 4.0), (9.0, -12.0), (-w - 9.0, -12.0)],
            );
            labels.push(format!(
                "<text class=\"port-label\" x=\"{}\" y=\"{}\" font-size=\"10\" font-weight=\"bold\" fill=\"{PORT}\" stroke=\"#ffffff\" stroke-width=\"3\" stroke-linejoin=\"round\" paint-order=\"stroke\">{}</text>",
                n(lx),
                n(ly),
                text
            ));
        }
        out.extend(labels);
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1\"/>",
            n(ox),
            n(oy),
            n(map_w),
            n(map_h)
        ));
        if slot == shown.len() - 1 {
            let bar = [1.0, 2.0, 5.0, 10.0].iter().copied().rev().find(|b| b * scale <= map_w / 3.0).unwrap_or(1.0);
            out.push(format!(
                "<g class=\"scale\"><line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"3\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{} mm</text></g>",
                n(ox),
                n(oy + map_h + 16.0),
                n(ox + bar * scale),
                n(oy + map_h + 16.0),
                n(ox + bar * scale + 6.0),
                n(oy + map_h + 20.0),
                n(bar)
            ));
        }
        out.push("</g>".to_owned());
        top = oy + map_h + if slot == shown.len() - 1 { 34.0 } else { 22.0 };
        if slot == 0 {
            column_top = oy;
        }
    }

    // Right column: colour bar, legend, loop inductance, the vias.
    let column_start = out.len();
    let (bar_x, bar_y, bar_h) = (side, column_top + 18.0, 180.0);
    out.push("<g class=\"colorbar\">".to_owned());
    out.push(format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">sheet current, dB re 0 dB</text>",
        n(bar_x),
        n(column_top + 4.0)
    ));
    let block = bar_h / STEPS as f64;
    for k in 0..STEPS {
        out.push(format!(
            "<rect x=\"{}\" y=\"{}\" width=\"18\" height=\"{}\" fill=\"{}\"/>",
            n(bar_x),
            n(bar_y + bar_h - (k as f64 + 1.0) * block),
            n(block + 0.3),
            scale_color((k as f64 + 0.5) / STEPS as f64)
        ));
    }
    let ticks = (RANGE_DB / 10.0).round() as usize;
    for k in 0..=ticks {
        let level = -RANGE_DB + 10.0 * k as f64;
        let yy = bar_y + bar_h - bar_h * k as f64 / ticks as f64;
        out.push(format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{} dB</text>",
            n(bar_x + 18.0),
            n(yy),
            n(bar_x + 23.0),
            n(yy),
            n(bar_x + 27.0),
            n(yy + 4.0),
            n(level)
        ));
    }
    let mut cy = bar_y + bar_h + 18.0;
    let note = format!(
        "0 dB = {} A/mm per ampere into the port: the level the strongest 1 % of the cells of the layer with the largest peak exceed; cells above it, up to {} A/mm ({} dB), are drawn in the top colour. Copper more than {} dB below 0 dB takes the bottom colour; copper without a filament is white.{}",
        n(reference),
        n(raw_peak),
        n(rel_db(raw_peak)),
        n(RANGE_DB),
        if hidden.is_empty() {
            String::new()
        } else {
            format!(" No map for {}: more than {} dB below 0 dB.", hidden.join(", "), n(LAYER_DB))
        }
    );
    for line in wrap_px(&note, 10.0, column) {
        out.push(format!(
            "<text class=\"scale-note\" x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"{MUTED}\">{}</text>",
            n(bar_x),
            n(cy),
            escape(&line)
        ));
        cy += 12.0;
    }
    out.push("</g>".to_owned());
    // Legend.
    cy += 10.0;
    out.push("<g class=\"legend\">".to_owned());
    let legend: Vec<(String, &str)> = vec![
        (
            "<path d=\"M0 0L20 0M13 -4L20 0L13 4\" fill=\"none\" stroke=\"#111827\" stroke-opacity=\"0.75\" stroke-width=\"2.8\" stroke-linecap=\"round\"/><path d=\"M0 0L20 0M13 -4L20 0L13 4\" fill=\"none\" stroke=\"#ffffff\" stroke-width=\"1.2\" stroke-linecap=\"round\"/>".to_owned(),
            "current direction at the port current's peak",
        ),
        (
            format!(
                "<rect x=\"0\" y=\"-5\" width=\"22\" height=\"10\" fill=\"#ffffff\" fill-opacity=\"0.18\" stroke=\"{INK}\" stroke-width=\"1\"/>"
            ),
            "pad of the loop",
        ),
        (
            format!("<circle cx=\"11\" cy=\"0\" r=\"6\" fill=\"none\" stroke=\"{PORT}\" stroke-width=\"2\"/>"),
            "the port, across the switch",
        ),
    ];
    let via_entries: Vec<(String, String)> = nets
        .iter()
        .map(|net| {
            (
                format!(
                    "<circle cx=\"11\" cy=\"0\" r=\"6\" fill=\"#ffffff\" fill-opacity=\"0.8\" stroke=\"{}\" stroke-width=\"1.6\"/>",
                    net_colour(net)
                ),
                format!("via of {net}: area is its share of the layer change"),
            )
        })
        .collect();
    let legend: Vec<(String, String)> = legend.into_iter().map(|(g, l)| (g, l.to_owned())).chain(via_entries).collect();
    for (glyph, label) in &legend {
        out.push(format!("<g transform=\"translate({} {})\">{glyph}</g>", n(bar_x), n(cy)));
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{label}</text>",
            n(bar_x + 30.0),
            n(cy + 4.0)
        ));
        cy += 18.0;
    }
    out.push("</g>".to_owned());
    // Loop inductance at the solved frequencies.
    cy += 14.0;
    let heading = |out: &mut Vec<String>, cy: &mut f64, text: &str| {
        out.push(format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"12\" font-weight=\"bold\" fill=\"{INK}\">{}</text>",
            n(bar_x),
            n(*cy),
            escape(text)
        ));
        *cy += 17.0;
    };
    heading(&mut out, &mut cy, "loop impedance across the switch");
    for (hz, l_nh, r_mohm) in &panel.impedance {
        out.push(format!(
            "<text class=\"inductance-row\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
            n(bar_x),
            n(cy),
            escape(&format!("{}: {} nH, {} mohm", frequency_label(*hz), n(*l_nh), n(*r_mohm)))
        ));
        cy += 14.0;
    }
    cy += 12.0;
    heading(&mut out, &mut cy, "division between the vias");
    if panel.transitions.is_empty() {
        for line in wrap_px("none: the loop's current does not change layer through a via", 11.0, column) {
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                n(bar_x),
                n(cy),
                escape(&line)
            ));
            cy += 14.0;
        }
    }
    let near_pad = |via: &ViaMark| -> Option<&str> {
        panel
            .pads
            .iter()
            .map(|p| {
                let dx = (p.bounds[0] - via.position[0]).max(via.position[0] - p.bounds[2]).max(0.0);
                let dy = (p.bounds[1] - via.position[1]).max(via.position[1] - p.bounds[3]).max(0.0);
                (dx.hypot(dy), p.label.as_str())
            })
            .filter(|(d, _)| *d <= NEAR_PAD_MM)
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)))
            .map(|(_, label)| label)
    };
    for t in panel.transitions.iter().filter(|t| is_listed(t)) {
        let head = format!(
            "{}, {} to {}: {} A up, {} A down per A into the port",
            t.net,
            t.lower,
            t.upper,
            n(t.up),
            n(t.down)
        );
        let mut entries: Vec<String> = Vec::new();
        for &(via, share) in t.top.iter().filter(|(_, s)| *s >= LABEL_SHARE) {
            let number = numbers.get(&via).copied().unwrap_or(0);
            let near = near_pad(&panel.vias[via]).map_or(String::new(), |p| format!(" near {p}"));
            entries.push(format!("{number}: {} %{near}", n((share * 100.0).round())));
        }
        let rest = t.top.iter().filter(|(_, s)| *s < LABEL_SHARE).count();
        let mut row = format!("{head}. {}", entries.join("; "));
        if rest > 0 {
            row.push_str(&format!("; {} under {} %", counted(rest, "more via", "more vias"), n(LABEL_SHARE * 100.0)));
        }
        row.push('.');
        for (k, line) in wrap_px(&row, 11.0, column - 12.0).iter().enumerate() {
            out.push(format!(
                "<text class=\"via-row\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{}</text>",
                n(bar_x + if k == 0 { 0.0 } else { 12.0 }),
                n(cy),
                escape(line)
            ));
            cy += 14.0;
        }
        cy += 3.0;
    }
    let weaker = panel.transitions.iter().filter(|t| !is_listed(t)).count();
    if weaker > 0 {
        let text = format!(
            "{} {} under {} A each.",
            counted(weaker, "weaker layer change", "weaker layer changes"),
            if weaker == 1 { "carries" } else { "carry" },
            n(LISTED_AMPERE)
        );
        for line in wrap_px(&text, 11.0, column) {
            out.push(format!(
                "<text class=\"via-row\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                n(bar_x),
                n(cy),
                escape(&line)
            ));
            cy += 14.0;
        }
    }
    cy += 10.0;
    for line in wrap_px(
        "Share = |I| over the sum of |I| of the vias of that net between the panel's layer and the adjacent layer that carries the net's current; vias on a layer change under 0.1 A are small and unlabelled. Up and down are equal where the port's two terminals are on one layer; more than 1 A means current that circulates between the layers (induced by the loop). Current shares of the other capacitors are not part of this solution: the hot capacitor closes the loop as copper and the others are open ports (see --plot-impedance).",
        10.0,
        column,
    ) {
        out.push(format!(
            "<text class=\"column-note\" x=\"{}\" y=\"{}\" font-size=\"10\" fill=\"{MUTED}\">{}</text>",
            n(bar_x),
            n(cy),
            escape(&line)
        ));
        cy += 12.0;
    }
    if compact {
        // The thumbnail drops the column: a one-row colour scale under the map, a caption and the legend in rows.
        out.truncate(column_start);
        let map_bottom = top - 34.0;
        let (bar_w, bar_h) = (200.0, 10.0);
        let bx = (LEFT + map_w - bar_w).max(LEFT + 150.0);
        let by = map_bottom + 6.0;
        out.push("<g class=\"colorbar\">".to_owned());
        for k in 0..STEPS {
            out.push(format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                n(bx + bar_w * k as f64 / STEPS as f64),
                n(by),
                n(bar_w / STEPS as f64 + 0.3),
                n(bar_h),
                scale_color((k as f64 + 0.5) / STEPS as f64)
            ));
        }
        let ticks = (RANGE_DB / 10.0).round() as usize;
        for k in 0..=ticks {
            let x = bx + bar_w * k as f64 / ticks as f64;
            out.push(format!(
                "<line x1=\"{0}\" y1=\"{1}\" x2=\"{0}\" y2=\"{2}\" stroke=\"{INK}\" stroke-width=\"1\"/><text x=\"{0}\" y=\"{3}\" font-size=\"10\" text-anchor=\"middle\" fill=\"{INK}\">{4} dB</text>",
                n(x),
                n(by + bar_h),
                n(by + bar_h + 4.0),
                n(by + bar_h + 15.0),
                n(-RANGE_DB + 10.0 * k as f64)
            ));
        }
        out.push("</g>".to_owned());
        let mut ty = by + bar_h + 34.0;
        let caption = format!(
            "Sheet current per mesh cell for one ampere into the port, dB below the level the strongest 1 % of the cells exceed ({} A/mm); quasi-static extraction, {} filaments, status {}.",
            n(reference),
            panel.filaments,
            panel.status
        );
        for line in wrap_px(&caption, 11.0, width - 2.0 * LEFT) {
            out.push(format!(
                "<text class=\"scale-note\" x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                n(LEFT),
                n(ty),
                escape(&line)
            ));
            ty += 14.0;
        }
        ty += 6.0;
        let mut lx = LEFT;
        out.push("<g class=\"legend\">".to_owned());
        for (glyph, label) in &legend {
            let need = 30.0 + text_px(label, 11.0) + 16.0;
            if lx + need > width - LEFT && lx > LEFT {
                lx = LEFT;
                ty += 17.0;
            }
            out.push(format!("<g transform=\"translate({} {})\">{glyph}</g>", n(lx), n(ty)));
            out.push(format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\">{label}</text>",
                n(lx + 30.0),
                n(ty + 4.0)
            ));
            lx += need;
        }
        out.push("</g>".to_owned());
        let mut content = vec![format!("<defs>{}</defs>", defs.join(""))];
        content.extend(out);
        return FieldPicture { content: content.join("\n"), width, height: ty + 14.0 };
    }
    let height = top.max(cy) + 10.0;
    let mut content = vec![format!("<defs>{}</defs>", defs.join(""))];
    content.extend(out);
    FieldPicture { content: content.join("\n"), width, height }
}

/// The SVG of `panels`, stacked; `title` names the board in the document title.
pub fn current_svg(title: &str, pictures: &[FieldPicture]) -> String {
    stacked_svg(
        &format!("{} current distribution of the hot loop (screening estimate)", escape(title)),
        pictures,
        560.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> CurrentPanel {
        // A 10 x 4 mm window, a trace on the top layer carrying 1 A to the right and a plane below returning it.
        let top_bars: Vec<Bar> = (0..20)
            .map(|k| Bar {
                along_x: true,
                centre: [0.25 + 0.5 * f64::from(k), 2.0],
                length: 0.5,
                width: 0.5,
                re: 1.0,
                im: 0.0,
            })
            .collect();
        let plane_bars: Vec<Bar> = (0..20)
            .flat_map(|k| {
                (0..8).map(move |j| Bar {
                    along_x: true,
                    centre: [0.25 + 0.5 * f64::from(k), 0.25 + 0.5 * f64::from(j)],
                    length: 0.5,
                    width: 0.5,
                    re: -0.125,
                    im: 0.01,
                })
            })
            .collect();
        let square = |x0: f64, y0: f64, x1: f64, y1: f64| vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]];
        CurrentPanel {
            cell: "U1".to_owned(),
            frequency_hz: 10e6,
            window: [0.0, 0.0, 10.0, 4.0],
            layers: vec![
                LayerPanel {
                    name: "F.Cu".to_owned(),
                    z_mm: 0.0,
                    outlines: vec![square(0.0, 1.75, 10.0, 2.25)],
                    bars: top_bars,
                },
                LayerPanel {
                    name: "In1.Cu".to_owned(),
                    z_mm: -0.2,
                    outlines: vec![square(0.0, 0.0, 10.0, 4.0)],
                    bars: plane_bars,
                },
                LayerPanel { name: "B.Cu".to_owned(), z_mm: -1.6, outlines: Vec::new(), bars: Vec::new() },
            ],
            vias: vec![
                ViaMark {
                    name: "v1".to_owned(),
                    net: "GND".to_owned(),
                    position: [1.5, 2.0],
                    diameter: 0.35,
                    layers: vec![
                        ViaLayer { layer: 0, share: 1.0, throughput: 1.0 },
                        ViaLayer { layer: 1, share: 1.0, throughput: 1.0 },
                    ],
                },
                ViaMark {
                    name: "v2".to_owned(),
                    net: "GND".to_owned(),
                    position: [5.0, 3.0],
                    diameter: 0.35,
                    layers: vec![ViaLayer { layer: 1, share: 1.0, throughput: 0.05 }],
                },
            ],
            pads: vec![PadMark { label: "C1.1".to_owned(), layer: 0, bounds: [0.0, 1.5, 0.8, 2.5] }],
            port: PortMark { plus: (0, [9.5, 2.0]), minus: (0, [9.5, 2.0]) },
            transitions: vec![
                TransitionRow {
                    net: "GND".to_owned(),
                    lower: "In1.Cu".to_owned(),
                    upper: "F.Cu".to_owned(),
                    count: 1,
                    up: 1.0,
                    down: 1.0,
                    top: vec![(0, 1.0)],
                },
                TransitionRow {
                    net: "GND".to_owned(),
                    lower: "B.Cu".to_owned(),
                    upper: "In1.Cu".to_owned(),
                    count: 1,
                    up: 0.05,
                    down: 0.05,
                    top: vec![(1, 1.0)],
                },
            ],
            impedance: vec![(1e6, 4.2, 10.0), (10e6, 4.0, 12.0), (100e6, 3.9, 13.0)],
            status: "ok".to_owned(),
            status_reasons: Vec::new(),
            filaments: 480,
        }
    }

    #[test]
    fn the_map_has_a_panel_per_layer_with_current_and_says_what_it_is() {
        let svg = current_svg("board", &[current_picture(&panel())]);
        assert_eq!(svg, current_svg("board", &[current_picture(&panel())]), "the render is repeatable");
        assert!(svg.starts_with("<svg") && svg.contains("fill=\"#ffffff\""));
        assert_eq!(svg.matches("class=\"layer\"").count(), 2, "the empty bottom layer has no map");
        assert!(!svg.contains("data-layer=\"B.Cu\"") && svg.contains("No map for"));
        assert!(svg.contains("quasi-static extraction at 10 MHz") && svg.contains("Screening estimate"));
        assert!(svg.contains("data-via=\"v1\"") && svg.contains("data-pad=\"C1.1\"") && svg.contains("class=\"port\""));
        // The layer change of 1 A is listed in full with its via numbered and its nearest pad; the 0.05 A one only counted.
        assert!(svg.contains("GND, In1.Cu to F.Cu: 1 A up"));
        assert!(svg.contains(">1: 100 %</text>") && svg.contains("class=\"via-label\""));
        assert!(svg.contains("1 weaker layer change"), "a layer change under 0.1 A is summed, not listed");
        assert_eq!(svg.matches("class=\"via-label\"").count(), 2, "v1 is labelled on both layers, v2 never");
        assert!(svg.contains("data-via=\"v2\" data-net=\"GND\" data-share=\"0\""));
        assert!(!svg.contains("(s)") && !svg.contains("(es)"));
        assert!(svg.contains("10 MHz: 4 nH, 12 mohm"));
        assert!(!svg.contains("NaN"));
    }

    #[test]
    fn the_raster_sheet_current_is_the_current_over_the_width_and_the_direction_is_the_real_part() {
        let p = panel();
        let r = rasterise(&p.layers[0], p.window, 0.1);
        // The trace is 0.5 mm wide and carries 1 A: 2 A/mm, to the right.
        let at = (20 * r.nx) + 50; // y = 2.05 mm, x = 5.05 mm
        assert!((r.magnitude[at] - 2.0).abs() < 1e-9, "{}", r.magnitude[at]);
        assert!(r.re_x[at] > 0.0 && r.re_y[at] == 0.0);
        // Off the trace: nothing.
        assert!(r.magnitude[5 * r.nx + 50].is_nan());
        // The plane's return current runs the other way.
        let plane = rasterise(&p.layers[1], p.window, 0.1);
        assert!(plane.re_x[(20 * plane.nx) + 50] < 0.0);
        assert!(peak(&plane) < peak(&r));
    }

    #[test]
    fn colour_steps_are_one_db_wide_and_the_maximum_is_in_the_last() {
        assert_eq!(bin(0.0), Some(STEPS - 1));
        assert_eq!(bin(-0.5), Some(STEPS - 1));
        assert_eq!(bin(-1.5), Some(STEPS - 2));
        assert_eq!(bin(-RANGE_DB), Some(0));
        assert_eq!(bin(-RANGE_DB - 0.01), None);
        assert_eq!(bin(f64::NEG_INFINITY), None);
    }
}

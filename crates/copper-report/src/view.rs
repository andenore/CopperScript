//! A window of a board for review, one panel per copper layer (`pcbir.view`).
//!
//! `copper view` draws pads, tracks, vias and markers in a millimetre window,
//! highlights chosen nets and can align a manufacturer reference layout on a
//! package's pads. Alongside the picture it returns a short text legend of
//! what is in the window, so a text-only reader still gets the facts.
//!
//! The native renderer writes SVG, including saved fills and aligned reference
//! copper. A view is a reading aid: it neither refills zones nor checks clearances.

use copper_json::{Value, python_float_repr};

use crate::py::{self, Py, PyErr};
use crate::scene::{Point, Scene, fixed};

pub const PALETTE: [&str; 8] = ["#d1495b", "#2e86ab", "#3c9d5d", "#e08e0b", "#8e5bb5", "#0f8b8d", "#c05299", "#6d7f1e"];
const OTHER: &str = "#b9c0c8";
const GROUND: &str = "#7f8c99";
const MARKER: &str = "#e00000";
pub const SVG_NOTE: &str = "SVG view: saved fills are translucent; unfilled zones are dashed outlines. No refill or clearance check; rule-area keepouts are not shown.";

pub type Window = [f64; 4];

pub struct ViewRequest {
    pub window: Window,
    pub layers: Vec<String>,
    pub highlight: Vec<Value>,
    pub markers: Vec<Point>,
}

pub struct Alignment {
    pub reference: String,
    pub target: String,
    pub angle_degrees: f64,
    pub mirrored: bool,
    pub rms_mm: f64,
    pub matched_pads: usize,
    // Keep the unrounded fit for rendering; public rounded values are for the legend.
    angle_radians: f64,
    offset: Point,
}

impl Alignment {
    fn point(&self, p: Point) -> Point {
        let x = if self.mirrored { -p.0 } else { p.0 };
        let (s, c) = self.angle_radians.sin_cos();
        (c * x - s * p.1 + self.offset.0, s * x + c * p.1 + self.offset.1)
    }

    fn scene(&self, reference: &Scene) -> Scene {
        let mut scene = reference.clone();
        for pad in &mut scene.pads {
            pad.center = self.point(pad.center);
            pad.polygon.iter_mut().for_each(|p| *p = self.point(*p));
        }
        for track in &mut scene.tracks {
            track.start = self.point(track.start);
            track.end = self.point(track.end);
        }
        for via in &mut scene.vias {
            via.center = self.point(via.center);
        }
        for area in &mut scene.areas {
            area.outer.iter_mut().for_each(|p| *p = self.point(*p));
            area.holes.iter_mut().flatten().for_each(|p| *p = self.point(*p));
        }
        scene
    }
}

/// Python's `sum()` of floats: Neumaier's compensated summation.
pub fn float_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut values = values.into_iter();
    let Some(first) = values.next() else { return 0.0 };
    let (mut total, mut compensation) = (0.0 + first, 0.0f64);
    for x in values {
        let t = total + x;
        if total.abs() >= x.abs() {
            compensation += (total - t) + x;
        } else {
            compensation += (x - t) + total;
        }
        total = t;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

/// Python's `round(value, digits)`.
pub fn round_to(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.digits$}").parse().unwrap_or(value)
}

fn py_min(values: &[f64]) -> Py<f64> {
    let first = *values.first().ok_or_else(|| PyErr::value_error("min() iterable argument is empty"))?;
    Ok(values[1..].iter().fold(first, |best, value| if *value < best { *value } else { best }))
}

fn py_max(values: &[f64]) -> Py<f64> {
    let first = *values.first().ok_or_else(|| PyErr::value_error("max() iterable argument is empty"))?;
    Ok(values[1..].iter().fold(first, |best, value| if *value > best { *value } else { best }))
}

// --- choosing what to show ---------------------------------------------------------------

/// `window_around(points, margin)`: the points' frame, at least 4 mm a side.
pub fn window_around(points: &[Point], margin: f64) -> Py<Window> {
    let minimum = 4.0;
    let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
    let (mut x0, mut x1, mut y0, mut y1) =
        (py_min(&xs)? - margin, py_max(&xs)? + margin, py_min(&ys)? - margin, py_max(&ys)? + margin);
    for (lo, hi) in [(&mut x0, &mut x1), (&mut y0, &mut y1)] {
        if *hi - *lo < minimum {
            let c = (*lo + *hi) / 2.0;
            (*lo, *hi) = (c - minimum / 2.0, c + minimum / 2.0);
        }
    }
    Ok([x0, y0, x1, y1])
}

pub fn inside(point: Point, window: &Window) -> bool {
    window[0] <= point.0 && point.0 <= window[2] && window[1] <= point.1 && point.1 <= window[3]
}

pub fn segment_hits(a: Point, b: Point, window: &Window) -> bool {
    if inside(a, window) || inside(b, window) {
        return true;
    }
    let (max_x, min_x) = (if b.0 > a.0 { b.0 } else { a.0 }, if b.0 < a.0 { b.0 } else { a.0 });
    let (max_y, min_y) = (if b.1 > a.1 { b.1 } else { a.1 }, if b.1 < a.1 { b.1 } else { a.1 });
    !(max_x < window[0] || min_x > window[2] || max_y < window[1] || min_y > window[3])
}

fn poly_hits(points: &[Point], window: &Window) -> bool {
    if points.is_empty() {
        return false;
    }
    let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
    let (min_x, max_x) = (py_min(&xs).expect("points"), py_max(&xs).expect("points"));
    let (min_y, max_y) = (py_min(&ys).expect("points"), py_max(&ys).expect("points"));
    !(max_x < window[0] || min_x > window[2] || max_y < window[1] || min_y > window[3])
}

fn listed(nets: &[Value], net: &str) -> bool {
    nets.iter().any(|value| py::eq_str(value, net))
}

/// `layers_in_window(scene, window, nets)`: copper layers with highlighted
/// (or, failing that, any) copper inside the window.
pub fn layers_in_window(scene: &Scene, window: &Window, nets: &[Value]) -> Py<Vec<String>> {
    for net in nets {
        py::hashable(net)?;
    }
    let used = |only: &[Value]| {
        let mut found: Vec<String> = Vec::new();
        let mut add = |layer: &str| {
            if !found.iter().any(|seen| seen == layer) {
                found.push(layer.to_owned());
            }
        };
        for track in &scene.tracks {
            if (only.is_empty() || listed(only, &track.net)) && segment_hits(track.start, track.end, window) {
                add(&track.layer);
            }
        }
        // Through vias touch every layer, so they do not make a layer interesting.
        for pad in &scene.pads {
            if (only.is_empty() || listed(only, &pad.net)) && inside(pad.center, window) && pad.layers.len() <= 2 {
                for layer in &pad.layers {
                    add(layer);
                }
            }
        }
        for area in &scene.areas {
            if (only.is_empty() || listed(only, &area.net)) && area.kind == "fill" && poly_hits(&area.outer, window) {
                add(&area.layer);
            }
        }
        found
    };
    let mut found = if nets.is_empty() { Vec::new() } else { used(nets) };
    if found.is_empty() {
        found = used(&[]);
    }
    let layers: Vec<String> = scene.copper_layers.iter().filter(|layer| found.contains(layer)).cloned().collect();
    Ok(if layers.is_empty() { scene.copper_layers.iter().take(1).cloned().collect() } else { layers })
}

// --- reference alignment ----------------------------------------------------------------

fn kabsch(source: &[Point], target: &[Point]) -> (f64, Point, f64) {
    let n = source.len() as f64;
    let cs = (float_sum(source.iter().map(|p| p.0)) / n, float_sum(source.iter().map(|p| p.1)) / n);
    let cd = (float_sum(target.iter().map(|p| p.0)) / n, float_sum(target.iter().map(|p| p.1)) / n);
    let (mut sxx, mut sxy, mut syx, mut syy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (p, q) in source.iter().zip(target) {
        let (px, py, qx, qy) = (p.0 - cs.0, p.1 - cs.1, q.0 - cd.0, q.1 - cd.1);
        sxx += px * qx;
        sxy += px * qy;
        syx += py * qx;
        syy += py * qy;
    }
    let angle = (sxy - syx).atan2(sxx + syy);
    let (c, s) = (angle.cos(), angle.sin());
    let offset = (cd.0 - (c * cs.0 - s * cs.1), cd.1 - (s * cs.0 + c * cs.1));
    let error =
        float_sum(source.iter().zip(target).map(|(p, q)| {
            (c * p.0 - s * p.1 + offset.0 - q.0).powf(2.0) + (s * p.0 + c * p.1 + offset.1 - q.1).powf(2.0)
        }));
    (angle, offset, (error / n).sqrt())
}

/// `align_reference(reference, ref_component, scene, component)`'s fit: pads
/// matched by number, mirroring tried as well, the better least-squares fit
/// wins. Preserve the complete fit for the SVG overlay.
pub fn align_reference(reference: &Scene, ref_component: &str, scene: &Scene, component: &str) -> Py<Alignment> {
    let centers = |scene: &Scene, reference: &str| {
        let mut table: Vec<(String, Point)> = Vec::new();
        for pad in scene.pads.iter().filter(|pad| pad.reference == reference) {
            match table.iter_mut().find(|(number, _)| *number == pad.pad) {
                Some((_, center)) => *center = pad.center,
                None => table.push((pad.pad.clone(), pad.center)),
            }
        }
        table
    };
    let (ours, theirs) = (centers(scene, component), centers(reference, ref_component));
    let lookup = |table: &[(String, Point)], number: &str| table.iter().find(|(n, _)| n == number).map(|(_, c)| *c);
    let mut common: Vec<String> =
        ours.iter().filter(|(number, _)| lookup(&theirs, number).is_some()).map(|(number, _)| number.clone()).collect();
    common.sort();
    if common.len() < 2 {
        return Err(PyErr::value_error(format!(
            "need at least two pads shared by {component} and reference {ref_component}; found {}",
            common.len()
        )));
    }
    let mut best: Option<(bool, f64, Point, f64)> = None;
    for mirrored in [false, true] {
        let source: Vec<Point> = common
            .iter()
            .map(|n| {
                let p = lookup(&theirs, n).expect("common");
                (if mirrored { -p.0 } else { p.0 }, p.1)
            })
            .collect();
        let target: Vec<Point> = common.iter().map(|n| lookup(&ours, n).expect("common")).collect();
        let (angle, offset, rms) = kabsch(&source, &target);
        if best.is_none_or(|(_, _, _, least)| rms < least - 1e-9) {
            best = Some((mirrored, angle, offset, rms));
        }
    }
    let (mirrored, angle, offset, rms) = best.expect("two candidates");
    Ok(Alignment {
        reference: ref_component.to_owned(),
        target: component.to_owned(),
        angle_degrees: round_to(copper_physical::pyfloat::float_mod(angle.to_degrees(), 360.0), 3),
        mirrored,
        rms_mm: round_to(rms, 4),
        matched_pads: common.len(),
        angle_radians: angle,
        offset,
    })
}

// --- legend -------------------------------------------------------------------------------

/// `net_colors(nets)`: palette colours in order.
pub fn net_colors(nets: &[Value]) -> Py<Vec<(Value, &'static str)>> {
    let mut colors: Vec<(Value, &'static str)> = Vec::new();
    for (index, net) in nets.iter().enumerate() {
        py::hashable(net)?;
        let color = PALETTE[index % PALETTE.len()];
        match colors.iter_mut().find(|(seen, _)| py::eq(seen, net)) {
            Some((_, existing)) => *existing = color,
            None => colors.push((net.clone(), color)),
        }
    }
    Ok(colors)
}

fn color(net: &str, colors: &[(Value, &'static str)]) -> &'static str {
    if let Some((_, color)) = colors.iter().find(|(value, _)| py::eq_str(value, net)) {
        return color;
    }
    if ["GND", "AGND", "DGND", "VSS"].contains(&net.to_uppercase().as_str()) { GROUND } else { OTHER }
}

pub fn legend(
    scene: &Scene,
    request: &ViewRequest,
    colors: &[(Value, &'static str)],
    alignment: Option<&Alignment>,
) -> Vec<String> {
    let limit = 14;
    let [x0, y0, x1, y1] = request.window;
    let mut lines = vec![format!(
        "window x {}..{} mm, y {}..{} mm (y down); layers {}",
        fixed(x0, 3),
        fixed(x1, 3),
        fixed(y0, 3),
        fixed(y1, 3),
        request.layers.join(", ")
    )];
    if scene.source == crate::issues::FIXED_PREFIX_VIEW_SOURCE {
        lines.push(
            "declared fixed-prefix copper is shown; viewing does not certify its admission or connectivity".to_owned(),
        );
    }
    if !colors.is_empty() {
        let shown: Vec<String> = request
            .highlight
            .iter()
            .map(|net| {
                let color = colors.iter().find(|(value, _)| py::eq(value, net)).map_or("", |(_, color)| *color);
                format!("{} {color}", py::text(net))
            })
            .collect();
        lines.push(format!("highlight: {}", shown.join(", ")));
    }
    let window = &request.window;
    let pads: Vec<_> = scene.pads.iter().filter(|p| inside(p.center, window)).collect();
    let vias: Vec<_> = scene.vias.iter().filter(|v| inside(v.center, window)).collect();
    let tracks: Vec<_> = scene
        .tracks
        .iter()
        .filter(|t| request.layers.contains(&t.layer) && segment_hits(t.start, t.end, window))
        .collect();
    lines.push(format!("in window: {} pads, {} vias, {} track segments", pads.len(), vias.len(), tracks.len()));
    let areas: Vec<_> =
        scene.areas.iter().filter(|a| request.layers.contains(&a.layer) && poly_hits(&a.outer, window)).collect();
    let fills = areas.iter().filter(|a| a.kind == "fill").count();
    lines.push(format!("in window: {fills} saved fill polygons, {} unfilled zone outlines", areas.len() - fills));
    let mut area_counts = indexmap::IndexMap::new();
    for area in &areas {
        *area_counts.entry((area.net.as_str(), area.layer.as_str(), area.kind)).or_insert(0usize) += 1;
    }
    let mut area_rows: Vec<_> = area_counts.into_iter().collect();
    area_rows.sort_by_key(|((net, _, _), _)| !listed(&request.highlight, net));
    for ((net, layer, kind), count) in area_rows.iter().take(limit) {
        let net = if net.is_empty() { "-" } else { net };
        lines.push(format!("  area {net} {layer}: {count} {kind} polygon(s) intersect window; contact not inferred"));
    }
    if area_rows.len() > limit {
        lines.push(format!("  ... {} more visible net/layer/area groups", area_rows.len() - limit));
    }
    let focus: Vec<_> = if request.highlight.is_empty() {
        pads.clone()
    } else {
        pads.iter().copied().filter(|p| listed(&request.highlight, &p.net)).collect()
    };
    for pad in focus.iter().take(limit) {
        let layers = if pad.layers.len() <= 2 { pad.layers.join("/") } else { "all layers".to_owned() };
        let net = if pad.net.is_empty() { "-" } else { pad.net.as_str() };
        lines.push(format!(
            "  pad {}.{} {net} @({},{}) {layers}",
            pad.reference,
            pad.pad,
            fixed(pad.center.0, 3),
            fixed(pad.center.1, 3)
        ));
    }
    if focus.len() > limit {
        lines.push(format!("  ... {} more pads", focus.len() - limit));
    }
    // A highlighted net's missing site may be occupied by another net. Keep
    // visible foreign via context, with highlighted vias first, instead of
    // making a text reader infer the blocker from an unlabelled circle.
    let mut via_context: Vec<_> =
        vias.iter().copied().filter(|via| via.layers.iter().any(|layer| request.layers.contains(layer))).collect();
    via_context.sort_by_key(|via| !listed(&request.highlight, &via.net));
    for via in via_context.iter().take(limit) {
        let net = if via.net.is_empty() { "-" } else { via.net.as_str() };
        lines.push(format!(
            "  via {net} @({},{}) diameter {} mm drill {} mm {}",
            fixed(via.center.0, 3),
            fixed(via.center.1, 3),
            fixed(via.size, 3),
            fixed(via.drill, 3),
            via.layers.join("/")
        ));
    }
    if via_context.len() > limit {
        lines.push(format!("  ... {} more visible vias", via_context.len() - limit));
    }
    for net in &request.highlight {
        let net_vias = vias.iter().filter(|v| py::eq_str(net, &v.net)).count();
        let mut per_layer: Vec<(&str, usize)> = Vec::new();
        for track in tracks.iter().filter(|t| py::eq_str(net, &t.net)) {
            match per_layer.iter_mut().find(|(layer, _)| *layer == track.layer) {
                Some((_, count)) => *count += 1,
                None => per_layer.push((&track.layer, 1)),
            }
        }
        if net_vias > 0 || !per_layer.is_empty() {
            let counts: Vec<String> = per_layer.iter().map(|(layer, count)| format!("{layer} {count}")).collect();
            let counts = if counts.is_empty() { "none".to_owned() } else { counts.join(", ") };
            lines.push(format!("  {}: {net_vias} via(s); tracks {counts}", py::text(net)));
        }
    }
    if !request.markers.is_empty() {
        let shown: Vec<String> =
            request.markers.iter().take(6).map(|(x, y)| format!("({},{})", fixed(*x, 3), fixed(*y, 3))).collect();
        let more = if request.markers.len() > 6 { " ..." } else { "" };
        lines.push(format!("markers (red rings): {}: {}{more}", request.markers.len(), shown.join(", ")));
    }
    if let Some(alignment) = alignment {
        lines.push(format!(
            "reference (magenta outlines): {} -> {}, {} pads, rotation {} deg{}, fit RMS {} mm",
            alignment.reference,
            alignment.target,
            alignment.matched_pads,
            python_float_repr(alignment.angle_degrees),
            if alignment.mirrored { ", mirrored" } else { "" },
            python_float_repr(alignment.rms_mm)
        ));
        lines.push(
            "reference layers match by KiCad layer name; stackup roles and side swaps are not inferred from the 2D fit"
                .to_owned(),
        );
    }
    lines
}

// --- rendering ------------------------------------------------------------------------

fn r(value: f64) -> String {
    python_float_repr(value)
}

fn area_path(outer: &[Point], holes: &[Vec<Point>]) -> String {
    std::iter::once(outer)
        .chain(holes.iter().map(Vec::as_slice))
        .filter(|ring| ring.len() >= 3)
        .map(|ring| {
            let points: Vec<_> = ring.iter().map(|p| format!("{} {}", r(p.0), r(p.1))).collect();
            format!("M {} Z", points.join(" L "))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn circle_hits(center: Point, radius: f64, window: &Window) -> bool {
    poly_hits(&[(center.0 - radius, center.1 - radius), (center.0 + radius, center.1 + radius)], window)
}

/// One clipped panel per layer. Reference geometry uses the full fitted transform.
pub fn svg(
    scene: &Scene,
    request: &ViewRequest,
    colors: &[(Value, &'static str)],
    reference: Option<(&Scene, &Alignment)>,
) -> String {
    let [x0, y0, x1, y1] = request.window;
    let window = &request.window;
    let reference = reference.map(|(scene, alignment)| alignment.scene(scene));
    let mut parts: Vec<String> = Vec::new();
    let (panel_width, panel_height, gutter, margin) = (x1 - x0, y1 - y0, 1.5, 0.25);
    for (i, layer) in request.layers.iter().enumerate() {
        let ox = i as f64 * (panel_width + gutter);
        // Each layer owns an SVG viewport, not just translated geometry on a
        // shared canvas. Copper and reference overlays cannot enter a neighbour
        // or its header. Keep a visible frame/gutter even when copper ends flush
        // with the window edge, so adjacent layers never look like one board.
        parts.push(format!(
            "<g class=\"layer-panel\"><rect x=\"{}\" y=\"-1\" width=\"{}\" height=\"{}\" fill=\"white\"/><text x=\"{}\" y=\"-0.3\" font-family=\"sans-serif\" font-size=\"0.6\">{layer}</text><svg class=\"layer-viewport\" x=\"{}\" y=\"0\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" overflow=\"hidden\">",
            r(ox), r(panel_width), r(panel_height + 1.0), r(ox + 0.15),
            r(ox), r(panel_width), r(panel_height), r(x0), r(y0), r(panel_width), r(panel_height)
        ));
        parts.push(format!(
            "<defs><clipPath id=\"panel-{i}\" clipPathUnits=\"userSpaceOnUse\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/></clipPath></defs><g clip-path=\"url(#panel-{i})\">",
            r(x0), r(y0), r(x1 - x0), r(y1 - y0)
        ));
        for area in &scene.areas {
            if &area.layer == layer && poly_hits(&area.outer, window) {
                let paint = if area.kind == "fill" {
                    format!("fill=\"{}\" fill-opacity=\"0.25\"", color(&area.net, colors))
                } else {
                    format!(
                        "fill=\"none\" stroke=\"{}\" stroke-width=\"0.04\" stroke-dasharray=\"0.2 0.15\"",
                        color(&area.net, colors)
                    )
                };
                parts.push(format!(
                    "<path class=\"{}\" d=\"{}\" fill-rule=\"evenodd\" {paint}/>",
                    area.kind,
                    area_path(&area.outer, &area.holes)
                ));
            }
        }
        for track in &scene.tracks {
            let half = track.width / 2.0;
            let expanded = [x0 - half, y0 - half, x1 + half, y1 + half];
            if &track.layer == layer && segment_hits(track.start, track.end, &expanded) {
                parts.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\"/>",
                    r(track.start.0),
                    r(track.start.1),
                    r(track.end.0),
                    r(track.end.1),
                    color(&track.net, colors),
                    r(track.width)
                ));
            }
        }
        for pad in &scene.pads {
            if pad.layers.contains(layer) && poly_hits(&pad.polygon, window) {
                let points: Vec<String> =
                    pad.polygon.iter().map(|(x, y)| format!("{},{}", fixed(*x, 4), fixed(*y, 4))).collect();
                parts.push(format!("<polygon points=\"{}\" fill=\"{}\"/>", points.join(" "), color(&pad.net, colors)));
            }
        }
        for via in &scene.vias {
            if via.layers.contains(layer) && circle_hits(via.center, via.size / 2.0, window) {
                parts.push(format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
                    r(via.center.0),
                    r(via.center.1),
                    r(via.size / 2.0),
                    color(&via.net, colors)
                ));
            }
        }
        // A drill is absence of copper even when a saved fill lies behind it.
        for (center, drill) in scene
            .pads
            .iter()
            .map(|p| (p.center, p.drill))
            .chain(scene.vias.iter().filter(|v| v.layers.contains(layer)).map(|v| (v.center, v.drill)))
        {
            if drill > 0.0 && circle_hits(center, drill / 2.0, window) {
                parts.push(format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"white\"/>",
                    r(center.0),
                    r(center.1),
                    r(drill / 2.0)
                ));
            }
        }
        if let Some(reference) = &reference {
            parts.push("<g class=\"reference\" fill=\"none\" stroke=\"#ff00ff\" stroke-width=\"0.04\">".to_owned());
            for area in &reference.areas {
                if &area.layer == layer && poly_hits(&area.outer, window) {
                    let dash = if area.kind == "fill" { "" } else { " stroke-dasharray=\"0.2 0.15\"" };
                    parts.push(format!("<path d=\"{}\"{dash}/>", area_path(&area.outer, &area.holes)));
                }
            }
            for track in reference.tracks.iter().filter(|t| &t.layer == layer) {
                let half = track.width / 2.0 + 0.02;
                if !segment_hits(track.start, track.end, &[x0 - half, y0 - half, x1 + half, y1 + half]) {
                    continue;
                }
                // A rounded rectangle is the outline of a finite-width round-ended segment.
                let dx = track.end.0 - track.start.0;
                let dy = track.end.1 - track.start.1;
                parts.push(format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" transform=\"translate({},{}) rotate({})\"/>",
                    r(-track.width / 2.0), r(-track.width / 2.0), r(dx.hypot(dy) + track.width), r(track.width), r(track.width / 2.0),
                    r(track.start.0), r(track.start.1), r(dy.atan2(dx).to_degrees())));
            }
            for pad in &reference.pads {
                if pad.layers.contains(layer) && poly_hits(&pad.polygon, window) {
                    parts.push(format!("<path d=\"{}\"/>", area_path(&pad.polygon, &[])));
                }
            }
            for via in &reference.vias {
                if via.layers.contains(layer) && circle_hits(via.center, via.size / 2.0, window) {
                    for radius in [via.size / 2.0, via.drill / 2.0] {
                        parts.push(format!(
                            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>",
                            r(via.center.0),
                            r(via.center.1),
                            r(radius)
                        ));
                    }
                }
            }
            parts.push("</g>".to_owned());
        }
        for (x, y) in &request.markers {
            parts.push(format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"0.35\" fill=\"none\" stroke=\"{MARKER}\" stroke-width=\"0.08\"/>",
                r(*x),
                r(*y)
            ));
        }
        parts.push(format!(
            "</g></svg><rect class=\"layer-frame\" x=\"{}\" y=\"-1\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#8b96a5\" stroke-width=\"0.04\"/><line x1=\"{}\" y1=\"0\" x2=\"{}\" y2=\"0\" stroke=\"#8b96a5\" stroke-width=\"0.04\"/></g>",
            r(ox), r(panel_width), r(panel_height + 1.0), r(ox), r(ox + panel_width)
        ));
    }
    let width = request.layers.len() as f64 * panel_width
        + request.layers.len().saturating_sub(1) as f64 * gutter
        + 2.0 * margin;
    let height = panel_height + 1.0 + 2.0 * margin;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#e9edf2\"/>{}</svg>",
        r(-margin),
        r(-1.0 - margin),
        r(width),
        r(height),
        r(-margin),
        r(-1.0 - margin),
        r(width),
        r(height),
        parts.concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneArea, ScenePad, SceneTrack, SceneVia};

    fn pad(number: &str, center: Point) -> ScenePad {
        ScenePad {
            reference: "U1".into(),
            pad: number.into(),
            net: "SIG".into(),
            layers: vec!["F.Cu".into()],
            polygon: vec![
                (center.0 - 0.2, center.1 - 0.2),
                (center.0 + 0.2, center.1 - 0.2),
                (center.0 + 0.2, center.1 + 0.2),
                (center.0 - 0.2, center.1 + 0.2),
            ],
            center,
            drill: 0.0,
        }
    }

    fn close(actual: Point, expected: Point) {
        assert!((actual.0 - expected.0).abs() < 1e-9, "{actual:?} != {expected:?}");
        assert!((actual.1 - expected.1).abs() < 1e-9, "{actual:?} != {expected:?}");
    }

    #[test]
    fn area_legend_distinguishes_fill_from_outline_and_prioritizes_highlight() {
        let area = |net: &str, layer: &str, kind| SceneArea {
            net: net.into(),
            layer: layer.into(),
            outer: vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0)],
            holes: vec![],
            kind,
        };
        let mut scene = Scene {
            areas: (0..16).map(|i| area(&format!("OTHER_{i}"), "In1.Cu", "fill")).collect(),
            ..Scene::default()
        };
        scene.areas.extend([
            area("SUPPLY", "In1.Cu", "zone"),
            area("SUPPLY", "In1.Cu", "fill"),
            area("SUPPLY", "In1.Cu", "fill"),
            area("HIDDEN_LAYER", "B.Cu", "fill"),
        ]);
        let request = ViewRequest {
            window: [1.0, 1.0, 4.0, 4.0],
            layers: vec!["In1.Cu".into()],
            highlight: vec![Value::from("SUPPLY")],
            markers: vec![],
        };
        let lines = legend(&scene, &request, &net_colors(&request.highlight).unwrap(), None);
        assert!(lines.iter().any(|line| line.contains("18 saved fill polygons, 1 unfilled zone outlines")));
        let areas: Vec<_> = lines.iter().filter(|line| line.starts_with("  area ")).collect();
        assert_eq!(areas.len(), 14);
        assert!(areas[0].contains("SUPPLY In1.Cu: 1 zone polygon(s)"));
        assert!(areas[1].contains("SUPPLY In1.Cu: 2 fill polygon(s)"));
        assert!(areas.iter().all(|line| line.contains("contact not inferred")));
        assert!(!lines.iter().any(|line| line.contains("HIDDEN_LAYER")));
        assert!(lines.iter().any(|line| line.contains("4 more visible net/layer/area groups")));
    }

    #[test]
    fn reference_fit_transforms_all_copper_without_rounding_or_mutating_source() {
        let reference = Scene {
            pads: vec![pad("1", (2.0, 3.0)), pad("2", (5.0, 3.0)), pad("3", (2.0, 5.0))],
            tracks: vec![SceneTrack {
                net: "SIG".into(),
                layer: "F.Cu".into(),
                start: (5.0, 3.0),
                end: (8.0, 4.0),
                width: 0.2,
            }],
            vias: vec![SceneVia {
                net: "SIG".into(),
                center: (8.0, 4.0),
                size: 0.6,
                drill: 0.3,
                layers: vec!["F.Cu".into(), "B.Cu".into()],
            }],
            areas: vec![SceneArea {
                net: "GND".into(),
                layer: "B.Cu".into(),
                outer: vec![(0.0, 0.0), (9.0, 0.0), (0.0, 9.0)],
                holes: vec![vec![(1.0, 1.0), (2.0, 1.0), (1.0, 2.0)]],
                kind: "fill",
            }],
            ..Scene::default()
        };
        let before = reference.clone();
        for mirrored in [false, true] {
            // Non-integral rotation/translation catches using rounded legend values to draw.
            let expected = Alignment {
                reference: "U1".into(),
                target: "U1".into(),
                angle_degrees: 37.123456,
                mirrored,
                rms_mm: 0.0,
                matched_pads: 3,
                angle_radians: 37.123456f64.to_radians(),
                offset: (100.123456, 40.654321),
            };
            let target = expected.scene(&reference);
            let fit = align_reference(&reference, "U1", &target, "U1").unwrap();
            assert_eq!(fit.mirrored, mirrored);
            let aligned = fit.scene(&reference);
            for (actual, expected) in aligned.pads.iter().zip(&target.pads) {
                close(actual.center, expected.center);
                for (a, b) in actual.polygon.iter().zip(&expected.polygon) {
                    close(*a, *b);
                }
            }
            close(aligned.tracks[0].start, target.tracks[0].start);
            close(aligned.tracks[0].end, target.tracks[0].end);
            close(aligned.vias[0].center, target.vias[0].center);
            for (a, b) in aligned.areas[0].outer.iter().zip(&target.areas[0].outer) {
                close(*a, *b);
            }
            close(aligned.areas[0].holes[0][0], target.areas[0].holes[0][0]);
            assert_eq!(aligned.tracks[0].layer, "F.Cu");
        }
        assert_eq!(reference, before);
    }

    #[test]
    fn svg_clips_panels_renders_fill_holes_and_includes_edge_overlaps() {
        let scene = Scene {
            pads: vec![pad("1", (-0.1, 1.0))],
            tracks: vec![SceneTrack {
                net: "SIG".into(),
                layer: "F.Cu".into(),
                start: (-5.0, 1.0),
                end: (15.0, 1.0),
                width: 0.2,
            }],
            areas: vec![
                SceneArea {
                    net: "GND".into(),
                    layer: "F.Cu".into(),
                    outer: vec![(-2.0, -2.0), (12.0, -2.0), (12.0, 12.0), (-2.0, 12.0)],
                    holes: vec![vec![(2.0, 2.0), (3.0, 2.0), (3.0, 3.0), (2.0, 3.0)]],
                    kind: "fill",
                },
                SceneArea {
                    net: "GND".into(),
                    layer: "B.Cu".into(),
                    outer: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
                    holes: vec![],
                    kind: "zone",
                },
            ],
            ..Scene::default()
        };
        let request = ViewRequest {
            window: [0.0, 0.0, 10.0, 10.0],
            layers: vec!["F.Cu".into(), "B.Cu".into()],
            highlight: vec![],
            markers: vec![(30.0, 1.0)],
        };
        let drawing = svg(&scene, &request, &[], None);
        assert!(drawing.contains("clip-path=\"url(#panel-0)\""));
        assert!(drawing.contains("clip-path=\"url(#panel-1)\""));
        assert!(drawing.contains("fill-rule=\"evenodd\" fill=\"#7f8c99\" fill-opacity=\"0.25\""));
        assert!(drawing.contains("M 2.0 2.0 L 3.0 2.0 L 3.0 3.0 L 2.0 3.0 Z"));
        assert!(drawing.contains("stroke-dasharray=\"0.2 0.15\""));
        assert!(drawing.contains("<polygon"), "pad touching the window must be drawn even if its center is outside");
        assert!(!drawing.contains("class=\"reference\""));
    }

    #[test]
    fn svg_contains_aligned_reference_copper_on_its_layer() {
        let reference = Scene {
            pads: vec![pad("1", (2.0, 3.0)), pad("2", (5.0, 3.0)), pad("3", (2.0, 5.0))],
            vias: vec![SceneVia {
                net: "SIG".into(),
                center: (8.0, 4.0),
                size: 0.6,
                drill: 0.3,
                layers: vec!["F.Cu".into()],
            }],
            ..Scene::default()
        };
        let mut target = reference.clone();
        for pad in &mut target.pads {
            pad.center = (20.0 - pad.center.0, 20.0 - pad.center.1);
            pad.polygon.iter_mut().for_each(|p| *p = (20.0 - p.0, 20.0 - p.1));
        }
        target.vias.clear();
        let fit = align_reference(&reference, "U1", &target, "U1").unwrap();
        let request = ViewRequest {
            window: [10.0, 10.0, 20.0, 20.0],
            layers: vec!["F.Cu".into(), "B.Cu".into()],
            highlight: vec![],
            markers: vec![],
        };
        let drawing = svg(&target, &request, &[], Some((&reference, &fit)));
        assert!(drawing.contains("stroke=\"#ff00ff\""));
        assert_eq!(
            drawing.matches("cx=\"12.0\" cy=\"16.0\"").count(),
            2,
            "transformed via land and drill only on F.Cu"
        );
        assert!(!drawing.contains("cx=\"8.0\""));
    }

    #[test]
    fn sums_like_python() {
        // sum([0.1] * 10) == 1.0 since Python 3.12.
        assert_eq!(float_sum(std::iter::repeat_n(0.1, 10)), 1.0);
        assert_eq!(float_sum([1e100, 1.0, -1e100]), 1.0);
        assert_eq!(round_to(0.00015, 4), 0.0001);
    }

    #[test]
    fn via_legend_keeps_foreign_context_and_prioritizes_highlight_with_an_exact_clip_count() {
        let via = |net: &str, x: f64, layers: &[&str]| SceneVia {
            net: net.into(),
            center: (x, 2.0),
            size: 0.45,
            drill: 0.20,
            layers: layers.iter().map(|layer| (*layer).to_owned()).collect(),
        };
        let mut scene = Scene {
            vias: vec![
                via("OTHER", 1.0, &["F.Cu", "B.Cu"]),
                via("SECOND", 3.0, &["F.Cu", "B.Cu"]),
                via("TARGET", 2.0, &["F.Cu", "B.Cu"]),
                via("HIDDEN_LAYER", 2.5, &["In2.Cu", "In3.Cu"]),
                via("OUTSIDE", 100.0, &["F.Cu", "B.Cu"]),
            ],
            ..Scene::default()
        };
        for index in 0..12 {
            scene.vias.push(via(&format!("EXTRA_{index}"), 4.0 + f64::from(index), &["F.Cu", "B.Cu"]));
        }
        let request = ViewRequest {
            window: [0.0, 0.0, 18.0, 4.0],
            layers: vec!["B.Cu".into()],
            highlight: vec![Value::from("TARGET")],
            markers: vec![],
        };
        let colors = net_colors(&request.highlight).unwrap();
        let lines = legend(&scene, &request, &colors, None);
        let samples: Vec<_> = lines.iter().filter(|line| line.starts_with("  via ")).collect();
        assert_eq!(samples.len(), 14);
        assert_eq!(samples[0], "  via TARGET @(2.000,2.000) diameter 0.450 mm drill 0.200 mm F.Cu/B.Cu");
        assert!(samples[1].starts_with("  via OTHER "));
        assert!(lines.iter().any(|line| line == "  ... 1 more visible vias"));
        assert!(!lines.iter().any(|line| line.contains("HIDDEN_LAYER") || line.contains("OUTSIDE")));
        let unhighlighted = ViewRequest { highlight: vec![], ..request };
        let all = legend(&scene, &unhighlighted, &[], None);
        let samples: Vec<_> = all.iter().filter(|line| line.starts_with("  via ")).collect();
        assert_eq!(samples.len(), 14);
        assert!(samples[0].starts_with("  via OTHER "));
    }
}

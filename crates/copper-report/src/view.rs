//! A window of a board for review, one panel per copper layer (`pcbir.view`).
//!
//! `copper view` draws pads, tracks, vias and markers in a millimetre window,
//! highlights chosen nets and can align a manufacturer reference layout on a
//! package's pads. Alongside the picture it returns a short text legend of
//! what is in the window, so a text-only reader still gets the facts.
//!
//! The picture is the SVG the Python implementation writes when Pillow is not
//! installed, which was the reference configuration (the qualification
//! environment installs no imaging library). A view is a reading aid: it
//! neither refills zones nor checks clearances.

use copper_json::{Value, python_float_repr};

use crate::py::{self, Py, PyErr};
use crate::scene::{Point, Scene, fixed};

pub const PALETTE: [&str; 8] = ["#d1495b", "#2e86ab", "#3c9d5d", "#e08e0b", "#8e5bb5", "#0f8b8d", "#c05299", "#6d7f1e"];
const OTHER: &str = "#b9c0c8";
const GROUND: &str = "#7f8c99";
const MARKER: &str = "#e00000";
/// The legend line the Python implementation adds when it falls back to SVG.
pub const SVG_NOTE: &str = "(Pillow not installed: wrote SVG)";

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
/// wins. (The aligned outlines themselves are only drawn into PNG views.)
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
    let mut best: Option<(bool, f64, f64)> = None;
    for mirrored in [false, true] {
        let source: Vec<Point> = common
            .iter()
            .map(|n| {
                let p = lookup(&theirs, n).expect("common");
                (if mirrored { -p.0 } else { p.0 }, p.1)
            })
            .collect();
        let target: Vec<Point> = common.iter().map(|n| lookup(&ours, n).expect("common")).collect();
        let (angle, _, rms) = kabsch(&source, &target);
        if best.is_none_or(|(_, _, least)| rms < least - 1e-9) {
            best = Some((mirrored, angle, rms));
        }
    }
    let (mirrored, angle, rms) = best.expect("two candidates");
    Ok(Alignment {
        reference: ref_component.to_owned(),
        target: component.to_owned(),
        angle_degrees: round_to(copper_physical::pyfloat::float_mod(angle.to_degrees(), 360.0), 3),
        mirrored,
        rms_mm: round_to(rms, 4),
        matched_pads: common.len(),
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
    }
    lines
}

// --- rendering ------------------------------------------------------------------------

fn r(value: f64) -> String {
    python_float_repr(value)
}

/// `_svg(scene, request, colors, reference)`: one group per layer, side by side.
pub fn svg(scene: &Scene, request: &ViewRequest, colors: &[(Value, &'static str)]) -> String {
    let [x0, y0, x1, y1] = request.window;
    let window = &request.window;
    let mut parts: Vec<String> = Vec::new();
    for (i, layer) in request.layers.iter().enumerate() {
        let ox = i as f64 * (x1 - x0 + 1.0);
        parts.push(format!(
            "<g transform=\"translate({},{})\"><text x=\"{}\" y=\"{}\" font-size=\"0.6\">{layer}</text>",
            r(ox - x0),
            r(-y0),
            r(x0),
            r(y0 - 0.3)
        ));
        for track in &scene.tracks {
            if &track.layer == layer && segment_hits(track.start, track.end, window) {
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
            if pad.layers.contains(layer) && inside(pad.center, window) {
                let points: Vec<String> =
                    pad.polygon.iter().map(|(x, y)| format!("{},{}", fixed(*x, 4), fixed(*y, 4))).collect();
                parts.push(format!("<polygon points=\"{}\" fill=\"{}\"/>", points.join(" "), color(&pad.net, colors)));
            }
        }
        for via in &scene.vias {
            if via.layers.contains(layer) && inside(via.center, window) {
                parts.push(format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\"/>",
                    r(via.center.0),
                    r(via.center.1),
                    r(via.size / 2.0),
                    color(&via.net, colors)
                ));
            }
        }
        for (x, y) in &request.markers {
            parts.push(format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"0.35\" fill=\"none\" stroke=\"{MARKER}\" stroke-width=\"0.08\"/>",
                r(*x),
                r(*y)
            ));
        }
        parts.push("</g>".to_owned());
    }
    let width = request.layers.len() as f64 * (x1 - x0 + 1.0);
    let height = y1 - y0 + 1.0;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 -1 {} {}\"><rect x=\"0\" y=\"-1\" width=\"{}\" height=\"{}\" fill=\"white\"/>{}</svg>",
        r(width),
        r(height),
        r(width),
        r(height),
        parts.concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_like_python() {
        // sum([0.1] * 10) == 1.0 since Python 3.12.
        assert_eq!(float_sum(std::iter::repeat_n(0.1, 10)), 1.0);
        assert_eq!(float_sum([1e100, 1.0, -1e100]), 1.0);
        assert_eq!(round_to(0.00015, 4), 0.0001);
    }
}

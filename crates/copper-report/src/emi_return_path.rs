//! The return-path map (`copper emi-check --plot-summary` on a board with `EMI-RETURN-PATH` findings): the board in
//! its own coordinates, millimetres, with the places where a return path is broken marked from the `measures` of the
//! findings and nothing else. Every mark is a numbered marker (a circle for a gap, a diamond for a reference change at
//! a via without a return path within its threshold, a square for a trace near the reference copper's edge, a triangle
//! for one near the board edge) and a compact legend beside the map (below it for a wide board) gives one short line
//! per number. Marks of one kind within a millimetre of each other are one mark, and a plane-edge mark within a
//! millimetre of a gap of the same subject is dropped (the gap already says the trace leaves the copper there). A gap
//! is also drawn as a short thick line across the trace when the traces of the subject say which way it runs. The
//! traces of the subject nets and the board outline are drawn behind the marks when a scene is given.
//!
//! **Screening evidence, never a pass or a fail.** The marks locate measured distances and lengths; a board with no
//! mark has nothing marked at these thresholds, which is not a pass. The footer of the document says what the figures
//! are, and each finding lists what the check leaves out in its `estimate.not_modelled`.

use copper_json::Value;

use crate::emi_field::FieldPicture;
use crate::emi_figure::{INK, MUTED, fit_text, hertz, one_decimal, sig3, text_el};
use crate::emi_plot::{escape, n};
use crate::scene::{Point, Scene};

const CODE: &str = "EMI-RETURN-PATH";
/// Width of the page, px: the width of the summary pages.
const WIDTH: f64 = 1400.0;
const MARGIN: f64 = 24.0;
const MAP_MAX_HEIGHT: f64 = 560.0;
/// The legend column right of the map, px, and the lines it lists at most.
const LEGEND_W: f64 = 440.0;
pub const MAX_LEGEND: usize = 24;
/// Marks of one kind this close (mm) are one mark.
const MERGE_MM: f64 = 1.0;
/// A board this much wider than tall puts its legend below the map.
const WIDE: f64 = 2.2;
const RULE: &str = "#aab4be";
const TRACK: &str = "#c4ccd4";
const GAP: &str = "#d4380d";
const VIA: &str = "#c2410c";
const PLANE_EDGE: &str = "#0e7490";
const BOARD_EDGE: &str = "#7c3aed";

/// What a located cause is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CauseKind {
    /// A void of the reference copper that a trace crosses.
    Gap,
    /// A reference change at a via with no return path within the threshold.
    UnreturnedVia,
    /// A trace within the edge limit of the reference copper's outer edge.
    PlaneEdge,
    /// A trace within the edge limit of the board outline.
    BoardEdge,
}

impl CauseKind {
    /// The name in JSON and in the `data-kind` attribute of the plot.
    pub fn name(self) -> &'static str {
        match self {
            CauseKind::Gap => "gap",
            CauseKind::UnreturnedVia => "reference_change_without_return_path",
            CauseKind::PlaneEdge => "plane_edge",
            CauseKind::BoardEdge => "board_edge",
        }
    }
}

/// One located cause read from the `measures` of an `EMI-RETURN-PATH` finding.
#[derive(Clone, Debug, PartialEq)]
pub struct Cause {
    pub kind: CauseKind,
    /// Board coordinates, millimetres.
    pub at: Point,
    /// What was measured there, in words.
    pub text: String,
    /// The measurement as a legend line says it: `slot 76 mm, 1.12 GHz`, `return 7.2 mm > 2.0 mm`, `1.0 mm < 4.7 mm`.
    pub short: String,
    /// The distance of an edge point to the edge, mm; the return distance of a via.
    pub distance_mm: Option<f64>,
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn point(value: Option<&Value>) -> Option<Point> {
    let pair = value?.as_array()?;
    Some((number(pair.first()?)?, number(pair.get(1)?)?))
}

fn field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(number)
}

fn flag(value: &Value, key: &str) -> Option<bool> {
    match value.get(key) {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    }
}

/// The findings of `document` (an `emi-check` document or a run's `emi` section) with the code `EMI-RETURN-PATH`.
pub fn findings(document: &Value) -> Vec<&Value> {
    document
        .get("findings")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("code").and_then(Value::as_str) == Some(CODE))
        .collect()
}

/// The name of the subject of `finding`: its `measures.subject`, else its nets joined.
pub fn subject(finding: &Value) -> String {
    match finding.get("measures").and_then(|m| m.get("subject")).and_then(Value::as_str) {
        Some(name) => name.to_owned(),
        None => finding
            .get("nets")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("/"),
    }
}

/// The located causes of `finding`, in the order gaps, reference changes without a return path within the threshold,
/// plane-edge points within the limit, board-edge points within the limit. A point is listed once per kind.
pub fn causes(finding: &Value) -> Vec<Cause> {
    let mut found: Vec<Cause> = Vec::new();
    let mut add = |kind: CauseKind, at: Point, text: String, short: String, distance_mm: Option<f64>| {
        if !found.iter().any(|c| c.kind == kind && c.at == at) {
            found.push(Cause { kind, at, text, short, distance_mm });
        }
    };
    let Some(measures) = finding.get("measures") else { return Vec::new() };
    for gap in measures.get("gaps").and_then(Value::as_array).unwrap_or_default() {
        let Some(at) = point(gap.get("position_mm")) else { continue };
        let mut parts: Vec<String> = Vec::new();
        let mut short: Vec<String> = Vec::new();
        if let Some(slot) = field(gap, "slot_length_mm") {
            parts.push(format!("slot {} mm", n(slot)));
            short.push(format!("slot {} mm", sig3(slot)));
        }
        if let Some(resonance) = field(gap, "resonance_hz") {
            parts.push(format!("resonance {} MHz", n(resonance / 1e6)));
            short.push(hertz(resonance));
        }
        let mut text = parts.join(", ");
        if gap.get("kind").and_then(Value::as_str) == Some("cut_to_plane_edge") {
            text.push_str(" (cut to the plane edge)");
        }
        add(CauseKind::Gap, at, text, short.join(", "), None);
    }
    for change in measures.get("reference_changes").and_then(Value::as_array).unwrap_or_default() {
        if flag(change, "within_threshold") != Some(false) {
            continue;
        }
        let Some(at) = point(change.get("position_mm")) else { continue };
        let name = change.get("via").and_then(Value::as_str).unwrap_or("via");
        let threshold = field(change, "threshold_mm").map_or_else(String::new, |t| format!(", threshold {} mm", n(t)));
        let text = match field(change, "return_distance_mm") {
            Some(distance) => format!("{name}: nearest return path {} mm{threshold}", n(distance)),
            None => format!("{name}: no return path found{threshold}"),
        };
        let threshold_short =
            field(change, "threshold_mm").map_or_else(String::new, |t| format!(" > {} mm", one_decimal(t)));
        let short = match field(change, "return_distance_mm") {
            Some(distance) => format!("return {} mm{threshold_short}", one_decimal(distance)),
            None => format!("no return path{threshold_short}"),
        };
        add(CauseKind::UnreturnedVia, at, text, short, field(change, "return_distance_mm"));
    }
    for (key, kind, name) in [
        ("plane_edge", CauseKind::PlaneEdge, "reference copper edge"),
        ("board_edge", CauseKind::BoardEdge, "board edge"),
    ] {
        for member in measures.get("members").and_then(Value::as_array).unwrap_or_default() {
            for layer in member.get("layers").and_then(Value::as_array).unwrap_or_default() {
                let Some(edge) = layer.get(key) else { continue };
                if flag(edge, "within_limit") != Some(true) {
                    continue;
                }
                let Some(at) = point(edge.get("at_mm")) else { continue };
                let layer_name = layer.get("layer").and_then(Value::as_str).unwrap_or("");
                let distance = field(edge, "nearest_mm").map_or_else(String::new, n);
                let mut text = format!("{layer_name}: trace {distance} mm from the {name}");
                if let Some(limit) = field(edge, "limit_mm") {
                    text.push_str(&format!(" (limit {} mm)", n(limit)));
                }
                let nearest = field(edge, "nearest_mm");
                let short = match (nearest, field(edge, "limit_mm")) {
                    (Some(d), Some(l)) => format!("{} mm < {} mm", one_decimal(d), one_decimal(l)),
                    (Some(d), None) => format!("{} mm", one_decimal(d)),
                    _ => String::new(),
                };
                add(kind, at, text, short, nearest);
            }
        }
    }
    found
}

/// One mark of the map: the causes of one kind that coincide, with the subjects they belong to.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: CauseKind,
    /// Board coordinates, millimetres.
    pub at: Point,
    /// The subjects (`measures.subject`, the pair label) the mark belongs to, without repeats.
    pub subjects: Vec<(String, String)>,
    /// The measurement in a legend line.
    pub short: String,
    pub distance_mm: Option<f64>,
}

fn distance(a: Point, b: Point) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn rank(kind: CauseKind) -> usize {
    match kind {
        CauseKind::Gap => 0,
        CauseKind::UnreturnedVia => 1,
        CauseKind::PlaneEdge => 2,
        CauseKind::BoardEdge => 3,
    }
}

/// The marks of `findings`: a plane-edge mark within a millimetre of a gap of its own subject is dropped, marks of one
/// kind within a millimetre of each other are merged (an edge keeps the smaller distance, the subjects are all listed),
/// and the rest are ordered gaps, vias, plane edges, board edges (each in report order).
pub fn marks(findings: &[&Value]) -> Vec<Mark> {
    let mut raw: Vec<(String, String, Cause)> = Vec::new();
    for finding in findings {
        let (key, label) = (subject(finding), crate::emi_signal::subject_label(finding));
        raw.extend(causes(finding).into_iter().map(|c| (key.clone(), label.clone(), c)));
    }
    let mut merged: Vec<Mark> = Vec::new();
    for (key, label, cause) in raw {
        match merged.iter_mut().find(|m| m.kind == cause.kind && distance(m.at, cause.at) <= MERGE_MM) {
            Some(mark) => {
                if !mark.subjects.iter().any(|(k, _)| *k == key) {
                    mark.subjects.push((key, label));
                }
                let nearer = matches!(cause.kind, CauseKind::PlaneEdge | CauseKind::BoardEdge)
                    && cause.distance_mm.zip(mark.distance_mm).is_some_and(|(new, old)| new < old);
                if nearer {
                    mark.at = cause.at;
                    mark.short = cause.short;
                    mark.distance_mm = cause.distance_mm;
                }
            }
            None => merged.push(Mark {
                kind: cause.kind,
                at: cause.at,
                subjects: vec![(key, label)],
                short: cause.short,
                distance_mm: cause.distance_mm,
            }),
        }
    }
    // A plane-edge mark where every one of its subjects has a gap within a millimetre is dropped: the gap says it.
    let gaps: Vec<(String, Point)> = merged
        .iter()
        .filter(|m| m.kind == CauseKind::Gap)
        .flat_map(|m| m.subjects.iter().map(|(key, _)| (key.clone(), m.at)))
        .collect();
    merged.retain(|m| {
        m.kind != CauseKind::PlaneEdge
            || !m.subjects.iter().all(|(key, _)| gaps.iter().any(|(k, at)| k == key && distance(*at, m.at) <= MERGE_MM))
    });
    merged.sort_by_key(|m| rank(m.kind));
    merged
}

impl Mark {
    fn label(&self) -> String {
        let mut labels: Vec<&str> = Vec::new();
        for (_, label) in &self.subjects {
            if !labels.contains(&label.as_str()) {
                labels.push(label);
            }
        }
        labels.join(", ")
    }

    fn keys(&self) -> String {
        self.subjects.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join(", ")
    }

    fn legend(&self, number: usize) -> String {
        let kind = match self.kind {
            CauseKind::Gap => "gap",
            CauseKind::UnreturnedVia => "via",
            CauseKind::PlaneEdge => "plane edge",
            CauseKind::BoardEdge => "board edge",
        };
        format!("{number}  {kind}  {}  {}", self.label(), self.short)
    }
}

/// The shape of a mark: the numbered marker when `number` is given, else the plain glyph of the legend.
fn marker(kind: CauseKind, cx: f64, cy: f64, scale: f64, number: Option<usize>) -> String {
    let outline = "stroke=\"#ffffff\" stroke-width=\"1.5\"";
    let (h9, h11) = (9.0 * scale, 11.0 * scale);
    let shape = match kind {
        CauseKind::Gap => {
            format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{GAP}\" {outline}/>", n(cx), n(cy), n(h9))
        }
        CauseKind::UnreturnedVia => format!(
            "<polygon points=\"{},{} {},{} {},{} {},{}\" fill=\"{VIA}\" {outline}/>",
            n(cx),
            n(cy - h11),
            n(cx + h11),
            n(cy),
            n(cx),
            n(cy + h11),
            n(cx - h11),
            n(cy)
        ),
        CauseKind::PlaneEdge => format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{PLANE_EDGE}\" {outline}/>",
            n(cx - h9),
            n(cy - h9),
            n(2.0 * h9),
            n(2.0 * h9)
        ),
        CauseKind::BoardEdge => format!(
            "<polygon points=\"{},{} {},{} {},{}\" fill=\"{BOARD_EDGE}\" {outline}/>",
            n(cx),
            n(cy - h11),
            n(cx + h11),
            n(cy + h9),
            n(cx - h11),
            n(cy + h9)
        ),
    };
    match number {
        Some(number) => {
            let dy = if kind == CauseKind::BoardEdge { 6.0 } else { 3.5 };
            format!(
                "{shape}\n{}",
                text_el(cx, cy + dy, 10.0, "#ffffff", "mark-number", "middle", true, &number.to_string())
            )
        }
        None => shape,
    }
}

/// The unit direction of the track of `nets` nearest to `at` (within a millimetre and a half), for orienting a gap.
fn trace_direction(scene: &Scene, nets: &[String], at: Point) -> Option<Point> {
    let mut best: Option<(f64, Point)> = None;
    for track in scene.tracks.iter().filter(|t| nets.contains(&t.net)) {
        let (a, b) = (track.start, track.end);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length2 = dx * dx + dy * dy;
        if length2 <= 0.0 {
            continue;
        }
        let t = (((at.0 - a.0) * dx + (at.1 - a.1) * dy) / length2).clamp(0.0, 1.0);
        let d = distance(at, (a.0 + t * dx, a.1 + t * dy));
        if d <= 1.5 && best.is_none_or(|(best_d, _)| d < best_d) {
            let length = length2.sqrt();
            best = Some((d, (dx / length, dy / length)));
        }
    }
    best.map(|(_, direction)| direction)
}

/// Where a marker of the numbered kind is drawn: at its position, or the nearest free spot around it (two numbers never
/// overlap), inside the map.
fn place(wanted: (f64, f64), placed: &[(f64, f64)], bounds: [f64; 4]) -> (f64, f64) {
    let free = |p: (f64, f64)| {
        p.0 >= bounds[0]
            && p.0 <= bounds[2]
            && p.1 >= bounds[1]
            && p.1 <= bounds[3]
            && placed.iter().all(|q| distance(*q, p) >= 22.0)
    };
    if free(wanted) {
        return wanted;
    }
    for ring in 1..=8 {
        let radius = 24.0 * f64::from(ring);
        for step in 0..8 {
            let angle = f64::from(step) * std::f64::consts::FRAC_PI_4;
            let p = (wanted.0 + radius * angle.cos(), wanted.1 + radius * angle.sin());
            if free(p) {
                return p;
            }
        }
    }
    wanted
}

/// The map of every `EMI-RETURN-PATH` finding of `document`, or `None` when it has none. `scene` adds the board
/// outline and the traces of the subjects' nets behind the marks.
pub fn return_path_picture(document: &Value, scene: Option<&Scene>) -> Option<FieldPicture> {
    let all = findings(document);
    if all.is_empty() {
        return None;
    }
    let marks = marks(&all);
    let nets: Vec<String> = all
        .iter()
        .flat_map(|f| f.get("nets").and_then(Value::as_array).unwrap_or_default())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let tracks: Vec<_> = scene.map(|s| s.tracks.iter().filter(|t| nets.contains(&t.net)).collect()).unwrap_or_default();
    let edges: &[(Point, Point)] = scene.map_or(&[], |s| s.edges.as_slice());

    // The window: the outline, the marks and the traces, with a margin.
    let mut window: Option<[f64; 4]> = None;
    let mut grow = |p: Point| {
        window =
            Some(window.map_or([p.0, p.1, p.0, p.1], |w| [w[0].min(p.0), w[1].min(p.1), w[2].max(p.0), w[3].max(p.1)]));
    };
    for (a, b) in edges {
        grow(*a);
        grow(*b);
    }
    for mark in &marks {
        grow(mark.at);
    }
    for track in &tracks {
        grow(track.start);
        grow(track.end);
    }
    let mut out: Vec<String> = Vec::new();
    out.push("<g class=\"return-path-map\">".to_owned());
    out.push(text_el(MARGIN, 34.0, 20.0, INK, "title", "", true, "Where the return path is broken"));
    out.push(text_el(
        MARGIN,
        56.0,
        13.0,
        MUTED,
        "subtitle",
        "",
        false,
        "numbered marks on the board, mm (y down); marks of one kind within 1 mm are one",
    ));
    let mut y = 76.0;
    match window {
        None => {
            out.push(text_el(
                MARGIN,
                y + 12.0,
                12.0,
                INK,
                "none",
                "",
                false,
                "no gap, reference change or edge point is located at these thresholds",
            ));
            y += 30.0;
        }
        Some(w) => {
            let pad = ((w[2] - w[0]).max(w[3] - w[1]) * 0.05).max(2.0);
            let (x0, y0) = (w[0] - pad, w[1] - pad);
            let (wm, hm) = ((w[2] - w[0]) + 2.0 * pad, (w[3] - w[1]) + 2.0 * pad);
            let below = wm / hm > WIDE;
            let map_width = if below { WIDTH - 2.0 * MARGIN } else { WIDTH - 3.0 * MARGIN - LEGEND_W };
            let scale = (map_width / wm).min(MAP_MAX_HEIGHT / hm);
            let (pw, ph) = (wm * scale, hm * scale);
            let top = y;
            let to_px = |p: Point| (MARGIN + (p.0 - x0) * scale, top + (p.1 - y0) * scale);
            out.push(format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#fbfcfd\" stroke=\"{RULE}\"/>",
                n(MARGIN),
                n(top),
                n(pw),
                n(ph)
            ));
            for track in &tracks {
                let (a, b) = (to_px(track.start), to_px(track.end));
                out.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{TRACK}\" stroke-width=\"{}\" stroke-linecap=\"round\"/>",
                    n(a.0),
                    n(a.1),
                    n(b.0),
                    n(b.1),
                    n((track.width * scale).max(1.0))
                ));
            }
            for (start, end) in edges {
                let (a, b) = (to_px(*start), to_px(*end));
                out.push(format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1.5\"/>",
                    n(a.0),
                    n(a.1),
                    n(b.0),
                    n(b.1)
                ));
            }
            // The gaps first, as a thick line across the trace where the traces say which way it runs.
            for mark in marks.iter().filter(|m| m.kind == CauseKind::Gap) {
                let subject_nets: Vec<String> = all
                    .iter()
                    .filter(|f| mark.subjects.iter().any(|(key, _)| *key == subject(f)))
                    .flat_map(|f| f.get("nets").and_then(Value::as_array).unwrap_or_default())
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                if let Some(direction) = scene.and_then(|s| trace_direction(s, &subject_nets, mark.at)) {
                    let (px, py) = to_px(mark.at);
                    let across = (-direction.1 * 14.0, direction.0 * 14.0);
                    out.push(format!(
                        "<line class=\"gap-line\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GAP}\" stroke-width=\"3\" stroke-linecap=\"round\"/>",
                        n(px - across.0),
                        n(py - across.1),
                        n(px + across.0),
                        n(py + across.1)
                    ));
                }
            }
            let bounds = [MARGIN + 10.0, top + 10.0, MARGIN + pw - 10.0, top + ph - 10.0];
            let mut placed: Vec<(f64, f64)> = Vec::new();
            for (index, mark) in marks.iter().enumerate() {
                let number = index + 1;
                let true_at = to_px(mark.at);
                let at = place(true_at, &placed, bounds);
                placed.push(at);
                out.push(format!(
                    "<g class=\"return-path-mark\" data-kind=\"{}\" data-subject=\"{}\" data-x-mm=\"{}\" data-y-mm=\"{}\" data-number=\"{number}\">",
                    mark.kind.name(),
                    escape(&mark.keys()),
                    n(mark.at.0),
                    n(mark.at.1)
                ));
                // A leader back to the true position, unless an earlier marker already sits on it (two marks at one
                // spot: the neighbour shows where, and a leader would be drawn over its number).
                let covered = placed[..index].iter().any(|q| distance(*q, true_at) < 12.0);
                if distance(at, true_at) > 1.0 && !covered {
                    out.push(format!(
                        "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{INK}\" stroke-width=\"1\"/>\n<circle cx=\"{}\" cy=\"{}\" r=\"2\" fill=\"{INK}\"/>",
                        n(true_at.0),
                        n(true_at.1),
                        n(at.0),
                        n(at.1),
                        n(true_at.0),
                        n(true_at.1)
                    ));
                }
                out.push(marker(mark.kind, at.0, at.1, 1.0, Some(number)));
                out.push("</g>".to_owned());
            }
            // The legend: a column right of the map, or two columns below a wide one.
            let shown = marks.len().min(MAX_LEGEND);
            let (columns, column_w, left, legend_top) = if below {
                (2, (WIDTH - 2.0 * MARGIN) / 2.0, MARGIN, top + ph + 22.0)
            } else {
                (1, LEGEND_W, MARGIN + pw + MARGIN, top + 14.0)
            };
            let per_column = shown.div_ceil(columns).max(1);
            for (index, mark) in marks.iter().take(shown).enumerate() {
                let (column, row) = (index / per_column, index % per_column);
                let (lx, ly) = (left + column as f64 * column_w, legend_top + row as f64 * 18.0);
                out.push(marker(mark.kind, lx + 7.0, ly - 4.0, 0.6, None));
                out.push(text_el(
                    lx + 20.0,
                    ly,
                    11.0,
                    INK,
                    "legend",
                    "",
                    false,
                    &fit_text(&mark.legend(index + 1), 11.0, column_w - 28.0),
                ));
            }
            let mut legend_bottom = legend_top + (per_column as f64) * 18.0;
            if marks.len() > shown {
                out.push(text_el(
                    left + 20.0,
                    legend_bottom,
                    11.0,
                    MUTED,
                    "more",
                    "",
                    false,
                    &fit_text(
                        &format!(
                            "+{} more, numbered {} to {}: copper issues RUN --stage emi",
                            marks.len() - shown,
                            shown + 1,
                            marks.len()
                        ),
                        11.0,
                        columns as f64 * column_w - 28.0,
                    ),
                ));
                legend_bottom += 18.0;
            }
            y = if below { legend_bottom + 8.0 } else { (top + ph).max(legend_bottom) + 8.0 };
        }
    }
    out.push("</g>".to_owned());
    Some(FieldPicture { content: out.join("\n"), width: WIDTH, height: y + 10.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emi_figure::check::{assert_inside, assert_no_overlap, plain, text_boxes};

    fn document() -> Value {
        copper_json::parse(
            r#"{"findings":[
              {"code":"EMI-HOT-LOOP","measures":{}},
              {"code":"EMI-RETURN-PATH","nets":["A_P","A_N"],"measures":{"subject":"A_P",
               "gaps":[{"position_mm":[40.0,40.0],"slot_length_mm":76.0,"resonance_hz":1123027388.0,"kind":"cut_to_plane_edge"}],
               "reference_changes":[
                 {"via":"via:2","position_mm":[25.0,62.0],"return_distance_mm":7.1589,"threshold_mm":2.0,"within_threshold":false},
                 {"via":"via:3","position_mm":[5.0,5.0],"return_distance_mm":1.0,"threshold_mm":2.0,"within_threshold":true},
                 {"via":"via:4","position_mm":[6.0,6.0],"return_distance_mm":null,"threshold_mm":2.0,"within_threshold":false}],
               "members":[{"layers":[{"layer":"F.Cu",
                 "plane_edge":{"at_mm":[3.0,30.0],"nearest_mm":1.0,"limit_mm":4.7,"within_limit":true},
                 "board_edge":{"at_mm":[3.0,30.0],"nearest_mm":3.0,"limit_mm":4.7,"within_limit":false}}]}]}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn causes_are_read_from_the_measures_only() {
        let doc = document();
        let found = findings(&doc);
        assert_eq!(found.len(), 1);
        let list = causes(found[0]);
        let kinds: Vec<&str> = list.iter().map(|c| c.kind.name()).collect();
        assert_eq!(
            kinds,
            ["gap", "reference_change_without_return_path", "reference_change_without_return_path", "plane_edge"]
        );
        assert_eq!(list[0].at, (40.0, 40.0));
        assert_eq!(list[0].text, "slot 76 mm, resonance 1123.03 MHz (cut to the plane edge)");
        assert_eq!(list[1].text, "via:2: nearest return path 7.16 mm, threshold 2 mm");
        assert_eq!(list[2].text, "via:4: no return path found, threshold 2 mm");
        assert_eq!(list[3].text, "F.Cu: trace 1 mm from the reference copper edge (limit 4.7 mm)");
        let short: Vec<&str> = list.iter().map(|c| c.short.as_str()).collect();
        assert_eq!(
            short,
            ["slot 76 mm, 1.12 GHz", "return 7.2 mm > 2.0 mm", "no return path > 2.0 mm", "1.0 mm < 4.7 mm"]
        );
    }

    /// A pair with a gap, a plane edge at it and board edges that coincide with those of two other subjects.
    fn merging() -> Value {
        copper_json::parse(
            r#"{"findings":[
              {"code":"EMI-RETURN-PATH","nets":["LANE_B_P","LANE_B_N"],"measures":{"subject":"LANE_B_P",
               "gaps":[{"position_mm":[40.0,40.0],"slot_length_mm":76.0,"resonance_hz":1123027388.0},
                       {"position_mm":[40.0,40.3],"slot_length_mm":76.0,"resonance_hz":1123027388.0}],
               "members":[{"layers":[{"layer":"F.Cu",
                 "plane_edge":{"at_mm":[41.0,40.0],"nearest_mm":0.0,"limit_mm":4.7,"within_limit":true},
                 "board_edge":{"at_mm":[30.0,40.0],"nearest_mm":3.0,"limit_mm":4.7,"within_limit":true}}]},
                {"layers":[{"layer":"F.Cu",
                 "board_edge":{"at_mm":[30.5,40.0],"nearest_mm":2.0,"limit_mm":4.7,"within_limit":true}}]}]}},
              {"code":"EMI-RETURN-PATH","nets":["CLK_E"],"measures":{"subject":"CLK_E",
               "members":[{"layers":[{"layer":"F.Cu",
                 "plane_edge":{"at_mm":[3.0,30.0],"nearest_mm":1.0,"limit_mm":4.7,"within_limit":true},
                 "board_edge":{"at_mm":[3.0,30.0],"nearest_mm":3.0,"limit_mm":4.7,"within_limit":true}}]}]}},
              {"code":"EMI-RETURN-PATH","nets":["CLK_F"],"measures":{"subject":"CLK_F",
               "members":[{"layers":[{"layer":"F.Cu",
                 "board_edge":{"at_mm":[30.2,40.4],"nearest_mm":2.5,"limit_mm":4.7,"within_limit":true}}]}]}}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn marks_that_coincide_are_one_and_a_plane_edge_at_a_gap_is_dropped() {
        let doc = merging();
        let found = marks(&findings(&doc));
        let kinds: Vec<(&str, String)> = found.iter().map(|m| (m.kind.name(), m.label())).collect();
        assert_eq!(
            kinds,
            [
                ("gap", "LANE_B_P/N".to_owned()),
                ("plane_edge", "CLK_E".to_owned()),
                ("board_edge", "LANE_B_P/N, CLK_F".to_owned()),
                ("board_edge", "CLK_E".to_owned())
            ],
            "the two gaps merge, the plane edge at the gap goes, the board edges within a millimetre merge"
        );
        assert_eq!(found[2].short, "2.0 mm < 4.7 mm", "an edge keeps the smaller distance");
        assert_eq!(found[2].distance_mm, Some(2.0));
        assert_eq!(found[2].keys(), "LANE_B_P, CLK_F");
    }

    #[test]
    fn the_map_has_a_numbered_mark_per_cause_and_a_legend_and_none_without_findings() {
        let doc = document();
        let picture = return_path_picture(&doc, None).expect("a picture");
        // The gap, the two vias and the one plane-edge point: the board-edge point is not within the limit.
        assert_eq!(picture.content.matches("class=\"return-path-mark\"").count(), 4);
        for number in 1..=4 {
            assert!(picture.content.contains(&format!("data-number=\"{number}\"")), "{number}");
        }
        let text = plain(&picture.content);
        for line in [
            "1  gap  A_P/N  slot 76 mm, 1.12 GHz",
            "2  via  A_P/N  return 7.2 mm > 2.0 mm",
            "3  via  A_P/N  no return path > 2.0 mm",
            "4  plane edge  A_P/N  1.0 mm < 4.7 mm",
        ] {
            assert!(text.contains(line), "{line} in {text}");
        }
        // The labels are not at the marks any more: the only texts are the title, the legend and the numbers.
        assert!(!text.contains("nearest return path") && !text.contains("resonance"), "{text}");
        assert_eq!(picture.width, WIDTH);
        assert_inside(&picture.content, picture.width, "map");
        assert_no_overlap(&picture.content, "map legend and numbers");
        let none = copper_json::parse(r#"{"findings":[{"code":"EMI-HOT-LOOP","measures":{}}]}"#).unwrap();
        assert!(return_path_picture(&none, None).is_none());
    }

    #[test]
    fn marks_close_on_the_page_are_moved_apart_and_a_long_legend_is_cut() {
        // Forty vias along a short line: their numbers would sit on each other.
        let vias: Vec<String> = (0..40)
            .map(|k| {
                format!(
                    r#"{{"via":"v{k}","position_mm":[{}.0,10.0],"return_distance_mm":9.0,"threshold_mm":2.0,"within_threshold":false}}"#,
                    10 + 2 * k
                )
            })
            .collect();
        let doc = copper_json::parse(&format!(
            r#"{{"findings":[{{"code":"EMI-RETURN-PATH","nets":["S"],"measures":{{"subject":"S","reference_changes":[{}]}}}}]}}"#,
            vias.join(",")
        ))
        .unwrap();
        let picture = return_path_picture(&doc, None).unwrap();
        assert_eq!(picture.content.matches("class=\"return-path-mark\"").count(), 40);
        assert_eq!(picture.content.matches("class=\"legend\"").count(), MAX_LEGEND);
        assert!(plain(&picture.content).contains("+16 more, numbered 25 to 40: copper issues RUN --stage emi"));
        assert_inside(&picture.content, picture.width, "long map");
        assert_no_overlap(&picture.content, "long map");
        assert!(text_boxes(&picture.content).iter().all(|t| t.top >= 0.0));
    }

    #[test]
    fn a_gap_is_also_drawn_across_its_trace_when_the_scene_has_it() {
        let doc = document();
        let scene = Scene {
            name: "s".to_owned(),
            copper_layers: vec!["F.Cu".to_owned()],
            pads: Vec::new(),
            tracks: vec![crate::scene::SceneTrack {
                net: "A_P".to_owned(),
                layer: "F.Cu".to_owned(),
                start: (20.0, 40.0),
                end: (60.0, 40.0),
                width: 0.2,
            }],
            vias: Vec::new(),
            areas: Vec::new(),
            edges: Vec::new(),
            footprints: Default::default(),
            source: String::new(),
        };
        let with = return_path_picture(&doc, Some(&scene)).unwrap();
        assert_eq!(with.content.matches("class=\"gap-line\"").count(), 1);
        // The trace runs along x, so the line across it is vertical.
        let line = with.content.split("class=\"gap-line\"").nth(1).unwrap();
        let value = |name: &str| -> f64 {
            line.split(&format!(" {name}=\"")).nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
        };
        assert!((value("x1") - value("x2")).abs() < 0.01 && (value("y1") - value("y2")).abs() > 10.0);
        let without = return_path_picture(&doc, None).unwrap();
        assert!(!without.content.contains("gap-line"), "no trace, no orientation");
    }
}

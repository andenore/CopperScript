//! The return-path map (`copper emi-check --plot-summary` on a board with `EMI-RETURN-PATH` findings): the board in
//! its own coordinates, millimetres, with the places where a return path is broken marked from the `measures` of the
//! findings and nothing else: each gap position labelled with its slot length and half-wave resonance, each reference
//! change at a via without a return path within its threshold (the distance against the threshold), and the nearest
//! plane-edge and board-edge points that are within the edge limit. The traces of the subject nets and the board
//! outline are drawn behind the marks when a scene is given.
//!
//! **Screening evidence, never a pass or a fail.** The marks locate measured distances and lengths; a board with no
//! mark has nothing marked at these thresholds, which is not a pass. What the check leaves out (slot radiation
//! efficiency, plane-pair cavity modes, enclosure and cable coupling, the other plane of a stripline) is stated on the
//! page and listed in each finding's `estimate.not_modelled`.

use copper_json::Value;

use crate::emi_field::{FieldPicture, text_px, wrap_px};
use crate::emi_plot::{escape, n};
use crate::scene::{Point, Scene};

const CODE: &str = "EMI-RETURN-PATH";
const WIDTH: f64 = 900.0;
const MARGIN: f64 = 24.0;
const MAP_MAX_HEIGHT: f64 = 560.0;
const INK: &str = "#1f2933";
const MUTED: &str = "#52606d";
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
    let mut add = |kind: CauseKind, at: Point, text: String| {
        if !found.iter().any(|c| c.kind == kind && c.at == at) {
            found.push(Cause { kind, at, text });
        }
    };
    let Some(measures) = finding.get("measures") else { return Vec::new() };
    for gap in measures.get("gaps").and_then(Value::as_array).unwrap_or_default() {
        let Some(at) = point(gap.get("position_mm")) else { continue };
        let mut parts: Vec<String> = Vec::new();
        if let Some(slot) = field(gap, "slot_length_mm") {
            parts.push(format!("slot {} mm", n(slot)));
        }
        if let Some(resonance) = field(gap, "resonance_hz") {
            parts.push(format!("resonance {} MHz", n(resonance / 1e6)));
        }
        let mut text = parts.join(", ");
        if gap.get("kind").and_then(Value::as_str) == Some("cut_to_plane_edge") {
            text.push_str(" (cut to the plane edge)");
        }
        add(CauseKind::Gap, at, text);
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
        add(CauseKind::UnreturnedVia, at, text);
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
                add(kind, at, text);
            }
        }
    }
    found
}

fn text_element(x: f64, y: f64, size: f64, fill: &str, body: &str, extra: &str) -> String {
    format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{fill}\"{extra}>{}</text>",
        n(x),
        n(y),
        n(size),
        escape(body)
    )
}

fn marker(kind: CauseKind, cx: f64, cy: f64) -> String {
    let outline = "stroke=\"#ffffff\" stroke-width=\"1.5\"";
    match kind {
        CauseKind::Gap => format!("<circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"{GAP}\" {outline}/>", n(cx), n(cy)),
        CauseKind::UnreturnedVia => format!(
            "<polygon points=\"{},{} {},{} {},{} {},{}\" fill=\"{VIA}\" {outline}/>",
            n(cx),
            n(cy - 7.0),
            n(cx + 7.0),
            n(cy),
            n(cx),
            n(cy + 7.0),
            n(cx - 7.0),
            n(cy)
        ),
        CauseKind::PlaneEdge => format!(
            "<rect x=\"{}\" y=\"{}\" width=\"11\" height=\"11\" fill=\"{PLANE_EDGE}\" {outline}/>",
            n(cx - 5.5),
            n(cy - 5.5)
        ),
        CauseKind::BoardEdge => format!(
            "<polygon points=\"{},{} {},{} {},{}\" fill=\"{BOARD_EDGE}\" {outline}/>",
            n(cx),
            n(cy - 7.0),
            n(cx + 7.0),
            n(cy + 6.0),
            n(cx - 7.0),
            n(cy + 6.0)
        ),
    }
}

fn paragraph(out: &mut Vec<String>, y: &mut f64, size: f64, colour: &str, body: &str) {
    for line in wrap_px(body, size, WIDTH - 2.0 * MARGIN) {
        out.push(text_element(MARGIN, *y, size, colour, &line, ""));
        *y += size * 1.35;
    }
}

/// The map of every `EMI-RETURN-PATH` finding of `document`, or `None` when it has none. `scene` adds the board
/// outline and the traces of the subjects' nets behind the marks.
pub fn return_path_picture(document: &Value, scene: Option<&Scene>) -> Option<FieldPicture> {
    let all = findings(document);
    if all.is_empty() {
        return None;
    }
    let mut marks: Vec<(String, Cause)> = Vec::new();
    for finding in &all {
        let name = subject(finding);
        marks.extend(causes(finding).into_iter().map(|c| (name.clone(), c)));
    }
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
    for (_, cause) in &marks {
        grow(cause.at);
    }
    for track in &tracks {
        grow(track.start);
        grow(track.end);
    }
    let mut out: Vec<String> = Vec::new();
    let mut y = 28.0;
    out.push(text_element(
        MARGIN,
        y,
        17.0,
        INK,
        "EMI-RETURN-PATH: where the return path is broken (screening evidence)",
        " font-weight=\"bold\"",
    ));
    y += 20.0;
    let counts = |kind: CauseKind| marks.iter().filter(|(_, c)| c.kind == kind).count();
    paragraph(
        &mut out,
        &mut y,
        12.0,
        MUTED,
        &format!(
            "{} finding(s), board coordinates in mm (y down, as the layout). {} gap(s), {} reference change(s) without a return path within the threshold, {} plane-edge and {} board-edge point(s) within the edge limit. Marks are read from the measures of the findings; a board with no mark has nothing marked at these thresholds, which is not a pass.",
            all.len(),
            counts(CauseKind::Gap),
            counts(CauseKind::UnreturnedVia),
            counts(CauseKind::PlaneEdge),
            counts(CauseKind::BoardEdge)
        ),
    );
    y += 4.0;
    let map_width = WIDTH - 2.0 * MARGIN;
    match window {
        None => paragraph(
            &mut out,
            &mut y,
            12.0,
            INK,
            "No gap, reference change or edge point is located in the measures of these findings (see their messages for the uncovered length).",
        ),
        Some(w) => {
            let pad = ((w[2] - w[0]).max(w[3] - w[1]) * 0.05).max(2.0);
            let (x0, y0) = (w[0] - pad, w[1] - pad);
            let (wm, hm) = ((w[2] - w[0]) + 2.0 * pad, (w[3] - w[1]) + 2.0 * pad);
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
            // A label beside its mark (left of it near the right edge of the map when it fits there), staggered over three rows.
            for (index, (name, cause)) in marks.iter().enumerate() {
                let (px, py) = to_px(cause.at);
                let label = format!("{name}: {}", cause.text);
                let width = text_px(&label, 11.0);
                // Left of the mark only when the label does not fit on the right and does fit on the left.
                let flip = px + 12.0 + width > MARGIN + pw && px - 12.0 - width >= MARGIN;
                let (anchor, dx) = if flip { (" text-anchor=\"end\"", -12.0) } else { ("", 12.0) };
                let dy = -9.0 + (index % 3) as f64 * 13.0;
                out.push(format!(
                    "<g class=\"return-path-mark\" data-kind=\"{}\" data-subject=\"{}\" data-x-mm=\"{}\" data-y-mm=\"{}\">",
                    cause.kind.name(),
                    escape(name),
                    n(cause.at.0),
                    n(cause.at.1)
                ));
                out.push(marker(cause.kind, px, py));
                out.push(text_element(
                    px + dx,
                    py + dy,
                    11.0,
                    INK,
                    &label,
                    &format!("{anchor} stroke=\"#ffffff\" stroke-width=\"3\" paint-order=\"stroke\""),
                ));
                out.push("</g>".to_owned());
            }
            y += ph + 16.0;
        }
    }
    // Legend.
    let mut x = MARGIN;
    for (kind, label) in [
        (CauseKind::Gap, "gap: slot length and half-wave resonance"),
        (CauseKind::UnreturnedVia, "reference change, no return path within the threshold"),
        (CauseKind::PlaneEdge, "within the edge limit of the reference copper"),
        (CauseKind::BoardEdge, "within the edge limit of the board outline"),
    ] {
        if x + 20.0 + text_px(label, 11.0) > WIDTH - MARGIN {
            x = MARGIN;
            y += 18.0;
        }
        out.push(marker(kind, x + 7.0, y - 4.0));
        out.push(text_element(x + 18.0, y, 11.0, INK, label, ""));
        x += 30.0 + text_px(label, 11.0);
    }
    y += 24.0;
    paragraph(
        &mut out,
        &mut y,
        11.0,
        MUTED,
        "Screening estimate from the layout, not EMC qualification and never a pass or a fail. Not modelled: slot radiation efficiency, plane-pair cavity modes, enclosure and cable coupling, return current outside the coverage window, the other plane of a stripline, and the common-mode part of a differential pair. Each finding lists these in estimate.not_modelled.",
    );
    Some(FieldPicture { content: out.join("\n"), width: WIDTH, height: y + 6.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn the_map_has_a_mark_per_cause_and_none_without_findings() {
        let picture = return_path_picture(&document(), None).expect("a picture");
        assert_eq!(picture.content.matches("class=\"return-path-mark\"").count(), 4);
        assert!(picture.content.contains("never a pass or a fail"));
        let none = copper_json::parse(r#"{"findings":[{"code":"EMI-HOT-LOOP","measures":{}}]}"#).unwrap();
        assert!(return_path_picture(&none, None).is_none());
    }
}

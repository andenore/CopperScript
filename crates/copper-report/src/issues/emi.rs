//! The `emi` stage: the screening findings of the run report's `emi` section
//! (`copper emi-check --run RUN --write`, or `route-board` itself) as located
//! issues. Findings are screening estimates, not EMC qualification, and carry
//! their uncertainty as three separate parts (numerical sensitivity, parameter
//! band, named omissions) in their `estimate`; accepted findings (`emi_accept`) are listed as accepted and
//! count as zero findings.

use copper_json::{Object, Value};

use super::RunIssue;
use crate::scene::{Point, Scene};

/// `Value` as a float (`Int` and `Float`).
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(number) => Some(*number),
        Value::Int(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

/// `loop_polygon_mm` as points; empty unless it is a polygon of at least three vertices.
fn polygon(measures: Option<&Value>) -> Vec<Point> {
    let points: Vec<Point> = measures
        .and_then(|m| m.get("loop_polygon_mm"))
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|vertex| {
            let vertex = vertex.as_array()?;
            Some((number(vertex.first()?)?, number(vertex.get(1)?)?))
        })
        .collect();
    if points.len() >= 3 { points } else { Vec::new() }
}

fn push_unique(points: &mut Vec<Point>, found: impl IntoIterator<Item = Point>) {
    for point in found {
        if !points.contains(&point) {
            points.push(point);
        }
    }
}

/// Pad centres of the named objects, none of a pad of `skip`: a `REF.PAD`, the pads of a component
/// (those on `nets` when it has any), else the pads of a net of that name.
fn object_points(scene: &Scene, objects: &[String], nets: &[String], skip: &str) -> Vec<Point> {
    let mut points = Vec::new();
    for object in objects {
        if let Some((reference, pad)) = object.rsplit_once('.') {
            let pads = scene.pad(reference, pad);
            if !pads.is_empty() {
                push_unique(&mut points, pads.iter().filter(|p| p.reference != skip).map(|p| p.center));
                continue;
            }
        }
        let own: Vec<_> = scene.pads.iter().filter(|p| &p.reference == object && p.reference != skip).collect();
        if own.is_empty() {
            push_unique(
                &mut points,
                scene.pads.iter().filter(|p| &p.net == object && p.reference != skip).map(|p| p.center),
            );
            continue;
        }
        let on_nets: Vec<_> = own.iter().filter(|p| nets.contains(&p.net)).collect();
        if on_nets.is_empty() {
            push_unique(&mut points, own.iter().map(|p| p.center));
        } else {
            push_unique(&mut points, on_nets.iter().map(|p| p.center));
        }
    }
    points
}

/// Where a finding is: its objects' pad centres. A proximity finding is about the closest pair of
/// the source's pads and the victims' pads (those of the cell's own controller aside); a switch-node
/// finding also covers every pad of its nets.
fn locus(code: &str, cell: &str, objects: &[String], nets: &[String], scene: Option<&Scene>) -> Vec<Point> {
    let Some(scene) = scene else { return Vec::new() };
    if code == "EMI-PROXIMITY"
        && let Some((source, victims)) = objects.split_first()
        && !victims.is_empty()
    {
        let a = object_points(scene, std::slice::from_ref(source), nets, "");
        let b = object_points(scene, victims, nets, cell);
        let closest = a
            .iter()
            .flat_map(|p| b.iter().map(move |q| (*p, *q)))
            .min_by(|(p1, q1), (p2, q2)| (p1.0 - q1.0).hypot(p1.1 - q1.1).total_cmp(&(p2.0 - q2.0).hypot(p2.1 - q2.1)));
        if let Some((p, q)) = closest {
            return if p == q { vec![p] } else { vec![p, q] };
        }
    }
    let mut points = object_points(scene, objects, nets, "");
    if code == "EMI-SWITCH-NODE" {
        push_unique(&mut points, scene.pads.iter().filter(|p| nets.contains(&p.net)).map(|p| p.center));
    }
    points
}

/// The measures worth printing next to an issue: the scalars, not the geometry lists.
fn scalar_measures(measures: Option<&Value>) -> Value {
    let mut kept = Object::new();
    if let Some(Value::Object(measures)) = measures {
        for (key, value) in measures {
            if !matches!(value, Value::Array(_) | Value::Object(_)) {
                kept.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(kept)
}

/// The findings of an `emi` section in issue order: grouped by cell (report order inside a cell).
fn ordered_findings(section: &Value) -> Vec<&Value> {
    let Some(findings) = section.get("findings").and_then(Value::as_array) else { return Vec::new() };
    let cells: Vec<String> = section
        .get("cells")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|cell| cell.get("controller").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let cell_of = |finding: &Value| finding.get("cell").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut ordered: Vec<&Value> = findings.iter().filter(|f| matches!(f, Value::Object(_))).collect();
    // Stable: findings keep the report's ranking inside a cell.
    ordered.sort_by_key(|finding| {
        let cell = cell_of(finding);
        cells.iter().position(|known| *known == cell).unwrap_or(cells.len())
    });
    ordered
}

/// The finding behind issue `id` (`M1`, `M2`, ...) of the report's `emi` section.
pub fn emi_finding<'a>(report: &'a Value, id: &str) -> Option<&'a Value> {
    let index: usize = id.strip_prefix('M')?.parse().ok()?;
    ordered_findings(report.get("emi")?).get(index.checked_sub(1)?).copied()
}

/// One issue per finding of the report's `emi` section, grouped by cell (report order inside a
/// cell), ids `M1`, `M2`, ... in that order.
pub(super) fn emi_issues(report: &Value, scene: Option<&Scene>) -> Vec<RunIssue> {
    let Some(section) = report.get("emi") else { return Vec::new() };
    let ordered = ordered_findings(section);
    let cell_of = |finding: &Value| finding.get("cell").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut issues = Vec::new();
    for (index, finding) in ordered.into_iter().enumerate() {
        let text = |key: &str| finding.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let (code, message, cell) = (text("code"), text("message"), cell_of(finding));
        let (objects, nets, layers) =
            (strings(finding.get("objects")), strings(finding.get("nets")), strings(finding.get("layers")));
        let accepted = finding.get("accepted").and_then(|a| a.get("reason")).and_then(Value::as_str);
        let measures = finding.get("measures");
        let outline = if code == "EMI-HOT-LOOP" { polygon(measures) } else { Vec::new() };
        let points = if outline.is_empty() { locus(&code, &cell, &objects, &nets, scene) } else { outline.clone() };
        // The message names its cell ("U1: ..."); say it only when it does not.
        let cell_label = if cell.is_empty() || message.starts_with(&format!("{cell}:")) {
            String::new()
        } else {
            format!(" [{cell}]")
        };
        let title = match accepted {
            Some(reason) => format!("ACCEPTED ({reason}) screening estimate{cell_label}: {message}"),
            None => format!("screening estimate{cell_label}: {message}"),
        };
        let mut details = Object::new();
        details.insert(
            "evidence".to_owned(),
            Value::from(
                "screening estimate from the placed or routed layout; see estimate.not_modelled for what it leaves out, not EMC qualification",
            ),
        );
        details.insert("source".to_owned(), Value::from("route-report.json emi"));
        details.insert("cell".to_owned(), Value::from(cell.as_str()));
        details.insert("accepted".to_owned(), accepted.map_or(Value::Null, Value::from));
        details.insert("tags".to_owned(), finding.get("tags").cloned().unwrap_or(Value::Array(Vec::new())));
        details.insert("estimate".to_owned(), finding.get("estimate").cloned().unwrap_or(Value::Null));
        details.insert("margin_db".to_owned(), finding.get("margin_db").cloned().unwrap_or(Value::Null));
        details.insert("measures".to_owned(), scalar_measures(measures));
        let mut issue = RunIssue::new(format!("M{}", index + 1), "emi", code.as_str(), title)
            .count(i64::from(accepted.is_none()))
            .nets(nets.iter().map(|n| Value::from(n.as_str())).collect())
            .layers(layers.iter().map(|l| Value::from(l.as_str())).collect())
            .objects(objects)
            .points(points)
            .details(Value::Object(details));
        issue.outline = outline;
        issues.push(issue);
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ScenePad;

    fn pad(reference: &str, number: &str, net: &str, x: f64, y: f64) -> ScenePad {
        ScenePad {
            reference: reference.to_owned(),
            pad: number.to_owned(),
            net: net.to_owned(),
            layers: vec!["F.Cu".to_owned()],
            polygon: vec![(x - 0.2, y - 0.2), (x + 0.2, y - 0.2), (x + 0.2, y + 0.2)],
            center: (x, y),
            drill: 0.0,
        }
    }

    fn scene() -> Scene {
        Scene {
            copper_layers: vec!["F.Cu".to_owned()],
            pads: vec![
                pad("U1", "3", "SW", 10.0, 10.0),
                pad("U1", "4", "FB", 11.0, 10.0),
                pad("L1", "1", "SW", 12.0, 10.0),
                pad("L1", "2", "VOUT", 14.0, 10.0),
                pad("R1", "2", "FB", 15.0, 10.0),
                pad("R2", "1", "FB", 40.0, 40.0),
            ],
            ..Scene::default()
        }
    }

    fn report() -> Value {
        copper_json::parse(
            r#"{"emi":{"schema":"copperscript-emi-check/v0.1","cells":[{"controller":"U1"},{"controller":"U2"}],
            "findings":[
              {"code":"EMI-SWITCH-NODE","cell":"U2","message":"U2: switch net SW2","objects":["U2"],"nets":["SW2"],"layers":[],"tags":["pads_only"],"measures":{}},
              {"code":"EMI-HOT-LOOP","cell":"U1","message":"U1: hot loop","objects":["U1","C1"],"nets":["VIN","GND"],"layers":["F.Cu"],
               "tags":["traced_copper"],"estimate":{"value":8.5},"margin_db":21.5,"measures":{"loop_area_mm2":2.5,"loop_polygon_mm":[[1,1],[3,1],[3,2.5]]}},
              {"code":"EMI-PROXIMITY","cell":"U1","message":"U1: L1 is close to FB","objects":["L1","FB"],"nets":["FB"],"layers":[],"tags":[],
               "accepted":{"reason":"layout reviewed"},"measures":{"distance_mm":2.0}}
            ]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn findings_become_issues_grouped_by_cell_with_stable_ids() {
        let issues = emi_issues(&report(), Some(&scene()));
        let summary: Vec<(&str, &str, &str)> =
            issues.iter().map(|i| (i.id.as_str(), i.stage, i.code.as_str().unwrap())).collect();
        assert_eq!(
            summary,
            [("M1", "emi", "EMI-HOT-LOOP"), ("M2", "emi", "EMI-PROXIMITY"), ("M3", "emi", "EMI-SWITCH-NODE")]
        );
        // The hot loop: its polygon is the outline and frames the window.
        let hot = &issues[0];
        assert_eq!(hot.outline, [(1.0, 1.0), (3.0, 1.0), (3.0, 2.5)]);
        assert_eq!(hot.points, hot.outline);
        assert_eq!(hot.window(), Some([-0.5, -0.5, 4.5, 4.0]));
        assert_eq!(hot.count, Value::from(1i64));
        assert_eq!(hot.layers, [Value::from("F.Cu")]);
        let details = hot.details.get("measures").unwrap();
        assert_eq!(details.get("loop_area_mm2"), Some(&Value::Float(2.5)));
        assert!(details.get("loop_polygon_mm").is_none(), "geometry lists stay out of the details");
        assert!(hot.title.as_str().unwrap().starts_with("screening estimate: U1: hot loop"));
        // Accepted: listed, labelled, not counted.
        let accepted = &issues[1];
        assert_eq!(accepted.count, Value::from(0i64));
        assert!(accepted.title.as_str().unwrap().starts_with("ACCEPTED (layout reviewed) screening estimate"));
        assert_eq!(accepted.details.get("accepted"), Some(&Value::from("layout reviewed")));
        // Proximity is located at the closest pair of source and victim pads, not at every victim pad.
        assert_eq!(accepted.points, [(14.0, 10.0), (15.0, 10.0)]);
        assert!(accepted.outline.is_empty());
        // A finding whose objects are not on the board has no position.
        assert!(issues[2].points.is_empty() && issues[2].window().is_none());
    }

    #[test]
    fn a_switch_node_covers_the_pads_of_its_net_and_without_a_scene_nothing_is_located() {
        let finding = |code: &str, objects: &str, nets: &str| {
            format!(
                r#"{{"emi":{{"cells":[{{"controller":"U1"}}],"findings":[{{"code":"{code}","cell":"U1","message":"m","objects":{objects},"nets":{nets},"measures":{{}}}}]}}}}"#
            )
        };
        let report = copper_json::parse(&finding("EMI-SWITCH-NODE", r#"["U1"]"#, r#"["SW"]"#)).unwrap();
        let issues = emi_issues(&report, Some(&scene()));
        assert_eq!(issues[0].points, [(10.0, 10.0), (12.0, 10.0)]);
        assert_eq!(issues[0].title.as_str(), Some("screening estimate [U1]: m"));
        assert!(emi_issues(&report, None)[0].points.is_empty());
        // A hot loop without a usable polygon falls back to its objects' pads on its nets.
        let report = copper_json::parse(&finding("EMI-HOT-LOOP", r#"["U1","L1"]"#, r#"["SW"]"#)).unwrap();
        let issues = emi_issues(&report, Some(&scene()));
        assert!(issues[0].outline.is_empty());
        assert_eq!(issues[0].points, [(10.0, 10.0), (12.0, 10.0)]);
    }

    #[test]
    fn reports_without_findings_or_with_an_error_record_have_no_emi_issues() {
        let none = copper_json::parse(r#"{"status":"pass"}"#).unwrap();
        assert!(emi_issues(&none, Some(&scene())).is_empty());
        let error = copper_json::parse(r#"{"emi":{"error":"no view","evidence_grade":"screening"}}"#).unwrap();
        assert!(emi_issues(&error, None).is_empty());
        let empty = copper_json::parse(r#"{"emi":{"cells":[],"findings":[]}}"#).unwrap();
        assert!(emi_issues(&empty, None).is_empty());
    }
}

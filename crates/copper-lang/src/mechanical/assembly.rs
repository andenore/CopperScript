//! Component-owned mechanical exceptions (`pcbir.mechanical_assembly`).

use copper_physical::geometry::ring_edges;
use copper_physical::mechanical::signed_area_twice;
use copper_physical::{BoardEdge, BoardOutline, BoardSide, Point};

use super::{Fail, Lower, board_side, fail, length, point, strip};
use crate::pyvalue;
use crate::syntax::{MechanicalItemDecl, Scalar, Value};

/// `BodyOverhang`: an audited body allowance past a straight board edge.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyOverhang {
    pub id: String,
    pub reference: String,
    pub edge: String,
    pub start_nm: i64,
    pub end_nm: i64,
    pub distance_nm: i64,
    pub reason: String,
}

/// `ComponentHeight`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentHeight {
    pub id: String,
    pub reference: Value,
    pub height_nm: i64,
}

/// `AssemblyEnvelope`: an enclosure height limit over a region.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyEnvelope {
    pub id: String,
    pub outline: BoardOutline,
    pub side: BoardSide,
    pub maximum_height_nm: i64,
}

/// `AssemblyAccess`: a component-relative region kept clear for assembly.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyAccess {
    pub id: String,
    pub reference: Value,
    pub outline: BoardOutline,
    pub side: String,
    pub purpose: String,
}

#[derive(Default)]
pub struct Assembly {
    pub overhangs: Vec<BodyOverhang>,
    pub heights: Vec<ComponentHeight>,
    pub enclosures: Vec<AssemblyEnvelope>,
    pub access: Vec<AssemblyAccess>,
}

/// `expanded_body_outline`: the outline with an exterior allowance on one
/// named axis-aligned edge; only body containment queries see it.
pub fn expanded_body_outline(
    outline: &BoardOutline,
    edges: &[BoardEdge],
    allowance: &BodyOverhang,
) -> Lower<BoardOutline> {
    let Some(edge) = edges.iter().find(|edge| edge.id == allowance.edge) else {
        return fail("overhang requires an existing named straight boundary edge");
    };
    if outline.circular_boundary.is_some() {
        return fail("overhang requires an existing named straight boundary edge");
    }
    let (dx, dy) = (edge.end.x_nm - edge.start.x_nm, edge.end.y_nm - edge.start.y_nm);
    if dx != 0 && dy != 0 {
        return fail("body overhang currently requires an axis-aligned edge");
    }
    let length = dx.abs() + dy.abs();
    if allowance.end_nm > length {
        return fail("body overhang interval exceeds its edge");
    }
    let pairs = ring_edges(&outline.vertices);
    if !pairs.contains(&(edge.start, edge.end)) && !pairs.contains(&(edge.end, edge.start)) {
        return fail("body overhang edge was removed from the outline");
    }
    let forward = pairs.contains(&(edge.start, edge.end));
    let winding = if signed_area_twice(&outline.vertices) > 0 { 1 } else { -1 };
    let (ux, uy) = (dx.div_euclid(length), dy.div_euclid(length));
    let outward = winding * if forward { 1 } else { -1 };
    let normal = Point::new(uy * outward * allowance.distance_nm, -ux * outward * allowance.distance_nm);
    let a = Point::new(edge.start.x_nm + ux * allowance.start_nm, edge.start.y_nm + uy * allowance.start_nm);
    let b = Point::new(edge.start.x_nm + ux * allowance.end_nm, edge.start.y_nm + uy * allowance.end_nm);
    let mut bump = [
        a,
        Point::new(a.x_nm + normal.x_nm, a.y_nm + normal.y_nm),
        Point::new(b.x_nm + normal.x_nm, b.y_nm + normal.y_nm),
        b,
    ];
    if !forward {
        bump.reverse();
    }
    let mut expanded = Vec::new();
    for &(start, end) in &pairs {
        expanded.push(start);
        if (start == edge.start && end == edge.end) || (start == edge.end && end == edge.start) {
            expanded.extend(bump.iter().copied());
        }
    }
    let mut simple: Vec<Point> = Vec::new();
    for p in expanded {
        if simple.last() != Some(&p) {
            simple.push(p);
        }
    }
    if simple.len() > 1 && simple.last() == simple.first() {
        simple.pop();
    }
    loop {
        let count = simple.len();
        let collinear = (0..count).find(|&i| {
            let (a, p, b) = (simple[(i + count - 1) % count], simple[i], simple[(i + 1) % count]);
            i128::from(p.x_nm - a.x_nm) * i128::from(b.y_nm - p.y_nm)
                == i128::from(p.y_nm - a.y_nm) * i128::from(b.x_nm - p.x_nm)
        });
        match collinear {
            Some(i) => {
                simple.remove(i);
            }
            None => break,
        }
    }
    Ok(BoardOutline::new(simple, outline.cutouts.clone(), None, None)?)
}

/// Python's `tuple(point(v) for v in value)`: iterate the value as Python would.
fn iterated_points(value: &Value) -> Lower<Vec<Point>> {
    match value {
        Value::Points(points) => points.iter().map(|(x, y)| point(&Value::Point(x.clone(), y.clone()))).collect(),
        Value::Point(..) => fail("expected a coordinate pair (x, y)"),
        Value::Scalar(Scalar::Str(text)) if text.is_empty() => Ok(Vec::new()),
        Value::Scalar(Scalar::Str(_)) => fail("expected a coordinate pair (x, y)"),
        other => fail(format!("'{}' object is not iterable", pyvalue::type_name(other))),
    }
}

fn identity(value: &Value) -> Option<String> {
    match value {
        Value::Scalar(Scalar::Str(text)) if !strip(text).is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn exact_keys(parameters: &indexmap::IndexMap<String, Value>, keys: &[&str]) -> bool {
    parameters.len() == keys.len() && keys.iter().all(|key| parameters.contains_key(*key))
}

/// `lower_assembly`.
pub fn lower_assembly(items: &[&MechanicalItemDecl], outline: &BoardOutline, edges: &[BoardEdge]) -> Lower<Assembly> {
    let mut assembly = Assembly::default();
    for item in items {
        let p = &item.parameters;
        match item.kind.as_str() {
            "overhang" => {
                if !exact_keys(p, &["component", "edge", "start", "end", "distance", "reason"]) {
                    return fail("overhang requires exactly component, edge, start, end, distance and reason");
                }
                let (start, end, distance) = (length(&p["start"])?, length(&p["end"])?, length(&p["distance"])?);
                let identities = (identity(&p["component"]), identity(&p["edge"]), identity(&p["reason"]));
                let (Some(reference), Some(edge), Some(reason), false) =
                    (identities.0, identities.1, identities.2, strip(&item.name).is_empty())
                else {
                    return fail("body overhang requires identities and an explicit audit reason");
                };
                if !(0 <= start && start < end) || distance <= 0 {
                    return fail("body overhang requires a positive depth and ordered nonnegative edge interval");
                }
                let policy = BodyOverhang {
                    id: item.name.clone(),
                    reference,
                    edge,
                    start_nm: start,
                    end_nm: end,
                    distance_nm: distance,
                    reason,
                };
                expanded_body_outline(outline, edges, &policy)?;
                assembly.overhangs.push(policy);
            }
            "component_height" => {
                if !exact_keys(p, &["component", "height"]) {
                    return fail("component_height requires component and height");
                }
                let height = length(&p["height"])?;
                if item.name.is_empty() || !pyvalue::truthy(&p["component"]) || height <= 0 {
                    return fail("component height requires identities and a positive typed length");
                }
                assembly.heights.push(ComponentHeight {
                    id: item.name.clone(),
                    reference: p["component"].clone(),
                    height_nm: height,
                });
            }
            kind => {
                let keys: &[&str] =
                    if kind == "enclosure" { &["side", "maximum_height"] } else { &["component", "side", "purpose"] };
                let region = match item.shape.as_str() {
                    "rectangle" => {
                        let required: Vec<&str> = keys.iter().copied().chain(["width", "height"]).collect();
                        let unknown = p.keys().any(|key| !required.contains(&key.as_str()) && key != "origin");
                        if unknown || !required.iter().all(|key| p.contains_key(*key)) {
                            return fail("assembly rectangle has missing or unknown properties");
                        }
                        let origin = p.get("origin").map(point).transpose()?.unwrap_or(Point::new(0, 0));
                        let (width, height) = (length(&p["width"])?, length(&p["height"])?);
                        BoardOutline::rectangle(width, height, Some(origin))?
                    }
                    "polygon" => {
                        let required: Vec<&str> = keys.iter().copied().chain(["vertices"]).collect();
                        if !exact_keys(p, &required) {
                            return fail("assembly polygon has missing or unknown properties");
                        }
                        BoardOutline::polygon(iterated_points(&p["vertices"])?)?
                    }
                    _ => return fail("assembly regions require rectangle or polygon geometry"),
                };
                if kind == "enclosure" {
                    let side = board_side(&p["side"])?;
                    let height = length(&p["maximum_height"])?;
                    if item.name.is_empty() || height < 0 {
                        return fail("enclosure requires an id, side and nonnegative typed height");
                    }
                    assembly.enclosures.push(AssemblyEnvelope {
                        id: item.name.clone(),
                        outline: region,
                        side,
                        maximum_height_nm: height,
                    });
                } else {
                    let side = pyvalue::as_str(&p["side"]).filter(|side| ["component", "opposite"].contains(side));
                    let purpose = pyvalue::as_str(&p["purpose"]).filter(|purpose| !strip(purpose).is_empty());
                    let (Some(side), Some(purpose), true, false) =
                        (side, purpose, pyvalue::truthy(&p["component"]), item.name.is_empty())
                    else {
                        return fail("assembly access requires an owner, relative side and explicit purpose");
                    };
                    assembly.access.push(AssemblyAccess {
                        id: item.name.clone(),
                        reference: p["component"].clone(),
                        outline: region,
                        side: side.to_owned(),
                        purpose: purpose.to_owned(),
                    });
                }
            }
        }
    }
    let overhang_references: Vec<Value> =
        assembly.overhangs.iter().map(|o| Value::Scalar(Scalar::Str(o.reference.clone()))).collect();
    let conflicting = pyvalue::distinct_count(&overhang_references) != assembly.overhangs.len()
        || pyvalue::distinct_count(assembly.heights.iter().map(|h| &h.reference)) != assembly.heights.len()
        || pyvalue::distinct_count(assembly.access.iter().map(|a| &a.reference)) != assembly.access.len();
    if conflicting {
        return Err(Fail::Value("conflicting assembly component ownership".to_owned()));
    }
    Ok(assembly)
}

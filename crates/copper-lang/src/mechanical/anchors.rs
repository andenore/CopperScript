//! Named board frames and source-owned attachments (`pcbir.mechanical_anchors`).

use std::cmp::Ordering;

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};

use copper_json::python_str_repr as repr;
use copper_physical::mechanical::{boundary_line_pairs, signed_area_twice};
use copper_physical::{BoardDatum, BoardEdge, BoardOutline, BoardSide, Point};
use copper_units::Decimal;

use super::{Fail, Lower, board_side, fail, point};
use crate::pyvalue;
use crate::syntax::{Location, MechanicalItemDecl, Scalar, Value};

/// `MechanicalAttachment`: a component pose fixed to a datum, edge or point.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalAttachment {
    pub id: String,
    pub reference: String,
    pub target: Option<String>,
    pub position: Point,
    pub offset: Point,
    pub anchor: String,
    pub anchor_pad: Option<String>,
    pub anchor_point: Option<Point>,
    pub rotation: Decimal,
    pub side: BoardSide,
    pub location: Location,
}

fn keys_within(parameters: &IndexMap<String, Value>, allowed: &[&str]) -> bool {
    parameters.keys().all(|key| allowed.contains(&key.as_str()))
}

/// `resolve_datums`: absolute and relative datums, resolved in name order.
pub fn resolve_datums(items: &[&MechanicalItemDecl]) -> Lower<Vec<BoardDatum>> {
    let nodes: IndexMap<&str, &MechanicalItemDecl> = items.iter().map(|item| (item.name.as_str(), *item)).collect();
    let mut result: IndexMap<String, BoardDatum> = IndexMap::new();
    let mut visiting: Vec<String> = Vec::new();

    fn resolve(
        name: &str,
        nodes: &IndexMap<&str, &MechanicalItemDecl>,
        result: &mut IndexMap<String, BoardDatum>,
        visiting: &mut Vec<String>,
    ) -> Lower<BoardDatum> {
        if let Some(datum) = result.get(name) {
            return Ok(datum.clone());
        }
        let Some(node) = nodes.get(name) else { return fail(format!("unknown datum {}", repr(name))) };
        if visiting.iter().any(|entry| entry == name) {
            let mut cycle = visiting.clone();
            cycle.push(name.to_owned());
            return fail(format!("cyclic datum dependency: {}", cycle.join(" -> ")));
        }
        visiting.push(name.to_owned());
        let p = &node.parameters;
        if !keys_within(p, &["position", "relative_to", "offset"])
            || p.contains_key("position") == p.contains_key("relative_to")
        {
            return fail("datum requires exactly position or relative_to/offset");
        }
        let datum = if let Some(position) = p.get("position") {
            if p.contains_key("offset") {
                return fail("absolute datum cannot also have an offset");
            }
            BoardDatum { id: name.to_owned(), position: point(position)?, relative_to: None, offset: Point::new(0, 0) }
        } else {
            let (Some(Value::Scalar(Scalar::Str(parent))), true) = (p.get("relative_to"), p.contains_key("offset"))
            else {
                return fail("relative datum requires a named parent and offset");
            };
            let parent = resolve(parent, nodes, result, visiting)?;
            let offset = point(&p["offset"])?;
            BoardDatum {
                id: name.to_owned(),
                position: Point::new(parent.position.x_nm + offset.x_nm, parent.position.y_nm + offset.y_nm),
                relative_to: Some(parent.id.clone()),
                offset,
            }
        };
        visiting.pop();
        result.insert(name.to_owned(), datum.clone());
        Ok(datum)
    }

    let mut names: Vec<&str> = nodes.keys().copied().collect();
    names.sort_unstable();
    names.into_iter().map(|name| resolve(name, &nodes, &mut result, &mut visiting)).collect()
}

/// `resolve_edges`: named edges that are actual outer boundary segments.
pub fn resolve_edges(items: &[&MechanicalItemDecl], outline: &BoardOutline) -> Lower<Vec<BoardEdge>> {
    if !items.is_empty() && outline.circular_boundary.is_some() {
        return fail("straight named edges require a polygon outline; circular arc edges are not sampled");
    }
    let pairs = boundary_line_pairs(outline);
    let mut result = Vec::new();
    for item in items {
        let p = &item.parameters;
        if p.len() != 2 || !p.contains_key("start") || !p.contains_key("end") {
            return fail("named edge requires exactly start and end");
        }
        let (start, end) = (point(&p["start"])?, point(&p["end"])?);
        let edge = BoardEdge::new(&item.name, start, end)?;
        if !pairs.contains(&(edge.start, edge.end)) && !pairs.contains(&(edge.end, edge.start)) {
            return fail(format!("named edge {} is not an actual outer boundary segment", repr(&edge.id)));
        }
        result.push(edge);
    }
    Ok(result)
}

/// Compare `numerator / sqrt(length_squared)` with `value`, exactly.
fn compare_scaled(numerator: &BigInt, length_squared: &BigInt, value: &num_rational::BigRational) -> Ordering {
    let left_sign = numerator.signum();
    let right_sign = if value.is_positive() {
        BigInt::from(1)
    } else if value.is_negative() {
        BigInt::from(-1)
    } else {
        BigInt::zero()
    };
    if left_sign != right_sign {
        return left_sign.cmp(&right_sign);
    }
    // Same sign: compare squares, numerator^2 * denominator^2 against
    // value_numerator^2 * length_squared, reversing for negatives.
    let left = numerator * numerator * value.denom() * value.denom();
    let right = value.numer() * value.numer() * length_squared;
    let ordering = left.cmp(&right);
    if left_sign.is_negative() { ordering.reverse() } else { ordering }
}

/// `int((start + numerator / sqrt(length_squared)).to_integral_value(ROUND_HALF_EVEN))`.
///
/// Python computes it with 60-digit decimals, which are exact or far from a
/// tie for every board coordinate; this evaluates it exactly.
fn round_offset(start: i64, numerator: &BigInt, length_squared: &BigInt) -> i64 {
    use num_rational::BigRational;
    let estimate = numerator.to_f64().unwrap_or(0.0) / length_squared.to_f64().unwrap_or(1.0).sqrt();
    let mut floor = BigInt::from(estimate.floor() as i64);
    let at = |k: &BigInt| BigRational::from_integer(k.clone());
    while compare_scaled(numerator, length_squared, &at(&floor)) == Ordering::Less {
        floor -= 1;
    }
    while compare_scaled(numerator, length_squared, &at(&(&floor + 1))) != Ordering::Less {
        floor += 1;
    }
    let half = at(&floor) + BigRational::new(BigInt::from(1), BigInt::from(2));
    let base = BigInt::from(start) + &floor;
    let rounded = match compare_scaled(numerator, length_squared, &half) {
        Ordering::Less => base,
        Ordering::Greater => base + 1,
        Ordering::Equal => {
            if (&base % 2u8).is_zero() {
                base
            } else {
                base + 1
            }
        }
    };
    rounded.to_i64().expect("board coordinates fit in i64")
}

/// `resolve_attachments`.
pub fn resolve_attachments(
    items: &[&MechanicalItemDecl],
    datums: &[BoardDatum],
    edges: &[BoardEdge],
    outline: &BoardOutline,
) -> Lower<Vec<MechanicalAttachment>> {
    let datums: IndexMap<&str, &BoardDatum> = datums.iter().map(|d| (d.id.as_str(), d)).collect();
    let edges: IndexMap<&str, &BoardEdge> = edges.iter().map(|e| (e.id.as_str(), e)).collect();
    let pairs = boundary_line_pairs(outline);
    let winding: i64 = if signed_area_twice(&outline.vertices) > 0 { 1 } else { -1 };
    let mut result = Vec::new();
    let mut references: IndexSet<String> = IndexSet::new();
    let allowed =
        ["component", "target", "position", "offset", "anchor", "anchor_pad", "anchor_point", "rotation", "side"];
    for item in items {
        let p = &item.parameters;
        if !keys_within(p, &allowed)
            || !["component", "rotation", "side"].iter().all(|key| p.contains_key(*key))
            || p.contains_key("target") == p.contains_key("position")
        {
            return fail("attachment requires component, rotation, side and exactly target or position");
        }
        let reference = match &p["component"] {
            Value::Scalar(Scalar::Str(text)) if !text.is_empty() && !references.contains(text) => text.clone(),
            _ => return fail("attachment component must be unique and explicitly named"),
        };
        references.insert(reference.clone());
        let offset = p.get("offset").map(point).transpose()?.unwrap_or(Point::new(0, 0));
        let target = p.get("target");
        let position = match target {
            None => {
                if p.contains_key("offset") {
                    return fail("numeric attachment uses position directly, not an additional offset");
                }
                point(&p["position"])?
            }
            Some(target) => {
                let name = pyvalue::as_str(target);
                if let Some(datum) = name.and_then(|name| datums.get(name)) {
                    Point::new(datum.position.x_nm + offset.x_nm, datum.position.y_nm + offset.y_nm)
                } else if let Some(edge) = name.and_then(|name| edges.get(name)) {
                    let (dx, dy) = (edge.end.x_nm - edge.start.x_nm, edge.end.y_nm - edge.start.y_nm);
                    let inward = winding * if pairs.contains(&(edge.start, edge.end)) { 1 } else { -1 };
                    let length_squared = BigInt::from(dx) * dx + BigInt::from(dy) * dy;
                    let along = BigInt::from(offset.x_nm);
                    if along.is_negative() || &along * &along > length_squared {
                        return fail("edge attachment along-distance lies outside the segment");
                    }
                    let (ox, oy) = (BigInt::from(offset.x_nm), BigInt::from(offset.y_nm));
                    let x = BigInt::from(dx) * &ox - BigInt::from(dy) * &oy * inward;
                    let y = BigInt::from(dy) * &ox + BigInt::from(dx) * &oy * inward;
                    Point::new(
                        round_offset(edge.start.x_nm, &x, &length_squared),
                        round_offset(edge.start.y_nm, &y, &length_squared),
                    )
                } else {
                    return fail(format!("unknown attachment target {}", pyvalue::repr(target)));
                }
            }
        };
        let anchor = match p.get("anchor") {
            None => "origin".to_owned(),
            Some(Value::Scalar(Scalar::Str(text))) if ["origin", "pad", "mating_face"].contains(&text.as_str()) => {
                text.clone()
            }
            Some(_) => return fail("anchor must be origin, pad or mating_face"),
        };
        if (anchor == "pad") != p.contains_key("anchor_pad")
            || (anchor == "mating_face") != p.contains_key("anchor_point")
        {
            return fail("pad/mating_face anchors require exactly their corresponding anchor_pad/anchor_point");
        }
        let pad = p.get("anchor_pad").map(pyvalue::text);
        if let Some(value) = p.get("anchor_pad") {
            let valid = matches!(value, Value::Scalar(Scalar::Str(_) | Scalar::Int(_)))
                && pad.as_deref().is_some_and(|pad| !pad.is_empty());
            if !valid {
                return fail("anchor_pad must identify a physical pad");
            }
        }
        let rotation = match &p["rotation"] {
            Value::Scalar(scalar @ (Scalar::Int(_) | Scalar::Float(_))) => pyvalue::decimal(scalar),
            _ => None,
        };
        let Some(rotation) = rotation else { return fail("attachment rotation requires finite numeric degrees") };
        let anchor_point = p.get("anchor_point").map(point).transpose()?;
        let full = Decimal::from_i64(360);
        let rotation = rotation
            .rem(&full)
            .and_then(|value| value.add(&full))
            .and_then(|value| value.rem(&full))
            .map_err(|failure| Fail::Rust(failure.to_string()))?;
        let side = board_side(&p["side"])?;
        result.push(MechanicalAttachment {
            id: item.name.clone(),
            reference,
            target: target.map(pyvalue::text),
            position,
            offset,
            anchor,
            anchor_pad: pad,
            anchor_point,
            rotation,
            side,
            location: item.location.clone(),
        });
    }
    Ok(result)
}

//! `pcbir.editor.artwork`: read-only artwork geometry derived from the
//! accepted physical IR.

use std::f64::consts::PI;

use copper_json::Value;
use copper_lang::silkscreen::{Artwork, Graphic};
use copper_pcb::board::Placement;
use copper_pcb::transform::transformed_local_point;
use copper_physical::curves::arc_circle;
use copper_physical::footprint::{FootprintGraphic, FootprintLayer, PhysicalFootprint};
use copper_physical::pyfloat::{float_mod, ratio_to_f64};
use copper_physical::{BoardSide, Point};
use num_bigint::BigInt;
use num_rational::BigRational;

use crate::{Result, object, point_value};

/// The shape of one graphic, whatever its owner.
enum Shape<'a> {
    Line(Point, Point),
    Rectangle(Point, Point),
    Circle(Point, Point),
    Arc(Point, Point, Point),
    Polygon(&'a [Point]),
}

fn graphic_scene(shape: Shape, width: i64, filled: bool, transform: &dyn Fn(Point) -> Point) -> Result<Value> {
    let point = |p: Point| point_value(transform(p));
    let mut entries = vec![("width_nm", Value::from(width)), ("filled", Value::from(filled))];
    match shape {
        Shape::Line(start, end) => {
            entries.extend([("kind", Value::from("line")), ("start", point(start)), ("end", point(end))]);
        }
        Shape::Rectangle(a, b) => {
            let corners = [a, Point::new(b.x_nm, a.y_nm), b, Point::new(a.x_nm, b.y_nm)];
            entries.extend([
                ("kind", Value::from("polygon")),
                ("vertices", Value::Array(corners.into_iter().map(point).collect())),
            ]);
        }
        Shape::Circle(center, end) => {
            entries.extend([("kind", Value::from("circle")), ("center", point(center)), ("end", point(end))]);
        }
        Shape::Polygon(points) => {
            entries.extend([
                ("kind", Value::from("polygon")),
                ("vertices", Value::Array(points.iter().map(|p| point(*p)).collect())),
            ]);
        }
        Shape::Arc(start, mid, end) => {
            let (a, m, b) = (transform(start), transform(mid), transform(end));
            let (cx, cy, r2) = arc_circle(a, m, b).map_err(crate::Error::Value)?;
            let integer = |value: i64| BigRational::from_integer(BigInt::from(value));
            let angle = |p: Point| ratio_to_f64(&(integer(p.y_nm) - &cy)).atan2(ratio_to_f64(&(integer(p.x_nm) - &cx)));
            let angles = [angle(a), angle(m), angle(b)];
            let mut sweep = float_mod(angles[2] - angles[0], 2.0 * PI);
            if float_mod(angles[1] - angles[0], 2.0 * PI) > sweep {
                sweep -= 2.0 * PI;
            }
            entries.extend([
                ("kind", Value::from("arc")),
                ("start", point_value(a)),
                ("mid", point_value(m)),
                ("end", point_value(b)),
                ("radius_nm", Value::from(ratio_to_f64(&r2).sqrt())),
                ("large_arc", Value::from(sweep.abs() > PI + 1e-12)),
                ("sweep", Value::from(sweep > 0.0)),
            ]);
        }
    }
    Ok(object(entries))
}

fn footprint_graphic_scene(graphic: &FootprintGraphic, transform: &dyn Fn(Point) -> Point) -> Result<Value> {
    match graphic {
        FootprintGraphic::Line { start, end, width_nm, .. } => {
            graphic_scene(Shape::Line(*start, *end), *width_nm, false, transform)
        }
        FootprintGraphic::Rectangle { start, end, width_nm, filled, .. } => {
            graphic_scene(Shape::Rectangle(*start, *end), *width_nm, *filled, transform)
        }
        FootprintGraphic::Circle { center, end, width_nm, filled, .. } => {
            graphic_scene(Shape::Circle(*center, *end), *width_nm, *filled, transform)
        }
        FootprintGraphic::Arc { start, midpoint, end, width_nm, .. } => {
            graphic_scene(Shape::Arc(*start, *midpoint, *end), *width_nm, false, transform)
        }
        FootprintGraphic::Polygon { points, width_nm, filled, .. } => {
            graphic_scene(Shape::Polygon(points), *width_nm, *filled, transform)
        }
    }
}

fn artwork_graphic_scene(graphic: &Graphic) -> Result<Value> {
    let identity = |p: Point| p;
    match graphic {
        Graphic::Line { start, end, width_nm } => graphic_scene(Shape::Line(*start, *end), *width_nm, false, &identity),
        Graphic::Rectangle { start, end, width_nm, filled } => {
            graphic_scene(Shape::Rectangle(*start, *end), *width_nm, *filled, &identity)
        }
        Graphic::Circle { center, end, width_nm, filled } => {
            graphic_scene(Shape::Circle(*center, *end), *width_nm, *filled, &identity)
        }
        Graphic::Arc { start, midpoint, end, width_nm } => {
            graphic_scene(Shape::Arc(*start, *midpoint, *end), *width_nm, false, &identity)
        }
        Graphic::Polygon { points, width_nm, filled } => {
            graphic_scene(Shape::Polygon(points), *width_nm, *filled, &identity)
        }
    }
}

/// `board_artwork_scene`.
pub fn board_artwork_scene(artwork: &[Artwork]) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for item in artwork {
        let (id, side, material, exposed) = match item {
            Artwork::Text { id, side, material, exposed, .. }
            | Artwork::Graphic { id, side, material, exposed, .. } => (id, *side, *material, *exposed),
        };
        let layer = format!(
            "{}{}",
            if side == BoardSide::Front { "F" } else { "B" },
            if material.value() == "copper" { ".Cu" } else { ".SilkS" }
        );
        let mut entries = vec![
            ("id", Value::from(id.as_str())),
            ("side", Value::from(side.value())),
            ("material", Value::from(material.value())),
            ("exposed", Value::from(exposed)),
            ("branding", Value::from(id.starts_with("__copperscript:"))),
            ("layer", Value::from(layer)),
        ];
        let entry = match item {
            Artwork::Text { text, position, size_nm, thickness_nm, rotation_degrees, .. } => {
                entries.extend([
                    ("kind", Value::from("text")),
                    ("text", Value::from(text.as_str())),
                    ("position", point_value(*position)),
                    ("size_nm", Value::from(*size_nm)),
                    ("width_nm", Value::from(*thickness_nm)),
                    ("rotation", Value::from(rotation_degrees.to_sci_string())),
                    ("mirrored", Value::from(side == BoardSide::Back)),
                ]);
                object(entries)
            }
            Artwork::Graphic { graphic, .. } => {
                let mut entry = object(entries);
                if let (Value::Object(target), Value::Object(shape)) = (&mut entry, artwork_graphic_scene(graphic)?) {
                    target.extend(shape);
                }
                entry
            }
        };
        result.push(entry);
    }
    Ok(result)
}

/// `footprint_silkscreen_scene`: the silkscreen graphics of one placed footprint.
pub fn footprint_silkscreen_scene(footprint: &PhysicalFootprint, pose: &Placement) -> Result<Vec<Value>> {
    let transform = |p: Point| transformed_local_point(pose, p);
    let mut result = Vec::new();
    for (index, graphic) in footprint.graphics.iter().enumerate() {
        if graphic.layer() != FootprintLayer::Silkscreen {
            continue;
        }
        let mut entry = object(vec![("id", Value::from(format!("{}:silkscreen:{index}", pose.reference)))]);
        if let (Value::Object(target), Value::Object(shape)) =
            (&mut entry, footprint_graphic_scene(graphic, &transform)?)
        {
            target.extend(shape);
        }
        result.push(entry);
    }
    Ok(result)
}

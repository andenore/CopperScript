//! Footprint-local transforms (`placement._transform_local`,
//! `transformed_local_point`) and placed pad copper (`drc.placed_pad_shape`).
//!
//! These use floats exactly as the Python code does: degrees to radians,
//! `cos`/`sin` and Python's round-half-even to integer nanometres.

use copper_physical::footprint::{FootprintPad, PadShape};
use copper_physical::pyfloat::round_int;
use copper_physical::{BoardSide, Point, RoundedConvexShape};
use copper_units::Decimal;

use crate::board::Placement;

pub fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

/// A rotation prepared once for all points of a footprint. Keep the separate
/// sin/cos operations and arithmetic order used by the compatible transform.
#[derive(Clone, Copy, Debug)]
struct Rotation {
    cosine: f64,
    sine: f64,
}

impl Rotation {
    fn new(degrees: f64) -> Self {
        let angle = degrees.to_radians();
        Self { cosine: angle.cos(), sine: angle.sin() }
    }

    fn apply(self, x: i64, y: i64) -> Point {
        let (x, y) = (x as f64, y as f64);
        Point::new(round_int(x * self.cosine - y * self.sine), round_int(x * self.sine + y * self.cosine))
    }

    fn offset(self, center: Point, x: i64, y: i64) -> Point {
        let point = self.apply(x, y);
        Point::new(center.x_nm + point.x_nm, center.y_nm + point.y_nm)
    }
}

/// An immutable placement's transform, reusable across its pads and vertices.
/// Recreate it when the placement changes; no global or stale-pose cache.
#[derive(Clone, Copy, Debug)]
pub struct PlacementTransform {
    position: Point,
    side: BoardSide,
    rotation: Rotation,
}

impl PlacementTransform {
    pub fn new(placement: &Placement) -> Self {
        Self {
            position: placement.position,
            side: placement.side,
            rotation: Rotation::new(-decimal_f64(&placement.rotation_degrees)),
        }
    }

    pub fn apply(&self, point: Point) -> Point {
        let x = if self.side == BoardSide::Back { -point.x_nm } else { point.x_nm };
        self.rotation.offset(self.position, x, point.y_nm)
    }
}

/// `_transform_local`: mirror on the back, then rotate by the KiCad angle.
pub fn transform_local(point: Point, rotation: &Decimal, side: BoardSide) -> Point {
    let x = if side == BoardSide::Back { -point.x_nm } else { point.x_nm };
    Rotation::new(-decimal_f64(rotation)).apply(x, point.y_nm)
}

/// `transformed_local_point`.
pub fn transformed_local_point(placement: &Placement, point: Point) -> Point {
    PlacementTransform::new(placement).apply(point)
}

/// `drc._rotate_offset`: `center` plus `(x, y)` rotated by `degrees`.
pub fn rotate_offset(center: Point, x: i64, y: i64, degrees: f64) -> Point {
    Rotation::new(degrees).offset(center, x, y)
}

/// `drc.placed_pad_shape`: the pad's copper at a board position.
pub fn placed_pad_shape(position: Point, pad: &FootprintPad, placement: &Placement) -> RoundedConvexShape {
    let (width, height) = (pad.size.width_nm, pad.size.height_nm);
    let sum = placement.rotation_degrees.add(&pad.rotation_degrees).expect("finite rotations");
    let angle = -decimal_f64(&sum);
    let shape = |spine: Vec<Point>, radius: i64| RoundedConvexShape::new(spine, radius).expect("valid pad shape");
    match pad.shape {
        PadShape::Circle => shape(vec![position], width.min(height) / 2),
        PadShape::Oval => {
            let rotation = Rotation::new(angle);
            let radius = width.min(height) / 2;
            let half_run = (width.max(height) - 2 * radius) / 2;
            let local = if width >= height { [(-half_run, 0), (half_run, 0)] } else { [(0, -half_run), (0, half_run)] };
            shape(local.iter().map(|(x, y)| rotation.offset(position, *x, *y)).collect(), radius)
        }
        PadShape::Rectangle | PadShape::Roundrect => {
            let rotation = Rotation::new(angle);
            let radius = if pad.shape == PadShape::Roundrect {
                (i128::from(width.min(height)) * i128::from(pad.roundrect_ratio_ppm)).div_euclid(1_000_000) as i64
            } else {
                0
            };
            let (half_width, half_height) = (width / 2 - radius, height / 2 - radius);
            let corners = [
                (-half_width, -half_height),
                (half_width, -half_height),
                (half_width, half_height),
                (-half_width, half_height),
            ];
            shape(corners.iter().map(|(x, y)| rotation.offset(position, *x, *y)).collect(), radius)
        }
    }
}

/// `transformed_pad_position`: the board position of one pad of a placed component.
pub fn transformed_pad_position(
    board: &crate::board::PhysicalBoard,
    placement: &Placement,
    pad_number: &str,
) -> crate::Result<Point> {
    let footprint = &board.footprints[&placement.footprint];
    let pad = footprint.pads.iter().find(|pad| pad.number == pad_number).ok_or_else(|| {
        crate::Error::Value(format!(
            "component {} has no pad {}",
            copper_json::python_str_repr(&placement.reference),
            copper_json::python_str_repr(pad_number)
        ))
    })?;
    Ok(transformed_local_point(placement, pad.position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_physical::footprint::{PadKind, Size};

    // Frozen pre-optimization arithmetic: deliberately do not call Rotation or
    // the compatibility wrappers, which now share the prepared implementation.
    fn legacy_offset(center: Point, x: i64, y: i64, degrees: f64) -> Point {
        let angle = degrees.to_radians();
        let (x, y) = (x as f64, y as f64);
        Point::new(
            center.x_nm + round_int(x * angle.cos() - y * angle.sin()),
            center.y_nm + round_int(x * angle.sin() + y * angle.cos()),
        )
    }

    fn placement(degrees: &str, side: BoardSide) -> Placement {
        Placement {
            reference: "U1".to_owned(),
            footprint: "test".to_owned(),
            position: Point::new(12_345_679, -23_456_789),
            rotation_degrees: Decimal::parse(degrees).unwrap(),
            side,
            value: String::new(),
            source_path: None,
        }
    }

    #[test]
    fn prepared_placement_preserves_rounding_and_mirroring() {
        let points = [
            Point::new(0, 0),
            Point::new(1, -1),
            Point::new(1_000_001, -3_000_003),
            Point::new(-5_000_005, 7_000_007),
            Point::new(9_000_000_000_001, -8_000_000_000_003),
        ];
        for degrees in ["-360", "-180", "-90", "-37.5", "-0", "0", "0.000001", "30", "45", "90", "180", "359.999999"] {
            for side in [BoardSide::Front, BoardSide::Back] {
                let pose = placement(degrees, side);
                let transform = PlacementTransform::new(&pose);
                for point in points {
                    let x = if side == BoardSide::Back { -point.x_nm } else { point.x_nm };
                    let expected = legacy_offset(pose.position, x, point.y_nm, -decimal_f64(&pose.rotation_degrees));
                    assert_eq!(transform.apply(point), expected, "{degrees}, {side:?}, {point:?}");
                    assert_eq!(transformed_local_point(&pose, point), expected);
                }
            }
        }
    }

    #[test]
    fn prepared_pad_rotation_preserves_each_copper_vertex() {
        for degrees in ["-90", "-37.5", "0", "30", "90", "180"] {
            for side in [BoardSide::Front, BoardSide::Back] {
                let pose = placement(degrees, side);
                for pad_degrees in ["-45", "0", "12.3456789", "90"] {
                    for (width, height) in [(1_000_001, 2_000_003), (3_000_007, 1_000_001)] {
                        for kind in [PadShape::Circle, PadShape::Oval, PadShape::Rectangle, PadShape::Roundrect] {
                            let pad = FootprintPad {
                                number: "1".to_owned(),
                                position: Point::new(123_457, -234_569),
                                size: Size::new(width, height).unwrap(),
                                kind: PadKind::Smd,
                                shape: kind,
                                rotation_degrees: Decimal::parse(pad_degrees).unwrap(),
                                drill: None,
                                roundrect_ratio_ppm: 234_567,
                                has_solder_mask: true,
                                has_solder_paste: true,
                                zone_connection: None,
                                heatsink: false,
                                remove_unused_layers: false,
                                connector_contact: false,
                            };
                            let position = PlacementTransform::new(&pose).apply(pad.position);
                            let radius = match kind {
                                PadShape::Circle | PadShape::Oval => width.min(height) / 2,
                                PadShape::Roundrect => (i128::from(width.min(height)) * 234_567 / 1_000_000) as i64,
                                PadShape::Rectangle => 0,
                            };
                            let local = match kind {
                                PadShape::Circle => vec![(0, 0)],
                                PadShape::Oval => {
                                    let run = (width.max(height) - 2 * radius) / 2;
                                    if width >= height { vec![(-run, 0), (run, 0)] } else { vec![(0, -run), (0, run)] }
                                }
                                _ => {
                                    let (x, y) = (width / 2 - radius, height / 2 - radius);
                                    vec![(-x, -y), (x, -y), (x, y), (-x, y)]
                                }
                            };
                            let angle = -decimal_f64(&pose.rotation_degrees.add(&pad.rotation_degrees).unwrap());
                            let expected = RoundedConvexShape::new(
                                local.into_iter().map(|(x, y)| legacy_offset(position, x, y, angle)).collect(),
                                radius,
                            )
                            .unwrap();
                            assert_eq!(
                                placed_pad_shape(position, &pad, &pose),
                                expected,
                                "{degrees}, {pad_degrees}, {side:?}, {kind:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

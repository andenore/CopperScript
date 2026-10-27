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

pub(crate) fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

/// `_transform_local`: mirror on the back, then rotate by the KiCad angle.
pub fn transform_local(point: Point, rotation: &Decimal, side: BoardSide) -> Point {
    let x = if side == BoardSide::Back { -point.x_nm } else { point.x_nm };
    let angle = (-decimal_f64(rotation)).to_radians();
    let (x, y) = (x as f64, point.y_nm as f64);
    Point::new(round_int(x * angle.cos() - y * angle.sin()), round_int(x * angle.sin() + y * angle.cos()))
}

/// `transformed_local_point`.
pub fn transformed_local_point(placement: &Placement, point: Point) -> Point {
    let offset = transform_local(point, &placement.rotation_degrees, placement.side);
    Point::new(placement.position.x_nm + offset.x_nm, placement.position.y_nm + offset.y_nm)
}

fn rotate_offset(center: Point, x: i64, y: i64, degrees: f64) -> Point {
    let angle = degrees.to_radians();
    let (x, y) = (x as f64, y as f64);
    Point::new(
        center.x_nm + round_int(x * angle.cos() - y * angle.sin()),
        center.y_nm + round_int(x * angle.sin() + y * angle.cos()),
    )
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
            let radius = width.min(height) / 2;
            let half_run = (width.max(height) - 2 * radius) / 2;
            let local = if width >= height { [(-half_run, 0), (half_run, 0)] } else { [(0, -half_run), (0, half_run)] };
            shape(local.iter().map(|(x, y)| rotate_offset(position, *x, *y, angle)).collect(), radius)
        }
        PadShape::Rectangle | PadShape::Roundrect => {
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
            shape(corners.iter().map(|(x, y)| rotate_offset(position, *x, *y, angle)).collect(), radius)
        }
    }
}

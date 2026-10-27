//! Closed line/arc boundary paths with bounded inscribed query rings
//! (`pcbir.mechanical_curves`).

use std::f64::consts::{PI, SQRT_2};

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{One, Signed, Zero};

use crate::board::{BoardBoundaryPath, BoundarySegment, Result};
use crate::geometry::{Point, rational};
use crate::mechanical::{signed_area_twice, validated_ring};
use crate::pyfloat::{float_mod, ratio_to_f64, round_int};

fn cross(a: Point, b: Point, c: Point) -> i128 {
    let (ax, ay, bx, by, cx, cy) = (
        i128::from(a.x_nm),
        i128::from(a.y_nm),
        i128::from(b.x_nm),
        i128::from(b.y_nm),
        i128::from(c.x_nm),
        i128::from(c.y_nm),
    );
    (bx - ax) * (cy - by) - (by - ay) * (cx - bx)
}

fn big(value: i64) -> BigInt {
    BigInt::from(value)
}

/// `arc_circle`: the exact circumcentre and squared radius of three points.
pub fn arc_circle(start: Point, mid: Point, end: Point) -> Result<(BigRational, BigRational, BigRational)> {
    let (ax, ay, mx, my, bx, by) =
        (big(start.x_nm), big(start.y_nm), big(mid.x_nm), big(mid.y_nm), big(end.x_nm), big(end.y_nm));
    let determinant = BigInt::from(2) * (&ax * (&my - &by) + &mx * (&by - &ay) + &bx * (&ay - &my));
    if determinant.is_zero() {
        return Err("boundary arc points cannot be collinear".to_owned());
    }
    let (qa, qm, qb) = (&ax * &ax + &ay * &ay, &mx * &mx + &my * &my, &bx * &bx + &by * &by);
    let cx = BigRational::new(&qa * (&my - &by) + &qm * (&by - &ay) + &qb * (&ay - &my), determinant.clone());
    let cy = BigRational::new(&qa * (&bx - &mx) + &qm * (&ax - &bx) + &qb * (&mx - &ax), determinant);
    let dx = BigRational::from_integer(ax) - &cx;
    let dy = BigRational::from_integer(ay) - &cy;
    let r2 = &dx * &dx + &dy * &dy;
    Ok((cx, cy, r2))
}

/// `arc_angles`: centre, squared radius, start angle and signed sweep.
pub fn arc_angles(start: Point, mid: Point, end: Point) -> Result<(BigRational, BigRational, BigRational, f64, f64)> {
    let (cx, cy, r2) = arc_circle(start, mid, end)?;
    let angle = |p: Point| {
        let y = ratio_to_f64(&(BigRational::from_integer(big(p.y_nm)) - &cy));
        let x = ratio_to_f64(&(BigRational::from_integer(big(p.x_nm)) - &cx));
        y.atan2(x)
    };
    let angles = [angle(start), angle(mid), angle(end)];
    let mut sweep = float_mod(angles[2] - angles[0], 2.0 * PI);
    if float_mod(angles[1] - angles[0], 2.0 * PI) > sweep {
        sweep -= 2.0 * PI;
    }
    if sweep.abs() > PI + 1e-12 {
        return Err("curved boundary arcs must span at most 180 degrees".to_owned());
    }
    Ok((cx, cy, r2, angles[0], sweep))
}

/// `_distance_squared`: from the rational centre to segment `a`-`b`.
fn distance_squared(cx: &BigRational, cy: &BigRational, a: Point, b: Point) -> Result<BigRational> {
    let (dx, dy) = (big(b.x_nm - a.x_nm), big(b.y_nm - a.y_nm));
    let length = &dx * &dx + &dy * &dy;
    if length.is_zero() {
        return Err("Fraction(0, 0)".to_owned());
    }
    let (ax, ay) = (BigRational::from_integer(big(a.x_nm)), BigRational::from_integer(big(a.y_nm)));
    let (dxr, dyr) = (BigRational::from_integer(dx), BigRational::from_integer(dy));
    let projection = ((cx - &ax) * &dxr + (cy - &ay) * &dyr) / BigRational::from_integer(length);
    let t = BigRational::zero().max(BigRational::one().min(projection));
    let x = cx - &ax - &t * &dxr;
    let y = cy - &ay - &t * &dyr;
    Ok(&x * &x + &y * &y)
}

/// Whether `distance < (sqrt(r2) - error)**2`, exactly.
///
/// Python evaluates this with 60-digit decimals; exact arithmetic agrees
/// wherever those digits can tell the two sides apart.
fn below_chord_limit(distance: &BigRational, r2: &BigRational, error: i64) -> bool {
    let e = rational(i128::from(error));
    let four = rational(4);
    if *r2 >= &e * &e {
        // sqrt(d) < r - e  <=>  2e*sqrt(d) < r2 - d - e^2.
        let rest = r2 - distance - &e * &e;
        rest.is_positive() && &four * &e * &e * distance < &rest * &rest
    } else {
        // sqrt(d) < e - r  <=>  2*sqrt(d*r2) < e^2 - d - r2.
        let rest = &e * &e - distance - r2;
        rest.is_positive() && &four * distance * r2 < &rest * &rest
    }
}

/// `path_query_ring`: the certified inscribed ring of a convex line/arc path.
pub fn path_query_ring(path: &BoardBoundaryPath) -> Result<Vec<Point>> {
    let mut vertices: Vec<Point> = Vec::new();
    let mut turns: Vec<i8> = Vec::new();
    for segment in &path.segments {
        let (start, mid, end) = match segment {
            BoundarySegment::Line { start, .. } => {
                vertices.push(*start);
                continue;
            }
            BoundarySegment::Arc { start, mid, end, .. } => (*start, *mid, *end),
        };
        let (cx, cy, r2, angle, sweep) = arc_angles(start, mid, end)?;
        let radius = ratio_to_f64(&r2).sqrt();
        let error = path.maximum_chord_error_nm;
        if (error as f64).partial_cmp(&radius) != Some(std::cmp::Ordering::Less) {
            return Err("curve chord error must be smaller than its radius".to_owned());
        }
        let step = 2.0 * (1.0 - 0.5_f64.min((error - 2) as f64 / (2.0 * radius))).acos();
        let count: i64 = if step != 0.0 { 2.max((sweep.abs() / step).ceil() as i64) } else { 4097 };
        if count + vertices.len() as i64 > 4096 {
            return Err("curve query exceeds 4096 vertices; increase chord error".to_owned());
        }
        let mut ring = vec![start];
        let (fx, fy) = (ratio_to_f64(&cx), ratio_to_f64(&cy));
        for i in 1..count {
            let theta = angle + sweep * i as f64 / count as f64;
            let x = fx + (radius - 1.0) * theta.cos();
            let y = fy + (radius - 1.0) * theta.sin();
            let p = Point::new(round_int(x), round_int(y));
            let dx = BigRational::from_integer(big(p.x_nm)) - &cx;
            let dy = BigRational::from_integer(big(p.y_nm)) - &cy;
            if &dx * &dx + &dy * &dy > r2 {
                return Err("curve query quantization cannot prove inscribed geometry".to_owned());
            }
            ring.push(p);
        }
        ring.push(end);
        for pair in ring.windows(2) {
            if below_chord_limit(&distance_squared(&cx, &cy, pair[0], pair[1])?, &r2, error) {
                return Err("curve query exceeds its declared chord-error bound".to_owned());
            }
        }
        ring.pop();
        vertices.extend(ring);
        turns.push(if sweep > 0.0 { 1 } else { -1 });
    }
    let vertices = validated_ring(vertices)?;
    let winding: i8 = if signed_area_twice(&vertices) > 0 { 1 } else { -1 };
    let count = vertices.len();
    if !turns.is_empty()
        && (turns.iter().any(|&turn| turn != winding)
            || (0..count).any(|i| {
                i128::from(winding) * cross(vertices[(i + count - 1) % count], vertices[i], vertices[(i + 1) % count])
                    < 0
            }))
    {
        return Err("curved paths currently require convex topology; concave curved queries are unsupported".to_owned());
    }
    Ok(vertices)
}

/// `rounded_rectangle_path`: four lines and four quarter arcs.
pub fn rounded_rectangle_path(
    origin: Point,
    width: i64,
    height: i64,
    radius: i64,
    error: i64,
) -> Result<BoardBoundaryPath> {
    if !(0 < 2 * radius && 2 * radius < width.min(height)) {
        return Err("rounded rectangle radius must be positive and below half its dimensions".to_owned());
    }
    let (x, y) = (origin.x_nm, origin.y_nm);
    let (right, bottom) = (x + width, y + height);
    let p = [
        Point::new(x + radius, y),
        Point::new(right - radius, y),
        Point::new(right, y + radius),
        Point::new(right, bottom - radius),
        Point::new(right - radius, bottom),
        Point::new(x + radius, bottom),
        Point::new(x, bottom - radius),
        Point::new(x, y + radius),
    ];
    let q = round_int(radius as f64 / SQRT_2);
    let mids = [
        Point::new(right - radius + q, y + radius - q),
        Point::new(right - radius + q, bottom - radius + q),
        Point::new(x + radius - q, bottom - radius + q),
        Point::new(x + radius - q, y + radius - q),
    ];
    let names = ["top", "top_right", "right", "bottom_right", "bottom", "bottom_left", "left", "top_left"];
    let mut segments = Vec::new();
    for i in 0..8 {
        let id = format!("outline/{}", names[i]);
        segments.push(if i % 2 == 0 {
            BoundarySegment::line(&id, p[i], p[(i + 1) % 8])?
        } else {
            BoundarySegment::arc(&id, p[i], mids[i / 2], p[(i + 1) % 8])?
        });
    }
    BoardBoundaryPath::new(segments, error)
}

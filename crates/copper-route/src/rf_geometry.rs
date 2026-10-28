//! Native filled-polygon decoding (`pcbir.rf_geometry.native_fill_polygon`),
//! reference-plane coverage (`reference_coverage`)
//! and the positive-area overlap tests the fill-fragment contacts need.
//!
//! Python builds these with shapely. Here a decoded fill is its outer ring and
//! holes, and overlaps are clipped areas in floats (decision D6: rounding is
//! not reproduced, so only marginal contacts can differ). Topologies shapely
//! would node or heal in ways not reproduced here (rings touching at a
//! vertex, a vertex on another edge) report not implemented, never a guess.

use indexmap::{IndexMap, IndexSet};

use crate::Result;

type P = (f64, f64);

/// A decoded fill: one outer ring and its holes (shapely's `Polygon`).
#[derive(Clone, Debug)]
pub struct FillPolygon {
    pub outer: Vec<P>,
    pub holes: Vec<Vec<P>>,
}

/// `native_fill_polygon` outcome.
#[derive(Clone, Debug)]
pub enum NativeFill {
    Polygon(FillPolygon),
    /// A valid decode whose union is not one polygon (`MultiPolygon`): its
    /// disjoint faces.
    Multiple(Vec<FillPolygon>),
    /// `ValueError`: an unsupported or invalid representation.
    Invalid,
}

fn orientation(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i128 {
    i128::from(b.0 - a.0) * i128::from(c.1 - a.1) - i128::from(b.1 - a.1) * i128::from(c.0 - a.0)
}

fn on_segment(p: (i64, i64), a: (i64, i64), b: (i64, i64)) -> bool {
    orientation(a, b, p) == 0
        && a.0.min(b.0) <= p.0
        && p.0 <= a.0.max(b.0)
        && a.1.min(b.1) <= p.1
        && p.1 <= a.1.max(b.1)
}

fn segments_touch(a: (i64, i64), b: (i64, i64), c: (i64, i64), d: (i64, i64)) -> bool {
    let (d1, d2) = (orientation(c, d, a), orientation(c, d, b));
    let (d3, d4) = (orientation(a, b, c), orientation(a, b, d));
    if ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0)) {
        return true;
    }
    on_segment(a, c, d) || on_segment(b, c, d) || on_segment(c, a, b) || on_segment(d, a, b)
}

type Edge = ((i64, i64), (i64, i64));

/// Pairs of edges that touch other than at one shared endpoint of edges
/// consecutive in a ring (`next[i]` follows edge `i`).
fn edges_simple(edges: &[Edge], next: &[usize]) -> bool {
    let mut order: Vec<usize> = (0..edges.len()).collect();
    order.sort_by_key(|&i| edges[i].0.0.min(edges[i].1.0));
    for (position, &i) in order.iter().enumerate() {
        let (a, b) = edges[i];
        let right = a.0.max(b.0);
        for &j in &order[position + 1..] {
            let (c, d) = edges[j];
            if c.0.min(d.0) > right {
                break;
            }
            if a.1.max(b.1) < c.1.min(d.1) || c.1.max(d.1) < a.1.min(b.1) {
                continue;
            }
            let consecutive = next[i] == j || next[j] == i;
            if consecutive {
                // Sharing exactly the joint: reject only a fold back.
                let (joint, first, second) = if next[i] == j { (b, a, d) } else { (a, b, c) };
                let folded = orientation(first, joint, second) == 0
                    && (i128::from(first.0 - joint.0) * i128::from(second.0 - joint.0)
                        + i128::from(first.1 - joint.1) * i128::from(second.1 - joint.1))
                        > 0;
                let doubly = next[i] == j && next[j] == i;
                if folded || doubly {
                    return false;
                }
                continue;
            }
            if segments_touch(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

fn ring_area(ring: &[(i64, i64)]) -> i128 {
    let mut twice = 0i128;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        twice += i128::from(a.0) * i128::from(b.1) - i128::from(b.0) * i128::from(a.1);
    }
    twice
}

fn point_in_ring(point: (i64, i64), ring: &[(i64, i64)]) -> bool {
    let (x, y) = (point.0 as f64, point.1 as f64);
    let mut inside = false;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        if (a.1 > point.1) != (b.1 > point.1)
            && x < (b.0 - a.0) as f64 * (y - a.1 as f64) / (b.1 - a.1) as f64 + a.0 as f64
        {
            inside = !inside;
        }
    }
    inside
}

fn floats(ring: &[(i64, i64)]) -> Vec<P> {
    ring.iter().map(|&(x, y)| (x as f64, y as f64)).collect()
}

/// `native_fill_polygon({"outer": vertices, "holes": []})`.
pub fn native_fill_polygon(vertices: &[(i64, i64)]) -> Result<NativeFill> {
    let mut ring: Vec<(i64, i64)> = Vec::new();
    for &point in vertices {
        if ring.last() != Some(&point) {
            ring.push(point);
        }
    }
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() >= 3 {
        let edges: Vec<Edge> = (0..ring.len()).map(|i| (ring[i], ring[(i + 1) % ring.len()])).collect();
        let next: Vec<usize> = (0..ring.len()).map(|i| (i + 1) % ring.len()).collect();
        if ring_area(&ring) != 0 && edges_simple(&edges, &next) {
            return Ok(NativeFill::Polygon(FillPolygon { outer: floats(&ring), holes: Vec::new() }));
        }
    }
    // Slit-fractured holes: remove exact reversed edge pairs only.
    let edges: Vec<Edge> = (0..vertices.len())
        .map(|i| (vertices[i], vertices[(i + 1) % vertices.len()]))
        .filter(|(a, b)| a != b)
        .collect();
    let mut counts: IndexMap<Edge, usize> = IndexMap::new();
    for edge in &edges {
        *counts.entry(*edge).or_insert(0) += 1;
    }
    if counts.values().any(|&count| count != 1) {
        return Ok(NativeFill::Invalid);
    }
    let bridges: IndexSet<Edge> = counts.keys().filter(|(a, b)| counts.contains_key(&(*b, *a))).copied().collect();
    if bridges.is_empty() {
        return Ok(NativeFill::Invalid);
    }
    let lines: Vec<Edge> = edges.iter().filter(|edge| !bridges.contains(*edge)).copied().collect();
    if lines.is_empty() {
        return Ok(NativeFill::Invalid);
    }
    // Remaining linework must be disjoint simple rings (each vertex one in,
    // one out); shapely's polygonize is not reproduced for anything else.
    let mut successor: IndexMap<(i64, i64), usize> = IndexMap::new();
    let mut incoming: IndexSet<(i64, i64)> = IndexSet::new();
    for (index, (a, b)) in lines.iter().enumerate() {
        if successor.insert(*a, index).is_some() || !incoming.insert(*b) {
            return copper_pcb::not_implemented("native fill linework with a shared vertex (shapely polygonize)");
        }
    }
    if successor.len() != incoming.len() || successor.keys().any(|point| !incoming.contains(point)) {
        return copper_pcb::not_implemented("open native fill linework (shapely polygonize)");
    }
    let next: Vec<usize> = lines.iter().map(|(_, b)| successor[b]).collect();
    if !edges_simple(&lines, &next) {
        return copper_pcb::not_implemented("crossing or touching native fill linework (shapely polygonize)");
    }
    let mut seen = vec![false; lines.len()];
    let mut rings: Vec<Vec<(i64, i64)>> = Vec::new();
    for start in 0..lines.len() {
        if seen[start] {
            continue;
        }
        let mut ring = Vec::new();
        let mut cursor = start;
        while !seen[cursor] {
            seen[cursor] = true;
            ring.push(lines[cursor].0);
            cursor = next[cursor];
        }
        if ring.len() < 3 || ring_area(&ring) == 0 {
            return copper_pcb::not_implemented("degenerate native fill ring (shapely polygonize)");
        }
        rings.push(ring);
    }
    let depths: Vec<usize> = (0..rings.len())
        .map(|i| (0..rings.len()).filter(|&j| j != i && point_in_ring(rings[i][0], &rings[j])).count())
        .collect();
    let outers: Vec<usize> = (0..rings.len()).filter(|&i| depths[i].is_multiple_of(2)).collect();
    match outers.as_slice() {
        [] => Ok(NativeFill::Invalid),
        [outer] if depths[*outer] == 0 => Ok(NativeFill::Polygon(FillPolygon {
            outer: floats(&rings[*outer]),
            holes: (0..rings.len()).filter(|&i| depths[i] == 1).map(|i| floats(&rings[i])).collect(),
        })),
        _ => Ok(NativeFill::Multiple(
            outers
                .iter()
                .map(|&outer| FillPolygon {
                    outer: floats(&rings[outer]),
                    holes: (0..rings.len())
                        .filter(|&i| depths[i] == depths[outer] + 1 && point_in_ring(rings[i][0], &rings[outer]))
                        .map(|i| floats(&rings[i]))
                        .collect(),
                })
                .collect(),
        )),
    }
}

/// A straight track as `reference_coverage` reads it.
pub struct CoverageTrack {
    pub net: String,
    pub layer: String,
    pub width_nm: i64,
    pub start: (i64, i64),
    pub end: (i64, i64),
}

/// `qualification.check(identifier, status, detail, **metrics)`.
fn check(identifier: &str, status: &str, detail: &str, metrics: copper_json::Object) -> copper_json::Object {
    let mut document = copper_json::Object::new();
    document.insert("id".to_owned(), copper_json::Value::from(identifier));
    document.insert("status".to_owned(), copper_json::Value::from(status));
    document.insert("stage".to_owned(), copper_json::Value::from("design"));
    document.insert("detail".to_owned(), copper_json::Value::from(detail));
    document.insert("metrics".to_owned(), copper_json::Value::Object(metrics));
    document
}

/// `reference_coverage` (margin 0): whether each signal track's square-capped
/// envelope lies inside the adjacent reference layer's native fill.
pub fn reference_coverage(
    layers: &[String],
    tracks: &[CoverageTrack],
    fills: &[(&str, &str, &[(i64, i64)])],
    identifier: &str,
    net: &str,
    reference_net: &str,
    reference_layer: &str,
) -> Result<copper_json::Object> {
    use copper_json::{Object, Value};
    let unique: IndexSet<&String> = layers.iter().collect();
    let Some(reference) = layers.iter().position(|layer| layer == reference_layer) else {
        return Err(copper_pcb::Error::Value("invalid reference layer/order".to_owned()));
    };
    if unique.len() != layers.len() {
        return Err(copper_pcb::Error::Value("invalid reference layer/order".to_owned()));
    }
    let own: Vec<&CoverageTrack> = tracks.iter().filter(|track| track.net == net).collect();
    if own.is_empty() {
        return Ok(check(identifier, "incomplete", &format!("No supported straight tracks for {net}"), Object::new()));
    }
    if own.iter().any(|track| {
        layers.iter().position(|layer| *layer == track.layer).is_none_or(|index| index.abs_diff(reference) != 1)
    }) {
        return Ok(check(
            identifier,
            "fail",
            "Signal tracks do not share the declared adjacent reference",
            Object::new(),
        ));
    }
    let selected: Vec<&[(i64, i64)]> = fills
        .iter()
        .filter(|(fill_net, layer, _)| *fill_net == reference_net && *layer == reference_layer)
        .map(|(_, _, vertices)| *vertices)
        .collect();
    if selected.is_empty() {
        return Ok(check(
            identifier,
            "incomplete",
            "No native filled reference copper; zone outlines are not evidence",
            Object::new(),
        ));
    }
    let mut polygons: Vec<FillPolygon> = Vec::new();
    for vertices in selected {
        match native_fill_polygon(vertices)? {
            NativeFill::Polygon(polygon) => polygons.push(polygon),
            NativeFill::Multiple(faces) => polygons.extend(faces),
            NativeFill::Invalid => {
                return Ok(check(
                    identifier,
                    "incomplete",
                    "Unsupported/invalid filled polygon; no automatic healing",
                    Object::new(),
                ));
            }
        }
    }
    // The union's area is the faces' sum only while they do not overlap.
    for index in 0..polygons.len() {
        for other in 0..index {
            if polygons[index].overlap(&polygons[other]) > 0.0 {
                return copper_pcb::not_implemented("overlapping native reference fills (shapely unary_union)");
            }
        }
    }
    let mut uncovered = Vec::new();
    let mut length = 0.0;
    for (index, track) in own.iter().enumerate() {
        if track.width_nm <= 0 {
            return Err(copper_pcb::Error::Value("invalid track width".to_owned()));
        }
        if track.start == track.end {
            return Err(copper_pcb::Error::Value("zero-length RF track".to_owned()));
        }
        let (dx, dy) = ((track.end.0 - track.start.0) as f64, (track.end.1 - track.start.1) as f64);
        let span = dx.hypot(dy);
        length += span;
        // Square caps/miter joins conservatively contain the rounded conductor.
        let half = track.width_nm as f64 / 2.0;
        let (ux, uy) = (dx / span * half, dy / span * half);
        let (sx, sy) = (track.start.0 as f64 - ux, track.start.1 as f64 - uy);
        let (ex, ey) = (track.end.0 as f64 + ux, track.end.1 as f64 + uy);
        let envelope = Convex(vec![(sx + uy, sy - ux), (ex + uy, ey - ux), (ex - uy, ey + ux), (sx - uy, sy + ux)]);
        let envelope =
            Convex(if signed_area(&envelope.0) < 0.0 { envelope.0.into_iter().rev().collect() } else { envelope.0 });
        let area = envelope.area();
        let covered: f64 = polygons.iter().map(|polygon| polygon.convex_overlap(&envelope)).sum();
        if area - covered > area * 1e-9 {
            uncovered.push(Value::from(index as i64));
        }
    }
    let mut metrics = Object::new();
    let failed = !uncovered.is_empty();
    metrics.insert("unsupported_segment_indices".to_owned(), Value::Array(uncovered));
    metrics.insert("track_count".to_owned(), Value::from(own.len() as i64));
    metrics.insert("total_segment_length_nm".to_owned(), Value::Float(length));
    metrics.insert("reference_layer".to_owned(), Value::from(reference_layer));
    metrics.insert("margin_nm".to_owned(), Value::from(0i64));
    Ok(check(
        identifier,
        if failed { "fail" } else { "pass" },
        "Projected conductor coverage only; no electrical/impedance proof",
        metrics,
    ))
}

fn signed_area(ring: &[P]) -> f64 {
    let mut twice = 0.0;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        twice += a.0 * b.1 - b.0 * a.1;
    }
    twice / 2.0
}

/// Sutherland-Hodgman: `subject` clipped by the convex counter-clockwise `clip`.
/// The signed area of the result is the overlap of any simple subject ring.
fn clip_convex(subject: &[P], clip: &[P]) -> Vec<P> {
    let mut output = subject.to_vec();
    for (index, a) in clip.iter().enumerate() {
        if output.is_empty() {
            break;
        }
        let b = clip[(index + 1) % clip.len()];
        let side = |p: P| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        let input = std::mem::take(&mut output);
        for (position, current) in input.iter().enumerate() {
            let previous = input[(position + input.len() - 1) % input.len()];
            let (sc, sp) = (side(*current), side(previous));
            if sc >= 0.0 {
                if sp < 0.0 {
                    let t = sp / (sp - sc);
                    output.push((previous.0 + t * (current.0 - previous.0), previous.1 + t * (current.1 - previous.1)));
                }
                output.push(*current);
            } else if sp >= 0.0 {
                let t = sp / (sp - sc);
                output.push((previous.0 + t * (current.0 - previous.0), previous.1 + t * (current.1 - previous.1)));
            }
        }
    }
    output
}

/// A convex counter-clockwise polygon (possibly degenerate: no area).
#[derive(Clone, Debug)]
pub struct Convex(pub Vec<P>);

impl Convex {
    pub fn area(&self) -> f64 {
        if self.0.len() < 3 { 0.0 } else { signed_area(&self.0).abs() }
    }

    /// The convex intersection with another convex polygon.
    pub fn intersection(&self, other: &Convex) -> Convex {
        if self.0.len() < 3 || other.0.len() < 3 {
            return Convex(Vec::new());
        }
        Convex(clip_convex(&self.0, &other.0))
    }
}

fn hull(mut points: Vec<P>) -> Vec<P> {
    points.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: P, a: P, b: P| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<P> = Vec::new();
    for &p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<P> = Vec::new();
    for &p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// `MultiPoint(spine).convex_hull.buffer(radius, quad_segs=16)` (64 segments
/// per circle), as the Minkowski sum of the hull and that regular polygon.
pub fn buffered_hull(spine: &[(i64, i64)], radius: f64) -> Convex {
    let points: Vec<P> = spine.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
    if radius <= 0.0 {
        return Convex(hull(points));
    }
    let mut sum = Vec::with_capacity(points.len() * 64);
    for &(x, y) in &points {
        for step in 0..64 {
            let angle = f64::from(step) * std::f64::consts::PI / 32.0;
            sum.push((x + radius * angle.cos(), y + radius * angle.sin()));
        }
    }
    Convex(hull(sum))
}

impl FillPolygon {
    /// The overlap area with a convex polygon.
    pub fn convex_overlap(&self, convex: &Convex) -> f64 {
        if convex.0.len() < 3 {
            return 0.0;
        }
        let mut area = signed_area(&clip_convex(&self.outer, &convex.0)).abs();
        for hole in &self.holes {
            area -= signed_area(&clip_convex(hole, &convex.0)).abs();
        }
        area
    }

    fn rings(&self) -> impl Iterator<Item = (&Vec<P>, f64)> {
        std::iter::once((&self.outer, 1.0)).chain(self.holes.iter().map(|hole| (hole, -1.0)))
    }

    /// The overlap area with another fill (a boundary integral over each
    /// ring pair; both are simple rings, oriented here counter-clockwise).
    pub fn overlap(&self, other: &FillPolygon) -> f64 {
        let mut area = 0.0;
        for (first, first_sign) in self.rings() {
            for (second, second_sign) in other.rings() {
                area += first_sign * second_sign * ring_overlap(first, second);
            }
        }
        area
    }
}

fn counter_clockwise(ring: &[P]) -> Vec<P> {
    let mut ring = ring.to_vec();
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

/// Inside (1), outside (-1) or on the boundary (0, with the edge direction).
fn classify(point: P, ring: &[P]) -> (i32, Option<P>) {
    let scale = ring.iter().fold(1.0f64, |m, p| m.max(p.0.abs()).max(p.1.abs()));
    let tolerance = scale * 1e-12;
    let mut inside = false;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy);
        let cross = dx * (point.1 - a.1) - dy * (point.0 - a.0);
        let along = dx * (point.0 - a.0) + dy * (point.1 - a.1);
        if length > 0.0 && cross.abs() <= tolerance * length && along >= 0.0 && along <= length * length {
            return (0, Some((dx, dy)));
        }
        if (a.1 > point.1) != (b.1 > point.1) && point.0 < dx * (point.1 - a.1) / dy + a.0 {
            inside = !inside;
        }
    }
    (if inside { 1 } else { -1 }, None)
}

fn boundary_inside(first: &[P], second: &[P], shared: bool) -> f64 {
    let mut total = 0.0;
    for (index, a) in first.iter().enumerate() {
        let b = first[(index + 1) % first.len()];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let mut cuts = vec![0.0, 1.0];
        for (other, c) in second.iter().enumerate() {
            let d = second[(other + 1) % second.len()];
            let (ex, ey) = (d.0 - c.0, d.1 - c.1);
            let denominator = dx * ey - dy * ex;
            if denominator != 0.0 {
                let t = ((c.0 - a.0) * ey - (c.1 - a.1) * ex) / denominator;
                let u = ((c.0 - a.0) * dy - (c.1 - a.1) * dx) / denominator;
                if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                    cuts.push(t);
                }
            } else {
                let length = dx * dx + dy * dy;
                for p in [*c, d] {
                    let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length;
                    if t > 0.0 && t < 1.0 {
                        cuts.push(t);
                    }
                }
            }
        }
        cuts.sort_by(|x, y| x.partial_cmp(y).expect("finite"));
        for pair in cuts.windows(2) {
            if pair[1] <= pair[0] {
                continue;
            }
            let (s, e) = ((a.0 + pair[0] * dx, a.1 + pair[0] * dy), (a.0 + pair[1] * dx, a.1 + pair[1] * dy));
            let middle = ((s.0 + e.0) / 2.0, (s.1 + e.1) / 2.0);
            let include = match classify(middle, second) {
                (1, _) => true,
                (0, Some(direction)) => shared && direction.0 * dx + direction.1 * dy > 0.0,
                _ => false,
            };
            if include {
                total += s.0 * e.1 - e.0 * s.1;
            }
        }
    }
    total / 2.0
}

/// The overlap area of two simple rings.
fn ring_overlap(first: &[P], second: &[P]) -> f64 {
    let bounds = |ring: &[P]| {
        ring.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), p| {
            (a.min(p.0), b.min(p.1), c.max(p.0), d.max(p.1))
        })
    };
    let (f, s) = (bounds(first), bounds(second));
    if f.2 <= s.0 || s.2 <= f.0 || f.3 <= s.1 || s.3 <= f.1 {
        return 0.0;
    }
    let (first, second) = (counter_clockwise(first), counter_clockwise(second));
    (boundary_inside(&first, &second, true) + boundary_inside(&second, &first, false)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square with a square hole joined by a reversed-edge slit (KiCad's
    /// fractured fill); the areas are shapely's.
    fn slit_square() -> FillPolygon {
        let outer = [
            (0, 0),
            (100, 0),
            (100, 100),
            (0, 100),
            (0, 50),
            (40, 50),
            (40, 60),
            (60, 60),
            (60, 40),
            (40, 40),
            (40, 50),
            (0, 50),
        ];
        match native_fill_polygon(&outer).expect("decodes") {
            NativeFill::Polygon(polygon) => polygon,
            other => panic!("{other:?}"),
        }
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() <= 1e-6 * expected.abs().max(1.0), "{actual} != {expected}");
    }

    #[test]
    fn slit_fill_decodes_to_a_polygon_with_a_hole() {
        let polygon = slit_square();
        assert_eq!(polygon.holes.len(), 1);
        close(signed_area(&polygon.outer).abs() - signed_area(&polygon.holes[0]).abs(), 9600.0);
    }

    #[test]
    fn overlaps_match_shapely() {
        let polygon = slit_square();
        close(polygon.convex_overlap(&buffered_hull(&[(50, 50)], 15.0)), 305.7234103728364);
        close(polygon.convex_overlap(&buffered_hull(&[(20, 20)], 5.0)), 78.41371226364846);
        let inside_hole = buffered_hull(&[(45, 45), (55, 45), (55, 55), (45, 55)], 0.0);
        close(polygon.convex_overlap(&inside_hole), 0.0);
        let NativeFill::Polygon(corner) = native_fill_polygon(&[(90, 90), (150, 90), (150, 150), (90, 150)]).unwrap()
        else {
            panic!("simple square")
        };
        close(polygon.overlap(&corner), 100.0);
        close(corner.overlap(&polygon), 100.0);
    }

    #[test]
    fn invalid_walks_are_rejected() {
        // A bow tie crosses itself and has no reversed slit.
        assert!(matches!(native_fill_polygon(&[(0, 0), (10, 10), (10, 0), (0, 10)]).unwrap(), NativeFill::Invalid));
    }

    #[test]
    fn reference_coverage_matches_shapely() {
        let outer = [
            (0, 0),
            (100, 0),
            (100, 100),
            (0, 100),
            (0, 50),
            (40, 50),
            (40, 60),
            (60, 60),
            (60, 40),
            (40, 40),
            (40, 50),
            (0, 50),
        ];
        let track =
            |start, end| CoverageTrack { net: "RF".to_owned(), layer: "F.Cu".to_owned(), width_nm: 4, start, end };
        let tracks = [track((10, 10), (90, 10)), track((10, 45), (90, 45)), track((3, 80), (97, 80))];
        let layers = ["F.Cu".to_owned(), "In1.Cu".to_owned()];
        let result = reference_coverage(
            &layers,
            &tracks,
            &[("GND", "In1.Cu", &outer[..])],
            "reference:RF",
            "RF",
            "GND",
            "In1.Cu",
        )
        .unwrap();
        let text = copper_json::Dumps::new().dumps(&copper_json::Value::Object(result));
        assert_eq!(
            text,
            "{\"id\": \"reference:RF\", \"status\": \"fail\", \"stage\": \"design\", \"detail\": \"Projected conductor \
             coverage only; no electrical/impedance proof\", \"metrics\": {\"unsupported_segment_indices\": [1], \
             \"track_count\": 3, \"total_segment_length_nm\": 254.0, \"reference_layer\": \"In1.Cu\", \"margin_nm\": 0}}"
        );
    }
}

//! The faces of slit-fractured native fill linework: what
//! `pcbir.rf_geometry.native_fill_polygon` gets from shapely's `polygonize`,
//! its even-odd face selection and `unary_union`.
//!
//! The linework (the walk's edges less its reversed slit pairs) is noded
//! exactly: a vertex on another edge splits that edge and coincident pieces
//! of collinear edges merge, while a proper crossing, a piece covered an even
//! number of times or a ring crossing an edge at a vertex on it rejects the
//! walk (Python's "dropped or crossing linework"). Every vertex of the noded arrangement then has even
//! degree and adjacent faces have opposite even-odd parity, so the selected
//! region's boundary is the boundary cycles of its odd faces. Those cycles
//! are split at repeated vertices into simple rings, the OGC form shapely
//! returns: rings that touch at a vertex stay separate rings, a hole may
//! touch its shell at a point, and faces meeting only at a point are separate
//! polygons. Coordinates stay integer nanometres throughout; midpoints are
//! taken in doubled coordinates.

use indexmap::IndexMap;

type I = (i64, i64);
type Segment = (I, I);

/// A decoded face: its shell and holes, as vertex rings.
pub type Face = (Vec<I>, Vec<Vec<I>>);

fn orientation(a: I, b: I, c: I) -> i128 {
    i128::from(b.0 - a.0) * i128::from(c.1 - a.1) - i128::from(b.1 - a.1) * i128::from(c.0 - a.0)
}

/// `p` lies strictly between `a` and `b` on their (collinear) segment.
fn strictly_inside(p: I, a: I, b: I) -> bool {
    p != a && p != b && a.0.min(b.0) <= p.0 && p.0 <= a.0.max(b.0) && a.1.min(b.1) <= p.1 && p.1 <= a.1.max(b.1)
}

/// The segments split at every vertex lying on another segment's interior,
/// each piece with its segment's index; `None` for a proper crossing.
fn node(lines: &[Segment]) -> Option<(Vec<Segment>, Vec<usize>)> {
    let mut cuts: Vec<Vec<I>> = vec![Vec::new(); lines.len()];
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by_key(|&i| lines[i].0.0.min(lines[i].1.0));
    for (position, &i) in order.iter().enumerate() {
        let (a, b) = lines[i];
        let right = a.0.max(b.0);
        for &j in &order[position + 1..] {
            let (c, d) = lines[j];
            if c.0.min(d.0) > right {
                break;
            }
            if a.1.max(b.1) < c.1.min(d.1) || c.1.max(d.1) < a.1.min(b.1) {
                continue;
            }
            let (o1, o2) = (orientation(a, b, c), orientation(a, b, d));
            let (o3, o4) = (orientation(c, d, a), orientation(c, d, b));
            if o1.signum() * o2.signum() < 0 && o3.signum() * o4.signum() < 0 {
                return None;
            }
            for (zero, point, segment, (start, end)) in
                [(o1, c, i, (a, b)), (o2, d, i, (a, b)), (o3, a, j, (c, d)), (o4, b, j, (c, d))]
            {
                if zero == 0 && strictly_inside(point, start, end) {
                    cuts[segment].push(point);
                }
            }
        }
    }
    let (mut pieces, mut parents) = (Vec::new(), Vec::new());
    for (parent, (&(a, b), mut points)) in lines.iter().zip(cuts).enumerate() {
        let distance = |p: &I| i128::from(p.0 - a.0).abs() + i128::from(p.1 - a.1).abs();
        points.sort_by_key(distance);
        points.dedup();
        let mut start = a;
        for point in points.into_iter().chain([b]) {
            pieces.push((start, point));
            parents.push(parent);
            start = point;
        }
    }
    Some((pieces, parents))
}

/// Counter-clockwise angular order of direction vectors from the +x axis.
fn angular(a: I, b: I) -> std::cmp::Ordering {
    let half = |v: I| v.1 < 0 || (v.1 == 0 && v.0 < 0);
    half(a).cmp(&half(b)).then_with(|| 0.cmp(&orientation((0, 0), a, b)))
}

fn twice_area(ring: &[I]) -> i128 {
    let mut twice = 0i128;
    for (index, a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        twice += i128::from(a.0) * i128::from(b.1) - i128::from(b.0) * i128::from(a.1);
    }
    twice
}

/// Even-odd crossings of the ray from `origin` (doubled coordinates) along
/// +x (or +y with `vertical`) with the doubled segments other than `skip`.
fn ray_parity(pieces: &[Segment], skip: usize, origin: I, vertical: bool) -> bool {
    let swap = |p: I| if vertical { (p.1, p.0) } else { p };
    let (x, y) = swap(origin);
    let mut odd = false;
    for (index, &(a, b)) in pieces.iter().enumerate() {
        if index == skip {
            continue;
        }
        let (a, b) = (swap((2 * a.0, 2 * a.1)), swap((2 * b.0, 2 * b.1)));
        if (a.1 > y) == (b.1 > y) {
            continue;
        }
        // The crossing's abscissa exceeds x: (a.x - x)(b.y - a.y) + (b.x - a.x)(y - a.y) has the sign of b.y - a.y.
        let numerator = i128::from(a.0 - x) * i128::from(b.1 - a.1) + i128::from(b.0 - a.0) * i128::from(y - a.1);
        if (numerator > 0) == (b.1 > a.1) && numerator != 0 {
            odd = !odd;
        }
    }
    odd
}

/// Strict even-odd containment of a doubled point in a ring.
fn contains(ring: &[I], point: I) -> bool {
    let mut inside = false;
    for (index, &a) in ring.iter().enumerate() {
        let b = ring[(index + 1) % ring.len()];
        let (a, b) = ((2 * a.0, 2 * a.1), (2 * b.0, 2 * b.1));
        if (a.1 > point.1) == (b.1 > point.1) {
            continue;
        }
        let numerator =
            i128::from(a.0 - point.0) * i128::from(b.1 - a.1) + i128::from(b.0 - a.0) * i128::from(point.1 - a.1);
        if (numerator > 0) == (b.1 > a.1) && numerator != 0 {
            inside = !inside;
        }
    }
    inside
}

/// Splits a closed vertex walk at repeated vertices into simple rings.
fn simple_rings(walk: &[usize]) -> Vec<Vec<usize>> {
    let mut rings = Vec::new();
    let mut path: Vec<usize> = Vec::new();
    let mut position: IndexMap<usize, usize> = IndexMap::new();
    for &vertex in walk {
        if let Some(&start) = position.get(&vertex) {
            let ring: Vec<usize> = path.drain(start + 1..).collect();
            for removed in &ring {
                position.shift_remove(removed);
            }
            rings.push(std::iter::once(vertex).chain(ring).collect());
        } else {
            position.insert(vertex, path.len());
            path.push(vertex);
        }
    }
    rings.push(path);
    rings
}

/// The odd faces of the linework, or `None` when it is not a valid
/// arrangement (crossing or dropped linework, no selected face).
pub fn odd_faces(lines: &[Segment]) -> Option<Vec<Face>> {
    let (split, parents) = node(lines)?;
    // Coincident pieces of collinear segments merge: an odd count bounds the
    // region once, an even count bounds nothing (dropped linework).
    let mut merged: IndexMap<Segment, usize> = IndexMap::new();
    let mut reversed = Vec::with_capacity(split.len());
    let mut unique = Vec::with_capacity(split.len());
    for &(a, b) in &split {
        let key = if a < b { (a, b) } else { (b, a) };
        let entry = merged.entry(key);
        unique.push(entry.index());
        *entry.or_default() += 1;
        reversed.push(a > b);
    }
    if merged.values().any(|count| count.is_multiple_of(2)) {
        return None;
    }
    let pieces: Vec<Segment> = merged.into_keys().collect();
    let mut vertices: IndexMap<I, Vec<usize>> = IndexMap::new();
    // Half-edge 2k runs along piece k, 2k + 1 against it.
    let origin = |half: usize| if half.is_multiple_of(2) { pieces[half / 2].0 } else { pieces[half / 2].1 };
    let target = |half: usize| origin(half ^ 1);
    for half in 0..2 * pieces.len() {
        vertices.entry(origin(half)).or_default().push(half);
    }
    let mut slot = vec![0usize; 2 * pieces.len()];
    for (point, outgoing) in vertices.iter_mut() {
        outgoing.sort_by(|&a, &b| {
            let (ta, tb) = (target(a), target(b));
            angular((ta.0 - point.0, ta.1 - point.1), (tb.0 - point.0, tb.1 - point.1))
        });
        for (index, &half) in outgoing.iter().enumerate() {
            slot[half] = index;
        }
    }
    // The face on a half-edge's left continues along the outgoing half-edge
    // just clockwise of its twin.
    let next = |half: usize| {
        let twin = half ^ 1;
        let outgoing = &vertices[&origin(twin)];
        outgoing[(slot[twin] + outgoing.len() - 1) % outgoing.len()]
    };
    let mut seen = vec![false; 2 * pieces.len()];
    // Whether the selected region lies left of each piece, along its segment.
    let mut left = vec![false; pieces.len()];
    let mut shells: Vec<Vec<I>> = Vec::new();
    let mut holes: Vec<Vec<I>> = Vec::new();
    for start in 0..2 * pieces.len() {
        if seen[start] {
            continue;
        }
        let mut cycle = Vec::new();
        let mut half = start;
        while !seen[half] {
            seen[half] = true;
            cycle.push(half);
            half = next(half);
        }
        // The cycle's face parity: the left side of its first half-edge.
        let (a, b) = (origin(start), target(start));
        let middle = (a.0 + b.0, a.1 + b.1);
        let odd = if a.1 != b.1 {
            ray_parity(&pieces, start / 2, middle, false) != (b.1 > a.1)
        } else {
            ray_parity(&pieces, start / 2, middle, true) == (b.0 > a.0)
        };
        if !odd {
            continue;
        }
        for &half in &cycle {
            left[half / 2] = half.is_multiple_of(2);
        }
        let walk: Vec<usize> =
            cycle.iter().map(|&half| vertices.get_index_of(&origin(half)).expect("vertex")).collect();
        for ring in simple_rings(&walk) {
            let ring: Vec<I> = ring.into_iter().map(|index| *vertices.get_index(index).expect("vertex").0).collect();
            match twice_area(&ring) {
                0 => return None,
                area if area > 0 => shells.push(ring),
                _ => holes.push(ring),
            }
        }
    }
    // A vertex on a segment is a touch only when the region keeps its side
    // of the segment there; otherwise another ring crosses it at that vertex.
    let side = |piece: usize| left[unique[piece]] != reversed[piece];
    if (1..split.len()).any(|piece| parents[piece] == parents[piece - 1] && side(piece) != side(piece - 1)) {
        return None;
    }
    if shells.is_empty() {
        return None;
    }
    let areas: Vec<i128> = shells.iter().map(|shell| twice_area(shell)).collect();
    let mut faces: Vec<Face> = shells.into_iter().map(|shell| (shell, Vec::new())).collect();
    for hole in holes {
        let probe = (hole[0].0 + hole[1].0, hole[0].1 + hole[1].1);
        let owner =
            (0..faces.len()).filter(|&index| contains(&faces[index].0, probe)).min_by_key(|&index| areas[index])?;
        faces[owner].1.push(hole);
    }
    Some(faces)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(face: &Face) -> i128 {
        twice_area(&face.0) - face.1.iter().map(|hole| -twice_area(hole)).sum::<i128>()
    }

    fn ring(points: &[I]) -> Vec<Segment> {
        (0..points.len()).map(|i| (points[i], points[(i + 1) % points.len()])).collect()
    }

    #[test]
    fn two_holes_sharing_a_vertex_are_two_holes() {
        let mut lines = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        lines.extend(ring(&[(20, 20), (20, 40), (40, 40), (40, 60), (60, 60), (60, 40), (40, 40), (40, 20)]));
        let faces = odd_faces(&lines).expect("valid");
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].1.len(), 2);
        assert_eq!(area(&faces[0]), 2 * 9200);
        for hole in &faces[0].1 {
            assert_eq!(hole.len(), 4);
        }
    }

    #[test]
    fn a_hole_touching_the_shell_edge_stays_a_hole() {
        let mut lines = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        lines.extend(ring(&[(40, 50), (40, 60), (60, 60), (60, 40), (50, 0), (40, 40)]));
        let faces = odd_faces(&lines).expect("valid");
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].0.len(), 5, "the shell is split at the touching vertex");
        assert_eq!(faces[0].1.len(), 1);
        assert_eq!(area(&faces[0]), 2 * 9200);
    }

    #[test]
    fn faces_meeting_at_a_corner_are_separate() {
        let mut lines = ring(&[(0, 0), (10, 0), (10, 10), (0, 10)]);
        lines.extend(ring(&[(10, 10), (20, 10), (20, 20), (10, 20)]));
        let faces = odd_faces(&lines).expect("valid");
        assert_eq!(faces.len(), 2);
        assert!(faces.iter().all(|face| face.1.is_empty() && area(face) == 200));
    }

    #[test]
    fn an_island_inside_a_hole_is_its_own_face() {
        let mut lines = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        lines.extend(ring(&[(10, 10), (10, 90), (90, 90), (90, 10)]));
        lines.extend(ring(&[(40, 40), (60, 40), (60, 60), (40, 60)]));
        let mut faces = odd_faces(&lines).expect("valid");
        faces.sort_by_key(area);
        assert_eq!(faces.len(), 2);
        assert_eq!((area(&faces[0]), faces[0].1.len()), (2 * 400, 0));
        assert_eq!((area(&faces[1]), faces[1].1.len()), (2 * 3600, 1));
    }

    #[test]
    fn a_hole_pinching_the_shell_at_two_vertices_splits_the_face() {
        // A diamond hole whose left and right corners touch the shell's sides.
        let mut lines = ring(&[(0, 0), (100, 0), (100, 50), (100, 100), (0, 100), (0, 50)]);
        lines.extend(ring(&[(0, 50), (50, 90), (100, 50), (50, 10)]));
        let mut faces = odd_faces(&lines).expect("valid");
        faces.sort_by_key(area);
        assert_eq!(faces.len(), 2);
        assert!(faces.iter().all(|face| face.1.is_empty()));
        assert_eq!(faces.iter().map(area).sum::<i128>(), 2 * (10000 - 4000));
    }

    #[test]
    fn crossing_and_dropped_linework_is_rejected() {
        let mut crossing = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        crossing.extend(ring(&[(20, 20), (50, 20), (50, 50), (20, 50)]));
        crossing.extend(ring(&[(30, 30), (60, 30), (60, 60), (30, 60)]));
        assert!(odd_faces(&crossing).is_none());
        let mut overlap = ring(&[(0, 0), (100, 0), (100, 100), (0, 100)]);
        overlap.extend([((40, 50), (50, 50)), ((50, 50), (60, 50)), ((60, 50), (40, 50))]);
        assert!(odd_faces(&overlap).is_none());
        // A triangle hole crossing a square hole's edges at vertices on them.
        let mut through = ring(&[(0, 0), (80, 0), (80, 80), (0, 80)]);
        through.extend(ring(&[(60, 30), (40, 30), (40, 60), (60, 60)]));
        through.extend(ring(&[(70, 10), (50, 30), (60, 40)]));
        assert!(odd_faces(&through).is_none());
    }

    #[test]
    fn a_zero_area_ring_along_an_edge_bounds_nothing_more() {
        // Back and forth along the shell's bottom edge: the edge is covered
        // three times there, so it still bounds the region once.
        let mut lines = ring(&[(0, 0), (60, 0), (60, 60), (0, 60)]);
        lines.extend([((10, 0), (20, 0)), ((20, 0), (30, 0)), ((30, 0), (10, 0))]);
        let faces = odd_faces(&lines).expect("valid");
        assert_eq!(faces.len(), 1);
        assert!(faces[0].1.is_empty());
        assert_eq!(area(&faces[0]), 2 * 3600);
    }

    #[test]
    fn rings_touching_at_a_shared_apex_are_separate_faces() {
        // Two triangles meeting only at their shared apex: both are filled.
        let mut lines = ring(&[(0, 0), (10, 10), (0, 20)]);
        lines.extend(ring(&[(20, 0), (10, 10), (20, 20)]));
        let faces = odd_faces(&lines).expect("valid");
        assert_eq!(faces.len(), 2);
        assert_eq!(faces.iter().map(area).sum::<i128>(), 2 * 200);
    }
}

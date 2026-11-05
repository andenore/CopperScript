//! A native fill walk with many slit-joined holes decodes to its exact area
//! and every void; ported from `tests/test_native_fill_crossings.py`. The
//! `_EdgeYIndex` parity accelerator cases are not ported (Rust's decoder has
//! no such index). Shapely's `covers(point)` becomes the overlap of a
//! 0.5-unit square around the point.

use copper_route::rf_geometry::{Convex, FillPolygon, NativeFill, native_fill_polygon};

fn ring_area(ring: &[(f64, f64)]) -> f64 {
    let n = ring.len();
    ((0..n).map(|i| ring[i].0 * ring[(i + 1) % n].1 - ring[(i + 1) % n].0 * ring[i].1).sum::<f64>() / 2.0).abs()
}

fn covers(polygon: &FillPolygon, x: f64, y: f64) -> bool {
    let probe = Convex(vec![(x - 0.25, y - 0.25), (x + 0.25, y - 0.25), (x + 0.25, y + 0.25), (x - 0.25, y + 0.25)]);
    polygon.convex_overlap(&probe) > 0.0
}

#[test]
fn many_native_holes_keep_exact_area_and_every_void() {
    let count = 20;
    let anchors: Vec<(i64, i64)> = (0..count).flat_map(|y| (0..count).map(move |x| (3 + 6 * x, 3 + 6 * y))).collect();
    let extent = 6 * count + 2;
    let mut walk = vec![(0, 0), (extent, 0), (extent, extent), (0, extent), (0, 0), anchors[0]];
    for (i, &(x, y)) in anchors.iter().enumerate() {
        walk.extend([(x, y + 2), (x + 2, y + 2), (x + 2, y), (x, y)]);
        if i + 1 < anchors.len() {
            walk.push(anchors[i + 1]);
        }
    }
    walk.extend(anchors[..anchors.len() - 1].iter().rev());
    walk.push((0, 0));
    let NativeFill::Polygon(shape) = native_fill_polygon(&walk).unwrap() else { panic!("one polygon expected") };
    let area = ring_area(&shape.outer) - shape.holes.iter().map(|h| ring_area(h)).sum::<f64>();
    assert_eq!(area, (extent * extent - 4 * anchors.len() as i64) as f64);
    assert_eq!(shape.holes.len(), anchors.len());
    assert!(anchors.iter().all(|&(x, y)| !covers(&shape, (x + 1) as f64, (y + 1) as f64)));
    assert!(covers(&shape, 1.0, 1.0));
}

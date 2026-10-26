//! Prototype Rust mirror of `RoutingClearanceIndex`'s binned copper.
//!
//! The Python index stays the owner of every object. It mirrors each `_add`,
//! `_drop_object` and `_compact` here, and asks this module the two hottest
//! queries: `_clear` (foreign copper clearance) and `pad_copper_clear`.
//! Bins, visiting order and every predicate replicate `pcbir.geometry`
//! exactly with integer arithmetic. A product that would overflow `i128`
//! returns `None`, and the caller then answers in Python.

use pyo3::prelude::*;
use rustc_hash::FxHashMap;

type P = (i64, i64);

struct Obj {
    net: u32,
    layers: Vec<u32>,
    spine: Vec<P>,
    radius: i64,
    clearance: i64, // the object's own clearance_nm floor
    rule: i64,      // net-rule clearance of the object's net, 0 without a rule
    is_pad: bool,
    pad: i64, // pad reference id, -1 for none
    bounds: [i64; 4],
}

fn spine_of(flat: &[i64]) -> PyResult<Vec<P>> {
    if flat.is_empty() || flat.len() % 2 != 0 {
        return Err(pyo3::exceptions::PyValueError::new_err("spine needs x, y pairs"));
    }
    Ok(flat.chunks_exact(2).map(|c| (c[0], c[1])).collect())
}

/// `geometry.bounds(spine).expanded(radius)`.
fn bounds_of(spine: &[P], r: i64) -> [i64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (spine[0].0, spine[0].1, spine[0].0, spine[0].1);
    for &(x, y) in &spine[1..] {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    [x0 - r, y0 - r, x1 + r, y1 + r]
}

/// `Bounds.intersects_expanded`.
fn intersects_expanded(a: &[i64; 4], b: &[i64; 4], d: i64) -> bool {
    !(a[2] + d < b[0] || b[2] < a[0] - d || a[3] + d < b[1] || b[3] < a[1] - d)
}

#[inline]
fn w(v: i64) -> i128 {
    v as i128
}

fn orientation(a: P, b: P, c: P) -> i32 {
    let value = (w(b.0) - w(a.0)) * (w(c.1) - w(a.1)) - (w(b.1) - w(a.1)) * (w(c.0) - w(a.0));
    (value > 0) as i32 - (value < 0) as i32
}

fn point_on_segment(p: P, s: P, e: P) -> bool {
    orientation(s, e, p) == 0
        && s.0.min(e.0) <= p.0
        && p.0 <= s.0.max(e.0)
        && s.1.min(e.1) <= p.1
        && p.1 <= s.1.max(e.1)
}

fn segments_intersect(a: P, b: P, c: P, d: P) -> bool {
    let (ab_c, ab_d) = (orientation(a, b, c), orientation(a, b, d));
    let (cd_a, cd_b) = (orientation(c, d, a), orientation(c, d, b));
    if ab_c != ab_d && cd_a != cd_b {
        return true;
    }
    (ab_c == 0 && point_on_segment(c, a, b))
        || (ab_d == 0 && point_on_segment(d, a, b))
        || (cd_a == 0 && point_on_segment(a, c, d))
        || (cd_b == 0 && point_on_segment(b, c, d))
}

/// `geometry._coordinates_in_polygon` for integer coordinates.
fn point_in_polygon(p: P, polygon: &[P]) -> bool {
    let (x, y) = (w(p.0), w(p.1));
    let n = polygon.len();
    let mut inside = false;
    for i in 0..n {
        let (f, s) = (polygon[i], polygon[(i + 1) % n]);
        let (fx, fy, sx, sy) = (w(f.0), w(f.1), w(s.0), w(s.1));
        if (sx - fx) * (y - fy) == (sy - fy) * (x - fx)
            && fx.min(sx) <= x
            && x <= fx.max(sx)
            && fy.min(sy) <= y
            && y <= fy.max(sy)
        {
            return true;
        }
        if (fy > y) == (sy > y) {
            continue;
        }
        let lhs = (x - fx) * (sy - fy);
        let rhs = (sx - fx) * (y - fy);
        if if sy > fy { lhs < rhs } else { lhs > rhs } {
            inside = !inside;
        }
    }
    inside
}

/// `geometry._point_segment_at_least_squared` with multiplier 1.
fn point_segment_at_least(p: P, s: P, e: P, minimum: i128) -> Option<bool> {
    let (dx, dy) = (w(e.0) - w(s.0), w(e.1) - w(s.1));
    let (px, py) = (w(p.0) - w(s.0), w(p.1) - w(s.1));
    let length = dx * dx + dy * dy;
    let projection = px * dx + py * dy;
    if projection <= 0 {
        return Some(px * px + py * py >= minimum);
    }
    if projection >= length {
        let (qx, qy) = (w(p.0) - w(e.0), w(p.1) - w(e.1));
        return Some(qx * qx + qy * qy >= minimum);
    }
    let cross = px * dy - py * dx;
    Some(cross.checked_mul(cross)? >= minimum.checked_mul(length)?)
}

/// `geometry._segment_at_least_squared` with multiplier 1.
fn segment_at_least(a: P, b: P, c: P, d: P, minimum: i128) -> Option<bool> {
    if minimum == 0 {
        return Some(true);
    }
    if segments_intersect(a, b, c, d) {
        return Some(false);
    }
    Some(
        point_segment_at_least(a, c, d, minimum)?
            && point_segment_at_least(b, c, d, minimum)?
            && point_segment_at_least(c, a, b, minimum)?
            && point_segment_at_least(d, a, b, minimum)?,
    )
}

fn edges(spine: &[P]) -> Vec<(P, P)> {
    if spine.len() == 2 {
        return vec![(spine[0], spine[1])];
    }
    (0..spine.len()).map(|i| (spine[i], spine[(i + 1) % spine.len()])).collect()
}

/// `geometry._point_spine_at_least_squared`.
fn point_spine_at_least(p: P, spine: &[P], minimum: i128) -> Option<bool> {
    if spine.len() == 1 {
        let (dx, dy) = (w(p.0) - w(spine[0].0), w(p.1) - w(spine[0].1));
        return Some(dx * dx + dy * dy >= minimum);
    }
    if spine.len() == 2 {
        return point_segment_at_least(p, spine[0], spine[1], minimum);
    }
    if point_in_polygon(p, spine) {
        return Some(false);
    }
    for (s, e) in edges(spine) {
        if !point_segment_at_least(p, s, e, minimum)? {
            return Some(false);
        }
    }
    Some(true)
}

/// `geometry._spines_at_least_squared`.
fn spines_at_least(first: &[P], second: &[P], minimum: i128) -> Option<bool> {
    if minimum == 0 {
        return Some(true);
    }
    if first.len() == 1 {
        return point_spine_at_least(first[0], second, minimum);
    }
    if second.len() == 1 {
        return point_spine_at_least(second[0], first, minimum);
    }
    if first.len() == 2 && second.len() == 2 {
        return segment_at_least(first[0], first[1], second[0], second[1], minimum);
    }
    if first.len() >= 3 && point_in_polygon(second[0], first) {
        return Some(false);
    }
    if second.len() >= 3 && point_in_polygon(first[0], second) {
        return Some(false);
    }
    for (a, b) in edges(first) {
        for (c, d) in edges(second) {
            if !segment_at_least(a, b, c, d, minimum)? {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// `geometry.shapes_clear(query, other, clearance)`.
fn shapes_clear(spine: &[P], radius: i64, bounds: &[i64; 4], other: &Obj, clearance: i64) -> Option<bool> {
    if !intersects_expanded(bounds, &other.bounds, clearance) {
        return Some(true);
    }
    let required = w(radius) + w(other.radius) + w(clearance);
    spines_at_least(spine, &other.spine, required.checked_mul(required)?)
}

#[pyclass(module = "pcbir.native._clearance_rs")]
struct ClearanceMirror {
    bin: i64,
    objects: Vec<Option<Obj>>,
    bins: FxHashMap<(u32, i64, i64), Vec<u32>>,
    stamp: Vec<u32>,
    epoch: u32,
}

impl ClearanceMirror {
    fn cells(&self, b: &[i64; 4], distance: i64) -> (std::ops::RangeInclusive<i64>, std::ops::RangeInclusive<i64>) {
        let s = self.bin;
        (
            (b[0] - distance).div_euclid(s)..=(b[2] + distance).div_euclid(s),
            (b[1] - distance).div_euclid(s)..=(b[3] + distance).div_euclid(s),
        )
    }

    fn next_epoch(&mut self) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.iter_mut().for_each(|v| *v = 0);
            self.epoch = 1;
        }
        self.epoch
    }

    /// Visit each object in the query's bins once (`nearby_copper` order);
    /// stop at the first `Some(false)` or `None` that `test` returns.
    fn scan<F>(&mut self, layers: &[u32], b: &[i64; 4], distance: i64, mut test: F) -> Option<bool>
    where
        F: FnMut(&Obj) -> Option<bool>,
    {
        let epoch = self.next_epoch();
        let (xs, ys) = self.cells(b, distance);
        for &layer in layers {
            for x in xs.clone() {
                for y in ys.clone() {
                    let Some(ids) = self.bins.get(&(layer, x, y)) else { continue };
                    for &id in ids {
                        let slot = id as usize;
                        if self.stamp[slot] == epoch {
                            continue;
                        }
                        self.stamp[slot] = epoch;
                        let obj = self.objects[slot].as_ref()?;
                        match test(obj) {
                            Some(true) => {}
                            other => return other,
                        }
                    }
                }
            }
        }
        Some(true)
    }
}

#[pymethods]
impl ClearanceMirror {
    #[new]
    fn new(bin_size_nm: i64) -> PyResult<Self> {
        if bin_size_nm <= 0 {
            return Err(pyo3::exceptions::PyValueError::new_err("clearance bin size must be positive"));
        }
        Ok(Self { bin: bin_size_nm, objects: Vec::new(), bins: FxHashMap::default(), stamp: Vec::new(), epoch: 0 })
    }

    /// Mirror `_add`: the object at Python slot `slot`, in the bins of its bounds.
    #[pyo3(signature = (slot, net, layers, spine, radius, clearance, rule, is_pad, pad))]
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, slot: usize, net: u32, layers: Vec<u32>, spine: Vec<i64>, radius: i64,
           clearance: i64, rule: i64, is_pad: bool, pad: i64) -> PyResult<()> {
        let spine = spine_of(&spine)?;
        let bounds = bounds_of(&spine, radius);
        if self.objects.len() <= slot {
            self.objects.resize_with(slot + 1, || None);
            self.stamp.resize(slot + 1, 0);
        }
        let (xs, ys) = self.cells(&bounds, 0);
        for &layer in &layers {
            for x in xs.clone() {
                for y in ys.clone() {
                    self.bins.entry((layer, x, y)).or_default().push(slot as u32);
                }
            }
        }
        self.objects[slot] = Some(Obj { net, layers, spine, radius, clearance, rule, is_pad, pad, bounds });
        Ok(())
    }

    /// Mirror `_drop_object`: undo exactly the bin insertions of `add`.
    fn remove(&mut self, slot: usize) -> PyResult<()> {
        let obj = self
            .objects
            .get_mut(slot)
            .and_then(Option::take)
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(slot))?;
        let (xs, ys) = self.cells(&obj.bounds, 0);
        for &layer in &obj.layers {
            for x in xs.clone() {
                for y in ys.clone() {
                    let key = (layer, x, y);
                    if let Some(ids) = self.bins.get_mut(&key) {
                        if let Some(at) = ids.iter().position(|&id| id as usize == slot) {
                            ids.remove(at);
                        }
                        if ids.is_empty() {
                            self.bins.remove(&key);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.objects.iter().filter(|o| o.is_some()).count()
    }

    /// `RoutingClearanceIndex._clear` without breakout regions.
    ///
    /// `own` is the query net's rule clearance (board minimum without a rule),
    /// `reach` bounds every spacing a rule can require, `distance` is the
    /// index's maximum clearance (the broad-phase expansion). None: overflow.
    #[pyo3(signature = (net, spine, radius, layers, floor, own, reach, distance))]
    #[allow(clippy::too_many_arguments)]
    fn clear(&mut self, net: u32, spine: Vec<i64>, radius: i64, layers: Vec<u32>, floor: i64,
             own: i64, reach: i64, distance: i64) -> PyResult<Option<bool>> {
        let spine = spine_of(&spine)?;
        let b = bounds_of(&spine, radius);
        Ok(self.scan(&layers, &b, distance, |other| {
            if other.net == net {
                return Some(true);
            }
            let margin = reach.max(other.clearance);
            if b[2] + margin < other.bounds[0] || other.bounds[2] < b[0] - margin
                || b[3] + margin < other.bounds[1] || other.bounds[3] < b[1] - margin {
                return Some(true);
            }
            let clearance = own.max(other.rule).max(floor).max(other.clearance);
            shapes_clear(&spine, radius, &b, other, clearance)
        }))
    }

    /// `RoutingClearanceIndex.pad_copper_clear`; `allowed` is a pad id or -1.
    #[pyo3(signature = (spine, radius, layers, allowed, distance))]
    fn pad_clear(&mut self, spine: Vec<i64>, radius: i64, layers: Vec<u32>, allowed: i64,
                 distance: i64) -> PyResult<Option<bool>> {
        let spine = spine_of(&spine)?;
        let b = bounds_of(&spine, radius);
        Ok(self.scan(&layers, &b, distance, |other| {
            if !other.is_pad || (allowed >= 0 && other.pad == allowed) {
                return Some(true);
            }
            shapes_clear(&spine, radius, &b, other, 1)
        }))
    }
}

#[pymodule]
fn _clearance_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ClearanceMirror>()?;
    Ok(())
}

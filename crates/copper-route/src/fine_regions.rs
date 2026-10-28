//! Fine-patch planning for the multi-resolution search lattice
//! (`pcbir.fine_regions`).
//!
//! A coarse base lattice over the board plus rectangular fine patches at
//! `base_pitch // k` around the packages whose pins need them: every full
//! ball array, and with `dense_packages` every package with at least
//! `minimum_pads` pins closer than `pad_pitch_threshold_nm`. Pure planning;
//! the nested lattice is built from the plan.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use num_integer::Integer;

use copper_json::{Object, Value};
use copper_pcb::board::{PhysicalBoard, Placement};
use copper_pcb::transform::transformed_local_point;
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint};
use copper_physical::pyfloat::{round_digits, round_int};
use copper_physical::{CopperLayer, Point};
use copper_units::Decimal;

use crate::package_patterns::{CircularArray, circular_array};
use crate::unified_error::{Result, fail};

const COPPER_KINDS: [PadKind; 2] = [PadKind::Smd, PadKind::ThroughHole];

fn mm_text(value_nm: i64) -> String {
    let value = Decimal::from_i64(value_nm).div(&Decimal::from_i64(1_000_000)).expect("nonzero divisor");
    value.decimal_text() + " mm"
}

fn mm(value_nm: i64) -> f64 {
    value_nm as f64 / 1_000_000.0
}

fn decimal_f64(value: &Decimal) -> f64 {
    value.to_sci_string().parse().expect("finite decimal")
}

/// `LatticeBounds`: origin and node counts of a uniform lattice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatticeBounds {
    pub pitch_nm: i64,
    pub x0_nm: i64,
    pub y0_nm: i64,
    pub nx: i64,
    pub ny: i64,
}

impl LatticeBounds {
    pub fn x1_nm(&self) -> i64 {
        self.x0_nm + (self.nx - 1) * self.pitch_nm
    }

    pub fn y1_nm(&self) -> i64 {
        self.y0_nm + (self.ny - 1) * self.pitch_nm
    }

    /// Nodes per copper layer.
    pub fn plane(&self) -> i64 {
        self.nx * self.ny
    }
}

/// `FinePatch`: one rectangle refined to `base_pitch // factor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinePatch {
    pub x0_nm: i64,
    pub y0_nm: i64,
    pub x1_nm: i64,
    pub y1_nm: i64,
    pub factor: i64,
    pub references: Vec<String>,
    pub reason: String,
    /// `None` means every copper layer.
    pub layers: Option<Vec<CopperLayer>>,
    pub waste_nodes: i64,
}

impl FinePatch {
    pub fn pitch_nm(&self, base_pitch_nm: i64) -> i64 {
        base_pitch_nm.div_euclid(self.factor)
    }

    pub fn cells(&self, base_pitch_nm: i64) -> (i64, i64) {
        ((self.x1_nm - self.x0_nm).div_euclid(base_pitch_nm), (self.y1_nm - self.y0_nm).div_euclid(base_pitch_nm))
    }

    pub fn extra_nodes_per_layer(&self, base_pitch_nm: i64) -> i64 {
        let (cx, cy) = self.cells(base_pitch_nm);
        let k = self.factor;
        (cx * k + 1) * (cy * k + 1) - (cx + 1) * (cy + 1)
    }
}

/// `FineRegionOptions`: knobs of the fine-patch planner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FineRegionOptions {
    pub pad_pitch_threshold_nm: i64,
    pub margin_nm: i64,
    pub maximum_factor: i64,
    pub merge_gap_nm: i64,
    pub minimum_area_nm2: i64,
    pub minimum_pads: i64,
    pub enabled: bool,
    pub dense_packages: bool,
}

impl Default for FineRegionOptions {
    fn default() -> Self {
        FineRegionOptions {
            pad_pitch_threshold_nm: 650_000,
            margin_nm: 1_500_000,
            maximum_factor: 8,
            merge_gap_nm: 500_000,
            minimum_area_nm2: 1_000_000 * 1_000_000,
            minimum_pads: 8,
            enabled: true,
            dense_packages: false,
        }
    }
}

impl FineRegionOptions {
    /// `__post_init__` (the boolean type check is the decoder's).
    pub fn validated(self) -> Result<Self> {
        if self.pad_pitch_threshold_nm <= 0 {
            return fail("fine-region pad pitch threshold must be positive");
        }
        if self.margin_nm < 0 || self.merge_gap_nm < 0 || self.minimum_area_nm2 < 0 {
            return fail("fine-region margin, merge gap and minimum area cannot be negative");
        }
        if self.maximum_factor < 2 {
            return fail("fine-region maximum factor must be at least 2");
        }
        if self.minimum_pads < 2 {
            return fail("fine-region minimum pad count must be at least 2");
        }
        Ok(self)
    }
}

/// `FineRegionPlan`: fine patches for one board and base pitch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FineRegionPlan {
    pub base_pitch_nm: i64,
    pub patches: Vec<FinePatch>,
    pub base_nodes: i64,
    pub fine_nodes: i64,
    pub uniform_nodes_at_finest: i64,
    pub layer_count: i64,
    pub skipped: Vec<(String, String)>,
}

impl FineRegionPlan {
    pub fn finest_pitch_nm(&self) -> i64 {
        let finest = self.patches.iter().map(|p| p.factor).max().unwrap_or(1);
        self.base_pitch_nm.div_euclid(finest)
    }

    pub fn total_nodes(&self) -> i64 {
        self.base_nodes + self.fine_nodes
    }

    /// `report()`: node counts and one row per patch, in mm.
    pub fn report(&self) -> Object {
        let total = self.total_nodes();
        let int = |value: i64| Value::Int(value.into());
        let ratio = if total != 0 { round_digits(self.uniform_nodes_at_finest as f64 / total as f64, 3) } else { 0.0 };
        let patches = self
            .patches
            .iter()
            .map(|p| {
                let layers = match &p.layers {
                    None => Value::from("all"),
                    Some(layers) => Value::Array(layers.iter().map(|l| Value::from(l.value())).collect()),
                };
                Value::Object(Object::from([
                    (
                        "references".to_owned(),
                        Value::Array(p.references.iter().map(|r| Value::from(r.as_str())).collect()),
                    ),
                    ("reason".to_owned(), Value::from(p.reason.as_str())),
                    ("factor".to_owned(), int(p.factor)),
                    ("pitch_mm".to_owned(), Value::Float(mm(p.pitch_nm(self.base_pitch_nm)))),
                    ("x0_mm".to_owned(), Value::Float(mm(p.x0_nm))),
                    ("y0_mm".to_owned(), Value::Float(mm(p.y0_nm))),
                    ("x1_mm".to_owned(), Value::Float(mm(p.x1_nm))),
                    ("y1_mm".to_owned(), Value::Float(mm(p.y1_nm))),
                    ("width_mm".to_owned(), Value::Float(mm(p.x1_nm - p.x0_nm))),
                    ("height_mm".to_owned(), Value::Float(mm(p.y1_nm - p.y0_nm))),
                    ("layers".to_owned(), layers),
                    ("extra_nodes".to_owned(), int(p.extra_nodes_per_layer(self.base_pitch_nm) * self.layer_count)),
                    ("waste_nodes".to_owned(), int(p.waste_nodes)),
                ]))
            })
            .collect();
        let skipped = self
            .skipped
            .iter()
            .map(|(reference, why)| {
                Value::Object(Object::from([
                    ("reference".to_owned(), Value::from(reference.as_str())),
                    ("reason".to_owned(), Value::from(why.as_str())),
                ]))
            })
            .collect();
        Object::from([
            ("base_pitch_mm".to_owned(), Value::Float(mm(self.base_pitch_nm))),
            ("finest_pitch_mm".to_owned(), Value::Float(mm(self.finest_pitch_nm()))),
            ("layers".to_owned(), int(self.layer_count)),
            ("base_nodes".to_owned(), int(self.base_nodes)),
            ("fine_nodes".to_owned(), int(self.fine_nodes)),
            ("total_nodes".to_owned(), int(total)),
            ("uniform_nodes_at_finest".to_owned(), int(self.uniform_nodes_at_finest)),
            ("uniform_to_nested_ratio".to_owned(), Value::Float(ratio)),
            ("patches".to_owned(), Value::Array(patches)),
            ("skipped".to_owned(), Value::Array(skipped)),
        ])
    }
}

/// `base_lattice_bounds`: `_Grid`'s origin and node counts over the outline.
pub fn base_lattice_bounds(board: &PhysicalBoard, base_pitch_nm: i64) -> Result<LatticeBounds> {
    if base_pitch_nm <= 0 {
        return fail("lattice pitch must be positive");
    }
    let vertices = &board.outline.vertices;
    let min_x = vertices.iter().map(|p| p.x_nm).min().expect("outline vertices");
    let min_y = vertices.iter().map(|p| p.y_nm).min().expect("outline vertices");
    let max_x = vertices.iter().map(|p| p.x_nm).max().expect("outline vertices");
    let max_y = vertices.iter().map(|p| p.y_nm).max().expect("outline vertices");
    let x0 = min_x.div_euclid(base_pitch_nm) * base_pitch_nm;
    let y0 = min_y.div_euclid(base_pitch_nm) * base_pitch_nm;
    Ok(LatticeBounds {
        pitch_nm: base_pitch_nm,
        x0_nm: x0,
        y0_nm: y0,
        nx: (max_x - x0).div_euclid(base_pitch_nm) + 2,
        ny: (max_y - y0).div_euclid(base_pitch_nm) + 2,
    })
}

/// `candidate_factors`: k >= 2 dividing the base pitch, coarsest first.
pub fn candidate_factors(base_pitch_nm: i64, maximum_factor: i64) -> Vec<i64> {
    (2..=maximum_factor).filter(|k| base_pitch_nm.rem_euclid(*k) == 0).collect()
}

/// `_Spacing`: footprint-local pin spacing.
#[derive(Clone, Debug)]
struct Spacing {
    spacing_nm: i64,
    dense_pads: Vec<usize>,
    pairs: Vec<(usize, usize)>,
    pin_count: i64,
}

fn local_spacing(footprint: &PhysicalFootprint, threshold_nm: i64) -> Option<Spacing> {
    let pads: Vec<(usize, &FootprintPad)> =
        footprint.pads.iter().enumerate().filter(|(_, pad)| COPPER_KINDS.contains(&pad.kind)).collect();
    if pads.len() < 2 {
        return None;
    }
    let mut buckets: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, pad) in &pads {
        let key = (pad.position.x_nm.div_euclid(threshold_nm), pad.position.y_nm.div_euclid(threshold_nm));
        buckets.entry(key).or_default().push(*i);
    }
    let limit = threshold_nm as i128 * threshold_nm as i128;
    let mut pairs = Vec::new();
    let mut dense = BTreeSet::new();
    let mut best: Option<i128> = None;
    for (&(bx, by), members) in &buckets {
        for dx in [-1, 0, 1] {
            for dy in [-1, 0, 1] {
                let Some(neighbours) = buckets.get(&(bx + dx, by + dy)) else { continue };
                for &j in neighbours {
                    for &i in members {
                        if j <= i {
                            continue;
                        }
                        let (a, b) = (&footprint.pads[i], &footprint.pads[j]);
                        if a.number == b.number {
                            continue;
                        }
                        let ddx = (a.position.x_nm - b.position.x_nm) as i128;
                        let ddy = (a.position.y_nm - b.position.y_nm) as i128;
                        let d2 = ddx * ddx + ddy * ddy;
                        if d2 >= limit {
                            continue;
                        }
                        pairs.push((i, j));
                        dense.insert(i);
                        dense.insert(j);
                        best = Some(best.map_or(d2, |b| b.min(d2)));
                    }
                }
            }
        }
    }
    let best = best?;
    let pins: BTreeSet<&str> = dense.iter().map(|&i| footprint.pads[i].number.as_str()).collect();
    pairs.sort_unstable();
    Some(Spacing {
        spacing_nm: round_int((best as f64).powf(0.5)),
        dense_pads: dense.into_iter().collect(),
        pairs,
        pin_count: pins.len() as i64,
    })
}

/// `_pad_extent`: board-frame bounding box of one pad's copper.
fn pad_extent(pose: &Placement, pad: &FootprintPad) -> (i64, i64, i64, i64) {
    if pad.shape == PadShape::Circle {
        let centre = transformed_local_point(pose, pad.position);
        let r = pad.size.width_nm.max(pad.size.height_nm).div_euclid(2);
        return (centre.x_nm - r, centre.y_nm - r, centre.x_nm + r, centre.y_nm + r);
    }
    let (hw, hh) = (pad.size.width_nm as f64 / 2.0, pad.size.height_nm as f64 / 2.0);
    let angle = (-decimal_f64(&pad.rotation_degrees)).to_radians();
    let (c, s) = (angle.cos(), angle.sin());
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (ox, oy) in [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
        let local =
            Point::new(pad.position.x_nm + round_int(ox * c - oy * s), pad.position.y_nm + round_int(ox * s + oy * c));
        let point = transformed_local_point(pose, local);
        xs.push(point.x_nm);
        ys.push(point.y_nm);
    }
    (*xs.iter().min().unwrap(), *ys.iter().min().unwrap(), *xs.iter().max().unwrap(), *ys.iter().max().unwrap())
}

fn array_points(array: &CircularArray, pose: &Placement) -> (IndexMap<usize, Point>, Vec<(usize, usize)>) {
    let mut points = IndexMap::new();
    let mut pairs = Vec::new();
    let columns = array.xs_nm.len();
    for (row, &y) in array.ys_nm.iter().enumerate() {
        for (column, &x) in array.xs_nm.iter().enumerate() {
            let index = row * columns + column;
            points.insert(index, transformed_local_point(pose, Point::new(x, y)));
            if column != 0 {
                pairs.push((index - 1, index));
            }
            if row != 0 {
                pairs.push((index - columns, index));
            }
        }
    }
    (points, pairs)
}

/// `array_alignment`: whether every placed ball centre and every adjacent
/// midpoint is a multiple of `fine_nm` on both axes.
pub fn array_alignment(array: &CircularArray, pose: &Placement) -> impl Fn(i64) -> bool {
    let (balls, adjacent) = array_points(array, pose);
    move |fine| dense_aligned(&balls, &adjacent, fine)
}

fn dense_aligned(points: &IndexMap<usize, Point>, pairs: &[(usize, usize)], fine: i64) -> bool {
    if points.values().any(|p| p.x_nm.rem_euclid(fine) != 0 || p.y_nm.rem_euclid(fine) != 0) {
        return false;
    }
    let double = 2 * fine;
    pairs.iter().all(|(i, j)| {
        let (a, b) = (points[i], points[j]);
        (a.x_nm + b.x_nm).rem_euclid(double) == 0 && (a.y_nm + b.y_nm).rem_euclid(double) == 0
    })
}

#[derive(Clone, Debug)]
struct Need {
    reference: String,
    factor: i64,
    reason: String,
    box_: (i64, i64, i64, i64),
}

type SpacingCache = IndexMap<(String, i64), Option<Spacing>>;

fn package_need(
    board: &PhysicalBoard,
    pose: &Placement,
    base: i64,
    options: &FineRegionOptions,
    spacing_cache: &mut SpacingCache,
) -> (Option<Need>, Option<String>) {
    let footprint = &board.footprints[&pose.footprint];
    let array = circular_array(board, &pose.reference).ok().flatten();
    let (spacing, aligned, kind): (i64, Box<dyn Fn(i64) -> bool>, String) = if let Some(array) = &array {
        let kind = format!("{}x{} ball array at {}", array.xs_nm.len(), array.ys_nm.len(), mm_text(array.pitch_nm));
        (array.pitch_nm, Box::new(array_alignment(array, pose)), kind)
    } else {
        let key = (pose.footprint.clone(), options.pad_pitch_threshold_nm);
        let local = spacing_cache
            .entry(key)
            .or_insert_with(|| local_spacing(footprint, options.pad_pitch_threshold_nm))
            .clone();
        let Some(local) = local.filter(|local| local.pin_count >= options.minimum_pads) else {
            return (None, None);
        };
        let points: IndexMap<usize, Point> =
            local.dense_pads.iter().map(|&i| (i, transformed_local_point(pose, footprint.pads[i].position))).collect();
        let kind = format!("{} pins at {}", local.pin_count, mm_text(local.spacing_nm));
        let pairs = local.pairs.clone();
        (local.spacing_nm, Box::new(move |fine| dense_aligned(&points, &pairs, fine)), kind)
    };
    let extents: Vec<_> =
        footprint.pads.iter().filter(|pad| COPPER_KINDS.contains(&pad.kind)).map(|pad| pad_extent(pose, pad)).collect();
    let box_ = (
        extents.iter().map(|e| e.0).min().unwrap(),
        extents.iter().map(|e| e.1).min().unwrap(),
        extents.iter().map(|e| e.2).max().unwrap(),
        extents.iter().map(|e| e.3).max().unwrap(),
    );
    if ((box_.2 - box_.0) as i128) * ((box_.3 - box_.1) as i128) < options.minimum_area_nm2 as i128 {
        return (None, Some(format!("{kind}: pad field below the minimum area")));
    }
    if aligned(base) {
        return (None, Some(format!("{kind}: aligned at the base pitch")));
    }
    let factors = candidate_factors(base, options.maximum_factor);
    if factors.is_empty() {
        return (None, Some(format!("{kind}: no factor up to {} divides the base pitch", options.maximum_factor)));
    }
    let mut need = factors.iter().find(|&&k| aligned(base.div_euclid(k))).map(|&k| Need {
        reference: pose.reference.clone(),
        factor: k,
        reason: format!("{kind}: k={k} puts nodes on pin centres and channel centre lines"),
        box_,
    });
    if need.is_none() {
        let k =
            factors.iter().copied().find(|k| 2 * base.div_euclid(*k) <= spacing).unwrap_or(*factors.last().unwrap());
        let reason = if 2 * base.div_euclid(k) <= spacing {
            format!(
                "{kind}: no exact k up to {}; fallback k={k}, pitch {} at most half the spacing",
                options.maximum_factor,
                mm_text(base.div_euclid(k))
            )
        } else {
            format!("{kind}: no exact k up to {}; fallback to the finest k={k}", options.maximum_factor)
        };
        need = Some(Need { reference: pose.reference.clone(), factor: k, reason, box_ });
    }
    let need = need.expect("need");
    if array.is_none() && !options.dense_packages {
        return (
            None,
            Some(format!(
                "{kind}: would get k={}; dense packages are refined only with dense_packages (fine_regions=\"dense\")",
                need.factor
            )),
        );
    }
    (Some(need), None)
}

type Rect = (i64, i64, i64, i64);

fn snap_out(box_: Rect, lattice: &LatticeBounds, clip: Rect) -> Option<Rect> {
    let p = lattice.pitch_nm;
    let x0 = lattice.x0_nm + (box_.0 - lattice.x0_nm).div_euclid(p) * p;
    let y0 = lattice.y0_nm + (box_.1 - lattice.y0_nm).div_euclid(p) * p;
    let x1 = lattice.x0_nm - (lattice.x0_nm - box_.2).div_euclid(p) * p;
    let y1 = lattice.y0_nm - (lattice.y0_nm - box_.3).div_euclid(p) * p;
    let (x0, y0, x1, y1) = (x0.max(clip.0), y0.max(clip.1), x1.min(clip.2), y1.min(clip.3));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

fn gap(a: Rect, b: Rect) -> i64 {
    0.max(b.0 - a.2).max(a.0 - b.2).max(b.1 - a.3).max(a.1 - b.3)
}

fn union_cells(rects: &[Rect], pitch: i64) -> i64 {
    let xs: Vec<i64> = rects.iter().flat_map(|r| [r.0, r.2]).collect::<BTreeSet<_>>().into_iter().collect();
    let ys: Vec<i64> = rects.iter().flat_map(|r| [r.1, r.3]).collect::<BTreeSet<_>>().into_iter().collect();
    let mut cells = 0;
    for xw in xs.windows(2) {
        for yw in ys.windows(2) {
            let (xa, xb, ya, yb) = (xw[0], xw[1], yw[0], yw[1]);
            if rects.iter().any(|r| r.0 <= xa && xb <= r.2 && r.1 <= ya && yb <= r.3) {
                cells += (xb - xa).div_euclid(pitch) * (yb - ya).div_euclid(pitch);
            }
        }
    }
    cells
}

#[derive(Clone, Debug)]
struct Group {
    rect: Rect,
    needs: Vec<(Need, Rect)>,
}

fn merge(mut groups: Vec<Group>, gap_nm: i64) -> Vec<Group> {
    'outer: loop {
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let (a, b) = (groups[i].rect, groups[j].rect);
                if gap(a, b) <= gap_nm {
                    let mut needs = groups[i].needs.clone();
                    needs.extend(groups[j].needs.iter().cloned());
                    groups[i] = Group { rect: (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)), needs };
                    groups.remove(j);
                    continue 'outer;
                }
            }
        }
        return groups;
    }
}

fn group_factor(needs: &[(Need, Rect)], base: i64, maximum: i64) -> (i64, String) {
    let factors: Vec<i64> = needs.iter().map(|(need, _)| need.factor).collect::<BTreeSet<_>>().into_iter().collect();
    let mut lcm: i64 = 1;
    for &k in &factors {
        lcm = lcm * k / lcm.gcd(&k);
    }
    if lcm <= maximum && base.rem_euclid(lcm) == 0 {
        return (lcm, String::new());
    }
    let k = *factors.last().expect("factors");
    let mut lost: Vec<&str> = needs
        .iter()
        .filter(|(need, _)| k.rem_euclid(need.factor) != 0)
        .map(|(need, _)| need.reference.as_str())
        .collect();
    lost.sort_unstable();
    (
        k,
        format!(
            "; lcm {lcm} exceeds the maximum factor or the base pitch, using k={k}: {} lose exact alignment",
            lost.join(", ")
        ),
    )
}

/// `plan_fine_regions`: fine lattice patches around the packages whose pins need them.
pub fn plan_fine_regions(
    board: &PhysicalBoard,
    base_pitch_nm: i64,
    options: Option<&FineRegionOptions>,
) -> Result<FineRegionPlan> {
    let defaults = FineRegionOptions::default();
    let options = options.unwrap_or(&defaults);
    let lattice = base_lattice_bounds(board, base_pitch_nm)?;
    let layers = board.stackup.copper_layers.len() as i64;
    let base_nodes = lattice.plane() * layers;
    if !options.enabled {
        return Ok(FineRegionPlan {
            base_pitch_nm,
            patches: Vec::new(),
            base_nodes,
            fine_nodes: 0,
            uniform_nodes_at_finest: base_nodes,
            layer_count: layers,
            skipped: Vec::new(),
        });
    }
    let vertices = &board.outline.vertices;
    let p = base_pitch_nm;
    let max_x = vertices.iter().map(|v| v.x_nm).max().expect("outline vertices");
    let max_y = vertices.iter().map(|v| v.y_nm).max().expect("outline vertices");
    let clip = (
        lattice.x0_nm,
        lattice.y0_nm,
        lattice.x0_nm - (lattice.x0_nm - max_x).div_euclid(p) * p,
        lattice.y0_nm - (lattice.y0_nm - max_y).div_euclid(p) * p,
    );
    let mut groups = Vec::new();
    let mut skipped = Vec::new();
    let mut cache = SpacingCache::new();
    let m = options.margin_nm;
    let mut poses: Vec<&Placement> = board.placements.iter().collect();
    poses.sort_by(|a, b| a.reference.cmp(&b.reference));
    for pose in poses {
        let (need, why) = package_need(board, pose, base_pitch_nm, options, &mut cache);
        let Some(need) = need else {
            if let Some(why) = why {
                skipped.push((pose.reference.clone(), why));
            }
            continue;
        };
        let b = need.box_;
        let Some(rect) = snap_out((b.0 - m, b.1 - m, b.2 + m, b.3 + m), &lattice, clip) else {
            skipped.push((pose.reference.clone(), "outside the board outline".to_owned()));
            continue;
        };
        groups.push(Group { rect, needs: vec![(need, rect)] });
    }
    let mut patches = Vec::new();
    for group in merge(groups, options.merge_gap_nm) {
        let (k, note) = group_factor(&group.needs, base_pitch_nm, options.maximum_factor);
        let rects: Vec<Rect> = group.needs.iter().map(|(_, rect)| *rect).collect();
        let (x0, y0, x1, y1) = group.rect;
        let cells = (x1 - x0).div_euclid(p) * (y1 - y0).div_euclid(p);
        let waste = (cells - union_cells(&rects, p)) * k * k * layers;
        let mut references: Vec<String> = group.needs.iter().map(|(need, _)| need.reference.clone()).collect();
        references.sort();
        let mut ordered: Vec<&Need> = group.needs.iter().map(|(need, _)| need).collect();
        ordered.sort_by(|a, b| a.reference.cmp(&b.reference));
        let mut reasons =
            ordered.iter().map(|need| format!("{}: {}", need.reference, need.reason)).collect::<Vec<_>>().join("; ");
        if group.needs.len() > 1 {
            reasons =
                format!("merged {} packages within {}; {reasons}", group.needs.len(), mm_text(options.merge_gap_nm));
        }
        patches.push(FinePatch {
            x0_nm: x0,
            y0_nm: y0,
            x1_nm: x1,
            y1_nm: y1,
            factor: k,
            references,
            reason: reasons + &note,
            layers: None,
            waste_nodes: waste,
        });
    }
    patches.sort_by(|a, b| a.references.cmp(&b.references));
    let fine_nodes = patches.iter().map(|patch| patch.extra_nodes_per_layer(p)).sum::<i64>() * layers;
    let finest = patches.iter().map(|patch| patch.factor).max().unwrap_or(1);
    let uniform =
        if finest > 1 { base_lattice_bounds(board, p.div_euclid(finest))?.plane() * layers } else { base_nodes };
    Ok(FineRegionPlan {
        base_pitch_nm: p,
        patches,
        base_nodes,
        fine_nodes,
        uniform_nodes_at_finest: uniform,
        layer_count: layers,
        skipped,
    })
}

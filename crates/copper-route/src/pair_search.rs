//! Bounded same-layer joint package escape and octilinear pair search
//! (`pcbir.pair_search`).
//!
//! Both lanes follow one oriented spine. Pad fanout is selected jointly and
//! turns use offset-line intersections, not independent D+/D- maze routes.
//! This module proposes copper only: the critical stage owns profile and
//! native-DRC acceptance.

use copper_json::{Object, Value};
use copper_physical::{CopperLayer, Point};

use crate::pair_terminals::blocker_document;
use crate::routing_clearance::RoutingBlocker;

/// `PairPortExample`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPortExample {
    pub center: Point,
    pub heading: i64,
    pub sign: i64,
    pub can_depart: bool,
}

/// `PairPortInventory`: the joint ports formed at one package side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairPortInventory {
    pub component: String,
    pub layer: CopperLayer,
    pub pitch_nm: i64,
    pub port_count: i64,
    pub departable_count: i64,
    pub examples: Vec<PairPortExample>,
}

/// `PairTransitionExample`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairTransitionExample {
    pub stage: String,
    pub positions: Vec<Point>,
    pub blockers: Vec<RoutingBlocker>,
    pub local_expanded_states: i64,
    pub local_discovered_nodes: i64,
}

/// `PairTransitionInventory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairTransitionInventory {
    pub component: String,
    pub surface: CopperLayer,
    pub middle_layer: CopperLayer,
    pub pitch_nm: i64,
    pub transitions: i64,
    pub direct_contacts: i64,
    pub stage_counts: Vec<(String, i64)>,
    pub examples: Vec<PairTransitionExample>,
}

/// `PairSearchAttempt`: one bounded continuation of the coupled search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairSearchAttempt {
    pub layer: CopperLayer,
    pub pitch_nm: i64,
    pub start: Point,
    pub start_heading: i64,
    pub start_sign: i64,
    pub end: Point,
    pub end_heading: i64,
    pub end_sign: i64,
    pub expanded_states: i64,
    pub legal_advances: i64,
    pub minimum_goal_distance_nm: Option<i64>,
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Object>())
}

/// `asdict(Point)`.
pub fn point_document(point: Point) -> Value {
    object(vec![("x_nm", Value::from(point.x_nm)), ("y_nm", Value::from(point.y_nm))])
}

/// `asdict(PairPortInventory)`.
pub fn port_inventory_document(item: &PairPortInventory) -> Value {
    object(vec![
        ("component", Value::from(item.component.as_str())),
        ("layer", Value::from(item.layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("port_count", Value::from(item.port_count)),
        ("departable_count", Value::from(item.departable_count)),
        (
            "examples",
            Value::Array(
                item.examples
                    .iter()
                    .map(|example| {
                        object(vec![
                            ("center", point_document(example.center)),
                            ("heading", Value::from(example.heading)),
                            ("sign", Value::from(example.sign)),
                            ("can_depart", Value::from(example.can_depart)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `asdict(PairTransitionInventory)`.
pub fn transition_inventory_document(item: &PairTransitionInventory) -> Value {
    object(vec![
        ("component", Value::from(item.component.as_str())),
        ("surface", Value::from(item.surface.value())),
        ("middle_layer", Value::from(item.middle_layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("transitions", Value::from(item.transitions)),
        ("direct_contacts", Value::from(item.direct_contacts)),
        (
            "stage_counts",
            Value::Array(
                item.stage_counts
                    .iter()
                    .map(|(stage, count)| Value::Array(vec![Value::from(stage.as_str()), Value::from(*count)]))
                    .collect(),
            ),
        ),
        (
            "examples",
            Value::Array(
                item.examples
                    .iter()
                    .map(|example| {
                        object(vec![
                            ("stage", Value::from(example.stage.as_str())),
                            ("positions", Value::Array(example.positions.iter().map(|p| point_document(*p)).collect())),
                            ("blockers", Value::Array(example.blockers.iter().map(blocker_document).collect())),
                            ("local_expanded_states", Value::from(example.local_expanded_states)),
                            ("local_discovered_nodes", Value::from(example.local_discovered_nodes)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `asdict(PairSearchAttempt)`.
pub fn attempt_document(item: &PairSearchAttempt) -> Value {
    object(vec![
        ("layer", Value::from(item.layer.value())),
        ("pitch_nm", Value::from(item.pitch_nm)),
        ("start", point_document(item.start)),
        ("start_heading", Value::from(item.start_heading)),
        ("start_sign", Value::from(item.start_sign)),
        ("end", point_document(item.end)),
        ("end_heading", Value::from(item.end_heading)),
        ("end_sign", Value::from(item.end_sign)),
        ("expanded_states", Value::from(item.expanded_states)),
        ("legal_advances", Value::from(item.legal_advances)),
        ("minimum_goal_distance_nm", item.minimum_goal_distance_nm.map_or(Value::Null, Value::from)),
    ])
}

// ---------------------------------------------------------------------------
// Joint escape and spine search

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use indexmap::IndexMap;

use copper_pcb::board::{NetRoutingRule, PhysicalBoard, TrackSegment};
use copper_physical::RoundedConvexShape;
use copper_physical::geometry::shapes_clear;
use copper_physical::pyfloat::{hypot, round_int};

use crate::Result;
use crate::routing::GlobalNetRoute;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::track_inside_board;

/// `_HEADS`: the eight octilinear headings, counter-clockwise from +x.
pub const HEADS: [(i64, i64); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
/// `_PORT_LENGTHS` in nanometres (0.5, 0.75, 1, 1.5, 2 and 3 mm).
pub const PORT_LENGTHS_NM: [i64; 6] = [500_000, 750_000, 1_000_000, 1_500_000, 2_000_000, 3_000_000];

/// `_Port`: a joint package escape ending at `center` along `heading`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub center: Point,
    /// Outward from the package.
    pub heading: usize,
    /// The first member's side relative to the outward heading.
    pub sign: i64,
    pub first: Vec<TrackSegment>,
    pub second: Vec<TrackSegment>,
    pub preference: i64,
}

/// `PairSearchCandidate`.
#[derive(Clone, Debug)]
pub struct PairSearchCandidate {
    pub first: Vec<TrackSegment>,
    pub second: Vec<TrackSegment>,
    pub expanded_states: i64,
    pub search_index: i64,
    pub spine: Vec<Point>,
    pub start_port: Option<Port>,
    pub end_port: Option<Port>,
    pub via_pairs: Vec<(copper_pcb::board::Via, copper_pcb::board::Via)>,
    pub return_vias: Vec<copper_pcb::board::Via>,
}

fn pt(x: i64, y: i64) -> Point {
    Point::new(x, y)
}

/// `_normal`: the lane offset normal of a heading (diagonals rounded up).
pub fn normal(heading: usize, offset: i64) -> Point {
    let (dx, dy) = HEADS[heading];
    let amount = if dx != 0 && dy != 0 { (offset as f64 / 2f64.sqrt()).ceil() as i64 } else { offset };
    pt(-dy * amount, dx * amount)
}

/// `_shift`.
pub fn shift(point: Point, normal: Point, sign: i64) -> Point {
    pt(point.x_nm + sign * normal.x_nm, point.y_nm + sign * normal.y_nm)
}

/// Python's `round(Fraction(numerator, denominator))`: half to even.
fn round_fraction(numerator: i128, denominator: i128) -> i128 {
    let (mut n, mut d) = (numerator, denominator);
    if d < 0 {
        n = -n;
        d = -d;
    }
    let floor = n.div_euclid(d);
    let twice = 2 * n.rem_euclid(d);
    if twice > d || (twice == d && floor % 2 != 0) { floor + 1 } else { floor }
}

/// `_miter`: the lane point at a spine corner, on the offset lines' intersection.
fn miter(point: Point, incoming: usize, outgoing: usize, offset: i64, sign: i64) -> Point {
    let (first, second) = (normal(incoming, offset), normal(outgoing, offset));
    if incoming == outgoing {
        return shift(point, first, sign);
    }
    let (ax, ay) = HEADS[incoming];
    let (bx, by) = HEADS[outgoing];
    let determinant = i128::from(ax * by - ay * bx);
    let numerator = i128::from(sign)
        * (i128::from(second.x_nm - first.x_nm) * i128::from(by)
            - i128::from(second.y_nm - first.y_nm) * i128::from(bx));
    pt(
        point.x_nm + sign * first.x_nm + round_fraction(numerator * i128::from(ax), determinant) as i64,
        point.y_nm + sign * first.y_nm + round_fraction(numerator * i128::from(ay), determinant) as i64,
    )
}

/// `_heading`: the octilinear heading of a segment (2 nm diagonal tolerance).
pub fn heading(start: Point, end: Point) -> Option<usize> {
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    if dx == 0 && dy == 0 {
        return None;
    }
    if dx != 0 && dy != 0 && (dx.abs() - dy.abs()).abs() > 2 {
        return None;
    }
    HEADS.iter().position(|head| *head == (dx.signum(), dy.signum()))
}

fn turn(first: usize, second: usize) -> usize {
    ((first + 8 - second) % 8).min((second + 8 - first) % 8)
}

/// `_paths`: the direct path, or both one-corner octilinear paths.
pub fn paths(start: Point, end: Point) -> Vec<Vec<Point>> {
    if start == end {
        return vec![vec![start]];
    }
    if heading(start, end).is_some() {
        return vec![vec![start, end]];
    }
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    let distance = dx.abs().min(dy.abs());
    let (sx, sy) = (if dx > 0 { 1 } else { -1 }, if dy > 0 { 1 } else { -1 });
    vec![
        vec![start, pt(start.x_nm + sx * distance, start.y_nm + sy * distance), end],
        vec![start, pt(end.x_nm - sx * distance, end.y_nm - sy * distance), end],
    ]
}

/// Shortest straight leg of a mid-way collar jog (`_collar_paths`).
const COLLAR_LEG_NM: i64 = 50_000;

/// `_collar_paths`: `paths` plus a collar that leaves its land straight,
/// takes the 45-degree jog mid-way and finishes straight into the port, when
/// each straight leg is at least `COLLAR_LEG_NM`.
pub fn collar_paths(start: Point, end: Point) -> Vec<Vec<Point>> {
    let mut result = paths(start, end);
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    let (jog, run) = (dx.abs().min(dy.abs()), dx.abs().max(dy.abs()));
    if jog == 0 || run - jog < 2 * COLLAR_LEG_NM {
        return result;
    }
    let (sx, sy) = (if dx > 0 { 1 } else { -1 }, if dy > 0 { 1 } else { -1 });
    let lead = (run - jog).div_euclid(2);
    let first = if dy.abs() > dx.abs() {
        pt(start.x_nm, start.y_nm + sy * lead)
    } else {
        pt(start.x_nm + sx * lead, start.y_nm)
    };
    let second = pt(first.x_nm + sx * jog, first.y_nm + sy * jog);
    result.push(vec![start, first, second, end]);
    result
}

/// `_tracks`: lane copper, cut at breakout regions as it will be emitted.
pub fn tracks(
    net: &str,
    points: &[Point],
    width: i64,
    layer: CopperLayer,
    breakout: Option<&copper_drc::breakout::BreakoutRegions>,
) -> Vec<TrackSegment> {
    let tracks: Vec<TrackSegment> = points
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .map(|pair| TrackSegment { net: net.to_owned(), start: pair[0], end: pair[1], width_nm: width, layer })
        .collect();
    match breakout {
        Some(breakout) if breakout.any() => breakout.split_tracks(&tracks, Some(width)),
        _ => tracks,
    }
}

fn capsule(track: &TrackSegment) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(vec![track.start, track.end], track.width_nm.div_euclid(2))?)
}

/// `_legal`: every lane piece octilinear, inside the board and clear, and
/// the members spaced by the clearance (the pair gap for a breakout pair).
pub fn legal(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    first: &[TrackSegment],
    second: &[TrackSegment],
    clearance: i64,
) -> Result<bool> {
    for track in first.iter().chain(second) {
        if heading(track.start, track.end).is_none()
            || !track_inside_board(board, track.start, track.end, track.width_nm)?
            || !index.can_track(&track.net, track.start, track.end, track.width_nm, track.layer)?
        {
            return Ok(false);
        }
    }
    let breakout = &index.breakout;
    let pair = !first.is_empty() && !second.is_empty() && breakout.pair(&first[0].net, &second[0].net);
    for a in first {
        for b in second {
            if a.layer != b.layer {
                continue;
            }
            let spacing = if pair {
                let a_land = breakout.region(&a.net, &[a.start, a.end]);
                let b_land = breakout.region(&b.net, &[b.start, b.end]);
                breakout.spacing_nm(&a.net, a_land.as_deref(), &b.net, b_land.as_deref())
            } else {
                clearance
            };
            if !shapes_clear(&capsule(a)?, &capsule(b)?, spacing) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Python's `ceil(x)` of a finite float.
fn ceil(value: f64) -> i64 {
    value.ceil() as i64
}

/// `_collinear_fork`: one analytic wide-and-tapered collar for end-on cardinal lands.
fn collinear_fork(
    first: Point,
    second: Point,
    center: Point,
    heading_index: usize,
    sign: i64,
    offset: i64,
    expansion: f64,
) -> Option<[Vec<Point>; 2]> {
    if !heading_index.is_multiple_of(2) {
        return None;
    }
    let (dx, dy) = HEADS[heading_index];
    let (nx, ny) = (-dy, dx);
    if (first.x_nm - second.x_nm) * nx + (first.y_nm - second.y_nm) * ny != 0 {
        return None;
    }
    let middle = pt((first.x_nm + second.x_nm).div_euclid(2), (first.y_nm + second.y_nm).div_euclid(2));
    let half = ceil(hypot((first.x_nm - second.x_nm) as f64, (first.y_nm - second.y_nm) as f64) / 2.0);
    let widest = (2 * offset).max(half);
    let spread = if expansion == 1.0 { widest } else { ceil(expansion * widest as f64) };
    let far = [first, second]
        .iter()
        .map(|point| (point.x_nm - middle.x_nm) * dx + (point.y_nm - middle.y_nm) * dy)
        .max()
        .expect("two lands");
    let wide_distance = far + spread + 2 * offset;
    let narrow_distance = wide_distance + spread - offset;
    let distance = (center.x_nm - middle.x_nm) * dx + (center.y_nm - middle.y_nm) * dy;
    if distance <= narrow_distance {
        return None;
    }
    let lead = spread.div_euclid(2);
    let path = |point: Point, lane: i64| -> Vec<Point> {
        vec![
            point,
            pt(point.x_nm + nx * lane * lead, point.y_nm + ny * lane * lead),
            pt(
                point.x_nm + dx * (spread - lead) + nx * lane * spread,
                point.y_nm + dy * (spread - lead) + ny * lane * spread,
            ),
            pt(
                middle.x_nm + dx * wide_distance + nx * lane * spread,
                middle.y_nm + dy * wide_distance + ny * lane * spread,
            ),
            pt(
                middle.x_nm + dx * narrow_distance + nx * lane * offset,
                middle.y_nm + dy * narrow_distance + ny * lane * offset,
            ),
            pt(center.x_nm + nx * lane * offset, center.y_nm + ny * lane * offset),
        ]
    };
    Some([path(first, sign), path(second, -sign)])
}

/// The lane endpoints of a `(first, second)` member pair.
pub struct PortRequest<'a> {
    pub board: &'a PhysicalBoard,
    pub index: &'a RoutingClearanceIndex<'a>,
    pub first_name: &'a str,
    pub second_name: &'a str,
    pub first: Point,
    pub second: Point,
    pub component: &'a str,
    pub width: i64,
    pub offset: i64,
    pub clearance: i64,
    pub layer: CopperLayer,
    /// Port lengths from the land midpoint, shortest first (`lengths`).
    pub lengths: &'a [i64],
    pub allow_local_fallback: bool,
}

impl PortRequest<'_> {
    fn lane(&self, member: usize, points: &[Point]) -> Vec<TrackSegment> {
        let name = if member == 0 { self.first_name } else { self.second_name };
        tracks(name, points, self.width, self.layer, Some(&self.index.breakout))
    }

    fn legal(&self, first: &[TrackSegment], second: &[TrackSegment]) -> Result<bool> {
        legal(self.board, self.index, first, second, self.clearance)
    }

    /// `_converging_port`: converge end-on round lands before turning both lanes outward.
    fn converging_port(
        &self,
        middle: Point,
        heading_index: usize,
        sign: i64,
        distance: i64,
        transverse_extra: i64,
    ) -> Option<Port> {
        let breakout = &self.index.breakout;
        if !heading_index.is_multiple_of(2) || !breakout.pair(self.first_name, self.second_name) {
            return None;
        }
        let (dx, dy) = HEADS[heading_index];
        let (nx, ny) = (-dy, dx);
        let (first, second) = (self.first, self.second);
        if (first.x_nm - second.x_nm) * nx + (first.y_nm - second.y_nm) * ny != 0 {
            return None;
        }
        let lands = [breakout.region(self.first_name, &[first]), breakout.region(self.second_name, &[second])];
        for (identity, point) in lands.iter().zip([first, second]) {
            let identity = identity.as_ref()?;
            if !identity.starts_with(&format!("pad:{}.", self.component)) {
                return None;
            }
            let land = breakout.lands.values().flatten().find(|land| &land.identity == identity)?;
            if land.shape.spine != [point] || land.shape.radius_nm <= 0 {
                return None;
            }
        }
        let widths = [
            breakout.track_width_nm(self.first_name, lands[0].as_deref(), self.width),
            breakout.track_width_nm(self.second_name, lands[1].as_deref(), self.width),
        ];
        if widths[0] != widths[1] {
            return None;
        }
        let gap = breakout.spacing_nm(self.first_name, lands[0].as_deref(), self.second_name, lands[1].as_deref());
        let neck = (widths[0] + gap + 1).div_euclid(2);
        let pitch = ((first.x_nm - second.x_nm) * dx + (first.y_nm - second.y_nm) * dy).abs();
        let converge = pitch.div_euclid(2) - neck;
        let diagonal = converge.div_euclid(3);
        let transverse = pitch.div_euclid(2) - widths[0].div_euclid(4) + transverse_extra;
        if diagonal <= 0 || self.offset < neck || transverse < diagonal + 2 * neck {
            return None;
        }
        let lead = converge - diagonal;
        let far = [first, second]
            .iter()
            .map(|p| (p.x_nm - middle.x_nm) * dx + (p.y_nm - middle.y_nm) * dy)
            .max()
            .expect("two lands");
        let run = far + pitch.div_euclid(2);
        let taper = self.offset - neck;
        if distance <= run + taper {
            return None;
        }
        let first_near = (first.x_nm - middle.x_nm) * dx + (first.y_nm - middle.y_nm) * dy < 0;
        let side = if first_near { sign } else { -sign };
        let center = pt(
            middle.x_nm + dx * distance + nx * side * transverse,
            middle.y_nm + dy * distance + ny * side * transverse,
        );
        let at = |a: i64, b: i64| pt(middle.x_nm + dx * a + nx * b, middle.y_nm + dy * a + ny * b);
        let mut lanes = Vec::new();
        for (member, (point, lane)) in [(first, sign), (second, -sign)].into_iter().enumerate() {
            let u = (point.x_nm - middle.x_nm) * dx + (point.y_nm - middle.y_nm) * dy;
            let inward = if u < 0 { 1 } else { -1 };
            let stem = u + inward * converge;
            let v = side * transverse + lane * neck;
            let points = [
                point,
                at(u + inward * lead, 0),
                at(stem, side * diagonal),
                at(stem, v - side * neck),
                at(stem + neck, v),
                at(run, v),
                at(run + taper, side * transverse + lane * self.offset),
                at(distance, side * transverse + lane * self.offset),
            ];
            lanes.push(self.lane(member, &points));
        }
        let second_lane = lanes.pop().expect("two lanes");
        let first_lane = lanes.pop().expect("two lanes");
        Some(Port { center, heading: heading_index, sign, first: first_lane, second: second_lane, preference: -1 })
    }

    /// `_lateral_port_centers`: the two nearest projected obstacle boundaries
    /// inside a finite lateral strip.
    fn lateral_port_centers(&self, middle: Point, heading_index: usize, distance: i64) -> Result<Vec<Point>> {
        if !heading_index.is_multiple_of(2) {
            return Ok(Vec::new());
        }
        let (dx, dy) = HEADS[heading_index];
        let (nx, ny) = (-dy, dx);
        let reach = distance.min(2_000_000);
        let margin = self.offset + (self.width + 1).div_euclid(2) + self.clearance;
        let at = |u: i64, v: i64| pt(middle.x_nm + dx * u + nx * v, middle.y_nm + dy * u + ny * v);
        let corridor = RoundedConvexShape::new(
            vec![
                at(0, -reach - margin),
                at(distance + 500_000, -reach - margin),
                at(distance + 500_000, reach + margin),
                at(0, reach + margin),
            ],
            0,
        )?;
        let mut intervals: Vec<(i64, i64)> = Vec::new();
        for item in self.index.nearby_copper(&corridor, &[self.layer], self.clearance) {
            if item.net == self.first_name || item.net == self.second_name {
                continue;
            }
            let bounds = item.shape.bounds;
            let mut projections = Vec::new();
            for x in [bounds.min_x, bounds.max_x] {
                for y in [bounds.min_y, bounds.max_y] {
                    let (x, y) = (x - middle.x_nm, y - middle.y_nm);
                    projections.push((dx * x + dy * y, nx * x + ny * y));
                }
            }
            let (max_u, min_u) = (
                projections.iter().map(|p| p.0).max().expect("corners"),
                projections.iter().map(|p| p.0).min().expect("corners"),
            );
            if max_u < 0 || min_u > distance + 500_000 {
                continue;
            }
            intervals.push((
                projections.iter().map(|p| p.1).min().expect("corners") - margin,
                projections.iter().map(|p| p.1).max().expect("corners") + margin,
            ));
        }
        intervals.sort();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (lo, hi) in intervals {
            match merged.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        let Some(blocked) = merged.iter().find(|(lo, hi)| *lo <= 0 && 0 <= *hi) else { return Ok(Vec::new()) };
        let mut shifts = [blocked.0 - 1, blocked.1 + 1];
        shifts.sort_by_key(|value| (value.abs(), *value));
        Ok(shifts.into_iter().filter(|value| value.abs() <= reach).map(|value| at(distance, value)).collect())
    }

    /// `_offset_round_ports`: at most two offset collars per cardinal heading
    /// for round plated lands.
    fn offset_round_ports(&self, headings: &[usize]) -> Result<Vec<Port>> {
        use copper_physical::footprint::PadKind;
        if !self.index.breakout.pair(self.first_name, self.second_name) {
            return Ok(Vec::new());
        }
        let board = self.board;
        let pose = board.placements.iter().find(|p| p.reference == self.component).expect("a placed component");
        let footprint = &board.footprints[&pose.footprint];
        let (mut own, mut foreign) = (Vec::new(), Vec::new());
        for pad in &footprint.pads {
            let position = copper_pcb::transform::transformed_local_point(pose, pad.position);
            if position == self.first || position == self.second {
                if pad.kind != PadKind::ThroughHole || pad.drill.is_none() {
                    return Ok(Vec::new());
                }
                let shape = copper_pcb::transform::placed_pad_shape(position, pad, pose);
                if shape.spine != [position] {
                    return Ok(Vec::new());
                }
                own.push(position);
            } else if pad.kind != PadKind::Smd
                || (pose.side == copper_physical::BoardSide::Front) == (self.layer == CopperLayer::Front)
            {
                foreign.push(copper_pcb::transform::placed_pad_shape(position, pad, pose).bounds);
            }
        }
        if own.len() != 2 {
            return Ok(Vec::new());
        }
        let (first, second) = (self.first, self.second);
        let middle = pt((first.x_nm + second.x_nm).div_euclid(2), (first.y_nm + second.y_nm).div_euclid(2));
        let reach = *self.lengths.iter().max().expect("lengths");
        let margin = self.offset + (self.width + 1).div_euclid(2) + self.clearance + 4;
        let mut results = Vec::new();
        for &heading_index in headings {
            if !heading_index.is_multiple_of(2) {
                continue;
            }
            let (dx, dy) = HEADS[heading_index];
            let (nx, ny) = (-dy, dx);
            let dot = (first.x_nm - second.x_nm) * nx + (first.y_nm - second.y_nm) * ny;
            if dot == 0 {
                continue;
            }
            let sign = if dot > 0 { 1 } else { -1 };
            let mut shifts: indexmap::IndexSet<i64> = indexmap::IndexSet::new();
            for bounds in &foreign {
                let mut projected = Vec::new();
                for x in [bounds.min_x, bounds.max_x] {
                    for y in [bounds.min_y, bounds.max_y] {
                        let (x, y) = (x - middle.x_nm, y - middle.y_nm);
                        projected.push((x * dx + y * dy, x * nx + y * ny));
                    }
                }
                if projected.iter().map(|p| p.0).max().expect("corners") < 0
                    || projected.iter().map(|p| p.0).min().expect("corners") > reach
                {
                    continue;
                }
                for value in [
                    projected.iter().map(|p| p.1).min().expect("corners") - margin,
                    projected.iter().map(|p| p.1).max().expect("corners") + margin,
                ] {
                    if 0 < value.abs() && value.abs() <= reach {
                        shifts.insert(value);
                    }
                }
            }
            let side_values = |side: i64| -> Vec<i64> {
                let mut values: Vec<i64> = shifts.iter().copied().filter(|v| (*v > 0) == (side > 0)).collect();
                values.sort_by_key(|v| v.abs());
                values.truncate(2);
                values
            };
            let sides = [side_values(-1), side_values(1)];
            let mut ordered = Vec::new();
            for rank in 0..2 {
                for values in &sides {
                    if rank < values.len() {
                        ordered.push(values[rank]);
                    }
                }
            }
            let mut admitted: Vec<i64> = Vec::new();
            for value in ordered {
                let side = if value > 0 { 1 } else { -1 };
                if admitted.contains(&side) {
                    continue;
                }
                'lengths: for &travel in self.lengths.iter().rev() {
                    let center = pt(middle.x_nm + dx * travel + nx * value, middle.y_nm + dy * travel + ny * value);
                    let lane_normal = normal(heading_index, self.offset);
                    let targets = [shift(center, lane_normal, sign), shift(center, lane_normal, -sign)];
                    let approaches = targets.map(|p| pt(p.x_nm - dx * 125_000, p.y_nm - dy * 125_000));
                    for a in paths(first, approaches[0]) {
                        for b in paths(second, approaches[1]) {
                            let mut a = a.clone();
                            a.push(targets[0]);
                            let mut b = b.clone();
                            b.push(targets[1]);
                            let (first_lane, second_lane) = (self.lane(0, &a), self.lane(1, &b));
                            if self.legal(&first_lane, &second_lane)? {
                                results.push(Port {
                                    center,
                                    heading: heading_index,
                                    sign,
                                    first: first_lane,
                                    second: second_lane,
                                    preference: -1,
                                });
                                admitted.push(side);
                                break 'lengths;
                            }
                        }
                    }
                }
                if admitted.len() >= 2 {
                    break;
                }
            }
        }
        Ok(results)
    }

    /// `_ports`: the joint collars from this pair of lands outward.
    pub fn ports(&self) -> Result<Vec<Port>> {
        let (first, second) = (self.first, self.second);
        let separation = match heading(second, first) {
            Some(separation) => separation,
            None => {
                let (vx, vy) = (i128::from(first.x_nm - second.x_nm), i128::from(first.y_nm - second.y_nm));
                if vx == 0 && vy == 0 {
                    return Ok(Vec::new());
                }
                // max(range(8), key=(Fraction(max(0, v.h)^2, |h|^2), -h)).
                let mut best = 0usize;
                for h in 1..8 {
                    let score = |h: usize| {
                        let (hx, hy) = (i128::from(HEADS[h].0), i128::from(HEADS[h].1));
                        let dot = (vx * hx + vy * hy).max(0);
                        (dot * dot, hx * hx + hy * hy)
                    };
                    let ((a, b), (c, d)) = (score(h), score(best));
                    // a/b vs c/d, then -h vs -best.
                    if a * d > c * b || (a * d == c * b && h < best) {
                        best = h;
                    }
                }
                best
            }
        };
        let middle = pt((first.x_nm + second.x_nm).div_euclid(2), (first.y_nm + second.y_nm).div_euclid(2));
        let placement =
            self.board.placements.iter().find(|item| item.reference == self.component).expect("a placed component");
        let outward = pt(middle.x_nm - placement.position.x_nm, middle.y_nm - placement.position.y_nm);
        let preferred = [(separation + 6) % 8, (separation + 2) % 8];
        let mut headings: Vec<usize> = (0..8).collect();
        headings
            .sort_by_key(|h| (!preferred.contains(h), -(HEADS[*h].0 * outward.x_nm + HEADS[*h].1 * outward.y_nm), *h));
        let mut ports = Vec::new();
        for (preference, &heading_index) in headings.iter().enumerate() {
            let lane_normal = normal(heading_index, self.offset);
            let dot = (first.x_nm - second.x_nm) * lane_normal.x_nm + (first.y_nm - second.y_nm) * lane_normal.y_nm;
            let signs: &[i64] = if dot > 0 {
                &[1]
            } else if dot < 0 {
                &[-1]
            } else {
                &[-1, 1]
            };
            for &sign in signs {
                for &step in self.lengths {
                    let (dx, dy) = HEADS[heading_index];
                    if dot == 0 && dx * outward.x_nm + dy * outward.y_nm > 0 {
                        let mut converging = None;
                        for extra in [0, self.clearance, 2 * self.clearance] {
                            if let Some(candidate) = self.converging_port(middle, heading_index, sign, step, extra)
                                && self.legal(&candidate.first, &candidate.second)?
                            {
                                converging = Some(candidate);
                                break;
                            }
                        }
                        if let Some(port) = converging {
                            ports.push(port);
                            continue;
                        }
                    }
                    let center = pt(middle.x_nm + dx * step, middle.y_nm + dy * step);
                    let targets = [shift(center, lane_normal, sign), shift(center, lane_normal, -sign)];
                    let mut admitted =
                        self.admit(center, heading_index, sign, &targets, preference as i64, true, &mut ports)?;
                    if !admitted
                        && dot != 0
                        && heading_index % 2 == 0
                        && self.index.breakout.pair(self.first_name, self.second_name)
                    {
                        for shifted in self.lateral_port_centers(middle, heading_index, step)? {
                            let shifted_targets =
                                [shift(shifted, lane_normal, sign), shift(shifted, lane_normal, -sign)];
                            if self.admit(
                                shifted,
                                heading_index,
                                sign,
                                &shifted_targets,
                                preference as i64,
                                false,
                                &mut ports,
                            )? {
                                admitted = true;
                                break;
                            }
                        }
                    }
                    if !admitted && dot == 0 {
                        for expansion in [1.0, 1.5, 2.0] {
                            if let Some([a, b]) =
                                collinear_fork(first, second, center, heading_index, sign, self.offset, expansion)
                            {
                                let (a, b) = (self.lane(0, &a), self.lane(1, &b));
                                if self.legal(&a, &b)? {
                                    ports.push(Port {
                                        center,
                                        heading: heading_index,
                                        sign,
                                        first: a,
                                        second: b,
                                        preference: preference as i64,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        ports.extend(self.offset_round_ports(&headings)?);
        if ports.is_empty() && self.allow_local_fallback && self.index.breakout.pair(self.first_name, self.second_name)
        {
            self.local_fallback(middle, &headings, &mut ports)?;
        }
        Ok(ports)
    }

    /// The finite local alternative for recessed paired lands: exact
    /// source-to-port lead-ins found by the local access search.
    fn local_fallback(&self, middle: Point, headings: &[usize], ports: &mut Vec<Port>) -> Result<()> {
        use crate::pin_access::{LocalAccessBounds, local_access_path};
        for (preference, &heading_index) in headings.iter().enumerate() {
            if !heading_index.is_multiple_of(2) {
                continue;
            }
            let (dx, dy) = HEADS[heading_index];
            let lane_normal = normal(heading_index, self.offset);
            let dot = (self.first.x_nm - self.second.x_nm) * lane_normal.x_nm
                + (self.first.y_nm - self.second.y_nm) * lane_normal.y_nm;
            let signs: &[i64] = if dot > 0 {
                &[1]
            } else if dot < 0 {
                &[-1]
            } else {
                &[1, -1]
            };
            'slots: for &sign in signs {
                for distance in [1_000_000, 1_500_000, 2_000_000, 3_000_000, 4_000_000] {
                    if ports.iter().filter(|port| port.heading == heading_index).count() >= 2 {
                        break 'slots;
                    }
                    let center = pt(middle.x_nm + dx * distance, middle.y_nm + dy * distance);
                    let targets = [shift(center, lane_normal, sign), shift(center, lane_normal, -sign)];
                    let approaches = targets.map(|p| pt(p.x_nm - dx * 125_000, p.y_nm - dy * 125_000));
                    let tails =
                        [self.lane(0, &[approaches[0], targets[0]]), self.lane(1, &[approaches[1], targets[1]])];
                    if !self.legal(&tails[0], &tails[1])? {
                        continue;
                    }
                    for order in [[0usize, 1], [1, 0]] {
                        let mut copper = tails[0].clone();
                        copper.extend(tails[1].iter().cloned());
                        let held = with_tracks(self.board, &copper);
                        let mut reserved = RoutingClearanceIndex::new(&held)?;
                        reserved.pair_access = true;
                        let mut lanes: [Option<Vec<TrackSegment>>; 2] = [None, None];
                        for member in order {
                            let (name, land) = if member == 0 {
                                (self.first_name, self.first)
                            } else {
                                (self.second_name, self.second)
                            };
                            let bounds =
                                LocalAccessBounds { step_nm: 25_000, detour_nm: 500_000, maximum_states: 2400 };
                            let Some(path) = local_access_path(
                                self.board,
                                &reserved,
                                name,
                                land,
                                approaches[member],
                                self.layer,
                                self.width,
                                bounds,
                                None,
                                None,
                            )?
                            else {
                                break;
                            };
                            let mut lane = path.clone();
                            lane.extend(tails[member].iter().cloned());
                            lanes[member] = Some(lane);
                            for track in path {
                                reserved.add_track(track, true, None)?;
                            }
                        }
                        if let [Some(a), Some(b)] = &lanes
                            && self.legal(a, b)?
                        {
                            ports.push(Port {
                                center,
                                heading: heading_index,
                                sign,
                                first: a.clone(),
                                second: b.clone(),
                                preference: preference as i64,
                            });
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The first legal collar to `targets` that reaches them along `heading`:
    /// one-corner paths, plus the mid-way jog when `collar`.
    #[allow(clippy::too_many_arguments)]
    fn admit(
        &self,
        center: Point,
        heading_index: usize,
        sign: i64,
        targets: &[Point; 2],
        preference: i64,
        collar: bool,
        ports: &mut Vec<Port>,
    ) -> Result<bool> {
        let candidates = if collar { collar_paths } else { paths };
        for a in candidates(self.first, targets[0]) {
            for b in candidates(self.second, targets[1]) {
                if heading(a[a.len() - 2], a[a.len() - 1]) != Some(heading_index)
                    || heading(b[b.len() - 2], b[b.len() - 1]) != Some(heading_index)
                {
                    continue;
                }
                let (first_lane, second_lane) = (self.lane(0, &a), self.lane(1, &b));
                if self.legal(&first_lane, &second_lane)? {
                    ports.push(Port {
                        center,
                        heading: heading_index,
                        sign,
                        first: first_lane,
                        second: second_lane,
                        preference,
                    });
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// `_lane_paths`: both mitered lanes along a spine, or `None` when a turn is
/// too sharp or a short edge collapses.
pub fn lane_paths(
    points: &[Point],
    incoming: usize,
    outgoing: usize,
    offset: i64,
    sign: i64,
) -> Option<(Vec<Point>, Vec<Point>)> {
    let mut unique: Vec<Point> = Vec::new();
    for (index, point) in points.iter().enumerate() {
        if index == 0 || *point != points[index - 1] {
            unique.push(*point);
        }
    }
    if unique.len() < 2 {
        return None;
    }
    let headings: Vec<usize> = unique.windows(2).map(|pair| heading(pair[0], pair[1])).collect::<Option<_>>()?;
    let mut orientations = vec![incoming];
    orientations.extend(&headings);
    orientations.push(outgoing);
    if orientations.windows(2).any(|pair| turn(pair[0], pair[1]) > 1) {
        return None;
    }
    let (mut first, mut second) = (Vec::new(), Vec::new());
    for (index, point) in unique.iter().enumerate() {
        let previous = if index == 0 { incoming } else { headings[index - 1] };
        let following = if index == unique.len() - 1 { outgoing } else { headings[index] };
        first.push(miter(*point, previous, following, offset, sign));
        second.push(miter(*point, previous, following, offset, -sign));
    }
    for lane in [&first, &second] {
        for (pair, expected) in lane.windows(2).zip(&headings) {
            if heading(pair[0], pair[1]) != Some(*expected) {
                return None;
            }
        }
    }
    Some((first, second))
}

/// `PairSearchStats`: search-local telemetry, never copper ownership.
#[derive(Default)]
pub struct PairSearchStats {
    pub port_pairs: i64,
    pub searches: i64,
    pub expanded_states: i64,
    pub candidates: i64,
    pub resumed_states: i64,
    pub resumed_portals: i64,
    pub port_inventory: Vec<PairPortInventory>,
    pub transition_inventory: Vec<PairTransitionInventory>,
    pub blocked_launches: i64,
    pub launch_rejections: Vec<String>,
    pub terminal_port_rejections: Vec<String>,
    /// Exact suspended frontiers (`_continuations`), in trial order.
    pub continuations: Vec<Continuation>,
}

fn add_unique(items: &mut Vec<String>, message: String) {
    if !items.contains(&message) {
        items.push(message);
    }
}

impl PairSearchStats {
    /// `reject_launch`: bounded exact blocker evidence of a blocked launch.
    pub fn reject_launch(&mut self, index: &RoutingClearanceIndex<'_>, tracks: &[TrackSegment]) -> Result<()> {
        self.blocked_launches += 1;
        if self.launch_rejections.len() >= 8 {
            return Ok(());
        }
        let millimetres = |value: i64| -> String {
            let decimal = copper_units::Decimal::from_i64(value)
                .div(&copper_units::Decimal::from_i64(1_000_000))
                .expect("a finite quotient");
            copper_units::Quantity::of(copper_units::Dimension::Length, &decimal, "mm")
                .expect("millimetres")
                .to_string()
        };
        for track in tracks {
            for blocker in index.track_blocker_records(track)? {
                let parts: Vec<&str> = blocker.identity.split(':').collect();
                let label = if parts[0] == "pad" { parts[..2.min(parts.len())].join(":") } else { parts[0].to_owned() };
                let at = format!("{}, {}", millimetres(track.start.x_nm), millimetres(track.start.y_nm));
                let message = format!("{} launch at ({at}): {}, {label}", track.net, blocker.reason);
                add_unique(&mut self.launch_rejections, message);
                self.launch_rejections.truncate(8);
                if self.launch_rejections.len() >= 8 {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// `_Continuation`: one portal trial's exact A* frontier, resumable under
/// the aggregate bound. Its board is immutable, so the ownership
/// fingerprints of `pair_search_resume` always agree with it.
pub struct Continuation {
    pub start: Port,
    pub end: Port,
    pub layer: CopperLayer,
    pub pitch: i64,
    pub search_index: i64,
    pub search: Option<SpineSearch>,
    pub registered: bool,
    pub via_pairs: Vec<(copper_pcb::board::Via, copper_pcb::board::Via)>,
    pub return_vias: Vec<copper_pcb::board::Via>,
    pub active: bool,
    pub resumed: bool,
    pub expanded_states: i64,
    pub legal_advances: i64,
    pub minimum_goal_distance_nm: f64,
    /// The escaped board the frontier searches.
    pub board: PhysicalBoard,
}

/// `_SpineSearchResult`.
pub struct SpineResult {
    pub first: Vec<TrackSegment>,
    pub second: Vec<TrackSegment>,
    pub expanded_states: i64,
    pub spine: Vec<Point>,
}

type State = (Point, usize);

/// `_search_steps`: the A* frontier over (point, heading) spine states,
/// advanced one complete expansion at a time.
pub struct SpineSearch {
    first_name: String,
    second_name: String,
    width: i64,
    offset: i64,
    clearance: i64,
    pitch: i64,
    inward: usize,
    initial: State,
    best: IndexMap<State, i64>,
    previous: IndexMap<State, Option<State>>,
    serial: i64,
    queue: BinaryHeap<Reverse<(i64, i64, i64, Point, usize)>>,
    legal_cache: IndexMap<(Point, usize, usize, bool), bool>,
    expanded: i64,
    finished: bool,
}

/// One `next()` of the generator.
pub enum Step {
    Expanded,
    Found(SpineResult),
    Exhausted,
}

impl SpineSearch {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        first_name: &str,
        second_name: &str,
        start: &Port,
        end: &Port,
        width: i64,
        offset: i64,
        clearance: i64,
        pitch: i64,
    ) -> Self {
        let initial = (start.center, start.heading);
        let mut queue = BinaryHeap::new();
        queue.push(Reverse((0, 0, 0, initial.0, initial.1)));
        SpineSearch {
            first_name: first_name.to_owned(),
            second_name: second_name.to_owned(),
            width,
            offset,
            clearance,
            pitch,
            inward: (end.heading + 4) % 8,
            initial,
            best: IndexMap::from([(initial, 0)]),
            previous: IndexMap::from([(initial, None)]),
            serial: 0,
            queue,
            legal_cache: IndexMap::new(),
            expanded: 0,
            finished: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn legal_step(
        &mut self,
        board: &PhysicalBoard,
        index: &RoutingClearanceIndex<'_>,
        layer: CopperLayer,
        start: &Port,
        stats: &mut PairSearchStats,
        continuation_expanded: i64,
        point: Point,
        old: usize,
        new: usize,
        initial_step: bool,
    ) -> Result<bool> {
        let key = (point, old, new, initial_step);
        if let Some(value) = self.legal_cache.get(&key) {
            return Ok(*value);
        }
        let pitch = self.pitch;
        let (dx, dy) = HEADS[old];
        let behind = pt(point.x_nm - dx * pitch, point.y_nm - dy * pitch);
        let (dx, dy) = HEADS[new];
        let ahead = pt(point.x_nm + dx * pitch, point.y_nm + dy * pitch);
        let points: Vec<Point> = if initial_step { vec![point, ahead] } else { vec![behind, point, ahead] };
        let lanes = lane_paths(&points, old, new, self.offset, start.sign);
        let breakout = Some(&index.breakout);
        let value = match &lanes {
            Some((a, b)) => legal(
                board,
                index,
                &tracks(&self.first_name, a, self.width, layer, breakout),
                &tracks(&self.second_name, b, self.width, layer, breakout),
                self.clearance,
            )?,
            None => false,
        };
        self.legal_cache.insert(key, value);
        if continuation_expanded <= 4
            && let Some((a, b)) = &lanes
            && !value
        {
            let mut launch = tracks(&self.first_name, a, self.width, layer, breakout);
            launch.extend(tracks(&self.second_name, b, self.width, layer, breakout));
            stats.reject_launch(index, &launch)?;
        }
        Ok(value)
    }
}

/// `_goal_paths`: bounded terminal collars, cheap bridges first.
fn goal_paths(point: Point, end: Point, heading_index: usize, inward: usize, pitch: i64) -> Vec<Vec<Point>> {
    let (dx, dy) = HEADS[inward];
    let approach = pt(end.x_nm - dx * pitch, end.y_nm - dy * pitch);
    let mut result: Vec<Vec<Point>> = paths(point, end);
    for path in paths(point, approach) {
        let mut path = path;
        path.push(end);
        result.push(path);
    }
    for direction in [heading_index, (heading_index + 7) % 8, (heading_index + 1) % 8] {
        let (dx, dy) = HEADS[direction];
        for distance in [pitch, 2 * pitch] {
            let advanced = pt(point.x_nm + dx * distance, point.y_nm + dy * distance);
            for path in paths(advanced, approach) {
                let mut full = vec![point];
                full.extend(path);
                full.push(end);
                result.push(full);
            }
        }
    }
    for turn_by in [7usize, 1] {
        let (dx, dy) = HEADS[(heading_index + turn_by) % 8];
        let (fx, fy) = HEADS[heading_index];
        for distance in [(pitch.div_euclid(2)).max(1), pitch] {
            let first = pt(point.x_nm + dx * distance, point.y_nm + dy * distance);
            let second = pt(first.x_nm + fx * distance, first.y_nm + fy * distance);
            for path in paths(second, approach) {
                let mut full = vec![point, first];
                full.extend(path);
                full.push(end);
                result.push(full);
            }
        }
    }
    result
}

impl Continuation {
    /// One generator step: a complete state expansion, a found candidate, or the end.
    fn step(&mut self, index: &RoutingClearanceIndex<'_>, stats: &mut PairSearchStats) -> Result<Step> {
        let Some(search) = self.search.as_mut() else { return Ok(Step::Exhausted) };
        if search.finished {
            return Ok(Step::Exhausted);
        }
        while let Some(Reverse((_, cost, _, x, h))) = search.queue.pop() {
            let state = (x, h);
            if Some(&cost) != search.best.get(&state) {
                continue;
            }
            let (point, heading_index) = state;
            search.expanded += 1;
            stats.expanded_states += 1;
            self.expanded_states = search.expanded;
            let distance =
                hypot((point.x_nm - self.end.center.x_nm) as f64, (point.y_nm - self.end.center.y_nm) as f64);
            if distance < self.minimum_goal_distance_nm {
                self.minimum_goal_distance_nm = distance;
            }
            if distance <= (4 * search.pitch) as f64 {
                for tail in goal_paths(point, self.end.center, heading_index, search.inward, search.pitch) {
                    let mut spine = Vec::new();
                    let mut cursor = Some(state);
                    while let Some(current) = cursor {
                        spine.push(current.0);
                        cursor = search.previous[&current];
                    }
                    spine.reverse();
                    let mut points = spine;
                    points.extend_from_slice(&tail[1..]);
                    if points.len() < 2
                        || heading(points[0], points[1]) != Some(self.start.heading)
                        || heading(points[points.len() - 2], points[points.len() - 1]) != Some(search.inward)
                    {
                        continue;
                    }
                    let Some((first_lane, second_lane)) =
                        lane_paths(&points, self.start.heading, search.inward, search.offset, self.start.sign)
                    else {
                        continue;
                    };
                    let breakout = Some(&index.breakout);
                    let a = tracks(&search.first_name, &first_lane, search.width, self.layer, breakout);
                    let b = tracks(&search.second_name, &second_lane, search.width, self.layer, breakout);
                    if legal(&self.board, index, &a, &b, search.clearance)? {
                        self.active = false;
                        search.finished = true;
                        return Ok(Step::Found(SpineResult {
                            first: a,
                            second: b,
                            expanded_states: search.expanded,
                            spine: points,
                        }));
                    }
                }
            }
            for new in [(heading_index + 7) % 8, heading_index, (heading_index + 1) % 8] {
                if state == search.initial && new != heading_index {
                    continue;
                }
                let initial_step = state == search.initial;
                let expanded = self.expanded_states;
                let start = self.start.clone();
                if !search.legal_step(
                    &self.board,
                    index,
                    self.layer,
                    &start,
                    stats,
                    expanded,
                    point,
                    heading_index,
                    new,
                    initial_step,
                )? {
                    continue;
                }
                self.legal_advances += 1;
                let (dx, dy) = HEADS[new];
                let pitch = search.pitch;
                let target = pt(point.x_nm + dx * pitch, point.y_nm + dy * pitch);
                let next_state = (target, new);
                let next_cost = cost
                    + round_int(hypot((dx * pitch) as f64, (dy * pitch) as f64))
                    + if new != heading_index { pitch.div_euclid(4) } else { 0 };
                if next_cost >= search.best.get(&next_state).copied().unwrap_or(i64::MAX) {
                    continue;
                }
                search.best.insert(next_state, next_cost);
                search.previous.insert(next_state, Some(state));
                search.serial += 1;
                let heuristic = round_int(hypot(
                    (target.x_nm - self.end.center.x_nm) as f64,
                    (target.y_nm - self.end.center.y_nm) as f64,
                ));
                search.queue.push(Reverse((next_cost + heuristic, next_cost, search.serial, target, new)));
            }
            self.active = !search.queue.is_empty();
            return Ok(Step::Expanded);
        }
        self.active = false;
        search.finished = true;
        Ok(Step::Exhausted)
    }

    /// `_advance`: at most `budget` generator steps.
    pub fn advance(&mut self, budget: i64, stats: &mut PairSearchStats) -> Result<Option<SpineResult>> {
        let board = self.board.clone();
        let index = RoutingClearanceIndex::new(&board)?;
        for _ in 0..budget {
            match self.step(&index, stats)? {
                Step::Expanded => {}
                Step::Found(result) => {
                    self.search = None;
                    return Ok(Some(result));
                }
                Step::Exhausted => {
                    self.active = false;
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }
}

/// `_remaining_states`: a shared counter caps work across ports, layers, pitches and families.
pub fn remaining_states(stats: &PairSearchStats, maximum_states: i64, maximum_total_states: Option<i64>) -> i64 {
    match maximum_total_states {
        None => maximum_states,
        Some(total) => 0.max(maximum_states.min(total - stats.expanded_states)),
    }
}

/// `_pair_profile`: width, lane offset, clearance and surface layer, or `None`.
fn pair_profile(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    first_guide: &GlobalNetRoute,
    second_guide: &GlobalNetRoute,
) -> Result<Option<(i64, i64, i64, CopperLayer)>> {
    let default = board.rules.default_track_width_nm;
    let width = first_rule.width_nm.filter(|w| *w != 0).unwrap_or(default);
    if second_rule.width_nm.filter(|w| *w != 0).unwrap_or(default) != width {
        return Ok(None);
    }
    let Some(gap) = first_rule.pair_gap_nm else { return Ok(None) };
    if second_rule.pair_gap_nm != Some(gap) || first_guide.accesses.len() != 2 || second_guide.accesses.len() != 2 {
        return Ok(None);
    }
    if crate::pair_terminals::terminal_partners(first_guide, second_guide).is_none() {
        return Ok(None);
    }
    let second_layers = crate::routing_layers::routing_layers(board, &second_rule.net, Some(second_rule));
    let mut layers: Vec<CopperLayer> = crate::routing_layers::routing_layers(board, &first_rule.net, Some(first_rule))
        .into_iter()
        .filter(|layer| second_layers.contains(layer))
        .collect();
    for access in first_guide.accesses.iter().chain(&second_guide.accesses) {
        let contact = crate::pair_terminals::terminal_copper_layers(board, &access.pad)?;
        layers.retain(|layer| contact.contains(layer));
    }
    let order = &board.stackup.copper_layers;
    let Some(layer) = layers.iter().copied().min_by_key(|layer| crate::layer_index(order, *layer)) else {
        return Ok(None);
    };
    let offset = (width + gap + 1).div_euclid(2);
    let clearance = board
        .rules
        .minimum_clearance_nm
        .max(first_rule.clearance_nm.unwrap_or(0))
        .max(second_rule.clearance_nm.unwrap_or(0));
    Ok(Some((width, offset, clearance, layer)))
}

/// `_can_depart`: the mandatory first spine step along the port's heading clears.
#[allow(clippy::too_many_arguments)]
pub fn can_depart(
    board: &PhysicalBoard,
    index: &RoutingClearanceIndex<'_>,
    first_name: &str,
    second_name: &str,
    port: &Port,
    width: i64,
    offset: i64,
    clearance: i64,
    layer: CopperLayer,
    pitch: i64,
) -> Result<bool> {
    let (dx, dy) = HEADS[port.heading];
    let ahead = pt(port.center.x_nm + dx * pitch, port.center.y_nm + dy * pitch);
    let Some((a, b)) = lane_paths(&[port.center, ahead], port.heading, port.heading, offset, port.sign) else {
        return Ok(false);
    };
    let breakout = Some(&index.breakout);
    legal(
        board,
        index,
        &tracks(first_name, &a, width, layer, breakout),
        &tracks(second_name, &b, width, layer, breakout),
        clearance,
    )
}

fn with_tracks(board: &PhysicalBoard, extra: &[TrackSegment]) -> PhysicalBoard {
    let mut escaped = board.clone();
    escaped.tracks.extend(extra.iter().cloned());
    escaped
}

/// The bounds of one `paired_candidates` call.
#[derive(Clone, Copy)]
pub struct SearchBounds {
    pub maximum_searches: i64,
    pub maximum_states: i64,
    pub pitch_nm: i64,
    pub maximum_total_states: Option<i64>,
}

fn reversed(tracks: &[TrackSegment]) -> Vec<TrackSegment> {
    tracks
        .iter()
        .rev()
        .map(|t| TrackSegment { net: t.net.clone(), start: t.end, end: t.start, width_nm: t.width_nm, layer: t.layer })
        .collect()
}

/// `paired_candidates`: deterministic joint surface candidates, in order;
/// never reserves or modifies input copper. `accept` sees each candidate
/// and returns whether the caller is done.
#[allow(clippy::too_many_arguments)]
pub fn paired_candidates(
    board: &PhysicalBoard,
    first_rule: &NetRoutingRule,
    second_rule: &NetRoutingRule,
    first_guide: &GlobalNetRoute,
    second_guide: &GlobalNetRoute,
    bounds: SearchBounds,
    stats: &mut PairSearchStats,
    accept: &mut dyn FnMut(PairSearchCandidate, &mut PairSearchStats) -> Result<bool>,
) -> Result<bool> {
    if bounds.maximum_searches.min(bounds.maximum_states).min(bounds.pitch_nm) <= 0 {
        return Err(crate::Error::Value("joint pair search bounds must be positive".to_owned()));
    }
    if bounds.maximum_total_states.is_some_and(|total| total <= 0) {
        return Err(crate::Error::Value("joint pair aggregate state bound must be positive".to_owned()));
    }
    if bounds.maximum_total_states.is_some_and(|total| stats.expanded_states >= total) {
        return Ok(false);
    }
    let Some((width, offset, clearance, layer)) =
        pair_profile(board, first_rule, second_rule, first_guide, second_guide)?
    else {
        return Ok(false);
    };
    let partners = crate::pair_terminals::terminal_partners(first_guide, second_guide).expect("profiled partners");
    let index = RoutingClearanceIndex::new(board)?;
    let mut accesses: Vec<&crate::routing::PinAccess> = first_guide.accesses.iter().collect();
    accesses.sort_by(|a, b| a.pad.component.cmp(&b.pad.component));
    let mut groups: Vec<Vec<Port>> = Vec::new();
    for access in &accesses {
        let request = PortRequest {
            board,
            index: &index,
            first_name: &first_rule.net,
            second_name: &second_rule.net,
            first: access.pad_position,
            second: partners[&format!("pad:{}", access.pad)].pad_position,
            component: &access.pad.component,
            width,
            offset,
            clearance,
            layer,
            lengths: &PORT_LENGTHS_NM,
            allow_local_fallback: true,
        };
        groups.push(request.ports()?);
    }
    for (access, group) in accesses.iter().zip(&groups) {
        if group.is_empty() {
            let message = format!("no exact paired collar at {} on {}", access.pad.component, layer.value());
            add_unique(&mut stats.terminal_port_rejections, message);
            stats.terminal_port_rejections.truncate(4);
        }
    }
    // `_departure_preferences`: clear onward collars first.
    let mut onward: Vec<Vec<i64>> = Vec::new();
    for group in &groups {
        let mut ranks = Vec::new();
        for port in group {
            let mut copper = port.first.clone();
            copper.extend(port.second.iter().cloned());
            let escaped = with_tracks(board, &copper);
            let escaped_index = RoutingClearanceIndex::new(&escaped)?;
            let departs = can_depart(
                &escaped,
                &escaped_index,
                &first_rule.net,
                &second_rule.net,
                port,
                width,
                offset,
                clearance,
                layer,
                bounds.pitch_nm,
            )?;
            ranks.push(i64::from(!departs));
        }
        onward.push(ranks);
    }
    for ((access, ports), ranks) in accesses.iter().zip(&groups).zip(&onward) {
        if stats.port_inventory.len() >= 24 {
            break;
        }
        let mut examples = Vec::new();
        let mut per_heading: IndexMap<(usize, i64), i64> = IndexMap::new();
        for (port, rank) in ports.iter().zip(ranks) {
            let key = (port.heading, port.preference);
            if per_heading.get(&key).copied().unwrap_or(0) >= 2 || examples.len() >= 16 {
                continue;
            }
            *per_heading.entry(key).or_default() += 1;
            examples.push(PairPortExample {
                center: port.center,
                heading: port.heading as i64,
                sign: port.sign,
                can_depart: *rank == 0,
            });
        }
        stats.port_inventory.push(PairPortInventory {
            component: access.pad.component.clone(),
            layer,
            pitch_nm: bounds.pitch_nm,
            port_count: ports.len() as i64,
            departable_count: ranks.iter().filter(|rank| **rank == 0).count() as i64,
            examples,
        });
    }
    let mut combinations: Vec<(usize, usize)> = Vec::new();
    if groups.len() == 2 {
        for (i, a) in groups[0].iter().enumerate() {
            for (j, b) in groups[1].iter().enumerate() {
                if a.sign == -b.sign {
                    combinations.push((i, j));
                }
            }
        }
    }
    combinations.sort_by(|&(i, j), &(k, l)| {
        let key = |i: usize, j: usize| {
            let (a, b) = (&groups[0][i], &groups[1][j]);
            (
                onward[0][i] + onward[1][j],
                a.preference + b.preference,
                hypot((a.center.x_nm - b.center.x_nm) as f64, (a.center.y_nm - b.center.y_nm) as f64),
                a.center.x_nm,
                a.center.y_nm,
                b.center.x_nm,
                b.center.y_nm,
            )
        };
        key(i, j).partial_cmp(&key(k, l)).expect("finite keys")
    });
    stats.port_pairs += combinations.len() as i64;
    if groups.iter().all(|group| !group.is_empty()) && combinations.is_empty() {
        add_unique(&mut stats.terminal_port_rejections, "paired collars have no compatible lane order".to_owned());
        let excess = stats.terminal_port_rejections.len().saturating_sub(4);
        stats.terminal_port_rejections.drain(..excess);
    }
    let searches_at_entry = stats.searches;
    for (search_index, &(i, j)) in combinations.iter().take(128).enumerate() {
        let search_index = search_index as i64 + 1;
        if stats.searches - searches_at_entry >= bounds.maximum_searches {
            break;
        }
        let budget = remaining_states(stats, bounds.maximum_states, bounds.maximum_total_states);
        if budget == 0 {
            break;
        }
        let (start, end) = (&groups[0][i], &groups[1][j]);
        let mut first_escape = start.first.clone();
        first_escape.extend(end.first.iter().cloned());
        let mut second_escape = start.second.clone();
        second_escape.extend(end.second.iter().cloned());
        if !legal(board, &index, &first_escape, &second_escape, clearance)? {
            continue;
        }
        let mut escaped_copper = start.first.clone();
        escaped_copper.extend(start.second.iter().cloned());
        escaped_copper.extend(end.first.iter().cloned());
        escaped_copper.extend(end.second.iter().cloned());
        let escaped_board = with_tracks(board, &escaped_copper);
        let escaped_index = RoutingClearanceIndex::new(&escaped_board)?;
        if !can_depart(
            &escaped_board,
            &escaped_index,
            &first_rule.net,
            &second_rule.net,
            start,
            width,
            offset,
            clearance,
            layer,
            bounds.pitch_nm,
        )? {
            let (dx, dy) = HEADS[start.heading];
            let ahead = pt(start.center.x_nm + dx * bounds.pitch_nm, start.center.y_nm + dy * bounds.pitch_nm);
            if let Some((a, b)) = lane_paths(&[start.center, ahead], start.heading, start.heading, offset, start.sign) {
                let mut launch = tracks(&first_rule.net, &a, width, layer, Some(&index.breakout));
                launch.extend(tracks(&second_rule.net, &b, width, layer, Some(&index.breakout)));
                stats.reject_launch(&escaped_index, &launch)?;
            }
            continue;
        }
        stats.searches += 1;
        let mut continuation = Continuation {
            start: start.clone(),
            end: end.clone(),
            layer,
            pitch: bounds.pitch_nm,
            search_index: stats.searches,
            search: Some(SpineSearch::new(
                &first_rule.net,
                &second_rule.net,
                start,
                end,
                width,
                offset,
                clearance,
                bounds.pitch_nm,
            )),
            registered: false,
            via_pairs: Vec::new(),
            return_vias: Vec::new(),
            active: true,
            resumed: false,
            expanded_states: 0,
            legal_advances: 0,
            minimum_goal_distance_nm: f64::INFINITY,
            board: escaped_board,
        };
        let candidate = continuation.advance(budget, stats)?;
        // `register_pair_search`: retain the frontier only under an aggregate bound.
        if bounds.maximum_total_states.is_some() {
            continuation.registered = true;
            stats.continuations.push(continuation);
        }
        if let Some(found) = candidate {
            stats.candidates += 1;
            let mut first = start.first.clone();
            first.extend(found.first);
            first.extend(reversed(&end.first));
            let mut second = start.second.clone();
            second.extend(found.second);
            second.extend(reversed(&end.second));
            let candidate = PairSearchCandidate {
                first,
                second,
                expanded_states: found.expanded_states,
                search_index,
                spine: found.spine,
                start_port: Some(start.clone()),
                end_port: Some(end.clone()),
                via_pairs: Vec::new(),
                return_vias: Vec::new(),
            };
            if accept(candidate, stats)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// `pair_search_attempts`: the first sixteen frontiers' telemetry.
pub fn pair_search_attempts(stats: &PairSearchStats) -> Vec<PairSearchAttempt> {
    stats
        .continuations
        .iter()
        .take(16)
        .map(|c| PairSearchAttempt {
            layer: c.layer,
            pitch_nm: c.pitch,
            start: c.start.center,
            start_heading: c.start.heading as i64,
            start_sign: c.start.sign,
            end: c.end.center,
            end_heading: c.end.heading as i64,
            end_sign: c.end.sign,
            expanded_states: c.expanded_states,
            legal_advances: c.legal_advances,
            minimum_goal_distance_nm: c
                .minimum_goal_distance_nm
                .is_finite()
                .then(|| round_int(c.minimum_goal_distance_nm)),
        })
        .collect()
}

/// `pair_search_resume.resume_pair_candidates`: continue measured viable
/// frontiers under the original aggregate cap, closest observed approach
/// first. No new portal or search slot is created.
pub fn resume_pair_candidates(
    stats: &mut PairSearchStats,
    maximum_total_states: i64,
    accept: &mut dyn FnMut(PairSearchCandidate, &mut PairSearchStats) -> Result<bool>,
) -> Result<bool> {
    if maximum_total_states <= 0 {
        return Err(crate::Error::Value("joint pair aggregate state bound must be positive".to_owned()));
    }
    let mut continuations = std::mem::take(&mut stats.continuations);
    let mut eligible: Vec<usize> = (0..continuations.len())
        .filter(|&i| continuations[i].active && continuations[i].legal_advances != 0 && continuations[i].registered)
        .collect();
    eligible.sort_by(|&a, &b| {
        continuations[a]
            .minimum_goal_distance_nm
            .partial_cmp(&continuations[b].minimum_goal_distance_nm)
            .expect("distances")
    });
    let mut done = false;
    for i in eligible {
        let remaining = maximum_total_states - stats.expanded_states;
        if remaining <= 0 {
            break;
        }
        let c = &mut continuations[i];
        if !c.resumed {
            stats.resumed_portals += 1;
            c.resumed = true;
        }
        let before = stats.expanded_states;
        let found = c.advance(remaining, stats)?;
        stats.resumed_states += stats.expanded_states - before;
        let Some(found) = found else { continue };
        stats.candidates += 1;
        let mut first = c.start.first.clone();
        first.extend(found.first);
        first.extend(reversed(&c.end.first));
        let mut second = c.start.second.clone();
        second.extend(found.second);
        second.extend(reversed(&c.end.second));
        let candidate = PairSearchCandidate {
            first,
            second,
            expanded_states: found.expanded_states,
            search_index: c.search_index,
            spine: found.spine,
            start_port: Some(c.start.clone()),
            end_port: Some(c.end.clone()),
            via_pairs: c.via_pairs.clone(),
            return_vias: c.return_vias.clone(),
        };
        stats.continuations = std::mem::take(&mut continuations);
        let accepted = accept(candidate, stats);
        continuations = std::mem::take(&mut stats.continuations);
        if accepted? {
            done = true;
            break;
        }
    }
    stats.continuations = continuations;
    Ok(done)
}

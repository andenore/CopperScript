//! Native design-rule check of a filled board: the electrical and
//! fabrication checks of KiCad's DRC that a release gate needs, on the
//! board as plotted (zone fills included), without KiCad.
//!
//! Every copper object becomes a spine (a point, a polyline or a closed
//! polygon) grown by a radius, so pads, vias and tracks are exact; fills are
//! the saved polygons; arcs are flattened to 10 nm and grown by that error.
//! Distances are edge to edge, and a shortfall up to KiCad's DRC epsilon
//! (0.5 um) passes, as in KiCad.
//!
//! Checked: copper clearance and shorts, hole clearance, hole to hole,
//! copper to board edge (and copper outside the board), track width, via
//! diameter, annular width, hole size, keepout areas, unconnected pads,
//! dangling tracks and vias, tracks ending off a via's centre, isolated
//! copper islands and starved thermal reliefs. [`NOT_CHECKED`] lists what KiCad checks and
//! this does not; custom-rule constraints that would tighten a check made
//! here are refused.

use indexmap::{IndexMap, IndexSet};

use crate::board::{Board, Footprint, Layer, Pad, PadKind, PadShape, Shape, TrackPath, ZoneConnection};
use crate::fill::{Sets, board_area, flashes};
use crate::font;
use crate::geometry::{Point, flatten_arc, rotate, rotate_f64};
use crate::poly;
use crate::rules::{Item, Rules};

pub type Result<T> = std::result::Result<T, String>;

/// KiCad's DRC epsilon (`DRCEpsilon`, 0.0005 mm): a shortfall up to it passes.
pub const EPSILON: f64 = 500.0;
/// Chord error of flattened arcs; strokes grow by it so they never shrink.
const ARC_ERROR: f64 = 10.0;
/// Items closer than this (nm) touch.
const TOUCH: f64 = 1.0;
/// Spatial index cell size and the longest indexed segment.
const CELL: f64 = 500_000.0;
const PIECE: f64 = 1_000_000.0;

/// Custom-rule constraints that would tighten a check made here: refused,
/// since the check would silently apply the board minimum instead.
const REFUSED: &[&str] = &[
    "track_width",
    "via_diameter",
    "hole_size",
    "annular_width",
    "hole_to_hole",
    "connection_width",
    "min_resolved_spokes",
    "assertion",
];

/// KiCad checks this check does not make.
pub const NOT_CHECKED: &[&str] = &[
    "courtyards (courtyards_overlap, courtyard_clearance, malformed_courtyard, npth_inside_courtyard)",
    "silkscreen (silk_over_copper, silk_edge_clearance, silk_overlap, text_height, text_thickness)",
    "solder mask bridges (solder_mask_bridge)",
    "footprint libraries and schematic parity (lib_footprint_issues, lib_footprint_mismatch, footprint parity)",
    "length, skew and differential-pair rules",
    "keepouts that forbid footprints",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Violation {
    /// KiCad's violation type name.
    pub kind: &'static str,
    pub description: String,
    /// Board nanometres, KiCad frame (Y down).
    pub at: (f64, f64),
    pub items: Vec<String>,
    /// The KiCad UUIDs of the items, where the board gives them.
    pub uuids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub violations: Vec<Violation>,
    pub unconnected: Vec<Violation>,
    /// KiCad checks not made and custom-rule constraints not modelled.
    pub not_checked: Vec<String>,
    /// Copper objects (per layer), holes and edge segments checked.
    pub copper_items: usize,
    pub holes: usize,
    pub edge_segments: usize,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.unconnected.is_empty()
    }

    /// The report as JSON, shaped like KiCad's DRC report (positions in
    /// millimetres, Y down), plus what was not checked.
    pub fn to_json(&self, source: &str, rule_files: &[String]) -> copper_json::Value {
        use copper_json::{Object, Value};
        let object = |pairs: Vec<(&str, Value)>| {
            let mut object = Object::new();
            for (key, value) in pairs {
                object.insert(key.to_owned(), value);
            }
            Value::Object(object)
        };
        let round = |nm: f64| Value::Float((nm / 1e3).round() / 1e3);
        let entries = |list: &[Violation]| {
            Value::Array(
                list.iter()
                    .map(|violation| {
                        object(vec![
                            ("type", Value::from(violation.kind)),
                            ("severity", Value::from("error")),
                            ("description", Value::from(violation.description.as_str())),
                            ("pos", object(vec![("x", round(violation.at.0)), ("y", round(violation.at.1))])),
                            (
                                "items",
                                Value::Array(
                                    violation
                                        .items
                                        .iter()
                                        .enumerate()
                                        .map(|(index, item)| {
                                            let mut pairs = vec![("description", Value::from(item.as_str()))];
                                            if let Some(uuid) = violation.uuids.get(index) {
                                                pairs.push(("uuid", Value::from(uuid.as_str())));
                                            }
                                            object(pairs)
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            )
        };
        let strings = |list: &[String]| Value::Array(list.iter().map(|text| Value::from(text.as_str())).collect());
        object(vec![
            ("schema", Value::from("copperscript-drc/v0.1")),
            ("source", Value::from(source)),
            ("rules", strings(rule_files)),
            ("passed", Value::Bool(self.passed())),
            ("violations", entries(&self.violations)),
            ("unconnected_items", entries(&self.unconnected)),
            (
                "checked",
                object(vec![
                    ("copper_items", Value::from(self.copper_items as u64)),
                    ("holes", Value::from(self.holes as u64)),
                    ("edge_segments", Value::from(self.edge_segments as u64)),
                ]),
            ),
            ("not_checked", strings(&self.not_checked)),
        ])
    }

    /// One line per finding, then the totals.
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for violation in self.violations.iter().chain(&self.unconnected) {
            lines.push(format!(
                "  {} at {}: {} ({})",
                violation.kind,
                at_text(violation.at),
                violation.description,
                violation.items.join("; ")
            ));
        }
        let mut counts: IndexMap<&str, usize> = IndexMap::new();
        for violation in &self.violations {
            *counts.entry(violation.kind).or_default() += 1;
        }
        let counts: Vec<String> = counts.iter().map(|(kind, count)| format!("{count} {kind}")).collect();
        lines.push(format!(
            "DRC {}: {} violation(s){}, {} unconnected; checked {} copper item(s), {} hole(s), {} edge segment(s); \
             {} check group(s) not made",
            if self.passed() { "PASSED" } else { "FAILED" },
            self.violations.len(),
            if counts.is_empty() { String::new() } else { format!(" ({})", counts.join(", ")) },
            self.unconnected.len(),
            self.copper_items,
            self.holes,
            self.edge_segments,
            self.not_checked.len()
        ));
        lines.join("\n")
    }
}

type P = (f64, f64);
/// A hole, the owner of the copper it is checked against, and the pour (zone
/// and layer) or single item that owner stands for.
type HoleKey = (usize, Owner, Option<(usize, usize)>);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Pad(usize),
    Via(usize),
    Track(usize),
    Fill(usize),
    Drawing(usize),
    Text(usize),
}

/// One copper object on one layer: spines grown by `radius`. Area spines
/// are closed polygons with an interior (even-odd); others are polylines.
struct Copper<'a> {
    owner: Owner,
    layer: usize,
    net: Option<&'a str>,
    rule: Item<'a>,
    area: bool,
    paths: Vec<Vec<P>>,
    radius: f64,
}

/// A drilled hole: a point or a slot spine and its radius.
struct Hole<'a> {
    owner: Owner,
    net: Option<&'a str>,
    rule: Item<'a>,
    a: P,
    b: P,
    radius: f64,
    plated: bool,
}

#[derive(Clone, Copy, Debug)]
struct Feature {
    a: P,
    b: P,
    r: f64,
    item: usize,
}

fn lerp(a: P, b: P, t: f64) -> P {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Split `a`-`b` into indexed pieces no longer than [`PIECE`].
fn pieces(a: P, b: P, r: f64, item: usize, out: &mut Vec<Feature>) {
    let count = (((b.0 - a.0).hypot(b.1 - a.1) / PIECE).ceil() as usize).max(1);
    let mut from = a;
    for index in 1..=count {
        let to = if index == count { b } else { lerp(a, b, index as f64 / count as f64) };
        out.push(Feature { a: from, b: to, r, item });
        from = to;
    }
}

fn path_features(path: &[P], closed: bool, r: f64, item: usize, out: &mut Vec<Feature>) {
    if path.len() == 1 {
        out.push(Feature { a: path[0], b: path[0], r, item });
        return;
    }
    for pair in path.windows(2) {
        pieces(pair[0], pair[1], r, item, out);
    }
    if closed && path.len() > 2 {
        pieces(path[path.len() - 1], path[0], r, item, out);
    }
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Distance from `p` to segment `a`-`b`, and the closest point.
fn point_segment(p: P, a: P, b: P) -> (f64, P) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0) };
    let q = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - q.0).hypot(p.1 - q.1), q)
}

/// Distance between two segments, and the midpoint of the closest pair.
fn segment_distance(a: P, b: P, c: P, d: P) -> (f64, P) {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return (0.0, lerp(a, b, d1 / (d1 - d2)));
    }
    let candidates = [
        (point_segment(a, c, d), a),
        (point_segment(b, c, d), b),
        (point_segment(c, a, b), c),
        (point_segment(d, a, b), d),
    ];
    let ((distance, q), p) = candidates.into_iter().min_by(|x, y| x.0.0.total_cmp(&y.0.0)).expect("four candidates");
    (distance, lerp(p, q, 0.5))
}

/// Edge-to-edge distance of two grown features (negative when they overlap).
fn feature_distance(f: &Feature, g: &Feature) -> (f64, P) {
    let (distance, at) = segment_distance(f.a, f.b, g.a, g.b);
    (distance - f.r - g.r, at)
}

type Box2 = (P, P);

fn feature_box(f: &Feature, grow: f64) -> Box2 {
    ((f.a.0.min(f.b.0) - grow, f.a.1.min(f.b.1) - grow), (f.a.0.max(f.b.0) + grow, f.a.1.max(f.b.1) + grow))
}

/// A uniform grid of feature indices over the board.
struct Grid {
    x0: f64,
    y0: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(bounds: Box2) -> Grid {
        let nx = (((bounds.1.0 - bounds.0.0) / CELL).ceil() as usize).max(1);
        let ny = (((bounds.1.1 - bounds.0.1) / CELL).ceil() as usize).max(1);
        Grid { x0: bounds.0.0, y0: bounds.0.1, nx, ny, cells: vec![Vec::new(); nx * ny] }
    }

    fn span(&self, b: Box2) -> (usize, usize, usize, usize) {
        let column = |x: f64| (((x - self.x0) / CELL).floor().max(0.0) as usize).min(self.nx - 1);
        let row = |y: f64| (((y - self.y0) / CELL).floor().max(0.0) as usize).min(self.ny - 1);
        (column(b.0.0), row(b.0.1), column(b.1.0), row(b.1.1))
    }

    fn insert(&mut self, index: usize, f: &Feature) {
        let (x0, y0, x1, y1) = self.span(feature_box(f, f.r));
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.cells[y * self.nx + x].push(index as u32);
            }
        }
    }

    fn build(bounds: Box2, features: &[Feature]) -> Grid {
        let mut grid = Grid::new(bounds);
        for (index, feature) in features.iter().enumerate() {
            grid.insert(index, feature);
        }
        grid
    }

    /// Each feature index in the cells `b` covers, once per call.
    fn visit(&self, b: Box2, stamp: &mut [u32], token: u32, mut visit: impl FnMut(usize)) {
        let (x0, y0, x1, y1) = self.span(b);
        for y in y0..=y1 {
            for x in x0..=x1 {
                for &index in &self.cells[y * self.nx + x] {
                    if stamp[index as usize] != token {
                        stamp[index as usize] = token;
                        visit(index as usize);
                    }
                }
            }
        }
    }
}

/// Horizontal bands of area edges, for point-in-area tests.
struct Bands {
    y0: f64,
    bands: Vec<Vec<u32>>,
}

impl Bands {
    fn build(bounds: Box2, features: &[Feature], area: impl Fn(usize) -> bool) -> Bands {
        let count = (((bounds.1.1 - bounds.0.1) / CELL).ceil() as usize).max(1);
        let mut bands = vec![Vec::new(); count];
        let band = |y: f64| (((y - bounds.0.1) / CELL).floor().max(0.0) as usize).min(count - 1);
        for (index, f) in features.iter().enumerate() {
            if area(f.item) && f.a.1 != f.b.1 {
                for list in &mut bands[band(f.a.1.min(f.b.1))..=band(f.a.1.max(f.b.1))] {
                    list.push(index as u32);
                }
            }
        }
        Bands { y0: bounds.0.1, bands }
    }

    /// The area items whose polygon interior holds `p` (even-odd).
    fn inside(&self, features: &[Feature], p: P) -> Vec<usize> {
        let index = ((p.1 - self.y0) / CELL).floor();
        if index < 0.0 || index as usize >= self.bands.len() {
            return Vec::new();
        }
        let mut parity: IndexMap<usize, bool> = IndexMap::new();
        for &feature in &self.bands[index as usize] {
            let f = &features[feature as usize];
            if (f.a.1 <= p.1) != (f.b.1 <= p.1) {
                let x = f.a.0 + (p.1 - f.a.1) * (f.b.0 - f.a.0) / (f.b.1 - f.a.1);
                if x > p.0 {
                    *parity.entry(f.item).or_default() ^= true;
                }
            }
        }
        parity.into_iter().filter(|(_, odd)| *odd).map(|(item, _)| item).collect()
    }
}

/// Where segment `a`-`b` crosses `c`-`d` (touching ends included).
fn crossing(a: P, b: P, c: P, d: P) -> Option<P> {
    let (r, s) = ((b.0 - a.0, b.1 - a.1), (d.0 - c.0, d.1 - c.1));
    let denominator = r.0 * s.1 - r.1 * s.0;
    if denominator == 0.0 {
        return None;
    }
    let (qx, qy) = (c.0 - a.0, c.1 - a.1);
    let t = (qx * s.1 - qy * s.0) / denominator;
    let u = (qx * r.1 - qy * r.0) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((a.0 + t * r.0, a.1 + t * r.1))
}

/// Even-odd point in polygon.
fn even_odd(polygon: &[P], p: P) -> bool {
    let mut inside = false;
    for index in 0..polygon.len() {
        let (a, b) = (polygon[index], polygon[(index + 1) % polygon.len()]);
        if (a.1 <= p.1) != (b.1 <= p.1) && a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) > p.0 {
            inside = !inside;
        }
    }
    inside
}

/// A pad's outline grown by `growth`, curves erring outside by `max_error`.
fn grown_pad(pad: &Pad, growth: f64, max_error: f64) -> Vec<P> {
    let (w, h) = if pad.shape == PadShape::Circle {
        (pad.width as f64, pad.width as f64)
    } else {
        (pad.width as f64, pad.height as f64)
    };
    let radius = match pad.shape {
        PadShape::Circle | PadShape::Oval => w.min(h) / 2.0,
        PadShape::RoundRect(ratio) => (w.min(h) * ratio).round(),
        PadShape::Rect => 0.0,
    };
    let shapes =
        poly::rounded_rect(pad.center, w + 2.0 * growth, h + 2.0 * growth, radius + growth, pad.angle, max_error);
    shapes
        .first()
        .and_then(|shape| shape.first())
        .map(|contour| contour.iter().map(|p| (p.x as f64, p.y as f64)).collect())
        .unwrap_or_default()
}

fn mm(nm: f64) -> String {
    format!("{:.4}", nm / 1e6)
}

fn at_text(p: P) -> String {
    format!("({}, {}) mm", mm(p.0), mm(p.1))
}

fn net_text(net: Option<&str>) -> String {
    net.map_or("<no net>".to_owned(), |net| format!("[{net}]"))
}

struct Checker<'a> {
    board: &'a Board,
    rules: &'a Rules,
    pads: Vec<(&'a Footprint, &'a Pad)>,
    items: Vec<Copper<'a>>,
    holes: Vec<Hole<'a>>,
    violations: Vec<Violation>,
}

fn same_net(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_some() && a == b
}

/// A pad's copper as a grown spine: area, paths and radius.
fn pad_geometry(pad: &Pad) -> (bool, Vec<Vec<P>>, f64) {
    let center = (pad.center.x as f64, pad.center.y as f64);
    let place = |x: f64, y: f64| {
        let (rx, ry) = rotate_f64(x, y, pad.angle);
        (center.0 + rx, center.1 + ry)
    };
    let (w, h) = if pad.shape == PadShape::Circle {
        (pad.width as f64, pad.width as f64)
    } else {
        (pad.width as f64, pad.height as f64)
    };
    let radius = match pad.shape {
        PadShape::Circle | PadShape::Oval => w.min(h) / 2.0,
        PadShape::RoundRect(ratio) => (w.min(h) * ratio).round().clamp(0.0, w.min(h) / 2.0),
        PadShape::Rect => 0.0,
    };
    let (hx, hy) = (w / 2.0 - radius, h / 2.0 - radius);
    if hx <= 0.0 && hy <= 0.0 {
        (false, vec![vec![center]], radius)
    } else if hx <= 0.0 || hy <= 0.0 {
        (false, vec![vec![place(-hx.max(0.0), -hy.max(0.0)), place(hx.max(0.0), hy.max(0.0))]], radius)
    } else {
        (true, vec![vec![place(hx, -hy), place(hx, hy), place(-hx, hy), place(-hx, -hy)]], radius)
    }
}

fn point(p: Point) -> P {
    (p.x as f64, p.y as f64)
}

fn open_path(mut points: Vec<P>) -> Vec<P> {
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

impl<'a> Checker<'a> {
    fn describe(&self, item: &Copper) -> String {
        let layer = &self.board.copper_layers[item.layer];
        match item.owner {
            Owner::Pad(k) => {
                let (footprint, pad) = self.pads[k];
                format!("pad {}.{} {} on {layer}", footprint.reference, pad.number, net_text(item.net))
            }
            Owner::Via(k) => format!("via {} at {}", net_text(item.net), at_text(point(self.board.vias[k].center))),
            Owner::Track(k) => {
                let track = &self.board.tracks[k];
                format!("track {} {} mm wide on {layer}", net_text(item.net), mm(track.width as f64))
            }
            Owner::Fill(_) => format!("zone fill {} on {layer}", net_text(item.net)),
            Owner::Drawing(_) => format!("copper graphic {} on {layer}", net_text(item.net)),
            Owner::Text(k) => format!("copper text {:?} on {layer}", self.board.texts[k].text),
        }
    }

    fn describe_hole(&self, hole: &Hole) -> String {
        match hole.owner {
            Owner::Pad(k) => {
                let (footprint, pad) = self.pads[k];
                let kind = if hole.plated { "plated" } else { "non-plated" };
                format!("{kind} hole of pad {}.{} {}", footprint.reference, pad.number, net_text(hole.net))
            }
            _ => format!("via hole {} at {}", net_text(hole.net), at_text(hole.a)),
        }
    }

    fn report(&mut self, kind: &'static str, description: String, at: P, items: Vec<String>) {
        self.report_on(kind, description, at, items, Vec::new());
    }

    /// A finding with the KiCad UUIDs of its items (unknown ones left out,
    /// so the list may be shorter than `items`).
    fn report_on(&mut self, kind: &'static str, description: String, at: P, items: Vec<String>, uuids: Vec<String>) {
        if !self.rules.ignores(kind) {
            self.violations.push(Violation { kind, description, at, items, uuids });
        }
    }

    /// The KiCad UUID of a copper object: its own, or its zone's for a fill.
    fn uuid(&self, item: &Copper) -> Option<String> {
        let board = self.board;
        match item.owner {
            Owner::Pad(k) => self.pads[k].1.uuid.clone(),
            Owner::Via(k) => board.vias[k].uuid.clone(),
            Owner::Track(k) => board.tracks[k].uuid.clone(),
            Owner::Fill(k) => {
                board.fills[k].zone.map(|zone| board.zones[zone].uuid.clone()).filter(|uuid| !uuid.is_empty())
            }
            Owner::Drawing(_) | Owner::Text(_) => None,
        }
    }

    fn uuids(&self, items: &[&Copper]) -> Vec<String> {
        items.iter().filter_map(|item| self.uuid(item)).collect()
    }

    fn hole_uuid(&self, hole: &Hole) -> Option<String> {
        match hole.owner {
            Owner::Pad(k) => self.pads[k].1.uuid.clone(),
            Owner::Via(k) => self.board.vias[k].uuid.clone(),
            _ => None,
        }
    }

    fn collect(&mut self) -> Result<()> {
        let board = self.board;
        let count = board.copper_count();
        let pads = self.pads.clone();
        for (k, (footprint, pad)) in pads.iter().enumerate() {
            let rule = Item {
                net: pad.net.as_deref(),
                reference: Some(&footprint.reference),
                local_clearance: pad.zones.clearance.or(footprint.zones.clearance),
            };
            let (area, paths, radius) = pad_geometry(pad);
            for layer in (0..count).filter(|&layer| flashes(pad, layer)) {
                self.items.push(Copper {
                    owner: Owner::Pad(k),
                    layer,
                    net: pad.net.as_deref(),
                    rule,
                    area,
                    paths: paths.clone(),
                    radius,
                });
            }
            if let Some((w, h)) = pad.drill {
                let (radius, half) = if w >= h {
                    (h as f64 / 2.0, Point::new((w - h) / 2, 0))
                } else {
                    (w as f64 / 2.0, Point::new(0, (h - w) / 2))
                };
                let offset = point(rotate(half, pad.angle));
                let c = point(pad.center);
                self.holes.push(Hole {
                    owner: Owner::Pad(k),
                    net: pad.net.as_deref(),
                    rule,
                    a: (c.0 - offset.0, c.1 - offset.1),
                    b: (c.0 + offset.0, c.1 + offset.1),
                    radius,
                    plated: pad.kind == PadKind::ThroughHole,
                });
            }
        }
        for (k, via) in board.vias.iter().enumerate() {
            let rule = Item { net: via.net.as_deref(), ..Item::default() };
            for layer in 0..count {
                self.items.push(Copper {
                    owner: Owner::Via(k),
                    layer,
                    net: via.net.as_deref(),
                    rule,
                    area: false,
                    paths: vec![vec![point(via.center)]],
                    radius: via.size as f64 / 2.0,
                });
            }
            let c = point(via.center);
            self.holes.push(Hole {
                owner: Owner::Via(k),
                net: via.net.as_deref(),
                rule,
                a: c,
                b: c,
                radius: via.drill as f64 / 2.0,
                plated: true,
            });
        }
        for (k, track) in board.tracks.iter().enumerate() {
            let (path, radius) = match track.path {
                TrackPath::Line { start, end } => (vec![point(start), point(end)], track.width as f64 / 2.0),
                TrackPath::Arc { start, mid, end } => {
                    (flatten_arc(start, mid, end, ARC_ERROR), track.width as f64 / 2.0 + ARC_ERROR)
                }
            };
            self.items.push(Copper {
                owner: Owner::Track(k),
                layer: track.layer,
                net: track.net.as_deref(),
                rule: Item { net: track.net.as_deref(), ..Item::default() },
                area: false,
                paths: vec![path],
                radius,
            });
        }
        for (k, fill) in board.fills.iter().enumerate() {
            let Layer::Copper(layer) = fill.layer else { continue };
            let zone = fill.zone.map(|zone| &board.zones[zone]);
            self.items.push(Copper {
                owner: Owner::Fill(k),
                layer,
                net: fill.net.as_deref(),
                rule: Item {
                    net: fill.net.as_deref(),
                    reference: None,
                    local_clearance: zone.and_then(|zone| zone.clearance),
                },
                area: true,
                paths: vec![open_path(fill.contour.flattened(ARC_ERROR))],
                radius: 0.0,
            });
        }
        for (k, drawing) in board.drawings.iter().enumerate() {
            let Layer::Copper(layer) = drawing.layer else { continue };
            let half = drawing.width as f64 / 2.0;
            let (area, path, radius) = match &drawing.shape {
                Shape::Line { start, end } => (false, vec![point(*start), point(*end)], half),
                Shape::Arc { start, mid, end } => (false, flatten_arc(*start, *mid, *end, ARC_ERROR), half + ARC_ERROR),
                Shape::Circle { center, radius } if drawing.filled => {
                    (false, vec![point(*center)], *radius as f64 + half)
                }
                Shape::Circle { center, radius } => {
                    let ring = flatten_arc(
                        Point::new(center.x + radius, center.y),
                        Point::new(center.x - radius, center.y),
                        Point::new(center.x + radius, center.y),
                        ARC_ERROR,
                    );
                    (false, ring, half + ARC_ERROR)
                }
                Shape::Polygon(contour) => {
                    let mut points = open_path(contour.flattened(ARC_ERROR));
                    if drawing.filled {
                        (true, points, half + ARC_ERROR)
                    } else {
                        points.push(points[0]);
                        (false, points, half + ARC_ERROR)
                    }
                }
            };
            self.items.push(Copper {
                owner: Owner::Drawing(k),
                layer,
                net: drawing.net.as_deref(),
                rule: Item { net: drawing.net.as_deref(), ..Item::default() },
                area,
                paths: vec![path],
                radius,
            });
        }
        for (k, text) in board.texts.iter().enumerate() {
            let Layer::Copper(layer) = text.layer else { continue };
            let (strokes, pen) = font::strokes(text)?;
            if strokes.is_empty() {
                continue;
            }
            self.items.push(Copper {
                owner: Owner::Text(k),
                layer,
                net: None,
                rule: Item::default(),
                area: false,
                paths: strokes,
                radius: pen as f64 / 2.0,
            });
        }
        Ok(())
    }

    fn features(&self, item: usize, out: &mut Vec<Feature>) {
        let copper = &self.items[item];
        for path in &copper.paths {
            path_features(path, copper.area, copper.radius, item, out);
        }
    }

    fn sizes(&mut self) {
        let rules = self.rules;
        let board = self.board;
        let short = |actual: f64, minimum: i64| minimum > 0 && actual < minimum as f64 - EPSILON;
        for track in &board.tracks {
            if short(track.width as f64, rules.min_track_width) {
                let at = match track.path {
                    TrackPath::Line { start, .. } | TrackPath::Arc { start, .. } => point(start),
                };
                self.report(
                    "track_width",
                    format!(
                        "track width {} mm is below the minimum {} mm",
                        mm(track.width as f64),
                        mm(rules.min_track_width as f64)
                    ),
                    at,
                    vec![format!("track {} on {}", net_text(track.net.as_deref()), board.copper_layers[track.layer])],
                );
            }
        }
        for via in &board.vias {
            let name = vec![format!("via {} at {}", net_text(via.net.as_deref()), at_text(point(via.center)))];
            let at = point(via.center);
            if short(via.size as f64, rules.min_via_diameter) {
                let text = format!(
                    "via diameter {} mm is below the minimum {} mm",
                    mm(via.size as f64),
                    mm(rules.min_via_diameter as f64)
                );
                self.report("via_diameter", text, at, name.clone());
            }
            if short(via.drill as f64, rules.min_through_hole_diameter) {
                let text = format!(
                    "via hole {} mm is below the minimum {} mm",
                    mm(via.drill as f64),
                    mm(rules.min_through_hole_diameter as f64)
                );
                self.report("drill_out_of_range", text, at, name.clone());
            }
            let ring = (via.size - via.drill) as f64 / 2.0;
            if short(ring, rules.min_via_annular_width) {
                let text = format!(
                    "annular width {} mm is below the minimum {} mm",
                    mm(ring),
                    mm(rules.min_via_annular_width as f64)
                );
                self.report("annular_width", text, at, name);
            }
        }
        let pads = self.pads.clone();
        for (footprint, pad) in pads {
            let Some((dw, dh)) = pad.drill else { continue };
            let name = vec![format!("pad {}.{} {}", footprint.reference, pad.number, net_text(pad.net.as_deref()))];
            let at = point(pad.center);
            let drill = dw.min(dh) as f64;
            if short(drill, rules.min_through_hole_diameter) {
                let text = format!(
                    "hole {} mm is below the minimum {} mm",
                    mm(drill),
                    mm(rules.min_through_hole_diameter as f64)
                );
                self.report("drill_out_of_range", text, at, name.clone());
            }
            let copper = (0..board.copper_count()).any(|layer| flashes(pad, layer));
            if pad.kind == PadKind::ThroughHole && copper {
                let (w, h) =
                    if pad.shape == PadShape::Circle { (pad.width, pad.width) } else { (pad.width, pad.height) };
                let ring = ((w - dw) as f64 / 2.0).min((h - dh) as f64 / 2.0);
                if short(ring, rules.min_via_annular_width) {
                    let text = format!(
                        "annular width {} mm is below the minimum {} mm",
                        mm(ring),
                        mm(rules.min_via_annular_width as f64)
                    );
                    self.report("annular_width", text, at, name);
                }
            }
        }
    }

    /// KiCad's `starved_thermal` on `layer`: each thermally connected pad
    /// needs two spokes (the default `min_resolved_spokes`). A spoke is a pair
    /// of crossings of a fill outline with the pad grown by half the relief
    /// gap; spokes into an island touching one object only are counted only
    /// when there is nothing else, and connected tracks from inside the pad
    /// into the fill count as spokes.
    fn starved_thermals(
        &self,
        layer: usize,
        features: &[Feature],
        grid: &Grid,
        stamp: &mut [u32],
        token: &mut u32,
        contacts: &IndexMap<usize, IndexSet<Owner>>,
    ) -> Vec<(usize, usize, P, String)> {
        let board = self.board;
        let mut found = Vec::new();
        for (z, zone) in board.zones.iter().enumerate() {
            if zone.keepout.is_some() || !zone.layers.contains(&layer) || zone.net.is_none() {
                continue;
            }
            let fills: Vec<usize> = (0..self.items.len())
                .filter(|&i| {
                    let item = &self.items[i];
                    item.layer == layer && matches!(item.owner, Owner::Fill(k) if board.fills[k].zone == Some(z))
                })
                .collect();
            if fills.is_empty() {
                continue;
            }
            let zone_box = zone.outline.iter().flat_map(|path| path.flattened(ARC_ERROR)).fold(None::<Box2>, |b, p| {
                Some(b.map_or((p, p), |b| ((b.0.0.min(p.0), b.0.1.min(p.1)), (b.1.0.max(p.0), b.1.1.max(p.1)))))
            });
            let Some(zone_box) = zone_box else { continue };
            for (k, (footprint, pad)) in self.pads.iter().enumerate() {
                if pad.net != zone.net || !flashes(pad, layer) {
                    continue;
                }
                let reach = pad.width.max(pad.height) as f64;
                let c = point(pad.center);
                if c.0 + reach < zone_box.0.0
                    || c.0 - reach > zone_box.1.0
                    || c.1 + reach < zone_box.0.1
                    || c.1 - reach > zone_box.1.1
                {
                    continue;
                }
                if crate::fill::effective_connection(footprint, pad, zone, layer) != ZoneConnection::Thermal {
                    continue;
                }
                let gap = pad.zones.thermal_gap.or(footprint.zones.thermal_gap).unwrap_or(zone.thermal_gap);
                let outline = grown_pad(pad, (gap / 2) as f64, self.rules.max_error as f64);
                let (lo, hi) = outline.iter().fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(lo, hi), p| {
                    ((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)))
                });
                // Crossings of each fill outline with the grown pad.
                let mut crossings: IndexMap<usize, IndexSet<(i64, i64)>> = IndexMap::new();
                *token = token.wrapping_add(1);
                grid.visit((lo, hi), stamp, *token, |gi| {
                    let g = &features[gi];
                    if !fills.contains(&g.item) {
                        return;
                    }
                    for index in 0..outline.len() {
                        let (a, b) = (outline[index], outline[(index + 1) % outline.len()]);
                        if let Some(x) = crossing(a, b, g.a, g.b) {
                            crossings.entry(g.item).or_default().insert((x.0.round() as i64, x.1.round() as i64));
                        }
                    }
                });
                let (mut spokes, mut ignored) = (0, 0);
                let mut ignored_at = c;
                for (fill, points) in &crossings {
                    if points.len() < 2 {
                        continue;
                    }
                    if contacts.get(fill).map_or(0, IndexSet::len) == 1 {
                        ignored += points.len() / 2;
                        let (a, b) = (points[0], points[1]);
                        ignored_at = ((a.0 + b.0) as f64 / 2.0, (a.1 + b.1) as f64 / 2.0);
                    } else {
                        spokes += points.len() / 2;
                    }
                }
                if spokes == 0 && ignored == 0 {
                    continue;
                }
                if spokes < 2 {
                    // Tracks from inside the grown pad into the fill.
                    let in_fill = |p: P| fills.iter().any(|&f| even_odd(&self.items[f].paths[0], p));
                    for track in board.tracks.iter().filter(|t| t.layer == layer && t.net == pad.net) {
                        let (start, end) = match track.path {
                            TrackPath::Line { start, end } | TrackPath::Arc { start, end, .. } => {
                                (point(start), point(end))
                            }
                        };
                        if (even_odd(&outline, start) && in_fill(end)) || (even_odd(&outline, end) && in_fill(start)) {
                            spokes += 1;
                        }
                    }
                }
                if spokes == 0 {
                    spokes += ignored;
                    ignored = 0;
                }
                if spokes < 2 {
                    let layer_name = &board.copper_layers[layer];
                    let (text, at) = if ignored > 0 {
                        (format!("layer {layer_name}: {ignored} spoke(s) connected to an isolated island"), ignored_at)
                    } else {
                        (format!("layer {layer_name}: {spokes} of the minimum 2 thermal spokes connected"), c)
                    };
                    found.push((z, k, at, text));
                }
            }
        }
        found
    }

    /// KiCad's `track_not_centered_on_via`: a track ending inside a via of
    /// its net but off its centre, when no track on the layer reaches the
    /// centre.
    fn off_centre(&self, layer_items: &[usize]) -> Vec<(usize, usize, P)> {
        let board = self.board;
        let ends = |k: usize| match board.tracks[k].path {
            TrackPath::Line { start, end } | TrackPath::Arc { start, end, .. } => [start, end],
        };
        let vias: Vec<usize> =
            layer_items.iter().copied().filter(|&i| matches!(self.items[i].owner, Owner::Via(_))).collect();
        let tracks: Vec<usize> =
            layer_items.iter().copied().filter(|&i| matches!(self.items[i].owner, Owner::Track(_))).collect();
        let mut centred: IndexSet<usize> = IndexSet::new();
        let mut at_centre: IndexMap<(i64, i64), Vec<usize>> = IndexMap::new();
        for &v in &vias {
            let Owner::Via(k) = self.items[v].owner else { continue };
            at_centre.entry((board.vias[k].center.x, board.vias[k].center.y)).or_default().push(v);
        }
        for &t in &tracks {
            let Owner::Track(k) = self.items[t].owner else { continue };
            for end in ends(k) {
                for &v in at_centre.get(&(end.x, end.y)).into_iter().flatten() {
                    if same_net(self.items[t].net, self.items[v].net) {
                        centred.insert(v);
                    }
                }
            }
        }
        let mut found = Vec::new();
        for &t in &tracks {
            let Owner::Track(k) = self.items[t].owner else { continue };
            'via: for &v in &vias {
                if centred.contains(&v) || !same_net(self.items[t].net, self.items[v].net) {
                    continue;
                }
                let Owner::Via(j) = self.items[v].owner else { continue };
                let via = &board.vias[j];
                for end in ends(k) {
                    let distance = ((end.x - via.center.x) as f64).hypot((end.y - via.center.y) as f64);
                    if distance <= via.size as f64 / 2.0 && end != via.center {
                        found.push((t, v, point(end)));
                        break 'via;
                    }
                }
            }
        }
        found
    }

    fn holes_to_holes(&mut self) {
        let minimum = self.rules.min_hole_to_hole as f64;
        if minimum <= 0.0 {
            return;
        }
        let mut order: Vec<usize> = (0..self.holes.len()).collect();
        let left = |hole: &Hole| hole.a.0.min(hole.b.0) - hole.radius;
        order.sort_by(|&a, &b| left(&self.holes[a]).total_cmp(&left(&self.holes[b])).then(a.cmp(&b)));
        let mut found = Vec::new();
        for (position, &i) in order.iter().enumerate() {
            let a = &self.holes[i];
            let right = a.a.0.max(a.b.0) + a.radius + minimum;
            for &j in &order[position + 1..] {
                let b = &self.holes[j];
                if left(b) > right {
                    break;
                }
                if a.owner == b.owner {
                    continue;
                }
                let (distance, at) = segment_distance(a.a, a.b, b.a, b.b);
                let gap = distance - a.radius - b.radius;
                if gap < minimum - EPSILON {
                    found.push((i.min(j), i.max(j), gap, at));
                }
            }
        }
        found.sort_by_key(|entry| (entry.0, entry.1));
        for (i, j, gap, at) in found {
            let items = vec![self.describe_hole(&self.holes[i]), self.describe_hole(&self.holes[j])];
            let text = format!("hole to hole {} mm is below the minimum {} mm", mm(gap.max(0.0)), mm(minimum));
            self.report("hole_to_hole", text, at, items);
        }
    }

    /// Edge segments: Edge.Cuts graphics and non-plated slots (milled).
    fn edges(&self) -> (Vec<Feature>, Vec<Option<Owner>>) {
        let mut features = Vec::new();
        let mut owners = Vec::new();
        for drawing in self.board.drawings.iter().filter(|drawing| drawing.layer == Layer::EdgeCuts) {
            let index = owners.len();
            owners.push(None);
            match &drawing.shape {
                Shape::Line { start, end } => pieces(point(*start), point(*end), 0.0, index, &mut features),
                Shape::Arc { start, mid, end } => {
                    path_features(&flatten_arc(*start, *mid, *end, ARC_ERROR), false, 0.0, index, &mut features)
                }
                Shape::Circle { center, radius } => {
                    let ring = flatten_arc(
                        Point::new(center.x + radius, center.y),
                        Point::new(center.x - radius, center.y),
                        Point::new(center.x + radius, center.y),
                        ARC_ERROR,
                    );
                    path_features(&ring, false, 0.0, index, &mut features);
                }
                Shape::Polygon(contour) => {
                    path_features(&open_path(contour.flattened(ARC_ERROR)), true, 0.0, index, &mut features)
                }
            }
        }
        for hole in self.holes.iter().filter(|hole| !hole.plated && hole.a != hole.b) {
            let index = owners.len();
            owners.push(Some(hole.owner));
            features.push(Feature { a: hole.a, b: hole.b, r: hole.radius, item: index });
        }
        (features, owners)
    }

    fn run(&mut self) -> Result<Vec<Violation>> {
        let board = self.board;
        let rules = self.rules;
        let count = board.copper_count();
        let reach = rules.worst_clearance() as f64 + EPSILON;
        // Per-layer features of every copper object.
        let mut layer_features: Vec<Vec<Feature>> = vec![Vec::new(); count];
        // Each item's features: a range of its layer's list.
        let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(self.items.len());
        for item in 0..self.items.len() {
            let layer = self.items[item].layer;
            let from = layer_features[layer].len();
            self.features(item, &mut layer_features[layer]);
            ranges.push((from, layer_features[layer].len()));
        }
        let (edge_features, edge_owners) = self.edges();
        let mut bounds: Option<Box2> = None;
        let mut extend = |f: &Feature| {
            let (lo, hi) = feature_box(f, f.r);
            let b = bounds.get_or_insert((lo, hi));
            *b = ((b.0.0.min(lo.0), b.0.1.min(lo.1)), (b.1.0.max(hi.0), b.1.1.max(hi.1)));
        };
        layer_features.iter().flatten().chain(&edge_features).for_each(&mut extend);
        let Some(bounds) = bounds else { return Ok(Vec::new()) };
        let bounds = ((bounds.0.0 - reach, bounds.0.1 - reach), (bounds.1.0 + reach, bounds.1.1 + reach));
        let outline = board_area(board, ARC_ERROR)?;

        let mut sets = Sets((0..self.items.len()).collect());
        let mut touched = vec![false; self.items.len()];
        // The distinct objects each fill outline touches directly (KiCad's
        // connected items of a zone layer), for single-connection islands.
        let mut contacts: IndexMap<usize, IndexSet<Owner>> = IndexMap::new();
        let is_fill = |item: usize| matches!(self.items[item].owner, Owner::Fill(_));
        let mut starved: Vec<(usize, usize, P, String)> = Vec::new();
        let mut off_centre: Vec<(usize, usize, P)> = Vec::new();
        // Closest approach of each different-net item pair, hole-item pair
        // and item-edge pair within reach.
        let mut pairs: IndexMap<(usize, usize), (f64, P)> = IndexMap::new();
        let mut hole_pairs: IndexMap<(usize, usize), (f64, P)> = IndexMap::new();
        let mut edge_gaps: IndexMap<usize, (f64, P, usize)> = IndexMap::new();
        let mut keepout_hits: IndexMap<(usize, usize), P> = IndexMap::new();
        let mut dangling: Vec<(usize, P)> = Vec::new();
        let edge_grid = Grid::build(bounds, &edge_features);
        let mut edge_stamp = vec![u32::MAX; edge_features.len()];
        let mut token: u32 = 0;
        let next = |token: &mut u32| {
            *token = token.wrapping_add(1);
            *token
        };
        let keep = |map: &mut IndexMap<(usize, usize), (f64, P)>, key: (usize, usize), d: f64, at: P| {
            map.entry(key)
                .and_modify(|best| {
                    if d < best.0 {
                        *best = (d, at)
                    }
                })
                .or_insert((d, at));
        };
        for (layer, features) in layer_features.iter().enumerate() {
            let grid = Grid::build(bounds, features);
            let bands = Bands::build(bounds, features, |item| self.items[item].area);
            let mut stamp = vec![u32::MAX; features.len()];
            for (fi, f) in features.iter().enumerate() {
                let a = f.item;
                let t = next(&mut token);
                grid.visit(feature_box(f, f.r + reach), &mut stamp, t, |gi| {
                    let g = &features[gi];
                    let b = g.item;
                    if gi <= fi || a == b || self.items[a].owner == self.items[b].owner {
                        return;
                    }
                    if same_net(self.items[a].net, self.items[b].net) {
                        let fills = is_fill(a) || is_fill(b);
                        if touched[a] && touched[b] && !fills && sets.find(a) == sets.find(b) {
                            return;
                        }
                        if fills
                            && contacts.get(&a).is_some_and(|seen| seen.contains(&self.items[b].owner))
                            && contacts.get(&b).is_some_and(|seen| seen.contains(&self.items[a].owner))
                        {
                            return;
                        }
                        if feature_distance(f, g).0 <= TOUCH {
                            sets.join(a, b);
                            touched[a] = true;
                            touched[b] = true;
                            if is_fill(a) {
                                contacts.entry(a).or_default().insert(self.items[b].owner);
                            }
                            if is_fill(b) {
                                contacts.entry(b).or_default().insert(self.items[a].owner);
                            }
                        }
                        return;
                    }
                    let (d, at) = feature_distance(f, g);
                    if d < reach {
                        keep(&mut pairs, (a.min(b), a.max(b)), d, at);
                    }
                });
                // Board edges.
                let t = next(&mut token);
                edge_grid.visit(feature_box(f, f.r + reach), &mut edge_stamp, t, |ei| {
                    let e = &edge_features[ei];
                    if edge_owners[e.item] == Some(self.items[a].owner) {
                        return;
                    }
                    let (d, at) = feature_distance(f, e);
                    if d < reach {
                        edge_gaps
                            .entry(a)
                            .and_modify(|best| {
                                if d < best.0 {
                                    *best = (d, at, e.item)
                                }
                            })
                            .or_insert((d, at, e.item));
                    }
                });
            }
            // Containment: an object's first spine point inside another's area.
            let layer_items: Vec<usize> = (0..self.items.len()).filter(|&i| self.items[i].layer == layer).collect();
            for &i in &layer_items {
                let anchor = self.items[i].paths[0][0];
                for j in bands.inside(features, anchor) {
                    if j == i || self.items[i].owner == self.items[j].owner {
                        continue;
                    }
                    if same_net(self.items[i].net, self.items[j].net) {
                        sets.join(i, j);
                        touched[i] = true;
                        touched[j] = true;
                        if is_fill(i) {
                            contacts.entry(i).or_default().insert(self.items[j].owner);
                        }
                        if is_fill(j) {
                            contacts.entry(j).or_default().insert(self.items[i].owner);
                        }
                    } else {
                        keep(&mut pairs, (i.min(j), i.max(j)), -1.0, anchor);
                    }
                }
                if !poly::shapes_contain(&outline, anchor) {
                    edge_gaps.insert(i, (-1.0, anchor, usize::MAX));
                }
            }
            starved.extend(self.starved_thermals(layer, features, &grid, &mut stamp, &mut token, &contacts));
            off_centre.extend(self.off_centre(&layer_items));
            // Holes against this layer's copper.
            let mut hole_stamp = vec![u32::MAX; features.len()];
            for (h, hole) in self.holes.iter().enumerate() {
                let probe = Feature { a: hole.a, b: hole.b, r: hole.radius, item: usize::MAX };
                let t = next(&mut token);
                grid.visit(feature_box(&probe, hole.radius + reach), &mut hole_stamp, t, |gi| {
                    let g = &features[gi];
                    let item = &self.items[g.item];
                    if item.owner == hole.owner || same_net(hole.net, item.net) {
                        return;
                    }
                    let (d, at) = feature_distance(&probe, g);
                    if d < reach {
                        keep(&mut hole_pairs, (h, g.item), d, at);
                    }
                });
                for j in bands.inside(features, hole.a) {
                    let item = &self.items[j];
                    if item.owner != hole.owner && !same_net(hole.net, item.net) {
                        keep(&mut hole_pairs, (h, j), -1.0, hole.a);
                    }
                }
            }
            // Keepouts that forbid tracks, vias, pads or pours on this layer.
            for (z, zone) in board.zones.iter().enumerate() {
                if zone.keepout.is_none() || !zone.layers.contains(&layer) {
                    continue;
                }
                let forbids = |owner: Owner| {
                    let kind = match owner {
                        Owner::Track(_) => "tracks",
                        Owner::Via(_) => "vias",
                        Owner::Pad(_) => "pads",
                        Owner::Fill(_) => "copperpour",
                        _ => return false,
                    };
                    zone.not_allowed.iter().any(|forbidden| forbidden == kind)
                };
                let contours: Vec<Vec<P>> =
                    zone.outline.iter().map(|path| open_path(path.flattened(ARC_ERROR))).collect();
                let mut keepout_features = Vec::new();
                for contour in &contours {
                    path_features(contour, true, 0.0, 0, &mut keepout_features);
                }
                let shapes = poly::even_odd(
                    &contours
                        .iter()
                        .map(|contour| {
                            contour
                                .iter()
                                .map(|&(x, y)| {
                                    i_overlay::i_float::int::point::IntPoint::new(x.round() as i64, y.round() as i64)
                                })
                                .collect()
                        })
                        .collect::<Vec<_>>(),
                );
                let probe_stamp = &mut vec![u32::MAX; features.len()];
                for k in &keepout_features {
                    let t = next(&mut token);
                    grid.visit(feature_box(k, 0.0), probe_stamp, t, |gi| {
                        let g = &features[gi];
                        if !forbids(self.items[g.item].owner) || keepout_hits.contains_key(&(z, g.item)) {
                            return;
                        }
                        let (d, at) = feature_distance(k, g);
                        if d < -EPSILON {
                            keepout_hits.insert((z, g.item), at);
                        }
                    });
                }
                for &i in &layer_items {
                    let anchor = self.items[i].paths[0][0];
                    if forbids(self.items[i].owner)
                        && !keepout_hits.contains_key(&(z, i))
                        && poly::shapes_contain(&shapes, anchor)
                    {
                        keepout_hits.insert((z, i), anchor);
                    }
                }
            }
            // Dangling tracks, as KiCad decides them: each connected item of
            // the net counts for the track end it hit-tests within the
            // track's half width (the nearer end when a track or via reaches
            // both; both ends for a pad, pour or graphic), and a track is
            // dangling unless both ends are counted.
            for &i in &layer_items {
                let item = &self.items[i];
                if !matches!(item.owner, Owner::Track(_)) {
                    continue;
                }
                let mut connected: IndexSet<usize> = IndexSet::new();
                for f in &features[ranges[i].0..ranges[i].1] {
                    let t = next(&mut token);
                    grid.visit(feature_box(f, f.r + TOUCH), &mut stamp, t, |gi| {
                        let g = &features[gi];
                        let j = g.item;
                        if j != i
                            && self.items[j].owner != item.owner
                            && same_net(item.net, self.items[j].net)
                            && !connected.contains(&j)
                            && feature_distance(f, g).0 <= TOUCH
                        {
                            connected.insert(j);
                        }
                    });
                }
                let path = &item.paths[0];
                let (start, end) = (path[0], path[path.len() - 1]);
                for p in [start, end] {
                    for j in bands.inside(features, p) {
                        if j != i && same_net(item.net, self.items[j].net) {
                            connected.insert(j);
                        }
                    }
                }
                let accuracy = item.radius;
                let inside_start = bands.inside(features, start);
                let inside_end = bands.inside(features, end);
                let hit = |j: usize, p: P, inside: &[usize]| {
                    features[ranges[j].0..ranges[j].1].iter().any(|g| point_segment(p, g.a, g.b).0 <= g.r + accuracy)
                        || inside.contains(&j)
                };
                let distance = |a: P, b: P| (a.0 - b.0).hypot(a.1 - b.1);
                let min_distance = |j: usize, p: P| {
                    let other = &self.items[j];
                    match other.owner {
                        Owner::Track(_) => {
                            let path = &other.paths[0];
                            distance(path[0], p).min(distance(path[path.len() - 1], p))
                        }
                        _ => distance(other.paths[0][0], p),
                    }
                };
                let (mut at_start, mut at_end) = (0, 0);
                for &j in &connected {
                    match (hit(j, start, &inside_start), hit(j, end, &inside_end)) {
                        // A track inside a pad or a pour is not dangling.
                        (true, true) if !matches!(self.items[j].owner, Owner::Track(_) | Owner::Via(_)) => {
                            at_start += 1;
                            at_end += 1;
                        }
                        (true, true) if min_distance(j, start) < min_distance(j, end) => at_start += 1,
                        (true, true) | (false, true) => at_end += 1,
                        (true, false) => at_start += 1,
                        (false, false) => {}
                    }
                    if at_start > 0 && at_end > 0 {
                        break;
                    }
                }
                if at_start == 0 || at_end == 0 {
                    dangling.push((i, if at_start == 0 { start } else { end }));
                }
            }
        }

        // Copper clearance; overlapping copper of different nets is a
        // clearance of 0 mm, as KiCad reports it.
        for (&(i, j), &(d, at)) in &pairs {
            let (a, b) = (&self.items[i], &self.items[j]);
            let required = rules.clearance(&a.rule, &b.rule) as f64;
            if d < required - EPSILON {
                let items = vec![self.describe(a), self.describe(b)];
                let uuids = self.uuids(&[a, b]);
                let text = if d <= 0.0 {
                    format!("clearance 0 mm (overlap) is below the required {} mm", mm(required))
                } else {
                    format!("clearance {} mm is below the required {} mm", mm(d), mm(required))
                };
                self.report_on("clearance", text, at, items, uuids);
            }
        }
        // A hole against a via or pad once, not once per layer; against a
        // pour once per zone and layer.
        let mut holes_seen: IndexMap<HoleKey, (usize, f64, P)> = IndexMap::new();
        for (&(h, i), &(d, at)) in &hole_pairs {
            let item = &self.items[i];
            let zone = match item.owner {
                Owner::Fill(k) => Some((board.fills[k].zone.unwrap_or(usize::MAX), item.layer)),
                Owner::Track(_) | Owner::Drawing(_) | Owner::Text(_) => Some((usize::MAX, i)),
                Owner::Pad(_) | Owner::Via(_) => None,
            };
            let owner = if matches!(item.owner, Owner::Fill(_)) { Owner::Fill(0) } else { item.owner };
            holes_seen
                .entry((h, owner, zone))
                .and_modify(|best| {
                    if d < best.1 {
                        *best = (i, d, at)
                    }
                })
                .or_insert((i, d, at));
        }
        for (&(h, _, _), &(i, d, at)) in &holes_seen {
            let (hole, item) = (&self.holes[h], &self.items[i]);
            let required = rules.hole_clearance(&hole.rule, &item.rule) as f64;
            if d < required - EPSILON {
                let items = vec![self.describe_hole(hole), self.describe(item)];
                let uuids: Vec<String> = self.hole_uuid(hole).into_iter().chain(self.uuid(item)).collect();
                let text = format!("hole clearance {} mm is below the required {} mm", mm(d.max(0.0)), mm(required));
                self.report_on("hole_clearance", text, at, items, uuids);
            }
        }
        for (&i, &(d, at, _)) in &edge_gaps {
            let item = &self.items[i];
            let required = rules.edge_clearance(&item.rule) as f64;
            if d < required - EPSILON {
                let items = vec![self.describe(item)];
                let uuids = self.uuids(&[item]);
                let text = if d == -1.0 && !poly::shapes_contain(&outline, at) {
                    "copper outside the board outline".to_owned()
                } else {
                    format!("board edge clearance {} mm is below the required {} mm", mm(d.max(0.0)), mm(required))
                };
                self.report_on("copper_edge_clearance", text, at, items, uuids);
            }
        }
        for (&(z, i), &at) in &keepout_hits {
            let items = vec![self.describe(&self.items[i])];
            let uuids: Vec<String> =
                self.uuid(&self.items[i]).into_iter().chain(Some(board.zones[z].uuid.clone())).collect();
            let name = &board.zones[z].name;
            self.report_on("items_not_allowed", format!("item inside keepout area {name:?}"), at, items, uuids);
        }
        for &(i, at) in &dangling {
            let items = vec![self.describe(&self.items[i])];
            let uuids = self.uuids(&[&self.items[i]]);
            self.report_on("track_dangling", "track end is not connected".to_owned(), at, items, uuids);
        }
        for (track, via, at) in &off_centre {
            let items = vec![self.describe(&self.items[*track]), self.describe(&self.items[*via])];
            let uuids = self.uuids(&[&self.items[*track], &self.items[*via]]);
            let text = "track ends inside a via, off its centre".to_owned();
            self.report_on("track_not_centered_on_via", text, *at, items, uuids);
        }
        for (zone, pad, at, text) in &starved {
            let mut uuids: Vec<String> = Some(board.zones[*zone].uuid.clone()).into_iter().collect();
            uuids.extend(self.pads[*pad].1.uuid.clone());
            let (footprint, found) = self.pads[*pad];
            let items = vec![
                format!("zone {:?} {}", board.zones[*zone].name, net_text(board.zones[*zone].net.as_deref())),
                format!("pad {}.{} {}", footprint.reference, found.number, net_text(found.net.as_deref())),
            ];
            self.report_on("starved_thermal", text.clone(), *at, items, uuids);
        }

        // Connectivity across layers: vias and plated pads join their layers.
        let mut by_owner: IndexMap<Owner, Vec<usize>> = IndexMap::new();
        for (index, item) in self.items.iter().enumerate() {
            by_owner.entry(item.owner).or_default().push(index);
        }
        for (owner, members) in &by_owner {
            let plated = match *owner {
                Owner::Via(_) => true,
                Owner::Pad(k) => self.pads[k].1.kind == PadKind::ThroughHole,
                _ => false,
            };
            if plated {
                for pair in members.windows(2) {
                    sets.join(pair[0], pair[1]);
                }
            }
        }
        let mut first_of_group: IndexMap<(usize, usize, Option<&str>), usize> = IndexMap::new();
        for (owner, members) in &by_owner {
            let Owner::Pad(k) = *owner else { continue };
            let (footprint, pad) = self.pads[k];
            let index = board.footprints.iter().position(|candidate| std::ptr::eq(candidate, footprint));
            let Some(group) = footprint.jumper_groups.iter().position(|group| group.contains(&pad.number)) else {
                continue;
            };
            let key = (index.unwrap_or(usize::MAX), group, pad.net.as_deref());
            match first_of_group.get(&key) {
                Some(&first) => sets.join(first, members[0]),
                None => {
                    first_of_group.insert(key, members[0]);
                }
            }
        }
        for (k, via) in board.vias.iter().enumerate() {
            let layers =
                by_owner.get(&Owner::Via(k)).map_or(0, |members| members.iter().filter(|&&i| touched[i]).count());
            if layers < 2 {
                let text = if layers == 0 { "via is not connected" } else { "via is connected on only one layer" };
                let items = vec![format!("via {} at {}", net_text(via.net.as_deref()), at_text(point(via.center)))];
                let uuids = via.uuid.clone().into_iter().collect();
                self.report_on("via_dangling", text.to_owned(), point(via.center), items, uuids);
            }
        }
        // Isolated copper: a fill outline whose connected copper reaches no
        // pad (KiCad's orphaned cluster).
        let mut with_pad: IndexSet<usize> = IndexSet::new();
        for (index, item) in self.items.iter().enumerate() {
            if matches!(item.owner, Owner::Pad(_)) && item.net.is_some() {
                with_pad.insert(sets.find(index));
            }
        }
        for index in 0..self.items.len() {
            let item = &self.items[index];
            if !matches!(item.owner, Owner::Fill(_)) || with_pad.contains(&sets.find(index)) {
                continue;
            }
            let items = vec![self.describe(item)];
            let uuids = self.uuids(&[item]);
            let at = item.paths[0][0];
            self.report_on("isolated_copper", "isolated copper fill".to_owned(), at, items, uuids);
        }
        // Unconnected: a net whose pads, tracks and vias form several groups.
        let mut groups: IndexMap<&str, IndexMap<usize, Vec<usize>>> = IndexMap::new();
        for (index, item) in self.items.iter().enumerate() {
            let Some(net) = item.net else { continue };
            if matches!(item.owner, Owner::Fill(_) | Owner::Drawing(_) | Owner::Text(_)) {
                continue;
            }
            let root = sets.find(index);
            groups.entry(net).or_default().entry(root).or_default().push(index);
        }
        let mut unconnected = Vec::new();
        for (net, clusters) in &groups {
            if clusters.len() < 2 {
                continue;
            }
            let mut clusters: Vec<&Vec<usize>> = clusters.values().collect();
            clusters.sort_by_key(|members| std::cmp::Reverse(members.len()));
            let name = |members: &Vec<usize>| {
                let mut names: IndexSet<String> = IndexSet::new();
                for &i in members {
                    let item = &self.items[i];
                    names.insert(match item.owner {
                        Owner::Pad(k) => format!("{}.{}", self.pads[k].0.reference, self.pads[k].1.number),
                        Owner::Via(k) => format!("via at {}", at_text(point(board.vias[k].center))),
                        _ => format!("track on {}", board.copper_layers[item.layer]),
                    });
                }
                let names: Vec<String> = names.into_iter().collect();
                if names.len() > 4 {
                    format!("{}, ... ({} items)", names[..4].join(", "), names.len())
                } else {
                    names.join(", ")
                }
            };
            // KiCad's ratsnest: the minimum spanning tree over the groups,
            // each link between the closest anchors (pad and via centres,
            // track ends) of the two groups it joins.
            let anchors = |members: &Vec<usize>| -> Vec<(P, usize)> {
                let mut found = Vec::new();
                for &i in members {
                    let item = &self.items[i];
                    match item.owner {
                        Owner::Track(_) => {
                            let path = &item.paths[0];
                            found.push((path[0], i));
                            found.push((path[path.len() - 1], i));
                        }
                        _ => found.push((item.paths[0][0], i)),
                    }
                }
                found
            };
            let anchor_lists: Vec<Vec<(P, usize)>> = clusters.iter().map(|members| anchors(members)).collect();
            let mut links: Vec<(f64, usize, usize, usize, usize)> = Vec::new();
            for a in 0..clusters.len() {
                for b in a + 1..clusters.len() {
                    let mut best: Option<(f64, usize, usize)> = None;
                    for &(p, i) in &anchor_lists[a] {
                        for &(q, j) in &anchor_lists[b] {
                            let d = (p.0 - q.0).hypot(p.1 - q.1);
                            if best.is_none_or(|(e, _, _)| d < e) {
                                best = Some((d, i, j));
                            }
                        }
                    }
                    if let Some((d, i, j)) = best {
                        links.push((d, a, b, i, j));
                    }
                }
            }
            links.sort_by(|x, y| x.0.total_cmp(&y.0).then((x.1, x.2).cmp(&(y.1, y.2))));
            let mut joined = Sets((0..clusters.len()).collect());
            for (_, a, b, i, j) in links {
                if joined.find(a) == joined.find(b) {
                    continue;
                }
                joined.join(a, b);
                unconnected.push(Violation {
                    kind: "unconnected_items",
                    description: format!(
                        "net [{net}]: {} is not connected to {}",
                        name(clusters[a]),
                        name(clusters[b])
                    ),
                    at: self.items[i].paths[0][0],
                    items: vec![self.describe(&self.items[i]), self.describe(&self.items[j])],
                    uuids: self.uuids(&[&self.items[i], &self.items[j]]),
                });
            }
        }
        if self.rules.ignores("unconnected_items") {
            unconnected.clear();
        }
        Ok(unconnected)
    }
}

/// Check `board` (with its fills) against `rules`.
pub fn check(board: &Board, rules: &Rules) -> Result<Report> {
    let refused: Vec<String> = rules
        .other_constraints
        .iter()
        .filter(|(_, kind)| REFUSED.contains(&kind.as_str()))
        .map(|(name, kind)| format!("{name:?} ({kind})"))
        .collect();
    if !refused.is_empty() {
        return Err(format!("custom rules with constraints the native DRC does not model: {}", refused.join(", ")));
    }
    let mut checker = Checker {
        board,
        rules,
        pads: board.pads().collect(),
        items: Vec::new(),
        holes: Vec::new(),
        violations: Vec::new(),
    };
    checker.collect()?;
    checker.sizes();
    checker.holes_to_holes();
    let unconnected = checker.run()?;
    let mut not_checked: Vec<String> = NOT_CHECKED.iter().map(|text| (*text).to_owned()).collect();
    let mut others: IndexSet<String> = IndexSet::new();
    for (name, kind) in &rules.other_constraints {
        others.insert(format!("custom rule {name:?}: {kind}"));
    }
    not_checked.extend(others);
    let edge_segments = board.drawings.iter().filter(|drawing| drawing.layer == Layer::EdgeCuts).count();
    Ok(Report {
        violations: checker.violations,
        unconnected,
        not_checked,
        copper_items: checker.items.len(),
        holes: checker.holes.len(),
        edge_segments,
    })
}

#[cfg(test)]
mod tests;

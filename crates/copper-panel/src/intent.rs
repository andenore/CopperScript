//! What a board's source says about its edges, for the panel to respect:
//! where tabs must not go (an antenna region opened to the perimeter, a body
//! overhang), the envelope a connector body occupies beyond the edge, and the
//! radiator ends that are open by design (native DRC reports them as
//! dangling; route-board's verification waives exactly those). All in the
//! board's own frame; [`check`] places them on the panel.

use copper_cam::poly;
use copper_lang::panel::{PanelSpec, Side};
use indexmap::IndexMap;

use crate::compose::{Point, Pose, mm};
use crate::frame::BuiltTab;

/// A stretch of a side where no tab may go.
#[derive(Clone, Debug, PartialEq)]
pub struct KeepClear {
    pub side: Side,
    pub start: i64,
    pub end: i64,
    pub reason: String,
}

/// A component body standing `distance` beyond a side over `[start, end]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Overhang {
    pub reference: String,
    pub side: Side,
    pub start: i64,
    pub end: i64,
    pub distance: i64,
}

/// A synthesized radiator's centre line as the source chose it.
#[derive(Clone, Debug, PartialEq)]
pub struct Radiator {
    pub name: String,
    pub net: String,
    pub layer: String,
    pub path: Vec<Point>,
}

/// A track end that is open by design (a synthesized radiator's tip).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenEnd {
    pub at: Point,
    pub net: String,
    pub layer: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardIntent {
    /// The source's outline rings; empty skips the outline check.
    pub outline: Vec<Vec<Point>>,
    pub keep_clear: Vec<KeepClear>,
    pub overhangs: Vec<Overhang>,
    pub open_ends: Vec<OpenEnd>,
    /// The source's footprint references; empty skips the reference check.
    pub references: Vec<String>,
    /// The source's synthesized radiators, which the routed board must carry.
    pub radiators: Vec<Radiator>,
}

/// How far beyond an antenna region's perimeter opening tabs stay away.
pub const ANTENNA_TAB_MARGIN: i64 = 1_000_000;

/// The outline's bounding box and the side a segment lies on, with the
/// segment's interval along that side (measured as [`crate::frame`] does).
pub fn side_of(bounds: [i64; 4], a: Point, b: Point) -> Option<(Side, i64, i64)> {
    let [x0, y0, x1, y1] = bounds;
    let span = |p: i64, q: i64, origin: i64| ((p.min(q)) - origin, (p.max(q)) - origin);
    if a.1 == b.1 && a.1 == y0 {
        let (s, e) = span(a.0, b.0, x0);
        Some((Side::Top, s, e))
    } else if a.1 == b.1 && a.1 == y1 {
        let (s, e) = span(a.0, b.0, x0);
        Some((Side::Bottom, s, e))
    } else if a.0 == b.0 && a.0 == x0 {
        let (s, e) = span(a.1, b.1, y0);
        Some((Side::Left, s, e))
    } else if a.0 == b.0 && a.0 == x1 {
        let (s, e) = span(a.1, b.1, y0);
        Some((Side::Right, s, e))
    } else {
        None
    }
}

fn normalized(rings: &[Vec<Point>]) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = rings
        .iter()
        .map(|ring| {
            let mut points = ring.clone();
            points.sort_unstable();
            points.dedup();
            points
        })
        .collect();
    out.sort();
    out
}

fn bounds(rings: &[Vec<Point>]) -> [i64; 4] {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for (x, y) in rings.iter().flatten() {
        b = [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)];
    }
    b
}

fn rect_shapes(corners: [Point; 4]) -> poly::Shapes {
    poly::polygon(&corners.iter().map(|(x, y)| (*x as f64, *y as f64)).collect::<Vec<_>>())
}

/// A tab touching `board`'s `side` over `[lo, hi]` must keep off its
/// keep-clear stretches and body overhangs.
fn contact_clear(
    intents: &IndexMap<String, BoardIntent>,
    tab: &str,
    board: &str,
    side: Side,
    lo: i64,
    hi: i64,
) -> Result<(), String> {
    let Some(intent) = intents.get(board) else { return Ok(()) };
    for keep in intent.keep_clear.iter().filter(|k| k.side == side) {
        if lo < keep.end && hi > keep.start {
            return Err(format!(
                "tab {tab} on board {board}'s {} side ({}-{} mm) overlaps {} ({}-{} mm)",
                side.as_str(),
                mm(lo),
                mm(hi),
                keep.reason,
                mm(keep.start),
                mm(keep.end)
            ));
        }
    }
    for overhang in intent.overhangs.iter().filter(|o| o.side == side) {
        if lo < overhang.end && hi > overhang.start {
            return Err(format!(
                "tab {tab} on board {board}'s {} side overlaps {}'s body overhang ({}-{} mm)",
                side.as_str(),
                overhang.reference,
                mm(overhang.start),
                mm(overhang.end)
            ));
        }
    }
    Ok(())
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let (px, py, ax, ay, bx, by) = (p.0 as f64, p.1 as f64, a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / length).clamp(0.0, 1.0) };
    (px - ax - t * dx).hypot(py - ay - t * dy)
}

/// The routed board must come from the source the intent was read from: the
/// outline alone does not tell two boards apart, so the footprint references
/// must agree and every synthesized radiator must lie on the board's copper
/// (its segments' midpoints and its open tip on a track of the feed net and
/// layer). Without this a source with the same outline but another antenna
/// would move the keep-clear stretches off the real radiator.
pub fn verify_source(board: &str, intent: &BoardIntent, pcb: &copper_cam::board::Board) -> Result<(), String> {
    if !intent.references.is_empty() {
        let mut routed: Vec<&str> =
            pcb.footprints.iter().map(|f| f.reference.as_str()).filter(|r| !r.is_empty()).collect();
        routed.sort_unstable();
        routed.dedup();
        let mut source: Vec<&str> = intent.references.iter().map(String::as_str).collect();
        source.sort_unstable();
        source.dedup();
        if routed != source {
            let missing: Vec<&str> = source.iter().copied().filter(|r| !routed.contains(r)).collect();
            let extra: Vec<&str> = routed.iter().copied().filter(|r| !source.contains(r)).collect();
            return Err(format!(
                "board {board}: its routed pcb's footprints differ from its source's (missing {}; not in the source {}); \
                 route the source again or fix the panel's pcb path",
                if missing.is_empty() { "none".to_owned() } else { missing.join(", ") },
                if extra.is_empty() { "none".to_owned() } else { extra.join(", ") }
            ));
        }
    }
    for radiator in &intent.radiators {
        let layer = pcb.copper_layers.iter().position(|l| *l == radiator.layer);
        let tracks: Vec<(Point, Point, i64)> = pcb
            .tracks
            .iter()
            .filter(|t| Some(t.layer) == layer && t.net.as_deref() == Some(radiator.net.as_str()))
            .flat_map(|t| match t.path {
                copper_cam::board::TrackPath::Line { start, end } => {
                    vec![((start.x, start.y), (end.x, end.y), t.width)]
                }
                copper_cam::board::TrackPath::Arc { start, mid, end } => {
                    vec![((start.x, start.y), (mid.x, mid.y), t.width), ((mid.x, mid.y), (end.x, end.y), t.width)]
                }
            })
            .collect();
        let mut samples: Vec<Point> =
            radiator.path.windows(2).map(|w| ((w[0].0 + w[1].0) / 2, (w[0].1 + w[1].1) / 2)).collect();
        samples.extend(radiator.path.last().copied());
        for p in samples {
            if !tracks.iter().any(|(a, b, width)| distance_to_segment(p, *a, *b) <= (*width / 2 + 1_000) as f64) {
                return Err(format!(
                    "board {board}: antenna {}'s radiator from the source is not on the routed pcb's {} copper of \
                     net {} near ({}, {}) mm; the pcb was routed from another source",
                    radiator.name,
                    radiator.layer,
                    radiator.net,
                    mm(p.0),
                    mm(p.1)
                ));
            }
        }
    }
    Ok(())
}

/// A side of the outline's bounding box in millimetres: `v` is the distance
/// beyond the side (outward), `u` the position along it from its start.
#[derive(Clone, Copy)]
struct SideFrame {
    side: Side,
    bounds: [f64; 4],
}

impl SideFrame {
    fn uv(&self, p: (f64, f64)) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.bounds;
        match self.side {
            Side::Top => (p.0 - x0, y0 - p.1),
            Side::Bottom => (p.0 - x0, p.1 - y1),
            Side::Left => (p.1 - y0, x0 - p.0),
            Side::Right => (p.1 - y0, p.0 - x1),
        }
    }

    fn length(&self) -> f64 {
        let [x0, y0, x1, y1] = self.bounds;
        if matches!(self.side, Side::Top | Side::Bottom) { x1 - x0 } else { y1 - y0 }
    }

    /// The side's line as (is it `y = k`, k).
    fn line(&self) -> (bool, f64) {
        let [x0, y0, x1, y1] = self.bounds;
        match self.side {
            Side::Top => (true, y0),
            Side::Bottom => (true, y1),
            Side::Left => (false, x0),
            Side::Right => (false, x1),
        }
    }
}

/// A courtyard curve on the board, in millimetres.
#[derive(Clone, Copy, Debug)]
enum Curve {
    Segment((f64, f64), (f64, f64)),
    /// From the first point through the second to the third.
    Arc((f64, f64), (f64, f64), (f64, f64)),
    Circle((f64, f64), f64),
}

/// A circle through three points and the arc from `s` through `m` to `e`
/// as (centre, radius, start angle, signed sweep); `None` when straight.
fn arc_geometry(s: (f64, f64), m: (f64, f64), e: (f64, f64)) -> Option<((f64, f64), f64, f64, f64)> {
    let d = 2.0 * (s.0 * (m.1 - e.1) + m.0 * (e.1 - s.1) + e.0 * (s.1 - m.1));
    if d.abs() < 1e-12 {
        return None;
    }
    let sq = |p: (f64, f64)| p.0 * p.0 + p.1 * p.1;
    let cx = (sq(s) * (m.1 - e.1) + sq(m) * (e.1 - s.1) + sq(e) * (s.1 - m.1)) / d;
    let cy = (sq(s) * (e.0 - m.0) + sq(m) * (s.0 - e.0) + sq(e) * (m.0 - s.0)) / d;
    let tau = std::f64::consts::TAU;
    let angle = |p: (f64, f64)| (p.1 - cy).atan2(p.0 - cx).rem_euclid(tau);
    let (a0, am, a2) = (angle(s), angle(m), angle(e));
    let ccw = |from: f64, to: f64| (to - from).rem_euclid(tau);
    let sweep = if ccw(a0, am) <= ccw(a0, a2) { ccw(a0, a2) } else { ccw(a0, a2) - tau };
    Some(((cx, cy), (s.0 - cx).hypot(s.1 - cy), a0, sweep))
}

/// Chords per arc so none strays more than a quarter micron from it (the
/// keep-out is matched within a micron of the edge); `r` in nanometres.
fn arc_segments(r: f64, sweep: f64) -> usize {
    const SAGITTA: f64 = 250.0;
    let step = if r <= SAGITTA { std::f64::consts::FRAC_PI_2 } else { 2.0 * (1.0 - SAGITTA / r).acos() };
    ((sweep.abs() / step).ceil() as usize).clamp(8, 1_000_000)
}

/// The parts of `curve` beyond `frame`'s side, each as the (u, v) points
/// that bound it: its ends, where it crosses the side's line, and its axis
/// extremes. An arc may leave the side, return and leave again: each
/// stretch beyond is its own part. Empty when no part lies beyond.
fn beyond(curve: Curve, frame: SideFrame) -> Vec<Vec<(f64, f64)>> {
    let mut parts: Vec<Vec<(f64, f64)>> = Vec::new();
    match curve {
        Curve::Segment(a, b) => {
            let (p, q) = (frame.uv(a), frame.uv(b));
            let mut found: Vec<(f64, f64)> = Vec::new();
            if p.1 > 0.0 {
                found.push(p);
            }
            if q.1 > 0.0 {
                found.push(q);
            }
            if (p.1 > 0.0) != (q.1 > 0.0) && p.1 != q.1 {
                let t = p.1 / (p.1 - q.1);
                found.push((p.0 + t * (q.0 - p.0), 0.0));
            }
            if !found.is_empty() {
                parts.push(found);
            }
        }
        Curve::Arc(..) | Curve::Circle(..) => {
            let tau = std::f64::consts::TAU;
            let (c, r, a0, sweep) = match curve {
                Curve::Arc(s, m, e) => match arc_geometry(s, m, e) {
                    Some(geometry) => geometry,
                    None => return beyond(Curve::Segment(s, e), frame),
                },
                Curve::Circle(c, r) => (c, r, 0.0, tau),
                Curve::Segment(..) => unreachable!(),
            };
            // Arc length parameter s in [0, |sweep|] from the start.
            let (total, direction) = (sweep.abs(), sweep.signum());
            let at = |s: f64| {
                let t = a0 + direction * s;
                (c.0 + r * t.cos(), c.1 + r * t.sin())
            };
            let param = |t: f64| ((t - a0) * direction).rem_euclid(tau);
            // Cut where the circle meets the side's line.
            let mut cuts = vec![0.0, total];
            let (horizontal, k) = frame.line();
            let offset = if horizontal { k - c.1 } else { k - c.0 };
            if offset.abs() < r {
                let half = (r * r - offset * offset).sqrt();
                for sign in [-1.0, 1.0] {
                    let p = if horizontal { (c.0 + sign * half, k) } else { (k, c.1 + sign * half) };
                    let s = param((p.1 - c.1).atan2(p.0 - c.0));
                    if s > 1e-12 && s < total - 1e-12 {
                        cuts.push(s);
                    }
                }
            }
            cuts.sort_by(f64::total_cmp);
            for w in cuts.windows(2) {
                let (sa, sb) = (w[0], w[1]);
                if sb - sa < 1e-12 || frame.uv(at((sa + sb) / 2.0)).1 <= 0.0 {
                    continue;
                }
                let mut points = vec![at(sa), at(sb)];
                for quarter in 0..4 {
                    let s = param(quarter as f64 * std::f64::consts::FRAC_PI_2);
                    if s >= sa && s <= sb {
                        points.push(at(s));
                    }
                }
                parts.push(points.into_iter().map(|p| frame.uv(p)).map(|(u, v)| (u, v.max(0.0))).collect());
            }
        }
    }
    parts
}

/// Keep-clear stretches the routed board carries itself: every rule area
/// that forbids copper pours (an antenna region, a connector launch) where
/// it covers a side of the outline, ending on the edge or crossing it, plus
/// the antenna margin. They hold whatever the source says, so a wrong
/// source cannot open a radiator edge to tabs.
pub fn routed_keep_clear(pcb: &copper_cam::board::Board, outline: &[Vec<Point>]) -> Vec<KeepClear> {
    let b = bounds(outline);
    let mut found: Vec<KeepClear> = Vec::new();
    for zone in pcb
        .zones
        .iter()
        .filter(|z| z.keepout == Some(true) && !z.in_footprint && z.not_allowed.iter().any(|kind| kind == "copperpour"))
    {
        // Contours inside others are holes (even-odd), as native CAM reads them.
        let mut contours: Vec<poly::Contour> = Vec::new();
        for contour in &zone.outline {
            let mut ring: Vec<(f64, f64)> = vec![(contour.start.x as f64, contour.start.y as f64)];
            for edge in &contour.edges {
                let end = edge.end();
                let end = (end.x as f64, end.y as f64);
                if let copper_cam::geometry::Edge::Arc { mid, .. } = edge
                    && let Some((c, r, a0, sweep)) =
                        arc_geometry(*ring.last().expect("a point"), (mid.x as f64, mid.y as f64), end)
                {
                    let n = arc_segments(r, sweep);
                    ring.extend(
                        (1..n)
                            .map(|k| a0 + sweep * k as f64 / n as f64)
                            .map(|t| (c.0 + r * t.cos(), c.1 + r * t.sin())),
                    );
                }
                ring.push(end);
            }
            contours.extend(poly::polygon(&ring).into_iter().flatten());
        }
        let shapes = poly::even_odd(&contours);
        // The side, a micron either way: what of the keep-out touches it.
        let strips = [
            (Side::Top, [b[0], b[1] - 1_000, b[2], b[1] + 1_000]),
            (Side::Bottom, [b[0], b[3] - 1_000, b[2], b[3] + 1_000]),
            (Side::Left, [b[0] - 1_000, b[1], b[0] + 1_000, b[3]]),
            (Side::Right, [b[2] - 1_000, b[1], b[2] + 1_000, b[3]]),
        ];
        for (side, r) in strips {
            let strip = rect_shapes([(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])]);
            for piece in poly::intersection(&shapes, &strip) {
                let along: Vec<i64> = piece
                    .iter()
                    .flatten()
                    .map(|p| if matches!(side, Side::Top | Side::Bottom) { p.x - b[0] } else { p.y - b[1] })
                    .collect();
                let (Some(start), Some(end)) = (along.iter().min().copied(), along.iter().max().copied()) else {
                    continue;
                };
                if end > start {
                    let keep = KeepClear {
                        side,
                        start: start - ANTENNA_TAB_MARGIN,
                        end: end + ANTENNA_TAB_MARGIN,
                        reason: format!("the routed board's keep-out {} opened to the perimeter", zone.name),
                    };
                    if !found.contains(&keep) {
                        found.push(keep);
                    }
                }
            }
        }
    }
    found
}

/// A point of a footprint's own frame on the board: KiCad turns footprints
/// counter-clockwise on screen (Y down) by their `at` angle.
fn footprint_point(at: (f64, f64, f64), x: f64, y: f64) -> (f64, f64) {
    let (sin, cos) = at.2.to_radians().sin_cos();
    (at.0 + x * cos + y * sin, at.1 - x * sin + y * cos)
}

/// Body overhangs the routed board shows itself: every footprint whose
/// courtyard reaches beyond a side of the outline. With the source's own
/// overhangs they keep tabs off the connector and its envelope off rails
/// and boards, so a source that lost its overhang declaration cannot open
/// the launch to tabs.
pub fn routed_overhangs(tree: &crate::sexpr::Node, outline: &[Vec<Point>]) -> Vec<Overhang> {
    let b = bounds(outline);
    let number = |n: &crate::sexpr::Node, k: usize| n.value(k).and_then(|v| v.parse::<f64>().ok());
    let mut found: Vec<Overhang> = Vec::new();
    for footprint in tree.children("footprint") {
        let Some(at) = footprint.child("at") else { continue };
        let at = (number(at, 1).unwrap_or(0.0), number(at, 2).unwrap_or(0.0), number(at, 3).unwrap_or(0.0));
        let reference = footprint
            .children("property")
            .find(|p| p.string_value(1).as_deref() == Some("Reference"))
            .and_then(|p| p.string_value(2))
            .unwrap_or_default();
        let mut curves: Vec<Curve> = Vec::new();
        for item in footprint.items() {
            let layer = item.child("layer").and_then(|l| l.string_value(1)).unwrap_or_default();
            if !layer.ends_with(".CrtYd") && !layer.ends_with(".Courtyard") {
                continue;
            }
            let xy = |tag: &str| item.child(tag).and_then(|n| Some((number(n, 1)?, number(n, 2)?)));
            let on_board = |p: (f64, f64)| footprint_point(at, p.0, p.1);
            // The courtyard's curves on the board (a turned footprint turns them).
            match item.tag() {
                Some("fp_rect") => {
                    if let (Some((sx, sy)), Some((ex, ey))) = (xy("start"), xy("end")) {
                        let c = [(sx, sy), (ex, sy), (ex, ey), (sx, ey)].map(on_board);
                        curves.extend((0..4).map(|k| Curve::Segment(c[k], c[(k + 1) % 4])));
                    }
                }
                Some("fp_circle") => {
                    if let (Some(c), Some(e)) = (xy("center"), xy("end")) {
                        curves.push(Curve::Circle(on_board(c), (e.0 - c.0).hypot(e.1 - c.1)));
                    }
                }
                Some("fp_arc") => {
                    if let (Some(s), Some(m), Some(e)) = (xy("start"), xy("mid"), xy("end")) {
                        curves.push(Curve::Arc(on_board(s), on_board(m), on_board(e)));
                    }
                }
                Some("fp_poly") => {
                    if let Some(pts) = item.child("pts") {
                        let mut ring: Vec<Curve> = Vec::new();
                        let mut last: Option<(f64, f64)> = None;
                        let mut first: Option<(f64, f64)> = None;
                        for node in pts.items() {
                            let at = |tag: &str| node.child(tag).and_then(|n| Some((number(n, 1)?, number(n, 2)?)));
                            let (start, end, curve) = match node.tag() {
                                Some("xy") => match number(node, 1).zip(number(node, 2)).map(on_board) {
                                    Some(p) => (p, p, None),
                                    None => continue,
                                },
                                Some("arc") => match (at("start"), at("mid"), at("end")) {
                                    (Some(s), Some(m), Some(e)) => {
                                        let (s, m, e) = (on_board(s), on_board(m), on_board(e));
                                        (s, e, Some(Curve::Arc(s, m, e)))
                                    }
                                    _ => continue,
                                },
                                _ => continue,
                            };
                            if let Some(previous) = last {
                                ring.push(Curve::Segment(previous, start));
                            }
                            ring.extend(curve);
                            first.get_or_insert(start);
                            last = Some(end);
                        }
                        if let (Some(first), Some(last)) = (first, last) {
                            ring.push(Curve::Segment(last, first));
                        }
                        curves.extend(ring);
                    }
                }
                _ => {
                    if let (Some(s), Some(e)) = (xy("start"), xy("end")) {
                        curves.push(Curve::Segment(on_board(s), on_board(e)));
                    }
                }
            }
        }
        // Per side: each stretch where the courtyard reaches beyond it, and
        // how far. Curves that cross the side in one place chain through
        // shared ends into one stretch; separate crossings stay separate.
        let mm_bounds = [b[0], b[1], b[2], b[3]].map(|v| v as f64 / 1e6);
        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            let frame = SideFrame { side, bounds: mm_bounds };
            let nm = |v: f64| (v * 1e6).round() as i64;
            let mut stretches: Vec<Overhang> = Vec::new();
            for points in curves.iter().flat_map(|curve| beyond(*curve, frame)) {
                let depth = points.iter().map(|p| p.1).fold(0.0, f64::max);
                let start = nm(points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min).max(0.0));
                let end = nm(points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max).min(frame.length()));
                if end >= start {
                    stretches.push(Overhang { reference: reference.clone(), side, start, end, distance: nm(depth) });
                }
            }
            let mut merged: Vec<Overhang> = Vec::new();
            merge_overhangs(&mut merged, stretches);
            found.extend(merged.into_iter().filter(|o| o.distance > 1_000 && o.end > o.start));
        }
    }
    found
}

/// Merge overhangs: those of one component and side that overlap or touch
/// become one covering both; separate stretches stay separate.
pub fn merge_overhangs(into: &mut Vec<Overhang>, more: Vec<Overhang>) {
    for o in more {
        let mut merged = o;
        // Absorb every existing stretch it reaches, then any those reach.
        while let Some(k) = into.iter().position(|e| {
            e.reference == merged.reference && e.side == merged.side && e.start <= merged.end && merged.start <= e.end
        }) {
            let e = into.remove(k);
            merged.start = merged.start.min(e.start);
            merged.end = merged.end.max(e.end);
            merged.distance = merged.distance.max(e.distance);
        }
        into.push(merged);
    }
    into.sort_by(|a, b| (&a.reference, a.side as u8, a.start).cmp(&(&b.reference, b.side as u8, b.start)));
}

fn segment_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    [
        distance_to_segment(a, c, d),
        distance_to_segment(b, c, d),
        distance_to_segment(c, a, b),
        distance_to_segment(d, a, b),
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
}

/// The smallest distance between two sets of outline rings (0 when they meet).
pub fn ring_distance(a: &[Vec<Point>], b: &[Vec<Point>]) -> i64 {
    let shapes = |rings: &[Vec<Point>]| -> poly::Shapes {
        rings
            .iter()
            .flat_map(|ring| poly::polygon(&ring.iter().map(|(x, y)| (*x as f64, *y as f64)).collect::<Vec<_>>()))
            .collect()
    };
    if !poly::intersection(&shapes(a), &shapes(b)).is_empty() {
        return 0;
    }
    let mut best = f64::INFINITY;
    for ra in a {
        for rb in b {
            for i in 0..ra.len() {
                for j in 0..rb.len() {
                    let d = segment_distance(ra[i], ra[(i + 1) % ra.len()], rb[j], rb[(j + 1) % rb.len()]);
                    best = best.min(d);
                }
            }
        }
    }
    best.floor() as i64
}

/// Check the panel against every board's intent; returns the panel's open
/// ends (panel coordinates, prefixed nets) for the DRC waiver.
pub fn check(
    spec: &PanelSpec,
    intents: &IndexMap<String, BoardIntent>,
    local: &IndexMap<String, Vec<Vec<Point>>>,
    outlines: &IndexMap<String, Vec<Vec<Point>>>,
    poses: &IndexMap<String, Pose>,
    tabs: &[BuiltTab],
    rails: &[[i64; 4]],
) -> Result<Vec<OpenEnd>, String> {
    for (board, intent) in intents {
        if !intent.outline.is_empty() && normalized(&intent.outline) != normalized(&local[board]) {
            return Err(format!(
                "board {board}: its routed pcb's outline differs from its source's; route the source again or fix the panel's pcb path"
            ));
        }
    }
    for tab in tabs {
        let (lo, hi) = (tab.at - tab.width / 2, tab.at + tab.width / 2);
        contact_clear(intents, &tab.name, &tab.board, tab.side, lo, hi)?;
        // A tab joining two boards lands on the neighbour's side too.
        if let Some(across) = &tab.across {
            let normal = poses[&tab.board].rotate(match tab.side {
                Side::Top => (0, -1),
                Side::Bottom => (0, 1),
                Side::Left => (-1, 0),
                Side::Right => (1, 0),
            });
            let far = |p: Point| (p.0 + normal.0 * tab.length, p.1 + normal.1 * tab.length);
            let neighbour = &poses[across];
            let (a, b) = (neighbour.invert(far(tab.edge.0)), neighbour.invert(far(tab.edge.1)));
            match side_of(bounds(&local[across]), a, b) {
                Some((side, lo, hi)) => contact_clear(intents, &tab.name, across, side, lo, hi)?,
                None => {
                    return Err(format!(
                        "tab {} lands on board {} away from a straight side of its outline",
                        tab.name, across
                    ));
                }
            }
        }
    }
    // Body envelopes beyond the edge must stay clear of rails and other
    // boards, unless the component is fitted after depanelization.
    let mut open_ends = Vec::new();
    for (board, intent) in intents {
        let pose = poses[board];
        let b = bounds(&local[board]);
        for overhang in &intent.overhangs {
            if spec.assemble_after_depanel.iter().any(|e| *e == format!("{board}.{}", overhang.reference)) {
                continue;
            }
            let (start, along, normal) = match overhang.side {
                Side::Top => ((b[0], b[1]), (1, 0), (0, -1)),
                Side::Bottom => ((b[0], b[3]), (1, 0), (0, 1)),
                Side::Left => ((b[0], b[1]), (0, 1), (-1, 0)),
                Side::Right => ((b[2], b[1]), (0, 1), (1, 0)),
            };
            let at = |t: i64, d: i64| {
                pose.apply((start.0 + along.0 * t + normal.0 * d, start.1 + along.1 * t + normal.1 * d))
            };
            let envelope = rect_shapes([
                at(overhang.start, 1_000),
                at(overhang.end, 1_000),
                at(overhang.end, overhang.distance),
                at(overhang.start, overhang.distance),
            ]);
            let mut blockers: Vec<String> = Vec::new();
            for rail in rails {
                let r = rect_shapes([(rail[0], rail[1]), (rail[2], rail[1]), (rail[2], rail[3]), (rail[0], rail[3])]);
                if !poly::intersection(&envelope, &r).is_empty() {
                    blockers.push("a rail".to_owned());
                }
            }
            for (other, rings) in outlines {
                if other == board {
                    continue;
                }
                for ring in rings {
                    let shapes = poly::polygon(&ring.iter().map(|(x, y)| (*x as f64, *y as f64)).collect::<Vec<_>>());
                    if !poly::intersection(&envelope, &shapes).is_empty() {
                        blockers.push(format!("board {other}"));
                    }
                }
            }
            if !blockers.is_empty() {
                blockers.dedup();
                return Err(format!(
                    "board {board}: {}'s body stands {} mm beyond its {} side into {}; turn the board, move the \
                     blocker, or list \"{board}.{}\" in assemble_after_depanel",
                    overhang.reference,
                    mm(overhang.distance),
                    overhang.side.as_str(),
                    blockers.join(" and "),
                    overhang.reference
                ));
            }
        }
        for end in &intent.open_ends {
            open_ends.push(OpenEnd {
                at: pose.apply(end.at),
                net: format!("{board}/{}", end.net),
                layer: end.layer.clone(),
            });
        }
    }
    Ok(open_ends)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_inverts() {
        for turns in 0..4 {
            let pose = Pose { quarter_turns: turns, offset: (7, -3) };
            assert_eq!(pose.invert(pose.apply((5, 11))), (5, 11));
        }
    }

    #[test]
    fn only_the_part_beyond_a_side_counts() {
        let bottom = SideFrame { side: Side::Bottom, bounds: [0.0, 0.0, 10.0, 6.0] };
        // A segment crossing y = 6 at x = 3.6, inside up to x = 10.
        let points = beyond(Curve::Segment((2.0, 6.5), (10.0, 4.0)), bottom).concat();
        let us: Vec<f64> = points.iter().map(|p| p.0).collect();
        assert!(us.iter().any(|u| (u - 2.0).abs() < 1e-9) && us.iter().any(|u| (u - 3.6).abs() < 1e-9), "{points:?}");
        assert!(us.iter().all(|u| *u <= 3.6 + 1e-9), "{points:?}");
        // A circle centred 2 mm inside, 2.004 mm in radius: 4 um beyond over its chord.
        let points = beyond(Curve::Circle((5.0, 4.0), 2.004), bottom).concat();
        let depth = points.iter().map(|p| p.1).fold(0.0, f64::max);
        let width =
            points.iter().map(|p| p.0).fold(f64::MIN, f64::max) - points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        assert!(
            (depth - 0.004).abs() < 1e-9 && (width - 2.0 * (2.004f64 * 2.004 - 4.0).sqrt()).abs() < 1e-9,
            "{points:?}"
        );
        // An arc bulging beyond with both ends inside.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let points = beyond(Curve::Arc((5.0 - h, 5.0 + h), (5.0, 6.0 + 0.5), (5.0 + h, 5.0 + h)), bottom).concat();
        assert!(points.iter().map(|p| p.1).fold(0.0, f64::max) > 0.0, "{points:?}");
        // Nothing beyond, nothing found.
        assert!(beyond(Curve::Segment((1.0, 1.0), (9.0, 5.0)), bottom).is_empty());
        // An arc going the long way round a cap leaves two separate parts.
        let at = |deg: f64| {
            let t = deg.to_radians();
            (5.0 + t.cos(), 5.5 + t.sin())
        };
        let parts = beyond(Curve::Arc(at(100.0), at(270.0), at(80.0)), bottom);
        assert_eq!(parts.len(), 2, "{parts:?}");
        let spans: Vec<(f64, f64)> = parts
            .iter()
            .map(|p| (p.iter().map(|q| q.0).fold(f64::MAX, f64::min), p.iter().map(|q| q.0).fold(f64::MIN, f64::max)))
            .collect();
        assert!(
            spans.iter().all(|(a, b)| *b < 5.0 - 0.17 || *a > 5.0 + 0.17),
            "the gap 80-100 deg stays open: {spans:?}"
        );
    }

    #[test]
    fn segments_map_to_sides_and_intervals() {
        let b = [0, 0, 40, 31];
        assert_eq!(side_of(b, (10, 31), (27, 31)), Some((Side::Bottom, 10, 27)));
        assert_eq!(side_of(b, (40, 6), (40, 0)), Some((Side::Right, 0, 6)));
        assert_eq!(side_of(b, (5, 5), (6, 5)), None);
    }
}

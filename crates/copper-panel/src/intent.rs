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

/// Keep-clear stretches the routed board carries itself: every rule area
/// that forbids copper pours (an antenna region, a connector launch) along
/// the outline's sides, plus the antenna margin. They hold whatever the
/// source says, so a wrong source cannot open a radiator edge to tabs.
pub fn routed_keep_clear(pcb: &copper_cam::board::Board, outline: &[Vec<Point>]) -> Vec<KeepClear> {
    let b = bounds(outline);
    let mut found: Vec<KeepClear> = Vec::new();
    for zone in pcb
        .zones
        .iter()
        .filter(|z| z.keepout == Some(true) && !z.in_footprint && z.not_allowed.iter().any(|kind| kind == "copperpour"))
    {
        for contour in &zone.outline {
            let mut from = (contour.start.x, contour.start.y);
            for edge in &contour.edges {
                let end = edge.end();
                let to = (end.x, end.y);
                if let Some((side, start, end)) = side_of(b, from, to)
                    && end > start
                {
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
                from = to;
            }
        }
    }
    found
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
        if tab.across != "rail" {
            let normal = poses[&tab.board].rotate(match tab.side {
                Side::Top => (0, -1),
                Side::Bottom => (0, 1),
                Side::Left => (-1, 0),
                Side::Right => (1, 0),
            });
            let far = |p: Point| (p.0 + normal.0 * tab.length, p.1 + normal.1 * tab.length);
            let neighbour = &poses[&tab.across];
            let (a, b) = (neighbour.invert(far(tab.edge.0)), neighbour.invert(far(tab.edge.1)));
            match side_of(bounds(&local[&tab.across]), a, b) {
                Some((side, lo, hi)) => contact_clear(intents, &tab.name, &tab.across, side, lo, hi)?,
                None => {
                    return Err(format!(
                        "tab {} lands on board {} away from a straight side of its outline",
                        tab.name, tab.across
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
    fn segments_map_to_sides_and_intervals() {
        let b = [0, 0, 40, 31];
        assert_eq!(side_of(b, (10, 31), (27, 31)), Some((Side::Bottom, 10, 27)));
        assert_eq!(side_of(b, (40, 6), (40, 0)), Some((Side::Right, 0, 6)));
        assert_eq!(side_of(b, (5, 5), (6, 5)), None);
    }
}

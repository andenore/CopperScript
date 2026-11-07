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
        let Some(intent) = intents.get(&tab.board) else { continue };
        let (lo, hi) = (tab.at - tab.width / 2, tab.at + tab.width / 2);
        for keep in intent.keep_clear.iter().filter(|k| k.side == tab.side) {
            if lo < keep.end && hi > keep.start {
                return Err(format!(
                    "tab {} on board {}'s {} side ({}-{} mm) overlaps {} ({}-{} mm)",
                    tab.name,
                    tab.board,
                    tab.side.as_str(),
                    mm(lo),
                    mm(hi),
                    keep.reason,
                    mm(keep.start),
                    mm(keep.end)
                ));
            }
        }
        for overhang in intent.overhangs.iter().filter(|o| o.side == tab.side) {
            if lo < overhang.end && hi > overhang.start {
                return Err(format!(
                    "tab {} on board {}'s {} side overlaps {}'s body overhang ({}-{} mm)",
                    tab.name,
                    tab.board,
                    tab.side.as_str(),
                    overhang.reference,
                    mm(overhang.start),
                    mm(overhang.end)
                ));
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
    fn segments_map_to_sides_and_intervals() {
        let b = [0, 0, 40, 31];
        assert_eq!(side_of(b, (10, 31), (27, 31)), Some((Side::Bottom, 10, 27)));
        assert_eq!(side_of(b, (40, 6), (40, 0)), Some((Side::Right, 0, 6)));
        assert_eq!(side_of(b, (5, 5), (6, 5)), None);
    }
}

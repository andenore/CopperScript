//! Conservative, exact-clearance 45-degree cleanup of ordinary route corners
//! (`pcbir.route_style`).
//!
//! PENDING: owned by the detailed routing streams; ported for the critical
//! stage's surface paths (`allow_movable_conflicts` is not needed there).
//! Only degree-two, perpendicular octilinear corners are changed. An
//! obstacle is a reason to retain a corner, never to emit an unchecked
//! shortcut; so is a necked-down corner whose diagonal would leave its
//! breakout region.

use indexmap::IndexMap;

use copper_pcb::board::{PhysicalBoard, TrackSegment, Via};
use copper_physical::geometry::shapes_clear;
use copper_physical::{CopperLayer, Point, RoundedConvexShape};

use crate::Result;
use crate::routing_clearance::RoutingClearanceIndex;
use crate::surface_path::track_inside_board;

fn sign(value: i64) -> i64 {
    (value > 0) as i64 - (value < 0) as i64
}

/// `chamfer_ordinary_corners(board, tracks, vias, clearance, committed_tracks=...)`.
pub fn chamfer_ordinary_corners(
    board: &PhysicalBoard,
    tracks: &[TrackSegment],
    vias: &[Via],
    clearance: &RoutingClearanceIndex<'_>,
    committed_tracks: Option<&[TrackSegment]>,
) -> Result<Vec<TrackSegment>> {
    let committed = committed_tracks.unwrap_or(&board.tracks);
    let mut active: Vec<TrackSegment> = tracks.to_vec();
    let mut endpoints: IndexMap<(CopperLayer, Point), Vec<usize>> = IndexMap::new();
    for (index, track) in tracks.iter().enumerate() {
        endpoints.entry((track.layer, track.start)).or_default().push(index);
        endpoints.entry((track.layer, track.end)).or_default().push(index);
    }
    let mut corners: Vec<(&(CopperLayer, Point), &Vec<usize>)> = endpoints.iter().collect();
    corners.sort_by(|a, b| (a.0.0.value(), a.0.1.x_nm, a.0.1.y_nm).cmp(&(b.0.0.value(), b.0.1.x_nm, b.0.1.y_nm)));
    for ((layer, corner), indexes) in corners {
        let (layer, corner) = (*layer, *corner);
        let [i, j] = indexes.as_slice() else { continue };
        let (i, j) = (*i, *j);
        let (first, second) = (active[i].clone(), active[j].clone());
        if first.net != second.net || first.width_nm != second.width_nm {
            continue;
        }
        if (corner != first.start && corner != first.end) || (corner != second.start && corner != second.end) {
            continue;
        }
        let a = if first.start == corner { first.end } else { first.start };
        let b = if second.start == corner { second.end } else { second.start };
        let (dx, dy) = (a.x_nm - corner.x_nm, a.y_nm - corner.y_nm);
        let (ex, ey) = (b.x_nm - corner.x_nm, b.y_nm - corner.y_nm);
        if i128::from(dx) * i128::from(ex) + i128::from(dy) * i128::from(ey) != 0
            || dx.abs().max(dy.abs()) == 0
            || ex.abs().max(ey.abs()) == 0
        {
            continue;
        }
        if !((dx == 0 || dy == 0 || dx.abs() == dy.abs()) && (ex == 0 || ey == 0 || ex.abs() == ey.abs())) {
            continue;
        }
        let u = (sign(dx), sign(dy));
        let v = (sign(ex), sign(ey));
        for divisor in [4, 8, 16] {
            let trim = dx.abs().max(dy.abs()).min(ex.abs().max(ey.abs())).div_euclid(divisor);
            if trim < first.width_nm {
                continue;
            }
            let p = Point::new(corner.x_nm + u.0 * trim, corner.y_nm + u.1 * trim);
            let q = Point::new(corner.x_nm + v.0 * trim, corner.y_nm + v.1 * trim);
            let half = first.width_nm.div_euclid(2);
            let removed =
                [RoundedConvexShape::new(vec![p, corner], half)?, RoundedConvexShape::new(vec![corner, q], half)?];
            if removed.iter().any(|shape| !clearance.pad_copper_clear(shape, &[layer], None)) {
                continue;
            }
            let mut touched = false;
            for (k, track) in active
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i && *k != j)
                .chain(committed.iter().enumerate().map(|(k, t)| (usize::MAX - k, t)))
            {
                let _ = k;
                if track.layer == layer && track.net == first.net {
                    let other = RoundedConvexShape::new(vec![track.start, track.end], track.width_nm.div_euclid(2))?;
                    if removed.iter().any(|shape| !shapes_clear(shape, &other, 1)) {
                        touched = true;
                        break;
                    }
                }
            }
            if touched {
                continue;
            }
            let mut via_touched = false;
            for via in board.vias.iter().chain(vias) {
                if via.net == first.net {
                    let other = RoundedConvexShape::new(vec![via.position], via.size_nm.div_euclid(2))?;
                    if removed.iter().any(|shape| !shapes_clear(shape, &other, 1)) {
                        via_touched = true;
                        break;
                    }
                }
            }
            if via_touched {
                continue;
            }
            let breakout = &clearance.breakout;
            if breakout.ordinary.contains(&first.net)
                && first.width_nm < breakout.width_nm(&first.net)
                && breakout.region(&first.net, &[p, q]).is_none()
            {
                continue;
            }
            if !track_inside_board(board, p, q, first.width_nm)? {
                continue;
            }
            if !clearance.can_track(&first.net, p, q, first.width_nm, layer)? {
                continue;
            }
            active[i] = TrackSegment { net: first.net.clone(), start: a, end: p, width_nm: first.width_nm, layer };
            active[j] = TrackSegment { net: second.net.clone(), start: q, end: b, width_nm: second.width_nm, layer };
            active.push(TrackSegment { net: first.net.clone(), start: p, end: q, width_nm: first.width_nm, layer });
            break;
        }
    }
    Ok(active)
}

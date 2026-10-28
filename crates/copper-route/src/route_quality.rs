//! Routing preferences separate from electrical clearance and connectivity
//! (`pcbir.route_quality`).

use copper_pcb::board::TrackSegment;
use copper_physical::Point;

/// `proper_same_net_crossing`: an interior/interior X of one net on one layer.
pub fn proper_same_net_crossing(first: &TrackSegment, second: &TrackSegment) -> bool {
    if first.net != second.net || first.layer != second.layer {
        return false;
    }
    let side = |a: Point, b: Point, p: Point| -> i128 {
        i128::from(b.x_nm - a.x_nm) * i128::from(p.y_nm - a.y_nm)
            - i128::from(b.y_nm - a.y_nm) * i128::from(p.x_nm - a.x_nm)
    };
    let (a, b, c, d) = (first.start, first.end, second.start, second.end);
    side(a, b, c).signum() * side(a, b, d).signum() < 0 && side(c, d, a).signum() * side(c, d, b).signum() < 0
}

/// `escape_paths_cross`.
pub fn escape_paths_cross(first: &[TrackSegment], second: &[TrackSegment]) -> bool {
    first.iter().any(|a| second.iter().any(|b| proper_same_net_crossing(a, b)))
}

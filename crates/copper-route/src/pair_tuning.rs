//! Intra-pair skew tuning of a coupled pair (`pcbir.critical._tune_pair`
//! and its helpers): serpentine bumps on the shorter member next to where
//! the mismatch arises.

use copper_drc::breakout::BreakoutRegions;
use copper_pcb::board::TrackSegment;
use copper_physical::Point;
use copper_physical::pyfloat::{hypot, sum};

use crate::critical_tuning::{bump_chamfers, bump_gain, bump_path};

/// `_MAX_TUNING_BUMPS`.
pub const MAX_TUNING_BUMPS: i64 = 8;

/// `_BREAKOUT_TUNING_MARGIN_NM`: a tuned pair inside breakout regions aims
/// this far inside its `max_skew`, which absorbs the nanometres a recut
/// 45-degree corner may round.
pub const BREAKOUT_TUNING_MARGIN_NM: i64 = 32;

/// What `_tune_pair` changed: the added length and the original indices of
/// the target's segments it replaced by bumps (the other segments are kept
/// as they were).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairTuning {
    pub added_nm: i64,
    pub first_tuned: bool,
    pub replaced: Vec<usize>,
}

fn track_length(tracks: &[TrackSegment]) -> i64 {
    tracks.iter().map(|track| copper_physical::pyfloat::round_int(segment_length(track))).sum()
}

/// `_tune_pair`: compensate excess intra-pair skew next to where the
/// mismatch arises. The shorter member receives at most eight chamfered
/// bumps, all of the least height that brings the skew within `max_skew_nm`
/// less `margin_nm`; segments inside a `breakout` region come last. When
/// they do not fit, the geometry is unchanged. Returns what was added.
pub fn tune_pair(
    first: &mut Vec<TrackSegment>,
    second: &mut Vec<TrackSegment>,
    max_skew_nm: i64,
    amplitude_limit_nm: i64,
    margin_nm: i64,
    breakout: Option<&BreakoutRegions>,
) -> PairTuning {
    let (first_length, second_length) = (track_length(first), track_length(second));
    let skew = (first_length - second_length).abs();
    if skew <= max_skew_nm || amplitude_limit_nm <= 0 {
        return PairTuning::default();
    }
    let excess = skew - max_skew_nm + margin_nm.min(max_skew_nm);
    let first_tuned = first_length < second_length;
    let (target, partner): (&mut Vec<TrackSegment>, &Vec<TrackSegment>) =
        if first_tuned { (first, second) } else { (second, first) };
    let width = target.iter().map(|track| track.width_nm).min().expect("a routed member has tracks");
    let gain = |height: i64| -> i64 {
        let (base, top) = bump_chamfers(height, width, 0);
        bump_gain(height, base, top)
    };
    if gain(amplitude_limit_nm) <= 0 {
        return PairTuning::default();
    }
    let count = -((-excess).div_euclid(gain(amplitude_limit_nm)));
    if count > MAX_TUNING_BUMPS {
        return PairTuning::default();
    }
    // Short of the excess at `low`, enough at `height`.
    let (mut low, mut height) = (0, amplitude_limit_nm);
    while height - low > 1 {
        let middle = (low + height).div_euclid(2);
        if count * gain(middle) < excess {
            low = middle;
        } else {
            height = middle;
        }
    }
    let mut placements: Vec<(usize, bool, i64)> = Vec::new();
    let mut remaining = count;
    let mut sites = tuning_sites(target, partner);
    if let Some(breakout) = breakout {
        let inside = |index: usize| {
            let track = &target[index];
            breakout.inside_any(Point::new(
                (track.start.x_nm + track.end.x_nm).div_euclid(2),
                (track.start.y_nm + track.end.y_nm).div_euclid(2),
            ))
        };
        sites.sort_by_key(|site| inside(site.0));
    }
    for (index, from_start) in sites {
        let track = &target[index];
        let length = (track.end.x_nm - track.start.x_nm).abs() + (track.end.y_nm - track.start.y_nm).abs();
        // n bumps of pitch p need (2n + 1) * p along the segment.
        let capacity = (length.div_euclid(3 * track.width_nm) - 1).div_euclid(2);
        if capacity <= 0 {
            continue;
        }
        // A site is listed once per segment; a later entry for the same
        // index (never produced) would overwrite, as the Python dict does.
        let bumps = capacity.min(remaining);
        placements.retain(|entry| entry.0 != index);
        placements.push((index, from_start, bumps));
        remaining -= bumps;
        if remaining == 0 {
            break;
        }
    }
    if remaining != 0 {
        return PairTuning::default();
    }
    placements.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let partner = partner.clone();
    for &(index, from_start, bumps) in &placements {
        let pieces = serpentine(&target[index], bumps, height, width, from_start, &partner);
        target.splice(index..index + 1, pieces);
    }
    let mut replaced: Vec<usize> = placements.iter().map(|entry| entry.0).collect();
    replaced.sort_unstable();
    PairTuning { added_nm: count * gain(height), first_tuned, replaced }
}

fn axis_aligned(track: &TrackSegment) -> bool {
    track.start != track.end && (track.start.x_nm == track.end.x_nm || track.start.y_nm == track.end.y_nm)
}

/// `_tuning_sites`: axis-aligned target segments, nearest the mismatch
/// origin first, each with whether its bumps start at the segment's start.
fn tuning_sites(target: &[TrackSegment], partner: &[TrackSegment]) -> Vec<(usize, bool)> {
    let axis: Vec<usize> = (0..target.len()).filter(|index| axis_aligned(&target[*index])).collect();
    let (Some(walk), Some(partner_walk)) = (ordered_chain(target), ordered_chain(partner)) else {
        let mut ordered = axis;
        ordered.sort_by(|a, b| {
            (-segment_length(&target[*a]), *a)
                .partial_cmp(&(-segment_length(&target[*b]), *b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return ordered.into_iter().map(|index| (index, true)).collect();
    };
    let first = &target[walk[0].0];
    let mut vertices = vec![if walk[0].1 { first.start } else { first.end }];
    for &(index, forward) in &walk {
        vertices.push(if forward { target[index].end } else { target[index].start });
    }
    let mut arcs = vec![0.0];
    for &(index, _) in &walk {
        let last = *arcs.last().expect("arcs start at zero");
        arcs.push(last + segment_length(&target[index]));
    }
    let mut path: Vec<(Point, Point)> = partner_walk
        .iter()
        .map(|&(index, forward)| {
            let track = &partner[index];
            if forward { (track.start, track.end) } else { (track.end, track.start) }
        })
        .collect();
    let distance = |a: Point, b: Point| hypot((a.x_nm - b.x_nm) as f64, (a.y_nm - b.y_nm) as f64);
    let (head, tail) = (vertices[0], *vertices.last().expect("a walk has vertices"));
    let (path_start, path_end) = (path[0].0, path.last().expect("a walk has segments").1);
    if distance(head, path_end) + distance(tail, path_start) < distance(head, path_start) + distance(tail, path_end) {
        path = path.into_iter().rev().map(|(start, end)| (end, start)).collect();
    }
    let partner_length = sum(path.iter().map(|(a, b)| distance(*b, *a)));
    // Partner-minus-target arc length at each target segment midpoint. Its
    // growth between consecutive samples belongs to the vertex between them.
    let mut lead = vec![0.0];
    for (position, &(index, _)) in walk.iter().enumerate() {
        let track = &target[index];
        let midpoint =
            ((track.start.x_nm + track.end.x_nm) as f64 / 2.0, (track.start.y_nm + track.end.y_nm) as f64 / 2.0);
        lead.push(arc_projection(&path, midpoint) - (arcs[position] + arcs[position + 1]) / 2.0);
    }
    lead.push(partner_length - arcs.last().expect("arcs start at zero"));
    let gains: Vec<f64> = (0..vertices.len()).map(|vertex| lead[vertex + 1] - lead[vertex]).collect();
    // `max(range, key=(gain, -vertex))`: the largest gain, the first vertex on ties.
    let mut origin = 0;
    for vertex in 1..gains.len() {
        if gains[vertex] > gains[origin] {
            origin = vertex;
        }
    }
    let mut sites: Vec<(f64, usize, usize, bool)> = Vec::new();
    for (position, &(index, forward)) in walk.iter().enumerate() {
        if !axis.contains(&index) {
            continue;
        }
        let (distance, near_walk_start) = if position >= origin {
            (arcs[position] - arcs[origin], true)
        } else {
            (arcs[origin] - arcs[position + 1], false)
        };
        sites.push((distance, position, index, near_walk_start == forward));
    }
    sites.sort_by(|a, b| {
        a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then((a.1, a.2, a.3).cmp(&(b.1, b.2, b.3)))
    });
    sites.into_iter().map(|(_, _, index, from_start)| (index, from_start)).collect()
}

/// `_ordered_chain`: walk a simple two-ended path; `(index, walked
/// start-to-end)` per segment.
fn ordered_chain(tracks: &[TrackSegment]) -> Option<Vec<(usize, bool)>> {
    let live: Vec<usize> = (0..tracks.len()).filter(|index| tracks[*index].start != tracks[*index].end).collect();
    let mut ends: indexmap::IndexMap<Point, Vec<usize>> = indexmap::IndexMap::new();
    for &index in &live {
        for point in [tracks[index].start, tracks[index].end] {
            ends.entry(point).or_default().push(index);
        }
    }
    let mut terminals: Vec<Point> =
        ends.iter().filter(|(_, items)| items.len() == 1).map(|(point, _)| *point).collect();
    terminals.sort_by_key(|point| (point.x_nm, point.y_nm));
    if live.is_empty() || terminals.len() != 2 || ends.values().any(|items| items.len() > 2) {
        return None;
    }
    let mut walk = Vec::new();
    let mut used: indexmap::IndexSet<usize> = indexmap::IndexSet::new();
    let mut point = terminals[0];
    while let Some(&index) = ends[&point].iter().find(|index| !used.contains(*index)) {
        used.insert(index);
        let forward = tracks[index].start == point;
        walk.push((index, forward));
        point = if forward { tracks[index].end } else { tracks[index].start };
    }
    (used.len() == live.len()).then_some(walk)
}

/// `_nearest_on_segment`: distance, nearest x/y and fraction along
/// `start`-`end` for `point`.
pub(crate) fn nearest_on_segment(point: (f64, f64), start: Point, end: Point) -> (f64, f64, f64, f64) {
    let (dx, dy) = (end.x_nm - start.x_nm, end.y_nm - start.y_nm);
    let length_squared = dx * dx + dy * dy;
    let fraction = if length_squared == 0 {
        0.0
    } else {
        (((point.0 - start.x_nm as f64) * dx as f64 + (point.1 - start.y_nm as f64) * dy as f64)
            / length_squared as f64)
            .clamp(0.0, 1.0)
    };
    let (x, y) = (start.x_nm as f64 + fraction * dx as f64, start.y_nm as f64 + fraction * dy as f64);
    (hypot(x - point.0, y - point.1), x, y, fraction)
}

/// `_arc_projection`: arc length along `path` of the point nearest `point`
/// (first on ties).
fn arc_projection(path: &[(Point, Point)], point: (f64, f64)) -> f64 {
    let mut best: Option<(f64, f64)> = None;
    let mut travelled = 0.0;
    for (start, end) in path {
        let length = hypot((end.x_nm - start.x_nm) as f64, (end.y_nm - start.y_nm) as f64);
        let (distance, _, _, fraction) = nearest_on_segment(point, *start, *end);
        if best.is_none_or(|(kept, _)| distance < kept) {
            best = Some((distance, travelled + fraction * length));
        }
        travelled += length;
    }
    best.map_or(0.0, |(_, arc)| arc)
}

/// `_serpentine`: replace one axis-aligned segment with `bumps` chamfered
/// bumps whose chamfer leg is at most `chamfer_width`.
fn serpentine(
    track: &TrackSegment,
    bumps: i64,
    height: i64,
    chamfer_width: i64,
    from_start: bool,
    partner: &[TrackSegment],
) -> Vec<TrackSegment> {
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    let length = dx.abs() + dy.abs();
    let (ux, uy) = (dx.signum(), dy.signum());
    let pitch = 3 * track.width_nm;
    let mut offsets: Vec<i64> = (0..bumps).map(|bump| pitch + 2 * pitch * bump).collect();
    if !from_start {
        offsets = offsets.iter().map(|offset| length - pitch - offset).collect();
        offsets.sort_unstable();
    }
    let (nx, ny) = away_from_partner(track, partner);
    let (base, top) = bump_chamfers(height, chamfer_width, 0);
    let at = |distance: i64, rise: i64| {
        Point::new(track.start.x_nm + ux * distance + nx * rise, track.start.y_nm + uy * distance + ny * rise)
    };
    let mut points = vec![track.start];
    for offset in offsets {
        points.extend(bump_path(offset, offset + pitch, height, base, top).into_iter().map(|(a, r)| at(a, r)));
    }
    points.push(track.end);
    points
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .map(|pair| TrackSegment {
            net: track.net.clone(),
            start: pair[0],
            end: pair[1],
            width_nm: track.width_nm,
            layer: track.layer,
        })
        .collect()
}

/// `_away_from_partner`: unit normal of an axis-aligned segment pointing
/// away from its partner.
fn away_from_partner(track: &TrackSegment, partner: &[TrackSegment]) -> (i64, i64) {
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    let (ux, uy) = (dx.signum(), dy.signum());
    let left = (-uy, ux);
    let midpoint = ((track.start.x_nm + track.end.x_nm) as f64 / 2.0, (track.start.y_nm + track.end.y_nm) as f64 / 2.0);
    let same_layer: Vec<&TrackSegment> =
        partner.iter().filter(|item| item.layer == track.layer && item.start != item.end).collect();
    let nearby: Vec<&TrackSegment> =
        if same_layer.is_empty() { partner.iter().filter(|item| item.start != item.end).collect() } else { same_layer };
    let mut nearest: Option<(f64, f64, f64, f64)> = None;
    for item in nearby {
        let candidate = nearest_on_segment(midpoint, item.start, item.end);
        if nearest.is_none_or(|kept| candidate.0 < kept.0) {
            nearest = Some(candidate);
        }
    }
    if let Some((_, x, y, _)) = nearest {
        let side = left.0 as f64 * (x - midpoint.0) + left.1 as f64 * (y - midpoint.1);
        if side > 0.0 {
            return (-left.0, -left.1);
        }
        if side < 0.0 {
            return left;
        }
    }
    if dx != 0 { (0, 1) } else { (1, 0) }
}

/// `_segment_length`.
pub(crate) fn segment_length(track: &TrackSegment) -> f64 {
    hypot((track.end.x_nm - track.start.x_nm) as f64, (track.end.y_nm - track.start.y_nm) as f64)
}

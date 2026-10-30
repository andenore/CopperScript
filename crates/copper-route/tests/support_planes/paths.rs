//! Path geometry of tuned copper: headings, sharp bends and lane offsets
//! (`_heading`, `_sharp_bends`, `_lane_offsets` of the Python tests).

use copper_pcb::board::TrackSegment;
use copper_physical::Point;
use copper_physical::geometry::segment_distance_squared;
use num_bigint::BigInt;
use num_rational::BigRational;

pub fn of_net(tracks: &[TrackSegment], net: &str) -> Vec<TrackSegment> {
    tracks.iter().filter(|track| track.net == net).cloned().collect()
}

pub fn ends(tracks: &[TrackSegment]) -> impl Iterator<Item = Point> + '_ {
    tracks.iter().flat_map(|track| [track.start, track.end])
}

pub fn squared(value: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(value) * BigInt::from(value))
}

/// Least squared distance between any track of `first` and any of `second`.
pub fn least_gap_squared(first: &[TrackSegment], second: &[TrackSegment]) -> BigRational {
    first
        .iter()
        .flat_map(|a| second.iter().map(move |b| segment_distance_squared(a.start, a.end, b.start, b.end)))
        .min()
        .expect("tracks")
}

pub const COMPASS: [(i64, i64); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Compass index of an axis-aligned or exactly 45-degree track (`_heading`).
pub fn heading(track: &TrackSegment) -> usize {
    let (dx, dy) = (track.end.x_nm - track.start.x_nm, track.end.y_nm - track.start.y_nm);
    assert!((dx == 0) != (dy == 0) || (dx.abs() == dy.abs() && dx != 0), "{track:?}");
    COMPASS.iter().position(|step| *step == (dx.signum(), dy.signum())).unwrap()
}

/// Vertices of a continuous path that turn by more than 45 degrees, except `keep`.
pub fn sharp_bends(path: &[TrackSegment], keep: &[Point]) -> Vec<Point> {
    path.windows(2)
        .filter(|pair| {
            let (a, b) = (heading(&pair[0]), heading(&pair[1]));
            !keep.contains(&pair[0].end) && ((b + 8 - a) % 8).min((a + 8 - b) % 8) > 1
        })
        .map(|pair| pair[0].end)
        .collect()
}

pub fn continuous(path: &[TrackSegment]) -> bool {
    path.windows(2).all(|pair| pair[0].end == pair[1].start)
}

/// Squared line distance from each track of `first` to its nearest parallel track of `second`.
pub fn lane_offsets(first: &[TrackSegment], second: &[TrackSegment]) -> Vec<BigRational> {
    first
        .iter()
        .map(|a| {
            let nearest = second
                .iter()
                .filter(|b| heading(b) % 4 == heading(a) % 4)
                .min_by_key(|b| segment_distance_squared(a.start, a.end, b.start, b.end))
                .expect("parallel track");
            let (dx, dy) = (i128::from(a.end.x_nm - a.start.x_nm), i128::from(a.end.y_nm - a.start.y_nm));
            let cross =
                dx * i128::from(nearest.start.y_nm - a.start.y_nm) - dy * i128::from(nearest.start.x_nm - a.start.x_nm);
            BigRational::new(BigInt::from(cross * cross), BigInt::from(dx * dx + dy * dy))
        })
        .collect()
}

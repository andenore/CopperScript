//! A one-bit raster of a Gerber image on a square grid. A pixel is dark when
//! its centre is inside the shape; polygons fill by the nonzero winding rule.
//! Coordinates are Gerber nanometres (Y up); row 0 is the bottom row.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Gerber coordinates of the lower-left corner of pixel (0, 0).
    pub x0: f64,
    pub y0: f64,
    pub pitch: f64,
    pub width: usize,
    pub height: usize,
}

impl Frame {
    /// A frame covering `bounds` (min x, min y, max x, max y) with a margin.
    pub fn covering(bounds: (f64, f64, f64, f64), pitch: f64) -> Frame {
        let margin = 4.0 * pitch;
        let x0 = ((bounds.0 - margin) / pitch).floor() * pitch;
        let y0 = ((bounds.1 - margin) / pitch).floor() * pitch;
        let width = (((bounds.2 + margin - x0) / pitch).ceil() as usize).max(1);
        let height = (((bounds.3 + margin - y0) / pitch).ceil() as usize).max(1);
        Frame { x0, y0, pitch, width, height }
    }
}

/// A connected region: pixel count, then min column, min row, max column, max row.
pub type Blob = (u64, usize, usize, usize, usize);

#[derive(Clone)]
pub struct Raster {
    pub frame: Frame,
    words: usize,
    bits: Vec<u64>,
}

impl Raster {
    pub fn new(frame: Frame) -> Raster {
        let words = frame.width.div_ceil(64);
        Raster { frame, words, bits: vec![0; words * frame.height] }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.words + x / 64] >> (x % 64) & 1 == 1
    }

    pub fn count(&self) -> u64 {
        self.bits.iter().map(|word| u64::from(word.count_ones())).sum()
    }

    /// Set (`dark`) or clear pixels `from..to` of `row`.
    fn span(&mut self, row: usize, from: usize, to: usize, dark: bool) {
        if from >= to {
            return;
        }
        let base = row * self.words;
        let (first, last) = (from / 64, (to - 1) / 64);
        for word in first..=last {
            let lo = if word == first { from % 64 } else { 0 };
            let hi = if word == last { (to - 1) % 64 + 1 } else { 64 };
            let mask = if hi - lo == 64 { u64::MAX } else { ((1u64 << (hi - lo)) - 1) << lo };
            if dark {
                self.bits[base + word] |= mask;
            } else {
                self.bits[base + word] &= !mask;
            }
        }
    }

    /// Pixel columns whose centres lie in `[left, right]`.
    fn columns(&self, left: f64, right: f64) -> Option<(usize, usize)> {
        let f = &self.frame;
        let from = ((left - f.x0) / f.pitch - 0.5).ceil().max(0.0);
        let to = ((right - f.x0) / f.pitch - 0.5).floor() + 1.0;
        let to = to.min(f.width as f64);
        (to > from).then_some((from as usize, to as usize))
    }

    fn rows(&self, bottom: f64, top: f64) -> std::ops::Range<usize> {
        let f = &self.frame;
        let from = ((bottom - f.y0) / f.pitch - 0.5).ceil().max(0.0) as usize;
        let to = (((top - f.y0) / f.pitch - 0.5).floor() + 1.0).clamp(0.0, f.height as f64) as usize;
        from..to.max(from)
    }

    fn row_centre(&self, row: usize) -> f64 {
        self.frame.y0 + (row as f64 + 0.5) * self.frame.pitch
    }

    pub fn disk(&mut self, cx: f64, cy: f64, radius: f64, dark: bool) {
        if radius <= 0.0 {
            return;
        }
        for row in self.rows(cy - radius, cy + radius) {
            let dy = self.row_centre(row) - cy;
            let half = (radius * radius - dy * dy).max(0.0).sqrt();
            if let Some((from, to)) = self.columns(cx - half, cx + half) {
                self.span(row, from, to, dark);
            }
        }
    }

    /// Fill a closed polygon (nonzero winding).
    pub fn polygon(&mut self, points: &[(f64, f64)], dark: bool) {
        if points.len() < 3 {
            return;
        }
        let (bottom, top) = points.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        for row in self.rows(bottom, top) {
            let y = self.row_centre(row);
            crossings.clear();
            for index in 0..points.len() {
                let (a, b) = (points[index], points[(index + 1) % points.len()]);
                if (a.1 <= y) != (b.1 <= y) {
                    let x = a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1);
                    crossings.push((x, if b.1 > a.1 { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                if winding != 0
                    && let Some((from, to)) = self.columns(pair[0].0, pair[1].0)
                {
                    self.span(row, from, to, dark);
                }
            }
        }
    }

    /// A stroke of round pen `width` from `a` to `b`.
    pub fn capsule(&mut self, a: (f64, f64), b: (f64, f64), width: f64, dark: bool) {
        let radius = width / 2.0;
        if radius <= 0.0 {
            return;
        }
        self.disk(a.0, a.1, radius, dark);
        self.disk(b.0, b.1, radius, dark);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length = dx.hypot(dy);
        if length > 0.0 {
            let (nx, ny) = (-dy / length * radius, dx / length * radius);
            self.polygon(
                &[(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)],
                dark,
            );
        }
    }

    /// Pixels set in exactly one of the rasters, which must share a frame.
    pub fn xor(&self, other: &Raster) -> Raster {
        assert_eq!(self.frame, other.frame);
        Raster {
            frame: self.frame,
            words: self.words,
            bits: self.bits.iter().zip(&other.bits).map(|(a, b)| a ^ b).collect(),
        }
    }

    /// Pixels still set after `radius` erosions by the 4-neighbourhood:
    /// differences thicker than the boundary noise of rasterising the same
    /// edge twice.
    pub fn eroded(&self, radius: usize) -> Raster {
        let mut result = self.clone();
        for _ in 0..radius {
            let source = result.clone();
            let (width, height, words) = (self.frame.width, self.frame.height, self.words);
            for row in 0..height {
                for word in 0..words {
                    let mut value = source.bits[row * words + word];
                    if value == 0 {
                        continue;
                    }
                    for neighbour in [row.checked_sub(1), Some(row + 1).filter(|&r| r < height)] {
                        value &= neighbour.map_or(0, |r| source.bits[r * words + word]);
                    }
                    let left = |r: usize| {
                        let here = source.bits[r * words + word];
                        let carry = if word > 0 { source.bits[r * words + word - 1] >> 63 } else { 0 };
                        here << 1 | carry
                    };
                    let right = |r: usize| {
                        let here = source.bits[r * words + word];
                        let carry = if word + 1 < words { source.bits[r * words + word + 1] << 63 } else { 0 };
                        here >> 1 | carry
                    };
                    value &= left(row) & right(row);
                    if word + 1 == words && width % 64 != 0 {
                        value &= (1u64 << (width % 64)) - 1;
                    }
                    result.bits[row * words + word] = value;
                }
            }
        }
        result
    }

    /// The number of 8-connected components and the `limit` largest.
    pub fn components(&self, limit: usize) -> (usize, Vec<Blob>) {
        let (width, height) = (self.frame.width, self.frame.height);
        let mut seen = vec![0u64; self.bits.len()];
        let mut found = Vec::new();
        let mut stack = Vec::new();
        for row in 0..height {
            for word in 0..self.words {
                let mut pending = self.bits[row * self.words + word] & !seen[row * self.words + word];
                while pending != 0 {
                    let x = word * 64 + pending.trailing_zeros() as usize;
                    pending &= pending - 1;
                    if seen[row * self.words + x / 64] >> (x % 64) & 1 == 1 {
                        continue;
                    }
                    let mut blob = (0u64, x, row, x, row);
                    stack.push((x, row));
                    seen[row * self.words + x / 64] |= 1 << (x % 64);
                    while let Some((px, py)) = stack.pop() {
                        blob.0 += 1;
                        blob.1 = blob.1.min(px);
                        blob.2 = blob.2.min(py);
                        blob.3 = blob.3.max(px);
                        blob.4 = blob.4.max(py);
                        for ny in py.saturating_sub(1)..=(py + 1).min(height - 1) {
                            for nx in px.saturating_sub(1)..=(px + 1).min(width - 1) {
                                let index = ny * self.words + nx / 64;
                                if self.bits[index] >> (nx % 64) & 1 == 1 && seen[index] >> (nx % 64) & 1 == 0 {
                                    seen[index] |= 1 << (nx % 64);
                                    stack.push((nx, ny));
                                }
                            }
                        }
                    }
                    found.push(blob);
                }
            }
        }
        let total = found.len();
        found.sort_by(|a, b| b.0.cmp(&a.0).then((a.1, a.2).cmp(&(b.1, b.2))));
        found.truncate(limit);
        (total, found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame { x0: 0.0, y0: 0.0, pitch: 1.0, width: 100, height: 100 }
    }

    #[test]
    fn disk_area_matches_pi_r_squared() {
        let mut raster = Raster::new(frame());
        raster.disk(50.0, 50.0, 20.0, true);
        let area = raster.count() as f64;
        assert!((area - std::f64::consts::PI * 400.0).abs() < 20.0, "{area}");
    }

    #[test]
    fn clear_polarity_removes_and_winding_keeps_holes() {
        let mut raster = Raster::new(frame());
        raster.polygon(&[(10.0, 10.0), (90.0, 10.0), (90.0, 90.0), (10.0, 90.0)], true);
        assert_eq!(raster.count(), 6400);
        raster.polygon(&[(40.0, 40.0), (60.0, 40.0), (60.0, 60.0), (40.0, 60.0)], false);
        assert_eq!(raster.count(), 6000);
        // A contour with a cut-in hole: outer counter-clockwise, inner clockwise.
        let mut cut = Raster::new(frame());
        cut.polygon(
            &[
                (10.0, 10.0),
                (90.0, 10.0),
                (90.0, 90.0),
                (10.0, 90.0),
                (10.0, 50.0),
                (40.0, 50.0),
                (40.0, 60.0),
                (60.0, 60.0),
                (60.0, 40.0),
                (40.0, 40.0),
                (40.0, 50.0),
                (10.0, 50.0),
            ],
            true,
        );
        assert_eq!(cut.count(), 6000);
    }

    #[test]
    fn erosion_drops_one_pixel_slivers_and_components_locate_blobs() {
        let mut raster = Raster::new(frame());
        raster.polygon(&[(0.0, 10.0), (100.0, 10.0), (100.0, 11.0), (0.0, 11.0)], true);
        raster.polygon(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0), (20.0, 30.0)], true);
        let eroded = raster.eroded(1);
        assert_eq!(eroded.count(), 64);
        let (total, blobs) = eroded.components(5);
        assert_eq!(total, 1);
        assert_eq!(blobs[0], (64, 21, 21, 28, 28));
    }
}

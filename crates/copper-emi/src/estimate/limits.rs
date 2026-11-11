//! Emission limit lines as data with log-frequency interpolation.
//!
//! **Every table in this module was entered from the published numbers as
//! remembered, not from the purchased standards. Values MUST be verified
//! against CISPR 32 / EN 55032, FCC 47 CFR Part 15 and the applicable product
//! standard before any compliance use.** Limits for other ports, frequency
//! ranges (e.g. above 1 GHz), measurement distances or classes are not
//! included on purpose: a missing table is better than a guessed one. CISPR 25
//! (automotive) is omitted.
//!
//! Interpolation: within a [`LimitSegment`] the level varies linearly in dB
//! against `log10(frequency)` (this is how the standards draw the
//! 150-500 kHz conducted slope); flat segments have equal end levels. At a
//! frequency shared by two segments (a step) the lower segment applies, which
//! is how the standards state the transition frequency.

use super::volts_to_dbuv;

/// Detector a limit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detector {
    /// Peak.
    Peak,
    /// CISPR quasi-peak.
    QuasiPeak,
    /// CISPR average.
    Average,
}

/// Equipment class (A: commercial/industrial, B: residential).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Class A.
    A,
    /// Class B.
    B,
}

/// Unit of the limit level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitUnit {
    /// Conducted: dB relative to 1 uV.
    DbUv,
    /// Radiated: dB relative to 1 uV/m.
    DbUvPerM,
}

/// Measurement distance of a radiated limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distance {
    /// 3 m.
    M3,
    /// 10 m.
    M10,
}

impl Distance {
    /// Distance in metres.
    pub fn metres(self) -> f64 {
        match self {
            Distance::M3 => 3.0,
            Distance::M10 => 10.0,
        }
    }
}

/// One piece of a limit line: level in dB from `level_start` at `f_start` to
/// `level_end` at `f_end`, linear in `log10(f)`. Frequencies in Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimitSegment {
    /// Lower edge, Hz.
    pub f_start: f64,
    /// Upper edge, Hz.
    pub f_end: f64,
    /// Level at `f_start`.
    pub level_start: f64,
    /// Level at `f_end`.
    pub level_end: f64,
}

/// A limit line: ordered, non-overlapping segments.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitLine {
    /// Standard and table the numbers are believed to come from.
    pub source: &'static str,
    /// Detector.
    pub detector: Detector,
    /// Level unit.
    pub unit: LimitUnit,
    /// Measurement distance in metres for radiated limits.
    pub distance: Option<f64>,
    /// Segments in ascending frequency.
    pub segments: Vec<LimitSegment>,
}

impl LimitLine {
    /// Limit level at `frequency` (Hz), or `None` outside the covered range.
    pub fn at(&self, frequency: f64) -> Option<f64> {
        let seg = self
            .segments
            .iter()
            .find(|s| frequency >= s.f_start * (1.0 - 1e-12) && frequency <= s.f_end * (1.0 + 1e-12))?;
        if (seg.level_end - seg.level_start).abs() < f64::EPSILON {
            return Some(seg.level_start);
        }
        let t = (frequency.log10() - seg.f_start.log10()) / (seg.f_end.log10() - seg.f_start.log10());
        Some(seg.level_start + t.clamp(0.0, 1.0) * (seg.level_end - seg.level_start))
    }

    /// Covered frequency range `(lowest, highest)` in Hz.
    pub fn range(&self) -> (f64, f64) {
        (self.segments.first().map_or(f64::NAN, |s| s.f_start), self.segments.last().map_or(f64::NAN, |s| s.f_end))
    }

    /// Margin in dB (limit minus level; positive means below the limit) at
    /// `frequency`, or `None` outside the covered range.
    pub fn margin(&self, frequency: f64, level: f64) -> Option<f64> {
        self.at(frequency).map(|l| l - level)
    }
}

fn seg(f_start: f64, f_end: f64, level_start: f64, level_end: f64) -> LimitSegment {
    LimitSegment { f_start, f_end, level_start, level_end }
}

/// Mains-port conducted limits of CISPR 32 and FCC 15.107 (identical numbers,
/// 150 kHz to 30 MHz): Class B quasi-peak 66 to 56 dBuV falling linearly with
/// log f from 0.15 to 0.5 MHz, 56 from 0.5 to 5 MHz, 60 from 5 to 30 MHz;
/// average 10 dB lower; Class A quasi-peak 79 (0.15-0.5 MHz) and 73
/// (0.5-30 MHz), average 66 and 60.
fn conducted_segments(class: Class, detector: Detector) -> Option<Vec<LimitSegment>> {
    const KHZ150: f64 = 150e3;
    const KHZ500: f64 = 500e3;
    const MHZ5: f64 = 5e6;
    const MHZ30: f64 = 30e6;
    match (class, detector) {
        (Class::B, Detector::QuasiPeak) => {
            Some(vec![seg(KHZ150, KHZ500, 66.0, 56.0), seg(KHZ500, MHZ5, 56.0, 56.0), seg(MHZ5, MHZ30, 60.0, 60.0)])
        }
        (Class::B, Detector::Average) => {
            Some(vec![seg(KHZ150, KHZ500, 56.0, 46.0), seg(KHZ500, MHZ5, 46.0, 46.0), seg(MHZ5, MHZ30, 50.0, 50.0)])
        }
        (Class::A, Detector::QuasiPeak) => Some(vec![seg(KHZ150, KHZ500, 79.0, 79.0), seg(KHZ500, MHZ30, 73.0, 73.0)]),
        (Class::A, Detector::Average) => Some(vec![seg(KHZ150, KHZ500, 66.0, 66.0), seg(KHZ500, MHZ30, 60.0, 60.0)]),
        (_, Detector::Peak) => None,
    }
}

/// CISPR 32 / EN 55032 AC mains port conducted emission limit, 150 kHz to
/// 30 MHz, in dBuV, for the quasi-peak or average detector (`None` for peak:
/// the standard has no peak limit here).
///
/// Believed source: CISPR 32 Annex A conducted-emission tables for the AC
/// mains power port (exact table numbers not verified). Confidence: high for
/// the numbers (they are the classic CISPR 22 values); verify before use.
pub fn cispr32_conducted(class: Class, detector: Detector) -> Option<LimitLine> {
    Some(LimitLine {
        source: "CISPR 32 / EN 55032 mains-port conducted (Annex A; table number unverified)",
        detector,
        unit: LimitUnit::DbUv,
        distance: None,
        segments: conducted_segments(class, detector)?,
    })
}

/// CISPR 32 / EN 55032 radiated emission limit, 30 MHz to 1 GHz, quasi-peak,
/// dBuV/m at 3 m or 10 m.
///
/// Values: at 10 m Class A 40 dBuV/m (30-230 MHz) and 47 (230-1000 MHz), Class
/// B 30 and 37; at 3 m Class A 50 and 57, Class B 40 and 47. Believed source:
/// CISPR 32 Annex A radiated tables for OATS/semi-anechoic chamber (table
/// numbers not verified). Confidence: high for 10 m, medium-high for 3 m
/// (the 3 m column is the 10 m column plus 10 dB in the standard, not
/// `20 log10(10/3)`); verify before use. Above 1 GHz is not included.
pub fn cispr32_radiated(class: Class, distance: Distance) -> LimitLine {
    let (low, high) = match (class, distance) {
        (Class::A, Distance::M10) => (40.0, 47.0),
        (Class::B, Distance::M10) => (30.0, 37.0),
        (Class::A, Distance::M3) => (50.0, 57.0),
        (Class::B, Distance::M3) => (40.0, 47.0),
    };
    LimitLine {
        source: "CISPR 32 / EN 55032 radiated 30-1000 MHz (Annex A; table number unverified)",
        detector: Detector::QuasiPeak,
        unit: LimitUnit::DbUvPerM,
        distance: Some(distance.metres()),
        segments: vec![seg(30e6, 230e6, low, low), seg(230e6, 1e9, high, high)],
    }
}

/// FCC 47 CFR 15.107 conducted limit (AC mains, 150 kHz to 30 MHz), dBuV.
/// Numerically identical to [`cispr32_conducted`]; `None` for the peak
/// detector. Believed source: 15.107(a) (Class A) and 15.107(b) (Class B).
/// Confidence: high; verify before use.
pub fn fcc15_conducted(class: Class, detector: Detector) -> Option<LimitLine> {
    Some(LimitLine {
        source: "FCC 47 CFR 15.107(a)/(b)",
        detector,
        unit: LimitUnit::DbUv,
        distance: None,
        segments: conducted_segments(class, detector)?,
    })
}

/// FCC 47 CFR 15.109 radiated limit for unintentional radiators, 30 MHz to
/// 1 GHz, at the distance the rule states (Class A at 10 m, Class B at 3 m),
/// in dBuV/m converted from the rule's uV/m values (quasi-peak below 1 GHz).
///
/// Class B at 3 m: 100 uV/m (30-88 MHz), 150 (88-216), 200 (216-960), 500
/// (above 960). Class A at 10 m: 90, 150, 210, 300 uV/m. Believed source:
/// 15.109(a) (Class B) and 15.109(b) (Class A). Confidence: high; verify
/// before use. Frequencies above 1 GHz (the rule continues to the sixth
/// harmonic or 40 GHz) are not included, so the last segment ends at 1 GHz.
pub fn fcc15_radiated(class: Class) -> LimitLine {
    let (uv, distance): ([f64; 4], f64) = match class {
        Class::B => ([100.0, 150.0, 200.0, 500.0], 3.0),
        Class::A => ([90.0, 150.0, 210.0, 300.0], 10.0),
    };
    let db = |i: usize| volts_to_dbuv(uv[i] * 1e-6);
    LimitLine {
        source: "FCC 47 CFR 15.109(a)/(b)",
        detector: Detector::QuasiPeak,
        unit: LimitUnit::DbUvPerM,
        distance: Some(distance),
        segments: vec![
            seg(30e6, 88e6, db(0), db(0)),
            seg(88e6, 216e6, db(1), db(1)),
            seg(216e6, 960e6, db(2), db(2)),
            // Only the 960 MHz - 1 GHz stretch of the open-ended last row.
            seg(960e6, 1e9, db(3), db(3)),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cispr_b_conducted_hand_values() {
        let qp = cispr32_conducted(Class::B, Detector::QuasiPeak).unwrap();
        assert_eq!(qp.at(150e3), Some(66.0));
        assert_eq!(qp.at(500e3), Some(56.0));
        assert_eq!(qp.at(1e6), Some(56.0));
        assert_eq!(qp.at(10e6), Some(60.0));
        assert_eq!(qp.at(5e6), Some(56.0), "lower segment applies at the step");
        // Log-linear: geometric midpoint of 150 kHz..500 kHz sits at 61 dB.
        let mid = (150e3f64 * 500e3).sqrt();
        assert!((qp.at(mid).unwrap() - 61.0).abs() < 1e-9);
        assert_eq!(qp.at(100e3), None);
        assert_eq!(qp.at(40e6), None);
        let av = cispr32_conducted(Class::B, Detector::Average).unwrap();
        for f in [150e3, mid, 1e6, 10e6, 30e6] {
            assert!((qp.at(f).unwrap() - av.at(f).unwrap() - 10.0).abs() < 1e-9);
        }
        assert!(cispr32_conducted(Class::B, Detector::Peak).is_none());
    }

    #[test]
    fn class_a_conducted_and_fcc_equality() {
        let a = cispr32_conducted(Class::A, Detector::QuasiPeak).unwrap();
        assert_eq!(a.at(200e3), Some(79.0));
        assert_eq!(a.at(2e6), Some(73.0));
        let a_av = cispr32_conducted(Class::A, Detector::Average).unwrap();
        assert_eq!(a_av.at(200e3), Some(66.0));
        assert_eq!(a_av.at(2e6), Some(60.0));
        for class in [Class::A, Class::B] {
            for det in [Detector::QuasiPeak, Detector::Average] {
                let c = cispr32_conducted(class, det).unwrap();
                let f = fcc15_conducted(class, det).unwrap();
                assert_eq!(c.segments, f.segments);
            }
        }
    }

    #[test]
    fn cispr_radiated_values_and_steps() {
        let b10 = cispr32_radiated(Class::B, Distance::M10);
        assert_eq!(b10.at(100e6), Some(30.0));
        assert_eq!(b10.at(230e6), Some(30.0));
        assert_eq!(b10.at(500e6), Some(37.0));
        assert_eq!(b10.distance, Some(10.0));
        let a10 = cispr32_radiated(Class::A, Distance::M10);
        assert_eq!((a10.at(100e6), a10.at(500e6)), (Some(40.0), Some(47.0)));
        let b3 = cispr32_radiated(Class::B, Distance::M3);
        assert_eq!((b3.at(100e6), b3.at(500e6)), (Some(40.0), Some(47.0)));
        assert_eq!(b3.range(), (30e6, 1e9));
        assert_eq!(b3.at(20e6), None);
        assert_eq!(b3.margin(100e6, 35.0), Some(5.0));
    }

    #[test]
    fn fcc_radiated_matches_published_db_values() {
        let b = fcc15_radiated(Class::B);
        assert!((b.at(50e6).unwrap() - 40.0).abs() < 1e-9);
        assert!((b.at(100e6).unwrap() - 43.52).abs() < 0.01);
        assert!((b.at(500e6).unwrap() - 46.02).abs() < 0.01);
        assert!((b.at(980e6).unwrap() - 53.98).abs() < 0.01);
        assert_eq!(b.distance, Some(3.0));
        let a = fcc15_radiated(Class::A);
        assert!((a.at(50e6).unwrap() - 39.08).abs() < 0.01);
        assert!((a.at(500e6).unwrap() - 46.44).abs() < 0.01);
        assert_eq!(a.distance, Some(10.0));
    }
}

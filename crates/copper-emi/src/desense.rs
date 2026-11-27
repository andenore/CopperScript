//! Desensitisation screening: the power a fast signal's spectral lines deliver into a receiver's antenna port
//! through a coupling `S` (`docs/coupling-query-plan.md`, `EMI-RF-DESENSE`).
//!
//! The source. A driver declared as a Thevenin source (`open_circuit_swing` behind `output_impedance` `R`) makes,
//! for a line of single-sided peak amplitude `a` of its open-circuit voltage, an available power `a^2 / (8 R)`:
//! the power it would put into a matched load. A driver declared by its loaded pin voltage (`signal_swing`) gives
//! the line's amplitude `a` at the pin into the line, taken as the power `a^2 / (2 R)` of that voltage across the
//! port's reference `R` (exact when the line is matched to `R`). Both are the power of the wave incident on the
//! coupling network from the driver's port, whose reference is the driver's termination.
//!
//! The path. `|S_rx,drv|^2` is the fraction of that incident power that arrives at the receiver's port. By
//! reciprocity (a passive structure, real reference impedances) it equals `|S_drv,rx|^2`, which is what a run that
//! excites the antenna's feed measures.
//!
//! The verdict. A spur received at `P` dBm passes when `P <= sensitivity - margin`, both declared on the antenna.
//! No published threshold is applied: the margin is the designer's. A CW spur at the sensitivity level is in the
//! receiver's channel as strong as the weakest signal it is specified to receive. Screening evidence.

use crate::signal::Line;

/// One received spur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spur {
    pub harmonic: u32,
    pub frequency: f64,
    /// The power incident from the driver at this line, dBm.
    pub source_dbm: f64,
    /// `|S|` from the driver's port to the receiver's, dB.
    pub coupling_db: f64,
    /// `source_dbm + coupling_db`.
    pub received_dbm: f64,
}

/// How the driver's swing is declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Open-circuit amplitude behind the source resistance.
    Thevenin,
    /// Amplitude at the pin into the port's reference.
    Loaded,
}

/// `10 log10(P / 1 mW)` of a power in watts.
pub fn dbm(watts: f64) -> f64 {
    10.0 * (watts / 1e-3).log10()
}

/// The incident power, W, of a line of peak amplitude `a` V from a source of `ohms`.
pub fn incident_power(a: f64, ohms: f64, source: Source) -> f64 {
    match source {
        Source::Thevenin => a * a / (8.0 * ohms),
        Source::Loaded => a * a / (2.0 * ohms),
    }
}

/// `coupling_db` at `f` by linear interpolation in dB between the samples `(freqs, db)`; `None` outside them.
pub fn interpolate_db(freqs: &[f64], db: &[f64], f: f64) -> Option<f64> {
    let k = freqs.windows(2).position(|w| w[0] <= f && f <= w[1])?;
    let (f0, f1) = (freqs[k], freqs[k + 1]);
    let t = if f1 > f0 { (f - f0) / (f1 - f0) } else { 0.0 };
    Some(db[k] + t * (db[k + 1] - db[k]))
}

/// The spurs of `lines` inside `band` received through the coupling sampled as `(freqs, coupling_db)`.
pub fn received_spurs(
    lines: &[Line],
    ohms: f64,
    source: Source,
    band: (f64, f64),
    freqs: &[f64],
    coupling_db: &[f64],
) -> Vec<Spur> {
    lines
        .iter()
        .filter(|l| l.frequency >= band.0 && l.frequency <= band.1)
        .filter_map(|l| {
            let coupling = interpolate_db(freqs, coupling_db, l.frequency)?;
            let source_dbm = dbm(incident_power(l.amplitude, ohms, source));
            Some(Spur {
                harmonic: l.harmonic,
                frequency: l.frequency,
                source_dbm,
                coupling_db: coupling,
                received_dbm: source_dbm + coupling,
            })
        })
        .collect()
}

/// The strongest spur and the margin `sensitivity - margin - received` (positive passes).
pub fn verdict(spurs: &[Spur], sensitivity_dbm: f64, margin_db: f64) -> Option<(Spur, f64)> {
    let worst = spurs.iter().copied().max_by(|a, b| a.received_dbm.total_cmp(&b.received_dbm))?;
    Some((worst, sensitivity_dbm - margin_db - worst.received_dbm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::clock_lines;

    #[test]
    fn a_thevenin_line_delivers_its_available_power() {
        // 1 V peak open circuit behind 50 ohm: 1/(8*50) W = 2.5 mW = 3.98 dBm.
        assert!((dbm(incident_power(1.0, 50.0, Source::Thevenin)) - 3.9794).abs() < 1e-3);
        // The same line loaded into a matched 50 ohm port: 0.5 V peak at the pin, the same power.
        let loaded = incident_power(0.5, 50.0, Source::Loaded);
        assert!((loaded - incident_power(1.0, 50.0, Source::Thevenin)).abs() < 1e-15);
    }

    #[test]
    fn coupling_interpolates_in_db_and_not_outside_the_sweep() {
        let (f, d) = ([1e9, 2e9, 3e9], [-40.0, -30.0, -50.0]);
        assert_eq!(interpolate_db(&f, &d, 1.5e9), Some(-35.0));
        assert_eq!(interpolate_db(&f, &d, 2.5e9), Some(-40.0));
        assert_eq!(interpolate_db(&f, &d, 0.5e9), None);
        assert_eq!(interpolate_db(&f, &d, 3.5e9), None);
    }

    #[test]
    fn a_48_mhz_clock_puts_only_its_odd_51st_line_into_the_2_4_ghz_band() {
        // 48 MHz harmonics in 2400-2483.5 MHz: the 50th (2400 MHz) and 51st (2448 MHz).
        let lines = clock_lines(3.3, 48e6, 0.5, 1e-9, 1e-9, 2.5e9);
        let band = (2.4e9, 2.4835e9);
        let spurs = received_spurs(&lines, 33.0, Source::Thevenin, band, &[2.3e9, 2.6e9], &[-40.0, -40.0]);
        let harmonics: Vec<u32> = spurs.iter().map(|s| s.harmonic).collect();
        // A 50 % clock has no even harmonics.
        assert_eq!(harmonics, [51]);
        let s = spurs[0];
        assert!((s.received_dbm - (s.source_dbm - 40.0)).abs() < 1e-12);
        // The line of a 1 ns-edged 3.3 V clock that far up is tens of dB below the fundamental.
        let fundamental = dbm(incident_power(lines[0].amplitude, 33.0, Source::Thevenin));
        assert!(s.source_dbm < fundamental - 30.0, "{} against {fundamental}", s.source_dbm);
        let (worst, margin) = verdict(&spurs, -95.0, 6.0).expect("a spur");
        assert_eq!(worst, s);
        assert!((margin - (-101.0 - s.received_dbm)).abs() < 1e-12);
    }
}

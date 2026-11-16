//! The rule-of-thumb defaults of `copper emi-check`, in one place.
//!
//! A default fills a gap the language leaves open (an edge time the datasheet
//! was not transcribed for, a device capacitance, a capacitor's DC-bias loss,
//! the inductor ripple when the inductance cannot be resolved). Declared values always win. A result that rests on a
//! default is reported as a BAND (a fast and a slow, or a low and a high bound)
//! next to the nominal value, carries a `rule_of_thumb_*` tag, and the value
//! and the rule are printed like every other input.
//!
//! **Every number in this module is a rule of thumb. None of them is verified
//! against a datasheet or a published source**; the ranges are the spread
//! engineers commonly quote for the device class, chosen so that the band is
//! wide rather than precise. They are screening aids, never datasheet facts.
//! Reviewing the table means reviewing this file.

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// The edge time (or slew rate) is a default band.
pub const TAG_EDGE: &str = "rule_of_thumb_edge";
/// The edge time comes from a declared `slew_rate` (exact, no band).
pub const TAG_EDGE_FROM_SLEW: &str = "edge_from_slew_rate";
/// A switch-node device capacitance is a default band.
pub const TAG_CAPACITANCE: &str = "rule_of_thumb_capacitance";
/// A ceramic capacitor's DC-bias loss is a default band.
pub const TAG_DC_BIAS: &str = "rule_of_thumb_dc_bias";
/// The inductor current ripple ratio is a default band (the power inductor's value was not found).
pub const TAG_RIPPLE: &str = "rule_of_thumb_ripple";
/// No `current_rise_time`/`current_fall_time` is declared, so the voltage edge stands in for the current commutation.
pub const TAG_CURRENT_EDGE_FROM_VOLTAGE: &str = "current_edge_from_voltage_edge";
/// Capacitor ESR and ESL are package defaults.
pub const TAG_PARASITICS: &str = "default_capacitor_parasitics";
/// The `basis` of an estimate that carries a band.
pub const BASIS: &str = "rule_of_thumb";

/// Whether a tag marks a result that rests on a rule of thumb.
pub fn is_rule_tag(tag: &str) -> bool {
    tag.starts_with("rule_of_thumb_")
}

/// The wording of an `input:` line for a defaulted input.
pub fn rule_text(rule: &str, attribute: &str) -> String {
    format!("(rule of thumb: {rule}; declare {attribute} to replace it)")
}

// ---------------------------------------------------------------------------
// 1. Edge time
// ---------------------------------------------------------------------------

/// A linear ramp takes 80 % of its full (0 to 100 %) time between the 10 % and 90 % points.
pub const RAMP_10_90: f64 = 0.8;
/// Nominal switch-node slew rate of an integrated silicon converter, V/ns.
///
/// Rule: integrated silicon switches (buck converter ICs with on-chip FETs)
/// switch at about 1 to 10 V/ns, typically around 3. Rule of thumb.
pub const SLEW_NOMINAL: f64 = 3.0;
/// Fast bound of the default slew band, V/ns (the unfavourable end for emission). Rule of thumb.
pub const SLEW_FAST: f64 = 10.0;
/// Slow bound of the default slew band, V/ns. Rule of thumb.
pub const SLEW_SLOW: f64 = 1.0;
/// A declared slew rate at or above this is reported as wide-bandgap class (GaN, SiC), V/ns.
/// Information only: it changes no number. Rule of thumb.
pub const WIDE_BANDGAP_SLEW: f64 = 20.0;

/// The 10-90 % edge time in seconds of a swing of `swing` volts at `slew` V/ns:
/// `t = 0.8 V / slew`. Exact for a linear ramp (a declared `slew_rate` uses it as is).
pub fn edge_10_90(swing: f64, slew_v_per_ns: f64) -> f64 {
    RAMP_10_90 * swing / (slew_v_per_ns * 1e9)
}

/// The default edge band for a swing: `(nominal, fast, slow)` 10-90 % times, seconds.
pub fn default_edge_band(swing: f64) -> (f64, f64, f64) {
    (edge_10_90(swing, SLEW_NOMINAL), edge_10_90(swing, SLEW_FAST), edge_10_90(swing, SLEW_SLOW))
}

/// The rule behind [`default_edge_band`], for `input:` lines.
pub fn edge_rule() -> String {
    format!(
        "integrated silicon switch node, slew {SLEW_NOMINAL} V/ns nominal, {SLEW_FAST} V/ns fast bound, {SLEW_SLOW} V/ns slow bound, \
         t(10-90 %) = 0.8 V_swing / slew"
    )
}

// ---------------------------------------------------------------------------
// 2. Switch-node device capacitance
// ---------------------------------------------------------------------------

/// A low, nominal and high capacitance, farads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapacitanceBand {
    pub low: f64,
    pub nominal: f64,
    pub high: f64,
}

impl CapacitanceBand {
    /// The sum of two independent contributions.
    pub fn plus(self, other: CapacitanceBand) -> CapacitanceBand {
        CapacitanceBand {
            low: self.low + other.low,
            nominal: self.nominal + other.nominal,
            high: self.high + other.high,
        }
    }
}

/// Switch-node capacitance of an integrated converter FET (Coss of the switch plus the
/// low-side body diode), nominal / low / high, pF. Rule of thumb.
pub const FET_COSS_PF: (f64, f64, f64) = (250.0, 100.0, 500.0);
/// Junction capacitance of a Schottky catch diode per ampere of rating, pF/A:
/// nominal / low / high. The cell's `current_peak` stands in for the diode rating. Rule of thumb.
pub const DIODE_PF_PER_A: (f64, f64, f64) = (75.0, 50.0, 100.0);
/// A Schottky diode reverse-biased at this voltage or more has its junction capacitance
/// divided by [`DIODE_HIGH_VOLTAGE_DIVISOR`] (capacitance falls with reverse bias), V. Rule of thumb.
pub const DIODE_HIGH_VOLTAGE: f64 = 12.0;
/// The derating divisor at or above [`DIODE_HIGH_VOLTAGE`] (a factor 1/3). Rule of thumb.
pub const DIODE_HIGH_VOLTAGE_DIVISOR: f64 = 3.0;

/// The converter FET's capacitance band.
pub fn fet_capacitance() -> CapacitanceBand {
    CapacitanceBand { low: FET_COSS_PF.1 * 1e-12, nominal: FET_COSS_PF.0 * 1e-12, high: FET_COSS_PF.2 * 1e-12 }
}

/// The catch diode's capacitance band for a rating proxy `current` (A) and reverse voltage
/// `reverse_v` (`None`: unknown, no derating). At or above [`DIODE_HIGH_VOLTAGE`] the nominal and low
/// ends are divided by 3 and the high end keeps the un-derated value.
pub fn diode_capacitance(current: f64, reverse_v: Option<f64>) -> CapacitanceBand {
    let derate =
        if reverse_v.is_some_and(|v| v >= DIODE_HIGH_VOLTAGE) { 1.0 / DIODE_HIGH_VOLTAGE_DIVISOR } else { 1.0 };
    CapacitanceBand {
        low: DIODE_PF_PER_A.1 * 1e-12 * current * derate,
        nominal: DIODE_PF_PER_A.0 * 1e-12 * current * derate,
        high: DIODE_PF_PER_A.2 * 1e-12 * current,
    }
}

/// The rule behind [`fet_capacitance`].
pub fn fet_rule() -> String {
    format!("integrated converter FET {} pF nominal, band {} to {} pF", FET_COSS_PF.0, FET_COSS_PF.1, FET_COSS_PF.2)
}

/// The rule behind [`diode_capacitance`].
pub fn diode_rule(derated: bool) -> String {
    format!(
        "Schottky catch diode {} pF per ampere of current_peak as the rating proxy (band {} to {} pF/A){}",
        DIODE_PF_PER_A.0,
        DIODE_PF_PER_A.1,
        DIODE_PF_PER_A.2,
        if derated {
            format!(
                ", divided by {DIODE_HIGH_VOLTAGE_DIVISOR} at {DIODE_HIGH_VOLTAGE} V reverse voltage or more (the band keeps the un-derated value as its high end)"
            )
        } else {
            String::new()
        }
    )
}

// ---------------------------------------------------------------------------
// 3. Ceramic DC-bias derating
// ---------------------------------------------------------------------------

/// Hot-supply voltage from which the nominal value of a ceramic capacitor is the derated one, V.
/// Below it the nominal is the un-derated value. Rule of thumb.
pub const DC_BIAS_FROM: f64 = 5.0;

/// Fraction of its capacitance a ceramic capacitor of this SMD package loses at DC bias:
/// 0402, 0603 and 0805 lose 50 %, 1206 35 %, 1210 and larger 25 %. Rule of thumb.
pub fn dc_bias_loss(package: &str) -> f64 {
    match package {
        "0402" | "0603" | "0805" => 0.50,
        "1206" => 0.35,
        _ => 0.25,
    }
}

/// Whether the nominal capacitance is the derated one at this hot-supply voltage.
pub fn nominal_is_derated(hot_supply_v: f64) -> bool {
    hot_supply_v >= DC_BIAS_FROM
}

/// The attribute that replaces the DC-bias rule: the capacitance at the bias the board applies, on the component.
pub const DC_BIAS_ATTRIBUTE: &str = "capacitance_at_bias on the component";

/// The rule behind [`dc_bias_loss`]. It applies to a capacitor in a ceramic SMD package whose component declares no
/// `capacitance_at_bias`, whatever its declared ESR and ESL.
pub fn dc_bias_rule(package: &str) -> String {
    format!(
        "ceramic DC-bias loss by package ({package}: {:.0} %; 0402, 0603, 0805 50 %, 1206 35 %, 1210 and larger 25 %); the band runs from no loss to this loss, \
         the nominal is the derated value from {DC_BIAS_FROM} V hot-supply voltage, else the un-derated value",
        dc_bias_loss(package) * 100.0
    )
}

// ---------------------------------------------------------------------------
// 4. Inductor ripple (only when the power inductor's value cannot be resolved)
// ---------------------------------------------------------------------------

/// Peak-to-peak inductor ripple as a fraction of the load current, nominal / low / high.
/// Rule: inductors are commonly chosen for a ripple of 20 to 40 % of the output current,
/// 30 % being the usual starting point. Used only when the power inductor's value cannot be
/// read from the board; with the inductance, the ripple is computed. Rule of thumb.
pub const RIPPLE_RATIO: (f64, f64, f64) = (0.3, 0.2, 0.4);

/// The rule behind [`RIPPLE_RATIO`].
pub fn ripple_rule() -> String {
    format!(
        "peak-to-peak inductor ripple {} x the load current (band {} to {}), the usual 20 to 40 % design range, \
         used because the power inductor's value was not found",
        RIPPLE_RATIO.0, RIPPLE_RATIO.1, RIPPLE_RATIO.2
    )
}

// ---------------------------------------------------------------------------
// 5. Capacitor parasitics by package
// ---------------------------------------------------------------------------

/// ESR assumed for a ceramic capacitor whose part declares none, ohm.
pub const DEFAULT_ESR: f64 = 5e-3;
/// ESL assumed by SMD package, H.
pub const PACKAGE_ESL: [(&str, f64); 5] =
    [("0402", 0.4e-9), ("0603", 0.5e-9), ("0805", 0.6e-9), ("1206", 0.8e-9), ("1210", 0.9e-9)];

// ---------------------------------------------------------------------------
// 6. Fast digital signals (`docs/emi-signal-cable-plan.md`, P1)
// ---------------------------------------------------------------------------

/// The edge time of a fast signal is a default band (no `rise_time`/`fall_time` declared on the driver).
pub const TAG_SIGNAL_EDGE: &str = "rule_of_thumb_signal_edge";
/// The swing of a fast signal is a default band (no `signal_swing` declared on the driver).
pub const TAG_SIGNAL_SWING: &str = "rule_of_thumb_signal_swing";

/// Edge time of a data signal as a fraction of the unit interval: `(nominal, fast, slow)`.
///
/// Basis. A 10-90 % rise time `t_r` and the -3 dB bandwidth `B` of a single-pole system are related by
/// `B = 0.35 / t_r` (Johnson and Graham, *High-Speed Digital Design: A Handbook of Black Magic*, Prentice Hall,
/// 1993, ch. 1; Bogatin, *Signal and Power Integrity - Simplified*, 3rd ed., ch. 2). With that relation 0.2 to
/// 0.35 UI is a driver bandwidth of 1.75 down to 1 times the bit rate. **The band itself is a rule of thumb:
/// no published source for the 0.2 to 0.35 UI figures is known to the author**, and drivers of the same rate
/// differ widely (standards specify a minimum transition time, not a typical one). Pass the 10-90 % time as the
/// ramp duration, as [`copper_emi::estimate::trapezoid_harmonic`] does.
pub const SIGNAL_EDGE_UI: (f64, f64, f64) = (0.25, 0.20, 0.35);
/// Edge time of a clock or other signal with no declared rate, seconds: `(nominal, fast, slow)`.
/// Rule of thumb for an on-board CMOS clock driver of a few ns down to a few hundred ps; **no published source
/// is cited for these three numbers**.
pub const SIGNAL_EDGE_CLOCK: (f64, f64, f64) = (1.0e-9, 0.3e-9, 3.0e-9);

/// The default edge band of a fast signal, `(nominal, fast, slow)` seconds: a fraction of the unit interval when
/// the driver declares a `data_rate`, else [`SIGNAL_EDGE_CLOCK`].
pub fn signal_edge_band(data_rate: Option<f64>) -> (f64, f64, f64) {
    match data_rate {
        Some(rate) if rate > 0.0 => {
            let (nominal, fast, slow) = SIGNAL_EDGE_UI;
            (nominal / rate, fast / rate, slow / rate)
        }
        _ => SIGNAL_EDGE_CLOCK,
    }
}

/// The rule behind [`signal_edge_band`], for `input:` lines.
pub fn signal_edge_rule(data_rate: bool) -> String {
    if data_rate {
        format!(
            "data signal, t(10-90 %) = {} UI nominal, {} UI fast bound, {} UI slow bound (B = 0.35 / t_r)",
            SIGNAL_EDGE_UI.0, SIGNAL_EDGE_UI.1, SIGNAL_EDGE_UI.2
        )
    } else {
        format!(
            "clock or undeclared rate, t(10-90 %) = {} ns nominal, {} ns fast bound, {} ns slow bound",
            SIGNAL_EDGE_CLOCK.0 * 1e9,
            SIGNAL_EDGE_CLOCK.1 * 1e9,
            SIGNAL_EDGE_CLOCK.2 * 1e9
        )
    }
}

/// Differential peak-to-peak swing band, volts: `(nominal, low, high)`.
///
/// Scale. LVDS (TIA/EIA-644-A) specifies a differential output voltage of 247 to 454 mV (350 mV nominal) into 100
/// ohm, and MIPI D-PHY high-speed transmitters 140 to 270 mV (200 mV nominal); both as remembered, to be checked
/// against the standards. Those are differential output voltages, not peak-to-peak values, and multi-gigabit
/// serial standards (PCIe, USB 3) use larger swings, so this band is a rule of thumb for **small-swing
/// differential links only** and **not a published figure**: declare `signal_swing` for anything else.
pub const SIGNAL_SWING_DIFFERENTIAL: (f64, f64, f64) = (0.2, 0.1, 0.4);
/// A single-ended driver's loaded swing as a fraction of its supply: `(nominal, low, high)`. A CMOS output
/// swings close to rail to rail; the 0.8 low bound allows for load and drive strength. Rule of thumb, **no
/// published source cited**.
pub const SIGNAL_SWING_SUPPLY_FRACTION: (f64, f64, f64) = (1.0, 0.8, 1.0);

/// The default swing band, `(nominal, low, high)` volts. A differential pair gets [`SIGNAL_SWING_DIFFERENTIAL`];
/// a single-ended signal needs the driver's supply voltage and has no default without it (`None`).
pub fn signal_swing_band(differential: bool, supply: Option<f64>) -> Option<(f64, f64, f64)> {
    if differential {
        return Some(SIGNAL_SWING_DIFFERENTIAL);
    }
    let supply = supply.filter(|volts| *volts > 0.0)?;
    let (nominal, low, high) = SIGNAL_SWING_SUPPLY_FRACTION;
    Some((nominal * supply, low * supply, high * supply))
}

/// The rule behind [`signal_swing_band`], for `input:` lines.
pub fn signal_swing_rule(differential: bool) -> String {
    if differential {
        format!(
            "small-swing differential link, {} V nominal, {} to {} V (LVDS and D-PHY scale; not a published figure)",
            SIGNAL_SWING_DIFFERENTIAL.0, SIGNAL_SWING_DIFFERENTIAL.1, SIGNAL_SWING_DIFFERENTIAL.2
        )
    } else {
        format!(
            "single-ended CMOS output, {} to {} of the driver's supply",
            SIGNAL_SWING_SUPPLY_FRACTION.1, SIGNAL_SWING_SUPPLY_FRACTION.2
        )
    }
}

// ---------------------------------------------------------------------------
// 7. Return paths (`docs/emi-signal-cable-plan.md`, P2)
// ---------------------------------------------------------------------------

/// The reference coverage window (`k H` each side of a trace) is a rule of thumb.
pub const TAG_RETURN_WINDOW: &str = "rule_of_thumb_return_window";
/// The plane-edge and board-edge distance (`k H`) is a rule of thumb.
pub const TAG_EDGE_DISTANCE: &str = "rule_of_thumb_edge_distance";
/// The return-via distance (a fraction of a wavelength at the knee frequency) is a rule of thumb.
pub const TAG_RETURN_VIA: &str = "rule_of_thumb_return_via_distance";

/// Reference coverage: a trace at height `H` over a plane is checked over `k H` each side of its centre line:
/// `(nominal, low, high)` in units of `H`.
///
/// Derived, not looked up. A thin line current `I` at height `H` over an infinite plane (the method of images: the
/// line and its mirror image) induces the plane current density `J(x) = I H / (pi (x^2 + H^2))` at the lateral
/// distance `x`, the distribution the return current takes at high frequency, where it follows the path of least
/// inductance. The fraction of `I` within `k H` of the line is therefore `(2 / pi) atan(k)`
/// ([`return_window_fraction`]): 50 % at 1 H, 70.5 % at 2 H, 79.5 % at 3 H, 87.4 % at 5 H. The nominal 3 H is the
/// plan's choice of "about four fifths"; the band 2 H to 5 H is the 70 % to 87 % range. The distribution assumes a
/// plane much wider than `H` and a thin trace; a wide trace or a stripline between two planes spreads differently.
/// This derivation, not a citation, is the source here.
pub const RETURN_WINDOW_H: (f64, f64, f64) = (3.0, 2.0, 5.0);

/// The fraction of a thin trace's return current that flows within `k` heights of its centre line over an
/// infinite plane, `(2 / pi) atan(k)` (see [`RETURN_WINDOW_H`]).
pub fn return_window_fraction(k: f64) -> f64 {
    2.0 / std::f64::consts::PI * k.atan()
}

/// Plane-edge and board-edge distance, in units of `H`: `(nominal, low, high)`. The "20 H rule" of EMC design
/// guides (Montrose, *Printed Circuit Board Design Techniques for EMC Compliance*) keeps a trace or a smaller power
/// plane back from the edge of its reference plane by `20 H` so that about 70 % of the fringing flux stays
/// inside; the book is quoted from memory and not checked against the text, so this is a **rule of thumb with no
/// verified citation**. The band 10 H to 100 H brackets the 10 H and 100 H flux-containment figures quoted with it.
pub const EDGE_DISTANCE_H: (f64, f64, f64) = (20.0, 10.0, 100.0);

/// A return via within `lambda / n` of the transition at the knee frequency (`n` nominal, then the two ends of
/// the band). Structures much shorter than a wavelength are electrically small; `lambda / 10` is the usual
/// bound for treating a structure as lumped and `lambda / 20` is the stricter figure used here. **Rule of
/// thumb, no published source cited**: the factor is a screening choice, not a derived limit.
pub const RETURN_VIA_LAMBDA_FRACTION: (f64, f64, f64) = (20.0, 10.0, 40.0);

/// The knee frequency of an edge, `0.35 / t_r` for a 10-90 % rise time (a single-pole bandwidth: the relation
/// P1 uses for its edge default).
pub fn knee_frequency(rise_time_10_90: f64) -> f64 {
    0.35 / rise_time_10_90
}

/// The return-via distance threshold, m: `lambda / n` at the knee frequency of a `rise_time_10_90` edge in a
/// medium of effective relative permittivity `eps_eff`, `lambda = c / (f sqrt(eps_eff))`.
pub fn return_via_threshold(rise_time_10_90: f64, eps_eff: f64, fraction: f64) -> f64 {
    copper_emi::C0 / (knee_frequency(rise_time_10_90) * eps_eff.sqrt() * fraction)
}

/// The rule behind [`RETURN_WINDOW_H`], for `input:` lines.
pub fn return_window_rule() -> String {
    format!(
        "return current taken within {} H each side of a trace ({:.1} % of a thin trace's return current over an infinite plane; derived, band {} H to {} H)",
        RETURN_WINDOW_H.0,
        100.0 * return_window_fraction(RETURN_WINDOW_H.0),
        RETURN_WINDOW_H.1,
        RETURN_WINDOW_H.2
    )
}

/// The rule behind [`EDGE_DISTANCE_H`], for `input:` lines.
pub fn edge_distance_rule() -> String {
    format!(
        "plane and board edges kept {} H from a trace (the 20 H rule of EMC design guides; band {} H to {} H; no verified citation)",
        EDGE_DISTANCE_H.0, EDGE_DISTANCE_H.1, EDGE_DISTANCE_H.2
    )
}

/// The rule behind [`RETURN_VIA_LAMBDA_FRACTION`], for `input:` lines.
pub fn return_via_rule() -> String {
    format!(
        "return via within lambda / {} at the knee frequency 0.35 / t_r (band lambda / {} to lambda / {}; no published source)",
        RETURN_VIA_LAMBDA_FRACTION.0, RETURN_VIA_LAMBDA_FRACTION.1, RETURN_VIA_LAMBDA_FRACTION.2
    )
}

#[cfg(test)]
fn ring(l: f64, c: f64) -> f64 {
    use std::f64::consts::PI;
    1.0 / (2.0 * PI * (l * c).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edge_follows_the_slew_rate() {
        // 24 V at 3 V/ns: 0.8 * 24 / 3 = 6.4 ns.
        assert!((edge_10_90(24.0, 3.0) - 6.4e-9).abs() < 1e-15);
        let (nominal, fast, slow) = default_edge_band(24.0);
        assert!((nominal - 6.4e-9).abs() < 1e-15);
        assert!((fast - 1.92e-9).abs() < 1e-15);
        assert!((slow - 19.2e-9).abs() < 1e-15);
        assert!(fast < nominal && nominal < slow);
    }

    #[test]
    fn the_diode_capacitance_follows_the_rating_and_the_reverse_voltage() {
        let low_voltage = diode_capacitance(4.5, Some(5.0));
        assert!((low_voltage.nominal - 337.5e-12).abs() < 1e-18);
        assert!((low_voltage.low - 225e-12).abs() < 1e-18);
        assert!((low_voltage.high - 450e-12).abs() < 1e-18);
        let high_voltage = diode_capacitance(4.5, Some(12.0));
        assert!((high_voltage.nominal - 112.5e-12).abs() < 1e-18);
        assert!((high_voltage.low - 75e-12).abs() < 1e-18);
        // The band keeps the un-derated value as its high end.
        assert!((high_voltage.high - 450e-12).abs() < 1e-18);
        assert_eq!(diode_capacitance(4.5, None), low_voltage);
        let fet = fet_capacitance();
        assert!(
            (fet.nominal - 250e-12).abs() < 1e-18
                && (fet.low - 100e-12).abs() < 1e-18
                && (fet.high - 500e-12).abs() < 1e-18
        );
        let sum = fet.plus(high_voltage);
        assert!((sum.nominal - 362.5e-12).abs() < 1e-18);
        // A larger capacitance rings lower.
        assert!(ring(10e-9, sum.high) < ring(10e-9, sum.low));
    }

    #[test]
    fn dc_bias_loss_depends_on_the_package() {
        for package in ["0402", "0603", "0805"] {
            assert_eq!(dc_bias_loss(package), 0.5);
        }
        assert_eq!(dc_bias_loss("1206"), 0.35);
        assert_eq!(dc_bias_loss("1210"), 0.25);
        assert!(!nominal_is_derated(3.3) && nominal_is_derated(5.0) && nominal_is_derated(12.0));
    }

    #[test]
    fn the_ripple_band_brackets_the_nominal_and_the_rule_says_where_it_comes_from() {
        assert!(RIPPLE_RATIO.1 < RIPPLE_RATIO.0 && RIPPLE_RATIO.0 < RIPPLE_RATIO.2);
        assert!(ripple_rule().contains("power inductor"));
    }

    #[test]
    fn signal_defaults_are_bands_around_a_nominal_and_carry_rule_tags() {
        assert!(is_rule_tag(TAG_SIGNAL_EDGE) && is_rule_tag(TAG_SIGNAL_SWING));
        // Data signal: fractions of the unit interval of 2.5 Gbps (400 ps).
        let (nominal, fast, slow) = signal_edge_band(Some(2.5e9));
        assert!(fast < nominal && nominal < slow);
        assert!((fast - 0.2 * 400e-12).abs() < 1e-15 && (slow - 0.35 * 400e-12).abs() < 1e-15);
        let (nominal, fast, slow) = signal_edge_band(None);
        assert!(fast < nominal && nominal < slow);
        assert_eq!(signal_edge_band(Some(0.0)), SIGNAL_EDGE_CLOCK);
        let (nominal, low, high) = signal_swing_band(true, None).unwrap();
        assert!(low < nominal && nominal < high);
        // Single-ended: only with a supply.
        assert_eq!(signal_swing_band(false, None), None);
        let (nominal, low, high) = signal_swing_band(false, Some(3.3)).unwrap();
        assert!((nominal - 3.3).abs() < 1e-12 && (low - 0.8 * 3.3).abs() < 1e-12 && (high - 3.3).abs() < 1e-12);
        assert!(signal_edge_rule(true).contains("UI") && signal_edge_rule(false).contains("ns"));
        assert!(signal_swing_rule(true).contains("differential") && signal_swing_rule(false).contains("supply"));
    }

    #[test]
    fn the_return_path_defaults_are_derived_or_say_they_have_no_source() {
        // The share of a thin trace's return current within k heights of it, (2 / pi) atan(k): 50 % at 1 H, 79.5 % at 3 H.
        assert!((return_window_fraction(1.0) - 0.5).abs() < 1e-12);
        assert!((return_window_fraction(3.0) - 0.7952).abs() < 1e-4);
        assert!(
            return_window_fraction(2.0) < return_window_fraction(3.0)
                && return_window_fraction(3.0) < return_window_fraction(5.0)
        );
        assert!(RETURN_WINDOW_H.1 < RETURN_WINDOW_H.0 && RETURN_WINDOW_H.0 < RETURN_WINDOW_H.2);
        assert!(EDGE_DISTANCE_H.1 < EDGE_DISTANCE_H.0 && EDGE_DISTANCE_H.0 < EDGE_DISTANCE_H.2);
        // lambda / 20 at the knee of a 1 ns edge in eps = 4: f = 350 MHz, lambda = 299792458 / (350e6 * 2) = 0.42827 m,
        // a twentieth is 21.41 mm.
        assert!((knee_frequency(1e-9) - 350e6).abs() < 1.0);
        let threshold = return_via_threshold(1e-9, 4.0, RETURN_VIA_LAMBDA_FRACTION.0);
        assert!((threshold - 0.021_413).abs() < 1e-5, "{threshold}");
        // A ten times faster edge shortens it by ten.
        assert!((return_via_threshold(1e-10, 4.0, 20.0) * 10.0 - threshold).abs() < 1e-12);
        for tag in [TAG_RETURN_WINDOW, TAG_EDGE_DISTANCE, TAG_RETURN_VIA] {
            assert!(is_rule_tag(tag), "{tag}");
        }
        assert!(return_window_rule().contains("derived") && edge_distance_rule().contains("no verified citation"));
        assert!(return_via_rule().contains("no published source"));
    }

    #[test]
    fn rule_tags_are_recognised() {
        assert!(
            is_rule_tag(TAG_EDGE)
                && is_rule_tag(TAG_CAPACITANCE)
                && is_rule_tag(TAG_DC_BIAS)
                && is_rule_tag(TAG_RIPPLE)
        );
        assert!(
            !is_rule_tag(TAG_EDGE_FROM_SLEW)
                && !is_rule_tag(TAG_PARASITICS)
                && !is_rule_tag(TAG_CURRENT_EDGE_FROM_VOLTAGE)
        );
    }
}

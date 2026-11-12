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

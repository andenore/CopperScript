//! The numbers of a signal source with the rule-of-thumb defaults applied.
//!
//! `copper_lang::emi::EmiModel::signals` carries only what the driver declares. A phase that needs an edge
//! time or a swing takes it from here: a declared value is used as is, a missing one comes from
//! [`super::defaults`] with its band, its `rule_of_thumb_*` tag and the rule text, never silently.

use copper_lang::emi_signals::{SignalKind, SignalNumber, SignalSource};

use super::defaults;

/// Where a number comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Basis {
    /// Declared on the named driving pin (`COMPONENT.PIN`).
    Declared(String),
    /// A rule of thumb from [`super::defaults`].
    Default,
}

/// One number of a signal in SI units, with where it comes from.
#[derive(Clone, Debug, PartialEq)]
pub struct Number {
    pub value: f64,
    pub basis: Basis,
    /// `(low, high)` of the default band; `None` for a declared value.
    pub band: Option<(f64, f64)>,
    /// The `rule_of_thumb_*` tag of a default.
    pub tag: Option<&'static str>,
    /// The rule behind a default, for `input:` lines.
    pub rule: Option<String>,
}

impl Number {
    fn declared(number: &SignalNumber, value: f64) -> Number {
        Number { value, basis: Basis::Declared(number.from.to_string()), band: None, tag: None, rule: None }
    }

    /// A default `(nominal, a, b)` with its band, tag and rule.
    fn default(band: (f64, f64, f64), tag: &'static str, rule: String) -> Number {
        let (nominal, a, b) = band;
        Number {
            value: nominal,
            basis: Basis::Default,
            band: Some((a.min(b), a.max(b))),
            tag: Some(tag),
            rule: Some(rule),
        }
    }

    pub fn is_default(&self) -> bool {
        self.basis == Basis::Default
    }
}

/// A signal source with every number it needs resolved; `None` where nothing is declared and no default exists.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedSignal {
    pub net: String,
    pub differential: bool,
    pub data_rate: Option<f64>,
    pub clock_frequency: Option<f64>,
    /// 10-90 % times, as declared (or defaulted). The spectrum functions of `copper_emi::signal` take the
    /// 0-100 % linear ramp: use [`ResolvedSignal::rise_ramp`] and [`ResolvedSignal::fall_ramp`].
    pub rise_time: Number,
    pub fall_time: Number,
    /// The loaded swing at the driver pins (differential: peak-to-peak); `None` for a single-ended signal with
    /// no declared swing and no known supply.
    pub swing: Option<Number>,
    /// Recorded, not modelled.
    pub spread_spectrum: Option<f64>,
}

impl ResolvedSignal {
    /// The 0-100 % linear ramp of the rising edge: the 10-90 % time divided by 0.8, the conversion the conducted
    /// model applies to a switch's edge times ([`defaults::RAMP_10_90`]).
    pub fn rise_ramp(&self) -> f64 {
        self.rise_time.value / defaults::RAMP_10_90
    }

    /// The 0-100 % linear ramp of the falling edge.
    pub fn fall_ramp(&self) -> f64 {
        self.fall_time.value / defaults::RAMP_10_90
    }
}

fn si(number: &SignalNumber) -> f64 {
    number.value.base_value().decimal_text().parse().unwrap_or(f64::NAN)
}

/// Resolve `source`. `supply_voltage` is the driver I/O supply when known; only a single-ended signal with
/// no declared swing uses it.
pub fn resolve(source: &SignalSource, supply_voltage: Option<f64>) -> ResolvedSignal {
    let data_rate = source.data_rate.as_ref().map(si);
    let differential = source.kind == SignalKind::Differential;
    let edge_default = || {
        Number::default(
            defaults::signal_edge_band(data_rate),
            defaults::TAG_SIGNAL_EDGE,
            defaults::signal_edge_rule(data_rate.is_some()),
        )
    };
    let rise_time = source.rise_time.as_ref().map_or_else(edge_default, |number| Number::declared(number, si(number)));
    let fall_time = source.fall_time.as_ref().map_or_else(edge_default, |number| Number::declared(number, si(number)));
    let swing = match &source.swing {
        Some(number) => Some(Number::declared(number, si(number))),
        None => defaults::signal_swing_band(differential, supply_voltage)
            .map(|band| Number::default(band, defaults::TAG_SIGNAL_SWING, defaults::signal_swing_rule(differential))),
    };
    ResolvedSignal {
        net: source.net.clone(),
        differential,
        data_rate,
        clock_frequency: source.clock_frequency.as_ref().map(si),
        rise_time,
        fall_time,
        swing,
        spread_spectrum: source.spread_spectrum.as_ref().map(si),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_lang::model::Endpoint;
    use copper_units::{Decimal, Dimension, Quantity};

    fn number(dimension: Dimension, value: &str, unit: &str) -> SignalNumber {
        SignalNumber {
            value: Quantity::of(dimension, &Decimal::parse(value).unwrap(), unit).unwrap(),
            from: Endpoint::new("U1", "TXP"),
        }
    }

    fn source(kind: SignalKind) -> SignalSource {
        SignalSource {
            net: "LANE_P".to_owned(),
            kind,
            partner: (kind == SignalKind::Differential).then(|| "LANE_N".to_owned()),
            pins: Vec::new(),
            fast_edge: false,
            data_rate: None,
            clock_frequency: None,
            rise_time: None,
            fall_time: None,
            swing: None,
            spread_spectrum: None,
            open_circuit_swing: None,
            output_impedance: None,
            members: Vec::new(),
        }
    }

    #[test]
    fn declared_numbers_win_and_carry_their_pin() {
        let mut declared = source(SignalKind::Differential);
        declared.data_rate = Some(number(Dimension::DataRate, "2.5", "Gbps"));
        declared.rise_time = Some(number(Dimension::Time, "80", "ns"));
        declared.fall_time = declared.rise_time.clone();
        declared.swing = Some(number(Dimension::Voltage, "300", "mV"));
        let resolved = resolve(&declared, None);
        assert_eq!(resolved.data_rate, Some(2.5e9));
        assert_eq!(resolved.rise_time.basis, Basis::Declared("U1.TXP".to_owned()));
        assert!((resolved.rise_time.value - 80e-9).abs() < 1e-18);
        assert!(resolved.rise_time.band.is_none() && resolved.rise_time.tag.is_none());
        let swing = resolved.swing.unwrap();
        assert!((swing.value - 0.3).abs() < 1e-12 && !swing.is_default());
    }

    #[test]
    fn a_missing_edge_and_swing_get_a_banded_tagged_default() {
        let mut declared = source(SignalKind::Differential);
        declared.data_rate = Some(number(Dimension::DataRate, "2.5", "Gbps"));
        let resolved = resolve(&declared, None);
        assert!(resolved.rise_time.is_default() && resolved.fall_time.is_default());
        let (low, high) = resolved.rise_time.band.unwrap();
        assert!((low - 0.2 * 400e-12).abs() < 1e-15 && (high - 0.35 * 400e-12).abs() < 1e-15);
        assert!(low < resolved.rise_time.value && resolved.rise_time.value < high);
        assert_eq!(resolved.rise_time.tag, Some("rule_of_thumb_signal_edge"));
        assert!(resolved.rise_time.rule.as_deref().unwrap().contains("UI"));
        let swing = resolved.swing.unwrap();
        assert_eq!(swing.tag, Some("rule_of_thumb_signal_swing"));
        assert_eq!(swing.band, Some((0.1, 0.4)));
        // A clock without a rate gets the absolute band; a single-ended signal needs a supply for its swing.
        let mut clock = source(SignalKind::SingleEnded);
        clock.clock_frequency = Some(number(Dimension::Frequency, "24", "MHz"));
        let resolved = resolve(&clock, None);
        assert_eq!(resolved.clock_frequency, Some(24e6));
        assert!((resolved.rise_time.value - 1e-9).abs() < 1e-18);
        assert!(resolved.swing.is_none());
        let with_supply = resolve(&clock, Some(1.8)).swing.unwrap();
        assert!((with_supply.value - 1.8).abs() < 1e-12 && with_supply.is_default());
    }
}

//! Named part libraries for `use library "NAME";`.
//!
//! The Python engine resolves these through installed entry points; the only
//! one CopperScript ships is the demonstration library (`tiny`, also
//! registered as `standard`) from `copperscript.library_data`.

use copper_units::{Decimal, Dimension, Quantity};
use indexmap::IndexMap;

use crate::model::*;

/// The parts of library `name`, or `None` when no such library is installed.
pub fn library(name: &str) -> Option<IndexMap<String, PartDefinition>> {
    match name {
        "tiny" | "standard" => Some(tiny_library()),
        _ => None,
    }
}

/// `copperscript.library_data._pin`; voltages are Python float literals, so
/// `12.0` keeps its decimal exponent like `Decimal(str(12.0))`.
fn pin(
    name: &str,
    number: &str,
    pin_type: PinType,
    minimum: Option<&str>,
    maximum: Option<&str>,
) -> PackagePinDefinition {
    let domain = if name == "GND" || name == "VSS" {
        SignalDomain::Ground
    } else if matches!(pin_type, PinType::PowerIn | PinType::PowerOut) {
        SignalDomain::Power
    } else if pin_type != PinType::Passive {
        SignalDomain::Digital
    } else {
        SignalDomain::Analog
    };
    let direction = match pin_type {
        PinType::Passive => Direction::Passive,
        PinType::Input | PinType::PowerIn => Direction::Input,
        PinType::Output | PinType::PowerOut => Direction::Output,
        PinType::Bidirectional | PinType::OpenDrain => Direction::Bidirectional,
    };
    let drive: &[DriveMode] = match pin_type {
        PinType::Output => &[DriveMode::PushPull],
        PinType::Bidirectional => &[DriveMode::PushPull, DriveMode::OpenDrain],
        PinType::OpenDrain => &[DriveMode::OpenDrain],
        _ => &[],
    };
    let volts = |text: &str| {
        Bound::Fixed(Quantity::of(Dimension::Voltage, &Decimal::parse(text).expect("literal"), "V").expect("volts"))
    };
    let mut profile = ElectricalProfile::simple(&[domain], &[direction], drive);
    if minimum.is_some() || maximum.is_some() {
        profile.voltage = Some(QuantityRange::new(minimum.map(volts), maximum.map(volts), "operating"));
    }
    PackagePinDefinition {
        name: name.to_owned(),
        number: number.to_owned(),
        profile: Some(profile),
        bonds: Vec::new(),
        connection_policy: ConnectionPolicy::Normal,
        required_net_traits: Default::default(),
        parallel_power_group: None,
    }
}

fn part(
    name: &str,
    category: &str,
    manufacturer: Option<&str>,
    pins: Vec<PackagePinDefinition>,
    footprints: &[&str],
) -> PartDefinition {
    let mut definition = PartDefinition::new(name, pins.into_iter().map(|pin| (pin.name.clone(), pin)).collect());
    definition.category = category.to_owned();
    definition.manufacturer = manufacturer.map(str::to_owned);
    definition.footprints = footprints.iter().map(|footprint| (*footprint).to_owned()).collect();
    definition
}

/// `copperscript.library_data.tiny_library`.
pub fn tiny_library() -> IndexMap<String, PartDefinition> {
    use PinType::*;
    let two = |a: &str, b: &str| {
        vec![
            pin(a, if a == "A" { "1" } else { a }, Passive, None, None),
            pin(b, if b == "K" { "2" } else { b }, Passive, None, None),
        ]
    };
    let parts = vec![
        part("RESISTOR", "passive.resistor", None, two("1", "2"), &["0402", "0603"]),
        part("CAPACITOR", "passive.capacitor", None, two("1", "2"), &["0402", "0603"]),
        part("INDUCTOR", "passive.inductor", None, two("1", "2"), &["L_4x4mm"]),
        part("SCHOTTKY_DIODE", "semiconductor.diode", None, two("A", "K"), &["SOD-123"]),
        part(
            "VOLTAGE_SOURCE",
            "power.source",
            None,
            vec![pin("OUT", "1", PowerOut, None, None), pin("GND", "2", Passive, None, None)],
            &[],
        ),
        part(
            "REGULATOR_3V3",
            "power.regulator",
            None,
            vec![
                pin("IN", "1", PowerIn, Some("3.6"), Some("12.0")),
                pin("GND", "2", PowerIn, None, None),
                pin("OUT", "3", PowerOut, None, None),
            ],
            &["SOT-23-3"],
        ),
        part(
            "BUCK_REGULATOR",
            "power.regulator.buck",
            None,
            vec![
                pin("VIN", "1", PowerIn, Some("4.5"), Some("18.0")),
                pin("GND", "2", PowerIn, None, None),
                pin("SW", "3", PowerOut, Some("0.0"), Some("18.0")),
                pin("FB", "4", Input, Some("0.0"), Some("3.6")),
                pin("EN", "5", Input, Some("0.0"), Some("18.0")),
            ],
            &["SOT-23-5"],
        ),
        part(
            "STM32_LIKE",
            "semiconductor.mcu",
            Some("Example Semiconductor"),
            vec![
                pin("VDD", "1", PowerIn, Some("1.8"), Some("3.6")),
                pin("VSS", "2", PowerIn, None, None),
                pin("PB6", "3", OpenDrain, Some("0.0"), Some("3.6")),
                pin("PB7", "4", OpenDrain, Some("0.0"), Some("3.6")),
                pin("PA0", "5", Bidirectional, Some("0.0"), Some("3.6")),
                pin("TX", "6", Output, Some("0.0"), Some("3.6")),
            ],
            &["LQFP-32"],
        ),
        part(
            "BME280_LIKE",
            "sensor.environmental",
            Some("Example Sensors"),
            vec![
                pin("VDD", "1", PowerIn, Some("1.71"), Some("3.6")),
                pin("GND", "2", PowerIn, None, None),
                pin("SCL", "3", OpenDrain, Some("0.0"), Some("3.6")),
                pin("SDA", "4", OpenDrain, Some("0.0"), Some("3.6")),
            ],
            &["LGA-8"],
        ),
    ];
    parts.into_iter().map(|part| (part.name.clone(), part)).collect()
}

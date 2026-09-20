"""A small MCU + environmental sensor board that passes v0.1 ERC."""

from pcbir import (
    Board,
    ComponentInstance,
    Constraint,
    ConstraintKind,
    Interface,
    Net,
    Supply,
    ep,
    kiloohms,
    millimeters,
    nanofarads,
    volts,
)
from pcbir.library import tiny_library


board = Board(
    name="ValidSensorBoard",
    library=tiny_library(),
    components=(
        ComponentInstance("PS1", "VOLTAGE_SOURCE"),
        ComponentInstance("U1", "REGULATOR_3V3", footprint="SOT-23-3"),
        ComponentInstance("U2", "STM32_LIKE", footprint="LQFP-32"),
        ComponentInstance("U3", "BME280_LIKE", footprint="LGA-8"),
        ComponentInstance("R1", "RESISTOR", value=kiloohms(4.7), footprint="0402"),
        ComponentInstance("R2", "RESISTOR", value=kiloohms(4.7), footprint="0402"),
        ComponentInstance("C1", "CAPACITOR", value=nanofarads(100), footprint="0402"),
    ),
    nets=(
        Net("VBUS", (ep("PS1.OUT"), ep("U1.IN"))),
        Net(
            "GND",
            (
                ep("PS1.GND"),
                ep("U1.GND"),
                ep("U2.VSS"),
                ep("U3.GND"),
                ep("C1.2"),
            ),
        ),
        Net(
            "V3V3",
            (
                ep("U1.OUT"),
                ep("U2.VDD"),
                ep("U3.VDD"),
                ep("R1.1"),
                ep("R2.1"),
                ep("C1.1"),
            ),
        ),
        Net("I2C_SDA", (ep("U2.PB7"), ep("U3.SDA"), ep("R1.2"))),
        Net("I2C_SCL", (ep("U2.PB6"), ep("U3.SCL"), ep("R2.2"))),
    ),
    supplies=(
        Supply("VBUS", volts(5), "VBUS", source=ep("PS1.OUT")),
        Supply("V3V3", volts(3.3), "V3V3", source=ep("U1.OUT")),
        # Ground is modeled as an externally established reference node.
        Supply("GND", volts(0), "GND", externally_driven=True),
    ),
    interfaces=(
        Interface(
            name="SENSOR_I2C",
            type_name="std.i2c",
            signals={"sda": "I2C_SDA", "scl": "I2C_SCL"},
            bindings={
                "U2": {"sda": "PB7", "scl": "PB6"},
                "U3": {"sda": "SDA", "scl": "SCL"},
            },
            pullup_supply="V3V3",
        ),
    ),
    # Physical intent is representable but deliberately not acted on in v0.1.
    constraints=(
        Constraint(
            ConstraintKind.MAX_DISTANCE,
            targets=("C1", "U2.VDD"),
            parameters={"distance": millimeters(3)},
        ),
    ),
)

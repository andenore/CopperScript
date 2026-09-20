"""An intentionally broken board used to demonstrate ERC diagnostics."""

from pcbir import (
    Board,
    ComponentInstance,
    Interface,
    InterfaceKind,
    Net,
    Supply,
    ep,
    volts,
)
from pcbir.library import tiny_library


board = Board(
    name="BrokenSensorBoard",
    library=tiny_library(),
    components=(
        ComponentInstance("PS1", "VOLTAGE_SOURCE"),
        ComponentInstance("REG1", "REGULATOR_3V3"),
        ComponentInstance("U1", "STM32_LIKE"),
        ComponentInstance("U1", "BME280_LIKE"),  # duplicate reference
        ComponentInstance("U2", "BME280_LIKE"),
        ComponentInstance("X1", "DOES_NOT_EXIST"),
    ),
    nets=(
        # Two outputs drive one net, and 5 V exceeds U1.VDD's 3.6 V maximum.
        Net("BAD_5V", (ep("PS1.OUT"), ep("REG1.OUT"), ep("U1.VDD"))),
        Net("GND", (ep("PS1.GND"), ep("REG1.GND"), ep("U1.VSS"), ep("U2.GND"))),
        # U2.VDD has no POWER_OUT or declared supply on its net.
        Net("FLOATING_POWER", (ep("U2.VDD"),)),
        Net("I2C_SDA", (ep("U1.PB7"), ep("U2.SDA"))),
        Net("I2C_SCL", (ep("U1.PB6"), ep("U2.SCL"))),
        # The same physical pin is assigned to a second net.
        Net("OTHER", (ep("U1.PB7"), ep("MISSING.PIN"))),
    ),
    supplies=(
        Supply("BAD_5V", volts(5), "BAD_5V", source=ep("PS1.OUT")),
        Supply("GND", volts(0), "GND", externally_driven=True),
    ),
    interfaces=(
        Interface(
            name="BROKEN_I2C",
            kind=InterfaceKind.I2C,
            signals={"sda": "I2C_SDA", "scl": "I2C_SCL"},
            bindings={
                "U1": {"sda": "PB7", "scl": "PB6"},
                "U2": {"sda": "SDA", "scl": "SCL"},
            },
            # This exists, but neither signal has a pull-up resistor to it.
            pullup_supply="BAD_5V",
        ),
    ),
)

"""A deliberately tiny component library for examples and tests."""

from __future__ import annotations

from .model import (
    Direction,
    DriveMode,
    ElectricalProfile,
    PackagePinDefinition,
    PartDefinition,
    PinType,
    QuantityRange,
    SignalDomain,
)
from .quantities import volts


def _pin(
    name: str,
    number: str,
    pin_type: PinType,
    voltage_min: float | None = None,
    voltage_max: float | None = None,
) -> PackagePinDefinition:
    domain = (
        SignalDomain.GROUND
        if name in {"GND", "VSS"}
        else SignalDomain.POWER
        if pin_type in {PinType.POWER_IN, PinType.POWER_OUT}
        else SignalDomain.DIGITAL
        if pin_type is not PinType.PASSIVE
        else SignalDomain.ANALOG
    )
    direction_map = {
        PinType.PASSIVE: {Direction.PASSIVE},
        PinType.INPUT: {Direction.INPUT},
        PinType.OUTPUT: {Direction.OUTPUT},
        PinType.BIDIRECTIONAL: {Direction.BIDIRECTIONAL},
        PinType.OPEN_DRAIN: {Direction.BIDIRECTIONAL},
        PinType.POWER_IN: {Direction.INPUT},
        PinType.POWER_OUT: {Direction.OUTPUT},
    }
    drive_map = {
        PinType.OUTPUT: {DriveMode.PUSH_PULL},
        PinType.BIDIRECTIONAL: {DriveMode.PUSH_PULL, DriveMode.OPEN_DRAIN},
        PinType.OPEN_DRAIN: {DriveMode.OPEN_DRAIN},
    }
    limits = (
        QuantityRange(
            minimum=volts(voltage_min) if voltage_min is not None else None,
            maximum=volts(voltage_max) if voltage_max is not None else None,
        )
        if voltage_min is not None or voltage_max is not None
        else None
    )
    return PackagePinDefinition(
        name=name,
        number=number,
        profile=ElectricalProfile(
            domains=frozenset({domain}),
            directions=frozenset(direction_map[pin_type]),
            drive_modes=frozenset(drive_map.get(pin_type, set())),
            voltage=limits,
        ),
    )


def tiny_library() -> dict[str, PartDefinition]:
    """Return fresh definitions for the v0.1 demonstration library."""

    parts = (
        PartDefinition(
            name="RESISTOR",
            category="passive.resistor",
            pins={
                "1": _pin("1", "1", PinType.PASSIVE),
                "2": _pin("2", "2", PinType.PASSIVE),
            },
            footprints=("0402", "0603"),
        ),
        PartDefinition(
            name="CAPACITOR",
            category="passive.capacitor",
            pins={
                "1": _pin("1", "1", PinType.PASSIVE),
                "2": _pin("2", "2", PinType.PASSIVE),
            },
            footprints=("0402", "0603"),
        ),
        PartDefinition(
            name="INDUCTOR",
            category="passive.inductor",
            pins={
                "1": _pin("1", "1", PinType.PASSIVE),
                "2": _pin("2", "2", PinType.PASSIVE),
            },
            footprints=("L_4x4mm",),
        ),
        PartDefinition(
            name="SCHOTTKY_DIODE",
            category="semiconductor.diode",
            pins={
                "A": _pin("A", "1", PinType.PASSIVE),
                "K": _pin("K", "2", PinType.PASSIVE),
            },
            footprints=("SOD-123",),
        ),
        PartDefinition(
            name="VOLTAGE_SOURCE",
            category="power.source",
            pins={
                "OUT": _pin("OUT", "1", PinType.POWER_OUT),
                "GND": _pin("GND", "2", PinType.PASSIVE),
            },
        ),
        PartDefinition(
            name="REGULATOR_3V3",
            category="power.regulator",
            pins={
                "IN": _pin("IN", "1", PinType.POWER_IN, 3.6, 12.0),
                "GND": _pin("GND", "2", PinType.POWER_IN),
                "OUT": _pin("OUT", "3", PinType.POWER_OUT),
            },
            footprints=("SOT-23-3",),
        ),
        PartDefinition(
            name="BUCK_REGULATOR",
            category="power.regulator.buck",
            pins={
                "VIN": _pin("VIN", "1", PinType.POWER_IN, 4.5, 18.0),
                "GND": _pin("GND", "2", PinType.POWER_IN),
                "SW": _pin("SW", "3", PinType.POWER_OUT, 0.0, 18.0),
                "FB": _pin("FB", "4", PinType.INPUT, 0.0, 3.6),
                "EN": _pin("EN", "5", PinType.INPUT, 0.0, 18.0),
            },
            footprints=("SOT-23-5",),
        ),
        PartDefinition(
            name="STM32_LIKE",
            category="semiconductor.mcu",
            manufacturer="Example Semiconductor",
            pins={
                "VDD": _pin("VDD", "1", PinType.POWER_IN, 1.8, 3.6),
                "VSS": _pin("VSS", "2", PinType.POWER_IN),
                "PB6": _pin("PB6", "3", PinType.OPEN_DRAIN, 0.0, 3.6),
                "PB7": _pin("PB7", "4", PinType.OPEN_DRAIN, 0.0, 3.6),
                "PA0": _pin("PA0", "5", PinType.BIDIRECTIONAL, 0.0, 3.6),
                "TX": _pin("TX", "6", PinType.OUTPUT, 0.0, 3.6),
            },
            footprints=("LQFP-32",),
        ),
        PartDefinition(
            name="BME280_LIKE",
            category="sensor.environmental",
            manufacturer="Example Sensors",
            pins={
                "VDD": _pin("VDD", "1", PinType.POWER_IN, 1.71, 3.6),
                "GND": _pin("GND", "2", PinType.POWER_IN),
                "SCL": _pin("SCL", "3", PinType.OPEN_DRAIN, 0.0, 3.6),
                "SDA": _pin("SDA", "4", PinType.OPEN_DRAIN, 0.0, 3.6),
            },
            footprints=("LGA-8",),
        ),
    )
    return {part.name: part for part in parts}

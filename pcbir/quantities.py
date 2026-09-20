"""Small, dependency-free typed quantities used by the PCB IR.

Values are stored in SI base units.  The display unit is retained only to make
diagnostics pleasant; equality and ordering use the normalized value.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from typing import ClassVar, Self


Number = int | float | str | Decimal


def _decimal(value: Number) -> Decimal:
    # Converting a float through str avoids importing its binary rounding noise.
    return value if isinstance(value, Decimal) else Decimal(str(value))


@dataclass(frozen=True, slots=True)
class Quantity:
    """A normalized, dimension-specific quantity.

    Subclasses define the legal units and their scale relative to the SI base
    unit.  Cross-dimension comparison is intentionally rejected.
    """

    base_value: Decimal
    display_unit: str

    UNITS: ClassVar[dict[str, Decimal]] = {}

    @classmethod
    def of(cls, value: Number, unit: str) -> Self:
        try:
            scale = cls.UNITS[unit]
        except KeyError as exc:
            allowed = ", ".join(cls.UNITS)
            raise ValueError(f"unsupported {cls.__name__} unit {unit!r}; use {allowed}") from exc
        return cls(_decimal(value) * scale, unit)

    @property
    def value(self) -> Decimal:
        """Value expressed in ``display_unit``."""

        return self.base_value / self.UNITS[self.display_unit]

    def in_unit(self, unit: str) -> Decimal:
        try:
            return self.base_value / self.UNITS[unit]
        except KeyError as exc:
            raise ValueError(f"unsupported {self.__class__.__name__} unit {unit!r}") from exc

    def _require_same_type(self, other: object) -> "Quantity":
        if type(self) is not type(other):
            raise TypeError(
                f"cannot compare {self.__class__.__name__} and "
                f"{other.__class__.__name__ if isinstance(other, Quantity) else type(other).__name__}"
            )
        return other

    def __lt__(self, other: object) -> bool:
        return self.base_value < self._require_same_type(other).base_value

    def __le__(self, other: object) -> bool:
        return self.base_value <= self._require_same_type(other).base_value

    def __str__(self) -> str:
        return f"{self.value.normalize()} {self.display_unit}"


@dataclass(frozen=True, slots=True)
class Voltage(Quantity):
    UNITS: ClassVar[dict[str, Decimal]] = {
        "V": Decimal("1"),
        "mV": Decimal("0.001"),
    }


@dataclass(frozen=True, slots=True)
class Resistance(Quantity):
    UNITS: ClassVar[dict[str, Decimal]] = {
        "ohm": Decimal("1"),
        "kohm": Decimal("1000"),
        "Mohm": Decimal("1000000"),
    }


@dataclass(frozen=True, slots=True)
class Capacitance(Quantity):
    UNITS: ClassVar[dict[str, Decimal]] = {
        "F": Decimal("1"),
        "uF": Decimal("0.000001"),
        "nF": Decimal("0.000000001"),
        "pF": Decimal("0.000000000001"),
    }


@dataclass(frozen=True, slots=True)
class Inductance(Quantity):
    UNITS: ClassVar[dict[str, Decimal]] = {
        "H": Decimal("1"),
        "mH": Decimal("0.001"),
        "uH": Decimal("0.000001"),
        "nH": Decimal("0.000000001"),
    }


@dataclass(frozen=True, slots=True)
class Length(Quantity):
    UNITS: ClassVar[dict[str, Decimal]] = {
        "m": Decimal("1"),
        "mm": Decimal("0.001"),
        "um": Decimal("0.000001"),
    }


def volts(value: Number) -> Voltage:
    return Voltage.of(value, "V")


def millivolts(value: Number) -> Voltage:
    return Voltage.of(value, "mV")


def ohms(value: Number) -> Resistance:
    return Resistance.of(value, "ohm")


def kiloohms(value: Number) -> Resistance:
    return Resistance.of(value, "kohm")


def nanofarads(value: Number) -> Capacitance:
    return Capacitance.of(value, "nF")


def microfarads(value: Number) -> Capacitance:
    return Capacitance.of(value, "uF")


def microhenries(value: Number) -> Inductance:
    return Inductance.of(value, "uH")


def millimeters(value: Number) -> Length:
    return Length.of(value, "mm")

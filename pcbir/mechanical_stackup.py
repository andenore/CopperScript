"""Declared board stack-up: source declarations and lowering (D-PHY plan L1).

A ``stackup`` block inside ``mechanical`` lists copper and dielectric layers
from top to bottom. It lowers to ``Stackup.physical_layers``; the selected
copper-layer count (``--layers``) must name exactly the declared copper layers.
The declaration is fabrication intent and screening input, never a substitute
for the fabricator's impedance or stack-up qualification.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from .physical import CopperLayer, Stackup, StackupLayer, StackupLayerKind
from .quantities import Length
from .syntax import CopperScriptError, RawQuantity, Scalar, SourceLocation


@dataclass(frozen=True, slots=True)
class StackupLayerDecl:
    location: SourceLocation
    kind: str
    name: str
    parameters: dict[str, Scalar]


@dataclass(frozen=True, slots=True)
class StackupDecl:
    location: SourceLocation
    layers: tuple[StackupLayerDecl, ...]


@dataclass(frozen=True, slots=True)
class MechanicalStackup:
    """Validated top-to-bottom physical layers declared by the source."""

    layers: tuple[StackupLayer, ...]
    location: SourceLocation

    @property
    def copper_layers(self) -> tuple[CopperLayer, ...]:
        return tuple(layer.copper_layer for layer in self.layers
                     if layer.copper_layer is not None)

    @property
    def thickness_nm(self) -> int:
        return sum(layer.thickness_nm for layer in self.layers)

    def to_stackup(self, selected: tuple[CopperLayer, ...]) -> Stackup:
        """Return the physical stack-up for the selected copper layers.

        The selected layers come from the physicalizer options (``--layers``);
        a mismatch is an error rather than a silent truncation or extension.
        """
        declared = self.copper_layers
        if declared != tuple(selected):
            raise ValueError(
                f"{self.location}: stackup declares {len(declared)} copper layers "
                f"({', '.join(layer.value for layer in declared)}) but the selected stack has "
                f"{len(selected)} ({', '.join(layer.value for layer in selected)}); "
                f"select --layers {len(declared)}"
            )
        return Stackup(copper_layers=declared, thickness_nm=self.thickness_nm,
                       physical_layers=self.layers)


_COPPER_PROPERTIES = {"thickness"}
_DIELECTRIC_PROPERTIES = {"thickness", "er", "loss_tangent", "material", "type"}


def lower_stackup(declaration: StackupDecl) -> MechanicalStackup:
    """Validate one ``stackup`` block and lower it to physical layers."""

    if not declaration.layers:
        raise CopperScriptError("MEC006", "stackup requires at least F.Cu and B.Cu copper layers",
                                declaration.location)
    names: set[str] = set()
    layers: list[StackupLayer] = []
    for index, item in enumerate(declaration.layers):
        def error(message: str) -> None:
            raise CopperScriptError("MEC006", message, item.location)

        expected = "copper" if index % 2 == 0 else "dielectric"
        if item.kind != expected:
            error(f"stackup layers must alternate copper and dielectric from top to bottom; "
                  f"expected {expected}, found {item.kind} {item.name!r}")
        if item.name in names:
            error(f"duplicate stackup layer {item.name!r}")
        names.add(item.name)
        allowed = _COPPER_PROPERTIES if item.kind == "copper" else _DIELECTRIC_PROPERTIES
        unknown = sorted(set(item.parameters) - allowed)
        if unknown:
            error(f"unknown {item.kind} stackup property {unknown[0]!r}")
        required = {"thickness"} if item.kind == "copper" else {"thickness", "er"}
        missing = sorted(required - set(item.parameters))
        if missing:
            error(f"{item.kind} layer {item.name!r} requires {missing[0]!r}")
        try:
            thickness = _length_nm(item.parameters["thickness"])
        except ValueError as exc:
            error(f"{item.kind} layer {item.name!r} thickness {exc}")
        if item.kind == "copper":
            try:
                copper = CopperLayer(item.name)
            except ValueError:
                error(f"unknown copper layer {item.name!r}; expected F.Cu, In1.Cu ... or B.Cu")
            layers.append(StackupLayer(item.name, StackupLayerKind.COPPER, thickness, copper))
            continue
        er = _positive_number(item.parameters["er"], "er", item, error)
        if er < 1:
            error(f"dielectric layer {item.name!r} er must be at least 1 (relative permittivity)")
        loss = (_positive_number(item.parameters["loss_tangent"], "loss_tangent", item, error)
                if "loss_tangent" in item.parameters else None)
        material = item.parameters.get("material")
        if material is not None and (not isinstance(material, str) or not material.strip()):
            error(f"dielectric layer {item.name!r} material must be a nonempty name")
        construction = item.parameters.get("type")
        if construction is not None and construction not in {"core", "prepreg"}:
            error(f"dielectric layer {item.name!r} type must be core or prepreg")
        layers.append(StackupLayer(item.name, StackupLayerKind.DIELECTRIC, thickness,
                                   material=material, relative_permittivity=er,
                                   loss_tangent=loss, dielectric_type=construction))
    if layers[-1].kind is not StackupLayerKind.COPPER:
        raise CopperScriptError("MEC006", "stackup must end with a copper layer",
                                declaration.layers[-1].location)
    copper = tuple(layer.copper_layer for layer in layers if layer.copper_layer is not None)
    internal = tuple(layer for layer in CopperLayer
                     if layer not in {CopperLayer.FRONT, CopperLayer.BACK})
    expected_order = (CopperLayer.FRONT, *internal[: max(len(copper) - 2, 0)], CopperLayer.BACK)
    if len(copper) < 2 or copper != expected_order:
        raise CopperScriptError(
            "MEC006",
            "stackup copper layers must be F.Cu, In1.Cu, In2.Cu ... B.Cu in order from top to bottom",
            declaration.location,
        )
    return MechanicalStackup(tuple(layers), declaration.location)


def _length_nm(value: object) -> int:
    if not isinstance(value, RawQuantity):
        raise ValueError("must be a typed length")
    try:
        quantity = Length.of(value.value, value.unit)
    except (ValueError, TypeError) as exc:
        raise ValueError(f"must be a length ({exc})") from exc
    nm = quantity.base_value * Decimal(1_000_000_000)
    if not nm.is_finite() or nm != nm.to_integral_value():
        raise ValueError("must be an exact integer number of nanometres")
    if nm <= 0:
        raise ValueError("must be positive")
    return int(nm)


def _positive_number(value: object, name: str, item: StackupLayerDecl, error) -> Decimal:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        error(f"dielectric layer {item.name!r} {name} must be a unitless number")
    number = Decimal(str(value))
    if not number.is_finite() or number <= 0:
        error(f"dielectric layer {item.name!r} {name} must be positive")
    return number

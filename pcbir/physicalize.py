"""Prototype electrical-to-physical lowering for backend development.

This is intentionally not an automatic placer or a footprint library.  It
creates deterministic proxy footprints and grid placements so the physical IR
and PCB backends can be exercised from existing ``.copper`` examples.  The
metadata clearly marks the result as a prototype that must not be fabricated.
"""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
import re
from typing import Mapping

from .elaborate import elaborate
from .model import Board, ComponentInstance, DeviceDefinition, PartDefinition
from .physical import (
    BoardOutline,
    FootprintPad,
    PadReference,
    PhysicalBoard,
    PhysicalFootprint,
    PhysicalNet,
    Placement,
    Point,
    Size,
)
from .quantities import Quantity


@dataclass(frozen=True, slots=True)
class PrototypePhysicalOptions:
    board_width_mm: float = 100.0
    board_height_mm: float = 80.0
    columns: int = 3
    margin_mm: float = 12.0

    def __post_init__(self) -> None:
        if self.board_width_mm <= 0 or self.board_height_mm <= 0:
            raise ValueError("prototype board dimensions must be positive")
        if self.columns < 1:
            raise ValueError("prototype placement columns must be at least one")
        if self.margin_mm <= 0:
            raise ValueError("prototype board margin must be positive")
        if self.margin_mm * 2 >= min(self.board_width_mm, self.board_height_mm):
            raise ValueError("prototype board margin leaves no placement area")


def prototype_physicalize(
    board: Board, options: PrototypePhysicalOptions | None = None
) -> PhysicalBoard:
    """Create a deterministic, inspectable PCB draft from electrical IR.

    Components without a selected footprint are omitted.  Selected footprint
    names are retained as provenance, but pad geometry is a generic proxy.
    """

    options = options or PrototypePhysicalOptions()
    flat = elaborate(board)
    components = tuple(
        sorted(
            (component for component in flat.components if component.footprint),
            key=lambda component: component.ref,
        )
    )
    component_index = {component.ref: component for component in components}
    footprints: dict[str, PhysicalFootprint] = {}
    placements: list[Placement] = []

    usable_width = options.board_width_mm - 2 * options.margin_mm
    usable_height = options.board_height_mm - 2 * options.margin_mm
    rows = max(1, (len(components) + options.columns - 1) // options.columns)
    for index, component in enumerate(components):
        part = flat.library[component.part]
        footprint = _proxy_footprint(part, component)
        footprints.setdefault(footprint.name, footprint)
        column = index % options.columns
        row = index // options.columns
        x = options.margin_mm + usable_width * (column + 0.5) / options.columns
        y = options.margin_mm + usable_height * (row + 0.5) / rows
        placements.append(
            Placement(
                reference=component.ref,
                footprint=footprint.name,
                position=Point.mm(x, y),
                value=_component_value(component, part),
                source_path=component.ref,
            )
        )

    nets: list[PhysicalNet] = []
    for net in sorted(flat.nets, key=lambda item: item.name):
        pad_refs: list[PadReference] = []
        for endpoint in net.endpoints:
            component = component_index.get(endpoint.component)
            if component is None:
                continue
            part = flat.library[component.part]
            pin_number = _physical_pin_number(
                component, part, flat.devices, endpoint.pin
            )
            if pin_number is not None:
                pad_refs.append(PadReference(component.ref, pin_number))
        if pad_refs:
            nets.append(PhysicalNet(net.name, tuple(sorted(set(pad_refs)))))

    omitted = sorted(
        component.ref for component in flat.components if not component.footprint
    )
    return PhysicalBoard(
        name=board.name,
        outline=BoardOutline.rectangle(
            options.board_width_mm, options.board_height_mm
        ),
        footprints=footprints,
        placements=tuple(placements),
        nets=tuple(nets),
        metadata={
            "generator": "copperscript-prototype-physicalizer",
            "prototype_footprints": "true",
            "fabrication_ready": "false",
            "omitted_components": ",".join(omitted),
        },
    )


def _proxy_footprint(
    part: PartDefinition,
    component: ComponentInstance,
) -> PhysicalFootprint:
    ordered_pins = tuple(sorted(part.pins.values(), key=lambda pin: _natural(pin.number)))
    count = len(ordered_pins)
    digest = sha256(
        f"{part.name}\0{component.footprint}\0{','.join(pin.number for pin in ordered_pins)}".encode()
    ).hexdigest()[:10]
    name = f"CopperScript/{_safe(component.footprint or part.name)}_{digest}"

    if count == 2:
        positions = (Point.mm(-1.0, 0), Point.mm(1.0, 0))
        body_size = Size.mm(1.6, 0.8)
    else:
        left_count = (count + 1) // 2
        right_count = count - left_count
        pitch = 1.27
        positions = tuple(
            Point.mm(-2.5, (index - (left_count - 1) / 2) * pitch)
            for index in range(left_count)
        ) + tuple(
            Point.mm(2.5, (index - (right_count - 1) / 2) * pitch)
            for index in range(right_count)
        )
        body_size = Size.mm(4.0, max(3.0, max(left_count, right_count) * pitch))

    pads = tuple(
        FootprintPad(pin.number, position, Size.mm(1.0, 1.0))
        for pin, position in zip(ordered_pins, positions, strict=True)
    )
    return PhysicalFootprint(
        name=name,
        pads=pads,
        body_size=body_size,
        source_library_id=component.footprint,
    )


def _physical_pin_number(
    component: ComponentInstance,
    part: PartDefinition,
    devices: Mapping[str, DeviceDefinition],
    endpoint_name: str,
) -> str | None:
    direct = part.pins.get(endpoint_name)
    if direct is not None:
        return direct.number

    unit_name, separator, terminal_name = endpoint_name.partition(".")
    device = devices.get(part.device or "")
    unit = device.units.get(unit_name) if device is not None and separator else None
    terminal = unit.terminals.get(terminal_name) if unit is not None else None
    if terminal is None:
        return None
    matches = [
        pin.number
        for pin in part.pins.values()
        if any(
            bond.pad == terminal.pad
            and (bond.when is None or bond.when.matches(component.modes))
            for bond in pin.bonds
        )
    ]
    return matches[0] if len(matches) == 1 else None


def _component_value(component: ComponentInstance, part: PartDefinition) -> str:
    if component.value is None:
        return part.name.rsplit(".", 1)[-1]
    if isinstance(component.value, Quantity):
        return str(component.value)
    return component.value


def _natural(value: str) -> tuple[tuple[int, object], ...]:
    return tuple(
        (0, int(piece)) if piece.isdigit() else (1, piece.casefold())
        for piece in re.split(r"(\d+)", value)
        if piece
    )


def _safe(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", value).strip("_") or "Footprint"

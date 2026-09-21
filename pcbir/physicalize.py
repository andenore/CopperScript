"""Electrical-to-physical lowering for PCB backend development.

This is intentionally not an automatic placer. It can resolve verified
external footprints or, by explicit request, generate proxy geometry. Both
paths use deterministic grid placement and mark the result as a draft.
"""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
import re
from typing import Callable, Mapping

from .elaborate import elaborate
from .footprints import FootprintResolver
from .importers import FootprintImportResult
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

    metadata = {
        "generator": "copperscript-prototype-physicalizer",
        "prototype_footprints": "true",
        "prototype_placement": "true",
        "fabrication_ready": "false",
    }
    return _physicalize(
        board,
        lambda part, component, selected: _proxy_footprint(
            part, component, selected
        ),
        options or PrototypePhysicalOptions(),
        metadata,
    )


def resolved_physicalize(
    board: Board,
    resolver: FootprintResolver,
    options: PrototypePhysicalOptions | None = None,
) -> PhysicalBoard:
    """Create a PCB draft using verified external footprint geometry.

    Placement is still a deterministic inspection grid.  Unlike
    :func:`prototype_physicalize`, this path never creates proxy pad geometry:
    every selected footprint must resolve and its electrical pad numbers must
    exactly match the corresponding part definition.
    """

    cache: dict[str, FootprintImportResult] = {}
    reported_warnings: set[str] = set()
    warning_messages: list[str] = []
    metadata = {
        "generator": "copperscript-resolved-footprint-physicalizer",
        "resolved_footprints": "true",
        "prototype_placement": "true",
        "fabrication_ready": "false",
    }

    def resolve_footprint(
        part: PartDefinition,
        component: ComponentInstance,
        selected: str,
    ) -> PhysicalFootprint:
        result = cache.get(selected)
        if result is None:
            result = resolver.resolve(selected)
            cache[selected] = result
        _validate_footprint_pins(part, component, result.footprint)
        if selected not in reported_warnings:
            reported_warnings.add(selected)
            warning_messages.extend(
                f"{selected}: {warning}" for warning in result.warnings
            )
            if warning_messages:
                metadata["footprint_import_warnings"] = "\n".join(warning_messages)
        return result.footprint

    return _physicalize(
        board,
        resolve_footprint,
        options or PrototypePhysicalOptions(),
        metadata,
    )


_FootprintProvider = Callable[
    [PartDefinition, ComponentInstance, str], PhysicalFootprint
]


def _physicalize(
    board: Board,
    footprint_provider: _FootprintProvider,
    options: PrototypePhysicalOptions,
    metadata: dict[str, str],
) -> PhysicalBoard:
    flat = elaborate(board)
    selected_components = tuple(
        sorted(
            (
                (component, flat.library[component.part], selected)
                for component in flat.components
                if (
                    selected := _selected_footprint(
                        component, flat.library[component.part]
                    )
                )
                is not None
            ),
            key=lambda item: item[0].ref,
        )
    )
    component_index = {
        component.ref: component for component, _, _ in selected_components
    }
    footprints: dict[str, PhysicalFootprint] = {}
    placements: list[Placement] = []

    usable_width = options.board_width_mm - 2 * options.margin_mm
    usable_height = options.board_height_mm - 2 * options.margin_mm
    rows = max(
        1, (len(selected_components) + options.columns - 1) // options.columns
    )
    for index, (component, part, selected) in enumerate(selected_components):
        footprint = footprint_provider(part, component, selected)
        existing = footprints.get(footprint.name)
        if existing is not None and existing != footprint:
            raise ValueError(
                f"footprint ID {footprint.name!r} resolved to conflicting geometry"
            )
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
        component.ref
        for component in flat.components
        if _selected_footprint(component, flat.library[component.part]) is None
    )
    metadata["omitted_components"] = ",".join(omitted)
    return PhysicalBoard(
        name=board.name,
        outline=BoardOutline.rectangle(
            options.board_width_mm, options.board_height_mm
        ),
        footprints=footprints,
        placements=tuple(placements),
        nets=tuple(nets),
        metadata=metadata,
    )


def _proxy_footprint(
    part: PartDefinition,
    component: ComponentInstance,
    selected: str,
) -> PhysicalFootprint:
    ordered_pins = tuple(sorted(part.pins.values(), key=lambda pin: _natural(pin.number)))
    count = len(ordered_pins)
    digest = sha256(
        f"{part.name}\0{selected}\0{','.join(pin.number for pin in ordered_pins)}".encode()
    ).hexdigest()[:10]
    name = f"CopperScript/{_safe(selected)}_{digest}"

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
        source_library_id=selected,
    )


def _selected_footprint(
    component: ComponentInstance, part: PartDefinition
) -> str | None:
    if component.footprint:
        return component.footprint
    return part.footprints[0] if part.footprints else None


def _validate_footprint_pins(
    part: PartDefinition,
    component: ComponentInstance,
    footprint: PhysicalFootprint,
) -> None:
    part_numbers = {pin.number for pin in part.pins.values()}
    footprint_numbers = {pad.number for pad in footprint.pads if pad.number}
    missing = sorted(part_numbers - footprint_numbers, key=_natural)
    extra = sorted(footprint_numbers - part_numbers, key=_natural)
    if missing or extra:
        details: list[str] = []
        if missing:
            details.append(f"missing pads {', '.join(missing)}")
        if extra:
            details.append(f"unknown pads {', '.join(extra)}")
        raise ValueError(
            f"component {component.ref!r} part {part.name!r} is incompatible with "
            f"footprint {footprint.source_library_id or footprint.name!r}: "
            + "; ".join(details)
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

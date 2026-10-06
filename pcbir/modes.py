"""Device mode evaluation shared by ERC, power analysis and every backend.

A component selects device modes with ``modes = "GROUP=CHOICE"``. Each
``mode_group`` may declare a ``default`` choice; that default applies wherever
a mode condition is evaluated (bonds, pads, mux options, route rules and signal
groups) when the component does not select the group itself. An explicit
selection always overrides the default. A group without a default and without
a selection has no active choice, so no condition on it matches (ERC reports
``MODE_NOT_SELECTED``).

All mode-condition evaluation goes through this module so that the electrical
checks and the physical/backend lowering always agree on which pads a package
pin is bonded to.
"""

from __future__ import annotations

from typing import Mapping

from .model import (
    BondDefinition,
    ComponentInstance,
    Condition,
    DeviceDefinition,
    DevicePadDefinition,
    PackagePinDefinition,
    PartDefinition,
)


def effective_modes(
    component: ComponentInstance, device: DeviceDefinition | None
) -> Mapping[str, str]:
    """The component's explicit selections layered over the device defaults."""

    if device is None or not device.mode_groups:
        return component.modes
    modes = {
        name: group.default
        for name, group in device.mode_groups.items()
        if group.default is not None
    }
    modes.update(component.modes)
    return modes


def condition_active(condition: Condition | None, modes: Mapping[str, str]) -> bool:
    """Unconditional items are always active; others need every selection."""

    return condition is None or condition.matches(modes)


def component_device(
    component: ComponentInstance,
    part: PartDefinition | None,
    devices: Mapping[str, DeviceDefinition],
) -> DeviceDefinition | None:
    return devices.get(part.device or "") if part is not None else None


def active_bonds(
    component: ComponentInstance,
    pin: PackagePinDefinition,
    device: DeviceDefinition | None,
) -> tuple[BondDefinition, ...]:
    modes = effective_modes(component, device)
    return tuple(bond for bond in pin.bonds if condition_active(bond.when, modes))


def active_bonded_pads(
    component: ComponentInstance,
    pin: PackagePinDefinition,
    device: DeviceDefinition | None,
) -> tuple[str, ...]:
    """Names of the device pads this package pin is bonded to in its modes."""

    return tuple(bond.pad for bond in active_bonds(component, pin, device))


def active_device_pads(
    component: ComponentInstance,
    pin: PackagePinDefinition,
    device: DeviceDefinition | None,
) -> tuple[DevicePadDefinition, ...]:
    """Actively bonded device pads whose own ``when`` condition also holds."""

    if device is None:
        return ()
    modes = effective_modes(component, device)
    pads = []
    for bond in pin.bonds:
        if not condition_active(bond.when, modes):
            continue
        pad = device.pads.get(bond.pad)
        if pad is not None and condition_active(pad.when, modes):
            pads.append(pad)
    return tuple(pads)


def pins_bonded_to_pad(
    component: ComponentInstance,
    part: PartDefinition,
    device: DeviceDefinition | None,
    pad: str,
) -> tuple[PackagePinDefinition, ...]:
    """Package pins whose active bonds include ``pad``."""

    return tuple(
        pin
        for pin in part.pins.values()
        if pad in active_bonded_pads(component, pin, device)
    )

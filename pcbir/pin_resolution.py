"""Electrical terminal resolution independent of PCB geometry and backend syntax."""
from .model import ComponentInstance, FlatElectricalView, PackagePinDefinition


def resolve_package_pin(view: FlatElectricalView, component: ComponentInstance,
                        terminal: str) -> PackagePinDefinition:
    part = view.library[component.part]
    direct = part.pins.get(terminal)
    if direct is not None:
        return direct
    matches = [p for p in part.pins.values() if p.name == terminal or p.number == terminal]
    if not matches:
        unit_name, separator, terminal_name = terminal.partition(".")
        device = view.devices.get(part.device or "")
        unit = device.units.get(unit_name) if device and separator else None
        binding = unit.terminals.get(terminal_name) if unit else None
        if binding:
            matches = [p for p in part.pins.values() if any(
                b.pad == binding.pad and (b.when is None or b.when.matches(component.modes))
                for b in p.bonds)]
    if matches and len({p.number for p in matches}) == 1:
        return sorted(matches, key=lambda p: p.name)[0]
    raise ValueError(f"{component.ref}.{terminal}: unknown or ambiguous package terminal")

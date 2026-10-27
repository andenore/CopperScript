//! Device mode evaluation shared by ERC and every backend (`pcbir.modes`).
//!
//! A component selects modes with `modes = "GROUP=CHOICE"`; a mode group's
//! `default` applies wherever the component does not select the group.

use indexmap::IndexMap;

use crate::model::*;

/// The component's explicit selections layered over the device defaults.
pub fn effective_modes(component: &ComponentInstance, device: Option<&DeviceDefinition>) -> IndexMap<String, String> {
    let Some(device) = device.filter(|device| !device.mode_groups.is_empty()) else {
        return component.modes.clone();
    };
    let mut modes: IndexMap<String, String> = device
        .mode_groups
        .iter()
        .filter_map(|(name, group)| group.default.clone().map(|choice| (name.clone(), choice)))
        .collect();
    for (group, choice) in &component.modes {
        modes.insert(group.clone(), choice.clone());
    }
    modes
}

pub fn condition_active(condition: Option<&Condition>, modes: &IndexMap<String, String>) -> bool {
    condition.is_none_or(|condition| condition.matches(modes))
}

pub fn active_bonds<'a>(
    component: &ComponentInstance,
    pin: &'a PackagePinDefinition,
    device: Option<&DeviceDefinition>,
) -> Vec<&'a BondDefinition> {
    let modes = effective_modes(component, device);
    pin.bonds.iter().filter(|bond| condition_active(bond.when.as_ref(), &modes)).collect()
}

/// Device pads this package pin is bonded to in the component's modes.
pub fn active_bonded_pads(
    component: &ComponentInstance,
    pin: &PackagePinDefinition,
    device: Option<&DeviceDefinition>,
) -> Vec<String> {
    active_bonds(component, pin, device).into_iter().map(|bond| bond.pad.clone()).collect()
}

/// Actively bonded device pads whose own `when` also holds.
pub fn active_device_pads<'a>(
    component: &ComponentInstance,
    pin: &PackagePinDefinition,
    device: Option<&'a DeviceDefinition>,
) -> Vec<&'a DevicePadDefinition> {
    let Some(device) = device else { return Vec::new() };
    let modes = effective_modes(component, Some(device));
    pin.bonds
        .iter()
        .filter(|bond| condition_active(bond.when.as_ref(), &modes))
        .filter_map(|bond| device.pads.get(&bond.pad))
        .filter(|pad| condition_active(pad.when.as_ref(), &modes))
        .collect()
}

/// Package pins whose active bonds include `pad`.
pub fn pins_bonded_to_pad<'a>(
    component: &ComponentInstance,
    part: &'a PartDefinition,
    device: Option<&DeviceDefinition>,
    pad: &str,
) -> Vec<&'a PackagePinDefinition> {
    part.pins.values().filter(|pin| active_bonded_pads(component, pin, device).iter().any(|name| name == pad)).collect()
}

pub fn component_device<'a>(
    part: Option<&PartDefinition>,
    devices: &'a IndexMap<String, DeviceDefinition>,
) -> Option<&'a DeviceDefinition> {
    part.and_then(|part| devices.get(part.device.as_deref().unwrap_or("")))
}

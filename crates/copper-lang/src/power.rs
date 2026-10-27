//! Discrete power-state analysis for powered and unpowered I/O domains
//! (`pcbir.power`).

use indexmap::{IndexMap, IndexSet};

use copper_json::python_str_repr as repr;

use crate::erc::Diagnostic;
use crate::model::{
    ComponentInstance, DevicePadDefinition, Direction, ElectricalProfile, Endpoint, FlatElectricalView,
    PackagePinDefinition, PartDefinition, PowerRailState, SignalDomain, UnpoweredBehavior,
};
use crate::modes::{active_bonded_pads, active_device_pads};

/// Components by reference; like the Python dict, the last declaration wins.
fn component_map(board: &FlatElectricalView) -> IndexMap<&str, &ComponentInstance> {
    board.components.iter().map(|component| (component.reference.as_str(), component)).collect()
}

/// `analyze_power_states`: explicit steady-state scenarios, without firmware.
pub fn analyze_power_states(board: &FlatElectricalView) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let supplies: IndexSet<&str> = board.supplies.iter().map(|supply| supply.name.as_str()).collect();
    let components = component_map(board);
    let mut pin_nets: IndexMap<Endpoint, Vec<String>> = IndexMap::new();
    for net in &board.nets {
        for endpoint in &net.endpoints {
            pin_nets.entry(endpoint.clone()).or_default().push(net.name.clone());
        }
    }
    let mut seen: IndexSet<&str> = IndexSet::new();
    for state in &board.power_states {
        if !seen.insert(&state.name) {
            diagnostics.push(Diagnostic::error(
                "DUPLICATE_POWER_STATE",
                format!("duplicate power state {}", repr(&state.name)),
                state.name.clone(),
            ));
            continue;
        }
        for rail in state.rails.keys() {
            if !supplies.contains(rail.as_str()) {
                diagnostics.push(Diagnostic::error(
                    "UNKNOWN_POWER_STATE_RAIL",
                    format!("power state references unknown supply {}", repr(rail)),
                    state.name.clone(),
                ));
            }
        }
        let domain_states = component_domain_states(board, &state.rails, &pin_nets);
        let active_outputs = active_peripheral_outputs(board, &domain_states);
        for net in &board.nets {
            let rail_is_on = board
                .supplies
                .iter()
                .filter(|supply| supply.net == net.name)
                .any(|supply| state.rails.get(&supply.name) == Some(&PowerRailState::On));
            let mut driven = rail_is_on;
            for endpoint in &net.endpoints {
                let Some((part, pin)) = resolve_pin(board, &components, endpoint) else { continue };
                if is_power_output(board, components[endpoint.component.as_str()], part, pin)
                    || active_outputs.contains(endpoint)
                {
                    driven = true;
                }
            }
            if !driven {
                continue;
            }
            for endpoint in &net.endpoints {
                let Some((part, pin)) = resolve_pin(board, &components, endpoint) else { continue };
                let component = components[endpoint.component.as_str()];
                if is_power_input(board, component, part, pin) {
                    continue;
                }
                for pad in bonded_pads(board, component, part, pin) {
                    let Some(domain) = &pad.power_domain else { continue };
                    if domain_states.get(&(endpoint.component.clone(), domain.clone())) != Some(&PowerRailState::Off) {
                        continue;
                    }
                    if matches!(pad.unpowered_behavior, UnpoweredBehavior::Tolerant | UnpoweredBehavior::HighImpedance)
                    {
                        continue;
                    }
                    diagnostics.push(Diagnostic::warning(
                        "POSSIBLE_BACKPOWER",
                        format!(
                            "net {} is driven while domain {domain} is off; unpowered behavior is {}",
                            net.name,
                            pad.unpowered_behavior.value()
                        ),
                        format!("{}:{endpoint}", state.name),
                    ));
                }
            }
        }
    }
    diagnostics
}

fn component_domain_states(
    board: &FlatElectricalView,
    rails: &IndexMap<String, PowerRailState>,
    pin_nets: &IndexMap<Endpoint, Vec<String>>,
) -> IndexMap<(String, String), PowerRailState> {
    let mut result = IndexMap::new();
    for component in &board.components {
        let Some(part) = board.library.get(&component.part) else { continue };
        let Some(device) = board.devices.get(part.device.as_deref().unwrap_or("")) else { continue };
        for domain in device.power_domains.values() {
            let mut states = Vec::new();
            for pin in part.pins.values() {
                let pads = active_bonded_pads(component, pin, Some(device));
                if !pads.iter().any(|pad| domain.supply_pads.contains(pad)) {
                    continue;
                }
                let endpoint = Endpoint::new(&component.reference, &pin.name);
                for net in pin_nets.get(&endpoint).into_iter().flatten() {
                    for supply in board.supplies.iter().filter(|supply| &supply.net == net) {
                        states.push(rails.get(&supply.name).copied().unwrap_or(PowerRailState::Unknown));
                    }
                }
            }
            let state = if !states.is_empty() && states.iter().all(|s| *s == PowerRailState::On) {
                PowerRailState::On
            } else if !states.is_empty() && states.iter().all(|s| *s == PowerRailState::Off) {
                PowerRailState::Off
            } else {
                PowerRailState::Unknown
            };
            result.insert((component.reference.clone(), domain.name.clone()), state);
        }
    }
    result
}

fn active_peripheral_outputs(
    board: &FlatElectricalView,
    domain_states: &IndexMap<(String, String), PowerRailState>,
) -> IndexSet<Endpoint> {
    let mut result = IndexSet::new();
    let components = component_map(board);
    for selection in &board.peripheral_selections {
        let Some(component) = components.get(selection.component.as_str()) else { continue };
        let Some(part) = board.library.get(&component.part) else { continue };
        let Some(device) = board.devices.get(part.device.as_deref().unwrap_or("")) else { continue };
        let Some(peripheral) = device.peripherals.get(&selection.peripheral) else { continue };
        for (name, chosen) in &selection.signals {
            let (Some(signal), Some(pin)) = (peripheral.signals.get(name), part.pins.get(&chosen.pin)) else {
                continue;
            };
            if !signal.profile.directions.contains(&Direction::Output) {
                continue;
            }
            let off = bonded_pads(board, component, part, pin).iter().any(|pad| {
                pad.power_domain.as_ref().is_some_and(|domain| {
                    domain_states.get(&(component.reference.clone(), domain.clone())) == Some(&PowerRailState::Off)
                })
            });
            if !off {
                result.insert(Endpoint::new(&component.reference, &pin.name));
            }
        }
    }
    result
}

fn resolve_pin<'a>(
    board: &'a FlatElectricalView,
    components: &IndexMap<&str, &'a ComponentInstance>,
    endpoint: &Endpoint,
) -> Option<(&'a PartDefinition, &'a PackagePinDefinition)> {
    let component = components.get(endpoint.component.as_str())?;
    let part = board.library.get(&component.part)?;
    Some((part, part.pins.get(&endpoint.pin)?))
}

fn bonded_pads<'a>(
    board: &'a FlatElectricalView,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &PackagePinDefinition,
) -> Vec<&'a DevicePadDefinition> {
    active_device_pads(component, pin, board.devices.get(part.device.as_deref().unwrap_or("")))
}

fn profiles<'a>(
    board: &'a FlatElectricalView,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &'a PackagePinDefinition,
) -> Vec<&'a ElectricalProfile> {
    let mut result: Vec<&ElectricalProfile> = pin.profile.iter().collect();
    result.extend(bonded_pads(board, component, part, pin).into_iter().map(|pad| &pad.profile));
    result
}

fn is_power_input(
    board: &FlatElectricalView,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &PackagePinDefinition,
) -> bool {
    profiles(board, component, part, pin).iter().any(|profile| {
        (profile.domains.contains(&SignalDomain::Power) || profile.domains.contains(&SignalDomain::Ground))
            && profile.directions.contains(&Direction::Input)
    })
}

fn is_power_output(
    board: &FlatElectricalView,
    component: &ComponentInstance,
    part: &PartDefinition,
    pin: &PackagePinDefinition,
) -> bool {
    profiles(board, component, part, pin).iter().any(|profile| {
        profile.domains.contains(&SignalDomain::Power) && profile.directions.contains(&Direction::Output)
    })
}

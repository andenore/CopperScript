//! Supply-net physical membership (`pcbir.power_planning`
//! `lower_power_domains` and `lower_power_feeds`). Membership only: no
//! connectivity, copper or power-state inference changes here.

use indexmap::{IndexMap, IndexSet};

use copper_lang::model::{ComponentInstance, Direction, FlatElectricalView, PinType, SignalDomain};

use crate::board::{PadReference, PhysicalNet, PhysicalPowerDomain, PowerFeedBoundary};
use crate::physicalize::physical_pin_number;

/// `lower_power_domains`: rails from explicit supplies and active power-pin profiles.
///
/// Zero-volt supplies and pins marked ground are not clustering domains;
/// distinct same-voltage nets stay distinct.
pub fn lower_power_domains(flat: &FlatElectricalView, physical_nets: &[PhysicalNet]) -> Vec<PhysicalPowerDomain> {
    let components: IndexMap<&str, &ComponentInstance> =
        flat.components.iter().map(|c| (c.reference.as_str(), c)).collect();
    let mut declared: IndexSet<String> =
        flat.supplies.iter().filter(|s| !s.voltage.base_value().is_zero()).map(|s| s.net.clone()).collect();
    let mut zero_rails: IndexSet<String> =
        flat.supplies.iter().filter(|s| s.voltage.base_value().is_zero()).map(|s| s.net.clone()).collect();
    let mut sources: IndexMap<String, IndexSet<PadReference>> = IndexMap::new();
    let mut source_ports: IndexMap<String, IndexSet<String>> = IndexMap::new();
    // A module-sourced supply's explicit power_out port identifies a physical
    // boundary: keep every internal terminal of it.
    for instance in &flat.module_instances {
        for (port, net_name) in &instance.connections {
            if instance.ports.get(port) != Some(&PinType::PowerOut) {
                continue;
            }
            let prefix = format!("{}/", instance.path);
            let terminals: Vec<&PadReference> = physical_nets
                .iter()
                .filter(|net| net.name == *net_name)
                .flat_map(|net| net.pads.iter())
                .filter(|pad| pad.component.starts_with(&prefix))
                .collect();
            if !terminals.is_empty() {
                declared.insert(net_name.clone());
                sources.entry(net_name.clone()).or_default().extend(terminals.into_iter().cloned());
                source_ports.entry(net_name.clone()).or_default().insert(format!("{}.{port}", instance.path));
            }
        }
    }
    for supply in &flat.supplies {
        let Some(source) = &supply.source else { continue };
        let Some(component) = components.get(source.component.as_str()) else { continue };
        let part = &flat.library[&component.part];
        if let Some(number) = physical_pin_number(component, part, &flat.devices, &source.pin) {
            sources.entry(supply.net.clone()).or_default().insert(PadReference::new(&component.reference, &number));
        }
    }
    for net in &flat.nets {
        for endpoint in &net.endpoints {
            let Some(component) = components.get(endpoint.component.as_str()) else { continue };
            let part = &flat.library[&component.part];
            let number = physical_pin_number(component, part, &flat.devices, &endpoint.pin);
            let device = flat.devices.get(part.device.as_deref().unwrap_or(""));
            let mut profiles = Vec::new();
            for pin in part.pins.values() {
                if Some(&pin.number) != number.as_ref() {
                    continue;
                }
                if let Some(profile) = &pin.profile {
                    profiles.push(profile);
                }
                let Some(device) = device else { continue };
                for bond in &pin.bonds {
                    let Some(pad) = device.pads.get(&bond.pad) else { continue };
                    if bond.when.as_ref().is_none_or(|when| when.matches(&component.modes))
                        && pad.when.as_ref().is_none_or(|when| when.matches(&component.modes))
                    {
                        profiles.push(&pad.profile);
                    }
                }
            }
            for profile in profiles {
                // A compatible-domain set is not the pin's assigned function.
                if profile.domains.len() == 1 && profile.domains.contains(&SignalDomain::Ground) {
                    zero_rails.insert(net.name.clone());
                } else if profile.domains.len() == 1 && profile.domains.contains(&SignalDomain::Power) {
                    declared.insert(net.name.clone());
                    if let Some(number) = &number
                        && profile.directions.contains(&Direction::Output)
                    {
                        sources
                            .entry(net.name.clone())
                            .or_default()
                            .insert(PadReference::new(&component.reference, number));
                    }
                }
            }
        }
    }
    physical_nets
        .iter()
        .filter(|net| declared.contains(&net.name) && !zero_rails.contains(&net.name))
        .map(|net| {
            let mut own: Vec<PadReference> = sources
                .get(&net.name)
                .map(|set| set.iter().filter(|pad| net.pads.contains(pad)).cloned().collect())
                .unwrap_or_default();
            own.sort();
            let mut ports: Vec<String> =
                source_ports.get(&net.name).map(|set| set.iter().cloned().collect()).unwrap_or_default();
            ports.sort();
            PhysicalPowerDomain { net: net.name.clone(), members: net.pads.clone(), sources: own, source_ports: ports }
        })
        .collect()
}

/// `lower_power_feeds`: orient explicit passive DC paths by distance from a real source.
///
/// Equal-distance bridges are not guessed directions.
pub fn lower_power_feeds(
    flat: &FlatElectricalView,
    physical_nets: &[PhysicalNet],
    domains: &[PhysicalPowerDomain],
) -> Vec<PowerFeedBoundary> {
    let mut distance: IndexMap<String, usize> =
        domains.iter().filter(|d| !d.sources.is_empty()).map(|d| (d.net.clone(), 0)).collect();
    for supply in &flat.supplies {
        if supply.externally_driven || supply.source.is_some() {
            distance.insert(supply.net.clone(), 0);
        }
    }
    let mut memberships: IndexMap<&PadReference, IndexSet<&str>> = IndexMap::new();
    for net in physical_nets {
        for pad in &net.pads {
            memberships.entry(pad).or_default().insert(net.name.as_str());
        }
    }
    let membership: IndexMap<&PadReference, &str> = memberships
        .iter()
        .filter(|(_, names)| names.len() == 1)
        .map(|(pad, names)| (*pad, *names.first().expect("one net")))
        .collect();
    let mut links: Vec<(String, PadReference, String, PadReference)> = Vec::new();
    for component in &flat.components {
        let part = &flat.library[&component.part];
        if !part.assembled || part.dc_power_path.is_empty() {
            continue;
        }
        let pads: Vec<PadReference> = part
            .dc_power_path
            .iter()
            .filter_map(|name| physical_pin_number(component, part, &flat.devices, name))
            .map(|number| PadReference::new(&component.reference, &number))
            .collect();
        if pads.len() == 2 && pads.iter().all(|p| membership.contains_key(p)) {
            let (a, b) = (&pads[0], &pads[1]);
            if membership[a] != membership[b] {
                links.push((membership[a].to_owned(), a.clone(), membership[b].to_owned(), b.clone()));
            }
        }
    }
    // Bounded breadth expansion, without turning cycles into sources.
    for depth in 0..links.len() {
        let mut additions: IndexMap<String, usize> = IndexMap::new();
        for (a, _, b, _) in &links {
            if distance.get(a) == Some(&depth) && !distance.contains_key(b) {
                additions.insert(b.clone(), depth + 1);
            }
            if distance.get(b) == Some(&depth) && !distance.contains_key(a) {
                additions.insert(a.clone(), depth + 1);
            }
        }
        if additions.is_empty() {
            break;
        }
        distance.extend(additions);
    }
    let mut feeds = Vec::new();
    for (a, ap, b, bp) in &links {
        let (Some(da), Some(db)) = (distance.get(a), distance.get(b)) else { continue };
        if da == db {
            continue;
        }
        let ((a, ap), (b, bp)) = if da > db { ((b, bp), (a, ap)) } else { ((a, ap), (b, bp)) };
        feeds.push(PowerFeedBoundary {
            net: b.clone(),
            terminal: bp.clone(),
            upstream_net: a.clone(),
            upstream_terminal: ap.clone(),
            provenance: "dc_power_path".to_owned(),
        });
    }
    feeds.sort_by(|x, y| (&x.net, &x.terminal, &x.upstream_net).cmp(&(&y.net, &y.terminal, &y.upstream_net)));
    feeds
}

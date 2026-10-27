//! A flat electrical view derived from the hierarchical IR (`pcbir.elaborate`).

use indexmap::IndexMap;

use crate::model::*;

#[derive(Clone)]
struct Template {
    name: String,
    ports: IndexMap<String, PinType>,
    body: FlatElectricalView,
}

/// `elaborate(board)`: expand every module instance, depth first.
pub fn elaborate(board: &Board) -> Result<FlatElectricalView, String> {
    let mut result = FlatElectricalView {
        name: board.name.clone(),
        library: board.library.clone(),
        components: board.components.clone(),
        nets: board.nets.clone(),
        supplies: board.supplies.clone(),
        interfaces: board.interfaces.clone(),
        constraints: board.constraints.clone(),
        module_instances: Vec::new(),
        devices: board.devices.clone(),
        peripheral_selections: board.peripheral_selections.clone(),
        power_states: board.power_states.clone(),
        physical_macros: board.physical_macros.clone(),
        net_aliases: board.nets.iter().map(|net| (net.name.clone(), net.name.clone())).collect(),
        no_connects: board.no_connects.clone(),
    };
    let mut cache = IndexMap::new();
    for instance in &board.module_instances {
        let template = template_for(&instance.module, &board.module_definitions, &mut cache, &[])?;
        result = expand_one(result, &instance.reference, &template)?;
    }
    Ok(result)
}

fn template_for(
    name: &str,
    definitions: &IndexMap<String, ModuleDefinition>,
    cache: &mut IndexMap<String, Template>,
    stack: &[String],
) -> Result<Template, String> {
    if let Some(template) = cache.get(name) {
        return Ok(template.clone());
    }
    if stack.iter().any(|entry| entry == name) {
        return Err(format!("cyclic module instantiation: {} -> {name}", stack.join(" -> ")));
    }
    let definition =
        definitions.get(name).ok_or_else(|| format!("KeyError: {}", copper_json::python_str_repr(name)))?;
    let mut body = FlatElectricalView {
        name: definition.name.clone(),
        library: definition.library.clone(),
        components: definition.components.clone(),
        nets: definition.nets.clone(),
        supplies: definition.supplies.clone(),
        interfaces: definition.interfaces.clone(),
        constraints: definition.constraints.clone(),
        module_instances: Vec::new(),
        devices: definition.devices.clone(),
        peripheral_selections: definition.peripheral_selections.clone(),
        power_states: Vec::new(),
        physical_macros: definition.physical_macros.clone(),
        net_aliases: definition.nets.iter().map(|net| (net.name.clone(), net.name.clone())).collect(),
        no_connects: definition.no_connects.clone(),
    };
    let mut nested_stack = stack.to_vec();
    nested_stack.push(name.to_owned());
    for instance in &definition.module_instances {
        let child = template_for(&instance.module, definitions, cache, &nested_stack)?;
        body = expand_one(body, &instance.reference, &child)?;
    }
    let template = Template { name: definition.name.clone(), ports: definition.ports.clone(), body };
    cache.insert(name.to_owned(), template.clone());
    Ok(template)
}

fn qualify(prefix: &str, name: &str) -> String {
    format!("{prefix}/{name}")
}

fn qualify_endpoint(prefix: &str, endpoint: &Endpoint) -> Endpoint {
    Endpoint { component: qualify(prefix, &endpoint.component), pin: endpoint.pin.clone() }
}

fn qualify_target(prefix: &str, target: &str, body: &FlatElectricalView, net_map: &IndexMap<String, String>) -> String {
    if let Some(mapped) = net_map.get(target) {
        return mapped.clone();
    }
    if let Some((owner, pin)) = target.split_once('.') {
        return format!("{}.{pin}", qualify(prefix, owner));
    }
    let named = body.components.iter().any(|c| c.reference == target)
        || body.supplies.iter().any(|s| s.name == target)
        || body.interfaces.iter().any(|i| i.name == target);
    if named { qualify(prefix, target) } else { target.to_owned() }
}

fn merge<V: PartialEq + Clone>(
    destination: &mut IndexMap<String, V>,
    incoming: &IndexMap<String, V>,
    what: &str,
) -> Result<(), String> {
    for (name, value) in incoming {
        match destination.get(name) {
            Some(existing) if existing != value => {
                return Err(format!("conflicting definitions for {what} {}", copper_json::python_str_repr(name)));
            }
            Some(_) => {}
            None => {
                destination.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn expand_one(parent: FlatElectricalView, instance: &str, template: &Template) -> Result<FlatElectricalView, String> {
    let mut net_entries: Vec<(String, Vec<Endpoint>)> =
        parent.nets.iter().map(|net| (net.name.clone(), net.endpoints.clone())).collect();
    let mut external: IndexMap<String, String> = IndexMap::new();
    for (net_name, endpoints) in &mut net_entries {
        endpoints.retain(|endpoint| {
            if endpoint.component == instance {
                external.insert(endpoint.pin.clone(), net_name.clone());
                false
            } else {
                true
            }
        });
    }
    let mut net_map: IndexMap<String, String> = IndexMap::new();
    let mut port_connections: IndexMap<String, String> = IndexMap::new();
    for child_net in &template.body.nets {
        let ports: Vec<&String> =
            child_net.endpoints.iter().filter(|e| e.component == "port").map(|e| &e.pin).collect();
        let mut mapped: Vec<&String> = ports.iter().filter_map(|port| external.get(*port)).collect();
        mapped.sort();
        mapped.dedup();
        if mapped.len() > 1 {
            return Err(format!("ports on {}.{} map to different parent nets", template.name, child_net.name));
        }
        let mapped_name =
            mapped.first().map(|name| (*name).clone()).unwrap_or_else(|| qualify(instance, &child_net.name));
        net_map.insert(child_net.name.clone(), mapped_name.clone());
        for port in ports {
            port_connections.insert(port.clone(), mapped_name.clone());
        }
        let internal: Vec<Endpoint> = child_net
            .endpoints
            .iter()
            .filter(|e| e.component != "port")
            .map(|e| qualify_endpoint(instance, e))
            .collect();
        match net_entries.iter_mut().find(|(name, _)| *name == mapped_name) {
            Some((_, endpoints)) => endpoints.extend(internal),
            None => net_entries.push((mapped_name, internal)),
        }
    }
    let mapped_net = |net: &str| net_map.get(net).cloned().unwrap_or_else(|| qualify(instance, net));

    let mut library = parent.library.clone();
    merge(&mut library, &template.body.library, "part")?;
    let mut devices = parent.devices.clone();
    merge(&mut devices, &template.body.devices, "device")?;

    let mut components = parent.components.clone();
    for component in &template.body.components {
        let mut properties = component.properties.clone();
        if let Some(target) = properties.get("decouples").cloned() {
            let endpoint = Endpoint::parse(&target)?;
            properties.insert(
                "decouples".to_owned(),
                Endpoint { component: qualify(instance, &endpoint.component), pin: endpoint.pin }.to_string(),
            );
        }
        components.push(ComponentInstance {
            reference: qualify(instance, &component.reference),
            properties,
            ..component.clone()
        });
    }

    let mut supplies: Vec<Supply> = parent
        .supplies
        .iter()
        .map(|supply| match &supply.source {
            Some(source) if source.component == instance => {
                Supply { source: None, externally_driven: true, ..supply.clone() }
            }
            _ => supply.clone(),
        })
        .collect();
    for supply in &template.body.supplies {
        let (source, externally_driven) = match &supply.source {
            Some(source) if source.component == "port" => (None, true),
            Some(source) => (Some(qualify_endpoint(instance, source)), supply.externally_driven),
            None => (None, supply.externally_driven),
        };
        supplies.push(Supply {
            name: qualify(instance, &supply.name),
            voltage: supply.voltage.clone(),
            net: mapped_net(&supply.net),
            source,
            externally_driven,
        });
    }

    let mut interfaces = parent.interfaces.clone();
    for interface in &template.body.interfaces {
        interfaces.push(Interface {
            name: qualify(instance, &interface.name),
            type_name: interface.type_name.clone(),
            signals: interface.signals.iter().map(|(signal, net)| (signal.clone(), mapped_net(net))).collect(),
            bindings: interface
                .bindings
                .iter()
                .map(|(component, pins)| (qualify(instance, component), pins.clone()))
                .collect(),
            pullup_supply: interface
                .pullup_supply
                .as_ref()
                .filter(|s| !s.is_empty())
                .map(|supply| qualify(instance, supply)),
        });
    }

    let mut constraints = parent.constraints.clone();
    for constraint in &template.body.constraints {
        constraints.push(Constraint {
            targets: constraint
                .targets
                .iter()
                .map(|target| qualify_target(instance, target, &template.body, &net_map))
                .collect(),
            ..constraint.clone()
        });
    }

    let mut selections = parent.peripheral_selections.clone();
    for selection in &template.body.peripheral_selections {
        selections.push(PeripheralSelection {
            component: qualify(instance, &selection.component),
            name: qualify(instance, &selection.name),
            ..selection.clone()
        });
    }

    let mut module_instances = parent.module_instances.clone();
    module_instances.push(ElaboratedModuleInstance {
        path: instance.to_owned(),
        module: template.name.clone(),
        ports: template.ports.clone(),
        connections: port_connections,
    });
    for nested in &template.body.module_instances {
        module_instances.push(ElaboratedModuleInstance {
            path: qualify(instance, &nested.path),
            module: nested.module.clone(),
            ports: nested.ports.clone(),
            connections: nested.connections.iter().map(|(port, net)| (port.clone(), mapped_net(net))).collect(),
        });
    }

    let mut physical_macros = parent.physical_macros.clone();
    for binding in &template.body.physical_macros {
        physical_macros.push(MacroBinding {
            name: qualify(instance, &binding.name),
            scope: if binding.scope.is_empty() { instance.to_owned() } else { qualify(instance, &binding.scope) },
            ..binding.clone()
        });
    }
    let mut net_aliases = parent.net_aliases.clone();
    for (alias, net) in &template.body.net_aliases {
        net_aliases.insert(qualify(instance, alias), mapped_net(net));
    }
    let mut no_connects = parent.no_connects.clone();
    for item in &template.body.no_connects {
        no_connects
            .push(NoConnect { endpoint: qualify_endpoint(instance, &item.endpoint), reason: item.reason.clone() });
    }

    Ok(FlatElectricalView {
        name: parent.name,
        library,
        components,
        nets: net_entries.into_iter().map(|(name, endpoints)| Net { name, endpoints }).collect(),
        supplies,
        interfaces,
        constraints,
        module_instances,
        devices,
        peripheral_selections: selections,
        power_states: parent.power_states,
        physical_macros,
        net_aliases,
        no_connects,
    })
}

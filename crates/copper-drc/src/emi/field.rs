//! The victims of a cell's near field, for `copper emi-check --plot-field`: the pads of the
//! `sensitive` nets outside the cell and of the cable connectors, by name only (the map looks
//! their positions up on the board it draws).

use super::*;

/// `[ { kind, name, pads: ["REF.PAD", ...] } ]` of the cell: every sensitive net (kind `sensitive_net`)
/// with pads outside the cell, and every cable connector with placed pads (kind `cable_connector`).
pub(super) fn victims(cx: &Context, cell: &SwitchingCell) -> Value {
    let mut own: BTreeSet<String> = BTreeSet::new();
    own.insert(cell.controller.clone());
    own.extend(cell.magnetic.iter().map(|m| m.component.clone()));
    own.extend(cell.catch_diode.iter().map(|d| d.component.clone()));
    own.extend(cell.bootstrap_capacitor.iter().map(|b| b.component.clone()));
    own.extend(cell.hot_capacitors.iter().map(|c| c.component.clone()));
    let names = |pads: Vec<PadGeom>| -> Vec<String> {
        let set: BTreeSet<String> = pads.iter().map(|p| format!("{}.{}", p.component, p.number)).collect();
        set.into_iter().collect()
    };
    let item = |kind: &str, name: &str, pads: Vec<String>| {
        let mut item = Object::new();
        put(&mut item, "kind", Value::from(kind));
        put(&mut item, "name", Value::from(name));
        put(&mut item, "pads", Value::Array(pads.iter().map(|p| Value::from(p.as_str())).collect()));
        Value::Object(item)
    };
    let mut list = Vec::new();
    for (net, classes) in &cx.model.nets {
        if !classes.sensitive || cell.switch_nets.contains(net) {
            continue;
        }
        let pads = names(cx.layout.pads_of_net(net).into_iter().filter(|p| !own.contains(&p.component)).collect());
        if !pads.is_empty() {
            list.push(item("sensitive_net", net, pads));
        }
    }
    for connector in &cx.connectors {
        let pads = names(cx.layout.pads_of_component(connector));
        if !pads.is_empty() {
            list.push(item("cable_connector", connector, pads));
        }
    }
    Value::Array(list)
}

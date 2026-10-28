//! Fresh exact terminal launch witnesses for the coarse capacity graph
//! (`pcbir.global_terminals`).

use indexmap::{IndexMap, IndexSet};

use copper_drc::digest::reprs;
use copper_pcb::Result;
use copper_pcb::board::{PadReference, PhysicalBoard, Via};
use copper_physical::{CopperLayer, Point};

use crate::routing::{AccessOptions, Graph, GridNode, PinAccess, Resource, edge_key, via_resource_id};
use crate::routing_clearance::RoutingClearanceIndex;

pub const MAX_TERMINAL_CAPACITY_PROPOSALS: i64 = 4096;
// Broad-phase reach for sampled witnesses that may collide with a terminal
// launch, beyond both via radii. The exact pairwise check decides.
const WITNESS_REACH_NM: i64 = 2_000_000;

type Span = (CopperLayer, CopperLayer);
type WitnessKey = (Span, Point);

/// `identifier.split(':')` of a via resource: `(low, high, x, y)`.
fn parse_via_identifier(identifier: &str) -> (i64, i64, i64, i64) {
    let parts: Vec<&str> = identifier.split(':').collect();
    let number = |index: usize| parts[index].parse::<i64>().expect("via identifier field");
    (number(1), number(2), number(3), number(4))
}

/// `str((from_layer, to_layer))`: the tuple's repr, for the collision sort key.
fn span_text(span: Span) -> String {
    format!("({}, {})", reprs::layer(span.0), reprs::layer(span.1))
}

fn pad_text(pad: &PadReference) -> String {
    format!(
        "PadReference(component={}, pad={})",
        copper_json::python_str_repr(&pad.component),
        copper_json::python_str_repr(&pad.pad)
    )
}

/// `register_terminal_capacity`: register exact terminal launches as capacity sites of the coarse graph.
///
/// Sampled via sites are capacity witnesses, never copper a route will place.
/// A terminal launch is real copper at its pad: when it collides only with
/// sampled witnesses it is registered and displaces them. `expand(key)` may
/// return a wider, still bounded candidate set for a pad left with no
/// admissible access.
pub fn register_terminal_capacity(
    board: &PhysicalBoard,
    graph: &Graph,
    access_options: AccessOptions,
    expand: Option<&dyn Fn(&(String, PadReference)) -> Result<Vec<PinAccess>>>,
) -> Result<(Graph, AccessOptions)> {
    let mut sites = graph.via_sites.clone();
    // Board copper, accepted terminal slots and witnesses in use: never displaced.
    let mut exact = RoutingClearanceIndex::new(board)?;
    let mut witnesses: IndexMap<WitnessKey, Via> = IndexMap::new();
    let mut owners: IndexMap<WitnessKey, Vec<String>> = IndexMap::new();
    let mut buckets: IndexMap<(i64, i64), Vec<WitnessKey>> = IndexMap::new();
    let mut identifiers: Vec<&String> = sites.keys().collect();
    identifiers.sort();
    for identifier in identifiers {
        let (low, high, _, _) = parse_via_identifier(identifier);
        let span = (graph.layers[low as usize], graph.layers[high as usize]);
        for point in &sites[identifier] {
            let key = (span, *point);
            owners.entry(key).or_default().push(identifier.clone());
            if !witnesses.contains_key(&key) {
                witnesses.insert(
                    key,
                    Via {
                        net: format!("<capacity-{}>", witnesses.len() + 1),
                        position: *point,
                        size_nm: board.rules.default_via_size_nm,
                        drill_nm: board.rules.default_via_drill_nm,
                        from_layer: span.0,
                        to_layer: span.1,
                        technology: None,
                        finish: "standard".to_owned(),
                    },
                );
                buckets
                    .entry((point.x_nm.div_euclid(WITNESS_REACH_NM), point.y_nm.div_euclid(WITNESS_REACH_NM)))
                    .or_default()
                    .push(key);
            }
        }
    }
    let mut protected: IndexSet<WitnessKey> = IndexSet::new();
    let mut proposals = 0;
    let mut changed: IndexSet<String> = IndexSet::new();

    struct Registry<'a, 'b> {
        exact: &'a mut RoutingClearanceIndex<'b>,
        witnesses: &'a mut IndexMap<WitnessKey, Via>,
        owners: &'a mut IndexMap<WitnessKey, Vec<String>>,
        buckets: &'a IndexMap<(i64, i64), Vec<WitnessKey>>,
        protected: &'a mut IndexSet<WitnessKey>,
        sites: &'a mut IndexMap<String, Vec<Point>>,
        proposals: &'a mut i64,
        changed: &'a mut IndexSet<String>,
    }

    impl Registry<'_, '_> {
        fn protect(&mut self, key: WitnessKey) -> Result<()> {
            if self.witnesses.contains_key(&key) && !self.protected.contains(&key) {
                self.protected.insert(key);
                self.exact.add_via(self.witnesses[&key].clone(), true, None)?;
            }
            Ok(())
        }

        fn colliding_witnesses(&self, via: &Via) -> Result<Vec<WitnessKey>> {
            let reach = WITNESS_REACH_NM;
            let (cx, cy) = (via.position.x_nm.div_euclid(reach), via.position.y_nm.div_euclid(reach));
            let mut found = Vec::new();
            for bx in [cx - 1, cx, cx + 1] {
                for by in [cy - 1, cy, cy + 1] {
                    for key in self.buckets.get(&(bx, by)).into_iter().flatten() {
                        let Some(witness) = self.witnesses.get(key) else { continue };
                        let point = key.1;
                        if (point.x_nm - via.position.x_nm).abs() > reach
                            || (point.y_nm - via.position.y_nm).abs() > reach
                        {
                            continue;
                        }
                        if !self.exact.candidate_vias_clear(&[via.clone(), witness.clone()])? {
                            found.push(*key);
                        }
                    }
                }
            }
            found.sort_by_key(|a| (a.1.x_nm, a.1.y_nm, span_text(a.0)));
            Ok(found)
        }

        fn admit(&mut self, graph: &Graph, access: &PinAccess) -> Result<bool> {
            let via = access.via.as_ref().expect("terminal via");
            let identifier = via_resource_id(graph, access.node, via.from_layer, via.to_layer);
            let (low, high, x, y) = parse_via_identifier(&identifier);
            let nodes = [GridNode::new(low, x, y), GridNode::new(high, x, y)];
            if nodes.iter().any(|node| !graph.legal_nodes.contains(node)) {
                return Ok(false);
            }
            if self.sites.get(&identifier).is_some_and(|positions| positions.contains(&via.position)) {
                // An existing sampled site is now real terminal copper.
                self.protect(((graph.layers[low as usize], graph.layers[high as usize]), via.position))?;
                return Ok(true);
            }
            if *self.proposals >= MAX_TERMINAL_CAPACITY_PROPOSALS {
                return Ok(false);
            }
            *self.proposals += 1;
            let capacity_via = Via { net: format!("<capacity-proposal-{}>", self.proposals), ..via.clone() };
            if !self.exact.can_via(
                &capacity_via.net,
                via.position,
                via.size_nm,
                via.from_layer,
                via.to_layer,
                Some(via.drill_nm),
                false,
                None,
            )? {
                return Ok(false);
            }
            for displaced in self.colliding_witnesses(&capacity_via)? {
                self.witnesses.shift_remove(&displaced);
                for owner in self.owners.shift_remove(&displaced).expect("witness owners") {
                    let kept: Vec<Point> =
                        self.sites[&owner].iter().copied().filter(|point| *point != displaced.1).collect();
                    self.sites.insert(owner.clone(), kept);
                    self.changed.insert(owner);
                }
            }
            self.sites.entry(identifier.clone()).or_default().push(via.position);
            self.exact.add_via(capacity_via, true, None)?;
            self.changed.insert(identifier);
            Ok(true)
        }
    }

    let mut registry = Registry {
        exact: &mut exact,
        witnesses: &mut witnesses,
        owners: &mut owners,
        buckets: &buckets,
        protected: &mut protected,
        sites: &mut sites,
        proposals: &mut proposals,
        changed: &mut changed,
    };
    // Two passes in the same deterministic pad order: first each pad's first
    // admissible launch, then the remaining alternatives. Otherwise a pad
    // earlier in the order can take every launch site near a neighbour.
    let mut ordered: Vec<(&(String, PadReference), &Vec<PinAccess>)> = access_options.iter().collect();
    ordered.sort_by(|a, b| (&a.0.0, pad_text(&a.0.1)).cmp(&(&b.0.0, pad_text(&b.0.1))));
    let mut decided: Vec<Vec<Option<bool>>> =
        ordered.iter().map(|(_, choices)| choices.iter().map(|a| a.via.is_none().then_some(true)).collect()).collect();
    for (index, (_, choices)) in ordered.iter().enumerate() {
        for (slot, access) in choices.iter().enumerate() {
            if decided[index][slot].is_none() {
                let admitted = registry.admit(graph, access)?;
                decided[index][slot] = Some(admitted);
                if admitted {
                    break;
                }
            }
        }
    }
    for (index, (_, choices)) in ordered.iter().enumerate() {
        for (slot, access) in choices.iter().enumerate() {
            if decided[index][slot].is_none() {
                decided[index][slot] = Some(registry.admit(graph, access)?);
            }
        }
    }
    let mut accepted: AccessOptions = ordered
        .iter()
        .enumerate()
        .map(|(index, (key, choices))| {
            let kept: Vec<PinAccess> = choices
                .iter()
                .enumerate()
                .filter(|(slot, _)| decided[index][*slot] == Some(true))
                .map(|(_, a)| a.clone())
                .collect();
            ((*key).clone(), kept)
        })
        .collect();
    if let Some(expand) = expand {
        // Fine-pitch neighbours can claim every nearby launch of a pad first.
        // Only a pad left with nothing tries its wider set, one launch each.
        for (key, choices) in &ordered {
            if !accepted[*key].is_empty() || choices.is_empty() {
                continue;
            }
            let tried: Vec<(GridNode, Option<Via>)> = choices.iter().map(|a| (a.node, a.via.clone())).collect();
            for access in expand(key)? {
                if tried.contains(&(access.node, access.via.clone())) {
                    continue;
                }
                if access.via.is_none() || registry.admit(graph, &access)? {
                    accepted.insert((*key).clone(), vec![access]);
                    break;
                }
            }
        }
    }
    let mut resources = graph.resources.clone();
    let mut changed_sorted: Vec<String> = changed.iter().cloned().collect();
    changed_sorted.sort();
    for identifier in changed_sorted {
        let (low, high, x, y) = parse_via_identifier(&identifier);
        let capacity = sites[&identifier].len() as i64;
        // A through launch contacts all declared copper layers in its span.
        // Only already-existing legal graph nodes receive alias edges.
        for first in low..high {
            for second in first + 1..=high {
                let (a, b) = (GridNode::new(first, x, y), GridNode::new(second, x, y));
                if !graph.legal_nodes.contains(&a) || !graph.legal_nodes.contains(&b) {
                    continue;
                }
                let key = edge_key(a, b);
                if capacity != 0 {
                    resources.insert(
                        key,
                        Resource { identifier: identifier.clone(), first: a, second: b, capacity, layer: None },
                    );
                } else if resources.get(&key).is_some_and(|resource| resource.identifier == identifier) {
                    resources.shift_remove(&key);
                }
            }
        }
    }
    Ok((graph.with(resources, sites), accepted))
}

/// `terminal_access_failures`: `(generated, registered)` counts of every empty terminal.
///
/// A missing local escape and a legal escape rejected by shared transition
/// capacity need different repairs; both counts are preserved.
pub fn terminal_access_failures(
    generated: &AccessOptions,
    registered: &AccessOptions,
) -> IndexMap<(String, PadReference), (i64, i64)> {
    generated
        .iter()
        .filter(|(key, _)| registered.get(*key).is_none_or(|choices| choices.is_empty()))
        .map(|(key, choices)| (key.clone(), (choices.len() as i64, registered.get(key).map_or(0, |c| c.len() as i64))))
        .collect()
}

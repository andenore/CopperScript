//! The router's search: net routes over the shared lattice (`_route_net`,
//! `_search`, `_astar_native`) and their claims (`_finish_route`).

use crate::routing_lattice::Lattice;
use indexmap::{IndexMap, IndexSet};

use copper_physical::pyfloat::hypot;

use super::astar::{FLAG_NECK, FLAG_NECK_MID, FLAG_REGION, FLAG_TARGET, NestedTables, Scratch, SearchInput, search};
use super::geometry::{disc_offsets, segment_offsets};
use super::rasters::FOREIGN;
use super::router::{Attach, Lead, NetRoute, NetSpec, Router};
use crate::routing_lattice::{IndexBox, MOVES, NestedLattice};
use crate::unified_error::{Result, not_implemented};
use copper_physical::Point;

/// Python's `round(radius, 6)` as a cache key.
fn radius_key(radius: f64) -> u64 {
    copper_physical::pyfloat::round_digits(radius, 6).to_bits()
}

thread_local! {
    static OFFSETS: std::cell::RefCell<IndexMap<(char, i64, i64, u64), std::rc::Rc<Vec<(i64, i64)>>>> =
        std::cell::RefCell::new(IndexMap::new());
}

/// `_segment_offsets_cached`.
pub fn segment_offsets_cached(dx: i64, dy: i64, radius: f64) -> std::rc::Rc<Vec<(i64, i64)>> {
    OFFSETS.with(|cache| {
        cache
            .borrow_mut()
            .entry(('s', dx, dy, radius_key(radius)))
            .or_insert_with(|| std::rc::Rc::new(segment_offsets(dx, dy, radius)))
            .clone()
    })
}

/// `_disc_offsets_cached`.
pub fn disc_offsets_cached(radius: f64) -> std::rc::Rc<Vec<(i64, i64)>> {
    OFFSETS.with(|cache| {
        cache
            .borrow_mut()
            .entry(('d', 0, 0, radius_key(radius)))
            .or_insert_with(|| std::rc::Rc::new(disc_offsets(radius)))
            .clone()
    })
}

/// What a search targets: candidate nodes (in order), and per target the
/// extra cost of ending there.
pub struct Targets<'a> {
    pub nodes: &'a [i64],
    pub extra: Option<&'a IndexMap<i64, f64>>,
}

impl Router<'_, '_> {
    /// `_lead_penalty`: congestion price of an off-grid lead-in.
    pub fn lead_penalty(&mut self, lead: &Lead, present: f64) -> f64 {
        if lead.is_empty() || self.ignore_soft_costs {
            return 0.0;
        }
        if !self.lead_nodes.contains_key(lead) {
            let g = &self.lattice;
            let mut nodes = Vec::new();
            for track in lead {
                let layer = g.layers.iter().position(|l| *l == track.layer).expect("lead layer") as i64;
                match g.as_nested() {
                    // Nodes along the lead in their own pitch, each weighted
                    // by that pitch in base pitches (a price per base pitch).
                    Some(nested) => nodes.extend(
                        nested
                            .segment_nodes_local(layer, track.start, track.end, 0.5, 1.0)
                            .into_iter()
                            .map(|(node, _, weight)| (node, weight)),
                    ),
                    None => nodes.extend(
                        g.segment_nodes(layer, track.start, track.end, g.pitch as f64 / 2.0 + 1.0)
                            .into_iter()
                            .map(|(node, _)| (node, 1.0)),
                    ),
                }
            }
            self.lead_nodes.insert(lead.clone(), nodes);
        }
        let nodes = &self.lead_nodes[lead];
        let exact =
            copper_physical::pyfloat::sum(lead.iter().map(|t| self.lead_history.get(t).copied().unwrap_or(0.0)));
        if nodes.is_empty() {
            return exact;
        }
        if self.lattice.is_nested() {
            let (hist, occ) = (&self.occupancy.history, &self.occupancy.occ);
            return copper_physical::pyfloat::sum(
                nodes.iter().map(|&(n, weight)| weight * (hist[n as usize] + present * occ[n as usize] as f64)),
            );
        }
        // numpy: hist[index].sum() + present * occ[index].sum()
        let hist = numpy_sum(nodes.iter().map(|&(n, _)| self.occupancy.history[n as usize]));
        let occ: i64 = nodes.iter().map(|&(n, _)| self.occupancy.occ[n as usize] as i64).sum();
        exact + (hist + present * occ as f64)
    }

    /// `_margin`: the search window margin in pitches.
    pub fn margin(&self, spec: &NetSpec) -> i64 {
        let age = self.conflict_age.get(&spec.name).copied().unwrap_or(0);
        let scale = self.options.window_growth_limit.min(1 + age.div_euclid(3));
        (self.options.window_margin_nm * scale).div_euclid(self.lattice.pitch)
    }

    /// `_route_length_nm`.
    pub fn route_length_nm(&self, route: &NetRoute) -> f64 {
        let leads = copper_physical::pyfloat::sum(route.leads.iter().flat_map(|lead| {
            lead.iter().map(|t| hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64))
        }));
        route.length * self.pitch as f64 + leads
    }

    /// `_route_net`.
    pub fn route_net(&mut self, index: usize, present: f64) -> Result<NetRoute> {
        let mut route = self.route_net_budgeted(index, present)?;
        let max_length = self.specs[index].max_length;
        if self.options.length_priority
            && max_length.is_some()
            && present != 0.0
            && route.connected
            && self.route_length_nm(&route) > max_length.expect("length") as f64
        {
            let direct = self.route_net_budgeted(index, 0.0)?;
            if direct.connected && self.route_length_nm(&direct) < self.route_length_nm(&route) {
                route = direct;
            }
        }
        if !route.connected && self.expired() {
            route.reason = "time budget exhausted".to_owned();
        } else if !route.connected
            && self.specs[index].layers.len() > 1
            && !self.board.stackup.via_technologies.is_empty()
        {
            return not_implemented("unified router via technologies");
        }
        Ok(route)
    }

    fn route_net_budgeted(&mut self, index: usize, present: f64) -> Result<NetRoute> {
        let mut route = self.route_net_once(index, present)?;
        let Some(budget) = self.specs[index].via_budget else { return Ok(route) };
        if route.via_xy.len() as i64 > budget {
            let factor = self.specs[index].via_factor;
            for step in [1.0, 10.0, 100.0, 1000.0] {
                self.specs[index].via_factor = factor * (self.options.via_overrun_factor * step);
                let retry = self.route_net_once(index, present)?;
                if retry.connected && retry.via_xy.len() < route.via_xy.len() {
                    route = retry;
                }
                if route.connected && route.via_xy.len() as i64 <= budget {
                    break;
                }
            }
            if route.via_xy.len() as i64 > budget {
                self.specs[index].via_factor = factor * self.options.via_overrun_factor;
                self.ignore_soft_costs = true;
                let direct = self.route_net_once(index, 0.0);
                self.ignore_soft_costs = false;
                let direct = direct?;
                if direct.connected && direct.via_xy.len() as i64 <= budget {
                    route = direct;
                }
            }
            self.specs[index].via_factor = factor;
            if route.via_xy.len() as i64 > budget {
                route =
                    NetRoute::empty(false, &format!("needs {} new vias, max_vias leaves {budget}", route.via_xy.len()));
            }
        }
        Ok(route)
    }

    fn route_net_once(&mut self, index: usize, present: f64) -> Result<NetRoute> {
        if !self.specs[index].plane.is_empty() {
            return self.route_plane_net(index, present);
        }
        let components: Vec<Vec<Attach>> = self.specs[index].components.clone();
        let mut route = NetRoute::empty(false, "");
        if components.len() < 2 {
            route.connected = true;
            return Ok(route);
        }
        if components.iter().any(Vec::is_empty) {
            let missing = components.iter().filter(|c| c.is_empty()).count();
            route.reason = format!("no legal terminal access for {missing} of {} pad groups", components.len());
            return Ok(route);
        }
        let mut tree: IndexMap<i64, f64> = IndexMap::new();
        let mut leads: IndexMap<i64, Lead> = IndexMap::new();
        for (node, lead, cost) in &components[0] {
            let penalty = self.lead_penalty(lead, present);
            tree.insert(*node, cost + penalty);
            leads.insert(*node, lead.clone());
        }
        let mut used_leads: IndexSet<Lead> = IndexSet::new();
        let mut remaining: Vec<usize> = (1..components.len()).collect();
        let base = self.lattice.point(*tree.keys().next().expect("first component"));
        remaining.sort_by_key(|&ci| {
            let here = self.lattice.point(components[ci][0].0);
            (here.x_nm - base.x_nm).abs() + (here.y_nm - base.y_nm).abs()
        });
        while !remaining.is_empty() {
            let mut targets: IndexMap<i64, (usize, Lead, f64)> = IndexMap::new();
            for &ci in &remaining {
                for (node, lead, cost) in &components[ci] {
                    targets.insert(*node, (ci, lead.clone(), *cost));
                }
            }
            let mut extra: IndexMap<i64, f64> = IndexMap::new();
            let keys: Vec<i64> = targets.keys().copied().collect();
            for node in &keys {
                let (_, lead, cost) = targets[node].clone();
                let penalty = self.lead_penalty(&lead, present);
                extra.insert(*node, cost + penalty);
            }
            let sources: Vec<(i64, f64)> = tree.iter().map(|(n, c)| (*n, *c)).collect();
            let found = self.search(index, &sources, Targets { nodes: &keys, extra: Some(&extra) }, present)?;
            let Some((path, end_node)) = found else {
                route.reason = format!("no path to {} remaining pad groups", remaining.len());
                route.connected = false;
                self.finish_route(&mut route, index);
                return Ok(route);
            };
            let (ci, lead, _) = targets[&end_node].clone();
            let mut copper: Lead = Vec::new();
            let start_lead = leads.get(&path[0]).cloned().unwrap_or_default();
            if !start_lead.is_empty() && !used_leads.contains(&start_lead) {
                used_leads.insert(start_lead.clone());
                copper.extend(start_lead);
            }
            if !lead.is_empty() && !used_leads.contains(&lead) {
                used_leads.insert(lead.clone());
                copper.extend(lead);
            }
            for &node in &path {
                tree.insert(node, 0.0);
            }
            route.paths.push(path);
            route.leads.push(copper);
            for (node, lead2, cost) in &components[ci] {
                if !tree.contains_key(node) {
                    let penalty = self.lead_penalty(lead2, present);
                    tree.insert(*node, cost + penalty);
                    leads.insert(*node, lead2.clone());
                }
            }
            remaining.retain(|&r| r != ci);
        }
        route.connected = true;
        self.finish_route(&mut route, index);
        Ok(route)
    }

    /// `_route_plane_net`: join every pad group (and small pour) of a zone net to its plane.
    fn route_plane_net(&mut self, index: usize, present: f64) -> Result<NetRoute> {
        let spec = self.specs[index].clone();
        let mut route = NetRoute::empty(false, "");
        let mut targets: IndexSet<i64> = spec.plane.clone();
        let mut missing = 0;
        let whole = self.lattice.full_box();
        let margin = self.margin(&spec);
        let count = spec.components.len();
        let first_island = count - spec.islands;
        let first_optional = count - spec.optional;
        let order: Vec<usize> = (first_island..count).chain(0..first_island).collect();
        for position in order {
            let component = &spec.components[position];
            let optional = position >= first_optional;
            if component.is_empty() {
                missing += (!optional) as usize;
                continue;
            }
            let excluded = spec.explicit.get(&position);
            if excluded.is_none()
                && (component.iter().any(|(node, lead, _)| targets.contains(node) && lead.is_empty())
                    || spec.joined.get(&position).is_some_and(|joined| joined.iter().any(|n| targets.contains(n))))
            {
                if position >= first_island {
                    targets.extend(component.iter().map(|(node, _, _)| *node));
                }
                continue;
            }
            let mut sources: IndexMap<i64, f64> = IndexMap::new();
            let mut leads: IndexMap<i64, Lead> = IndexMap::new();
            for (node, lead, cost) in component {
                let penalty = self.lead_penalty(lead, present);
                sources.insert(*node, cost + penalty);
                leads.insert(*node, lead.clone());
            }
            let source_list: Vec<(i64, f64)> = sources.iter().map(|(n, c)| (*n, *c)).collect();
            if let Some(excluded) = excluded {
                let allowed: Vec<i64> = targets
                    .iter()
                    .copied()
                    .filter(|n| !excluded.contains(&(self.lattice.layer_of(*n) as usize)))
                    .collect();
                let found = if allowed.is_empty() {
                    None
                } else {
                    self.astar(index, &source_list, &Targets { nodes: &allowed, extra: None }, present, whole, whole)?
                };
                let Some((path, _)) = found else {
                    missing += (!optional) as usize;
                    continue;
                };
                route.leads.push(leads.get(&path[0]).cloned().unwrap_or_default());
                targets.extend(path.iter().copied());
                route.paths.push(path);
                continue;
            }
            let bbox = self
                .lattice
                .grow_box(self.lattice.box_of(&sources.keys().copied().collect::<Vec<_>>()).expect("sources"), margin);
            let target_list: Vec<i64> = targets.iter().copied().collect();
            let mut found = if target_list.is_empty() {
                None
            } else {
                self.astar(index, &source_list, &Targets { nodes: &target_list, extra: None }, present, bbox, whole)?
            };
            if found.is_none() && !target_list.is_empty() {
                found = self.astar(
                    index,
                    &source_list,
                    &Targets { nodes: &target_list, extra: None },
                    present,
                    whole,
                    whole,
                )?;
            }
            let Some((path, _)) = found else {
                missing += (!optional) as usize;
                continue;
            };
            route.leads.push(leads.get(&path[0]).cloned().unwrap_or_default());
            targets.extend(path.iter().copied());
            route.paths.push(path);
            if position >= first_island {
                targets.extend(sources.keys().copied());
            }
        }
        route.connected = missing == 0;
        if missing != 0 {
            route.reason = format!("{missing} of {} pad groups cannot reach the plane", count - spec.optional);
        }
        self.finish_route(&mut route, index);
        Ok(route)
    }

    /// `_search`: the escape-biased search.
    pub fn search(
        &mut self,
        index: usize,
        sources: &[(i64, f64)],
        targets: Targets<'_>,
        present: f64,
    ) -> Result<Option<(Vec<i64>, i64)>> {
        if self.expired() {
            return Ok(None);
        }
        let name = self.specs[index].name.clone();
        let Some((nodes, amount)) = self.escape_bias(&name) else {
            return self.search_unbiased(index, sources, targets, present);
        };
        let saved: Vec<f64> = nodes.iter().map(|&n| self.occupancy.history[n as usize]).collect();
        for (&node, &add) in nodes.iter().zip(&amount) {
            self.occupancy.history[node as usize] += add;
        }
        let result = self.search_unbiased(index, sources, targets, present);
        for (&node, &value) in nodes.iter().zip(&saved) {
            self.occupancy.history[node as usize] = value;
        }
        result
    }

    /// `_escape_bias`: (nodes, extra history) of the net's escape-plan biases.
    fn escape_bias(&mut self, net: &str) -> Option<(Vec<i64>, Vec<f64>)> {
        if let Some(cached) = self.escape_bias_cache.get(net) {
            return cached.clone();
        }
        let g = &self.lattice;
        let mut extra: IndexMap<i64, f64> = IndexMap::new();
        for &node in self.exit_corridors.get(net).into_iter().flatten() {
            *extra.entry(node).or_insert(0.0) -= self.options.exit_reservation_cost;
        }
        for bias in &self.options.escape_layer_biases {
            if bias.net != net || !g.layers.contains(&bias.layer) || bias.rate == 0 {
                continue;
            }
            let layer = g.layers.iter().position(|l| *l == bias.layer).expect("layer") as i64;
            let area = &bias.area;
            for node in g.nodes_in(layer, area.min_x as f64, area.min_y as f64, area.max_x as f64, area.max_y as f64) {
                *extra.entry(node).or_insert(0.0) += bias.rate as f64 / 10.0;
            }
        }
        let result = if extra.is_empty() {
            None
        } else {
            let mut nodes: Vec<i64> = extra.keys().copied().collect();
            nodes.sort_unstable();
            let amounts = nodes.iter().map(|n| extra[n]).collect();
            Some((nodes, amounts))
        };
        self.escape_bias_cache.insert(net.to_owned(), result.clone());
        result
    }

    fn search_unbiased(
        &mut self,
        index: usize,
        sources: &[(i64, f64)],
        targets: Targets<'_>,
        present: f64,
    ) -> Result<Option<(Vec<i64>, i64)>> {
        let g = &self.lattice;
        let tbox = g.box_of(targets.nodes).expect("targets");
        let (sx0, sy0, sx1, sy1) = g.box_of(&sources.iter().map(|s| s.0).collect::<Vec<_>>()).expect("sources");
        let margin = self.margin(&self.specs[index]);
        let mut bbox = g.grow_box((tbox.0.min(sx0), tbox.1.min(sy0), tbox.2.max(sx1), tbox.3.max(sy1)), margin);
        let whole = g.full_box();
        if !self.guides.is_empty() {
            let name = self.specs[index].name.clone();
            if let Some(corridor) = self.guide_corridor(&name)? {
                // A guided net's window covers its corridor, so the soft guide
                // cost can be followed where the guide detours.
                let (cx0, cy0, cx1, cy1) = corridor.bounds;
                bbox = (bbox.0.min(cx0), bbox.1.min(cy0), bbox.2.max(cx1), bbox.3.max(cy1));
            }
        }
        for attempt in 0..3 {
            let result = self.astar(index, sources, &targets, present, bbox, tbox)?;
            if result.is_some() {
                return Ok(result);
            }
            if bbox == whole {
                return Ok(None);
            }
            bbox = self.lattice.grow_box(bbox, margin * (attempt + 2) * 2);
            if attempt == 1 {
                bbox = whole;
            }
        }
        Ok(None)
    }

    /// `_move_base`: heading-adjusted planar move costs `[layer][move]`.
    fn move_base(&mut self) -> Vec<f64> {
        if let Some(cache) = &self.move_cache {
            return cache.clone();
        }
        let mut table = Vec::new();
        if let Some(g) = self.lattice.as_nested() {
            // [block][layer][move]: the costs of `_nested_search_tables`.
            for block in 0..g.blocks.len() {
                for layer in 0..g.nl as usize {
                    let head = self.heading[layer];
                    let lc = self.layer_cost[layer];
                    for &(dx, dy, _, _, step) in g.block_moves(block) {
                        let mut base = step * (1.0 + lc);
                        if let Some(head) = head
                            && ((head == "h" && dy != 0 && dx == 0) || (head == "n" && dx != 0 && dy == 0))
                        {
                            base += self.options.wrong_way_cost * step;
                        }
                        table.push(base);
                    }
                }
            }
            self.move_cache = Some(table.clone());
            return table;
        }
        for layer in 0..self.lattice.nl as usize {
            let head = self.heading[layer];
            let lc = self.layer_cost[layer];
            for (dx, dy, step) in MOVES {
                let mut base = step * (1.0 + lc);
                if let Some(head) = head
                    && ((head == "h" && dy != 0 && dx == 0) || (head == "n" && dx != 0 && dy == 0))
                {
                    base += self.options.wrong_way_cost * step;
                }
                table.push(base);
            }
        }
        self.move_cache = Some(table.clone());
        table
    }

    /// `_turn_table`: bend_cost times the 45-degree turns from move a to move b.
    fn turn_table(&mut self) -> Vec<f64> {
        if let Some(cache) = &self.turn_cache {
            return cache.clone();
        }
        let headings = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
        let index: Vec<i64> = MOVES
            .iter()
            .map(|(dx, dy, _)| headings.iter().position(|h| *h == (*dx, *dy)).expect("heading") as i64)
            .collect();
        let bend = self.options.bend_cost;
        let mut table = Vec::new();
        for &a in &index {
            for &b in &index {
                table.push(bend * (a - b).abs().min(8 - (a - b).abs()) as f64);
            }
        }
        self.turn_cache = Some(table.clone());
        table
    }

    /// `_via_owners`: the via raster; a single-ended net that requires return
    /// vias may propose a via only within reach of an existing return barrel
    /// (a proposal: exact checks repeat at commit and in DRC).
    fn via_owner(&mut self, index: usize) -> Result<Vec<i32>> {
        let spec = &self.specs[index];
        let (size, drill, clearance) = (spec.via_size, spec.via_drill, spec.clearance);
        let rule = match self.rules.get(&spec.name) {
            Some(rule) if rule.require_return_vias && rule.differential_partner.is_none() => rule.clone(),
            _ => return Ok(self.rasters.via(size, drill, clearance, None).to_vec()),
        };
        if let Some(cached) = self.return_via_owners.get(&spec.name) {
            return Ok(cached.clone());
        }
        let name = spec.name.clone();
        let layers = &self.lattice.layers;
        let (low, high) = (layers[0], layers[layers.len() - 1]);
        let mut masks: Vec<Vec<bool>> = self.rasters.views.iter().map(|view| vec![false; view.px.len()]).collect();
        for via in &self.board.vias {
            if rule.return_via_net.as_deref() != Some(via.net.as_str())
                || via.net == name
                || !crate::return_paths::return_span_covers(self.board, via, low, high)
            {
                continue;
            }
            let Some(limit) = rule.maximum_return_via_distance_nm else {
                return not_implemented("required return vias without a maximum distance (Python crashes)");
            };
            let (x, y) = (via.position.x_nm, via.position.y_nm);
            for (view, mask) in self.rasters.views.iter().zip(&mut masks) {
                let block = &view.block;
                let (x0, y0, x1, y1) =
                    block.window((x - limit) as f64, (y - limit) as f64, (x + limit) as f64, (y + limit) as f64);
                if x0 > x1 || y0 > y1 {
                    continue;
                }
                for iy in y0..=y1 {
                    for ix in x0..=x1 {
                        let at = (iy * block.nx + ix) as usize;
                        if (view.px[at] - x as f64).hypot(view.py[at] - y as f64) <= limit as f64 {
                            mask[at] = true;
                        }
                    }
                }
            }
        }
        let mut owners = self.rasters.via(size, drill, clearance, None).to_vec();
        for (owner, inside) in owners.iter_mut().zip(masks.iter().flatten()) {
            if !inside {
                *owner = FOREIGN;
            }
        }
        self.return_via_owners.insert(name, owners.clone());
        Ok(owners)
    }

    /// `_astar` through the native kernel's semantics.
    fn astar(
        &mut self,
        index: usize,
        sources: &[(i64, f64)],
        targets: &Targets<'_>,
        present: f64,
        bbox: IndexBox,
        tbox: IndexBox,
    ) -> Result<Option<(Vec<i64>, i64)>> {
        if self.expired() {
            return Ok(None);
        }
        let spec = self.specs[index].clone();
        let (width, clearance) = (spec.width, spec.clearance);
        let owner = self.rasters.track(width, clearance).0.to_vec();
        let mid_owner = self.rasters.track(width, clearance).1.to_vec();
        let via_owner = self.via_owner(index)?;
        if !self.board.stackup.via_technologies.is_empty() {
            return not_implemented("unified router via technologies");
        }
        let layers: Vec<i32> =
            spec.layers.iter().map(|&l| l as i32 | if spec.target_only.contains(&l) { 0x100 } else { 0 }).collect();
        let move_base = self.move_base();
        let turn = if self.options.bend_cost != 0.0 { Some(self.turn_table()) } else { None };
        // Per node, guide_cost outside the searched net's corridor (`_n_guide`).
        let corridor = if self.guides.is_empty() { None } else { self.guide_corridor(&spec.name)? };
        if let Some(corridor) = &corridor {
            let size = self.lattice.size as usize;
            let cost = self.options.guide_cost;
            let guide = self.guide_nodes.get_or_insert_with(|| vec![cost; size]);
            for &node in &corridor.nodes {
                guide[node as usize] = 0.0;
            }
        }
        let marks: Vec<(Vec<i64>, u8)> = vec![
            (spec.neck.iter().copied().collect(), FLAG_NECK),
            (spec.neck_mid.iter().copied().collect(), FLAG_NECK_MID),
            (spec.region.iter().copied().collect(), FLAG_REGION),
            (targets.nodes.to_vec(), FLAG_TARGET),
        ];
        for (nodes, bit) in &marks {
            for &node in nodes {
                self.flags[node as usize] |= bit;
            }
        }
        if let Some(extra) = targets.extra {
            for (&node, &cost) in extra {
                self.extra[node as usize] = cost;
            }
        }
        if self.lattice.is_nested() && self.kernel_tables.is_none() {
            let tables = self.lattice.as_nested().expect("nested lattice").kernel_tables();
            self.kernel_tables = Some(tables.map_err(|error| match error {
                copper_pcb::Error::Value(message) => crate::unified_error::Error::Value(message),
                copper_pcb::Error::NotImplemented(what) => crate::unified_error::Error::NotImplemented(what),
            })?);
        }
        let zero_history;
        let (history, via_history) = if self.ignore_soft_costs {
            zero_history = (vec![0.0; self.occupancy.history.len()], vec![0.0; self.occupancy.via_history.len()]);
            (&zero_history.0, &zero_history.1)
        } else {
            (&self.occupancy.history, &self.occupancy.via_history)
        };
        let mut input = SearchInput {
            nx: self.lattice.nx,
            ny: self.lattice.ny,
            nl: self.lattice.nl as usize,
            owner: &owner,
            mid_owner: &mid_owner,
            via_owner: &via_owner,
            hist: history,
            occ: &self.occupancy.occ,
            via_hist: via_history,
            via_occ: &self.occupancy.via_occ,
            flags: &self.flags,
            nid: spec.nid,
            layers: &layers,
            move_base: &move_base,
            present,
            via_cost: self.via_cost * spec.via_factor,
            bbox,
            tbox,
            sources,
            target_extra: targets.extra.map(|_| &self.extra[..]),
            turn: turn.as_deref(),
            guide: corridor.as_ref().and(self.guide_nodes.as_deref()),
            via_transitions: None,
            max_expansions: self.options.maximum_expansions,
            nested: None,
        };
        input.nested = self.kernel_tables.as_ref().map(|(blocks, block_of, ux, uy, twins, steps)| NestedTables {
            blocks,
            block_of,
            ux,
            uy,
            twins,
            steps,
        });
        let scratch: &mut Scratch = &mut self.scratch;
        let path = search(&input, scratch);
        for (nodes, _) in &marks {
            for &node in nodes {
                self.flags[node as usize] = 0;
            }
        }
        if let Some(extra) = targets.extra {
            for &node in extra.keys() {
                self.extra[node as usize] = 0.0;
            }
        }
        if let (Some(corridor), Some(guide)) = (&corridor, self.guide_nodes.as_mut()) {
            for &node in &corridor.nodes {
                guide[node as usize] = self.options.guide_cost;
            }
        }
        Ok(path.map(|path| {
            let end = *path.last().expect("path");
            (path, end)
        }))
    }

    /// `_finish_route`: nodes, claims and length of a route.
    pub fn finish_route(&mut self, route: &mut NetRoute, index: usize) {
        if self.lattice.is_nested() {
            self.finish_route_nested(route, index);
            return;
        }
        let spec = &self.specs[index];
        let g = &self.lattice;
        let pitch = g.pitch as f64;
        let mut nodes: IndexSet<i64> = IndexSet::new();
        let mut claims: IndexSet<i64> = IndexSet::new();
        let mut via_claims: IndexSet<i64> = IndexSet::new();
        let mut body: IndexMap<i64, i64> = IndexMap::new();
        let default_width = self.default_width as f64;
        let via_body = (spec.via_size as f64 - default_width) / 2.0 / pitch;
        let neighbour = self.default_width.max(spec.width) as f64;
        let track_radius = (spec.width as f64 / 2.0 + spec.clearance as f64 + neighbour / 2.0) / pitch;
        let hole = self.board.rules.minimum_hole_clearance_nm as f64;
        let via_reach = (spec.via_size as f64 / 2.0 + spec.clearance as f64).max(spec.via_drill as f64 / 2.0 + hole);
        let via_radius = (via_reach + default_width / 2.0) / pitch;
        let via_via_radius = (via_reach + spec.via_size as f64 / 2.0) / pitch;
        let prospective_reach = (self.candidate_via_size as f64 / 2.0
            + spec.clearance.max(self.candidate_clearance) as f64)
            .max(self.candidate_via_drill as f64 / 2.0 + hole);
        let track_via_radius = (prospective_reach + spec.width as f64 / 2.0) / pitch;
        let mut length = 0.0;
        let mut changes: IndexMap<i64, (i64, i64)> = IndexMap::new();
        for path in &route.paths {
            for pair in path.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (la, lb) = (g.layer_of(a), g.layer_of(b));
                if la != lb {
                    let xy = g.column(a);
                    let (low, high) = changes.get(&xy).copied().unwrap_or((la, la));
                    changes.insert(xy, (low.min(la).min(lb), high.max(la).max(lb)));
                    continue;
                }
                let (dx, dy) = g.delta(a, b);
                length += hypot(dx as f64, dy as f64);
                claims.extend(g.offset_nodes(a, &segment_offsets_cached(dx, dy, track_radius)));
                via_claims.extend(g.offset_columns(g.column(a), &segment_offsets_cached(dx, dy, track_via_radius)));
            }
            nodes.extend(path.iter().copied());
            if path.len() == 1 {
                claims.extend(g.offset_nodes(path[0], &disc_offsets_cached(track_radius)));
            }
        }
        for leads in &route.leads {
            for track in leads {
                let layer = g.layers.iter().position(|l| *l == track.layer).expect("lead layer") as i64;
                let radius_nm = track.width_nm as f64 / 2.0
                    + spec.clearance as f64
                    + self.default_width.max(track.width_nm) as f64 / 2.0;
                let near = pitch / 2.0;
                for (node, distance) in g.segment_nodes(layer, track.start, track.end, radius_nm) {
                    claims.insert(node);
                    if distance <= near {
                        nodes.insert(node);
                    }
                }
                for (node, _) in
                    g.segment_nodes(layer, track.start, track.end, prospective_reach + track.width_nm as f64 / 2.0)
                {
                    via_claims.insert(node.rem_euclid(g.plane));
                }
            }
        }
        let via_xy: IndexSet<i64> = changes.keys().copied().collect();
        // Without via technologies every site is a through via.
        let nl = g.nl;
        for &xy in changes.keys() {
            if via_body >= 0.5 {
                for column in g.offset_columns(xy, &disc_offsets_cached(via_body)) {
                    for layer in 0..nl {
                        body.insert(g.layer_node(column, layer), xy);
                    }
                }
            }
            for column in g.offset_columns(xy, &disc_offsets_cached(via_radius)) {
                for layer in 0..nl {
                    claims.insert(g.layer_node(column, layer));
                }
            }
            via_claims.extend(g.offset_columns(xy, &disc_offsets_cached(via_via_radius)));
        }
        route.body = body.into_iter().filter(|(node, _)| !nodes.contains(node)).collect();
        route.nodes = nodes;
        route.via_xy = via_xy;
        route.claims = claims;
        route.via_claims = via_claims;
        route.length = length;
    }
}

// Nested-lattice claims: a claim disc (or a step's capsule) that lies
// wholly in its node's own block uses that block's offset tables, the
// radius scaled by the block's factor; one reaching across a patch
// boundary is collected geometrically from every block it overlaps.
// Portal twins are claimed together (they are one site).

/// `_nested_step_claims`.
fn nested_step_claims(g: &NestedLattice, pitch: i64, a: i64, b: i64, pa: Point, pb: Point, radius: f64) -> Vec<i64> {
    let block = &g.blocks[g.block_index(a.rem_euclid(g.plane))];
    let r = radius * pitch as f64;
    if g.owns(
        block,
        pa.x_nm.min(pb.x_nm) as f64 - r,
        pa.y_nm.min(pb.y_nm) as f64 - r,
        pa.x_nm.max(pb.x_nm) as f64 + r,
        pa.y_nm.max(pb.y_nm) as f64 + r,
    ) {
        let (dx, dy) = g.delta(a, b);
        return g.offset_nodes(a, &segment_offsets_cached(dx, dy, radius * block.factor as f64));
    }
    let mut nodes: Vec<i64> = g.segment_nodes(a.div_euclid(g.plane), pa, pb, r).into_iter().map(|(n, _)| n).collect();
    nodes.extend([a, b]);
    nodes
}

/// `_nested_disc_columns`.
fn nested_disc_columns(g: &NestedLattice, pitch: i64, column: i64, radius: f64) -> Vec<i64> {
    let block = &g.blocks[g.block_index(column)];
    let point = g.column_point(column);
    let r = radius * pitch as f64;
    let (x, y) = (point.x_nm as f64, point.y_nm as f64);
    if g.owns(block, x - r, y - r, x + r, y + r) {
        return g.offset_columns(column, &disc_offsets_cached(radius * block.factor as f64));
    }
    // np.union1d: sorted and distinct.
    let mut columns = g.columns_within(point, r);
    columns.push(column);
    columns.sort_unstable();
    columns.dedup();
    columns
}

impl Router<'_, '_> {
    /// `_finish_route_nested`: `_finish_route` on a nested lattice (lengths in base pitches).
    fn finish_route_nested(&mut self, route: &mut NetRoute, index: usize) {
        let spec = &self.specs[index];
        let g = self.lattice.as_nested().expect("nested lattice");
        let (plane, pitch) = (g.plane, self.pitch);
        let pf = pitch as f64;
        let mut nodes: IndexSet<i64> = IndexSet::new();
        let mut claims: IndexSet<i64> = IndexSet::new();
        let mut via_claims: IndexSet<i64> = IndexSet::new();
        let mut body: IndexMap<i64, i64> = IndexMap::new();
        let default_width = self.default_width as f64;
        let via_body = (spec.via_size as f64 - default_width) / 2.0 / pf;
        let neighbour = self.default_width.max(spec.width) as f64;
        let track_radius = (spec.width as f64 / 2.0 + spec.clearance as f64 + neighbour / 2.0) / pf;
        let hole = self.board.rules.minimum_hole_clearance_nm as f64;
        let via_reach = (spec.via_size as f64 / 2.0 + spec.clearance as f64).max(spec.via_drill as f64 / 2.0 + hole);
        let via_radius = (via_reach + default_width / 2.0) / pf;
        let via_via_radius = (via_reach + spec.via_size as f64 / 2.0) / pf;
        let prospective_reach = (self.candidate_via_size as f64 / 2.0
            + spec.clearance.max(self.candidate_clearance) as f64)
            .max(self.candidate_via_drill as f64 / 2.0 + hole);
        let track_via_radius = (prospective_reach + spec.width as f64 / 2.0) / pf;
        let mut length = 0.0;
        let mut changes: IndexMap<i64, (i64, i64)> = IndexMap::new();
        for path in &route.paths {
            let mut moved = false;
            for pair in path.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (la, lb) = (a.div_euclid(plane), b.div_euclid(plane));
                if la != lb {
                    let xy = a.rem_euclid(plane);
                    let (low, high) = changes.get(&xy).copied().unwrap_or((la, la));
                    changes.insert(xy, (low.min(la).min(lb), high.max(la).max(lb)));
                    moved = true;
                    continue;
                }
                let (pa, pb) = (g.point(a), g.point(b));
                if pa == pb {
                    continue; // a portal: the twins are one site
                }
                moved = true;
                length += hypot((pb.x_nm - pa.x_nm) as f64, (pb.y_nm - pa.y_nm) as f64) / pf;
                claims.extend(nested_step_claims(g, pitch, a, b, pa, pb, track_radius));
                via_claims.extend(
                    nested_step_claims(g, pitch, a, b, pa, pb, track_via_radius)
                        .into_iter()
                        .map(|n| n.rem_euclid(plane)),
                );
            }
            nodes.extend(path.iter().copied());
            if !moved {
                let column = path[0].rem_euclid(plane);
                let base = path[0] - column;
                claims.extend(nested_disc_columns(g, pitch, column, track_radius).into_iter().map(|c| c + base));
            }
        }
        for leads in &route.leads {
            for track in leads {
                let layer = g.layers.iter().position(|l| *l == track.layer).expect("lead layer") as i64;
                let radius_nm = track.width_nm as f64 / 2.0
                    + spec.clearance as f64
                    + self.default_width.max(track.width_nm) as f64 / 2.0;
                for (node, distance) in g.segment_nodes(layer, track.start, track.end, radius_nm) {
                    claims.insert(node);
                    if distance <= g.step_nm(node) as f64 / 2.0 {
                        nodes.insert(node);
                    }
                }
                for (node, _) in
                    g.segment_nodes(layer, track.start, track.end, prospective_reach + track.width_nm as f64 / 2.0)
                {
                    via_claims.insert(node.rem_euclid(plane));
                }
            }
        }
        let via_xy: IndexSet<i64> = changes.keys().copied().collect();
        // Without via technologies every site is a through via.
        let nl = g.nl;
        for &xy in changes.keys() {
            if via_body >= 0.5 {
                for column in g.columns_with_twins(&nested_disc_columns(g, pitch, xy, via_body)) {
                    for layer in 0..nl {
                        body.insert(layer * plane + column, xy);
                    }
                }
            }
            for column in nested_disc_columns(g, pitch, xy, via_radius) {
                for layer in 0..nl {
                    claims.insert(layer * plane + column);
                }
            }
            via_claims.extend(g.columns_with_twins(&nested_disc_columns(g, pitch, xy, via_via_radius)));
        }
        if !via_claims.is_empty() {
            via_claims = g.columns_with_twins(&via_claims.into_iter().collect::<Vec<_>>()).into_iter().collect();
        }
        let nodes: IndexSet<i64> = g.with_twins(&nodes.into_iter().collect::<Vec<_>>()).into_iter().collect();
        let claims: IndexSet<i64> = g.with_twins(&claims.into_iter().collect::<Vec<_>>()).into_iter().collect();
        route.body = body.into_iter().filter(|(node, _)| !nodes.contains(node)).collect();
        route.nodes = nodes;
        route.via_xy = via_xy;
        route.claims = claims;
        route.via_claims = via_claims;
        route.length = length;
    }
}

/// numpy's `sum` of a float64 array: pairwise summation in blocks of 8
/// (exact for the short arrays here when fewer than 9 elements).
pub fn numpy_sum(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<f64> = values.collect();
    pairwise_sum(&values)
}

fn pairwise_sum(values: &[f64]) -> f64 {
    let n = values.len();
    if n < 8 {
        let mut total = 0.0;
        for &v in values {
            total += v;
        }
        total
    } else if n <= 128 {
        let mut r = [0.0f64; 8];
        r.copy_from_slice(&values[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for k in 0..8 {
                r[k] += values[i + k];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += values[i];
            i += 1;
        }
        res
    } else {
        let mut half = n / 2;
        half -= half % 8;
        pairwise_sum(&values[..half]) + pairwise_sum(&values[half..])
    }
}

//! PathFinder negotiation (`negotiate`, `_conflicts`, `_penalise`) and the
//! exact materialisation of routes (`copper`, `commit`).

use crate::routing_lattice::Lattice;
use indexmap::{IndexMap, IndexSet};

use copper_json::Value;
use copper_pcb::board::{TrackSegment, Via};
use copper_physical::Point;

use super::router::{Lead, NetRoute, Router};
use super::search::disc_offsets_cached;
use crate::routing_clearance::{RoutingBlocker, RoutingClearanceIndex};
use crate::surface_path::track_inside_board;
use crate::unified_error::Result;

/// A progress event the router records for its caller.
pub type Event = (&'static str, &'static str, Vec<(&'static str, Value)>);

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

/// `_first_reason`: "reason: identity" of the first blocker record.
pub fn first_reason(records: &[RoutingBlocker]) -> String {
    records.first().map_or(String::new(), |r| format!("{}: {}", r.reason, r.identity))
}

/// Accepted copper per net.
pub type Accepted = IndexMap<String, (Vec<TrackSegment>, Vec<Via>)>;

enum Proposal {
    Track(TrackSegment),
    Via(Via),
}

impl Router<'_, '_> {
    /// `_conflicts`: negotiate actual emitted copper.
    pub fn conflicts(&mut self, routes: &IndexMap<String, NetRoute>) -> Result<IndexSet<String>> {
        self.exact_track_conflicts.clear();
        self.exact_lead_conflicts.clear();
        self.exact_via_conflicts.clear();
        let mut index = RoutingClearanceIndex::new(self.board)?;
        let mut objects: IndexMap<String, (String, Proposal)> = IndexMap::new();
        let mut raw_leads: IndexMap<(String, TrackSegment), IndexSet<TrackSegment>> = IndexMap::new();
        let mut names: Vec<&String> = routes.keys().collect();
        names.sort();
        for name in names {
            let route = &routes[name];
            if !route.connected {
                continue;
            }
            let spec_index = self.specs.iter().position(|s| &s.name == name).expect("spec");
            let raws: IndexSet<&TrackSegment> = route.leads.iter().flatten().collect();
            for raw in raws {
                for fragment in self.clearance.as_ref().expect("index").breakout.neck_down(std::slice::from_ref(raw)) {
                    raw_leads.entry((name.clone(), fragment)).or_default().insert(raw.clone());
                }
            }
            let mut tracks: IndexSet<TrackSegment> = IndexSet::new();
            let mut vias: IndexSet<Via> = IndexSet::new();
            for (path, leads) in route.paths.iter().zip(&route.leads) {
                let (t, v) = self.copper(spec_index, path, leads, None)?;
                tracks.extend(t);
                vias.extend(v);
            }
            let mut tracks: Vec<TrackSegment> = tracks.into_iter().collect();
            tracks.sort_by(|a, b| {
                (a.layer.value(), a.start.x_nm, a.start.y_nm, a.end.x_nm, a.end.y_nm, a.width_nm).cmp(&(
                    b.layer.value(),
                    b.start.x_nm,
                    b.start.y_nm,
                    b.end.x_nm,
                    b.end.y_nm,
                    b.width_nm,
                ))
            });
            for track in tracks {
                let token = format!("proposal-track:{}", objects.len());
                index.add_track(track.clone(), false, Some(token.clone()))?;
                objects.insert(token, (name.clone(), Proposal::Track(track)));
            }
            let mut vias: Vec<Via> = vias.into_iter().collect();
            vias.sort_by_key(|v| (v.position.x_nm, v.position.y_nm));
            for via in vias {
                let token = format!("proposal-via:{}", objects.len());
                index.add_via(via.clone(), false, Some(token.clone()))?;
                objects.insert(token, (name.clone(), Proposal::Via(via)));
            }
        }
        let mut bad: IndexSet<String> = IndexSet::new();
        let mut prices: Vec<(String, usize)> = Vec::new();
        for (position, (name, copper)) in objects.values().enumerate() {
            let records = match copper {
                Proposal::Track(track) => index.track_blocker_records(track)?,
                Proposal::Via(via) => index.via_blocker_records(via)?,
            };
            for record in records {
                if record.immutable {
                    continue;
                }
                let Some(other) = objects.get_index_of(&record.occurrence) else { continue };
                let owner = objects[other].0.clone();
                bad.insert(name.clone());
                bad.insert(owner.clone());
                prices.push((name.clone(), position));
                prices.push((owner, other));
            }
        }
        for (owner, position) in prices {
            match &objects[position].1 {
                Proposal::Track(track) => {
                    self.exact_track_conflicts.entry(owner.clone()).or_default().insert(track.clone());
                    let raws = raw_leads.get(&(owner.clone(), track.clone())).cloned().unwrap_or_default();
                    self.exact_lead_conflicts.entry(owner).or_default().extend(raws);
                }
                Proposal::Via(via) => {
                    let column = self.lattice.column(self.lattice.nearest(0, via.position));
                    self.exact_via_conflicts.entry(owner).or_default().insert(column);
                }
            }
        }
        Ok(bad)
    }

    /// `_penalise`.
    fn penalise(&mut self, names: &IndexSet<String>) {
        let inc = self.options.history_increment;
        let g = &self.lattice;
        for name in names {
            for track in self.exact_track_conflicts.get(name).into_iter().flatten() {
                let layer = g.layers.iter().position(|l| *l == track.layer).expect("layer") as i64;
                for (node, _) in g.segment_nodes(layer, track.start, track.end, g.pitch as f64 / 2.0 + 1.0) {
                    self.occupancy.history[node as usize] += inc;
                }
            }
            for &xy in self.exact_via_conflicts.get(name).into_iter().flatten() {
                self.occupancy.via_history[xy as usize] += inc;
            }
            for track in self.exact_lead_conflicts.get(name).into_iter().flatten() {
                *self.lead_history.entry(track.clone()).or_insert(0.0) += inc;
            }
        }
    }

    /// `negotiate`: the PathFinder loop; routes persist on the router.
    pub fn negotiate(
        &mut self,
        only: Option<&IndexSet<String>>,
        events: &mut Vec<Event>,
    ) -> Result<(IndexMap<String, NetRoute>, IndexSet<String>, i64)> {
        let options = self.options.clone();
        let planned: IndexMap<&str, usize> =
            options.escape_net_order.iter().enumerate().map(|(rank, name)| (name.as_str(), rank)).collect();
        let mut order: Vec<usize> = (0..self.specs.len()).collect();
        order.sort_by(|&a, &b| {
            let (sa, sb) = (&self.specs[a], &self.specs[b]);
            let key = |s: &super::router::NetSpec| {
                (planned.get(s.name.as_str()).copied().unwrap_or(planned.len()), s.span, s.name.clone())
            };
            key(sa).cmp(&key(sb))
        });
        let names: Vec<String> = order.iter().map(|&i| self.specs[i].name.clone()).collect();
        let by_name: IndexMap<String, usize> = order.iter().map(|&i| (self.specs[i].name.clone(), i)).collect();
        let mut present = self.present;
        let mut best_snapshot: Option<((usize, f64), IndexMap<String, NetRoute>, IndexSet<String>)> = None;
        let mut best_iteration = 0;
        let mut pending: Vec<String> = names
            .iter()
            .filter(|name| only.is_none_or(|o| o.contains(*name)) || !self.routes.contains_key(*name))
            .cloned()
            .collect();
        let mut iteration = 0;
        let mut failed: IndexSet<String> = IndexSet::new();
        let mut retries = 0;
        let mut retried = false;
        let mut next_iteration = 1;
        while next_iteration <= options.maximum_iterations {
            iteration = next_iteration;
            next_iteration += 1;
            events.push((
                "unified_iteration",
                "started",
                vec![("iteration", int(iteration)), ("nets", int(pending.len() as i64))],
            ));
            retried = retried || pending.iter().any(|name| failed.contains(name));
            for name in &pending {
                let index = by_name[name];
                if let Some(old) = self.routes.get(name).cloned() {
                    self.occupancy.add(&old, -1);
                }
                let route = self.route_net(index, present)?;
                self.occupancy.add(&route, 1);
                self.routes.insert(name.clone(), route);
            }
            let routes = self.routes.clone();
            let conflicts = self.conflicts(&routes)?;
            for name in &pending {
                let age =
                    if conflicts.contains(name) { self.conflict_age.get(name).copied().unwrap_or(0) + 1 } else { 0 };
                self.conflict_age.insert(name.clone(), age);
            }
            failed = names.iter().filter(|name| self.routes.get(*name).is_none_or(|r| !r.connected)).cloned().collect();
            let score =
                (failed.len() + conflicts.len(), copper_physical::pyfloat::sum(self.routes.values().map(|r| r.length)));
            if best_snapshot.as_ref().is_none_or(|best| score < best.0) {
                best_snapshot = Some((score, self.routes.clone(), conflicts.clone()));
                best_iteration = iteration;
            }
            events.push((
                "unified_iteration",
                "finished",
                vec![
                    ("iteration", int(iteration)),
                    ("conflicted", int(conflicts.len() as i64)),
                    ("failed", int(failed.len() as i64)),
                ],
            ));
            if conflicts.is_empty() {
                if options.failed_retry_before_exit
                    && !failed.is_empty()
                    && !retried
                    && iteration < options.maximum_iterations
                {
                    pending = names.iter().filter(|name| failed.contains(*name)).cloned().collect();
                    events.push((
                        "unified_iteration",
                        "failed_retry",
                        vec![("iteration", int(iteration + 1)), ("nets", int(pending.len() as i64))],
                    ));
                    continue;
                }
                break;
            }
            if options.stall_iterations != 0 && iteration - best_iteration >= options.stall_iterations {
                break;
            }
            self.penalise(&conflicts);
            present *= options.present_growth;
            if options.present_limit != 0.0 {
                present = present.min(options.present_limit);
            }
            self.present = present;
            let retry = options.failed_retry_every != 0
                && iteration % options.failed_retry_every == 0
                && (options.failed_retry_limit == 0 || retries < options.failed_retry_limit);
            retries += retry as i64;
            pending = names
                .iter()
                .filter(|name| conflicts.contains(*name) || (failed.contains(*name) && retry))
                .cloned()
                .collect();
            if options.full_reroute_every != 0 && iteration % options.full_reroute_every == 0 {
                pending = names.iter().filter(|name| !failed.contains(*name) || retry).cloned().collect();
            }
            if pending.is_empty() {
                break;
            }
        }
        let (snapshot, conflicts) = match best_snapshot {
            Some((_, snapshot, conflicts)) => (snapshot, conflicts),
            None => {
                let routes = self.routes.clone();
                let conflicts = self.conflicts(&routes)?;
                (routes, conflicts)
            }
        };
        if snapshot != self.routes {
            for route in self.routes.values().cloned().collect::<Vec<_>>() {
                self.occupancy.add(&route, -1);
            }
            self.routes = snapshot;
            for route in self.routes.values().cloned().collect::<Vec<_>>() {
                self.occupancy.add(&route, 1);
            }
        }
        for name in &names {
            if !self.routes.contains_key(name) {
                self.routes.insert(name.clone(), NetRoute::empty(false, "time budget exhausted"));
            }
        }
        Ok((self.routes.clone(), conflicts, iteration))
    }

    /// `copper`: tracks and vias of one path, with its lead-ins (through vias).
    pub fn copper(
        &self,
        index: usize,
        path: &[i64],
        leads: &Lead,
        _spans: Option<()>,
    ) -> Result<(Vec<TrackSegment>, Vec<Via>)> {
        let spec = &self.specs[index];
        let g = &self.lattice;
        let mut tracks: Vec<TrackSegment> = leads.clone();
        let mut vias: Vec<Via> = Vec::new();
        let width_of = |a: i64, b: i64| {
            if spec.region.contains(&a)
                && spec.region.contains(&b)
                && (spec.neck.contains(&a) || spec.neck.contains(&b) || spec.neck_mid.contains(&g.diagonal_cell(a, b)))
            {
                spec.neck_width
            } else {
                spec.width
            }
        };
        let track = |start: i64, end: i64, width: i64, layer: i64| TrackSegment {
            net: spec.name.clone(),
            start: g.point(start),
            end: g.point(end),
            width_nm: width,
            layer: g.layers[layer as usize],
        };
        if path.len() >= 2 {
            let mut run_start = path[0];
            let mut prev = path[0];
            let mut direction: Option<(i64, i64)> = None;
            let mut run_width: i64 = 0;
            for &node in &path[1..] {
                let (la, lb) = (g.layer_of(prev), g.layer_of(node));
                if la != lb {
                    if run_start != prev {
                        tracks.push(track(run_start, prev, run_width, la));
                    }
                    vias.push(Via {
                        net: spec.name.clone(),
                        position: g.point(prev),
                        size_nm: spec.via_size,
                        drill_nm: spec.via_drill,
                        from_layer: g.layers[0],
                        to_layer: *g.layers.last().expect("layers"),
                        technology: None,
                        finish: "standard".to_owned(),
                    });
                    run_start = node;
                    prev = node;
                    direction = None;
                    continue;
                }
                let step = g.delta(prev, node);
                if step == (0, 0) {
                    prev = node;
                    continue;
                }
                let width = width_of(prev, node);
                if let Some(direction) = direction
                    && (step != direction || width != run_width)
                {
                    tracks.push(track(run_start, prev, run_width, la));
                    run_start = prev;
                }
                direction = Some(step);
                run_width = width;
                prev = node;
            }
            if run_start != prev {
                tracks.push(track(run_start, prev, run_width, g.layer_of(prev)));
            }
        }
        let tracks = self.clearance.as_ref().expect("index").breakout.neck_down(&tracks);
        let mut unique: Vec<Via> = Vec::new();
        for via in vias {
            if !unique.contains(&via) {
                unique.push(via);
            }
        }
        Ok((tracks, unique))
    }

    /// `commit`: the exact check of every path; accepted copper, rejections and complete nets.
    pub fn commit(
        &mut self,
        routes: &IndexMap<String, NetRoute>,
        conflicts: &IndexSet<String>,
    ) -> Result<(Accepted, IndexMap<String, Vec<Point>>, IndexSet<String>)> {
        self.rejections.clear();
        let mut accepted: Accepted = IndexMap::new();
        let mut rejected: IndexMap<String, Vec<Point>> = IndexMap::new();
        let mut complete: IndexSet<String> = routes
            .iter()
            .filter(|(_, route)| route.connected && route.paths.is_empty())
            .map(|(name, _)| name.clone())
            .collect();
        let by_name: IndexMap<&str, usize> = self.specs.iter().enumerate().map(|(i, s)| (s.name.as_str(), i)).collect();
        let mut order: Vec<&String> = routes.iter().filter(|(_, r)| !r.paths.is_empty()).map(|(n, _)| n).collect();
        order.sort_by(|a, b| {
            let key = |name: &String| {
                (
                    conflicts.contains(name),
                    !routes[name].connected,
                    self.specs[by_name[name.as_str()]].span,
                    name.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        for name in order {
            let index = by_name[name.as_str()];
            let route = &routes[name];
            let mut all_ok = route.connected;
            let mut net_tracks: Vec<TrackSegment> = Vec::new();
            let mut net_vias: Vec<Via> = Vec::new();
            for (path, leads) in route.paths.iter().zip(&route.leads) {
                let (tracks, vias) = self.copper(index, path, leads, None)?;
                let mut bad: Vec<Point> = Vec::new();
                for track in &tracks {
                    if !(self.clearance.as_ref().expect("index").can_track(
                        name,
                        track.start,
                        track.end,
                        track.width_nm,
                        track.layer,
                    )? && track_inside_board(self.board, track.start, track.end, track.width_nm)?)
                    {
                        let at = Point::new(
                            (track.start.x_nm + track.end.x_nm).div_euclid(2),
                            (track.start.y_nm + track.end.y_nm).div_euclid(2),
                        );
                        bad.push(at);
                        let (blockers, fixed) = self.clearance.as_ref().expect("index").blocking_track_nets(track)?;
                        let mut blockers: Vec<String> = blockers.into_iter().collect();
                        blockers.sort();
                        let reason =
                            first_reason(&self.clearance.as_ref().expect("index").track_blocker_records(track)?);
                        self.rejections.entry(name.clone()).or_default().push((
                            at,
                            track.layer.value().to_owned(),
                            blockers,
                            fixed,
                            reason,
                        ));
                    }
                }
                for via in &vias {
                    if net_vias.contains(via) {
                        continue;
                    }
                    if !self.clearance.as_ref().expect("index").can_via(
                        name,
                        via.position,
                        via.size_nm,
                        via.from_layer,
                        via.to_layer,
                        Some(via.drill_nm),
                        false,
                        None,
                    )? {
                        bad.push(via.position);
                        let (blockers, fixed) = self.clearance.as_ref().expect("index").blocking_via_nets(via)?;
                        let mut blockers: Vec<String> = blockers.into_iter().collect();
                        blockers.sort();
                        let reason = first_reason(&self.clearance.as_ref().expect("index").via_blocker_records(via)?);
                        self.rejections.entry(name.clone()).or_default().push((
                            via.position,
                            "via".to_owned(),
                            blockers,
                            fixed,
                            reason,
                        ));
                    }
                }
                if bad.is_empty() {
                    let fresh: Vec<Via> = vias.iter().filter(|v| !net_vias.contains(v)).cloned().collect();
                    if self.clearance.as_ref().expect("index").candidate_via_conflict(&fresh, false)?.is_some() {
                        bad.push(vias[0].position);
                    }
                }
                if !bad.is_empty() {
                    rejected.entry(name.clone()).or_default().extend(bad);
                    all_ok = false;
                    continue;
                }
                for track in tracks {
                    self.clearance.as_mut().expect("index").add_track(track.clone(), false, None)?;
                    net_tracks.push(track);
                }
                for via in vias {
                    if !net_vias.contains(&via) {
                        self.clearance.as_mut().expect("index").add_via(via.clone(), false, None)?;
                        net_vias.push(via);
                    }
                }
            }
            if !net_tracks.is_empty() || !net_vias.is_empty() {
                accepted.insert(name.clone(), (net_tracks, net_vias));
            }
            if all_ok {
                complete.insert(name.clone());
            }
        }
        Ok((accepted, rejected, complete))
    }

    /// `history_seed`: carry congestion history over from an earlier router.
    pub fn history_seed(&mut self, other: &Router<'_, '_>) {
        if other.lattice.signature() == self.lattice.signature() {
            self.occupancy.copy_history(&other.occupancy);
            self.present = other.present;
            self.conflict_age = other.conflict_age.clone();
            self.lead_history = other.lead_history.clone();
        }
    }

    /// `penalise_points`.
    pub fn penalise_points(&mut self, points: &[Point], amount: f64) {
        let g = &self.lattice;
        for &point in points {
            for column in g.columns_around(point, &disc_offsets_cached(2.0)) {
                self.occupancy.via_history[column as usize] += amount;
                for node in g.column_nodes(column) {
                    self.occupancy.history[node as usize] += amount;
                }
            }
        }
    }
}

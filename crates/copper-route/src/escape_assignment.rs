//! Joint package escape assignment (`pcbir.escape_assignment`): its options.

use crate::{Error, Result};

/// `EscapeAssignmentOptions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscapeAssignmentOptions {
    /// None tries each missing root once, within the shared pair/query budgets.
    pub maximum_trials: Option<i64>,
    pub maximum_cluster_pins: i64,
    pub maximum_search_states: i64,
    pub maximum_pair_checks: i64,
    pub maximum_pair_queries: i64,
}

impl Default for EscapeAssignmentOptions {
    fn default() -> Self {
        EscapeAssignmentOptions {
            maximum_trials: None,
            maximum_cluster_pins: 12,
            maximum_search_states: 20_000,
            maximum_pair_checks: 200_000,
            maximum_pair_queries: 2_000_000,
        }
    }
}

impl EscapeAssignmentOptions {
    /// `__post_init__`.
    pub fn validated(self) -> Result<Self> {
        if self.maximum_trials.is_some_and(|trials| trials <= 0) {
            return Err(Error::Value("escape assignment trial budget must be a positive integer or None".to_owned()));
        }
        if [self.maximum_cluster_pins, self.maximum_search_states, self.maximum_pair_checks, self.maximum_pair_queries]
            .into_iter()
            .min()
            .expect("budgets")
            <= 0
        {
            return Err(Error::Value("escape assignment budgets must be positive".to_owned()));
        }
        Ok(self)
    }
}

// ---------------------------------------------------------------------------
// Joint assignment

use indexmap::{IndexMap, IndexSet};

use copper_pcb::board::{PadReference, PhysicalBoard, PhysicalNet, TrackSegment, Via};

use crate::route_quality::escape_paths_cross;
use crate::routing_clearance::RoutingClearanceIndex;

/// `EscapeCandidate`: a launch path and its via (None reuses an existing one).
pub type EscapeCandidate = (Vec<TrackSegment>, Option<Via>);

/// `EscapeAssignmentTrial`.
#[derive(Clone, Debug)]
pub struct EscapeAssignmentTrial {
    pub pad: PadReference,
    pub cluster: Vec<PadReference>,
    pub search_states: i64,
    pub solution_found: bool,
    pub diagnostic: String,
}

/// `EscapeAssignmentReport`.
#[derive(Clone, Debug)]
pub struct EscapeAssignmentReport {
    pub trials: Vec<EscapeAssignmentTrial>,
    pub pair_checks: i64,
    pub expanded_pads: Vec<PadReference>,
    pub native_accepted: bool,
    pub pair_queries: i64,
    pub broad_phase_accepts: i64,
}

/// `_BudgetExceeded`, or a real error.
enum Stop {
    Budget(String),
    Error(Error),
}

impl From<Error> for Stop {
    fn from(error: Error) -> Self {
        Stop::Error(error)
    }
}

/// `EscapeConflicts`: a lazy exact candidate-conflict graph scoped to one immutable board.
pub struct EscapeConflicts {
    board: PhysicalBoard,
    maximum_checks: i64,
    pub checks: i64,
    maximum_queries: i64,
    pub queries: i64,
    pub broad_phase_accepts: i64,
    identities: IndexMap<EscapeCandidate, usize>,
    bounds: Vec<Option<(i64, i64, i64, i64)>>,
    copper: IndexMap<usize, (Vec<TrackSegment>, Vec<Via>)>,
    cache: IndexMap<(usize, usize, bool), bool>,
    margin: i64,
}

impl EscapeConflicts {
    pub fn new(board: &PhysicalBoard, maximum_checks: i64, maximum_queries: i64) -> Self {
        // Input pads/keepouts were checked by domain generation; this board
        // checks only candidate-candidate copper and drills.
        let mut empty = board.clone();
        empty.name = "escape-conflicts".to_owned();
        empty.footprints.clear();
        empty.placements.clear();
        empty.nets = board.nets.iter().map(|n| PhysicalNet { name: n.name.clone(), pads: Vec::new() }).collect();
        empty.tracks.clear();
        empty.vias.clear();
        empty.metadata.clear();
        empty.regions.clear();
        empty.keepouts.clear();
        empty.placement_rules.clear();
        empty.relative_rules.clear();
        empty.placement_groups.clear();
        empty.power_domains.clear();
        empty.zones.clear();
        empty.polygons.clear();
        empty.copper_keepouts.clear();
        empty.rigid_clusters.clear();
        empty.hard_macros.clear();
        empty.materialized_macros.clear();
        empty.via_in_pad_rules.clear();
        empty.mechanical_holes.clear();
        let margin = board
            .net_routing_rules
            .iter()
            .map(|r| r.clearance_nm.unwrap_or(0))
            .chain([board.rules.minimum_clearance_nm, board.rules.minimum_hole_clearance_nm])
            .max()
            .expect("rules");
        EscapeConflicts {
            board: empty,
            maximum_checks,
            checks: 0,
            maximum_queries,
            queries: 0,
            broad_phase_accepts: 0,
            identities: IndexMap::new(),
            bounds: Vec::new(),
            copper: IndexMap::new(),
            cache: IndexMap::new(),
            margin,
        }
    }

    fn identity(&mut self, candidate: &EscapeCandidate) -> usize {
        if let Some(identity) = self.identities.get(candidate) {
            return *identity;
        }
        let identity = self.identities.len();
        self.identities.insert(candidate.clone(), identity);
        let (tracks, via) = candidate;
        let mut points: Vec<copper_physical::Point> = tracks.iter().flat_map(|t| [t.start, t.end]).collect();
        let mut radius = tracks.iter().map(|t| t.width_nm.div_euclid(2)).max().unwrap_or(0);
        if let Some(via) = via {
            points.push(via.position);
            radius = radius.max(via.size_nm.div_euclid(2)).max(via.drill_nm.div_euclid(2));
        }
        let margin = radius + self.margin;
        self.bounds.push(if points.is_empty() {
            None
        } else {
            Some((
                points.iter().map(|p| p.x_nm).min().expect("points") - margin,
                points.iter().map(|p| p.y_nm).min().expect("points") - margin,
                points.iter().map(|p| p.x_nm).max().expect("points") + margin,
                points.iter().map(|p| p.y_nm).max().expect("points") + margin,
            ))
        });
        identity
    }

    fn clear(&self, candidate: &EscapeCandidate, other: usize) -> Result<bool> {
        let mut index = RoutingClearanceIndex::new(&self.board)?;
        let (tracks, vias) = &self.copper[&other];
        for track in tracks {
            index.add_track(track.clone(), true, None)?;
        }
        for via in vias {
            index.add_via(via.clone(), true, None)?;
        }
        let (tracks, via) = candidate;
        for t in tracks {
            if !index.can_track(&t.net, t.start, t.end, t.width_nm, t.layer)? {
                return Ok(false);
            }
        }
        if let Some(v) = via {
            return index.can_via(
                &v.net,
                v.position,
                v.size_nm,
                v.from_layer,
                v.to_layer,
                Some(v.drill_nm),
                false,
                None,
            );
        }
        Ok(true)
    }

    /// `compatible(first, second, ordered_lanes=True)`.
    fn compatible(&mut self, first: &EscapeCandidate, second: &EscapeCandidate) -> std::result::Result<bool, Stop> {
        if self.queries >= self.maximum_queries {
            return Err(Stop::Budget("candidate compatibility query budget exhausted".to_owned()));
        }
        self.queries += 1;
        let (a, b) = (self.identity(first), self.identity(second));
        let key = (a.min(b), a.max(b), true);
        if let Some(answer) = self.cache.get(&key) {
            return Ok(*answer);
        }
        let separated = match (self.bounds[a], self.bounds[b]) {
            (Some(aa), Some(bb)) => aa.2 < bb.0 || bb.2 < aa.0 || aa.3 < bb.1 || bb.3 < aa.1,
            _ => true,
        };
        if separated {
            self.broad_phase_accepts += 1;
            return Ok(true);
        }
        if self.checks >= self.maximum_checks {
            return Err(Stop::Budget("candidate compatibility budget exhausted".to_owned()));
        }
        self.checks += 1;
        for (identity, candidate) in [(a, first), (b, second)] {
            self.copper.entry(identity).or_insert_with(|| (candidate.0.clone(), candidate.1.iter().cloned().collect()));
        }
        // The Python index of each candidate holds only its own copper, so a
        // fresh index per check is the same predicate.
        let answer = !escape_paths_cross(&first.0, &second.0) && self.clear(second, a)? && self.clear(first, b)?;
        self.cache.insert(key, answer);
        Ok(answer)
    }
}

type Domains = IndexMap<PadReference, Vec<EscapeCandidate>>;

/// `_finite_assignment`: MRV with forward checking.
#[allow(clippy::too_many_arguments)]
fn finite_assignment(
    remaining: &IndexMap<PadReference, Vec<usize>>,
    assigned: &IndexMap<PadReference, usize>,
    preferred: &IndexMap<PadReference, usize>,
    rank: &IndexMap<PadReference, usize>,
    domains: &Domains,
    conflicts: &mut EscapeConflicts,
    states: &mut i64,
    maximum_states: i64,
) -> std::result::Result<Option<IndexMap<PadReference, usize>>, Stop> {
    if remaining.is_empty() {
        return Ok(Some(assigned.clone()));
    }
    let pad =
        remaining.iter().min_by_key(|(p, values)| (values.len(), rank[*p])).map(|(p, _)| p.clone()).expect("pads");
    let mut choices = remaining[&pad].clone();
    choices.sort_by_key(|i| (Some(i) != preferred.get(&pad), *i, *i));
    for choice in choices {
        if *states >= maximum_states {
            return Err(Stop::Budget("local assignment search budget exhausted".to_owned()));
        }
        *states += 1;
        let mut filtered: IndexMap<PadReference, Vec<usize>> = IndexMap::new();
        for (other, values) in remaining {
            if *other == pad {
                continue;
            }
            let mut kept = Vec::new();
            for i in values {
                if conflicts.compatible(&domains[&pad][choice], &domains[other][*i])? {
                    kept.push(*i);
                }
            }
            filtered.insert(other.clone(), kept);
        }
        if filtered.values().any(|values| values.is_empty()) {
            continue;
        }
        let mut next = assigned.clone();
        next.insert(pad.clone(), choice);
        if let Some(answer) =
            finite_assignment(&filtered, &next, preferred, rank, domains, conflicts, states, maximum_states)?
        {
            return Ok(Some(answer));
        }
    }
    Ok(None)
}

/// `improve_escape_assignment`: try missing pins with current blockers;
/// outside selections stay fixed.
#[allow(clippy::type_complexity)]
pub fn improve_escape_assignment(
    board: &PhysicalBoard,
    ordered: &[PadReference],
    domains: &mut Domains,
    incumbent: &IndexMap<PadReference, usize>,
    expand: &mut dyn FnMut(&PadReference) -> Result<Vec<EscapeCandidate>>,
    options: &EscapeAssignmentOptions,
    mut fallback_expand: Option<&mut dyn FnMut(&PadReference) -> Result<Vec<EscapeCandidate>>>,
) -> Result<(IndexMap<PadReference, usize>, EscapeAssignmentReport)> {
    let mut selected = incumbent.clone();
    let mut conflicts = EscapeConflicts::new(board, options.maximum_pair_checks, options.maximum_pair_queries);
    let mut expanded: IndexSet<PadReference> = IndexSet::new();
    let mut fallback_expanded: IndexSet<PadReference> = IndexSet::new();
    let mut trials: Vec<EscapeAssignmentTrial> = Vec::new();
    let rank: IndexMap<PadReference, usize> = ordered.iter().enumerate().map(|(i, p)| (p.clone(), i)).collect();
    let extend = |domain: &mut Vec<EscapeCandidate>, additions: Vec<EscapeCandidate>| {
        for item in additions {
            if !domain.contains(&item) {
                domain.push(item);
            }
        }
    };
    for root in ordered {
        if selected.contains_key(root) || domains[root].is_empty() {
            continue;
        }
        if options.maximum_trials.is_some_and(|limit| trials.len() as i64 >= limit) {
            break;
        }
        let mut cluster: std::collections::BTreeSet<PadReference> = std::collections::BTreeSet::from([root.clone()]);
        let mut states = 0i64;
        let outcome = (|| -> std::result::Result<(bool, String), Stop> {
            for (pad, choice) in selected.clone() {
                let fixed = domains[&pad][choice].clone();
                for candidate in domains[root].clone() {
                    if !conflicts.compatible(&candidate, &fixed)? {
                        cluster.insert(pad.clone());
                        break;
                    }
                }
            }
            if cluster.len() as i64 > options.maximum_cluster_pins {
                return Ok((false, "local cluster pin budget exceeded".to_owned()));
            }
            loop {
                for pad in cluster.clone() {
                    if !expanded.contains(&pad) {
                        let additions = expand(&pad)?;
                        extend(domains.get_mut(&pad).expect("domain"), additions);
                        expanded.insert(pad);
                    }
                }
                let outside: Vec<(PadReference, EscapeCandidate)> = selected
                    .iter()
                    .filter(|(p, _)| !cluster.contains(*p))
                    .map(|(p, c)| (p.clone(), domains[p][*c].clone()))
                    .collect();
                let mut pressure: IndexMap<PadReference, i64> = IndexMap::new();
                let mut available: IndexMap<PadReference, Vec<usize>> = IndexMap::new();
                for pad in &cluster {
                    let mut values = Vec::new();
                    for (i, candidate) in domains[pad].clone().iter().enumerate() {
                        let mut blockers = Vec::new();
                        for (p, fixed) in &outside {
                            if !conflicts.compatible(candidate, fixed)? {
                                blockers.push(p.clone());
                            }
                        }
                        if blockers.is_empty() {
                            values.push(i);
                        } else {
                            for blocker in blockers {
                                *pressure.entry(blocker).or_insert(0) += 1;
                            }
                        }
                    }
                    available.insert(pad.clone(), values);
                }
                let answer = finite_assignment(
                    &available,
                    &IndexMap::new(),
                    &selected,
                    &rank,
                    domains,
                    &mut conflicts,
                    &mut states,
                    options.maximum_search_states,
                )?;
                if let Some(answer) = answer {
                    selected.extend(answer);
                    return Ok((true, "compatible local assignment found".to_owned()));
                }
                if pressure.is_empty() {
                    let mut changed = false;
                    if let Some(fallback) = fallback_expand.as_mut() {
                        for pad in cluster.clone() {
                            if !fallback_expanded.contains(&pad) {
                                let old = domains[&pad].clone();
                                let additions = fallback(&pad)?;
                                extend(domains.get_mut(&pad).expect("domain"), additions);
                                fallback_expanded.insert(pad.clone());
                                expanded.insert(pad.clone());
                                changed |= domains[&pad] != old;
                            }
                        }
                    }
                    if changed {
                        continue;
                    }
                    return Ok((false, "no compatible assignment in local domains".to_owned()));
                }
                if cluster.len() as i64 >= options.maximum_cluster_pins {
                    return Ok((false, "local cluster pin budget exceeded".to_owned()));
                }
                let blocker = pressure
                    .iter()
                    .min_by_key(|(p, count)| (-**count, rank[*p]))
                    .map(|(p, _)| p.clone())
                    .expect("pressure");
                cluster.insert(blocker);
            }
        })();
        match outcome {
            Ok((found, diagnostic)) => {
                let searched =
                    if diagnostic == "local cluster pin budget exceeded" && !found && states == 0 { 0 } else { states };
                trials.push(EscapeAssignmentTrial {
                    pad: root.clone(),
                    cluster: cluster.iter().cloned().collect(),
                    search_states: searched,
                    solution_found: found,
                    diagnostic,
                });
            }
            Err(Stop::Error(error)) => return Err(error),
            Err(Stop::Budget(message)) => {
                trials.push(EscapeAssignmentTrial {
                    pad: root.clone(),
                    cluster: cluster.iter().cloned().collect(),
                    search_states: states,
                    solution_found: false,
                    diagnostic: message,
                });
                if conflicts.checks >= options.maximum_pair_checks || conflicts.queries >= options.maximum_pair_queries
                {
                    break;
                }
            }
        }
    }
    let mut expanded_pads: Vec<PadReference> = expanded.into_iter().collect();
    expanded_pads.sort();
    Ok((
        selected,
        EscapeAssignmentReport {
            trials,
            pair_checks: conflicts.checks,
            expanded_pads,
            native_accepted: true,
            pair_queries: conflicts.queries,
            broad_phase_accepts: conflicts.broad_phase_accepts,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support_routers::*;

    /// `tests/test_routing_quality.py::test_greedy_and_joint_assignment_reject_the_same_net_x` (joint half).
    #[test]
    fn joint_assignment_rejects_the_same_net_x() {
        let board = board("quality", 10.0, 10.0, Vec::new(), Vec::new(), vec![net("P", &[])]);
        let wire = |a, b| track("P", a, b, 0.2, copper_physical::CopperLayer::Front);
        let first: EscapeCandidate = (vec![wire((2.0, 2.0), (4.0, 4.0))], None);
        let second: EscapeCandidate = (vec![wire((4.0, 2.0), (2.0, 4.0))], None);
        let joined: EscapeCandidate = (vec![wire((4.0, 4.0), (5.0, 3.0))], None);
        let mut conflicts = EscapeConflicts::new(&board, 10, 2_000_000);
        assert!(!conflicts.compatible(&first, &second).ok().expect("in budget"));
        assert!(conflicts.compatible(&first, &joined).ok().expect("in budget"));
    }

    fn candidate(
        net: &str,
        a: (f64, f64),
        b: (f64, f64),
        layer: copper_physical::CopperLayer,
        with_via: bool,
    ) -> EscapeCandidate {
        use copper_physical::CopperLayer;
        let path = vec![track(net, a, b, 0.25, layer)];
        (path, with_via.then(|| via(net, b, 0.8, 0.4, CopperLayer::Front, CopperLayer::Back)))
    }

    /// `tests/test_escape_assignment.py::board`.
    fn assignment_board() -> PhysicalBoard {
        let two = footprint("two", vec![pad("1", 0.0, -1.0, 0.3, 0.3), pad("2", 0.0, 1.0, 0.3, 0.3)], 2.0, 3.0);
        let one = footprint("one", vec![pad("1", 0.0, 0.0, 0.3, 0.3)], 1.0, 1.0);
        board(
            "assignment",
            14.0,
            14.0,
            vec![two, one],
            vec![placement("U", "two", 5.0, 7.0), placement("JA", "one", 11.0, 6.0), placement("JB", "one", 11.0, 8.0)],
            vec![net("A", &[("U", "1"), ("JA", "1")]), net("B", &[("U", "2"), ("JB", "1")])],
        )
    }

    fn ok(result: std::result::Result<bool, Stop>) -> bool {
        match result {
            Ok(answer) => answer,
            Err(Stop::Budget(message)) => panic!("budget: {message}"),
            Err(Stop::Error(error)) => panic!("{error}"),
        }
    }

    const F: copper_physical::CopperLayer = copper_physical::CopperLayer::Front;
    const B: copper_physical::CopperLayer = copper_physical::CopperLayer::Back;

    /// `tests/test_escape_assignment.py::test_conflicts_respect_layers_through_vias_same_net_and_drill_spacing`.
    #[test]
    fn conflicts_respect_layers_through_vias_same_net_and_drill_spacing() {
        let mut conflicts = EscapeConflicts::new(&assignment_board(), 100, 2_000_000);
        let a = candidate("A", (2.0, 2.0), (4.0, 2.0), F, false);
        let b = candidate("B", (3.0, 1.0), (3.0, 3.0), B, false);
        assert!(ok(conflicts.compatible(&a, &b)));
        assert!(!ok(conflicts.compatible(&a, &candidate("B", (3.0, 1.0), (3.0, 3.0), F, false))));
        let a_via = candidate("A", (2.0, 2.0), (4.0, 2.0), F, true);
        assert!(!ok(conflicts.compatible(&a_via, &candidate("B", (4.0, 1.0), (4.0, 3.0), B, false))));
        assert!(ok(conflicts.compatible(&a_via, &a_via)), "one reused hole, not two drills");
        assert!(!ok(conflicts.compatible(&a_via, &candidate("A", (2.0, 2.0), (4.6, 2.0), F, true))));
        let previous = conflicts.checks;
        assert!(ok(conflicts.compatible(&a, &b)));
        assert_eq!(conflicts.checks, previous);
    }

    /// `tests/test_escape_assignment.py::test_remote_candidates_do_not_starve_exact_compatibility_budget`.
    #[test]
    fn remote_candidates_do_not_starve_exact_compatibility_budget() {
        let mut conflicts = EscapeConflicts::new(&assignment_board(), 1, 100);
        let local = candidate("A", (2.0, 2.0), (4.0, 2.0), F, false);
        for i in 0..30 {
            let y = 8.0 + i as f64 / 100.0;
            assert!(ok(conflicts.compatible(&local, &candidate("B", (8.0, y), (9.0, y), F, false))));
        }
        assert!(conflicts.queries == 30 && conflicts.broad_phase_accepts == 30);
        assert!(conflicts.checks == 0 && conflicts.cache.is_empty());
        let blocker = candidate("B", (3.0, 1.0), (3.0, 3.0), F, false);
        assert!(!ok(conflicts.compatible(&local, &blocker)));
        assert_eq!(conflicts.checks, 1);
        // Cached exact answers also count toward the total query bound.
        assert!(!ok(conflicts.compatible(&local, &blocker)));
        assert!(conflicts.checks == 1 && conflicts.queries == 32);
    }

    /// `tests/test_escape_assignment.py::test_broad_phase_has_its_own_query_limit`.
    #[test]
    fn broad_phase_has_its_own_query_limit() {
        let mut conflicts = EscapeConflicts::new(&assignment_board(), 1, 1);
        let (first, remote) =
            (candidate("A", (2.0, 2.0), (4.0, 2.0), F, false), candidate("B", (8.0, 8.0), (9.0, 8.0), F, false));
        assert!(ok(conflicts.compatible(&first, &remote)));
        match conflicts.compatible(&first, &remote) {
            Err(Stop::Budget(message)) => assert!(message.contains("query budget"), "{message}"),
            _ => panic!("the query budget is exhausted"),
        }
        assert!(conflicts.checks == 0 && conflicts.queries == 1);
    }

    /// `tests/test_escape_assignment.py::test_foreign_clearance_overrides_enter_candidate_conflicts`.
    #[test]
    fn foreign_clearance_overrides_enter_candidate_conflicts() {
        let mut ruled = assignment_board();
        ruled.net_routing_rules = vec![copper_pcb::board::NetRoutingRule { clearance_nm: Some(mm(1.0)), ..rule("B") }];
        let (a, b) =
            (candidate("A", (2.0, 2.0), (4.0, 2.0), F, false), candidate("B", (2.0, 2.8), (4.0, 2.8), F, false));
        assert!(ok(EscapeConflicts::new(&assignment_board(), 100, 2_000_000).compatible(&a, &b)));
        assert!(!ok(EscapeConflicts::new(&ruled, 100, 2_000_000).compatible(&a, &b)));
        assert!(!ok(EscapeConflicts::new(&ruled, 100, 2_000_000).compatible(&b, &a)));
    }
}

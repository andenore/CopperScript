//! Physical-design readiness gates and the placement plan (`pcbir.layout`).

use indexmap::{IndexMap, IndexSet};

use copper_json::{Object, Value};
use copper_pcb::Result;
use copper_pcb::board::PhysicalBoard;
use copper_pcb::keepouts::placement_map;
use copper_physical::pyfloat::hypot;

use crate::escape::{EscapeChannel, EscapeSpacingModel};
use crate::metrics::PlacementMetrics;
use crate::options::PlannerOptions;
use crate::planner::{Candidate, generate_placement_candidates, select_placement_candidate};
use crate::scene::{Poses, Scene};
use crate::seed::{py_max, py_min, true_div};

/// `PLACEMENT_ALGORITHM`.
pub const ALGORITHM: &str = "hierarchical-analytical/hybrid-legalize/route-refine-v0.2";

/// `LayoutStage`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Prepare,
    Place,
    Route,
    Verify,
}

impl Stage {
    pub fn value(self) -> &'static str {
        match self {
            Stage::Prepare => "prepare",
            Stage::Place => "place",
            Stage::Route => "route",
            Stage::Verify => "verify",
        }
    }
}

/// `GateStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warning,
    Blocked,
    NotRun,
}

impl Status {
    pub fn value(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Warning => "warning",
            Status::Blocked => "blocked",
            Status::NotRun => "not_run",
        }
    }
}

/// `LayoutFinding`.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    pub stage: Stage,
    pub status: Status,
    pub message: String,
    pub subject: Option<String>,
}

fn finding(code: &'static str, stage: Stage, status: Status, message: String, subject: Option<String>) -> Finding {
    Finding { code, stage, status, message, subject }
}

/// `LayoutGate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Gate {
    pub stage: Stage,
    pub status: Status,
    pub summary: &'static str,
}

/// A corridor as the report lists it.
struct CorridorRow {
    name: String,
    nets: Vec<String>,
    side: &'static str,
    margin_nm: i64,
    terminals: Vec<String>,
    polygon: Vec<copper_physical::Point>,
}

/// `LayoutReport`.
pub struct LayoutReport {
    pub board_name: String,
    pub gates: Vec<Gate>,
    pub findings: Vec<Finding>,
    pub metrics: PlacementMetrics,
    pub candidates: Vec<Candidate>,
    pub selected_candidate: String,
    pub escape_channels: Vec<EscapeChannel>,
    corridors: Vec<CorridorRow>,
    skipped: Vec<(Vec<String>, String)>,
}

/// `PlacementPlan`: the planned board and its readiness report.
pub struct PlacementPlan {
    pub board: PhysicalBoard,
    pub report: LayoutReport,
}

fn int(value: i64) -> Value {
    Value::Int(value.into())
}

fn text(value: &str) -> Value {
    Value::Str(value.to_owned())
}

fn object(entries: Vec<(&str, Value)>) -> Value {
    Value::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn metrics_json(metrics: &PlacementMetrics) -> Value {
    object(vec![
        ("component_count", int(metrics.component_count)),
        ("net_count", int(metrics.net_count)),
        ("estimated_connection_count", int(metrics.estimated_connection_count)),
        ("half_perimeter_wire_length_nm", int(metrics.half_perimeter_wire_length_nm)),
        ("crossing_count", int(metrics.crossing_count)),
        ("estimated_via_count", int(metrics.estimated_via_count)),
        ("pin_escape_pressure", int(metrics.pin_escape_pressure)),
        ("escape_channel_penalty_nm", int(metrics.escape_channel_penalty_nm)),
        ("escape_channel_deficit_nm", int(metrics.escape_channel_deficit_nm)),
        ("escape_channel_pair_count", int(metrics.escape_channel_pair_count)),
        ("constraint_penalty_nm", int(metrics.constraint_penalty_nm)),
        ("minimum_constraint_margin_nm", int(metrics.minimum_constraint_margin_nm)),
        ("group_spread_nm", int(metrics.group_spread_nm)),
        ("power_domain_penalty_nm", int(metrics.power_domain_penalty_nm)),
        ("weighted_wire_length_nm", int(metrics.weighted_wire_length_nm)),
        ("local_connection_length_nm", int(metrics.local_connection_length_nm)),
        ("congestion_bin_count", int(metrics.congestion_bin_count)),
        ("congestion_capacity_per_bin", int(metrics.congestion_capacity_per_bin)),
        ("congestion_overflow", int(metrics.congestion_overflow)),
        ("maximum_congestion_utilization_ppm", int(metrics.maximum_congestion_utilization_ppm)),
    ])
}

impl LayoutReport {
    /// `LayoutReport.to_json`: the document, before requirement verification is added.
    pub fn document(&self) -> Value {
        let gates = self
            .gates
            .iter()
            .map(|g| {
                object(vec![
                    ("stage", text(g.stage.value())),
                    ("status", text(g.status.value())),
                    ("summary", text(g.summary)),
                ])
            })
            .collect();
        let findings = self
            .findings
            .iter()
            .map(|f| {
                let mut entries = vec![
                    ("code", text(f.code)),
                    ("stage", text(f.stage.value())),
                    ("status", text(f.status.value())),
                    ("message", text(&f.message)),
                ];
                if let Some(subject) = &f.subject {
                    entries.push(("subject", text(subject)));
                }
                object(entries)
            })
            .collect();
        let channels = self
            .escape_channels
            .iter()
            .map(|c| {
                object(vec![
                    ("left", text(&c.left)),
                    ("right", text(&c.right)),
                    ("axis", text(c.axis)),
                    ("left_side", text(c.left_side.value())),
                    ("right_side", text(c.right_side.value())),
                    ("gap_nm", int(c.gap_nm)),
                    ("required_gap_nm", int(c.required_gap_nm)),
                    ("left_depth_nm", int(c.left_depth_nm)),
                    ("right_depth_nm", int(c.right_depth_nm)),
                    ("left_rows", int(c.left_rows)),
                    ("right_rows", int(c.right_rows)),
                    ("pad_count", int(c.pad_count)),
                    ("deficit_nm", int(c.deficit_nm())),
                ])
            })
            .collect();
        let candidates = self
            .candidates
            .iter()
            .map(|c| {
                let s = &c.statistics;
                let mut statistics = vec![
                    ("analytical_iterations", int(s.analytical_iterations)),
                    ("exact_repair_count", int(s.exact_repair_count)),
                    ("relative_repair_count", int(s.relative_repair_count)),
                    ("refinement_move_count", int(s.refinement_move_count)),
                    ("refinement_swap_count", int(s.refinement_swap_count)),
                    ("routing_feedback_passes", int(s.routing_feedback_passes)),
                ];
                if !s.failed_seed_attempts.is_empty() {
                    let failures = s
                        .failed_seed_attempts
                        .iter()
                        .map(|f| {
                            object(vec![
                                ("seed", int(f.seed)),
                                ("failed_reference", text(&f.failed_reference)),
                                ("accepted_count", int(f.accepted_count)),
                                ("reason", text(&f.reason)),
                            ])
                        })
                        .collect();
                    statistics.push(("failed_seed_attempts", Value::Array(failures)));
                }
                object(vec![
                    ("id", text(&c.candidate_id)),
                    ("seed", int(c.seed)),
                    ("selected", Value::Bool(c.candidate_id == self.selected_candidate)),
                    ("metrics", metrics_json(&c.metrics)),
                    ("statistics", object(statistics)),
                ])
            })
            .collect();
        let corridors = self
            .corridors
            .iter()
            .map(|c| {
                object(vec![
                    ("name", text(&c.name)),
                    ("nets", Value::Array(c.nets.iter().map(|n| text(n)).collect())),
                    ("side", text(c.side)),
                    ("margin_nm", int(c.margin_nm)),
                    ("terminal_components", Value::Array(c.terminals.iter().map(|t| text(t)).collect())),
                    (
                        "polygon_nm",
                        Value::Array(c.polygon.iter().map(|p| Value::Array(vec![int(p.x_nm), int(p.y_nm)])).collect()),
                    ),
                ])
            })
            .collect();
        let skipped = self
            .skipped
            .iter()
            .map(|(nets, reason)| {
                object(vec![("nets", Value::Array(nets.iter().map(|n| text(n)).collect())), ("reason", text(reason))])
            })
            .collect();
        object(vec![
            ("schema", text("copperscript-layout-report/v0.1")),
            ("board", text(&self.board_name)),
            ("algorithm", text(ALGORITHM)),
            ("selected_candidate", text(&self.selected_candidate)),
            ("gates", Value::Array(gates)),
            ("findings", Value::Array(findings)),
            ("metrics", metrics_json(&self.metrics)),
            ("escape_channels", Value::Array(channels)),
            ("candidates", Value::Array(candidates)),
            ("reserved_corridors", Value::Array(corridors)),
            ("skipped_corridors", Value::Array(skipped)),
        ])
    }
}

/// `plan_placement`: deterministic Pareto candidates and the selected legal placement.
pub fn plan_placement(board: &PhysicalBoard, options: &PlannerOptions) -> Result<PlacementPlan> {
    let candidates = generate_placement_candidates(board, options)?;
    let selected = select_placement_candidate(&candidates)?;
    let mut planned = board.clone();
    planned.placements = selected.placements.clone();
    for (key, value) in [
        ("prototype_placement", "false".to_owned()),
        ("planned_placement", "true".to_owned()),
        ("layout_stage", "place".to_owned()),
        ("placement_algorithm", ALGORITHM.to_owned()),
        ("placement_candidate", selected.candidate_id.clone()),
        ("placement_candidate_count", candidates.len().to_string()),
        ("fabrication_ready", "false".to_owned()),
    ] {
        planned.metadata.insert(key.to_owned(), value);
    }
    let scene = Scene::new(board, options.policy());
    let spacing = EscapeSpacingModel::new(&scene, options.escape_margin_nm, options.escape_transit_lanes)?;
    let poses: Poses = selected.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect();
    let channels = spacing.channels(&poses, false);
    let (corridors, skipped) = copper_drc::legality::corridor_reservation(board)?;
    let mut findings = findings(board, &selected.metrics, &candidates, &channels, &corridors, &skipped);
    findings.extend(placement_intent_findings(&planned)?);
    let gates = gates(&findings);
    let rows = corridors
        .iter()
        .map(|c| CorridorRow {
            name: c.keepout.name.clone(),
            nets: vec![c.nets.0.clone(), c.nets.1.clone()],
            side: c.keepout.side.map_or("both", copper_physical::BoardSide::value),
            margin_nm: c.margin_nm,
            terminals: c.terminal_references.clone(),
            polygon: c.keepout.outline.vertices.clone(),
        })
        .collect();
    let report = LayoutReport {
        board_name: board.name.clone(),
        gates,
        findings,
        metrics: selected.metrics.clone(),
        selected_candidate: selected.candidate_id.clone(),
        candidates: candidates.clone(),
        escape_channels: channels,
        corridors: rows,
        skipped,
    };
    Ok(PlacementPlan { board: planned, report })
}

fn findings(
    board: &PhysicalBoard,
    metrics: &PlacementMetrics,
    candidates: &[Candidate],
    channels: &[EscapeChannel],
    corridors: &[copper_drc::legality::ReservedCorridor],
    skipped: &[(Vec<String>, String)],
) -> Vec<Finding> {
    let mut result = Vec::new();
    let meta = |key: &str| board.metadata.get(key).map(String::as_str).unwrap_or("");
    if meta("prototype_footprints") == "true" {
        result.push(finding(
            "PROXY_FOOTPRINTS",
            Stage::Prepare,
            Status::Warning,
            "proxy footprints are sufficient for workflow testing but not physical signoff".to_owned(),
            None,
        ));
    }
    if !meta("footprint_import_warnings").is_empty() {
        result.push(finding(
            "FOOTPRINT_IMPORT_WARNINGS",
            Stage::Prepare,
            Status::Warning,
            "one or more resolved footprints have lossy-import warnings".to_owned(),
            None,
        ));
    }
    let omitted = meta("omitted_components");
    if !omitted.is_empty() {
        result.push(finding(
            "OMITTED_COMPONENTS",
            Stage::Prepare,
            Status::Warning,
            format!("components without selected footprints were omitted: {omitted}"),
            None,
        ));
    }
    let targets = meta("omitted_constraint_targets");
    if !targets.is_empty() {
        result.push(finding(
            "OMITTED_CONSTRAINT_TARGETS",
            Stage::Prepare,
            Status::Warning,
            format!("physical constraints target omitted components: {targets}"),
            None,
        ));
    }
    for channel in channels {
        result.push(finding(
            "ESCAPE_CHANNEL_DEFICIT",
            Stage::Place,
            Status::Warning,
            format!(
                "{}/{}: facing {}-channel has {:.3} mm, estimated target {:.3} mm; fixed/proximity constraints \
                 remain authoritative and joint package access must be verified",
                channel.left,
                channel.right,
                channel.axis,
                channel.gap_nm as f64 / 1_000_000.0,
                channel.required_gap_nm as f64 / 1_000_000.0
            ),
            Some(format!("{},{}", channel.left, channel.right)),
        ));
    }
    for (nets, reason) in skipped {
        result.push(finding(
            "CORRIDOR_NOT_RESERVED",
            Stage::Prepare,
            Status::Warning,
            format!("reserve_corridor for {} was not applied: {reason}", nets.join("/")),
            Some(nets.join("/")),
        ));
    }
    if !corridors.is_empty() {
        let names: Vec<String> = corridors.iter().map(|c| format!("{}/{}", c.nets.0, c.nets.1)).collect();
        result.push(finding(
            "RESERVED_CORRIDORS",
            Stage::Place,
            Status::Pass,
            format!(
                "kept {} differential-pair corridor(s) free of non-terminal components: {}",
                corridors.len(),
                names.join(", ")
            ),
            None,
        ));
    }
    if metrics.congestion_overflow != 0 {
        result.push(finding(
            "ESTIMATED_CONGESTION",
            Stage::Place,
            Status::Warning,
            format!("coarse layer-aware routing estimate has {} overflow units", metrics.congestion_overflow),
            None,
        ));
    }
    if metrics.crossing_count != 0 {
        result.push(finding(
            "ESTIMATED_CROSSINGS",
            Stage::Place,
            Status::Warning,
            format!("coarse routing estimate contains {} same-layer crossings", metrics.crossing_count),
            None,
        ));
    }
    result.push(finding(
        "PLACEMENT_CANDIDATES",
        Stage::Place,
        Status::Pass,
        format!("generated {} non-dominated deterministic placement candidate(s)", candidates.len()),
        None,
    ));
    result.push(finding(
        "DETAILED_ROUTING_NOT_RUN",
        Stage::Route,
        Status::NotRun,
        "placement includes a coarse routability estimate only; no copper routes were generated".to_owned(),
        None,
    ));
    result.push(finding(
        "PHYSICAL_SIGNOFF_NOT_RUN",
        Stage::Verify,
        Status::NotRun,
        "DRC, SI/PI, thermal, RF, DFX, and independent CAM verification remain required".to_owned(),
        None,
    ));
    result
}

fn gates(findings: &[Finding]) -> Vec<Gate> {
    let warned = |stage: Stage| findings.iter().any(|f| f.stage == stage && f.status == Status::Warning);
    let (prepare, place) = (warned(Stage::Prepare), warned(Stage::Place));
    vec![
        Gate {
            stage: Stage::Prepare,
            status: if prepare { Status::Warning } else { Status::Pass },
            summary: if prepare {
                "inputs accepted for placement exploration"
            } else {
                "physical inputs are complete for placement"
            },
        },
        Gate {
            stage: Stage::Place,
            status: if place { Status::Warning } else { Status::Pass },
            summary: if place {
                "legal placement produced with routability warnings"
            } else {
                "legal placement produced with no estimated congestion or crossings"
            },
        },
        Gate { stage: Stage::Route, status: Status::NotRun, summary: "detailed routing is not run by plan-layout" },
        Gate {
            stage: Stage::Verify,
            status: Status::Blocked,
            summary: "release is blocked until routing and signoff pass",
        },
    ]
}

/// `placement_intent_findings`: authored poses that usually fight the routing.
///
/// A fixed small part far from its nearest signal partner (edge parts
/// excepted), fixed parts in a fixed differential pair's path, and the
/// automatic pair corridors movable parts were kept out of.
pub fn placement_intent_findings(board: &PhysicalBoard) -> Result<Vec<Finding>> {
    const REMOTE_MM: f64 = 8.0;
    const EDGE_MM: f64 = 2.0;
    let scene = Scene::new(board, PlannerOptions::default().policy());
    let mut result = Vec::new();
    let poses = placement_map(board);
    let mut fixed: Vec<String> = board
        .placement_rules
        .iter()
        .map(|r| r.reference.clone())
        .collect::<IndexSet<String>>()
        .into_iter()
        .filter(|r| scene.rule(r).is_some_and(|rule| rule.fixed_position.is_some()) && poses.contains_key(r))
        .collect();
    fixed.sort();
    let mut excluded: IndexSet<&str> = scene.plane_nets.iter().map(String::as_str).collect();
    excluded.extend(board.power_domains.iter().map(|d| d.net.as_str()));
    excluded.extend(board.decoupling_links.iter().map(|l| l.return_net.as_str()));
    let outline = &board.outline.vertices;
    let edge_distance_mm = |point: copper_physical::Point| {
        let mut best = f64::INFINITY;
        for (index, a) in outline.iter().enumerate() {
            let b = outline[(index + 1) % outline.len()];
            let (dx, dy) = (b.x_nm - a.x_nm, b.y_nm - a.y_nm);
            let length2 = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
            let t = if length2 == 0 {
                0.0
            } else {
                let dot =
                    i128::from(point.x_nm - a.x_nm) * i128::from(dx) + i128::from(point.y_nm - a.y_nm) * i128::from(dy);
                py_max(0.0, py_min(1.0, true_div(dot, length2)))
            };
            let distance =
                hypot((point.x_nm - a.x_nm) as f64 - t * dx as f64, (point.y_nm - a.y_nm) as f64 - t * dy as f64);
            best = py_min(best, distance);
        }
        best / 1_000_000.0
    };
    for reference in &fixed {
        let pose = &poses[reference];
        if board.footprints[&pose.footprint].pads.len() > 4 {
            continue;
        }
        // Edge parts (connectors, antennas, switches) are fixed for mechanical reasons.
        let nearest_edge = scene.polygon(pose).into_iter().map(edge_distance_mm).reduce(py_min).expect("polygon");
        if nearest_edge <= EDGE_MM {
            continue;
        }
        let mut best: Option<(f64, String, String)> = None;
        for net in &board.nets {
            if excluded.contains(net.name.as_str()) || net.pads.len() > 12 {
                continue;
            }
            for pad in net.pads.iter().filter(|p| p.component == *reference) {
                let here = scene.pad_position(pose, &pad.pad)?;
                for other in &net.pads {
                    if other.component == *reference || !poses.contains_key(&other.component) {
                        continue;
                    }
                    let there = scene.pad_position(&poses[&other.component], &other.pad)?;
                    let distance =
                        hypot((here.x_nm - there.x_nm) as f64, (here.y_nm - there.y_nm) as f64) / 1_000_000.0;
                    if best.as_ref().is_none_or(|(d, _, _)| distance < *d) {
                        best = Some((distance, net.name.clone(), format!("{}.{}", other.component, other.pad)));
                    }
                }
            }
        }
        if let Some((distance, net, partner)) = best
            && distance > REMOTE_MM
        {
            result.push(finding(
                "REMOTE_FIXED_PART",
                Stage::Place,
                Status::Warning,
                format!(
                    "{reference} is fixed {distance:.1} mm from its nearest signal partner {partner} (net {net}); \
                     if it serves that pin, move it there or drop its fixed_placement"
                ),
                Some(reference.clone()),
            ));
        }
    }
    let corridors = scene.checker.automatic_pair_corridors()?;
    let mut blocking: IndexMap<String, Vec<String>> = IndexMap::new();
    for corridor in corridors {
        for reference in &fixed {
            if corridor.terminal_references.contains(reference) {
                continue;
            }
            let pose = &poses[reference];
            if copper_drc::legality::keepout_blocks(board, &corridor.keepout, pose, &scene.polygon(pose)) {
                blocking.entry(reference.clone()).or_default().push(format!("{}/{}", corridor.nets.0, corridor.nets.1));
            }
        }
    }
    blocking.sort_keys();
    for (reference, pairs) in blocking {
        result.push(finding(
            "FIXED_PART_IN_PAIR_CORRIDOR",
            Stage::Place,
            Status::Warning,
            format!(
                "{reference} is fixed in the straight path of differential pair(s) {}; those pairs have to route around it",
                pairs.join(", ")
            ),
            Some(reference),
        ));
    }
    if !corridors.is_empty() {
        let names: Vec<String> = corridors.iter().map(|c| format!("{}/{}", c.nets.0, c.nets.1)).collect();
        result.push(finding(
            "AUTO_PAIR_CORRIDORS",
            Stage::Place,
            Status::Pass,
            format!(
                "kept movable parts out of {} differential-pair path(s) between fixed terminals: {}",
                corridors.len(),
                names.join(", ")
            ),
            None,
        ));
    }
    Ok(result)
}

/// `json.dumps(document, indent=2, sort_keys=True) + "\n"`.
pub fn report_json(document: &Value) -> String {
    copper_json::Dumps::new().indent(2).sort_keys(true).dumps(document) + "\n"
}

/// An empty JSON object.
pub fn empty_object() -> Object {
    Object::new()
}

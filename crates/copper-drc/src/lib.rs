//! Fail-closed physical design-rule checking (`pcbir.drc`): exact copper
//! connectivity, placement legality and the copper, drill and keepout
//! checks, plus hard-macro acceptance (`pcbir.hard_macros`
//! `materialize_hard_macros`), which needs them.
//!
//! [`run_physical_drc`] returns the findings, coverage and decision. The
//! signoff token (board and report digests), waivers and the breakout and
//! hole-clearance relaxation records are not yet rewritten; they never
//! change a finding. Filled-zone checks need zone fills, which only the
//! KiCad manufacturing stage produces.

pub mod breakout;
pub mod connectivity;
pub mod digest;
pub mod legality;
pub mod macros;
pub mod signal_integrity;

use indexmap::{IndexMap, IndexSet};

use copper_lang::silkscreen::{Artwork, Graphic, artwork_bounds, artwork_layer};
use copper_pcb::board::*;
use copper_pcb::keepouts::{placement_map, resolved_copper_keepouts};
use copper_pcb::transform::{placed_pad_shape, transformed_local_point};
use copper_pcb::{Error, Result};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape};
use copper_physical::geometry::{
    RoundedConvexShape, fraction_sqrt_floor, point_in_polygon, point_segment_distance_at_least,
    point_segment_distance_squared, polygon_triangles, rational, ring_edges, segment_distance_at_least,
    segment_distance_squared, segment_in_polygon, segments_intersect, shape_distance_squared, shapes_clear,
    shapes_touch_or_overlap,
};
use copper_physical::mechanical::{hole_shape, shape_in_outline, slot_shape};
use copper_physical::pyfloat::{hypot, round_int};
use copper_physical::{BoardSide, CopperLayer, Point};

use crate::breakout::BreakoutSpacing;
use crate::connectivity::{Candidate, explicit_copper_connectivity};

/// A rounded convex shape from parts validated upstream.
pub(crate) fn shape(spine: Vec<Point>, radius_nm: i64) -> Result<RoundedConvexShape> {
    Ok(RoundedConvexShape::new(spine, radius_nm)?)
}

/// A plated pad's drill as an SMD-like land (circle, or oval when elongated).
pub(crate) fn drill_shape(position: Point, pad: &FootprintPad, placement: &Placement) -> RoundedConvexShape {
    let drill = pad.drill.expect("a drilled pad");
    let kind = if pad.kind == PadKind::NonPlatedThroughHole { pad.kind } else { PadKind::Smd };
    let drill_pad = FootprintPad {
        size: drill,
        kind,
        drill: if kind == PadKind::NonPlatedThroughHole { Some(drill) } else { None },
        shape: if drill.width_nm == drill.height_nm { PadShape::Circle } else { PadShape::Oval },
        ..pad.clone()
    };
    placed_pad_shape(position, &drill_pad, placement)
}

/// `DrcSeverity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn value(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// `DrcFinding` (always active: waivers are not rewritten).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub objects: Vec<String>,
    pub nets: Vec<String>,
    pub layers: Vec<String>,
    pub required_nm: Option<i64>,
    pub measured_nm: Option<i64>,
}

impl Finding {
    fn new(code: &str, severity: Severity, message: String) -> Self {
        Finding {
            code: code.to_owned(),
            severity,
            message,
            objects: Vec::new(),
            nets: Vec::new(),
            layers: Vec::new(),
            required_nm: None,
            measured_nm: None,
        }
    }

    fn error(code: &str, message: String) -> Self {
        Finding::new(code, Severity::Error, message)
    }

    fn objects<S: Into<String>>(mut self, objects: impl IntoIterator<Item = S>) -> Self {
        self.objects = objects.into_iter().map(Into::into).collect();
        self
    }

    fn nets<S: Into<String>>(mut self, nets: impl IntoIterator<Item = S>) -> Self {
        self.nets = nets.into_iter().map(Into::into).collect();
        self
    }

    fn layers<S: Into<String>>(mut self, layers: impl IntoIterator<Item = S>) -> Self {
        self.layers = layers.into_iter().map(Into::into).collect();
        self
    }

    fn required(mut self, value: i64) -> Self {
        self.required_nm = Some(value);
        self
    }

    fn measured(mut self, value: i64) -> Self {
        self.measured_nm = Some(value);
        self
    }

    /// `_finding_key`.
    fn key(&self) -> (&str, &[String], &[String], &[String], &str) {
        (&self.code, &self.objects, &self.nets, &self.layers, &self.message)
    }
}

fn sorted_pair(first: &str, second: &str) -> Vec<String> {
    let mut pair = vec![first.to_owned(), second.to_owned()];
    pair.sort();
    pair
}

fn sorted_unique(items: &[&str]) -> Vec<String> {
    let set: IndexSet<&str> = items.iter().copied().collect();
    let mut result: Vec<String> = set.into_iter().map(str::to_owned).collect();
    result.sort();
    result
}

/// `DrcCoverageStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverageStatus {
    Executed,
    NotApplicable,
    Unsupported,
    Failed,
}

/// `DrcCoverage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    pub check: String,
    pub status: CoverageStatus,
    pub required: bool,
    pub detail: String,
}

/// `DrcDecision` without waivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Pass,
    Fail,
}

/// `PhysicalDrcPolicy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub require_completed_detailed_route: bool,
    pub fail_on_warnings: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { require_completed_detailed_route: true, fail_on_warnings: false }
    }
}

/// `PhysicalDrcReport` without the signoff token and relaxation records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub decision: Decision,
    pub complete: bool,
    pub findings: Vec<Finding>,
    pub coverage: Vec<Coverage>,
}

/// `EXTERNAL_PLACEMENT_KEY`.
pub const EXTERNAL_PLACEMENT_KEY: &str = "placement_legality";

fn coverage(check: &str, status: CoverageStatus, required: bool, detail: &str) -> Coverage {
    Coverage { check: check.to_owned(), status, required, detail: detail.to_owned() }
}

/// `run_physical_drc`: check the exact physical IR.
pub fn run_physical_drc(board: &PhysicalBoard, policy: Policy) -> Result<Report> {
    use CoverageStatus::*;
    let mut findings: Vec<Finding> = Vec::new();
    let mut covered: Vec<Coverage> = Vec::new();
    if policy.require_completed_detailed_route
        && board.metadata.get("detailed_routing").map(String::as_str) != Some("complete")
    {
        findings.push(Finding::error(
            "DRC-ROUTE-INCOMPLETE",
            "detailed routing is not complete for this exact board".to_owned(),
        ));
        covered.push(coverage("route_completeness", Failed, true, ""));
    } else {
        covered.push(coverage("route_completeness", Executed, true, ""));
    }
    check_connectivity(board, &mut findings)?;
    covered.push(coverage("connectivity", Executed, true, ""));
    let external = board.metadata.get(EXTERNAL_PLACEMENT_KEY).map(String::as_str) == Some("external");
    if external {
        covered.push(coverage("placement_legality", NotApplicable, true, ""));
    } else {
        let poses = placement_map(board);
        if !legality::placement_solution_is_legal(board, &poses)? {
            let reasons = legality::placement_rejection_reasons(board, &poses)?;
            findings.push(Finding::error(
                "DRC-PLACEMENT",
                format!("component placement violates board, keepout, or courtyard legality: {}", reasons.join("; ")),
            ));
        }
        covered.push(coverage("placement_legality", Executed, true, ""));
    }
    let gap_rules = board.placement_rules.iter().any(|r| r.additional_courtyard_gap_nm.is_some());
    covered.push(coverage("courtyard_gap", if gap_rules { Executed } else { NotApplicable }, true, ""));
    check_track_rules(board, &mut findings)?;
    covered.push(coverage("track_width_and_budgets", Executed, true, ""));
    check_differential_rules(board, &mut findings);
    covered.push(coverage("differential_geometry", Executed, true, ""));
    check_length_match(board, &mut findings)?;
    covered.push(coverage(
        "length_match",
        if board.match_groups.is_empty() { NotApplicable } else { Executed },
        true,
        if board.match_groups.is_empty() { "" } else { "group skew is checked once every member net is connected" },
    ));
    let returns = check_single_ended_return_vias(board, &mut findings)?;
    covered.push(coverage(
        "single_ended_return_vias",
        if returns { Executed } else { NotApplicable },
        true,
        "exact existing-barrel span and center distance; no filled-reference or SI proof",
    ));
    check_vias(board, &mut findings);
    covered.push(coverage("via_geometry_and_budgets", Executed, true, ""));
    check_board_edge(board, &mut findings)?;
    covered.push(coverage("copper_to_board_edge", Executed, true, ""));
    check_copper_spacing(board, &mut findings)?;
    check_fixed_polygons(board, &mut findings)?;
    check_copper_artwork(board, &mut findings)?;
    covered.push(coverage("shorts_and_clearance", Executed, true, ""));
    let copper_art: Vec<&Artwork> = board.silkscreen.iter().filter(|item| artwork_layer(item).is_some()).collect();
    if !copper_art.is_empty() {
        covered.push(coverage(
            "copper_artwork_envelopes",
            Executed,
            true,
            "conservative immutable reservations checked against copper, board material and holes",
        ));
        if copper_art.iter().any(|item| matches!(item, Artwork::Text { .. })) {
            covered.push(coverage(
                "copper_artwork_glyphs",
                Unsupported,
                true,
                "native font geometry is not normalized; copper text export is for inspection, not physical signoff",
            ));
        }
        if copper_art.iter().any(|item| artwork_filled(item)) {
            covered.push(coverage(
                "copper_artwork_feature_width",
                Unsupported,
                true,
                "minimum feature widths of filled artwork require final native/CAM geometry verification",
            ));
        }
        if copper_art.iter().any(|item| artwork_exposed(item)) {
            covered.push(coverage(
                "copper_artwork_mask",
                Unsupported,
                true,
                "matching mask openings require final native mask registration/sliver qualification",
            ));
        }
    }
    if board.net_routing_rules.iter().any(|r| r.breakout_length_nm.is_some()) {
        covered.push(coverage(
            "breakout_regions",
            Executed,
            true,
            "breakout width, pair gap and clearance apply only to copper within breakout_length of a terminal land; checks they relaxed are listed in breakout_relaxations",
        ));
    }
    check_non_plated_hole_clearance(board, &mut findings)?;
    covered.push(coverage("non_plated_hole_clearance", Executed, true, ""));
    if !board.component_hole_clearances.is_empty() {
        covered.push(coverage(
            "component_hole_clearance",
            Executed,
            true,
            "scoped hole_clearance applies only between a component's own pads and its own non-plated holes; checks it relaxed are listed in hole_clearance_relaxations",
        ));
    }
    check_via_hole_clearance(board, &mut findings)?;
    covered.push(coverage("via_hole_to_copper", Executed, true, ""));
    check_drill_spacing(board, &mut findings)?;
    covered.push(coverage("drill_to_drill_spacing", Executed, true, ""));
    let keepouts_covered = check_copper_keepouts(board, &mut findings)?;
    check_zone_routing_reservations(board, &mut findings)?;
    check_zone_priorities(board, &mut findings);
    if !board.zones.is_empty() {
        covered.push(coverage(
            "zone_priority",
            Executed,
            true,
            "intersecting or touching zone boundaries on one layer require distinct priorities",
        ));
    }
    if board.zones.iter().any(|z| z.reserve_routing) {
        covered.push(coverage(
            "zone_routing_reservations",
            Executed,
            true,
            "net-aware reserved polygons checked against all tracks, including macro copper",
        ));
    }
    covered.push(coverage(
        "copper_keepouts",
        if keepouts_covered { Executed } else { Failed },
        true,
        "polygonal keepout checks for tracks, vias, and component pads",
    ));
    covered.push(coverage(
        "copper_zones",
        if board.zones.is_empty() { NotApplicable } else { Unsupported },
        false,
        if board.zones.is_empty() {
            ""
        } else {
            "zone intent is refilled and checked by the pinned KiCad manufacturing stage"
        },
    ));
    for (name, detail) in [
        ("solder_mask", "mask sliver and expansion checks require final artwork"),
        ("silkscreen", "silkscreen-to-mask checks require final artwork"),
        ("creepage", "creepage requires a declared safety profile"),
        ("signal_integrity", "impedance and signal integrity require external qualification"),
        ("thermal", "thermal/current qualification requires external analysis"),
    ] {
        covered.push(coverage(name, Unsupported, false, detail));
    }
    if external {
        findings = findings.into_iter().map(as_input_copper).collect();
    }
    findings.sort_by(|a, b| a.key().cmp(&b.key()));
    let complete = covered.iter().all(|c| !c.required || matches!(c.status, Executed | NotApplicable));
    let blocking = findings
        .iter()
        .any(|f| f.severity == Severity::Error || (policy.fail_on_warnings && f.severity == Severity::Warning));
    let decision = if blocking || !complete { Decision::Fail } else { Decision::Pass };
    Ok(Report { decision, complete, findings, coverage: covered })
}

/// `_as_input_copper`: a pad-only spacing finding on an imported placement is input copper.
fn as_input_copper(item: Finding) -> Finding {
    const SOURCES: [&str; 6] = [
        "DRC-BOARD-EDGE",
        "DRC-CLEARANCE",
        "DRC-SHORT",
        "DRC-HOLE-CLEARANCE",
        "DRC-DRILL-SPACING",
        "DRC-COPPER-KEEPOUT",
    ];
    if !SOURCES.contains(&item.code.as_str())
        || item.objects.is_empty()
        || !item.objects.iter().all(|o| o.starts_with("pad:"))
    {
        return item;
    }
    Finding {
        message: format!("{} in the imported board's placed pads: {}", item.code, item.message),
        code: "DRC-INPUT-COPPER".to_owned(),
        severity: Severity::Warning,
        ..item
    }
}

fn artwork_filled(item: &Artwork) -> bool {
    match item {
        Artwork::Graphic { graphic, .. } => matches!(
            graphic,
            Graphic::Rectangle { filled: true, .. }
                | Graphic::Circle { filled: true, .. }
                | Graphic::Polygon { filled: true, .. }
        ),
        Artwork::Text { .. } => false,
    }
}

fn artwork_exposed(item: &Artwork) -> bool {
    match item {
        Artwork::Text { exposed, .. } | Artwork::Graphic { exposed, .. } => *exposed,
    }
}

fn artwork_id(item: &Artwork) -> &str {
    match item {
        Artwork::Text { id, .. } | Artwork::Graphic { id, .. } => id,
    }
}

fn artwork_width(item: &Artwork) -> i64 {
    match item {
        Artwork::Text { thickness_nm, .. } => *thickness_nm,
        Artwork::Graphic { graphic, .. } => graphic.width_nm(),
    }
}

fn capsule(track: &TrackSegment) -> Result<RoundedConvexShape> {
    shape(vec![track.start, track.end], track.width_nm / 2)
}

fn disk(position: Point, diameter_nm: i64) -> Result<RoundedConvexShape> {
    shape(vec![position], diameter_nm / 2)
}

fn layer_indexes(board: &PhysicalBoard) -> IndexMap<CopperLayer, usize> {
    board.stackup.copper_layers.iter().enumerate().map(|(i, l)| (*l, i)).collect()
}

/// `_via_covers_layer`.
fn via_covers_layer(board: &PhysicalBoard, via: &Via, layer: CopperLayer) -> Result<bool> {
    let indexes = layer_indexes(board);
    let index = |l: CopperLayer| indexes.get(&l).copied().ok_or_else(|| Error::Value(format!("{:?}", l.value())));
    let (a, b) = (index(via.from_layer)?, index(via.to_layer)?);
    let at = index(layer)?;
    Ok(a.min(b) <= at && at <= a.max(b))
}

/// `_via_spans_overlap`.
fn via_spans_overlap(board: &PhysicalBoard, first: &Via, second: &Via) -> Result<bool> {
    for layer in &board.stackup.copper_layers {
        if via_covers_layer(board, first, *layer)? && via_covers_layer(board, second, *layer)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn routed_length(board: &PhysicalBoard, net: &str) -> i64 {
    board
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| round_int(hypot((t.end.x_nm - t.start.x_nm) as f64, (t.end.y_nm - t.start.y_nm) as f64)))
        .sum()
}

/// `_check_connectivity`.
fn check_connectivity(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let graph = explicit_copper_connectivity(board, Candidate::default())?;
    let mut nets: Vec<&PhysicalNet> = board.nets.iter().collect();
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    for net in nets {
        if !graph.net_connected(net) {
            let mut pads = net.pads.clone();
            pads.sort();
            findings.push(
                Finding::error(
                    "DRC-OPEN-NET",
                    format!(
                        "net {} is not connected by exact copper and declared internal connections",
                        copper_json::python_str_repr(&net.name)
                    ),
                )
                .nets([net.name.as_str()])
                .objects(pads.iter().map(|p| format!("pad:{}.{}", p.component, p.pad))),
            );
        }
    }
    Ok(())
}

/// `_check_track_rules`.
fn check_track_rules(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let rules: IndexMap<&str, &NetRoutingRule> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let spacing = rules.values().any(|r| r.breakout_width_nm.is_some()).then(|| BreakoutSpacing::new(board));
    let contracts = copper_pcb::hard_macros::materialized_track_width_contracts(board)?;
    for (index, track) in board.tracks.iter().enumerate() {
        let rule = rules.get(track.net.as_str());
        let required = board.rules.minimum_track_width_nm.max(rule.and_then(|r| r.width_nm).unwrap_or(0));
        let mut relaxed = required;
        if let Some((hard, contract)) = contracts.get(&index) {
            relaxed = board.rules.minimum_track_width_nm.max(contract.minimum_width_nm);
            findings.push(
                Finding::new(
                    "DRC-MACRO-WIDTH-CONTRACT",
                    Severity::Info,
                    format!(
                        "track {index}: immutable macro {}, asset {}, purpose {}; host minimum {required} nm; {}",
                        copper_json::python_str_repr(&hard.cluster),
                        hard.asset_sha256,
                        copper_json::python_str_repr(&contract.purpose),
                        contract.evidence
                    ),
                )
                .objects([format!("track:{index}")])
                .nets([track.net.as_str()])
                .layers([track.layer.value()])
                .required(relaxed)
                .measured(track.width_nm),
            );
        } else if let (Some(spacing), Some(rule)) = (&spacing, rule)
            && let Some(width) = rule.breakout_width_nm
            && spacing.land(&track.net, &[track.start, track.end]).is_some()
        {
            relaxed = required.min(board.rules.minimum_track_width_nm.max(width));
        }
        if track.width_nm < relaxed {
            findings.push(
                Finding::error(
                    "DRC-TRACK-WIDTH",
                    format!(
                        "track {index} on {} is below its minimum or routing profile width",
                        copper_json::python_str_repr(&track.net)
                    ),
                )
                .objects([format!("track:{index}")])
                .nets([track.net.as_str()])
                .layers([track.layer.value()])
                .required(relaxed)
                .measured(track.width_nm),
            );
        }
    }
    let mut sorted: Vec<(&&str, &&NetRoutingRule)> = rules.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (net, rule) in sorted {
        let length = routed_length(board, net);
        if let Some(maximum) = rule.max_length_nm
            && length > maximum
        {
            findings.push(
                Finding::new(
                    "DRC-MAX-LENGTH",
                    Severity::Warning,
                    format!("net {} exceeds its maximum routed length", copper_json::python_str_repr(net)),
                )
                .nets([*net])
                .required(maximum)
                .measured(length),
            );
        }
        let disallowed: Vec<&str> = board
            .tracks
            .iter()
            .filter(|t| t.net == **net && !rule.allowed_layers.is_empty() && !rule.allowed_layers.contains(&t.layer))
            .map(|t| t.layer.value())
            .collect();
        if !disallowed.is_empty() {
            findings.push(
                Finding::error(
                    "DRC-LAYER",
                    format!("net {} uses a layer excluded by its routing profile", copper_json::python_str_repr(net)),
                )
                .nets([*net])
                .layers(sorted_unique(&disallowed)),
            );
        }
    }
    Ok(())
}

/// `_check_vias`.
fn check_vias(board: &PhysicalBoard, findings: &mut Vec<Finding>) {
    let layers = &board.stackup.copper_layers;
    for (index, via) in board.vias.iter().enumerate() {
        if !layers.contains(&via.from_layer) || !layers.contains(&via.to_layer) {
            findings.push(
                Finding::error("DRC-VIA-SPAN", format!("via {index} references a layer outside the stackup"))
                    .objects([format!("via:{index}")])
                    .nets([via.net.as_str()]),
            );
        }
        if via.size_nm - via.drill_nm <= 0 {
            findings.push(
                Finding::error("DRC-ANNULAR-RING", format!("via {index} has no annular ring"))
                    .objects([format!("via:{index}")])
                    .nets([via.net.as_str()]),
            );
        }
    }
    let mut rules: Vec<&NetRoutingRule> = board.net_routing_rules.iter().collect();
    rules.sort_by(|a, b| a.net.cmp(&b.net));
    rules.dedup_by(|a, b| a.net == b.net);
    for rule in rules {
        let count = board.vias.iter().filter(|v| v.net == rule.net).count() as i64;
        if let Some(maximum) = rule.max_vias
            && count > maximum
        {
            findings.push(
                Finding::error(
                    "DRC-MAX-VIAS",
                    format!("net {} exceeds its maximum via count", copper_json::python_str_repr(&rule.net)),
                )
                .nets([rule.net.as_str()])
                .required(maximum)
                .measured(count),
            );
        }
    }
}

/// `_check_differential_rules`.
fn check_differential_rules(board: &PhysicalBoard, findings: &mut Vec<Finding>) {
    let rules: IndexMap<&str, &NetRoutingRule> = board.net_routing_rules.iter().map(|r| (r.net.as_str(), r)).collect();
    let mut processed: Vec<Vec<String>> = Vec::new();
    let mut sorted: Vec<&NetRoutingRule> = board.net_routing_rules.iter().collect();
    sorted.sort_by(|a, b| a.net.cmp(&b.net));
    for rule in sorted {
        let Some(partner_net) = &rule.differential_partner else { continue };
        let pair = sorted_unique(&[rule.net.as_str(), partner_net.as_str()]);
        if processed.contains(&pair) {
            continue;
        }
        processed.push(pair.clone());
        let partner = rules.get(partner_net.as_str());
        let Some(partner) = partner.filter(|p| p.differential_partner.as_deref() == Some(rule.net.as_str())) else {
            findings.push(
                Finding::error("DRC-DIFF-PROFILE", "differential routing profiles are not symmetric".to_owned())
                    .nets(pair.clone()),
            );
            continue;
        };
        let skew = (routed_length(board, &rule.net) - routed_length(board, partner_net)).abs();
        let limits: Vec<i64> = [rule.max_skew_nm, partner.max_skew_nm].into_iter().flatten().collect();
        if let Some(limit) = limits.iter().min()
            && skew > *limit
        {
            findings.push(
                Finding::new("DRC-DIFF-SKEW", Severity::Warning, format!("differential pair skew is {skew} nm"))
                    .nets(pair.clone())
                    .required(*limit)
                    .measured(skew),
            );
        }
        let counts: IndexSet<usize> =
            pair.iter().map(|net| board.vias.iter().filter(|v| v.net == *net).count()).collect();
        if counts.len() != 1 {
            findings.push(
                Finding::error("DRC-DIFF-VIAS", "differential pair members have unequal via transitions".to_owned())
                    .nets(pair),
            );
        }
    }
}

/// `return_span_covers`.
fn return_span_covers(board: &PhysicalBoard, returned: &Via, first: CopperLayer, last: CopperLayer) -> bool {
    let indexes = layer_indexes(board);
    let (Some(f), Some(l), Some(x), Some(y)) =
        (indexes.get(&first), indexes.get(&last), indexes.get(&returned.from_layer), indexes.get(&returned.to_layer))
    else {
        return false;
    };
    let (a, b) = (f.min(l), f.max(l));
    let (x, y) = (x.min(y), x.max(y));
    x <= a && y >= b
}

/// `_check_single_ended_return_vias`.
fn check_single_ended_return_vias(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<bool> {
    let rules: IndexMap<&str, &NetRoutingRule> = board
        .net_routing_rules
        .iter()
        .filter(|r| r.require_return_vias && r.differential_partner.is_none())
        .map(|r| (r.net.as_str(), r))
        .collect();
    for (index, via) in board.vias.iter().enumerate() {
        let Some(rule) = rules.get(via.net.as_str()) else { continue };
        let covered = match (rule.maximum_return_via_distance_nm, &rule.return_via_net) {
            (Some(limit), Some(net)) if *net != via.net => board.vias.iter().any(|other| {
                let (dx, dy) = (
                    i128::from(other.position.x_nm - via.position.x_nm),
                    i128::from(other.position.y_nm - via.position.y_nm),
                );
                other.net == *net
                    && return_span_covers(board, other, via.from_layer, via.to_layer)
                    && dx * dx + dy * dy <= i128::from(limit) * i128::from(limit)
            }),
            _ => false,
        };
        if !covered {
            let net = rule.return_via_net.as_deref().unwrap_or("None");
            let limit = rule.maximum_return_via_distance_nm.map_or_else(|| "None".to_owned(), |v| v.to_string());
            let mut finding = Finding::error(
                "DRC-RETURN-VIA",
                format!("signal via {index} lacks an existing {net} return barrel covering its span within {limit} nm"),
            )
            .objects([format!("via:{index}")])
            .nets([via.net.as_str()]);
            finding.required_nm = rule.maximum_return_via_distance_nm;
            findings.push(finding);
        }
    }
    Ok(!rules.is_empty())
}

/// `_check_length_match` with `signal_integrity.verify_match_groups`.
fn check_length_match(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    if board.match_groups.is_empty() {
        return Ok(());
    }
    let members: IndexSet<String> = board.match_groups.iter().flat_map(|g| g.nets.iter().cloned()).collect();
    let graph = explicit_copper_connectivity(board, Candidate { only_nets: Some(&members), ..Candidate::default() })?;
    let physical: IndexMap<&str, &PhysicalNet> = board.nets.iter().map(|n| (n.name.as_str(), n)).collect();
    for group in &board.match_groups {
        let lengths: Vec<(&str, i64, bool)> = group
            .nets
            .iter()
            .map(|net| (net.as_str(), routed_length(board, net), graph.net_connected(physical[net.as_str()])))
            .collect();
        if !lengths.iter().all(|(_, _, routed)| *routed) {
            continue;
        }
        let skew =
            lengths.iter().map(|m| m.1).max().expect("members") - lengths.iter().map(|m| m.1).min().expect("members");
        if skew <= group.max_skew_nm {
            continue;
        }
        let listed: Vec<String> = lengths.iter().map(|(net, length, _)| format!("{net} {length} nm")).collect();
        let nets: Vec<&str> = lengths.iter().map(|m| m.0).collect();
        let mut nets: Vec<String> = nets.iter().map(|n| (*n).to_owned()).collect();
        nets.sort();
        findings.push(
            Finding::new(
                "DRC-LENGTH-MATCH",
                Severity::Warning,
                format!(
                    "length-match group {} skew is {skew} nm ({})",
                    copper_json::python_str_repr(&group.id),
                    listed.join(", ")
                ),
            )
            .objects([format!("match_group:{}", group.id)])
            .nets(nets)
            .required(group.max_skew_nm)
            .measured(skew),
        );
    }
    Ok(())
}

/// `_PadCopper`.
struct PadCopper {
    identity: String,
    net: String,
    shape: RoundedConvexShape,
    layers: Vec<CopperLayer>,
    clearance_nm: i64,
    component: String,
}

/// `_copper_pads`: every plated land, unconnected ones on their own net.
fn copper_pads(board: &PhysicalBoard) -> Vec<PadCopper> {
    let pad_nets: IndexMap<&PadReference, &str> =
        board.nets.iter().flat_map(|n| n.pads.iter().map(move |p| (p, n.name.as_str()))).collect();
    let mut result = Vec::new();
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in placements {
        let footprint = &board.footprints[&placement.footprint];
        for (index, pad) in footprint.pads.iter().enumerate() {
            if matches!(pad.kind, PadKind::NonPlatedThroughHole | PadKind::Aperture) {
                continue;
            }
            let reference = PadReference::new(&placement.reference, &pad.number);
            let net = pad_nets.get(&reference).map_or_else(
                || format!("<unconnected:{}.{}:{index}>", placement.reference, pad.number),
                |n| (*n).to_owned(),
            );
            let layers = if pad.kind == PadKind::Smd {
                vec![if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
            } else {
                board.stackup.copper_layers.clone()
            };
            let position = transformed_local_point(placement, pad.position);
            result.push(PadCopper {
                identity: format!("pad:{}.{}:{index}", placement.reference, pad.number),
                net,
                shape: placed_pad_shape(position, pad, placement),
                layers,
                clearance_nm: footprint.clearance_nm.unwrap_or(0),
                component: placement.reference.clone(),
            });
        }
    }
    result
}

/// `_point_segment_distance` (drc): the float distance.
fn float_point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    legality::float_point_segment_distance(point, start, end)
}

fn spine_edges(spine: &[Point]) -> Vec<(Point, Point)> {
    match spine.len() {
        1 => vec![(spine[0], spine[0])],
        2 => vec![(spine[0], spine[1])],
        _ => ring_edges(spine),
    }
}

/// `_check_board_edge`.
fn check_board_edge(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let clearance = board.rules.minimum_clearance_nm;
    let outline = &board.outline;
    if outline.circular_boundary.is_some() || outline.boundary_path.is_some() {
        let mut objects: Vec<(String, String, RoundedConvexShape)> = Vec::new();
        for (i, t) in board.tracks.iter().enumerate() {
            objects.push((format!("track:{i}"), t.net.clone(), shape(vec![t.start, t.end], (t.width_nm + 1) / 2)?));
        }
        for (i, v) in board.vias.iter().enumerate() {
            objects.push((format!("via:{i}"), v.net.clone(), shape(vec![v.position], (v.size_nm + 1) / 2)?));
        }
        objects.extend(copper_pads(board).into_iter().map(|p| (p.identity, p.net, p.shape)));
        for (identity, net, copper) in objects {
            if !shape_in_outline(&copper, outline, clearance)? {
                findings.push(
                    Finding::error("DRC-BOARD-EDGE", format!("{identity} violates circular board/cutout clearance"))
                        .objects([identity.as_str()])
                        .nets([net])
                        .required(clearance),
                );
            }
        }
        return Ok(());
    }
    let mut edges = ring_edges(&outline.vertices);
    for cutout in &outline.cutouts {
        edges.extend(ring_edges(&cutout.vertices));
    }
    for (index, track) in board.tracks.iter().enumerate() {
        let nearest = edges
            .iter()
            .map(|(a, b)| fraction_sqrt_floor(&segment_distance_squared(track.start, track.end, *a, *b)))
            .min()
            .expect("outline edges");
        let margin = nearest as f64 - track.width_nm as f64 / 2.0;
        let inside =
            shape_in_outline(&shape(vec![track.start, track.end], (track.width_nm + 1) / 2)?, outline, clearance)?;
        if !inside || margin < clearance as f64 {
            findings.push(
                Finding::error("DRC-BOARD-EDGE", format!("track {index} violates copper-to-board-edge clearance"))
                    .objects([format!("track:{index}")])
                    .nets([track.net.as_str()])
                    .layers([track.layer.value()])
                    .required(clearance)
                    .measured(round_int(margin).max(0)),
            );
        }
    }
    for (index, via) in board.vias.iter().enumerate() {
        let nearest =
            edges.iter().map(|(a, b)| float_point_segment_distance(via.position, *a, *b)).fold(f64::INFINITY, f64::min);
        let margin = nearest - via.size_nm as f64 / 2.0;
        if !shape_in_outline(&shape(vec![via.position], (via.size_nm + 1) / 2)?, outline, clearance)?
            || margin < clearance as f64
        {
            findings.push(
                Finding::error("DRC-BOARD-EDGE", format!("via {index} violates copper-to-board-edge clearance"))
                    .objects([format!("via:{index}")])
                    .nets([via.net.as_str()])
                    .required(clearance)
                    .measured(round_int(margin).max(0)),
            );
        }
    }
    for pad in copper_pads(board) {
        let nearest = spine_edges(&pad.shape.spine)
            .iter()
            .flat_map(|(s, e)| {
                edges.iter().map(move |(a, b)| fraction_sqrt_floor(&segment_distance_squared(*s, *e, *a, *b)))
            })
            .min()
            .expect("edges");
        let margin = nearest - pad.shape.radius_nm;
        if !shape_in_outline(&pad.shape, outline, clearance)? || margin < clearance {
            findings.push(
                Finding::error("DRC-BOARD-EDGE", format!("{} violates copper-to-board-edge clearance", pad.identity))
                    .objects([pad.identity.as_str()])
                    .nets([pad.net.as_str()])
                    .layers(pad.layers.iter().map(|l| l.value()))
                    .required(clearance)
                    .measured(margin.max(0)),
            );
        }
    }
    Ok(())
}

fn spacing_finding(
    distance_squared: &num_rational::BigRational,
    message: String,
    objects: [String; 2],
    nets: Vec<String>,
    layers: Vec<String>,
    required: i64,
) -> Finding {
    let code = if num_traits::Zero::is_zero(distance_squared) { "DRC-SHORT" } else { "DRC-CLEARANCE" };
    Finding::error(code, message)
        .objects(objects)
        .nets(nets)
        .layers(layers)
        .required(required)
        .measured(fraction_sqrt_floor(distance_squared))
}

/// `_check_copper_spacing`: shorts and clearance between foreign copper.
fn check_copper_spacing(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let spacing = BreakoutSpacing::new(board);
    let track_lands: Vec<Option<String>> =
        board.tracks.iter().map(|t| spacing.land(&t.net, &[t.start, t.end])).collect();
    let via_lands: Vec<Option<String>> = board.vias.iter().map(|v| spacing.land(&v.net, &[v.position])).collect();
    for (left_index, left) in board.tracks.iter().enumerate() {
        for (right_index, right) in board.tracks.iter().enumerate().skip(left_index + 1) {
            if left.net == right.net || left.layer != right.layer {
                continue;
            }
            let (clearance, _) = spacing.between(
                &left.net,
                track_lands[left_index].as_deref(),
                &right.net,
                track_lands[right_index].as_deref(),
            );
            let required_twice = left.width_nm + right.width_nm + 2 * clearance;
            if !segment_distance_at_least(left.start, left.end, right.start, right.end, required_twice, 2) {
                let distance = segment_distance_squared(left.start, left.end, right.start, right.end);
                findings.push(spacing_finding(
                    &distance,
                    format!("tracks {left_index} and {right_index} violate {}", spacing.subject(&left.net, &right.net)),
                    [format!("track:{left_index}"), format!("track:{right_index}")],
                    sorted_pair(&left.net, &right.net),
                    vec![left.layer.value().to_owned()],
                    (required_twice + 1) / 2,
                ));
            }
        }
    }
    for (track_index, track) in board.tracks.iter().enumerate() {
        for (via_index, via) in board.vias.iter().enumerate() {
            if track.net == via.net || !via_covers_layer(board, via, track.layer)? {
                continue;
            }
            let (clearance, _) = spacing.between(
                &track.net,
                track_lands[track_index].as_deref(),
                &via.net,
                via_lands[via_index].as_deref(),
            );
            let required_twice = track.width_nm + via.size_nm + 2 * clearance;
            if !point_segment_distance_at_least(via.position, track.start, track.end, required_twice, 2) {
                let distance = point_segment_distance_squared(via.position, track.start, track.end);
                findings.push(spacing_finding(
                    &distance,
                    format!(
                        "track {track_index} and via {via_index} violate {}",
                        spacing.subject(&track.net, &via.net)
                    ),
                    [format!("track:{track_index}"), format!("via:{via_index}")],
                    sorted_pair(&track.net, &via.net),
                    vec![track.layer.value().to_owned()],
                    (required_twice + 1) / 2,
                ));
            }
        }
    }
    for (left_index, left) in board.vias.iter().enumerate() {
        for (right_index, right) in board.vias.iter().enumerate().skip(left_index + 1) {
            if left.net == right.net || !via_spans_overlap(board, left, right)? {
                continue;
            }
            let (dx, dy) = (
                i128::from(left.position.x_nm - right.position.x_nm),
                i128::from(left.position.y_nm - right.position.y_nm),
            );
            let distance_squared = dx * dx + dy * dy;
            let (clearance, _) = spacing.between(
                &left.net,
                via_lands[left_index].as_deref(),
                &right.net,
                via_lands[right_index].as_deref(),
            );
            let required_twice = i128::from(left.size_nm + right.size_nm + 2 * clearance);
            if 4 * distance_squared < required_twice * required_twice {
                let distance = rational(distance_squared);
                findings.push(
                    spacing_finding(
                        &distance,
                        format!(
                            "vias {left_index} and {right_index} violate {}",
                            spacing.subject(&left.net, &right.net)
                        ),
                        [format!("via:{left_index}"), format!("via:{right_index}")],
                        sorted_pair(&left.net, &right.net),
                        Vec::new(),
                        ((required_twice + 1) / 2) as i64,
                    )
                    .measured(fraction_sqrt_floor(&distance)),
                );
            }
        }
    }
    let pads = copper_pads(board);
    let pad_lands: Vec<Option<String>> = pads.iter().map(|p| spacing.land(&p.net, &p.shape.spine)).collect();
    let pad_reach = |pad: &PadCopper| {
        let mut reach = board.rules.minimum_clearance_nm.max(pad.clearance_nm);
        if let Some(rule) = spacing.rules.get(&pad.net)
            && spacing.regions.declared.contains(&pad.net)
        {
            reach = reach.max(rule.clearance_nm.unwrap_or(0)).max(rule.pair_gap_nm.unwrap_or(0));
        }
        reach
    };
    for (track_index, track) in board.tracks.iter().enumerate() {
        let copper = capsule(track)?;
        for (pad, land) in pads.iter().zip(&pad_lands) {
            if track.net == pad.net || !pad.layers.contains(&track.layer) {
                continue;
            }
            let (relaxed, _) =
                spacing.between(&track.net, track_lands[track_index].as_deref(), &pad.net, land.as_deref());
            let clearance = pad.clearance_nm.max(relaxed);
            let required = copper.radius_nm + pad.shape.radius_nm + clearance;
            if !shapes_clear(&copper, &pad.shape, clearance) {
                let distance = shape_distance_squared(&copper, &pad.shape);
                findings.push(spacing_finding(
                    &distance,
                    format!(
                        "track {track_index} and {} violate {}",
                        pad.identity,
                        spacing.subject(&track.net, &pad.net)
                    ),
                    [format!("track:{track_index}"), pad.identity.clone()],
                    sorted_pair(&track.net, &pad.net),
                    vec![track.layer.value().to_owned()],
                    required,
                ));
            }
        }
    }
    for (via_index, via) in board.vias.iter().enumerate() {
        let copper = disk(via.position, via.size_nm)?;
        for (pad, land) in pads.iter().zip(&pad_lands) {
            let mut covered: Vec<&str> = Vec::new();
            for layer in &pad.layers {
                if via_covers_layer(board, via, *layer)? {
                    covered.push(layer.value());
                }
            }
            if covered.is_empty() {
                continue;
            }
            if via.net == pad.net {
                if via.finish != "filled-capped" && !shapes_clear(&copper, &pad.shape, 1) {
                    findings.push(
                        Finding::error(
                            "DRC-VIA-PAD-OVERLAP",
                            format!(
                                "ordinary via {via_index} overlaps {}; via-in-pad requires explicit qualification",
                                pad.identity
                            ),
                        )
                        .objects([format!("via:{via_index}"), pad.identity.clone()])
                        .nets([via.net.as_str()])
                        .layers(covered),
                    );
                }
                continue;
            }
            let (relaxed, _) = spacing.between(&via.net, via_lands[via_index].as_deref(), &pad.net, land.as_deref());
            let clearance = pad.clearance_nm.max(relaxed);
            let required = copper.radius_nm + pad.shape.radius_nm + clearance;
            if !shapes_clear(&copper, &pad.shape, clearance) {
                let distance = shape_distance_squared(&copper, &pad.shape);
                findings.push(spacing_finding(
                    &distance,
                    format!("via {via_index} and {} violate {}", pad.identity, spacing.subject(&via.net, &pad.net)),
                    [format!("via:{via_index}"), pad.identity.clone()],
                    sorted_pair(&via.net, &pad.net),
                    Vec::new(),
                    required,
                ));
            }
        }
    }
    let index = copper_physical::geometry::SpatialIndex::new(
        pads.iter().map(|p| (p.identity.clone(), p.shape.bounds.expanded(pad_reach(p)))),
    );
    let by_id: IndexMap<&str, usize> = pads.iter().enumerate().map(|(i, p)| (p.identity.as_str(), i)).collect();
    let mut visited: IndexSet<(String, String)> = IndexSet::new();
    for (left_position, left) in pads.iter().enumerate() {
        for right_id in index.query(left.shape.bounds.expanded(pad_reach(left))) {
            if right_id == left.identity {
                continue;
            }
            let pair = if left.identity.as_str() <= right_id {
                (left.identity.clone(), right_id.to_owned())
            } else {
                (right_id.to_owned(), left.identity.clone())
            };
            if !visited.insert(pair) {
                continue;
            }
            let right_position = by_id[right_id];
            let right = &pads[right_position];
            let shared: Vec<CopperLayer> = left.layers.iter().copied().filter(|l| right.layers.contains(l)).collect();
            if left.net == right.net || shared.is_empty() {
                continue;
            }
            let (relaxed, _) = spacing.between(
                &left.net,
                pad_lands[left_position].as_deref(),
                &right.net,
                pad_lands[right_position].as_deref(),
            );
            let clearance = left.clearance_nm.max(right.clearance_nm).max(relaxed);
            let required = left.shape.radius_nm + right.shape.radius_nm + clearance;
            if !shapes_clear(&left.shape, &right.shape, clearance) {
                let distance = shape_distance_squared(&left.shape, &right.shape);
                let layers: Vec<&str> = shared.iter().map(|l| l.value()).collect();
                findings.push(spacing_finding(
                    &distance,
                    format!(
                        "{} and {} violate {}",
                        left.identity,
                        right.identity,
                        spacing.subject(&left.net, &right.net)
                    ),
                    [left.identity.clone(), right.identity.clone()],
                    sorted_pair(&left.net, &right.net),
                    sorted_unique(&layers),
                    required,
                ));
            }
        }
    }
    Ok(())
}

/// `_check_zone_priorities`: equal-priority zones that intersect or touch on a layer.
fn check_zone_priorities(board: &PhysicalBoard, findings: &mut Vec<Finding>) {
    let mut zones: Vec<&CopperZone> = board.zones.iter().collect();
    zones.sort_by(|a, b| a.id.cmp(&b.id));
    fn rings(zone: &CopperZone) -> Vec<&[Point]> {
        let mut rings: Vec<&[Point]> = vec![&zone.outer];
        rings.extend(zone.holes.iter().map(Vec::as_slice));
        rings
    }
    let inside = |point: Point, zone: &CopperZone| {
        point_in_polygon(point, &zone.outer) && !zone.holes.iter().any(|h| point_in_polygon(point, h))
    };
    let boundaries: Vec<(copper_physical::Bounds, Vec<(Point, Point)>)> = zones
        .iter()
        .map(|zone| {
            let points: Vec<Point> = rings(zone).into_iter().flatten().copied().collect();
            let edges: Vec<(Point, Point)> = rings(zone).into_iter().flat_map(ring_edges).collect();
            (copper_physical::geometry::bounds(&points), edges)
        })
        .collect();
    for (index, first) in zones.iter().enumerate() {
        let (first_bounds, edges_a) = &boundaries[index];
        for (second_index, second) in zones[..index].iter().enumerate() {
            let layers: Vec<CopperLayer> = first.layers.iter().copied().filter(|l| second.layers.contains(l)).collect();
            if first.priority != second.priority || layers.is_empty() {
                continue;
            }
            let (second_bounds, edges_b) = &boundaries[second_index];
            if !first_bounds.intersects(*second_bounds) {
                continue;
            }
            if edges_a.iter().any(|&(a, b)| edges_b.iter().any(|&(c, d)| segments_intersect(a, b, c, d)))
                || first.outer.iter().any(|p| inside(*p, second))
                || second.outer.iter().any(|p| inside(*p, first))
            {
                let layer_names: Vec<&str> = layers.iter().map(|l| l.value()).collect();
                findings.push(
                    Finding::error(
                        "DRC-ZONE-PRIORITY",
                        format!(
                            "zones {} and {} intersect or touch with equal priority {}",
                            first.id, second.id, first.priority
                        ),
                    )
                    .objects(sorted_pair(&first.id, &second.id))
                    .nets(sorted_unique(&[first.net.as_str(), second.net.as_str()]))
                    .layers(sorted_unique(&layer_names)),
                );
            }
        }
    }
}

/// `_check_zone_routing_reservations` with `ZoneRoutingReservations.blocking_zones`.
fn check_zone_routing_reservations(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let reserved: Vec<&CopperZone> = board.zones.iter().filter(|z| z.reserve_routing).collect();
    if reserved.is_empty() {
        return Ok(());
    }
    let clearances: IndexMap<&str, i64> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r.clearance_nm.unwrap_or(0))).collect();
    for (index, track) in board.tracks.iter().enumerate() {
        let copper = capsule(track)?;
        for zone in &reserved {
            if zone.net == track.net || !zone.layers.contains(&track.layer) {
                continue;
            }
            let margin = board
                .rules
                .minimum_clearance_nm
                .max(zone.clearance_nm.unwrap_or(0))
                .max(clearances.get(track.net.as_str()).copied().unwrap_or(0))
                .max(clearances.get(zone.net.as_str()).copied().unwrap_or(0));
            if shapes_clear(&copper, &shape(zone.outer.clone(), 0)?, margin) {
                continue;
            }
            let mut in_hole = false;
            for hole in &zone.holes {
                let contained = ring_edges(&copper.spine).iter().all(|(a, b)| segment_in_polygon(*a, *b, hole));
                if contained {
                    let mut clear = true;
                    for (a, b) in ring_edges(hole) {
                        if !shapes_clear(&copper, &shape(vec![a, b], 0)?, margin) {
                            clear = false;
                            break;
                        }
                    }
                    if clear {
                        in_hole = true;
                        break;
                    }
                }
            }
            if in_hole {
                continue;
            }
            findings.push(
                Finding::error(
                    "DRC-ZONE-RESERVATION",
                    format!("track {index} enters routing reservation {} for {}", zone.id, zone.net),
                )
                .objects([zone.id.clone(), format!("track:{index}")])
                .nets([zone.net.as_str(), track.net.as_str()]),
            );
        }
    }
    Ok(())
}

/// `_check_copper_keepouts`: placed and board copper keepouts against actual copper.
fn check_copper_keepouts(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<bool> {
    let mut local_owner: IndexMap<String, &str> = IndexMap::new();
    for placement in &board.placements {
        for local in &board.footprints[&placement.footprint].keepouts {
            local_owner.insert(format!("{}/{}", placement.reference, local.id), placement.reference.as_str());
        }
    }
    let mut keepouts = resolved_copper_keepouts(board)?;
    keepouts.sort_by(|a, b| a.id.cmp(&b.id));
    for keepout in &keepouts {
        let region = shape(keepout.vertices.clone(), 0)?;
        for (index, track) in board.tracks.iter().enumerate() {
            if !keepout.block_tracks || !keepout.layers.contains(&track.layer) {
                continue;
            }
            if !shapes_clear(&region, &capsule(track)?, 0) {
                findings.push(
                    Finding::error("DRC-COPPER-KEEPOUT", format!("track {index} enters copper keepout {}", keepout.id))
                        .objects([keepout.id.clone(), format!("track:{index}")])
                        .nets([track.net.as_str()])
                        .layers([track.layer.value()]),
                );
            }
        }
        for (index, via) in board.vias.iter().enumerate() {
            if !keepout.block_vias {
                continue;
            }
            let mut covers = false;
            for layer in &keepout.layers {
                covers |= via_covers_layer(board, via, *layer)?;
            }
            if !covers {
                continue;
            }
            if !shapes_clear(&region, &disk(via.position, via.size_nm)?, 0) {
                findings.push(
                    Finding::error("DRC-COPPER-KEEPOUT", format!("via {index} enters copper keepout {}", keepout.id))
                        .objects([keepout.id.clone(), format!("via:{index}")])
                        .nets([via.net.as_str()]),
                );
            }
        }
        if keepout.block_pads {
            for placement in &board.placements {
                if local_owner.get(&keepout.id) == Some(&placement.reference.as_str()) {
                    continue;
                }
                for (pad_index, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
                    if matches!(pad.kind, PadKind::Aperture | PadKind::NonPlatedThroughHole) {
                        continue;
                    }
                    let pad_layers = if pad.kind == PadKind::Smd {
                        vec![if placement.side == BoardSide::Front { CopperLayer::Front } else { CopperLayer::Back }]
                    } else {
                        board.stackup.copper_layers.clone()
                    };
                    if !pad_layers.iter().any(|l| keepout.layers.contains(l)) {
                        continue;
                    }
                    let copper = placed_pad_shape(transformed_local_point(placement, pad.position), pad, placement);
                    if shapes_touch_or_overlap(&region, &copper) {
                        findings.push(
                            Finding::error(
                                "DRC-COPPER-KEEPOUT",
                                format!(
                                    "pad {}.{} enters copper keepout {}",
                                    placement.reference, pad.number, keepout.id
                                ),
                            )
                            .objects([
                                keepout.id.clone(),
                                format!("pad:{}.{}:{pad_index}", placement.reference, pad.number),
                            ]),
                        );
                    }
                }
            }
        }
    }
    // Rewritten keepouts are hole-free, so every keepout is covered.
    Ok(true)
}

/// `non_plated_holes`: placed drill envelopes, including unnumbered mechanical holes.
fn non_plated_holes(board: &PhysicalBoard) -> Vec<(String, RoundedConvexShape)> {
    let mut holes: Vec<(String, RoundedConvexShape)> =
        board.mechanical_holes.iter().map(|h| (format!("mechanical-hole:{}", h.id), hole_shape(h))).collect();
    holes.extend(board.mechanical_slots.iter().map(|s| (format!("mechanical-slot:{}", s.id), slot_shape(s))));
    let mut placements: Vec<&Placement> = board.placements.iter().collect();
    placements.sort_by(|a, b| a.reference.cmp(&b.reference));
    for placement in placements {
        for (index, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
            if pad.kind != PadKind::NonPlatedThroughHole {
                continue;
            }
            let position = transformed_local_point(placement, pad.position);
            holes.push((
                format!("hole:{}.{}:{index}", placement.reference, pad.number),
                drill_shape(position, pad, placement),
            ));
        }
    }
    holes
}

/// `_check_non_plated_hole_clearance`: copper to NPTH; a scoped rule covers its own pads and holes.
fn check_non_plated_hole_clearance(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let clearance = board.rules.minimum_hole_clearance_nm;
    let mut scoped: IndexMap<String, &ComponentHoleClearance> = IndexMap::new();
    for placement in &board.placements {
        let Some(rule) = board.component_hole_clearances.iter().rev().find(|r| r.reference == placement.reference)
        else {
            continue;
        };
        for (index, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
            if pad.kind == PadKind::NonPlatedThroughHole {
                scoped.insert(format!("hole:{}.{}:{index}", placement.reference, pad.number), rule);
            }
        }
    }
    let pads = copper_pads(board);
    for (identity, hole) in non_plated_holes(board) {
        let rule = scoped.get(&identity).copied();
        for pad in &pads {
            if let Some(rule) = rule.filter(|r| r.reference == pad.component) {
                if shapes_clear(&pad.shape, &hole, clearance) {
                    continue;
                }
                let distance = shape_distance_squared(&pad.shape, &hole);
                let four = rational(4) * distance;
                let measured =
                    ((fraction_sqrt_floor(&four) - 2 * (pad.shape.radius_nm + hole.radius_nm)).div_euclid(2)).max(0);
                if !shapes_clear(&pad.shape, &hole, rule.clearance_nm) {
                    findings.push(
                        Finding::error(
                            "DRC-HOLE-CLEARANCE",
                            format!(
                                "{} violates {identity} drill clearance (checked against the scoped hole_clearance of {})",
                                pad.identity, rule.reference
                            ),
                        )
                        .objects([pad.identity.clone(), identity.clone()])
                        .nets([pad.net.as_str()])
                        .required(rule.clearance_nm)
                        .measured(measured),
                    );
                }
            } else if !shapes_clear(&pad.shape, &hole, clearance) {
                findings.push(
                    Finding::error(
                        "DRC-HOLE-CLEARANCE",
                        format!("{} violates {identity} drill clearance", pad.identity),
                    )
                    .objects([pad.identity.clone(), identity.clone()])
                    .nets([pad.net.as_str()]),
                );
            }
        }
        for (index, track) in board.tracks.iter().enumerate() {
            if !shapes_clear(&capsule(track)?, &hole, clearance) {
                findings.push(
                    Finding::error("DRC-HOLE-CLEARANCE", format!("track {index} violates {identity} drill clearance"))
                        .objects([format!("track:{index}"), identity.clone()])
                        .nets([track.net.as_str()])
                        .layers([track.layer.value()]),
                );
            }
        }
        for (index, via) in board.vias.iter().enumerate() {
            if !shapes_clear(&disk(via.position, via.size_nm)?, &hole, clearance) {
                findings.push(
                    Finding::error("DRC-HOLE-CLEARANCE", format!("via {index} violates {identity} drill clearance"))
                        .objects([format!("via:{index}"), identity.clone()])
                        .nets([via.net.as_str()]),
                );
            }
        }
        for polygon in &board.polygons {
            let mut violates = false;
            for triangle in polygon_triangles(&polygon.vertices)? {
                if !shapes_clear(&shape(triangle.to_vec(), 0)?, &hole, clearance) {
                    violates = true;
                    break;
                }
            }
            if violates {
                findings.push(
                    Finding::error(
                        "DRC-HOLE-CLEARANCE",
                        format!("polygon {} violates {identity} drill clearance", polygon.id),
                    )
                    .objects([format!("polygon:{}", polygon.id), identity.clone()])
                    .nets([polygon.net.as_str()])
                    .layers([polygon.layer.value()]),
                );
            }
        }
    }
    Ok(())
}

/// `_check_via_hole_clearance`: plated via drills clear foreign copper, whatever the finish.
fn check_via_hole_clearance(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let pads = copper_pads(board);
    let clearance = board.rules.minimum_hole_clearance_nm;
    for (via_index, via) in board.vias.iter().enumerate() {
        let hole = disk(via.position, via.drill_nm)?;
        for (track_index, track) in board.tracks.iter().enumerate() {
            if track.net == via.net || !via_covers_layer(board, via, track.layer)? {
                continue;
            }
            if !shapes_clear(&hole, &capsule(track)?, clearance) {
                findings.push(
                    Finding::error(
                        "DRC-HOLE-CLEARANCE",
                        format!("via {via_index} drill violates track {track_index} clearance"),
                    )
                    .objects([format!("via:{via_index}"), format!("track:{track_index}")])
                    .nets(sorted_pair(&via.net, &track.net))
                    .required(clearance),
                );
            }
        }
        for (other_index, other) in board.vias.iter().enumerate() {
            if other_index == via_index || other.net == via.net || !via_spans_overlap(board, via, other)? {
                continue;
            }
            if !shapes_clear(&hole, &disk(other.position, other.size_nm)?, clearance) {
                findings.push(
                    Finding::error(
                        "DRC-HOLE-CLEARANCE",
                        format!("via {via_index} drill violates via {other_index} copper clearance"),
                    )
                    .objects([format!("via:{via_index}"), format!("via:{other_index}")])
                    .nets(sorted_pair(&via.net, &other.net))
                    .required(clearance),
                );
            }
        }
        for pad in &pads {
            if pad.net == via.net {
                continue;
            }
            let mut covers = false;
            for layer in &pad.layers {
                covers |= via_covers_layer(board, via, *layer)?;
            }
            if covers && !shapes_clear(&hole, &pad.shape, clearance) {
                findings.push(
                    Finding::error(
                        "DRC-HOLE-CLEARANCE",
                        format!("via {via_index} drill violates {} clearance", pad.identity),
                    )
                    .objects([format!("via:{via_index}"), pad.identity.clone()])
                    .nets(sorted_pair(&via.net, &pad.net))
                    .required(clearance),
                );
            }
        }
    }
    Ok(())
}

/// `_check_drill_spacing`: via-to-via and via-to-plated-pad drills, independent of net.
fn check_drill_spacing(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let clearance = board.rules.minimum_hole_clearance_nm;
    let npth = non_plated_holes(board);
    let mut other_drills: Vec<(String, RoundedConvexShape)> = Vec::new();
    for (i, via) in board.vias.iter().enumerate() {
        other_drills.push((format!("via:{i}"), shape(vec![via.position], (via.drill_nm + 1) / 2)?));
    }
    let mut plated: Vec<(String, RoundedConvexShape, String)> = Vec::new();
    for placement in &board.placements {
        for (i, pad) in board.footprints[&placement.footprint].pads.iter().enumerate() {
            if pad.kind == PadKind::ThroughHole && pad.drill.is_some() {
                let drill = drill_shape(transformed_local_point(placement, pad.position), pad, placement);
                other_drills.push((format!("drill:{}:{i}", placement.reference), drill.clone()));
                plated.push((
                    format!("{}.{}", placement.reference, pad.number),
                    drill,
                    format!("pad:{}.{}:{i}", placement.reference, pad.number),
                ));
            }
        }
    }
    for (i, (identity, hole)) in npth.iter().enumerate() {
        for (other_id, other) in npth[..i].iter().chain(other_drills.iter()) {
            if !shapes_clear(hole, other, clearance) {
                findings.push(
                    Finding::error("DRC-DRILL-SPACING", format!("{identity} and {other_id} violate hole spacing"))
                        .objects([identity.clone(), other_id.clone()])
                        .required(clearance),
                );
            }
        }
    }
    for (first_index, first) in board.vias.iter().enumerate() {
        let first_hole = disk(first.position, first.drill_nm)?;
        for (second_index, second) in board.vias.iter().enumerate().skip(first_index + 1) {
            if !shapes_clear(&first_hole, &disk(second.position, second.drill_nm)?, clearance) {
                findings.push(
                    Finding::error(
                        "DRC-DRILL-SPACING",
                        format!("vias {first_index} and {second_index} violate hole spacing"),
                    )
                    .objects([format!("via:{first_index}"), format!("via:{second_index}")])
                    .nets(sorted_unique(&[first.net.as_str(), second.net.as_str()]))
                    .required(clearance),
                );
            }
        }
        for (name, other_hole, identity) in &plated {
            if !shapes_clear(&first_hole, other_hole, clearance) {
                findings.push(
                    Finding::error("DRC-DRILL-SPACING", format!("via {first_index} violates {name} hole spacing"))
                        .objects([format!("via:{first_index}"), identity.clone()])
                        .nets([first.net.as_str()])
                        .required(clearance),
                );
            }
        }
    }
    Ok(())
}

/// `_shape_clear_of_region`: a shape outside a polygon (with holes) by a clearance.
fn shape_clear_of_region(
    copper: &RoundedConvexShape,
    outer: &[Point],
    holes: &[Vec<Point>],
    clearance: i64,
) -> Result<bool> {
    let in_material =
        |point: Point| point_in_polygon(point, outer) && !holes.iter().any(|h| point_in_polygon(point, h));
    if copper.spine.iter().any(|p| in_material(*p)) {
        return Ok(false);
    }
    for point in outer {
        if !shapes_clear(copper, &shape(vec![*point], 0)?, 1) {
            return Ok(false);
        }
    }
    let required = i128::from(copper.radius_nm + clearance);
    let limit = rational(required * required);
    let mut boundaries: Vec<&[Point]> = vec![outer];
    boundaries.extend(holes.iter().map(Vec::as_slice));
    for boundary in boundaries {
        let edges = ring_edges(boundary);
        for (a, b) in spine_edges(&copper.spine) {
            if edges.iter().any(|(c, d)| segment_distance_squared(a, b, *c, *d) < limit) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// `_check_copper_artwork`: immutable copper artwork reservations.
fn check_copper_artwork(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    let holes = non_plated_holes(board);
    let reservations: IndexMap<&str, &copper_physical::CopperKeepout> =
        board.copper_keepouts.iter().map(|k| (k.id.as_str(), k)).collect();
    for item in &board.silkscreen {
        let Some(layer) = artwork_layer(item) else { continue };
        let (left, low, right, high) = artwork_bounds(item)?;
        let envelope = shape(
            vec![Point::new(left, low), Point::new(right, low), Point::new(right, high), Point::new(left, high)],
            0,
        )?;
        let id = artwork_id(item);
        let identity = format!("artwork:{id}");
        let width = artwork_width(item);
        if 0 < width && width < board.rules.minimum_track_width_nm && !artwork_filled(item) {
            findings.push(
                Finding::error(
                    "DRC-ARTWORK-WIDTH",
                    format!("copper artwork {id} stroke is below the minimum copper width"),
                )
                .objects([identity.as_str()])
                .layers([layer.value()])
                .required(board.rules.minimum_track_width_nm)
                .measured(width),
            );
        }
        if !shape_in_outline(&envelope, &board.outline, board.rules.minimum_clearance_nm)? {
            findings.push(
                Finding::error(
                    "DRC-ARTWORK-BOARD-EDGE",
                    format!("copper artwork {id} envelope violates board material/edge clearance"),
                )
                .objects([identity.as_str()])
                .layers([layer.value()]),
            );
        }
        for (name, hole) in &holes {
            if !shapes_clear(&envelope, hole, board.rules.minimum_hole_clearance_nm) {
                findings.push(
                    Finding::error("DRC-ARTWORK-HOLE", format!("copper artwork {id} envelope violates hole clearance"))
                        .objects([identity.clone(), name.clone()])
                        .layers([layer.value()]),
                );
            }
        }
        let key = format!("__copper_artwork:{id}");
        let reservation =
            reservations.get(key.as_str()).ok_or_else(|| Error::Value(copper_json::python_str_repr(&key)))?;
        let reserved = shape(reservation.vertices.clone(), 0)?;
        for polygon in board.polygons.iter().filter(|p| p.layer == layer) {
            if !shape_clear_of_region(&reserved, &polygon.vertices, &[], 0)? {
                findings.push(
                    Finding::error(
                        "DRC-ARTWORK-CLEARANCE",
                        format!("fixed copper polygon {} enters artwork reservation {id}", polygon.id),
                    )
                    .objects([identity.clone(), format!("polygon:{}", polygon.id)])
                    .layers([layer.value()]),
                );
            }
        }
    }
    Ok(())
}

/// `_check_fixed_polygons`: owner copper polygons against the edge and foreign copper.
fn check_fixed_polygons(board: &PhysicalBoard, findings: &mut Vec<Finding>) -> Result<()> {
    if board.polygons.is_empty() {
        return Ok(());
    }
    let pads = copper_pads(board);
    let clearance = board.rules.minimum_clearance_nm;
    let net_clearances: IndexMap<&str, i64> =
        board.net_routing_rules.iter().map(|r| (r.net.as_str(), r.clearance_nm.unwrap_or(0))).collect();
    let between = |first: &str, second: &str| {
        clearance
            .max(net_clearances.get(first).copied().unwrap_or(0))
            .max(net_clearances.get(second).copied().unwrap_or(0))
    };
    for polygon in &board.polygons {
        let triangles: Vec<RoundedConvexShape> =
            polygon_triangles(&polygon.vertices)?.into_iter().map(|t| shape(t.to_vec(), 0)).collect::<Result<_>>()?;
        let mut outside = false;
        for triangle in &triangles {
            if !shape_in_outline(triangle, &board.outline, clearance)? {
                outside = true;
                break;
            }
        }
        let object = format!("polygon:{}", polygon.id);
        if outside {
            findings.push(
                Finding::error(
                    "DRC-BOARD-EDGE",
                    format!("polygon {} violates copper-to-board-edge clearance", polygon.id),
                )
                .objects([object.as_str()])
                .nets([polygon.net.as_str()])
                .layers([polygon.layer.value()]),
            );
        }
        for (index, track) in board.tracks.iter().enumerate() {
            if track.net != polygon.net
                && track.layer == polygon.layer
                && !shape_clear_of_region(&capsule(track)?, &polygon.vertices, &[], between(&polygon.net, &track.net))?
            {
                findings.push(
                    Finding::error("DRC-CLEARANCE", format!("polygon {} violates track {index} clearance", polygon.id))
                        .objects([object.clone(), format!("track:{index}")]),
                );
            }
        }
        for (index, via) in board.vias.iter().enumerate() {
            if via.net != polygon.net
                && via_covers_layer(board, via, polygon.layer)?
                && !shape_clear_of_region(
                    &disk(via.position, via.size_nm)?,
                    &polygon.vertices,
                    &[],
                    between(&polygon.net, &via.net),
                )?
            {
                findings.push(
                    Finding::error("DRC-CLEARANCE", format!("polygon {} violates via {index} clearance", polygon.id))
                        .objects([object.clone(), format!("via:{index}")]),
                );
            }
        }
        for pad in &pads {
            if pad.net != polygon.net
                && pad.layers.contains(&polygon.layer)
                && !shape_clear_of_region(
                    &pad.shape,
                    &polygon.vertices,
                    &[],
                    between(&polygon.net, &pad.net).max(pad.clearance_nm),
                )?
            {
                findings.push(
                    Finding::error(
                        "DRC-CLEARANCE",
                        format!("polygon {} violates {} clearance", polygon.id, pad.identity),
                    )
                    .objects([object.clone(), pad.identity.clone()]),
                );
            }
        }
        for other in &board.polygons {
            if other.id >= polygon.id || other.net == polygon.net || other.layer != polygon.layer {
                continue;
            }
            let mut violates = false;
            for triangle in &triangles {
                if !shape_clear_of_region(triangle, &other.vertices, &[], between(&polygon.net, &other.net))? {
                    violates = true;
                    break;
                }
            }
            if violates {
                findings.push(
                    Finding::error(
                        "DRC-CLEARANCE",
                        format!("polygons {} and {} violate clearance", polygon.id, other.id),
                    )
                    .objects([object.clone(), format!("polygon:{}", other.id)]),
                );
            }
        }
    }
    Ok(())
}

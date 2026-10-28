//! Small replayable routing corpus with explicit expected failures and quality
//! bounds (`pcbir.routing_acceptance`, the `benchmark-routing` command).
//!
//! Authored test lands are finite geometry, not reviewed manufacturer
//! components. All algorithmic work runs through the shared routing pipeline.

use std::path::Path;

use copper_drc::connectivity::{Candidate, explicit_copper_connectivity};
use copper_drc::digest::physical_board_digest;
use copper_drc::Severity;
use copper_json::{Dumps, Object, Value};
use copper_lang::paths;
use copper_pcb::board::{
    DecouplingLink, NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, ReturnViaPolicy, Stackup,
    TuningStyle,
};
use copper_pcb::{Result, kicad_pcb, project};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_pcb::board::RouteKind;
use copper_physical::{BoardOutline, CopperKeepout, CopperLayer, DesignRules, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

use crate::drc_report;
use crate::routing_metrics::routing_quality;

pub const SCHEMA: &str = "copperscript-routing-acceptance/v0.1";

/// One acceptance case.
#[derive(Clone, Debug)]
pub struct AcceptanceCase {
    pub id: String,
    pub board: PhysicalBoard,
    pub expect_connected: bool,
    pub maximum_vias: Option<i64>,
    pub maximum_length_nm: Option<i64>,
}

fn mm(value: i64) -> i64 {
    value * 1_000_000
}

fn pad(number: &str, position: Point, size_nm: i64) -> FootprintPad {
    FootprintPad {
        number: number.to_owned(),
        position,
        size: Size { width_nm: size_nm, height_nm: size_nm },
        kind: PadKind::Smd,
        shape: PadShape::Roundrect,
        rotation_degrees: Decimal::zero(),
        drill: None,
        roundrect_ratio_ppm: 250_000,
        has_solder_mask: true,
        has_solder_paste: true,
        zone_connection: None,
        heatsink: false,
        remove_unused_layers: false,
        connector_contact: false,
    }
}

fn footprint(name: &str, pads: Vec<FootprintPad>, body: Size) -> PhysicalFootprint {
    PhysicalFootprint {
        name: name.to_owned(),
        pads,
        body_size: body,
        source_library_id: None,
        graphics: Vec::new(),
        keepouts: Vec::new(),
        metadata: IndexMap::new(),
        courtyard: Vec::new(),
        height_nm: None,
        clearance_nm: None,
        exclude_from_bom: false,
        exclude_from_pos_files: false,
        internal_pad_groups: Vec::new(),
    }
}

/// `NetRoutingRule(net, **overrides)` with the dataclass defaults.
pub fn general_rule(net: &str) -> NetRoutingRule {
    NetRoutingRule {
        net: net.to_owned(),
        kind: RouteKind::General,
        priority: 0,
        width_nm: None,
        clearance_nm: None,
        allowed_layers: Vec::new(),
        max_vias: None,
        max_length_nm: None,
        differential_partner: None,
        pair_gap_nm: None,
        max_skew_nm: None,
        topology: "point_to_point".to_owned(),
        target_impedance_ohms: None,
        maximum_uncoupled_length_nm: None,
        maximum_stub_length_nm: None,
        tuning_amplitude_limit_nm: None,
        require_return_vias: false,
        reserve_corridor: false,
        return_via_net: None,
        maximum_return_via_distance_nm: None,
        impedance_evidence_digest: None,
        return_via_policy: ReturnViaPolicy::Always,
        shared_reference_layer: None,
        target_single_ended_ohms: None,
        impedance_tolerance_percent: None,
        layer_group: None,
        breakout_length_nm: None,
        breakout_width_nm: None,
        breakout_gap_nm: None,
        breakout_clearance_nm: None,
        tuning_style: TuningStyle::Bumps,
        tuning_spacing_nm: None,
        tuning_group: None,
    }
}

/// `CopperKeepout(id, layers, box)` with the dataclass defaults.
pub fn keepout(id: &str, layers: Vec<CopperLayer>, vertices: Vec<Point>) -> CopperKeepout {
    CopperKeepout {
        id: id.to_owned(),
        layers,
        vertices,
        block_tracks: true,
        block_vias: true,
        block_pads: false,
        block_zones: true,
        block_footprints: false,
    }
}

/// `PhysicalBoard(name, outline, footprints, placements, nets)` with every other field defaulted.
pub fn board(
    name: &str,
    outline: BoardOutline,
    footprints: IndexMap<String, PhysicalFootprint>,
    placements: Vec<Placement>,
    nets: Vec<PhysicalNet>,
) -> PhysicalBoard {
    PhysicalBoard {
        name: name.to_owned(),
        outline,
        footprints,
        placements,
        nets,
        stackup: Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
        rules: DesignRules::default(),
        tracks: Vec::new(),
        vias: Vec::new(),
        metadata: IndexMap::new(),
        regions: Vec::new(),
        keepouts: Vec::new(),
        placement_rules: Vec::new(),
        relative_rules: Vec::new(),
        placement_groups: Vec::new(),
        power_domains: Vec::new(),
        net_routing_rules: Vec::new(),
        zones: Vec::new(),
        polygons: Vec::new(),
        copper_keepouts: Vec::new(),
        rigid_clusters: Vec::new(),
        hard_macros: Vec::new(),
        materialized_macros: Vec::new(),
        via_in_pad_rules: Vec::new(),
        mechanical_holes: Vec::new(),
        mechanical_slots: Vec::new(),
        datums: Vec::new(),
        boundary_edges: Vec::new(),
        attachments: Vec::new(),
        component_heights: Vec::new(),
        body_overhangs: Vec::new(),
        assembly_envelopes: Vec::new(),
        assembly_access: Vec::new(),
        match_groups: Vec::new(),
        component_hole_clearances: Vec::new(),
        decoupling_links: Vec::new(),
        silkscreen: Vec::new(),
        power_feeds: Vec::new(),
        mechanical_references: Vec::new(),
    }
}

fn pose(reference: &str, x_mm: i64, y_mm: i64, footprint: &str) -> Placement {
    Placement::new(reference, footprint, Point::new(mm(x_mm), mm(y_mm)), &Decimal::zero()).expect("a valid pose")
}

fn net(name: &str, refs: &[&str]) -> PhysicalNet {
    PhysicalNet { name: name.to_owned(), pads: refs.iter().map(|r| PadReference::new(r, "1")).collect() }
}

fn rectangle(x0: i64, y0: i64, x1: i64, y1: i64) -> Vec<Point> {
    vec![
        Point::new(mm(x0), mm(y0)),
        Point::new(mm(x1), mm(y0)),
        Point::new(mm(x1), mm(y1)),
        Point::new(mm(x0), mm(y1)),
    ]
}

/// `acceptance_cases()`.
pub fn acceptance_cases() -> Result<Vec<AcceptanceCase>> {
    let terminal = footprint(
        "acceptance/terminal",
        vec![pad("1", Point::new(0, 0), 600_000)],
        Size { width_nm: mm(1), height_nm: mm(1) },
    );
    // Keep the authored body/silk envelope clear of both mask openings.
    let cap = footprint(
        "acceptance/bypass",
        vec![pad("1", Point::new(0, 0), 600_000), pad("2", Point::new(0, 1_200_000), 600_000)],
        Size { width_nm: mm(1), height_nm: mm(4) },
    );
    let outline = BoardOutline::rectangle(mm(20), mm(14), None).map_err(copper_pcb::Error::Value)?;
    let scope = "authored finite test lands; not qualified manufacturer footprints";
    let common_footprints: IndexMap<String, PhysicalFootprint> =
        IndexMap::from([(terminal.name.clone(), terminal.clone())]);
    let with_scope = |mut board: PhysicalBoard| {
        board.metadata.insert("geometry_scope".to_owned(), scope.to_owned());
        board
    };
    let mut detour = with_scope(board(
        "AcceptanceDetour",
        outline.clone(),
        common_footprints.clone(),
        vec![pose("A", 4, 7, &terminal.name), pose("B", 16, 7, &terminal.name)],
        vec![net("SIGNAL", &["A", "B"])],
    ));
    detour.net_routing_rules = vec![NetRoutingRule {
        allowed_layers: vec![CopperLayer::Front],
        max_vias: Some(0),
        ..general_rule("SIGNAL")
    }];
    detour.copper_keepouts = vec![keepout("obstacle", vec![CopperLayer::Front], rectangle(9, 5, 11, 9))];
    let mut crossing = with_scope(board(
        "AcceptanceCrossing",
        outline.clone(),
        common_footprints.clone(),
        vec![
            pose("A", 4, 7, &terminal.name),
            pose("B", 16, 7, &terminal.name),
            pose("C", 10, 3, &terminal.name),
            pose("D", 10, 11, &terminal.name),
        ],
        vec![net("H", &["A", "B"]), net("V", &["C", "D"])],
    ));
    crossing.stackup = Stackup::new(vec![CopperLayer::Front, CopperLayer::In1, CopperLayer::In2, CopperLayer::Back]);
    let mut bypass = with_scope(board(
        "AcceptanceBypass",
        outline.clone(),
        IndexMap::from([(terminal.name.clone(), terminal.clone()), (cap.name.clone(), cap.clone())]),
        vec![pose("U", 5, 7, &terminal.name), pose("C", 8, 7, &cap.name), pose("J", 16, 7, &terminal.name)],
        vec![
            net("POWER", &["U", "C", "J"]),
            PhysicalNet { name: "RETURN".to_owned(), pads: vec![PadReference::new("C", "2")] },
        ],
    ));
    bypass.decoupling_links = vec![DecouplingLink {
        capacitor: PadReference::new("C", "1"),
        target: PadReference::new("U", "1"),
        return_pad: PadReference::new("C", "2"),
        net: "POWER".to_owned(),
        return_net: "RETURN".to_owned(),
    }];
    let mut blocked = detour.clone();
    blocked.name = "AcceptanceBlocked".to_owned();
    blocked.copper_keepouts =
        vec![keepout("blocked-terminal", vec![CopperLayer::Front, CopperLayer::Back], rectangle(3, 6, 5, 8))];
    let bases: [(&str, PhysicalBoard, bool, Option<i64>, Option<i64>); 4] = [
        ("two-layer-detour", detour, true, Some(0), Some(25_000_000)),
        ("four-layer-crossing", crossing, true, Some(8), Some(40_000_000)),
        ("two-layer-branched-bypass", bypass, true, Some(4), Some(30_000_000)),
        ("two-layer-blocked-access", blocked, false, None, None),
    ];
    let mut cases = Vec::new();
    for (name, board, connected, max_vias, max_length) in bases {
        for rotated in [false, true] {
            let candidate = if rotated { renamed_rotated(&board)? } else { board.clone() };
            cases.push(AcceptanceCase {
                id: format!("{name}{}", if rotated { "-renamed-rotated" } else { "" }),
                board: candidate,
                expect_connected: connected,
                maximum_vias: max_vias,
                maximum_length_nm: max_length,
            });
        }
    }
    Ok(cases)
}

/// `_renamed_rotated(board)`: the same board rotated a quarter turn with `ALT_` names.
fn renamed_rotated(board: &PhysicalBoard) -> Result<PhysicalBoard> {
    let height = board.outline.vertices.iter().map(|p| p.y_nm).max().unwrap_or(0);
    let point = |p: Point| Point::new(height - p.y_nm, p.x_nm);
    let rename = |name: &str| format!("ALT_{name}");
    let pad = |p: &PadReference| PadReference::new(&rename(&p.component), &p.pad);
    let mut rotated = board.clone();
    rotated.name = format!("{}RenamedRotated", board.name);
    rotated.outline = BoardOutline::rectangle(mm(14), mm(20), None).map_err(copper_pcb::Error::Value)?;
    rotated.placements = board
        .placements
        .iter()
        .map(|p| {
            let mut pose = p.clone();
            pose.reference = rename(&p.reference);
            pose.position = point(p.position);
            pose.rotation_degrees = Decimal::from_i64(270);
            pose
        })
        .collect();
    rotated.nets = board
        .nets
        .iter()
        .map(|n| PhysicalNet { name: rename(&n.name), pads: n.pads.iter().map(pad).collect() })
        .collect();
    rotated.net_routing_rules = board
        .net_routing_rules
        .iter()
        .map(|r| NetRoutingRule { net: rename(&r.net), ..r.clone() })
        .collect();
    rotated.copper_keepouts = board
        .copper_keepouts
        .iter()
        .map(|k| CopperKeepout { vertices: k.vertices.iter().map(|v| point(*v)).collect(), ..k.clone() })
        .collect();
    rotated.decoupling_links = board
        .decoupling_links
        .iter()
        .map(|d| DecouplingLink {
            capacitor: pad(&d.capacitor),
            target: pad(&d.target),
            return_pad: pad(&d.return_pad),
            net: rename(&d.net),
            return_net: rename(&d.return_net),
        })
        .collect();
    Ok(rotated)
}

/// What `route_case` needs of the pipeline result: `assess_case` reads these.
pub struct RoutedCase {
    pub board: PhysicalBoard,
    pub status_pass: bool,
    pub drc: drc_report::DrcReport,
}

/// `assess_case(case, result, quality)`: expectations distinguish a truthful
/// failure from a successful board.
pub fn assess_case(case: &AcceptanceCase, result: &RoutedCase, quality: &Value) -> Result<Object> {
    let graph = explicit_copper_connectivity(&result.board, Candidate::default())?;
    let connected = result.board.nets.iter().all(|n| graph.net_connected(n));
    let hard: Vec<Value> = result
        .drc
        .findings()
        .iter()
        .filter(|f| f.severity == Severity::Error && f.code != "DRC-OPEN-NET" && f.code != "DRC-ROUTE-INCOMPLETE")
        .map(|f| Value::from(f.code.as_str()))
        .collect();
    let poses = |board: &PhysicalBoard| -> IndexMap<String, Placement> {
        board.placements.iter().map(|p| (p.reference.clone(), p.clone())).collect()
    };
    let mut checks = Object::new();
    checks.insert("expected_connectivity".to_owned(), Value::from(connected == case.expect_connected));
    checks.insert("no_new_hard_findings".to_owned(), Value::from(hard.is_empty()));
    checks.insert("fixed_poses_preserved".to_owned(), Value::from(poses(&case.board) == poses(&result.board)));
    checks.insert(
        "source_rules_preserved".to_owned(),
        Value::from(case.board.net_routing_rules == result.board.net_routing_rules),
    );
    checks.insert(
        "source_keepouts_preserved".to_owned(),
        Value::from(case.board.copper_keepouts == result.board.copper_keepouts),
    );
    checks.insert("truthful_pipeline_status".to_owned(), Value::from(result.status_pass == connected));
    if case.expect_connected {
        let count = |key: &str| quality.get(key).and_then(|v| match v {
            Value::Int(n) => n.to_string().parse::<i64>().ok(),
            _ => None,
        });
        checks.insert("via_bound".to_owned(), Value::from(count("via_count") <= case.maximum_vias));
        checks.insert("length_bound".to_owned(), Value::from(count("track_length_nm") <= case.maximum_length_nm));
        let surface = quality
            .get("decoupling")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().all(|d| d.get("surface_connected") == Some(&Value::Bool(true))));
        checks.insert("surface_bypasses".to_owned(), Value::from(surface));
    }
    let passed = checks.values().all(|v| *v == Value::Bool(true));
    let mut row = Object::new();
    row.insert("connected".to_owned(), Value::from(connected));
    row.insert("checks".to_owned(), Value::Object(checks));
    row.insert("hard_findings".to_owned(), Value::Array(hard));
    row.insert("expectations_passed".to_owned(), Value::from(passed));
    Ok(row)
}

/// How `run_acceptance` fails: an `OSError`/`ValueError`/`RuntimeError` message, or pending work.
pub enum AcceptanceError {
    Message(String),
    NotImplemented(String),
}

impl From<copper_pcb::Error> for AcceptanceError {
    fn from(error: copper_pcb::Error) -> Self {
        match error {
            copper_pcb::Error::NotImplemented(what) => AcceptanceError::NotImplemented(what),
            copper_pcb::Error::Value(message) => AcceptanceError::Message(message),
        }
    }
}

/// `run_acceptance(output, kicad_cli=...)`: the report, written to `OUTPUT/acceptance.json`.
///
/// `route` runs the shared routing pipeline on one case (`route_case`); while
/// the routers it needs are not rewritten it reports not implemented and the
/// command exits 3 after creating the output directory, as the harness expects
/// of pending work.
pub fn run_acceptance(
    output: &Path,
    kicad_cli: Option<&Path>,
    route: &mut dyn FnMut(&PhysicalBoard) -> Result<RoutedCase>,
) -> std::result::Result<Value, AcceptanceError> {
    let output = paths::resolve(output);
    let shown = paths::display(&output);
    // `mkdir(parents=True, exist_ok=False)`: the parents may exist, the directory itself may not.
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AcceptanceError::Message(paths::python_os_call_error(&e, Some(&paths::display(parent)))))?;
    }
    std::fs::create_dir(&output).map_err(|e| AcceptanceError::Message(paths::python_os_call_error(&e, Some(&shown))))?;
    let mut report = Object::new();
    report.insert("schema".to_owned(), Value::from(SCHEMA));
    report.insert("status".to_owned(), Value::from("incomplete"));
    report.insert(
        "scope".to_owned(),
        Value::from("bounded software acceptance; authored geometry; not an LLM success-rate or hardware qualification"),
    );
    report.insert("python".to_owned(), Value::from(crate::not_python_version()));
    report.insert("platform".to_owned(), Value::from(crate::platform_text()));
    let mut settings = Object::new();
    for (key, value) in [
        ("pitch_nm", 500_000i64),
        ("area_passes", 2),
        ("search_states", 10_000),
        ("placement_candidates", 1),
        ("placement_feedback_iterations", 1),
        ("package_trials", 0),
    ] {
        settings.insert(key.to_owned(), Value::from(value));
    }
    report.insert("settings".to_owned(), Value::Object(settings));
    report.insert("cases".to_owned(), Value::Array(Vec::new()));
    report.insert("fabrication_ready".to_owned(), Value::from(false));
    report.insert("implementation_digest".to_owned(), Value::from(crate::implementation_digest()));
    let write = |report: &Object| -> std::result::Result<(), AcceptanceError> {
        let path = output.join("acceptance.json");
        let text = Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(report.clone())) + "\n";
        copper_json::text::write_text(&path, &text)
            .map_err(|e| AcceptanceError::Message(paths::python_os_error(&e, Some(&paths::display(&path)))))
    };
    for case in acceptance_cases()? {
        let directory = output.join(&case.id);
        std::fs::create_dir(&directory)
            .map_err(|e| AcceptanceError::Message(paths::python_os_call_error(&e, Some(&paths::display(&directory)))))?;
        let result = route(&case.board)?;
        let quality = routing_quality(&result.board, None)?;
        let mut row = Object::new();
        row.insert("id".to_owned(), Value::from(case.id.as_str()));
        row.insert("layers".to_owned(), Value::from(result.board.stackup.copper_layers.len() as i64));
        row.insert("input_digest".to_owned(), Value::from(physical_board_digest(&case.board)));
        row.insert("output_digest".to_owned(), Value::from(physical_board_digest(&result.board)));
        row.insert("pipeline_status".to_owned(), Value::from(if result.status_pass { "pass" } else { "fail" }));
        row.insert("expect_connected".to_owned(), Value::from(case.expect_connected));
        row.insert("maximum_vias".to_owned(), case.maximum_vias.map_or(Value::Null, Value::from));
        row.insert("maximum_length_nm".to_owned(), case.maximum_length_nm.map_or(Value::Null, Value::from));
        for (key, value) in assess_case(&case, &result, &quality)? {
            row.insert(key, value);
        }
        row.insert("quality".to_owned(), quality);
        row.insert("native_status".to_owned(), Value::from("incomplete"));
        let manifest = kicad_pcb::generate(&result.board)?;
        project::write_kicad_project(&manifest, &directory.join("candidate.kicad_pcb"))
            .map_err(AcceptanceError::Message)?;
        let drc_path = directory.join("physical-drc.json");
        copper_json::text::write_text(&drc_path, &result.drc.to_json())
            .map_err(|e| AcceptanceError::Message(paths::python_os_error(&e, Some(&paths::display(&drc_path)))))?;
        if kicad_cli.is_some() {
            return Err(AcceptanceError::NotImplemented("native verification of acceptance cases".to_owned()));
        }
        if let Some(Value::Array(cases)) = report.get_mut("cases") {
            cases.push(Value::Object(row));
        }
        // Retain completed cases if a later process is interrupted.
        write(&report)?;
    }
    let cases: Vec<Value> = report.get("cases").and_then(Value::as_array).map(<[Value]>::to_vec).unwrap_or_default();
    let software = cases.iter().all(|r| r.get("expectations_passed") == Some(&Value::Bool(true)));
    report.insert("software_expectations_passed".to_owned(), Value::from(software));
    report.insert("implementation_digest_after".to_owned(), Value::from(crate::implementation_digest()));
    report.insert("implementation_stable".to_owned(), Value::from(true));
    let native = |status: &str| cases.iter().any(|r| r.get("native_status").and_then(Value::as_str) == Some(status));
    let all_native_pass = cases.iter().all(|r| r.get("native_status").and_then(Value::as_str) == Some("pass"));
    let status = if !software || native("fail") {
        "fail"
    } else if all_native_pass {
        "pass"
    } else {
        "incomplete"
    };
    report.insert("status".to_owned(), Value::from(status));
    write(&report)?;
    Ok(Value::Object(report))
}

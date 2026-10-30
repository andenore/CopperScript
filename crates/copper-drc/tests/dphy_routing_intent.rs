//! `tests/test_dphy_routing_intent.py` (d39067a): routing-intent language for
//! D-PHY routing (plan L4 length matching, L5 layer groups, L6 breakout
//! properties) and `DRC-LENGTH-MATCH` verification.

use copper_drc::{CoverageStatus, Policy, Severity, run_physical_drc, signal_integrity::verify_match_groups};
use copper_lang::compiler::{Design, compile_design_source};
use copper_pcb::board::{
    NetMatchGroup, NetRoutingRule, PadReference, PhysicalBoard, PhysicalNet, Placement, RouteKind, TrackSegment,
};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize, revalidate};
use copper_physical::footprint::{FootprintPad, PadKind, PadShape, PhysicalFootprint, Size};
use copper_physical::{BoardOutline, CopperLayer, Point};
use copper_units::Decimal;
use indexmap::IndexMap;

const MM: i64 = 1_000_000;

fn nm(text: &str) -> i64 {
    copper_pcb::nm_from_mm_text(text).expect("millimetres")
}

fn source(constraints: &str) -> String {
    format!(
        "board Lanes {{
        use library \"tiny\";
        component R1: RESISTOR {{ footprint = \"0402\"; }}
        component R2: RESISTOR {{ footprint = \"0402\"; }}
        component R3: RESISTOR {{ footprint = \"0402\"; }}
        component R4: RESISTOR {{ footprint = \"0402\"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        net D0P {{ R3.1; R4.1; }}
        net D0N {{ R3.2; R4.2; }}
        {constraints}
    }}"
    )
}

fn compile(constraints: &str) -> copper_lang::lower::Result<Design> {
    compile_design_source(&source(constraints), "lanes.copper", false, true)
}

fn generic() -> PhysicalOptions {
    PhysicalOptions::new(100.0, 80.0, 2, "generic").expect("options")
}

fn jlc() -> PhysicalOptions {
    PhysicalOptions::new(100.0, 80.0, 6, "jlcpcb-six-layer").expect("options")
}

fn physical(constraints: &str, options: &PhysicalOptions) -> copper_pcb::Result<PhysicalBoard> {
    physicalize(&compile(constraints).expect("compiled"), Footprints::Proxy, options, false, true)
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// --- L6 breakout + L2/L5 routing properties ---------------------------------

#[test]
fn signal_integrity_routing_properties_lower_to_the_rule() {
    let board = physical(
        "constraint routing(CKP) {
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm; clearance = 0.3mm;
            target_impedance_ohms = 100; target_single_ended_ohms = 50; impedance_tolerance_percent = 7.5;
            layer_group = \"csi-src\";
            breakout_length = 1.2mm; breakout_width = 0.1mm; breakout_gap = 0.15mm; breakout_clearance = 0.1mm;
        }",
        &jlc(),
    )
    .expect("physical");
    let rule = &board.net_routing_rules[0];
    assert_eq!(rule.target_single_ended_ohms, Some(50));
    assert_eq!(rule.impedance_tolerance_percent, Some(Decimal::parse("7.5").unwrap()));
    assert_eq!(rule.layer_group.as_deref(), Some("csi-src"));
    assert_eq!(
        (rule.breakout_length_nm, rule.breakout_width_nm, rule.breakout_gap_nm, rule.breakout_clearance_nm),
        (Some(nm("1.2")), Some(nm("0.1")), Some(nm("0.15")), Some(nm("0.1")))
    );
    // New intent is digest-bound only when declared.
    let plain = NetRoutingRule {
        layer_group: None,
        breakout_length_nm: None,
        breakout_width_nm: None,
        breakout_gap_nm: None,
        breakout_clearance_nm: None,
        impedance_tolerance_percent: None,
        target_single_ended_ohms: None,
        ..rule.clone()
    };
    let mut undeclared = board.clone();
    undeclared.net_routing_rules = vec![plain];
    assert_ne!(
        copper_drc::digest::physical_board_digest(&board),
        copper_drc::digest::physical_board_digest(&undeclared)
    );
}

#[test]
fn invalid_signal_integrity_properties_fail_with_locations() {
    for (properties, expected) in [
        ("target_impedance_ohms = 100; target_single_ended_ohms = 50.5;", "positive integer"),
        ("target_impedance_ohms = 100; target_single_ended_ohms = 0;", "positive integer"),
        ("target_impedance_ohms = 100; impedance_tolerance_percent = 0;", "greater than 0 and below 100"),
        ("target_impedance_ohms = 100; impedance_tolerance_percent = 100;", "greater than 0 and below 100"),
        ("target_impedance_ohms = 100; impedance_tolerance_percent = 5mm;", "must be a number"),
        ("impedance_tolerance_percent = 5;", "requires 'target_impedance_ohms'"),
        ("layer_group = \"\";", "nonempty name"),
        ("breakout_width = 0.1mm;", "require a positive 'breakout_length'"),
        ("breakout_length = 1mm;", "requires 'breakout_width'"),
        ("breakout_length = 0mm; breakout_width = 0.1mm;", "'breakout_length' must be positive"),
        ("breakout_length = 1mm; breakout_width = 0.1;", "'breakout_width' must be a length"),
        ("breakout_length = 1mm; breakout_width = 0.2mm;", "may only relax 'width'"),
        ("breakout_length = 1mm; breakout_gap = 0.3mm;", "may only relax 'pair_gap'"),
        ("breakout_length = 1mm; breakout_clearance = 0.4mm;", "may only relax 'clearance'"),
    ] {
        let constraint = format!(
            "
        constraint routing(CKP) {{
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm; clearance = 0.3mm;
            {properties}
        }}"
        );
        let error = compile(&constraint).err().unwrap_or_else(|| panic!("{properties} compiled"));
        assert!(error.message.contains(expected), "{properties}: {}", error.message);
        assert_eq!(error.code, "CMP110", "{properties}");
        assert_eq!(&*error.location.filename, "lanes.copper");
        assert_eq!(error.location.line, 12, "{properties}: the routing constraint declaration");
    }
}

#[test]
fn breakout_gap_requires_a_pair_gap() {
    let error =
        compile("constraint routing(CKP) { breakout_length = 1mm; breakout_gap = 0.1mm; }").err().expect("error");
    assert!(error.message.contains("requires a differential 'pair_gap'"), "{}", error.message);
}

#[test]
fn breakout_values_relax_board_defaults_but_respect_fabrication_minimums() {
    // No declared width/clearance: the board defaults are the effective profile.
    let relaxed = physical(
        "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.1mm; breakout_clearance = 0.09mm; }",
        &jlc(),
    )
    .expect("relaxed");
    assert_eq!(relaxed.net_routing_rules[0].breakout_width_nm, Some(nm("0.1")));
    for (constraint, options, expected) in [
        (
            "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.1mm; }",
            generic(),
            "breakout width is below the board minimum track width",
        ),
        (
            "constraint routing(CKP) { breakout_length = 1mm; breakout_clearance = 0.1mm; }",
            jlc(),
            "breakout clearance may only relax",
        ),
        (
            "constraint routing(CKP) { breakout_length = 1mm; breakout_width = 0.21mm; }",
            jlc(),
            "breakout width may only relax",
        ),
    ] {
        let error = physical(constraint, &options).err().unwrap_or_else(|| panic!("{constraint} physicalized"));
        assert!(message(&error).contains(expected), "{constraint}: {error}");
    }
}

fn differential() -> NetRoutingRule {
    NetRoutingRule {
        width_nm: Some(nm("0.14")),
        pair_gap_nm: Some(nm("0.26")),
        differential_partner: Some("B".to_owned()),
        kind: RouteKind::Differential,
        ..default_rule("A")
    }
}

/// `NetRoutingRule(net)` with the dataclass defaults.
fn default_rule(net: &str) -> NetRoutingRule {
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
        return_via_policy: copper_pcb::board::ReturnViaPolicy::Always,
        shared_reference_layer: None,
        target_single_ended_ohms: None,
        impedance_tolerance_percent: None,
        layer_group: None,
        breakout_length_nm: None,
        breakout_width_nm: None,
        breakout_gap_nm: None,
        breakout_clearance_nm: None,
        tuning_style: copper_pcb::board::TuningStyle::Bumps,
        tuning_spacing_nm: None,
        tuning_group: None,
    }
}

#[test]
fn net_routing_rule_validates_signal_intent_directly() {
    let base = differential();
    base.validate().expect("valid base");
    let rejected = |rule: NetRoutingRule, expected: &str| {
        let error = rule.validate().err().unwrap_or_else(|| panic!("{expected}: accepted"));
        assert!(message(&error).contains(expected), "{error}");
    };
    rejected(
        NetRoutingRule { breakout_length_nm: Some(1), breakout_gap_nm: Some(nm("0.3")), ..base.clone() },
        "breakout gap may only relax",
    );
    rejected(
        NetRoutingRule { breakout_width_nm: Some(nm("0.1")), ..base.clone() },
        "requires a positive breakout_length",
    );
    rejected(
        NetRoutingRule { impedance_tolerance_percent: Some(Decimal::from_i64(5)), ..base.clone() },
        "impedance tolerance requires",
    );
    let feed = |single: i64| NetRoutingRule {
        kind: RouteKind::RfFeed,
        target_impedance_ohms: Some(50),
        target_single_ended_ohms: Some(single),
        ..default_rule("A")
    };
    rejected(feed(45), "must agree");
    feed(50).validate().expect("agreeing targets");
}

// --- L4 length matching ------------------------------------------------------

#[test]
fn length_match_lowers_to_a_net_match_group() {
    let board = physical(
        "constraint length_match(CKP, CKN, D0P, D0N) { id = \"csi-src-lanes\"; max_skew = 1.5mm; }",
        &generic(),
    )
    .expect("physical");
    let nets: Vec<String> = ["CKP", "CKN", "D0P", "D0N"].map(str::to_owned).to_vec();
    assert_eq!(
        board.match_groups,
        vec![NetMatchGroup { id: "csi-src-lanes".to_owned(), nets, max_skew_nm: nm("1.5") }]
    );
    // Net targets are not mistaken for omitted placement targets.
    assert!(!board.metadata.contains_key("omitted_constraint_targets"));
}

#[test]
fn length_match_without_id_gets_a_deterministic_id() {
    let board = physical("constraint length_match(CKP, CKN) { max_skew = 0.15mm; }", &generic()).expect("physical");
    assert_eq!(board.match_groups[0].id, "length_match:0");
}

#[test]
fn invalid_length_match_fails_with_a_location() {
    for (declaration, expected) in [
        ("constraint length_match(CKP) { max_skew = 1mm; }", "at least two nets"),
        ("constraint length_match(CKP, CKP) { max_skew = 1mm; }", "more than once"),
        ("constraint length_match(CKP, R1.1) { max_skew = 1mm; }", "nets, not pins"),
        (
            "constraint length_match(CKP, CKN) { max_skew = 1mm; tolerance = 1mm; }",
            "unknown length_match parameter 'tolerance'",
        ),
        ("constraint length_match(CKP, CKN) { id = \"x\"; }", "requires 'max_skew'"),
        ("constraint length_match(CKP, CKN) { max_skew = 1; }", "must be a length"),
        ("constraint length_match(CKP, CKN) { max_skew = -1mm; }", "must be positive"),
    ] {
        let error = compile(declaration).err().unwrap_or_else(|| panic!("{declaration} compiled"));
        assert!(error.message.contains(expected), "{declaration}: {}", error.message);
        assert_eq!(error.code, "CMP111");
        assert_eq!(&*error.location.filename, "lanes.copper");
    }
}

#[test]
fn length_match_net_semantics_fail_with_the_constraint_location() {
    for (declarations, expected) in [
        ("constraint length_match(CKP, NOPE) { max_skew = 1mm; }", "unknown net 'NOPE'"),
        (
            "constraint length_match(CKP, CKN) { max_skew = 1mm; }constraint length_match(CKN, D0P) { max_skew = 1mm; }",
            "'CKN' already belongs",
        ),
        (
            "constraint length_match(CKP, CKN) { id = \"g\"; max_skew = 1mm; }constraint length_match(D0P, D0N) { id = \"g\"; max_skew = 1mm; }",
            "duplicate length_match id 'g'",
        ),
    ] {
        let error = physical(declarations, &generic()).err().unwrap_or_else(|| panic!("{declarations} physicalized"));
        let text = message(&error);
        let (location, rest) = text.split_once(": ").expect("a located message");
        assert!(location.starts_with("lanes.copper:"), "{text}");
        let mut parts = location.trim_start_matches("lanes.copper:").split(':');
        assert!(parts.all(|part| part.parse::<u64>().is_ok()), "{text}");
        assert!(rest.contains(expected), "{text}");
    }
}

#[test]
fn physical_board_rejects_inconsistent_match_groups() {
    let board = physical("", &generic()).expect("physical");
    let group = |id: &str, nets: &[&str]| NetMatchGroup {
        id: id.to_owned(),
        nets: nets.iter().map(|n| n.to_string()).collect(),
        max_skew_nm: 1,
    };
    for (groups, expected) in [
        (vec![group("g", &["CKP", "X"])], "unknown net"),
        (vec![group("g", &["CKP", "CKN"]), group("h", &["CKN", "D0P"])], "belongs to length-match groups"),
    ] {
        let mut changed = board.clone();
        changed.match_groups = groups;
        let error = revalidate(&mut changed).err().unwrap_or_else(|| panic!("{expected}: accepted"));
        assert!(message(&error).contains(expected), "{error}");
    }
    // Python's `NetMatchGroup.__post_init__` refused a one-net group; the
    // compiler (`CMP111`) is where Rust enforces it.
    let error = compile("constraint length_match(CKP) { max_skew = 1mm; }").err().expect("one net");
    assert!(error.message.contains("at least two nets"));
}

/// `_routed_lanes`: one straight front track of each length (mm) per net, in
/// name order, rows 3 mm apart from y = 3 mm.
fn routed_lanes(lengths: &[(&str, &str)], max_skew: &str) -> PhysicalBoard {
    let pad = FootprintPad {
        number: "1".to_owned(),
        position: Point::new(0, 0),
        size: Size { width_nm: 400_000, height_nm: 400_000 },
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
    };
    let footprint = PhysicalFootprint {
        name: "test/pad".to_owned(),
        pads: vec![pad],
        body_size: Size { width_nm: MM, height_nm: MM },
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
    };
    let mut sorted = lengths.to_vec();
    sorted.sort();
    let (mut placements, mut nets, mut tracks) = (Vec::new(), Vec::new(), Vec::new());
    let zero = Decimal::zero();
    for (row, (net, length)) in sorted.iter().enumerate() {
        let y = (3 + 3 * row as i64) * MM;
        let end = 2 * MM + nm(length);
        placements.push(Placement::new(&format!("{net}_A"), &footprint.name, Point::new(2 * MM, y), &zero).unwrap());
        placements.push(Placement::new(&format!("{net}_B"), &footprint.name, Point::new(end, y), &zero).unwrap());
        nets.push(PhysicalNet {
            name: net.to_string(),
            pads: vec![PadReference::new(&format!("{net}_A"), "1"), PadReference::new(&format!("{net}_B"), "1")],
        });
        tracks.push(TrackSegment {
            net: net.to_string(),
            start: Point::new(2 * MM, y),
            end: Point::new(end, y),
            width_nm: 200_000,
            layer: CopperLayer::Front,
        });
    }
    let mut board = lanes_board(footprint, placements, nets);
    board.tracks = tracks;
    board.metadata.insert("detailed_routing".to_owned(), "complete".to_owned());
    board.rules.minimum_clearance_nm = nm("0.15");
    board.match_groups = vec![NetMatchGroup {
        id: "lanes".to_owned(),
        nets: sorted.iter().map(|(net, _)| net.to_string()).collect(),
        max_skew_nm: nm(max_skew),
    }];
    board
}

/// `PhysicalBoard("Lanes", 30 x 20 mm, ...)` with the dataclass defaults.
fn lanes_board(footprint: PhysicalFootprint, placements: Vec<Placement>, nets: Vec<PhysicalNet>) -> PhysicalBoard {
    PhysicalBoard {
        name: "Lanes".to_owned(),
        outline: BoardOutline::rectangle(30 * MM, 20 * MM, None).expect("outline"),
        footprints: IndexMap::from([(footprint.name.clone(), footprint)]),
        placements,
        nets,
        stackup: copper_pcb::board::Stackup::new(vec![CopperLayer::Front, CopperLayer::Back]),
        rules: copper_physical::DesignRules::default(),
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

fn drc(board: &PhysicalBoard) -> copper_drc::Report {
    run_physical_drc(board, Policy::default()).expect("DRC")
}

#[test]
fn length_match_verification_reports_member_lengths_and_group_skew() {
    let routed = routed_lanes(&[("CKP", "10"), ("D0P", "11"), ("D1P", "10.4")], "1.5");
    let result = &verify_match_groups(&routed).expect("verified")[0];
    assert_eq!(
        result.members,
        vec![
            ("CKP".to_owned(), 10 * MM, true),
            ("D0P".to_owned(), 11 * MM, true),
            ("D1P".to_owned(), nm("10.4"), true)
        ]
    );
    assert_eq!((result.skew_nm, result.status), (Some(MM), "pass"));
    let copper_json::Value::Object(document) = result.to_value() else { panic!("object") };
    let copper_json::Value::Array(members) = &document["members"] else { panic!("members") };
    let copper_json::Value::Object(first) = &members[0] else { panic!("member") };
    assert_eq!(first["shortfall_nm"], copper_json::Value::from(MM));
    let report = drc(&routed);
    assert!(!report.findings.iter().any(|f| f.code == "DRC-LENGTH-MATCH"));
    let coverage = report.coverage.iter().find(|c| c.check == "length_match").expect("coverage");
    assert!(coverage.status == CoverageStatus::Executed && coverage.required);
}

#[test]
fn group_skew_over_the_limit_is_a_drc_review_warning() {
    // Connected copper over its group skew: a review warning, not a geometry
    // error (requirement verification still fails the declared group).
    let report = drc(&routed_lanes(&[("CKP", "10"), ("D0P", "12")], "1.5"));
    let finding = report.findings.iter().find(|f| f.code == "DRC-LENGTH-MATCH").expect("finding");
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!((finding.required_nm, finding.measured_nm), (Some(nm("1.5")), Some(2 * MM)));
    assert_eq!(finding.nets, ["CKP", "D0P"]);
    assert_eq!(finding.objects, ["match_group:lanes"]);
    assert!(!report.findings.iter().any(|f| f.severity == Severity::Error && f.code == "DRC-LENGTH-MATCH"));
}

#[test]
fn incompletely_routed_groups_are_not_judged() {
    let mut partial = routed_lanes(&[("CKP", "10"), ("D0P", "14")], "1.5");
    partial.tracks.truncate(1);
    let result = &verify_match_groups(&partial).expect("verified")[0];
    assert_eq!((result.status, result.skew_nm), ("incomplete", None));
    assert_eq!(result.members.iter().map(|m| m.2).collect::<Vec<_>>(), [true, false]);
    assert!(!drc(&partial).findings.iter().any(|f| f.code == "DRC-LENGTH-MATCH"));
}

#[test]
fn boards_without_groups_report_length_match_not_applicable() {
    let mut routed = routed_lanes(&[("CKP", "10"), ("D0P", "12")], "1.5");
    routed.match_groups.clear();
    let report = drc(&routed);
    let coverage = report.coverage.iter().find(|c| c.check == "length_match").expect("coverage");
    assert_eq!(coverage.status, CoverageStatus::NotApplicable);
    assert!(verify_match_groups(&routed).expect("verified").is_empty());
}

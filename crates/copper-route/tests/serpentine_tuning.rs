//! S-shaped serpentines through critical routing and the language (D-PHY
//! plan R10), ported from `tests/test_serpentine_tuning.py`. The
//! `UnitTuner` cases are in `src/test_support_planes.rs`.

mod support_planes;

use copper_drc::digest::physical_board_digest;
use copper_drc::signal_integrity::verify_match_groups;
use copper_pcb::board::{NetRoutingRule, PhysicalBoard, RouteKind, TuningStyle};
use copper_route::critical::{CriticalOptions, CriticalRoutingResult, critical_net_document, route_critical_nets};
use copper_route::critical_tuning::match_tuning_line;
use support_planes::lanes::*;
use support_planes::*;

const SERPENTINE_AMPLITUDE: i64 = 350_000;

fn critical(board: &PhysicalBoard, guides: &copper_route::GlobalRoutingResult) -> CriticalRoutingResult {
    route_critical_nets(board.clone(), guides, &CriticalOptions::default(), None).expect("critical")
}

fn with_style(mut board: PhysicalBoard, style: TuningStyle) -> PhysicalBoard {
    for rule in &mut board.net_routing_rules {
        rule.tuning_style = style;
    }
    board
}

#[test]
fn group_tuning_uses_serpentines_clear_of_neighbours_under_native_drc() {
    // LANE_B is 4 mm short; walls 0.5 mm off both sides of its 0.2/0.2 pair.
    let board = Lanes { amplitude: Some(SERPENTINE_AMPLITUDE), ..Lanes::new(&TWO_LANES) }
        .keepouts(vec![lane_keepout("above", 6.0, 14.8, 28.0, 17.0), lane_keepout("below", 6.0, 11.0, 28.0, 13.2)])
        .build();
    let board = with_style(board, TuningStyle::Serpentine);
    let guides = global(&board);
    let result = critical(&board, &guides);
    assert_eq!(result.match_tuning.len(), 1);
    let tuning = &result.match_tuning[0];
    assert_eq!(text(field(tuning, "status")), "tuned", "{}", text(field(tuning, "reason")));
    assert!(int(field(tuning, "skew_after_nm")).unwrap() <= int(field(tuning, "max_skew_nm")).unwrap());
    assert!(hard_findings(&result.board).is_empty(), "{:?}", hard_findings(&result.board));
    let units = field(tuning, "units").as_array().expect("units");
    assert_eq!(units.len(), 1);
    let unit = &units[0];
    let nets: Vec<&str> = field(unit, "nets").as_array().unwrap().iter().map(text).collect();
    assert_eq!(nets, ["LANE_B_N", "LANE_B_P"]);
    assert_eq!((text(field(unit, "stage")), text(field(unit, "style"))), ("final", "serpentine"));
    assert_eq!(int(field(unit, "bumps")), Some(0));
    let legs = int(field(unit, "legs")).unwrap();
    assert_eq!(legs as usize, field(unit, "amplitudes_nm").as_array().unwrap().len() + 1);
    assert_eq!(int(field(tuning, "legs")), Some(legs));
    assert_eq!(field(unit, "added_length_nm"), field(unit, "added_length_nm"));
    // Pair skew is unchanged and the copper stays between the walls.
    let lane: Vec<_> = result.board.tracks.iter().filter(|t| t.net.starts_with("LANE_B_")).cloned().collect();
    assert_eq!(net_length(&lane, "LANE_B_N"), net_length(&lane, "LANE_B_P"));
    assert!(lane.iter().flat_map(|t| [t.start, t.end]).all(|p| nm(13.3) <= p.y_nm && p.y_nm <= nm(14.7)));
    let checks = verify_match_groups(&result.board).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, "pass");
    // The unit document carries exactly the reported fields.
    let keys: Vec<&str> = unit.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["nets", "stage", "style", "added_length_nm", "bumps", "legs", "amplitudes_nm"]);
    let line = match_tuning_line(&tuning_result(tuning));
    assert!(line.contains(&format!("LANE_B_N/LANE_B_P serpentine {legs} legs up to")), "{line}");
    // Deterministic.
    let again = critical(&board, &global(&board));
    assert_eq!(again.board, result.board);
    assert_eq!(again.to_json().unwrap(), result.to_json().unwrap());
}

#[test]
fn bumps_style_reports_and_routes_as_before() {
    let board = Lanes::new(&TWO_LANES).build();
    assert_eq!(with_style(board.clone(), TuningStyle::Bumps), board, "bumps is the default");
    let guides = global(&board);
    let result = critical(&board, &guides);
    let tuning = &result.match_tuning[0];
    assert_eq!(text(field(tuning, "status")), "tuned");
    // Plain bump steps are not reported as units.
    assert!(!tuning.as_object().unwrap().contains_key("units"));
    let document = copper_json::parse(&result.to_json().unwrap()).unwrap();
    assert!(!field(&document, "match_tuning").as_array().unwrap()[0].as_object().unwrap().contains_key("units"));
    // A serpentine declared on nets outside any match group changes nothing.
    let mut alone = board.clone();
    alone.match_groups.clear();
    let serpentine = with_style(alone.clone(), TuningStyle::Serpentine);
    let (first, second) = (critical(&alone, &guides), critical(&serpentine, &guides));
    assert_eq!(first.locked_tracks, second.locked_tracks);
    let docs = |result: &CriticalRoutingResult| result.nets.iter().map(critical_net_document).collect::<Vec<_>>();
    assert_eq!(docs(&first), docs(&second));
    assert_eq!(first.routing_fingerprint, second.routing_fingerprint);
}

// --- Language ----------------------------------------------------------------

fn lanes_source(properties: &str, partner: &str) -> String {
    format!(
        r#"board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        constraint routing(CKP) {{
            kind = differential; partner = CKN; width = 0.14mm; pair_gap = 0.26mm;
            {properties}
        }}
        constraint routing(CKN) {{
            kind = differential; partner = CKP; width = 0.14mm; pair_gap = 0.26mm; {partner}
        }}
    }}"#
    )
}

const SERPENTINE: &str = r#"tuning_style = "serpentine";"#;

fn jlc(name: &str, source: &str) -> Result<PhysicalBoard, String> {
    physicalize_source(name, source, 6, "jlcpcb-six-layer")
}

#[test]
fn tuning_properties_lower_to_the_rule_and_bind_the_digest_only_when_declared() {
    let board =
        jlc("declared.copper", &lanes_source(r#"tuning_style = "serpentine"; tuning_spacing = 0.6mm;"#, SERPENTINE))
            .unwrap();
    let rule = board.net_routing_rules.iter().find(|rule| rule.net == "CKP").unwrap();
    assert_eq!((rule.tuning_style, rule.tuning_spacing_nm), (TuningStyle::Serpentine, Some(nm(0.6))));
    let plain = jlc("plain.copper", &lanes_source("", "")).unwrap();
    assert!(plain.net_routing_rules.iter().all(|rule| rule.tuning_style == TuningStyle::Bumps));
    let bumps = jlc("bumps.copper", &lanes_source(r#"tuning_style = "bumps";"#, "")).unwrap();
    assert_eq!(physical_board_digest(&bumps), physical_board_digest(&plain));
    assert_ne!(physical_board_digest(&board), physical_board_digest(&plain));
}

#[test]
fn invalid_tuning_properties_fail_with_locations() {
    let cases = [
        (r#"tuning_style = "zigzag";"#, r#"must be "bumps" or "serpentine""#),
        ("tuning_spacing = 0.5mm;", r#"requires tuning_style = "serpentine""#),
        (r#"tuning_style = "serpentine"; tuning_spacing = 0mm;"#, "'tuning_spacing' must be positive"),
        (r#"tuning_style = "serpentine"; tuning_spacing = 0.5;"#, "'tuning_spacing' must be a length"),
    ];
    for (index, (properties, message)) in cases.iter().enumerate() {
        let source = lanes_source(properties, SERPENTINE);
        let name = format!("lanes-{index}");
        let error = match load_source(&format!("{name}/lanes.copper"), &source) {
            Err(copper_lang::compiler::LoadError::Compile(error)) => error,
            other => panic!("{properties}: expected a compile error, got {:?}", other.map(|_| ())),
        };
        assert!(error.message.contains(message), "{properties}: {}", error.message);
        assert_eq!(error.code, "CMP110");
        assert!(error.location.filename.ends_with("lanes.copper"), "{}", error.location.filename);
    }
}

#[test]
fn pair_members_must_agree_on_the_tuning_style() {
    let error = jlc("disagree.copper", &lanes_source("", SERPENTINE)).unwrap_err();
    assert!(error.contains("must agree on tuning_style"), "{error}");
    let spacing = NetRoutingRule { kind: RouteKind::Clock, tuning_spacing_nm: Some(nm(0.5)), ..rule("A") };
    assert!(spacing.validate().unwrap_err().to_string().contains("tuning spacing requires"));
    // An unknown style cannot be represented (Python raised on construction).
    assert_eq!(TuningStyle::parse("zigzag"), None);
}

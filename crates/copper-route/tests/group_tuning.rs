//! Group serpentines: adjacent units of a length_match group tuned together
//! (D-PHY plan R14), ported from `tests/test_group_tuning.py`. The
//! `_drop_group_steps` case is in `src/test_support_planes.rs`.

mod support_planes;

use copper_drc::digest::physical_board_digest;
use copper_drc::signal_integrity::verify_match_groups;
use copper_json::Value;
use copper_pcb::board::{NetMatchGroup, NetRoutingRule, PhysicalBoard, RouteKind, TrackSegment, TuningStyle};
use copper_physical::geometry::segment_distance_squared;
use copper_physical::{CopperLayer, Point};
use copper_route::RoutingClearanceIndex;
use copper_route::critical::{
    CriticalOptions, CriticalRoutingResult, CriticalRoutingStatus, critical_net_document, route_critical_nets,
};
use copper_route::critical_group_tuning::{CORNER_SLACK_NM, GroupTuner, lane_corners};
use copper_route::critical_tuning::{TunerOptions, bump_chamfers, match_tuning_line};
use support_planes::groups::*;
use support_planes::lanes::{Lanes, SPACING, THREE_LANES, WIDTH};
use support_planes::paths::*;
use support_planes::*;

fn critical(board: &PhysicalBoard, guides: &copper_route::GlobalRoutingResult) -> CriticalRoutingResult {
    route_critical_nets(board.clone(), guides, &CriticalOptions::default(), None).expect("critical")
}

fn only_tuning(result: &CriticalRoutingResult) -> &Value {
    assert_eq!(result.match_tuning.len(), 1, "one match group");
    &result.match_tuning[0]
}

fn texts(value: &Value) -> Vec<&str> {
    value.as_array().unwrap().iter().map(text).collect()
}

fn units(tuning: &Value) -> Vec<Value> {
    tuning.as_object().unwrap().get("units").map_or_else(Vec::new, |units| units.as_array().unwrap().to_vec())
}

fn amplitudes(unit: &Value) -> Vec<i64> {
    field(unit, "amplitudes_nm").as_array().unwrap().iter().map(|a| int(a).unwrap()).collect()
}

fn path_of(tracks: &[TrackSegment], net: &str) -> Vec<TrackSegment> {
    tracks.iter().filter(|track| track.net == net).cloned().collect()
}

/// `_offsets`: rises of a net's track ends above (or below) `line`.
fn offsets(tracks: &[TrackSegment], line: i64) -> Vec<i64> {
    ends(tracks).map(|point| point.y_nm - line).collect()
}

/// `_check_lanes`: every lane gained `added`, pairs keep their spacing,
/// neighbours their gap, and no bend is sharper than 45 degrees.
fn check_lanes(board: &PhysicalBoard, added: i64) {
    let paths: Vec<Vec<TrackSegment>> = LANES.iter().map(|net| path_of(&board.tracks, net)).collect();
    for (net, path) in LANES.iter().zip(&paths) {
        assert_eq!(net_length(path, net), nm(20.0) + added, "{net}");
        assert!(continuous(path), "{net}");
        assert_eq!(sharp_bends(path, &[]), Vec::<Point>::new(), "{net}");
    }
    for index in 0..LANES.len() - 1 {
        let (first, second) = (LANES[index], LANES[index + 1]);
        let gap = if first[..first.len() - 2] == second[..second.len() - 2] { SPACING } else { nm(0.5) };
        assert_eq!(least_gap_squared(&paths[index], &paths[index + 1]), squared(gap), "{first}/{second}");
    }
    for index in 0..NAMES.len() {
        for offset in lane_offsets(&paths[2 * index], &paths[2 * index + 1]) {
            assert!(squared(SPACING) <= offset && offset < squared(SPACING + 2), "{}: {offset}", NAMES[index]);
        }
    }
}

fn added_by_net(tuning: &Value) -> Vec<(String, i64)> {
    field(tuning, "members")
        .as_array()
        .unwrap()
        .iter()
        .map(|member| (text(field(member, "net")).to_owned(), int(field(member, "added_length_nm")).unwrap()))
        .collect()
}

#[test]
fn a_hemmed_bundle_is_bent_as_one_group_while_routing() {
    let board = Group::default().build();
    let guides = global(&board);
    let result = critical(&board, &guides);
    let tuning = only_tuning(&result);
    assert_eq!(
        (text(field(tuning, "status")), int(field(tuning, "skew_before_nm")), int(field(tuning, "skew_after_nm"))),
        ("tuned", Some(nm(4.0)), Some(0)),
        "{}",
        text(field(tuning, "reason"))
    );
    assert_ne!(result.status, CriticalRoutingStatus::Failed);
    assert!(hard_findings(&result.board).is_empty(), "{:?}", hard_findings(&result.board));
    // One group step, as soon as all three pairs were accepted: every lane
    // gains exactly the 4 mm, in one-sided bumps below the bundle.
    let units = units(tuning);
    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(
        (text(field(unit, "group")), text(field(unit, "stage")), text(field(unit, "style"))),
        ("bundle", "routing", "bumps")
    );
    assert_eq!(texts(field(unit, "lanes")), LANES);
    assert_eq!(texts(field(unit, "nets")), LANES);
    let bumps = int(field(unit, "bumps")).unwrap();
    assert_eq!(bumps as usize, amplitudes(unit).len());
    assert!(amplitudes(unit).into_iter().max().unwrap() <= nm(2.0));
    let mut expected: Vec<(String, i64)> = LANES.iter().map(|net| ((*net).to_owned(), nm(4.0))).collect();
    expected.extend([("LANE_L_N".to_owned(), 0), ("LANE_L_P".to_owned(), 0)]);
    let mut added = added_by_net(tuning);
    added.sort();
    expected.sort();
    assert_eq!(added, expected);
    check_lanes(&result.board, nm(4.0));
    let top = LANES.iter().flat_map(|net| ends(&path_of(&result.board.tracks, net)).collect::<Vec<_>>());
    assert_eq!(top.map(|point| point.y_nm).min(), Some(nm(13.9)));
    let checks = verify_match_groups(&result.board).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, "pass");
    let document = copper_json::parse(&result.to_json().unwrap()).unwrap();
    let tuned = &field(&document, "match_tuning").as_array().unwrap()[0];
    let first_unit = &field(tuned, "units").as_array().unwrap()[0];
    assert_eq!(text(field(first_unit, "group")), "bundle");
    assert_eq!(texts(field(first_unit, "lanes")), LANES);
    let groups = field(tuned, "tuning_groups").as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(text(field(&groups[0], "name")), "bundle");
    assert_eq!(texts(field(&groups[0], "nets")), LANES);
    assert_eq!((text(field(&groups[0], "status")), text(field(&groups[0], "reason"))), ("tuned", ""));
    let line = match_tuning_line(&tuning_result(tuning));
    assert!(line.contains(&format!("group bundle {} bumps {bumps} bumps up to", LANES.join("/"))), "{line}");
    assert!(line.contains("while routing"), "{line}");
    // Deterministic.
    let again = critical(&board, &guides);
    assert_eq!(again.board, result.board);
    assert_eq!(again.to_json().unwrap(), result.to_json().unwrap());

    // Independent pair tuning exhausts the narrow room beside LANE_A. It
    // must report the remaining skew, never claim a pass: the copper stays
    // connected and the miss is a review warning (a length-match finding is
    // a warning, not a hard DRC error). The explicit group above is what
    // makes this hemmed bundle tunable.
    let alone = critical(&Group { group: None, ..Group::default() }.build(), &guides);
    let tuning = only_tuning(&alone);
    assert_eq!(text(field(tuning, "status")), "failed");
    assert!(hard_findings(&alone.board).is_empty(), "{:?}", hard_findings(&alone.board));
    let warnings: Vec<String> = copper_drc::run_physical_drc(&alone.board, copper_drc::Policy::default())
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.severity == copper_drc::Severity::Warning)
        .map(|finding| finding.code)
        .collect();
    assert!(warnings.iter().any(|code| code == "DRC-LENGTH-MATCH"), "{warnings:?}");
    assert_eq!(alone.status, CriticalRoutingStatus::Warning);
    let reason = text(field(tuning, "reason"));
    assert!(reason.contains("LANE_A_N/LANE_A_P: insufficient tuning room"), "{reason}");
    assert!(int(field(tuning, "skew_after_nm")).unwrap() > int(field(tuning, "max_skew_nm")).unwrap());
    let checks = verify_match_groups(&alone.board).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, "fail");
    // The Python test's last part monkeypatched `_tune_bundle_pairs` away to
    // exercise the final pass alone; Rust has no such seam (see PORTED-planes.md).
}

#[test]
fn a_run_too_short_for_a_group_bump_leaves_the_copper_and_says_why() {
    // 6 mm between the lands: a group bump needs the inner lane's 0.6 mm,
    // twice the 2.2 mm lane span and 0.6 mm at each end. A 0.2 mm amplitude
    // also rules out every pair's own bumps.
    let short = Group { run: 6.0, longest: 10.0, amplitude: 0.2, keepouts: Vec::new(), ..Group::default() };
    let board = short.build();
    let guides = global(&board);
    let result = critical(&board, &guides);
    let tuning = only_tuning(&result);
    assert_eq!(text(field(tuning, "status")), "failed");
    assert!(units(tuning).is_empty());
    let groups = field(tuning, "tuning_groups").as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!((text(field(&groups[0], "name")), text(field(&groups[0], "status"))), ("bundle", "failed"));
    let reason = text(field(&groups[0], "reason"));
    assert!(
        reason.contains(
            "the longest straight run of all 6 lanes is 6.000 mm, a one-sided group bump needs 6.200 mm: the \
             inner lane's 0.600 mm plus twice the 2.200 mm lane span, and 0.600 mm at each end"
        ),
        "{reason}"
    );
    let line = match_tuning_line(&tuning_result(tuning));
    assert!(line.contains("tuning group bundle failed (tuning group bundle: insufficient tuning room"), "{line}");
    let mut plain =
        Group { run: 6.0, longest: 10.0, amplitude: 0.2, keepouts: Vec::new(), group: None, ..Group::default() }
            .build();
    plain.match_groups.clear();
    let baseline = critical(&plain, &guides);
    assert_eq!(result.locked_tracks, baseline.locked_tracks);
    let docs = |result: &CriticalRoutingResult| result.nets.iter().map(critical_net_document).collect::<Vec<_>>();
    assert_eq!(docs(&result), docs(&baseline));
}

const LINES: [f64; 6] = [13.9, 14.3, 14.8, 15.2, 15.7, 16.1];

#[test]
fn a_group_serpentine_needs_room_on_both_sides() {
    let open = Group { style: TuningStyle::Serpentine, keepouts: Vec::new(), ..Group::default() }.build();
    let result = critical(&open, &global(&open));
    let tuning = only_tuning(&result);
    assert_eq!(text(field(tuning, "status")), "tuned", "{}", text(field(tuning, "reason")));
    assert!(hard_findings(&result.board).is_empty(), "{:?}", hard_findings(&result.board));
    let found = units(tuning);
    assert_eq!(found.len(), 1);
    assert_eq!((text(field(&found[0], "style")), int(field(&found[0], "bumps"))), ("serpentine", Some(0)));
    assert_eq!(int(field(&found[0], "legs")).unwrap() as usize, amplitudes(&found[0]).len() + 1);
    check_lanes(&result.board, nm(4.0));
    // Every lane snakes about its own line, to both sides.
    for (net, line) in LANES.iter().zip(LINES) {
        let rises = offsets(&path_of(&result.board.tracks, net), nm(line));
        assert!(*rises.iter().min().unwrap() < 0 && 0 < *rises.iter().max().unwrap(), "{net}");
    }
    // With the wall above, the group falls back to one-sided bumps below.
    let walled = Group { style: TuningStyle::Serpentine, ..Group::default() }.build();
    let result = critical(&walled, &global(&walled));
    let tuning = only_tuning(&result);
    let found = units(tuning);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (text(field(tuning, "status")), text(field(&found[0], "style")), int(field(&found[0], "legs"))),
        ("tuned", "bumps", Some(0))
    );
    check_lanes(&result.board, nm(4.0));
    for (net, line) in LANES.iter().zip(LINES) {
        assert_eq!(offsets(&path_of(&result.board.tracks, net), nm(line)).into_iter().min(), Some(0), "{net}");
    }
}

#[test]
fn uneven_lanes_gain_exactly_the_same_length() {
    // A pair, a single net and a pair at uneven gaps (y in mm).
    let lines = [("A", 10.0), ("B", 10.4), ("C", 11.13), ("D", 11.937), ("E", 12.337)];
    let tracks: Vec<TrackSegment> =
        lines.iter().map(|(net, y)| track(net, mm(5.0, *y), mm(45.0, *y), WIDTH, CopperLayer::Front)).collect();
    let empty = board("Empty", 60.0, 60.0);
    let obstacles = RoutingClearanceIndex::new(&empty).unwrap();
    let units: Vec<Vec<String>> = [&["A", "B"][..], &["C"], &["D", "E"]]
        .iter()
        .map(|unit| unit.iter().map(|n| (*n).to_owned()).collect())
        .collect();
    for style in [TuningStyle::Bumps, TuningStyle::Serpentine] {
        let tuner = GroupTuner::new(
            &empty,
            &obstacles,
            tracks.clone(),
            &units,
            &[SPACING, 0, SPACING],
            nm(2.0),
            nm(0.1),
            TunerOptions { style, ..TunerOptions::default() },
        )
        .unwrap();
        let mut styles = std::collections::BTreeSet::new();
        for added in [nm(1.9), nm(4.3), nm(7.0) + 2] {
            let plan = tuner.plan(added).unwrap();
            assert_eq!(plan.reason, "", "{style:?} {added}");
            assert_eq!(plan.lanes, lines.iter().map(|(net, _)| (*net).to_owned()).collect::<Vec<_>>());
            styles.insert(plan.style());
            let planned = plan.tracks.as_ref().expect("tracks");
            let paths: Vec<Vec<TrackSegment>> = lines.iter().map(|(net, _)| path_of(planned, net)).collect();
            for ((net, _), path) in lines.iter().zip(&paths) {
                assert_eq!(net_length(path, net), nm(40.0) + added, "{style:?} {net}");
                assert_eq!(sharp_bends(path, &[]), Vec::<Point>::new(), "{style:?} {net}");
            }
            for index in 0..lines.len() - 1 {
                let gap = nm(lines[index + 1].1) - nm(lines[index].1);
                let least = paths[index]
                    .iter()
                    .flat_map(|a| {
                        paths[index + 1].iter().map(move |b| segment_distance_squared(a.start, a.end, b.start, b.end))
                    })
                    .min()
                    .unwrap();
                assert_eq!(least, squared(gap), "{style:?} {added} lanes {index}/{}", index + 1);
            }
        }
        let expected: &[&str] = if style == TuningStyle::Serpentine { &["bumps", "serpentine"] } else { &["bumps"] };
        assert_eq!(styles.into_iter().collect::<Vec<_>>(), expected);
    }
}

/// `2k - round(k sqrt 2)`: the rounded length a chamfer with leg `k` saves.
fn diagonal_loss(leg: i64) -> i64 {
    2 * leg - ((2 * leg * leg) as f64).sqrt().round() as i64
}

#[test]
fn lane_corners_are_r9_for_a_pair_and_level_rounding_for_more_lanes() {
    // A pair alone takes R9's coupled chamfers: inside c, outside c + d.
    for chamfer in [1, 1_000, 99_999] {
        let (inside, outside) = bump_chamfers(4 * chamfer + nm(1.0), chamfer, SPACING);
        assert_eq!(
            lane_corners(&[SPACING], chamfer, CORNER_SLACK_NM),
            Some(vec![(outside, inside), (inside, outside)]),
            "{chamfer}"
        );
    }
    // More lanes: every lane loses the same rounded length per leg, and
    // adjacent offsets never exceed floor((2 - sqrt 2) x gap).
    let gaps = [SPACING, nm(0.5), 333_333, SPACING, 1_234_567];
    for chamfer in [1, 17, 2_000, 140_000] {
        let corners = lane_corners(&gaps, chamfer, CORNER_SLACK_NM).expect("corners");
        let losses: std::collections::BTreeSet<i64> =
            corners.iter().map(|(low, up)| diagonal_loss(*low) + diagonal_loss(*up)).collect();
        assert_eq!(losses.len(), 1, "{chamfer}: {corners:?}");
        assert_eq!((corners[corners.len() - 1].0, corners[0].1), (chamfer, chamfer));
        for (index, gap) in gaps.iter().enumerate() {
            let ((low, up), (next_low, next_up)) = (corners[index], corners[index + 1]);
            let step = 2 * gap - ((2 * gap * gap) as f64).sqrt() as i64 - 1;
            assert!((0..=step).contains(&(low - next_low)) && (0..=step).contains(&(next_up - up)), "{chamfer}");
        }
    }
}

#[test]
fn boards_without_tuning_groups_report_no_group_steps() {
    // The Python test also replaced `GroupTuner` with a refusing stub to
    // prove the group tuner is never reached; Rust keeps the observable part.
    let board = Lanes::new(&THREE_LANES).build();
    let result = critical(&board, &global(&board));
    let tuning = only_tuning(&result);
    assert_eq!(text(field(tuning, "status")), "tuned");
    assert!(!tuning.as_object().unwrap().contains_key("tuning_groups"));
    for unit in units(tuning) {
        let object = unit.as_object().unwrap();
        assert!(!object.contains_key("group") && !object.contains_key("lanes"));
    }
    let rebuilt = tuning_result(tuning);
    assert!(rebuilt.tuning_groups.is_empty());
    assert!(rebuilt.units.iter().all(|unit| unit.group.is_empty() && unit.lanes.is_empty()));
    let document = copper_json::parse(&result.to_json().unwrap()).unwrap();
    let tuned = field(&document, "match_tuning").as_array().unwrap()[0].as_object().unwrap().clone();
    assert!(!tuned.contains_key("tuning_groups"));
    assert!(!match_tuning_line(&rebuilt).contains("tuning group"));
}

// --- Language ----------------------------------------------------------------

const PARTNERS: [(&str, &str); 4] = [("CKP", "CKN"), ("CKN", "CKP"), ("D0P", "D0N"), ("D0N", "D0P")];
const ONE_GROUP: &str = "constraint length_match(CKP, CKN, D0P, D0N) { max_skew = 0.1mm; }";
const IN_LANES: &str = r#"tuning_group = "lanes";"#;

fn lanes_source(properties: &[(&str, &str)], groups: &str) -> String {
    let rules: Vec<String> = PARTNERS
        .iter()
        .map(|(net, partner)| {
            let extra = properties.iter().find(|(name, _)| name == net).map_or("", |(_, text)| *text);
            format!(
                "constraint routing({net}) {{
            kind = differential; partner = {partner}; width = 0.14mm; pair_gap = 0.26mm;
            tuning_amplitude_limit = 1mm; {extra}
        }}"
            )
        })
        .collect();
    format!(
        r#"board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        component R3: RESISTOR {{ footprint = "0402"; }}
        component R4: RESISTOR {{ footprint = "0402"; }}
        net CKP {{ R1.1; R2.1; }}
        net CKN {{ R1.2; R2.2; }}
        net D0P {{ R3.1; R4.1; }}
        net D0N {{ R3.2; R4.2; }}
        {}
        {groups}
    }}"#,
        rules.join("\n")
    )
}

fn all_in_lanes() -> Vec<(&'static str, &'static str)> {
    PARTNERS.iter().map(|(net, _)| (*net, IN_LANES)).collect()
}

fn jlc(name: &str, source: &str) -> Result<PhysicalBoard, String> {
    physicalize_source(name, source, 6, "jlcpcb-six-layer")
}

#[test]
fn tuning_group_lowers_to_the_rule_and_binds_the_digest_only_when_declared() {
    let board = jlc("group-declared.copper", &lanes_source(&all_in_lanes(), ONE_GROUP)).unwrap();
    for rule in &board.net_routing_rules {
        assert_eq!(rule.tuning_group.as_deref(), Some("lanes"), "{}", rule.net);
    }
    let names: std::collections::BTreeSet<&str> = board.net_routing_rules.iter().map(|r| r.net.as_str()).collect();
    assert_eq!(names, ["CKN", "CKP", "D0N", "D0P"].into_iter().collect());
    let plain = jlc("group-plain.copper", &lanes_source(&[], ONE_GROUP)).unwrap();
    assert!(plain.net_routing_rules.iter().all(|rule| rule.tuning_group.is_none()));
    assert_ne!(physical_board_digest(&board), physical_board_digest(&plain));
    let mut cleared = board.clone();
    for rule in &mut cleared.net_routing_rules {
        rule.tuning_group = None;
    }
    assert_eq!(physical_board_digest(&plain), physical_board_digest(&cleared));
}

#[test]
fn invalid_tuning_group_values_fail_with_locations() {
    let cases = [
        (r#"tuning_group = "";"#, "'tuning_group' must be a nonempty name"),
        ("tuning_group = 3;", "'tuning_group' must be a nonempty name"),
        (r#"kind = general; tuning_group = "lanes";"#, "'tuning_group' requires a critical routing kind"),
    ];
    for (index, (properties, message)) in cases.iter().enumerate() {
        let source = format!(
            r#"board Lanes {{
        use library "tiny";
        component R1: RESISTOR {{ footprint = "0402"; }}
        component R2: RESISTOR {{ footprint = "0402"; }}
        net CK {{ R1.1; R2.1; }}
        constraint routing(CK) {{ {properties} }}
    }}"#
        );
        let error = match load_source(&format!("group-invalid-{index}/lanes.copper"), &source) {
            Err(copper_lang::compiler::LoadError::Compile(error)) => error,
            other => panic!("{properties}: expected a compile error, got {:?}", other.map(|_| ())),
        };
        assert!(error.message.contains(message), "{properties}: {}", error.message);
        assert_eq!(error.code, "CMP110");
        assert!(error.location.filename.ends_with("lanes.copper"), "{}", error.location.filename);
    }
}

/// Extra routing properties by net.
type Properties = Vec<(&'static str, &'static str)>;

#[test]
fn tuning_group_semantics_fail_with_the_routing_constraint_location() {
    let split = "constraint length_match(CKP, CKN) { id = \"ck\"; max_skew = 0.1mm; }\
                 constraint length_match(D0P, D0N) { id = \"d0\"; max_skew = 0.1mm; }";
    let cases: [(Properties, &str, &str); 3] = [
        (
            vec![("CKP", IN_LANES), ("CKN", IN_LANES)],
            "",
            "tuning_group 'lanes' requires 'CKP' to belong to a length_match group",
        ),
        (all_in_lanes(), split, "tuning_group 'lanes' joins 'D0P' of length_match group 'd0' and 'CKP' of 'ck'"),
        (vec![("CKP", IN_LANES)], ONE_GROUP, "routing rules for 'CKP' and 'CKN' must agree on tuning_group"),
    ];
    for (index, (properties, groups, message)) in cases.iter().enumerate() {
        let source = lanes_source(properties, groups);
        let error = jlc(&format!("group-semantics-{index}/lanes.copper"), &source).unwrap_err();
        let expected = format!("CMP117: {message}");
        let at = error.find(&expected).unwrap_or_else(|| panic!("{error}"));
        // `lanes.copper:LINE:COLUMN: CMP117: ...`
        let location: Vec<&str> = error[..at].trim_end_matches(": ").rsplitn(3, ':').collect();
        assert!(location.len() == 3 && location[2].ends_with("lanes.copper"), "{error}");
        assert!(location[..2].iter().all(|part| part.parse::<u32>().is_ok()), "{error}");
    }
}

#[test]
fn physical_boards_reject_inconsistent_tuning_groups() {
    let blank = NetRoutingRule { kind: RouteKind::Clock, tuning_group: Some(" ".to_owned()), ..rule("A") };
    assert!(blank.validate().unwrap_err().to_string().contains("tuning group must be a nonempty name"));
    let general = NetRoutingRule { tuning_group: Some("lanes".to_owned()), ..rule("A") };
    assert!(general.validate().unwrap_err().to_string().contains("tuning group requires a critical routing kind"));
    let board = Group::default().build();
    // Derived boards may keep the rules without the groups (CMP117 checks the source).
    let mut ungrouped = board.clone();
    ungrouped.match_groups.clear();
    copper_pcb::physicalize::revalidate(&mut ungrouped).unwrap();
    assert!(ungrouped.match_groups.is_empty());
    let mut split = board.clone();
    let names = |nets: &[&str]| nets.iter().map(|net| (*net).to_owned()).collect::<Vec<_>>();
    split.match_groups = vec![
        NetMatchGroup { id: "a".to_owned(), nets: names(&LANES[..2]), max_skew_nm: 1 },
        NetMatchGroup { id: "b".to_owned(), nets: names(&LANES[2..]), max_skew_nm: 1 },
    ];
    let error = copper_pcb::physicalize::revalidate(&mut split).unwrap_err().to_string();
    assert!(error.contains("tuning group 'bundle' spans length-match groups 'a' and 'b'"), "{error}");
    let mut disagree = board;
    for rule in &mut disagree.net_routing_rules {
        if rule.net == "LANE_A_N" {
            rule.tuning_group = None;
        }
    }
    let error = copper_pcb::physicalize::revalidate(&mut disagree).unwrap_err().to_string();
    assert!(error.contains("must agree on tuning_group"), "{error}");
}

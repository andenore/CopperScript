use super::*;
use crate::board::parse_board;

const PROJECT: &str = r#"{"board": {"design_settings": {"rules": {"min_clearance": 0.2,
  "min_copper_edge_clearance": 0.5, "min_hole_clearance": 0.25, "min_hole_to_hole": 0.25,
  "min_track_width": 0.15, "min_via_diameter": 0.4, "min_via_annular_width": 0.1,
  "min_through_hole_diameter": 0.2}}},
  "net_settings": {"classes": [{"name": "Default", "clearance": 0.2}]}}"#;

/// A 20 x 20 mm two-layer board: GND pads at (5, 5) and a through-hole GND
/// pad at (15, 15), SIG pads at (5, 10) and (15, 10) joined by a track, a
/// GND via, and (with `zone`) a GND pour on both layers.
fn text(extra: &str, zone: bool) -> String {
    let pour = if zone {
        "(zone (net \"GND\") (layers \"F.Cu\" \"B.Cu\") (uuid \"a\") (name \"gnd\") (connect_pads yes (clearance 0.2))
    (min_thickness 0.25) (filled_areas_thickness no)
    (fill yes (thermal_gap 0.4) (thermal_bridge_width 0.4) (island_removal_mode 0))
    (polygon (pts (xy -1 -1) (xy 21 -1) (xy 21 21) (xy -1 21))))"
    } else {
        ""
    };
    format!(
        "(kicad_pcb (version 20260206)
  (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (25 \"Edge.Cuts\" user))
  (footprint \"L:P\" (layer \"F.Cu\") (at 5 5)
    (property \"Reference\" \"P1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (net \"GND\")))
  (footprint \"L:T\" (layer \"F.Cu\") (at 15 15)
    (property \"Reference\" \"T1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" thru_hole circle (at 0 0) (size 2 2) (drill 1) (layers \"*.Cu\") (net \"GND\")))
  (footprint \"L:R\" (layer \"F.Cu\") (at 10 10)
    (property \"Reference\" \"R1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd roundrect (at -5 0) (size 1 0.8) (roundrect_rratio 0.25) (layers \"F.Cu\") (net \"SIG\"))
    (pad \"2\" smd roundrect (at 5 0) (size 1 0.8) (roundrect_rratio 0.25) (layers \"F.Cu\") (net \"SIG\")))
  (gr_rect (start 0 0) (end 20 20) (stroke (width 0.1) (type default)) (fill no) (layer \"Edge.Cuts\"))
  (segment (start 5 10) (end 15 10) (width 0.3) (layer \"F.Cu\") (net \"SIG\"))
  (via (at 10 15) (size 0.6) (drill 0.3) (layers \"F.Cu\" \"B.Cu\") (net \"GND\"))
  {extra}
  {pour})"
    )
}

fn rules() -> Rules {
    Rules::load(PROJECT, None).unwrap()
}

fn kinds(report: &Report) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> =
        report.violations.iter().chain(&report.unconnected).map(|violation| violation.kind).collect();
    kinds.sort_unstable();
    kinds
}

fn checked(extra: &str) -> Report {
    check(&parse_board(&text(extra, false), "test").unwrap(), &rules()).unwrap()
}

#[test]
fn a_native_fill_passes() {
    let source = text("", true);
    let board = parse_board(&source, "test").unwrap();
    let fills = crate::fill::fill_zones(&board, &rules()).unwrap();
    let written = crate::pcb_write::write_fills(&source, &board, &fills).unwrap();
    let filled = parse_board(&written, "test").unwrap();
    let report = check(&filled, &rules()).unwrap();
    assert!(report.passed(), "{:#?}", report);
    assert!(report.violations.is_empty(), "{:#?}", report.violations);
    // Without the pour the GND pads are apart and the via joins nothing.
    let bare = checked("");
    assert_eq!(kinds(&bare), ["unconnected_items", "unconnected_items", "via_dangling"], "{bare:#?}");
}

#[test]
fn clearance_shorts_and_dangling_tracks() {
    // 0.4 mm apart centre to centre, 0.3 wide: a 0.1 mm gap to SIG.
    let near = checked("(segment (start 6 10.4) (end 14 10.4) (width 0.3) (layer \"F.Cu\") (net \"OTHER\"))");
    let clearance: Vec<&Violation> = near.violations.iter().filter(|v| v.kind == "clearance").collect();
    assert_eq!(clearance.len(), 1, "{near:#?}");
    assert!(clearance[0].description.contains("clearance 0.1000 mm"), "{}", clearance[0].description);
    // One finding per dangling track, as KiCad reports it.
    assert_eq!(near.violations.iter().filter(|v| v.kind == "track_dangling").count(), 1);
    let short = checked("(segment (start 10 9) (end 10 11) (width 0.2) (layer \"F.Cu\") (net \"OTHER\"))");
    assert!(short.violations.iter().any(|v| v.kind == "clearance" && v.description.contains("overlap")), "{short:#?}");
    // A gap within the 0.5 um epsilon passes: 0.2 mm minus 0.4 um.
    let close =
        checked("(segment (start 6 10.4995996) (end 14 10.4995996) (width 0.3) (layer \"F.Cu\") (net \"OTHER\"))");
    assert!(!kinds(&close).contains(&"clearance"), "{close:#?}");
}

#[test]
fn a_local_pad_clearance_is_checked_beyond_the_rule_reach() {
    // The OTHER pad's edge is 0.7 mm from the SIG track's: farther than every
    // board rule (the widest is the 0.5 mm edge clearance) but inside the
    // pad's own 1 mm clearance.
    let pad = "(footprint \"L:Q\" (layer \"F.Cu\") (at 10 11.35)
    (property \"Reference\" \"Q1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (net \"OTHER\") (clearance 1)))";
    let report = checked(pad);
    let clearance: Vec<&Violation> = report.violations.iter().filter(|v| v.kind == "clearance").collect();
    assert_eq!(clearance.len(), 1, "{report:#?}");
    assert!(clearance[0].description.contains("required 1.0000 mm"), "{}", clearance[0].description);
}

#[test]
fn opens_edges_holes_and_sizes() {
    // Without the SIG track, R1's pads are unconnected.
    let source =
        text("", false).replace("(segment (start 5 10) (end 15 10) (width 0.3) (layer \"F.Cu\") (net \"SIG\"))", "");
    let open = check(&parse_board(&source, "test").unwrap(), &rules()).unwrap();
    assert_eq!(open.unconnected.len(), 3, "{open:#?}");
    assert!(open.unconnected.iter().any(|v| v.description.contains("R1.")), "{open:#?}");
    let edge = checked("(segment (start 3 0.4) (end 8 0.4) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))");
    assert!(kinds(&edge).contains(&"copper_edge_clearance"), "{edge:#?}");
    let outside = checked("(segment (start 25 5) (end 26 5) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))");
    assert!(outside.violations.iter().any(|v| v.description.contains("outside")), "{outside:#?}");
    let ring = checked("(via (at 3 15) (size 0.45) (drill 0.35) (layers \"F.Cu\" \"B.Cu\") (net \"GND\"))");
    assert!(kinds(&ring).contains(&"annular_width"), "{ring:#?}");
    let thin = checked("(segment (start 5 10) (end 5 12) (width 0.1) (layer \"F.Cu\") (net \"SIG\"))");
    assert!(kinds(&thin).contains(&"track_width"), "{thin:#?}");
    // A via whose hole is 0.2 mm from the through-hole pad's hole.
    let holes = checked("(via (at 15.8 15) (size 0.4) (drill 0.2) (layers \"F.Cu\" \"B.Cu\") (net \"GND\"))");
    assert!(kinds(&holes).contains(&"hole_to_hole"), "{holes:#?}");
    // A non-plated hole 0.15 mm from the SIG track's edge.
    let npth = checked(
        "(footprint \"L:H\" (layer \"F.Cu\") (at 10 10.85)
    (property \"Reference\" \"H1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"\" np_thru_hole circle (at 0 0) (size 1.1 1.1) (drill 1.1) (layers \"*.Cu\")))",
    );
    assert!(kinds(&npth).contains(&"hole_clearance"), "{npth:#?}");
}

#[test]
fn tightening_custom_rules_are_refused_and_others_listed() {
    let board = parse_board(&text("", false), "test").unwrap();
    let width = "(rule \"W\" (constraint track_width (min 0.3mm)))";
    let refused = check(&board, &Rules::load(PROJECT, Some(width)).unwrap()).unwrap_err();
    assert!(refused.contains("track_width"), "{refused}");
    let courtyard = "(rule \"C\" (constraint courtyard_clearance (min 0.3mm)))";
    let report = check(&board, &Rules::load(PROJECT, Some(courtyard)).unwrap()).unwrap();
    assert!(report.not_checked.iter().any(|text| text.contains("courtyard_clearance")));
}

#[test]
fn jumper_pad_groups_join_pads_and_short_stubs_dangle() {
    // R1's pads, without the SIG track, joined inside the part.
    let source = text("", false)
        .replace("(segment (start 5 10) (end 15 10) (width 0.3) (layer \"F.Cu\") (net \"SIG\"))", "")
        .replace(
            "(footprint \"L:R\" (layer \"F.Cu\") (at 10 10)",
            "(footprint \"L:R\" (layer \"F.Cu\") (at 10 10) (jumper_pad_groups (\"1\" \"2\"))",
        );
    let joined = check(&parse_board(&source, "test").unwrap(), &rules()).unwrap();
    assert!(!joined.unconnected.iter().any(|v| v.description.contains("[SIG]")), "{joined:#?}");
    // A 35 um stub between two tracks that both sit nearer its far end.
    let stub = checked(
        "(segment (start 8 5) (end 8 5.75) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))
  (segment (start 8 5.775) (end 7.975 5.75) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))
  (segment (start 7.975 5.75) (end 7 5.75) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))",
    );
    let dangling: Vec<&Violation> = stub.violations.iter().filter(|v| v.kind == "track_dangling").collect();
    assert!(dangling.iter().any(|v| (v.at.0 - 8e6).abs() < 1.0 && (v.at.1 - 5.775e6).abs() < 1.0), "{dangling:#?}");
}

#[test]
fn off_centre_vias_isolated_fills_and_uuids() {
    // A SIG track ending inside the GND via's pad, 0.1 mm off its centre,
    // is a GND track there; the via has no centred track.
    // KiCad 10 ignores the check unless the project enables it.
    let track = "(segment (start 10.1 15) (end 12 15) (width 0.2) (layer \"F.Cu\") (net \"GND\") (uuid \"t-1\"))";
    assert!(!kinds(&checked(track)).contains(&"track_not_centered_on_via"));
    let enabled = PROJECT.replace(
        "\"min_through_hole_diameter\": 0.2}",
        "\"min_through_hole_diameter\": 0.2}, \"rule_severities\": {\"track_not_centered_on_via\": \"warning\"}",
    );
    let enabled_rules = Rules::load(&enabled, None).unwrap();
    let check_with = |extra: &str| check(&parse_board(&text(extra, false), "test").unwrap(), &enabled_rules).unwrap();
    let off = check_with(track);
    let found: Vec<&Violation> = off.violations.iter().filter(|v| v.kind == "track_not_centered_on_via").collect();
    assert_eq!(found.len(), 1, "{off:#?}");
    assert_eq!(found[0].uuids, ["t-1"], "{found:#?}");
    let centred = check_with("(segment (start 10 15) (end 12 15) (width 0.2) (layer \"F.Cu\") (net \"GND\"))");
    assert!(!kinds(&centred).contains(&"track_not_centered_on_via"), "{centred:#?}");
    // A kept fill fragment with no pad (island removal off) is isolated copper.
    let source = text("", true)
        .replace("(island_removal_mode 0)", "(island_removal_mode 1)")
        .replace("(layers \"F.Cu\" \"B.Cu\") (uuid \"a\")", "(layers \"F.Cu\" \"B.Cu\") (uuid \"zone-a\")")
        + "";
    let wall = "(segment (start 0 18) (end 20 18) (width 0.3) (layer \"F.Cu\") (net \"WALL\"))";
    let source = source.replace(
        "(via (at 10 15)",
        &format!(
            "{wall}
  (via (at 10 15)"
        ),
    );
    let board = parse_board(&source, "test").unwrap();
    let fills = crate::fill::fill_zones(&board, &rules()).unwrap();
    let written = crate::pcb_write::write_fills(&source, &board, &fills).unwrap();
    let report = check(&parse_board(&written, "test").unwrap(), &rules()).unwrap();
    let isolated: Vec<&Violation> = report.violations.iter().filter(|v| v.kind == "isolated_copper").collect();
    assert_eq!(isolated.len(), 1, "{report:#?}");
    assert_eq!(isolated[0].uuids, ["zone-a"]);
}

#[test]
fn opens_name_the_nearest_pads_of_a_spanning_tree() {
    // Three unrouted SIG pads in a row: KiCad's ratsnest links each to its
    // neighbour, never the two ends.
    let source = text(
        "(footprint \"L:Q\" (layer \"F.Cu\") (at 10 3)
    (property \"Reference\" \"Q1\" (at 0 0) (layer \"F.Fab\") (hide yes) (effects (font (size 1 1))))
    (pad \"1\" smd rect (at 0 0) (size 0.5 0.5) (layers \"F.Cu\") (net \"ROW\") (uuid \"q1\"))
    (pad \"2\" smd rect (at 2 0) (size 0.5 0.5) (layers \"F.Cu\") (net \"ROW\") (uuid \"q2\"))
    (pad \"3\" smd rect (at 5 0) (size 0.5 0.5) (layers \"F.Cu\") (net \"ROW\") (uuid \"q3\")))",
        false,
    );
    let report = check(&parse_board(&source, "test").unwrap(), &rules()).unwrap();
    let mut links: Vec<Vec<String>> = report
        .unconnected
        .iter()
        .filter(|v| v.description.contains("[ROW]"))
        .map(|v| {
            let mut uuids = v.uuids.clone();
            uuids.sort();
            uuids
        })
        .collect();
    links.sort();
    assert_eq!(links, [vec!["q1", "q2"], vec!["q2", "q3"]], "{report:#?}");
}

#[test]
fn the_json_report_states_its_coordinate_unit() {
    let board = parse_board(&text("", false), "test").unwrap();
    let report = check(&board, &Rules::load(PROJECT, None).unwrap()).unwrap();
    let json = report.to_json("test", &[]);
    assert_eq!(json.get("coordinate_units"), Some(&copper_json::Value::from("mm")));
    assert!(json.get("violations").is_some() && json.get("unconnected_items").is_some());
}

#[test]
fn expected_open_ends_excuse_only_the_named_track_end() {
    let board = "(segment (start 8 5) (end 8 7) (width 0.2) (layer \"F.Cu\") (net \"SIG\"))";
    let dangling = |report: &Report| report.violations.iter().filter(|v| v.kind == "track_dangling").count();
    let end = |x: f64, y: f64, net: &str, layer: &str| vec![(x * 1e6, y * 1e6, net.to_owned(), layer.to_owned())];
    let before = dangling(&checked(board));
    assert!(before >= 1);
    // Another net, another layer or another point excuses nothing.
    for ends in [end(8.0, 5.0, "GND", "F.Cu"), end(8.0, 5.0, "SIG", "B.Cu"), end(8.0, 5.1, "SIG", "F.Cu")] {
        let mut report = checked(board);
        assert_eq!(report.excuse_open_ends(&ends), 0);
        assert_eq!(dangling(&report), before);
    }
    let mut report = checked(board);
    assert_eq!(report.excuse_open_ends(&end(8.0, 5.0, "SIG", "F.Cu")), 1);
    assert_eq!(dangling(&report), before - 1);
}

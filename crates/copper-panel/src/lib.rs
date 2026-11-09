//! Fabrication panels: already-routed KiCad boards composed into one panel
//! board with rails, mouse-bite tabs, fiducials and tooling holes, checked
//! against each board's own intent (see docs/panelization-plan.md).

pub mod checks;
pub mod compose;
pub mod frame;
pub mod intent;
pub mod rules;
pub mod sexpr;

use std::path::Path;

use copper_json::Value as Json;
use copper_lang::panel::PanelSpec;
use indexmap::IndexMap;

use crate::compose::{BoardInput, Point, compose, mm, outline_rings, pose_for};
use crate::intent::BoardIntent;
use crate::sexpr::{Node, parse, print};

/// One board's routed files, as text.
pub struct BoardFiles {
    pub pcb: String,
    pub pro: String,
    pub dru: String,
}

/// The composed panel.
pub struct Panel {
    pub pcb: String,
    pub pro: String,
    pub dru: String,
    pub report: Json,
    /// The preset's fab checks (empty without a preset).
    pub checks: Vec<checks::Check>,
}

fn json_point(p: Point) -> Json {
    Json::Array(vec![Json::from(mm(p.0).as_str()), Json::from(mm(p.1).as_str())])
}

fn object(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn keep_clear_json(intents: &IndexMap<String, BoardIntent>) -> Json {
    let mut rows = Vec::new();
    for (board, intent) in intents {
        for k in &intent.keep_clear {
            rows.push(object(vec![
                ("board", Json::from(board.as_str())),
                ("side", Json::from(k.side.as_str())),
                ("from_mm", Json::from(mm(k.start).as_str())),
                ("to_mm", Json::from(mm(k.end).as_str())),
                ("reason", Json::from(k.reason.as_str())),
            ]));
        }
    }
    Json::Array(rows)
}

fn overhangs_json(spec: &PanelSpec, intents: &IndexMap<String, BoardIntent>) -> Json {
    let mut rows = Vec::new();
    for (board, intent) in intents {
        for o in &intent.overhangs {
            let after = spec.assemble_after_depanel.iter().any(|e| *e == format!("{board}.{}", o.reference));
            rows.push(object(vec![
                ("board", Json::from(board.as_str())),
                ("component", Json::from(o.reference.as_str())),
                ("side", Json::from(o.side.as_str())),
                ("from_mm", Json::from(mm(o.start).as_str())),
                ("to_mm", Json::from(mm(o.end).as_str())),
                ("distance_mm", Json::from(mm(o.distance).as_str())),
                ("assemble_after_depanel", Json::Bool(after)),
            ]));
        }
    }
    Json::Array(rows)
}

/// Read each board's routed `.kicad_pcb` and the `.kicad_pro` and
/// `.kicad_dru` beside it, paths relative to `base` (the panel file's folder).
pub fn read_boards(spec: &PanelSpec, base: &Path) -> Result<Vec<BoardFiles>, String> {
    spec.boards
        .iter()
        .map(|board| {
            let pcb = base.join(&board.pcb);
            let read = |path: &Path| {
                std::fs::read_to_string(path)
                    .map_err(|e| format!("board {}: cannot read {}: {e}", board.name, path.display()))
            };
            Ok(BoardFiles {
                pcb: read(&pcb)?,
                pro: read(&pcb.with_extension("kicad_pro"))?,
                dru: read(&pcb.with_extension("kicad_dru"))?,
            })
        })
        .collect()
}

/// Compose the panel from its boards' files and intents, in `spec.boards`
/// order (an empty intent skips that board's intent checks).
pub fn panelize(
    spec: &PanelSpec,
    files: &[BoardFiles],
    intents: &[BoardIntent],
    file_stem: &str,
) -> Result<Panel, String> {
    let mut inputs = Vec::new();
    let mut local = IndexMap::new();
    let mut poses = Vec::new();
    let mut intents = intents.to_vec();
    for ((board, file), intent) in spec.boards.iter().zip(files).zip(intents.iter_mut()) {
        let routed = copper_cam::board::parse_board(&file.pcb, &board.pcb)?;
        intent::verify_source(&board.name, intent, &routed)?;
        // Board rules are scoped by net and reference prefixes: copper on no
        // net and in no footprint carries neither, so a conditional rule
        // could not be kept to its board.
        let unnetted = routed.tracks.iter().filter(|t| t.net.as_deref().is_none_or(str::is_empty)).count()
            + routed.vias.iter().filter(|v| v.net.as_deref().is_none_or(str::is_empty)).count()
            + routed
                .zones
                .iter()
                .filter(|z| z.keepout.is_none() && !z.in_footprint && z.net.as_deref().is_none_or(str::is_empty))
                .count()
            // Copper graphics (a footprint's included: the native check gives
            // them no reference) and copper text carry no net either.
            + routed
                .drawings
                .iter()
                .filter(|d| {
                    matches!(d.layer, copper_cam::board::Layer::Copper(_)) && d.net.as_deref().is_none_or(str::is_empty)
                })
                .count()
            + routed.texts.iter().filter(|t| matches!(t.layer, copper_cam::board::Layer::Copper(_))).count();
        if unnetted > 0 && rules::has_conditional(&file.dru, &format!("board {} design rules", board.name))? {
            return Err(format!(
                "board {}: {unnetted} copper item(s) on no net, and conditional design rules that the panel can only \
                 keep to the board by net and reference; give that copper a net",
                board.name
            ));
        }
        let tree = parse(&file.pcb, &board.pcb)?;
        let rings = outline_rings(&tree, &format!("board {}", board.name))?;
        intent.keep_clear.extend(intent::routed_keep_clear(&routed, &rings));
        intent.declared_overhangs = intent.overhangs.clone();
        intent::merge_overhangs(&mut intent.overhangs, intent::routed_overhangs(&tree, &rings));
        intent.courtyards = intent::routed_courtyards(&tree);
        poses.push(pose_for(&rings, board.quarter_turns, (board.at[0], board.at[1])));
        local.insert(board.name.clone(), rings);
        inputs.push(BoardInput { instance: board.name.clone(), tree });
    }
    let composition = compose(&inputs, &poses)?;
    // Boards keep at least the milled gap between them, measured edge to
    // edge in every direction (a diagonal neighbour included).
    let names: Vec<&String> = composition.outlines.keys().collect();
    let mut min_gap = spec.spacing;
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let gap = intent::ring_distance(&composition.outlines[*a], &composition.outlines[*b]);
            if gap < spec.spacing {
                return Err(format!(
                    "boards {a} and {b} are {} mm apart, closer than the panel spacing ({} mm)",
                    mm(gap),
                    mm(spec.spacing)
                ));
            }
            min_gap = min_gap.min(gap);
        }
    }
    let frame = frame::build(spec, &local, &composition.outlines, &composition.poses)?;
    let intent_map: IndexMap<String, BoardIntent> =
        spec.boards.iter().zip(&intents).map(|(b, i)| (b.name.clone(), i.clone())).collect();
    let open_ends =
        intent::check(spec, &intent_map, &local, &composition.outlines, &composition.poses, &frame.tabs, &frame.rails)?;
    let mut tree = composition.tree;
    if let Node::List(items) = &mut tree {
        items.extend(frame.items);
    }
    let named = |f: fn(&BoardFiles) -> &String| -> Vec<(String, String)> {
        spec.boards.iter().zip(files).map(|(b, file)| (b.name.clone(), f(file).clone())).collect()
    };
    let dru = rules::merge_dru(&named(|f| &f.dru))?;
    let pro = rules::merge_pro(&named(|f| &f.pro), &format!("{file_stem}.kicad_pro"))?;
    let (x0, y0, x1, y1) = frame.bounds;
    let fab_checks = checks::run(spec, frame.bounds, &frame.tabs, min_gap);
    let report = object(vec![
        ("schema", Json::from("copperscript-panel/v0.1")),
        ("panel", Json::from(spec.name.as_str())),
        ("size_mm", json_point((x1 - x0, y1 - y0))),
        ("min_gap_mm", Json::from(mm(min_gap).as_str())),
        ("config", checks::config_json(spec)),
        ("checks", checks::checks_json(spec, &fab_checks)),
        (
            "boards",
            Json::Array(
                spec.boards
                    .iter()
                    .map(|b| {
                        object(vec![
                            ("name", Json::from(b.name.as_str())),
                            ("source", Json::from(b.source.as_str())),
                            ("pcb", Json::from(b.pcb.as_str())),
                            ("at_mm", json_point((b.at[0], b.at[1]))),
                            ("rotation_deg", Json::from(i64::from(b.quarter_turns) * 90)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "tabs",
            Json::Array(
                frame
                    .tabs
                    .iter()
                    .map(|t| {
                        object(vec![
                            ("name", Json::from(t.name.as_str())),
                            ("board", Json::from(t.board.as_str())),
                            ("side", Json::from(t.side.as_str())),
                            ("at_mm", Json::from(mm(t.at).as_str())),
                            ("width_mm", Json::from(mm(t.width).as_str())),
                            ("across", Json::from(t.across.as_deref().unwrap_or("rail"))),
                            ("length_mm", Json::from(mm(t.length).as_str())),
                            ("mouse_bites", Json::from(t.mouse_bites as i64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("rails", Json::from(frame.rails.len() as i64)),
        ("fiducials", Json::from(i64::from(spec.fiducials))),
        ("tooling_holes", Json::from(if spec.tooling_hole.is_some() { 4_i64 } else { 0 })),
        ("keep_clear", keep_clear_json(&intent_map)),
        ("overhangs", overhangs_json(spec, &intent_map)),
        (
            "expected_open_ends",
            Json::Array(
                open_ends
                    .iter()
                    .map(|e| {
                        object(vec![
                            ("at_mm", json_point(e.at)),
                            ("net", Json::from(e.net.as_str())),
                            ("layer", Json::from(e.layer.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]);
    Ok(Panel { pcb: print(&tree), pro, dru, report, checks: fab_checks })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r#"(kicad_pcb (version 20240108) (generator "copperscript")
  (general (thickness 1.6)) (paper "A4")
  (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
  (setup (pad_to_mask_clearance 0))
  (net 0 "") (net 1 "GND")
  (via (at 5 3) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "33333333-3333-3333-3333-333333333333"))
  (gr_line (start 0 0) (end 10 0) (layer "Edge.Cuts") (uuid "a"))
  (gr_line (start 10 0) (end 10 6) (layer "Edge.Cuts") (uuid "b"))
  (gr_line (start 10 6) (end 0 6) (layer "Edge.Cuts") (uuid "c"))
  (gr_line (start 0 6) (end 0 0) (layer "Edge.Cuts") (uuid "d")))"#;
    const PRO: &str = r#"{"board": {"design_settings": {}}, "meta": {"filename": "b.kicad_pro", "version": 3}, "net_settings": {"classes": []}}"#;
    const DRU: &str = "(version 1)\n";

    fn spec(extra: &str) -> PanelSpec {
        let text = format!(
            r#"panel Demo {{
                spacing = 2 mm;
                rails = {{ top = 5 mm; bottom = 5 mm; }};
                mouse_bite = {{ drill = 0.5 mm; pitch = 0.8 mm; inset = 0.25 mm; }};
                fiducials = 3;
                tooling_hole = 1.152 mm;
                board A {{ source = "a.copper"; pcb = "a.kicad_pcb"; at = (0 mm, 0 mm); }}
                board B {{ source = "b.copper"; pcb = "b.kicad_pcb"; at = (12 mm, 0 mm); }}
                tab a_top {{ board = A; side = top; at = 5 mm; }}
                tab a_bottom {{ board = A; side = bottom; at = 5 mm; }}
                tab a_right {{ board = A; side = right; at = 3 mm; width = 2 mm; }}
                tab b_top {{ board = B; side = top; at = 5 mm; }}
                {extra}
            }}"#
        );
        copper_lang::panel::lower_panel(&copper_lang::parse(&text, "panel.copper").expect("parsed")).expect("lowered")
    }

    fn none() -> Vec<BoardIntent> {
        vec![BoardIntent::default(), BoardIntent::default()]
    }

    fn files() -> Vec<BoardFiles> {
        (0..2).map(|_| BoardFiles { pcb: BOARD.to_owned(), pro: PRO.to_owned(), dru: DRU.to_owned() }).collect()
    }

    #[test]
    fn two_boards_between_rails_make_one_outline_with_tabs_and_holes() {
        let panel = panelize(&spec(""), &files(), &none(), "demo").expect("a panel");
        let board = copper_cam::board::parse_board(&panel.pcb, "demo").expect("copper-cam reads the panel");
        // Two vias, 3 fiducials, 4 tooling holes, one mouse-bite footprint per tab.
        assert_eq!(board.vias.len(), 2);
        let refs: Vec<&str> = board.footprints.iter().map(|f| f.reference.as_str()).collect();
        assert!(refs.contains(&"FID3") && refs.contains(&"TH4") && refs.contains(&"TAB-a_right"), "{refs:?}");
        // a_right bridges to board B, so it has bites on both ends; the others end on a rail.
        let tabs = match &panel.report {
            Json::Object(o) => o["tabs"].clone(),
            _ => unreachable!(),
        };
        let text = copper_json::Dumps::compact().dumps(&tabs);
        assert!(text.contains(r#""name":"a_right","board":"A","side":"right","at_mm":"3","width_mm":"2","across":"B","length_mm":"2","mouse_bites":4"#), "{text}");
        assert!(text.contains(r#""name":"a_top","board":"A","side":"top","at_mm":"5","width_mm":"3","across":"rail","length_mm":"2","mouse_bites":4"#), "{text}");
        // The outline is one panel contour with the milled gaps as holes.
        let outline: Vec<_> = board.drawings.iter().filter(|d| d.layer == copper_cam::board::Layer::EdgeCuts).collect();
        assert!(outline.len() >= 2, "{}", outline.len());
        // Size: 22 mm of boards and gap wide; 6 mm board + 2 x (2 mm gap + 5 mm rail) tall.
        assert!(copper_json::Dumps::compact().dumps(&panel.report).contains(r#""size_mm":["22","20"]"#));
    }

    #[test]
    fn tabs_that_bridge_nothing_or_overlap_fail() {
        let error = panelize(&spec("tab a_left { board = A; side = left; at = 3 mm; }"), &files(), &none(), "demo")
            .err()
            .expect("refused");
        assert!(error.contains("nothing lies across"), "{error}");
        let error = panelize(&spec("tab a_top2 { board = A; side = top; at = 6 mm; }"), &files(), &none(), "demo")
            .err()
            .expect("refused");
        assert!(error.contains("overlap"), "{error}");
        // a_right bridges A to B at A's 2-4 mm; the same bridge declared from B is refused.
        let error = panelize(
            &spec("tab b_left { board = B; side = left; at = 3 mm; width = 2 mm; }"),
            &files(),
            &none(),
            "demo",
        )
        .err()
        .expect("refused");
        assert!(
            error.contains("tabs a_right and b_left overlap") && error.contains("declare each bridge once"),
            "{error}"
        );
        let error = panelize(&spec("tab a_off { board = A; side = top; at = 9.5 mm; }"), &files(), &none(), "demo")
            .err()
            .expect("refused");
        assert!(error.contains("runs off"), "{error}");
    }

    #[test]
    fn board_intent_keeps_tabs_off_antennas_and_bodies_off_rails() {
        use crate::intent::{KeepClear, OpenEnd, Overhang};
        use copper_lang::panel::Side;
        let antenna = BoardIntent {
            keep_clear: vec![KeepClear {
                side: Side::Top,
                start: 4_000_000,
                end: 10_000_000,
                reason: "antenna a3's region".into(),
            }],
            ..BoardIntent::default()
        };
        let error = panelize(&spec(""), &files(), &[antenna, BoardIntent::default()], "demo").err().expect("refused");
        assert!(error.contains("tab a_top on board A's top side") && error.contains("antenna a3's region"), "{error}");
        // B's jack stands 4 mm beyond its bottom side: across the 2 mm gap into the bottom rail.
        let jack = BoardIntent {
            overhangs: vec![Overhang {
                reference: "J1".into(),
                side: Side::Bottom,
                start: 3_000_000,
                end: 7_000_000,
                distance: 4_000_000,
            }],
            open_ends: vec![OpenEnd { at: (9_000_000, 1_000_000), net: "FEED".into(), layer: "F.Cu".into() }],
            ..BoardIntent::default()
        };
        let both = [BoardIntent::default(), jack];
        let error = panelize(&spec(""), &files(), &both, "demo").err().expect("refused");
        assert!(error.contains("J1's body stands 4 mm beyond its bottom side into a rail"), "{error}");
        let mut after = spec("");
        after.assemble_after_depanel = vec!["B.J1".into()];
        let panel = panelize(&after, &files(), &both, "demo").expect("allowed when fitted after depanelization");
        let text = copper_json::Dumps::compact().dumps(&panel.report);
        assert!(text.contains(r#""component":"J1","side":"bottom""#), "{text}");
        assert!(text.contains(r#""assemble_after_depanel":true"#), "{text}");
        // B sits at x 12 mm, so its open end at (9, 1) lands at (21, 1).
        assert!(
            text.contains(r#""expected_open_ends":[{"at_mm":["21","1"],"net":"B/FEED","layer":"F.Cu"}]"#),
            "{text}"
        );
        // A's tab to B lands on B's left side at 1.5-4.5 mm: refused when B keeps that stretch clear.
        let left = BoardIntent {
            keep_clear: vec![KeepClear {
                side: Side::Left,
                start: 2_000_000,
                end: 5_000_000,
                reason: "B's launch".into(),
            }],
            ..BoardIntent::default()
        };
        let error = panelize(&spec(""), &files(), &[BoardIntent::default(), left], "demo").err().expect("refused");
        assert!(error.contains("tab a_right on board B's left side") && error.contains("B's launch"), "{error}");
        // A pcb whose outline differs from its source is refused.
        let wrong = BoardIntent {
            outline: vec![vec![(0, 0), (9_000_000, 0), (9_000_000, 6_000_000), (0, 6_000_000)]],
            ..BoardIntent::default()
        };
        let error = panelize(&spec(""), &files(), &[wrong, BoardIntent::default()], "demo").err().expect("refused");
        assert!(error.contains("outline differs"), "{error}");
    }

    #[test]
    fn a_preset_checks_the_panel_against_the_fab_rules() {
        use crate::checks::Status;
        let plain = panelize(&spec(""), &files(), &none(), "demo").expect("a panel");
        assert!(plain.checks.is_empty(), "no preset, no checks");
        let mut fab = spec("");
        fab.preset = Some("jlcpcb".into());
        let panel = panelize(&fab, &files(), &none(), "demo").expect("checks never refuse a panel");
        let status = |rule: &str| {
            let check = panel.checks.iter().find(|c| c.rule == rule).unwrap_or_else(|| panic!("{rule}"));
            (check.status, check.value.clone())
        };
        assert_eq!(status("min_tab_width"), (Status::Fail, "2 mm (tab a_right)".to_owned()));
        assert_eq!(status("min_mouse_bite_web"), (Status::Pass, "0.3 mm".to_owned()));
        assert_eq!(status("min_mouse_bites_per_tab").0, Status::Warn);
        assert_eq!(status("tooling_hole"), (Status::Warn, "1.152 mm".to_owned()));
        assert_eq!(status("fiducial_offset"), (Status::Warn, "2.5 mm".to_owned()));
        assert_eq!(status("max_designs"), (Status::Pass, "2".to_owned()));
        assert_eq!(status("min_spacing").0, Status::Pass);
        let text = copper_json::Dumps::compact().dumps(&panel.report);
        assert!(text.contains(r#""checks":{"preset":"jlcpcb","designs":2,"fail":1,"warn":3"#), "{text}");
        assert!(text.contains(r#""spacing":{"value":"2 mm","origin":"explicit"}"#), "{text}");
        // Copies of one routed board are one design.
        fab.boards[1].pcb = "./a.kicad_pcb".into();
        fab.boards[0].pcb = "./a.kicad_pcb".into();
        assert_eq!(crate::checks::designs(&fab), 1);
    }

    #[test]
    fn fiducials_follow_the_offset_and_must_fit_their_rail() {
        let mut offset = spec("");
        offset.fiducial_offset = Some(3_850_000);
        let panel = panelize(&offset, &files(), &none(), "demo").expect("a panel");
        // The top rail spans y -7 to -2 mm; 3.85 mm in from the edge is y -3.15 mm.
        assert!(panel.pcb.contains("(at 8 -3.15 0)"), "the first fiducial at 3.85 mm from the top edge");
        offset.fiducial_offset = Some(4_500_000);
        let error = panelize(&offset, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("mask openings off"), "{error}");
    }

    #[test]
    fn a_routed_board_must_match_its_source_beyond_the_outline() {
        use crate::intent::Radiator;
        // BOARD has no footprints and one GND via: a source naming J1 is another board.
        let parts = BoardIntent { references: vec!["J1".into()], ..BoardIntent::default() };
        let error = panelize(&spec(""), &files(), &[parts, BoardIntent::default()], "demo").err().expect("refused");
        assert!(
            error.contains("board A: its routed pcb's footprints differ") && error.contains("missing J1"),
            "{error}"
        );
        // A radiator the routed board does not carry: the source belongs to another board.
        let antenna = BoardIntent {
            radiators: vec![Radiator {
                name: "ant".into(),
                net: "GND".into(),
                layer: "F.Cu".into(),
                path: vec![(1_000_000, 1_000_000), (9_000_000, 1_000_000)],
            }],
            ..BoardIntent::default()
        };
        let error =
            panelize(&spec(""), &files(), &[BoardIntent::default(), antenna.clone()], "demo").err().expect("refused");
        assert!(error.contains("board B: antenna ant's radiator from the source is not on the routed pcb"), "{error}");
        // A pour keep-out on the routed board's edge holds tabs off, whatever the source says.
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
             (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 4 0) (xy 10 0) (xy 10 3) (xy 4 3))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(
            error.contains("tab a_top on board A's top side") && error.contains("keep-out cluster/a/a-edge"),
            "{error}"
        );
        // With the radiator routed as a track it is accepted.
        let routed = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(segment (start 1 1) (end 9 1) (width 0.5) (layer \"F.Cu\") (net 1) (uuid \"44444444-4444-4444-4444-444444444444\"))\n  (gr_line (start 0 0)",
            1,
        );
        let mut both = files();
        both[1].pcb = routed;
        panelize(&spec(""), &both, &[BoardIntent::default(), antenna], "demo").expect("the radiator is on the board");
    }

    #[test]
    fn diagonal_neighbours_keep_the_spacing_too() {
        // B 1 mm right of and 1 mm below A's corner: 1.414 mm apart, under 2 mm.
        let mut diagonal = spec("");
        diagonal.boards[1].at = [11_000_000, 7_000_000];
        diagonal.tabs.retain(|t| t.name != "a_right");
        let error = panelize(&diagonal, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("boards A and B are 1.414213 mm apart"), "{error}");
        // The fab check reads the real gap, not the configured spacing.
        let mut wide = spec("");
        wide.boards[1].at = [13_000_000, 0];
        wide.tabs.retain(|t| t.name != "a_right");
        wide.preset = Some("pcbway".into());
        let panel = panelize(&wide, &files(), &none(), "demo").expect("a panel");
        let gap = panel.checks.iter().find(|c| c.rule == "min_spacing").expect("min_spacing");
        assert_eq!(gap.value, "2 mm", "the rails sit 2 mm away; the boards 3 mm");
    }

    #[test]
    fn a_panel_that_falls_apart_is_refused() {
        let mut loose = spec("");
        loose.tabs.retain(|t| t.name != "a_right" && t.name != "b_top");
        let error = panelize(&loose, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("falls apart into 2 pieces") && error.contains("[board B]"), "{error}");
    }

    #[test]
    fn a_courtyard_beyond_the_edge_keeps_tabs_off_whatever_the_source_says() {
        // J9's courtyard reaches 2 mm below A's bottom side, over a_bottom (3.5-6.5 mm).
        let jack = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 5 5 0) \
             (property \"Reference\" \"J9\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_rect (start -1 -0.5) (end 1 3) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = jack;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(
            error.contains("tab a_bottom on board A's bottom side overlaps J9's body overhang (4-6 mm)"),
            "{error}"
        );
    }

    #[test]
    fn a_rotated_courtyard_counts_by_all_its_corners() {
        // A 2 x 0.4 mm courtyard at 45 deg, centred 0.7 mm above A's bottom
        // side under a_bottom (3.5-6.5 mm): its corners reach about 0.15 mm
        // below the edge, which its start and end corners alone do not show.
        let jack = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 5 5.3 45) \
             (property \"Reference\" \"J8\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_rect (start -1 -0.2) (end 1 0.2) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = jack;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(error.contains("tab a_bottom on board A's bottom side overlaps J8's body overhang"), "{error}");
    }

    #[test]
    fn a_circular_courtyard_counts_the_same_at_any_angle() {
        // A circle centred 2 mm above A's bottom side, 2.004 mm in radius:
        // 4 um beyond the edge over a_bottom (3.5-6.5 mm), turned or not.
        for angle in ["0", "2.5", "45"] {
            let jack = BOARD.replacen(
                "(gr_line (start 0 0)",
                &format!(
                    "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 5 4 {angle}) \
                     (property \"Reference\" \"J7\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
                     (fp_circle (center 0 0) (end 2.004 0) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)"
                ),
                1,
            );
            let mut marked = files();
            marked[0].pcb = jack;
            let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
            assert!(
                error.contains("tab a_bottom on board A's bottom side overlaps J7's body overhang"),
                "{angle}: {error}"
            );
        }
    }

    #[test]
    fn a_keepout_crossing_the_edge_still_holds_tabs_off() {
        // The keep-out reaches 1 mm beyond A's top side over a_top (3.5-6.5 mm).
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
             (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 4 -1) (xy 9 -1) (xy 9 3) (xy 4 3))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(
            error.contains("tab a_top on board A's top side") && error.contains("keep-out cluster/a/a-edge"),
            "{error}"
        );
    }

    #[test]
    fn only_the_courtyard_beyond_the_edge_holds_tabs_off() {
        // A triangle inside A from x 0.5 to 10 crosses the bottom side only
        // from 0.5 to 2.4 mm: a_bottom (3.5-6.5 mm) stays allowed, a tab at
        // 1-2 mm is refused.
        let triangle = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 0 0 0) \
             (property \"Reference\" \"J6\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_poly (pts (xy 0.5 6.5) (xy 10 4) (xy 0.5 4)) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = triangle;
        panelize(&spec(""), &marked, &none(), "demo").expect("a_bottom is clear of the overhang");
        let error = panelize(
            &spec("tab a_low { board = A; side = bottom; at = 1.5 mm; width = 1 mm; }"),
            &marked,
            &none(),
            "demo",
        )
        .err()
        .expect("refused");
        assert!(
            error.contains("tab a_low on board A's bottom side overlaps J6's body overhang (0.5-2.4 mm)"),
            "{error}"
        );
    }

    #[test]
    fn a_curved_keepout_reaching_the_edge_holds_tabs_off() {
        // A 40 mm radius arc whose apex stands 20 um beyond A's top side at
        // x = 5, between the samples of a coarse approximation.
        let (cx, cy, r) = (5.0_f64, 39.98_f64, 40.0_f64);
        let at = |deg: f64| {
            let t = deg.to_radians();
            format!("{:.6} {:.6}", cx + r * t.cos(), cy + r * t.sin())
        };
        let (s, m, e) = (at(185.0), at(267.5), at(350.0));
        let x = |p: &str| p.split(' ').next().unwrap_or("").to_owned();
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            &format!(
                "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
                 (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
                 (polygon (pts (arc (start {s}) (mid {m}) (end {e})) (xy {} 50) (xy {} 50))))\n  (gr_line (start 0 0)",
                x(&e),
                x(&s)
            ),
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(
            error.contains("tab a_top on board A's top side") && error.contains("keep-out cluster/a/a-edge"),
            "{error}"
        );
    }

    #[test]
    fn separate_overhangs_leave_the_stretch_between_them_to_tabs() {
        // A comb courtyard crossing A's bottom side at 1-2 mm and 8-9 mm.
        let comb = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 0 0 0) \
             (property \"Reference\" \"J5\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_poly (pts (xy 1 4) (xy 1 6.5) (xy 2 6.5) (xy 2 5) (xy 8 5) (xy 8 6.5) (xy 9 6.5) (xy 9 4)) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = comb;
        let panel =
            panelize(&spec(""), &marked, &none(), "demo").expect("a_bottom (3.5-6.5 mm) lies between the overhangs");
        let text = copper_json::Dumps::compact().dumps(&panel.report);
        assert!(text.contains(r#""component":"J5","side":"bottom","from_mm":"1","to_mm":"2""#), "{text}");
        assert!(text.contains(r#""component":"J5","side":"bottom","from_mm":"8","to_mm":"9""#), "{text}");
        let error = panelize(
            &spec("tab a_far { board = A; side = bottom; at = 8.5 mm; width = 1 mm; }"),
            &marked,
            &none(),
            "demo",
        )
        .err()
        .expect("refused");
        assert!(error.contains("overlaps J5's body overhang (8-9 mm)"), "{error}");
    }

    #[test]
    fn a_hole_in_a_keepout_leaves_its_stretch_to_tabs() {
        // The keep-out crosses A's top side from 1 to 9.5 mm, with a hole from
        // 2 to 8 mm through the edge: a_top (3.5-6.5 mm) lies in the hole,
        // clear of the 1 mm margins (keep-clear 0-3 and 7-10.5 mm).
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
             (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 1 -1) (xy 9.5 -1) (xy 9.5 4) (xy 1 4))) (polygon (pts (xy 2 -0.5) (xy 8 -0.5) (xy 8 2) (xy 2 2))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        panelize(&spec(""), &marked, &none(), "demo").expect("a_top lies in the keep-out's hole");
        let error = panelize(
            &spec("tab a_top_l { board = A; side = top; at = 1.5 mm; width = 1 mm; }"),
            &marked,
            &none(),
            "demo",
        )
        .err()
        .expect("refused");
        assert!(
            error.contains("tab a_top_l on board A's top side") && error.contains("keep-out cluster/a/a-edge"),
            "{error}"
        );
    }

    #[test]
    fn a_courtyard_touching_the_edge_from_outside_holds_tabs_off() {
        // A 0.5 mm circle outside A's right side, touching it at y = 3 mm,
        // under a_right (2-4 mm).
        let jack = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 10.5 3 0) \
             (property \"Reference\" \"J4\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_circle (center 0 0) (end 0.5 0) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = jack;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(
            error.contains("tab a_right on board A's right side overlaps J4's body overhang (2.5-3.5 mm)"),
            "{error}"
        );
    }

    #[test]
    fn a_courtyard_turning_past_a_corner_stays_off_the_rail() {
        // An L courtyard leaves A's right side at y 0.5-1 mm and turns up past
        // the top-right corner into the top rail (y -7 to -2 mm), between
        // boards A and B. Its overhang stretch on the right side (0-1 mm) is
        // clear of a_right, but its shape reaches the rail.
        let corner = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 0 0 0) \
             (property \"Reference\" \"J3\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_line (start 9 1) (end 11 1) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888881\")) \
             (fp_line (start 11 1) (end 11 -3) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888882\")) \
             (fp_line (start 11 -3) (end 10.5 -3) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888883\")) \
             (fp_line (start 10.5 -3) (end 10.5 0.5) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888884\")) \
             (fp_line (start 10.5 0.5) (end 9 0.5) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888885\")) \
             (fp_line (start 9 0.5) (end 9 1) (stroke (width 0.05) (type default)) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888886\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = corner;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(error.contains("board A: J3's courtyard (F.CrtYd) reaches into a rail"), "{error}");
        // Fitted after depanelization, it may.
        let mut after = spec("");
        after.assemble_after_depanel = vec!["A.J3".into()];
        panelize(&after, &marked, &none(), "demo").expect("allowed when fitted after depanelization");
    }

    #[test]
    fn front_and_back_courtyards_do_not_cancel_out() {
        // The same 4 mm circle on both courtyard layers, reaching 1.5 mm into
        // the top rail (y -7 to -2 mm). a_top is left out so the rail check
        // decides; A still joins B and the bottom rail.
        let both = |layers: &[&str]| {
            let circles: String = layers
                .iter()
                .enumerate()
                .map(|(k, layer)| {
                    format!("(fp_circle (center 0 0) (end 4 0) (stroke (width 0.05) (type default)) (fill none) (layer \"{layer}\") (uuid \"88888888-8888-8888-8888-88888888888{k}\")) ")
                })
                .collect();
            BOARD.replacen(
                "(gr_line (start 0 0)",
                &format!(
                    "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 5 0.5 0) \
                     (property \"Reference\" \"J2\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
                     {circles})\n  (gr_line (start 0 0)"
                ),
                1,
            )
        };
        let mut no_top = spec("");
        no_top.tabs.retain(|t| t.name != "a_top");
        for layers in [&["F.CrtYd"][..], &["B.CrtYd"][..], &["F.CrtYd", "B.CrtYd"][..]] {
            let mut marked = files();
            marked[0].pcb = both(layers);
            let error = panelize(&no_top, &marked, &none(), "demo").err().expect("refused");
            assert!(error.contains("J2's courtyard") && error.contains("reaches into a rail"), "{layers:?}: {error}");
        }
    }

    #[test]
    fn a_courtyard_stays_off_a_neighbours_tab() {
        // A's courtyard lies above its top side, in the gap below the top rail
        // (y -1.5 to -0.5 mm, x 8-16 mm): clear of the rail, both boards and
        // a_top, but over B's top tab (x 15.5-18.5 mm, y -2-0 mm).
        let reach = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at 0 0 0) \
             (property \"Reference\" \"J1\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
             (fp_rect (start 8 -1.5) (end 16 -0.5) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = reach;
        let error = panelize(&spec(""), &marked, &none(), "demo").err().expect("refused");
        assert!(error.contains("J1's courtyard (F.CrtYd) reaches into tab b_top"), "{error}");
        let mut after = spec("");
        after.assemble_after_depanel = vec!["A.J1".into()];
        panelize(&after, &marked, &none(), "demo").expect("allowed when fitted after depanelization");
    }

    #[test]
    fn an_unnetted_copper_graphic_counts_as_unnetted_copper() {
        let line = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(gr_line (start 1 4) (end 3 4) (stroke (width 0.2) (type solid)) (layer \"F.Cu\") (uuid \"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\"))\n  (gr_line (start 0 0)",
            1,
        );
        let mut both = files();
        both[0].pcb = line;
        both[0].dru =
            "(version 1)\n(rule \"gap\" (constraint clearance (min 1mm)) (condition \"A.NetName == 'GND'\"))\n".into();
        let error = panelize(&spec(""), &both, &none(), "demo").err().expect("refused");
        assert!(error.contains("board A: 1 copper item(s) on no net"), "{error}");
    }

    #[test]
    fn unnetted_copper_cannot_keep_conditional_rules_to_its_board() {
        let loose = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(segment (start 1 4) (end 3 4) (width 0.25) (layer \"F.Cu\") (net 0) (uuid \"99999999-9999-9999-9999-999999999999\"))\n  (gr_line (start 0 0)",
            1,
        );
        let mut both = files();
        both[0].pcb = loose;
        panelize(&spec(""), &both, &none(), "demo").expect("no conditional rules: nothing to keep");
        both[0].dru =
            "(version 1)\n(rule \"gap\" (constraint clearance (min 1mm)) (condition \"A.NetName == 'GND'\"))\n".into();
        let error = panelize(&spec(""), &both, &none(), "demo").err().expect("refused");
        assert!(error.contains("board A: 1 copper item(s) on no net"), "{error}");
    }

    #[test]
    fn boards_closer_than_the_spacing_fail() {
        let mut close = spec("");
        close.boards[1].at = [11_000_000, 0];
        let error = panelize(&close, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("closer than the panel spacing"), "{error}");
    }
}

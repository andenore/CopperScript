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
    // Board rules are scoped by net and reference prefixes, and they reach
    // copper with neither (no net, outside footprints) on any board.
    let mut conditional = false;
    for (board, file) in spec.boards.iter().zip(files) {
        conditional |= rules::has_conditional(&file.dru, &format!("board {} design rules", board.name))?;
    }
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
        if unnetted > 0 && conditional {
            return Err(format!(
                "board {}: {unnetted} copper item(s) on no net, and the panel has conditional design rules that it \
                 can only keep to their board by net and reference; give that copper a net",
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
    let named = |f: fn(&BoardFiles) -> &String| -> Vec<(String, String)> {
        spec.boards.iter().zip(files).map(|(b, file)| (b.name.clone(), f(file).clone())).collect()
    };
    let dru = rules::merge_dru(&named(|f| &f.dru))?;
    let pro = rules::merge_pro(&named(|f| &f.pro), &format!("{file_stem}.kicad_pro"))?;
    let copper_layers: Vec<String> = composition
        .tree
        .child("layers")
        .map(|layers| {
            layers.items().iter().filter_map(|l| l.string_value(1)).filter(|name| name.ends_with(".Cu")).collect()
        })
        .unwrap_or_default();
    let edge_keepouts = frame::tab_edge_keepouts(
        &frame.tabs,
        &composition.poses,
        rules::max_edge_clearance(&pro, &dru)?,
        &copper_layers,
    );
    let mut tree = composition.tree;
    if let Node::List(items) = &mut tree {
        items.extend(frame.items);
        items.extend(edge_keepouts);
    }
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
            &spec("tab a_low { board = A; side = bottom; at = 1 mm; width = 1 mm; }"),
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
            &spec("tab a_far { board = A; side = bottom; at = 9 mm; width = 1 mm; }"),
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
        // The keep-out crosses A's top side from 1 to 8.5 mm (1.5 mm short of
        // the right side), with a hole from 2 to 8 mm through the edge: a_top
        // (3.5-6.5 mm) lies in the hole, clear of the 1 mm margins
        // (keep-clear 0-3 and 7-9.5 mm).
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
             (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 1 -1) (xy 8.5 -1) (xy 8.5 4) (xy 1 4))) (polygon (pts (xy 2 -0.5) (xy 8 -0.5) (xy 8 2) (xy 2 2))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        panelize(&spec(""), &marked, &none(), "demo").expect("a_top lies in the keep-out's hole");
        let error = panelize(
            &spec("tab a_top_l { board = A; side = top; at = 1 mm; width = 1 mm; }"),
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
    fn a_tab_keeps_the_spacing_from_boards_it_passes() {
        // B sits diagonally below-right of A, corner to corner 2 mm apart;
        // a_bottom runs from A down to the bottom rail along B's left side.
        for (at, gap) in [("8.5", "0"), ("8", "0.5")] {
            let mut passing = spec("");
            passing.boards[1].at = [10_000_000, 8_000_000];
            passing.tabs.retain(|t| t.name != "a_right" && t.name != "b_top");
            for tab in &mut passing.tabs {
                if tab.name == "a_bottom" {
                    tab.at = (at.parse::<f64>().expect("a number") * 1e6) as i64;
                }
            }
            let error = panelize(&passing, &files(), &none(), "demo").err().expect("refused");
            assert!(error.contains(&format!("tab a_bottom passes board B {gap} mm away")), "{at}: {error}");
        }
    }

    #[test]
    fn fiducials_and_tooling_holes_must_fit_the_panel() {
        // Board A alone is 10 mm wide: FID1 (x 8) and TH2 (x 7) would overlap.
        let mut narrow = spec("");
        narrow.boards.truncate(1);
        narrow.tabs.retain(|t| t.board == "A" && t.name != "a_right");
        let error = panelize(&narrow, &files()[..1], &none()[..1], "demo").err().expect("refused");
        assert!(error.contains("the panel is 10 mm wide") && error.contains("would"), "{error}");
        narrow.fiducials = 0;
        narrow.tooling_hole = None;
        panelize(&narrow, &files()[..1], &none()[..1], "demo").expect("without them it fits");
    }

    #[test]
    fn unnetted_copper_on_any_board_is_refused_when_rules_are_scoped() {
        // B has no rules of its own but a netless copper line; A has a
        // conditional rule, which would otherwise reach B's line.
        let line = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(gr_line (start 1 4) (end 3 4) (stroke (width 0.2) (type solid)) (layer \"F.Cu\") (uuid \"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\"))\n  (gr_line (start 0 0)",
            1,
        );
        let mut both = files();
        both[0].dru =
            "(version 1)\n(rule \"gap\" (constraint clearance (min 1mm)) (condition \"A.NetName == 'GND'\"))\n".into();
        both[1].pcb = line;
        let error = panelize(&spec(""), &both, &none(), "demo").err().expect("refused");
        assert!(error.contains("board B: 1 copper item(s) on no net"), "{error}");
    }

    #[test]
    fn tabs_leave_the_router_room_between_them() {
        // b_left (y 4.5-5.5) bridges the same gap as a_right (y 2-4), 0.5 mm away.
        let error = panelize(
            &spec("tab b_left { board = B; side = left; at = 5 mm; width = 1 mm; }"),
            &files(),
            &none(),
            "demo",
        )
        .err()
        .expect("refused");
        assert!(error.contains("tabs a_right and b_left are 0.5 mm apart"), "{error}");
    }

    #[test]
    fn a_tab_no_wider_than_its_holes_is_refused() {
        let mut thin = spec("");
        for tab in &mut thin.tabs {
            if tab.name == "a_right" {
                tab.width = 500_000;
            }
        }
        let error = panelize(&thin, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("tab a_right is 0.5 mm wide, no wider than its 0.5 mm mouse-bite holes"), "{error}");
    }

    #[test]
    fn a_tooling_hole_must_fit_its_rail() {
        let mut thin = spec("");
        thin.rails.top = 3_000_000;
        thin.rails.bottom = 3_000_000;
        thin.tooling_hole = Some(3_200_000);
        let error = panelize(&thin, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("tooling holes of 3.2 mm need rails of at least 3.6 mm"), "{error}");
    }

    #[test]
    fn a_keepout_reaching_a_neighbour_is_cut_at_its_board() {
        // A's keep-out runs from x 8 to 14.5 mm, into B (x 12-22): on the panel
        // it stops at A's edge, and a_right (2-4 mm) stays allowed.
        let reach = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"ko\") \
             (hatch edge 0.5) (keepout (tracks not_allowed) (vias not_allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 8 5.1) (xy 14.5 5.1) (xy 14.5 5.9) (xy 8 5.9))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = reach;
        let panel = panelize(&spec(""), &marked, &none(), "demo").expect("a_right is clear of the keep-out");
        let board = copper_cam::board::parse_board(&panel.pcb, "panel").expect("copper-cam reads the panel");
        let ko = board.zones.iter().find(|z| z.name == "ko").expect("the keep-out");
        let max_x = ko
            .outline
            .iter()
            .flat_map(|c| std::iter::once(c.start).chain(c.edges.iter().map(|e| e.end())))
            .map(|p| p.x)
            .max()
            .expect("points");
        assert!(max_x <= 10_000_000, "{max_x}");
    }

    #[test]
    fn a_tab_must_land_with_its_whole_width() {
        // B lowered to y 3.999 mm: a_right (y 2-4) meets it over 1 um only.
        let mut partial = spec("");
        partial.boards[1].at = [12_000_000, 3_999_000];
        partial.tabs.retain(|t| t.name != "b_top");
        let error = panelize(&partial, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("tab a_right: only part of its far end lands on board B"), "{error}");
    }

    #[test]
    fn tab_landings_keep_the_edge_clearance() {
        // With a 1 mm copper-to-edge clearance, every tab landing gets a 1 mm
        // keep-out inside its board: a_top's from x 3.5 to 6.5, y 0 to 1.
        let pro = PRO.replace(
            r#""design_settings": {}"#,
            r#""design_settings": {"rules": {"min_copper_edge_clearance": 1.0, "min_hole_clearance": 0.25}}"#,
        );
        let mut both = files();
        for f in &mut both {
            f.pro = pro.clone();
        }
        let panel = panelize(&spec(""), &both, &none(), "demo").expect("a panel");
        let board = copper_cam::board::parse_board(&panel.pcb, "panel").expect("copper-cam reads the panel");
        let keepouts: Vec<&copper_cam::board::Zone> =
            board.zones.iter().filter(|z| z.name.starts_with("panel/tab-edge/")).collect();
        // a_top, a_bottom, b_top near ends, a_right near and far.
        assert_eq!(keepouts.len(), 5, "{:?}", keepouts.iter().map(|z| &z.name).collect::<Vec<_>>());
        let top = keepouts.iter().find(|z| z.name == "panel/tab-edge/a_top/near").expect("a_top's");
        let points: Vec<(i64, i64)> = top.outline[0].edges.iter().map(|e| (e.end().x, e.end().y)).collect();
        let (x0, x1) = (points.iter().map(|p| p.0).min().unwrap(), points.iter().map(|p| p.0).max().unwrap());
        let (y0, y1) = (points.iter().map(|p| p.1).min().unwrap(), points.iter().map(|p| p.1).max().unwrap());
        assert_eq!((x0, x1, y0, y1), (3_500_000, 6_500_000, 0, 1_000_000));
        assert!(top.not_allowed.iter().any(|k| k == "copperpour") && top.keepout == Some(true));
        // Without an edge clearance there are none.
        let plain = panelize(&spec(""), &files(), &none(), "demo").expect("a panel");
        assert!(!plain.pcb.contains("panel/tab-edge/"));
    }

    #[test]
    fn two_boards_courtyards_must_not_meet_between_them() {
        // A's J1 reaches x 8.5-11.5, B's J2 x 10.5-13.5 (on the panel), y
        // 0.2-1.8: neither reaches the other board, but they meet in the gap.
        let body = |x: &str, name: &str| {
            BOARD.replacen(
                "(gr_line (start 0 0)",
                &format!(
                    "(footprint \"X:J\" (layer \"F.Cu\") (uuid \"66666666-6666-6666-6666-666666666666\") (at {x} 1 0) \
                     (property \"Reference\" \"{name}\" (at 0 0 0) (layer \"F.Fab\") (uuid \"77777777-7777-7777-7777-777777777777\") (effects (font (size 1 1) (thickness 0.15)))) \
                     (fp_rect (start -1.5 -0.8) (end 1.5 0.8) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\") (uuid \"88888888-8888-8888-8888-888888888888\")))\n  (gr_line (start 0 0)"
                ),
                1,
            )
        };
        let mut both = files();
        both[0].pcb = body("10", "J1");
        both[1].pcb = body("0", "J2");
        let error = panelize(&spec(""), &both, &none(), "demo").err().expect("refused");
        assert!(error.contains("J1's courtyard (F.CrtYd) meets board B's J2's courtyard (F.CrtYd)"), "{error}");
        let mut after = spec("");
        after.assemble_after_depanel = vec!["A.J1".into()];
        panelize(&after, &both, &none(), "demo").expect("one fitted after depanelization");
    }

    #[test]
    fn the_keepout_margin_reaches_round_a_corner() {
        // A keep-out opened to A's left side (x 0-2, y 0.5-3) lies 0.5 mm
        // from the top side: a tab there at 0-1 mm is within its margin.
        let keepout = BOARD.replacen(
            "(gr_line (start 0 0)",
            "(zone (net 0) (net_name \"\") (layer \"F.Cu\") (uuid \"55555555-5555-5555-5555-555555555555\") (name \"cluster/a/a-edge\") \
             (hatch edge 0.5) (keepout (tracks allowed) (vias allowed) (pads allowed) (copperpour not_allowed) (footprints allowed)) \
             (polygon (pts (xy 0 0.5) (xy 2 0.5) (xy 2 3) (xy 0 3))))\n  (gr_line (start 0 0)",
            1,
        );
        let mut marked = files();
        marked[0].pcb = keepout;
        let error =
            panelize(&spec("tab a_tl { board = A; side = top; at = 0.5 mm; width = 1 mm; }"), &marked, &none(), "demo")
                .err()
                .expect("refused");
        assert!(
            error.contains("tab a_tl on board A's top side") && error.contains("keep-out cluster/a/a-edge"),
            "{error}"
        );
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

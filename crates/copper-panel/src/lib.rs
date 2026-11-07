//! Fabrication panels: already-routed KiCad boards composed into one panel
//! board with rails, mouse-bite tabs, fiducials and tooling holes, checked
//! against each board's own intent (see docs/panelization-plan.md).

pub mod compose;
pub mod frame;
pub mod intent;
pub mod rules;
pub mod sexpr;

use std::path::Path;

use copper_cam::poly;
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
    for (board, file) in spec.boards.iter().zip(files) {
        let tree = parse(&file.pcb, &board.pcb)?;
        let rings = outline_rings(&tree, &format!("board {}", board.name))?;
        poses.push(pose_for(&rings, board.quarter_turns, (board.at[0], board.at[1])));
        local.insert(board.name.clone(), rings);
        inputs.push(BoardInput { instance: board.name.clone(), tree });
    }
    let composition = compose(&inputs, &poses)?;
    // Boards keep at least the milled gap between them.
    let names: Vec<&String> = composition.outlines.keys().collect();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let grow = |name: &str| {
                let shapes: poly::Shapes = composition.outlines[name]
                    .iter()
                    .flat_map(|ring| {
                        poly::polygon(&ring.iter().map(|(x, y)| (*x as f64, *y as f64)).collect::<Vec<_>>())
                    })
                    .collect();
                poly::offset(&shapes, spec.spacing / 2 - 1_000, false, 1_000.0)
            };
            if !poly::intersection(&grow(a), &grow(b)).is_empty() {
                return Err(format!("boards {a} and {b} are closer than the panel spacing ({} mm)", mm(spec.spacing)));
            }
        }
    }
    let frame = frame::build(spec, &local, &composition.outlines, &composition.poses)?;
    let intent_map: IndexMap<String, BoardIntent> =
        spec.boards.iter().zip(intents).map(|(b, i)| (b.name.clone(), i.clone())).collect();
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
    let report = object(vec![
        ("schema", Json::from("copperscript-panel/v0.1")),
        ("panel", Json::from(spec.name.as_str())),
        ("size_mm", json_point((x1 - x0, y1 - y0))),
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
                            ("across", Json::from(t.across.as_str())),
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
    Ok(Panel { pcb: print(&tree), pro, dru, report })
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
    fn boards_closer_than_the_spacing_fail() {
        let mut close = spec("");
        close.boards[1].at = [11_000_000, 0];
        let error = panelize(&close, &files(), &none(), "demo").err().expect("refused");
        assert!(error.contains("closer than the panel spacing"), "{error}");
    }
}

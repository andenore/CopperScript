//! Small in-memory route-board adapter. No alternate solver or repair rules.
//! The website supplies a source-owned, explicitly synthetic teaching board.
use copper_json::{Dumps, Object, Value};
use copper_lang::{compiler::compile_design_source, erc, parse};
use copper_pcb::physicalize::{Footprints, PhysicalOptions, physicalize};
use copper_place::PlannerOptions;
use copper_report::scene::{Scene, scene_from_physical};
use copper_route::flow::{RoutingPipelineOptions, run_routing_pipeline};
use copper_route::routeflow::PlacementRoutingFeedbackOptions;
use copper_route::{GlobalRouterOptions, unified_router::UnifiedRouterOptions};
use wasm_bindgen::prelude::*;

fn obj<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(Object::from_iter(entries.map(|(key, value)| (key.to_owned(), value))))
}
fn text(value: &str) -> Value {
    Value::from(value)
}
fn number(value: f64) -> Value {
    Value::Float(value)
}
fn point(value: (f64, f64)) -> Value {
    Value::Array(vec![number(value.0), number(value.1)])
}
fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|s| text(s)).collect())
}
fn points(values: &[(f64, f64)]) -> Value {
    Value::Array(values.iter().map(|p| point(*p)).collect())
}

fn scene_document(scene: Scene) -> Value {
    obj([
        ("name", text(&scene.name)),
        ("layers", strings(&scene.copper_layers)),
        ("edges", Value::Array(scene.edges.iter().map(|(a, b)| Value::Array(vec![point(*a), point(*b)])).collect())),
        (
            "pads",
            Value::Array(
                scene
                    .pads
                    .iter()
                    .map(|pad| {
                        obj([
                            ("reference", text(&pad.reference)),
                            ("pad", text(&pad.pad)),
                            ("net", text(&pad.net)),
                            ("layers", strings(&pad.layers)),
                            ("polygon", points(&pad.polygon)),
                            ("center", point(pad.center)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "tracks",
            Value::Array(
                scene
                    .tracks
                    .iter()
                    .map(|track| {
                        obj([
                            ("net", text(&track.net)),
                            ("layer", text(&track.layer)),
                            ("start", point(track.start)),
                            ("end", point(track.end)),
                            ("width", number(track.width)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "vias",
            Value::Array(
                scene
                    .vias
                    .iter()
                    .map(|via| {
                        obj([
                            ("net", text(&via.net)),
                            ("center", point(via.center)),
                            ("size", number(via.size)),
                            ("drill", number(via.drill)),
                            ("layers", strings(&via.layers)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "footprints",
            Value::Array(
                scene
                    .footprints
                    .values()
                    .map(|f| {
                        obj([
                            ("reference", text(&f.reference)),
                            ("center", point(f.center)),
                            ("rotation", number(f.rotation)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn run(source: &str) -> Result<Value, String> {
    if source.len() > 8192 {
        return Err("The routing lab accepts at most 8 KiB of source.".into());
    }
    let parsed = parse(source, "routing.copper").map_err(|e| e.to_string())?;
    if !parsed.imports.is_empty() {
        return Err("Project imports are not supported by this browser prototype.".into());
    }
    let design = compile_design_source(source, "routing.copper", true, true).map_err(|e| e.to_string())?;
    let diagnostics = erc::check(&design.electrical, true)?;
    if erc::has_errors(&diagnostics) {
        return Err("Correct the circuit's electrical errors before routing.".into());
    }
    if design.electrical.components.len() > 8 || design.electrical.nets.len() > 8 {
        return Err("The routing lab supports at most eight components and eight nets.".into());
    }
    let mut physical_options = PhysicalOptions::new(40.0, 30.0, 2, "generic").map_err(|e| e.to_string())?;
    physical_options.margin_mm = 2.0;
    let board = physicalize(&design, Footprints::Proxy, &physical_options, true, true).map_err(|e| e.to_string())?;
    if board.outline.vertices.iter().any(|p| p.x_nm < 0 || p.y_nm < 0 || p.x_nm > 40_000_000 || p.y_nm > 30_000_000)
        || board.placements.len() != design.electrical.components.len()
        || board.placements.len() > 8
        || board.nets.len() > 8
        || board.placements.iter().map(|p| board.footprints[&p.footprint].pads.len()).sum::<usize>() > 16
        || board.outline.vertices.len() > 16
        || board.copper_keepouts.len() > 8
        || board.copper_keepouts.iter().any(|k| k.vertices.len() > 16)
        || !board.mechanical_holes.is_empty()
        || !board.mechanical_slots.is_empty()
        || !board.zones.is_empty()
        || !board.polygons.is_empty()
        || !board.hard_macros.is_empty()
        || !board.via_in_pad_rules.is_empty()
        || board.net_routing_rules.iter().any(|r| r.kind != copper_pcb::board::RouteKind::General)
        || board
            .placements
            .iter()
            .any(|p| !board.placement_rules.iter().any(|r| r.reference == p.reference && r.fixed_position.is_some()))
    {
        return Err(
            "Use a small board with fixed component positions, ordinary nets and no zones, fixed copper or macros."
                .into(),
        );
    }
    let keepouts = Value::Array(
        board
            .copper_keepouts
            .iter()
            .map(|k| {
                obj([
                    ("id", text(&k.id)),
                    ("layers", Value::Array(k.layers.iter().map(|l| text(l.value())).collect())),
                    (
                        "vertices",
                        Value::Array(
                            k.vertices.iter().map(|p| point((p.x_nm as f64 / 1e6, p.y_nm as f64 / 1e6))).collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    let input_scene =
        scene_from_physical(&copper_route::state_codec::encode_board(&board)).map_err(|e| format!("{e:?}"))?;
    let options = RoutingPipelineOptions {
        placement_options: Some(PlannerOptions {
            candidate_count: 1,
            seed_count: Some(1),
            analytical_iterations: 1,
            refinement_passes: 0,
            escape_spacing_passes: 0,
            ..Default::default()
        }),
        global_options: Some(GlobalRouterOptions { maximum_iterations: 3, ..Default::default() }),
        feedback_options: Some(PlacementRoutingFeedbackOptions {
            maximum_iterations: 1,
            maximum_trials_per_iteration: 0,
            ..Default::default()
        }),
        unified_options: Some(UnifiedRouterOptions {
            direct: true,
            maximum_iterations: 5,
            maximum_expansions: 15000,
            commit_rounds: 2,
            pitch_nm: Some(250_000),
            via_cost_mm: 0.5,
            time_budget_s: None,
            fine_regions: "off".into(),
            pre_route_decouplers: false,
            bga_fanout: false,
            export_congestion: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    let result = run_routing_pipeline(board, options, None).map_err(|e| e.to_string())?;
    let output =
        scene_from_physical(&copper_route::state_codec::encode_board(result.board())).map_err(|e| format!("{e:?}"))?;
    Ok(obj([
        ("schema", text("copperscript.routing-playground/v1")),
        ("error", Value::Null),
        ("before", scene_document(input_scene)),
        ("after", scene_document(output)),
        ("keepouts", keepouts),
        ("routing", copper_json::parse(&result.detailed.to_json()).map_err(|e| e.to_string())?),
        ("drc", result.drc.document(false)),
        ("native_verified", Value::Bool(false)),
        ("qualified_release", Value::Bool(false)),
    ]))
}

#[wasm_bindgen]
pub fn route(source: &str) -> String {
    let value = run(source)
        .unwrap_or_else(|error| obj([("schema", text("copperscript.routing-playground/v1")), ("error", text(&error))]));
    Dumps::compact().dumps(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOARD: &str = include_str!("../../../site/demo/routing/board.copper");
    #[test]
    fn standard_pipeline_routes_a_small_board() {
        let result = run(BOARD).unwrap();
        assert!(!result.get("after").unwrap().get("tracks").unwrap().as_array().unwrap().is_empty());
        assert!(matches!(result.get("native_verified"), Some(Value::Bool(false))));
    }
}

//! `pcbir.editor.definitions`: definition documents (macro and
//! board_profile) and the read-only macro instances of a board.
//!
//! Definition sessions are not implemented yet; `board_macro_features` is.

use copper_json::{Value, python_str_repr as repr};
use copper_pcb::board::{CopperZone, PadReference, PhysicalBoard, PhysicalHardMacro};
use copper_physical::{CopperLayer, Point};

use crate::object;

/// Python's enum member repr, `<CopperLayer.FRONT: 'F.Cu'>`.
fn layer(layer: &CopperLayer) -> String {
    let value = layer.value();
    let name = match value {
        "F.Cu" => "FRONT".to_owned(),
        "B.Cu" => "BACK".to_owned(),
        inner => format!("INTERNAL_{}", inner.trim_start_matches("In").trim_end_matches(".Cu")),
    };
    format!("<CopperLayer.{name}: {}>", repr(value))
}

fn member(class: &str, value: &str) -> String {
    format!("<{class}.{}: {}>", value.to_uppercase(), repr(value))
}

/// `repr(tuple(items))`.
fn tuple(items: Vec<String>) -> String {
    match items.len() {
        1 => format!("({},)", items[0]),
        _ => format!("({})", items.join(", ")),
    }
}

/// `repr(dict)` of an `asdict` record.
fn record(fields: Vec<(&str, String)>) -> String {
    let items: Vec<String> = fields.into_iter().map(|(k, v)| format!("{}: {v}", repr(k))).collect();
    format!("{{{}}}", items.join(", "))
}

fn point(p: &Point) -> String {
    record(vec![("x_nm", p.x_nm.to_string()), ("y_nm", p.y_nm.to_string())])
}

fn pad(p: &PadReference) -> String {
    record(vec![("component", repr(&p.component)), ("pad", repr(&p.pad))])
}

fn optional_int(value: Option<i64>) -> String {
    value.map_or("None".to_owned(), |v| v.to_string())
}

fn boolean(value: bool) -> String {
    (if value { "True" } else { "False" }).to_owned()
}

fn ring(vertices: &[Point]) -> String {
    record(vec![("vertices", tuple(vertices.iter().map(point).collect()))])
}

fn layers(items: &[CopperLayer]) -> String {
    tuple(items.iter().map(layer).collect())
}

fn zone(z: &CopperZone) -> String {
    record(vec![
        ("id", repr(&z.id)),
        ("net", repr(&z.net)),
        ("layers", layers(&z.layers)),
        (
            "outline",
            record(vec![("outer", ring(&z.outer)), ("holes", tuple(z.holes.iter().map(|h| ring(h)).collect()))]),
        ),
        ("priority", z.priority.to_string()),
        ("clearance_nm", optional_int(z.clearance_nm)),
        ("minimum_width_nm", z.minimum_width_nm.to_string()),
        ("pad_connection", member("ZoneConnection", z.pad_connection.value())),
        (
            "thermal",
            record(vec![
                ("gap_nm", z.thermal_gap_nm.to_string()),
                ("spoke_width_nm", z.thermal_spoke_width_nm.to_string()),
                ("spoke_count", "4".to_owned()),
            ]),
        ),
        ("island_policy", member("IslandPolicy", z.island_policy.value())),
        ("minimum_island_area_nm2", optional_int(z.minimum_island_area_nm2)),
        ("fill_mode", member("ZoneFillMode", "solid")),
        ("reserve_routing", boolean(z.reserve_routing)),
        ("allow_same_net_hard_macro_overlap", boolean(z.allow_same_net_hard_macro_overlap)),
    ])
}

/// `{k: str(v) for k, v in asdict(macro).items()}`.
fn macro_parameters(m: &PhysicalHardMacro) -> Vec<(&'static str, String)> {
    vec![
        ("cluster", m.cluster.clone()),
        ("asset_sha256", m.asset_sha256.clone()),
        (
            "tracks",
            tuple(
                m.tracks
                    .iter()
                    .map(|t| {
                        record(vec![
                            ("net", repr(&t.net)),
                            ("start", point(&t.start)),
                            ("end", point(&t.end)),
                            ("width_nm", t.width_nm.to_string()),
                            ("layer", layer(&t.layer)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "vias",
            tuple(
                m.vias
                    .iter()
                    .map(|v| {
                        record(vec![
                            ("net", repr(&v.net)),
                            ("position", point(&v.position)),
                            ("size_nm", v.size_nm.to_string()),
                            ("drill_nm", v.drill_nm.to_string()),
                            ("from_layer", layer(&v.from_layer)),
                            ("to_layer", layer(&v.to_layer)),
                            ("technology", v.technology.as_deref().map_or("None".to_owned(), repr)),
                            ("finish", repr(&v.finish)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "ports",
            tuple(
                m.ports
                    .iter()
                    .map(|p| {
                        record(vec![
                            ("name", repr(&p.name)),
                            ("net", repr(&p.net)),
                            ("position", point(&p.position)),
                            ("layer", layer(&p.layer)),
                            ("pads", tuple(p.pads.iter().map(pad).collect())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "protected_regions",
            tuple(
                m.protected_regions
                    .iter()
                    .map(|k| {
                        record(vec![
                            ("id", repr(&k.id)),
                            ("layers", layers(&k.layers)),
                            ("outline", record(vec![("outer", ring(&k.vertices)), ("holes", "()".to_owned())])),
                            ("block_tracks", boolean(k.block_tracks)),
                            ("block_vias", boolean(k.block_vias)),
                            ("block_pads", boolean(k.block_pads)),
                            ("block_zones", boolean(k.block_zones)),
                            ("block_footprints", boolean(k.block_footprints)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("required_layers", layers(&m.required_layers)),
        (
            "pad_bindings",
            tuple(m.pad_bindings.iter().map(|(p, net)| record(vec![("pad", pad(p)), ("net", repr(net))])).collect()),
        ),
        ("isolated_pads", tuple(m.isolated_pads.iter().map(pad).collect())),
        ("zones", tuple(m.zones.iter().map(zone).collect())),
        (
            "plane_returns",
            tuple(
                m.plane_returns
                    .iter()
                    .map(|r| {
                        record(vec![
                            ("net", repr(&r.net)),
                            ("layers", layers(&r.layers)),
                            ("pads", tuple(r.pads.iter().map(pad).collect())),
                            (
                                "dedicated_contacts",
                                tuple(
                                    r.dedicated_contacts.iter().map(|(p, at)| tuple(vec![pad(p), point(at)])).collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "polygons",
            tuple(
                m.polygons
                    .iter()
                    .map(|p| {
                        record(vec![
                            ("id", repr(&p.id)),
                            ("net", repr(&p.net)),
                            ("layer", layer(&p.layer)),
                            ("outline", ring(&p.vertices)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "width_contracts",
            tuple(
                m.width_contracts
                    .iter()
                    .map(|c| {
                        record(vec![
                            ("track_index", c.track_index.to_string()),
                            ("minimum_width_nm", c.minimum_width_nm.to_string()),
                            ("purpose", repr(&c.purpose)),
                            ("evidence", repr(&c.evidence)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]
}

/// `board_macro_features`: read-only immutable instance contracts.
pub fn board_macro_features(board: &PhysicalBoard) -> crate::Result<Vec<Value>> {
    let text = board.metadata.get("source_physical_macros").map_or("[]", String::as_str);
    let rows = copper_json::parse(text).map_err(|e| crate::Error::Uncaught(format!("JSONDecodeError: {e}")))?;
    let mut provenance = indexmap::IndexMap::new();
    for row in rows.as_array().unwrap_or(&[]) {
        if let Some(name) = row.get("name").and_then(Value::as_str) {
            provenance.insert(name.to_owned(), row.clone());
        }
    }
    Ok(board
        .hard_macros
        .iter()
        .map(|m| {
            let parameters =
                Value::Object(macro_parameters(m).into_iter().map(|(k, v)| (k.to_owned(), Value::from(v))).collect());
            object(vec![
                ("category", Value::from("macro_instance")),
                ("kind", Value::from("contracts")),
                ("name", Value::from(m.cluster.as_str())),
                ("editable", Value::from(false)),
                ("parameters", parameters),
                (
                    "definition_path",
                    provenance.get(&m.cluster).and_then(|r| r.get("source_path")).cloned().unwrap_or(Value::Null),
                ),
            ])
        })
        .collect())
}
